#!/usr/bin/env python3
"""Resumable beta releases and byte-for-byte promotion. GitHub access is REST only."""
import argparse
import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shlex
import shutil
import subprocess
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
STORE = Path.home() / ".local/share/scrawl-release"
REPO = "kitlangton/scrawl"
SITE = "https://scrawl.kitlangton.com"
NS = "http://www.andymatuschak.org/xml-namespaces/sparkle"
PUBLIC_KEY = "mIek27lttJe8cIBqVZFhh6reRKjpTx1h9ZY9OKWPtuM="
ET.register_namespace("sparkle", NS)


def run(*args, **kwargs):
    return subprocess.check_output([str(a) for a in args], text=True, **kwargs).strip()


def api(path, method="GET", data=None):
    args = ["gh", "api", path, "--method", method]
    if data is not None:
        args += ["--input", "-"]
    result = run(*args, input=json.dumps(data) if data is not None else None)
    return json.loads(result) if result else None


def releases():
    return json.loads(run("gh", "api", f"repos/{REPO}/releases?per_page=100", "--paginate", "--slurp"))


def parse_version(value):
    match = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)(?:-beta\.([1-9]\d*))?", value)
    if not match:
        raise ValueError(f"Invalid release version: {value}")
    a, b, c, beta = match.groups()
    return (int(a), int(b), int(c), int(beta) if beta else float("inf"))


def next_beta(package, versions):
    parsed = [parse_version(v) for v in versions]
    base = parse_version(package)[:3]
    stable = [v[:3] for v in parsed if v[3] == float("inf")]
    if stable and base <= max(stable):
        a, b, c = max(stable)
        base = (a, b, c + 1)
    # Continue a later unreleased version rather than rolling back to Cargo.toml.
    base = max([base] + [v[:3] for v in parsed])
    n = max([0] + [v[3] for v in parsed if v[:3] == base and v[3] != float("inf")]) + 1
    return ".".join(map(str, base)) + f"-beta.{n}"


def feed_items(xml):
    return ET.fromstring(xml).findall("./channel/item")


def build_number(item):
    return int(item.findtext(f"{{{NS}}}version"))


def next_build(xml, manifests):
    return max([0] + [build_number(i) for i in feed_items(xml)] + [m["build"] for m in manifests]) + 1


def promoted_feed(xml, build, version):
    """Add a stable item with the same signed enclosure and build, preserving beta history."""
    root = ET.fromstring(xml)
    channel = root.find("channel")
    items = channel.findall("item")
    stable = [i for i in items if i.find(f"{{{NS}}}channel") is None]
    if any(build_number(i) > build for i in stable):
        raise ValueError("Refusing to promote behind the current stable build")
    existing = [i for i in stable if build_number(i) == build]
    beta = next(i for i in items if build_number(i) == build and i.findtext(f"{{{NS}}}channel") == "beta")
    if existing:
        if existing[0].findtext(f"{{{NS}}}shortVersionString") != version or ET.tostring(existing[0].find("enclosure")) != ET.tostring(beta.find("enclosure")):
            raise ValueError("Stable build already exists with different metadata")
        return xml
    item = copy.deepcopy(beta)
    item.remove(item.find(f"{{{NS}}}channel"))
    item.find(f"{{{NS}}}shortVersionString").text = version
    item.find("title").text = version
    channel.insert(1, item)
    ET.indent(root)
    return ET.tostring(root, encoding="unicode", xml_declaration=True)


def save(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(data, indent=2) + "\n")
    temporary.replace(path)


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def immutable_copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists():
        if digest(source) != digest(destination):
            raise ValueError(f"Refusing to overwrite different bytes: {destination}")
    else:
        temporary = destination.with_suffix(destination.suffix + ".partial")
        shutil.copy2(source, temporary)
        temporary.replace(destination)


def download(url, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".partial")
    run("curl", "--fail", "--location", "--retry", "3", "--max-time", "1200", "--output", temporary, url)
    temporary.replace(destination)


def wait_file(path):
    # skulk owns waiting; no shell sleep/poll loop and no GitHub GraphQL.
    run("skulk", "poll", "--every", "60s", "--equals", "ready", "--deadline", "2h", "--wait",
        "--", f"if test -f {shlex.quote(str(path))}; then echo ready; else echo pending; fi")
    result = json.loads(path.read_text())
    if result.get("error"):
        raise RuntimeError(result["error"])


def gui_build(manifest, folder):
    done = folder / "build-result.json"
    label = f"dev.kit.scrawl-release-{manifest['build']}"
    domain = f"gui/{os.getuid()}"
    registered = subprocess.run(["launchctl", "print", f"{domain}/{label}"], capture_output=True).returncode == 0
    if registered and not done.exists():
        wait_file(done)
    if registered:
        run("launchctl", "bootout", f"{domain}/{label}")
    if done.exists() and not json.loads(done.read_text()).get("error"):
        return  # Signing completed before an interrupted archive/upload step.
    if done.exists():
        done.unlink()
    script = folder / "build.sh"
    script.write_text(
        "#!/bin/bash\nset -uo pipefail\n"
        'export PATH="$HOME/bin:$HOME/.cargo/bin:$HOME/.bun/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"\n'
        f"cd {shlex.quote(str(ROOT))}\n"
        f"export SCRAWL_VERSION={manifest['version']} SCRAWL_BUILD_NUMBER={manifest['build']}\n"
        f"export SCRAWL_SPARKLE_DIR={shlex.quote(str(STORE / 'sparkle-2.9.4'))}\n"
        f"export SCRAWL_DIST_DIR={shlex.quote(str(folder / 'artifacts'))}\n"
        "export SCRAWL_UPDATE_CHANNEL=beta SCRAWL_SKIP_APPCAST=1 SCRAWL_STAGE_SITE=0\n"
        f"bash scripts/release-app.sh > {shlex.quote(str(folder / 'build.log'))} 2>&1\n"
        'status=$?\n'
        f"if [ \"$status\" = 0 ]; then echo '{{}}'; else echo '{{\"error\":\"Build failed; see build.log\"}}'; fi > {shlex.quote(str(done))}.tmp\n"
        f"mv {shlex.quote(str(done))}.tmp {shlex.quote(str(done))}\n"
    )
    plist = folder / "build.plist"
    plist.write_bytes(plistlib.dumps({"Label": label, "ProgramArguments": ["/bin/bash", str(script)], "RunAtLoad": True}))
    run("launchctl", "bootstrap", domain, plist)
    try:
        wait_file(done)
    finally:
        run("launchctl", "bootout", f"{domain}/{label}")


def appcast(manifest, channel):
    script = STORE / "appcast.sh"
    backup = STORE / "appcast-before-channels.sh"
    if script.exists() and not backup.exists():
        shutil.copy2(script, backup)
    shutil.copy2(ROOT / "scripts/appcast-job.sh", script)
    script.chmod(0o755)
    shutil.copy2(Path(__file__), STORE / "release-channel.py")
    result = STORE / f"appcast-result-{uuid.uuid4().hex}.json"
    save(STORE / "appcast-request.json", {"manifest": manifest, "channel": channel, "result": str(result)})
    run("launchctl", "kickstart", f"gui/{os.getuid()}/dev.kit.scrawl-appcast")
    wait_file(result)


def appcast_job():
    request = json.loads((STORE / "appcast-request.json").read_text())
    try:
        m = request["manifest"]
        tool = STORE / "sparkle-2.9.4/bin/generate_appcast"
        if "--channel" not in run(tool, "--help"):
            raise RuntimeError("Sparkle generate_appcast lacks --channel")
        if run(tool.parent / "generate_keys", "-p") != PUBLIC_KEY:
            raise RuntimeError("Sparkle signing key does not match the shipped app's SUPublicEDKey")
        updates = STORE / "updates"
        feed = updates / "appcast.xml"
        if request["channel"] == "stable":
            feed.write_text(promoted_feed(feed.read_text(), m["build"], m["version"].split("-beta.")[0]))
        # Restrict insertion to this build: historical zips never become beta by accident.
        # maximum-versions=0 preserves every old archive and feed item for future deltas.
        run(tool, "--versions", m["build"], "--channel", "beta", "--maximum-versions", "0",
            "--download-url-prefix", SITE + "/releases/", updates)
        expected_channel = "beta" if request["channel"] == "beta" else None
        item = next(i for i in feed_items(feed.read_text()) if build_number(i) == m["build"]
                    and i.findtext(f"{{{NS}}}channel") == expected_channel)
        enclosure = item.find("enclosure")
        if not enclosure.get(f"{{{NS}}}edSignature") or not enclosure.get("url", "").endswith(m["zip"]):
            raise RuntimeError("Generated appcast does not contain the expected signed archive")
        run(tool.parent / "sign_update", "--verify", updates / m["zip"], enclosure.get(f"{{{NS}}}edSignature"))
        save(Path(request["result"]), {})
    except Exception as error:
        save(Path(request["result"]), {"error": str(error)})
        raise


def ensure_release(tag, commit, stable):
    # Fetch refs via REST rather than treating authentication/network errors as 'not found'.
    refs = api(f"repos/{REPO}/git/matching-refs/tags/{tag}")
    ref = next((r for r in refs if r["ref"] == f"refs/tags/{tag}"), None)
    if ref and ref["object"]["sha"] != commit:
        raise ValueError(f"Tag {tag} already points to another commit")
    if not ref:
        api(f"repos/{REPO}/git/refs", "POST", {"ref": f"refs/tags/{tag}", "sha": commit})
    existing = next((r for page in releases() for r in page if r["tag_name"] == tag), None)
    if existing:
        if existing["prerelease"] != (not stable) or existing["draft"]:
            raise ValueError(f"Release {tag} exists with a different publication state")
        return existing
    return api(f"repos/{REPO}/releases", "POST", {
        "tag_name": tag, "name": f"Scrawl {tag[1:]}", "prerelease": not stable,
        "make_latest": "false", "body": "Promoted from the tested beta, with identical signed artifacts." if stable else "Beta build. Report problems with the beta label.",
    })


def upload(release, path, name=None):
    name = name or path.name
    assets = api(f"repos/{REPO}/releases/{release['id']}/assets")
    existing = next((a for a in assets if a["name"] == name), None)
    if existing:
        if existing.get("digest") != "sha256:" + digest(path):
            raise ValueError(f"Published asset differs: {name}")
        return
    run("gh", "api", "--method", "POST", "-H", "Content-Type: application/octet-stream",
        f"https://uploads.github.com/repos/{REPO}/releases/{release['id']}/assets?name={name}",
        "--input", path)


def deploy(manifest, folder, stable):
    public = ROOT / "site/public"
    assets = public / "releases"
    assets.mkdir(exist_ok=True)
    feed = STORE / "updates/appcast.xml"
    # Hydrate ALL published assets, including prior stable downloads, before deploying a new
    # Workers Assets snapshot. A fresh worktree must not erase historical downloads.
    for page in releases():
        for release in page:
            if release["draft"]:
                continue
            for asset in release["assets"]:
                target = assets / asset["name"]
                if not target.exists():
                    download(asset["browser_download_url"], target)
                if asset.get("digest") and "sha256:" + digest(target) != asset["digest"]:
                    raise ValueError(f"Asset checksum differs: {target}")
    for enclosure in ET.fromstring(feed.read_text()).iter("enclosure"):
        url = enclosure.get("url")
        name = url.rsplit("/", 1)[1]
        target = assets / name
        source = STORE / "updates" / name
        if source.exists():
            immutable_copy(source, target)
        elif not target.exists():
            download(url, target)
    shutil.copy2(feed, public / "appcast.xml")
    latest = max((i for i in feed_items(feed.read_text()) if i.find(f"{{{NS}}}channel") is None), key=build_number)
    version = latest.findtext(f"{{{NS}}}shortVersionString")
    for suffix in ["arm64.dmg", "arm64.zip", "windows-x64.zip"]:
        name = f"Scrawl-{version}-{suffix}"
        target = assets / name
        if not target.exists():
            download(f"{SITE}/releases/{name}", target)
        shutil.copy2(target, assets / f"Scrawl-latest-{suffix}")
    # Stay on Pro with its hydrated (gitignored) downloads and Wrangler login.
    run("skulk", "run", "--slot", "heavy", "--",
        "bash", "-c", "cd site && bun run build && bunx wrangler@latest deploy", cwd=ROOT)


def cask_pr(manifest, stable):
    tap = "kitlangton/homebrew-tap"
    token = "scrawl" if stable else "scrawl@beta"
    version = manifest["version"].split("-beta.")[0] if stable else manifest["version"]
    path = f"Casks/{token}.rb"
    branch = f"scrawl-{'stable' if stable else 'beta'}-{manifest['build']}"
    if not stable and not any(f["name"] == "scrawl@beta.rb" for f in api(f"repos/{tap}/contents/Casks")):
        # Update the staged first-beta PR rather than leaving it pinned to its bootstrap DMG.
        branch = "scrawl-beta-channel"
    refs = api(f"repos/{tap}/git/matching-refs/heads/{branch}")
    if not any(r["ref"] == f"refs/heads/{branch}" for r in refs):
        main = api(f"repos/{tap}/git/ref/heads/main")
        api(f"repos/{tap}/git/refs", "POST", {"ref": f"refs/heads/{branch}", "sha": main["object"]["sha"]})
    import base64
    files = api(f"repos/{tap}/contents/Casks?ref={branch}")
    exists = any(f["name"] == token + ".rb" for f in files)
    source = api(f"repos/{tap}/contents/{path if exists else 'Casks/scrawl.rb'}?ref={branch}")
    text = base64.b64decode(source["content"]).decode()
    if not exists:
        text = text.replace('cask "scrawl"', 'cask "scrawl@beta"')
        text = text.replace('  name "Scrawl"', '  name "Scrawl Beta"\n  conflicts_with cask: "scrawl"')
    # Homebrew's Sparkle strategy does not filter prerelease channels itself.
    predicate = 'item.channel.nil?' if stable else 'item.channel == "beta"'
    livecheck = ('strategy :sparkle do |items|\n'
                 f'      items.select {{ |item| {predicate} }}.map(&:short_version)\n'
                 '    end')
    text = re.sub(r'strategy :sparkle, &:short_version', lambda _: livecheck, text)
    if not stable:
        text = text.replace("item.channel.nil?", 'item.channel == "beta"')
    text = re.sub(r'  version "[^"]+"', f'  version "{version}"', text)
    text = re.sub(r'  sha256 "[^"]+"', f'  sha256 "{manifest["sha256"][manifest["dmg"]]}"', text)
    if not exists or base64.b64decode(source["content"]).decode() != text:
        api(f"repos/{tap}/contents/{path}", "PUT", {"message": f"Update {token} to {version}", "branch": branch,
            **({"sha": source["sha"]} if exists else {}), "content": base64.b64encode(text.encode()).decode()})
    prs = api(f"repos/{tap}/pulls?head=kitlangton:{branch}&state=all")
    body = (f"## What\nUpdate {token} to {version}.\n\n## How\nUse the notarized release DMG and its SHA-256."
            "\n\n## Risk\nApp update only.\n\n## Proof\nArtifact hash recorded by the release script.")
    if not prs:
        pr = api(f"repos/{tap}/pulls", "POST", {"title": f"{token} {version}", "head": branch, "base": "main",
            "body": body})
        print(pr["html_url"])
    elif prs[0]["state"] == "open":
        api(f"repos/{tap}/pulls/{prs[0]['number']}", "PATCH", {"title": f"{token} {version}", "body": body})
        print(prs[0]["html_url"])


def execute_release(args):
    xml = run("curl", "--fail", "--silent", "--show-error", "--location", "--max-time", "30", SITE + "/appcast.xml")
    manifests = [json.loads(p.read_text()) for p in (STORE / "builds").glob("*/release.json")]
    commit = run("git", "rev-parse", "HEAD", cwd=ROOT)
    if args.command == "promote":
        manifest = next((m for m in manifests if m["build"] == args.build), None)
        if not manifest:
            raise ValueError(f"No archived beta build {args.build} on this host")
        promoted_feed(xml, args.build, manifest["version"].split("-beta.")[0])
    else:
        manifest = next((m for m in manifests if m["commit"] == commit), None)
        if not manifest:
            tags = json.loads(run("gh", "api", f"repos/{REPO}/tags?per_page=100", "--paginate", "--slurp"))
            versions = [t["name"] for page in tags for t in page if re.fullmatch(r"v\d+\.\d+\.\d+(?:-beta\.[1-9]\d*)?", t["name"])]
            versions += [m["version"] for m in manifests]
            package = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)[1]
            version = next_beta(package, versions)
            manifest = {"version": version, "build": next_build(xml, manifests), "commit": commit,
                        "dmg": f"Scrawl-{version}-arm64.dmg", "zip": f"Scrawl-{version}-arm64.zip"}
    stable = args.command == "promote"
    folder = STORE / "builds" / str(manifest["build"])
    print(json.dumps({"action": args.command, **manifest, "archive": str(folder)}, indent=2))
    if args.dry_run:
        print("Would " + ("verify archived hashes; publish identical artifacts as stable; " if stable else
              "reserve version/build; build, sign and notarize in a GUI job; publish a GitHub prerelease; ") +
              "run Pro's appcast job; deploy the site; open the cask PR.")
        return
    if run("hostname").split(".")[0].lower() != "kits-shiny-computer":
        raise RuntimeError("Run releases on Pro (the release host), from a clean checkout of merged main")
    changes = run("git", "status", "--porcelain", cwd=ROOT).splitlines()
    # The generated feed is the only tracked release output; allow it on a retry only when
    # it exactly matches the durable job output.
    generated = ROOT / "site/public/appcast.xml"
    archived_feed = STORE / "updates/appcast.xml"
    if generated.exists() and archived_feed.exists() and generated.read_bytes() == archived_feed.read_bytes():
        # Reject any changes other than the known generated file.
        changes = [line for line in run("git", "status", "--porcelain", cwd=ROOT).splitlines()
                   if not line.endswith(" site/public/appcast.xml")]
    if changes:
        raise RuntimeError("Release checkout must be clean")
    if not stable:
        run("git", "fetch", "origin", "main", cwd=ROOT)
        run("git", "merge-base", "--is-ancestor", commit, "origin/main", cwd=ROOT)
    if not archived_feed.exists():
        archived_feed.parent.mkdir(parents=True, exist_ok=True)
        archived_feed.write_text(xml)
    path = folder / "release.json"
    save(path, manifest)  # Reserve globally before any build starts; failed builds keep their number.
    if "sha256" not in manifest:
        if stable:
            raise ValueError("Beta artifacts were never completed")
        gui_build(manifest, folder)
        for name in [manifest["dmg"], manifest["zip"]]:
            immutable_copy(folder / "artifacts" / name, folder / name)
        manifest["sha256"] = {name: digest(folder / name) for name in [manifest["dmg"], manifest["zip"]]}
        save(path, manifest)
    for name, sha in manifest["sha256"].items():
        if digest(folder / name) != sha:
            raise ValueError(f"Archived artifact changed: {name}")
    version = manifest["version"].split("-beta.")[0] if stable else manifest["version"]
    release = ensure_release("v" + version, manifest["commit"], stable)
    for name in [manifest["dmg"], manifest["zip"]]:
        upload(release, folder / name, name.replace(manifest["version"], version))
    if stable:
        beta = api(f"repos/{REPO}/releases/tags/v{manifest['version']}")
        windows = f"Scrawl-{manifest['version']}-windows-x64.zip"
        if not any(a["name"] == windows for a in beta["assets"]):
            raise ValueError("Wait for the beta Windows release workflow before promotion")
        for asset in beta["assets"]:
            if ("windows" in asset["name"] or "linux" in asset["name"]) and not asset["name"].endswith(".sha256"):
                target = folder / asset["name"]
                download(asset["browser_download_url"], target)
                if asset.get("digest") != "sha256:" + digest(target):
                    raise ValueError("Platform artifact digest mismatch")
                upload(release, target, asset["name"].replace(manifest["version"], version))
    immutable_copy(folder / manifest["zip"], STORE / "updates" / manifest["zip"])
    appcast(manifest, "stable" if stable else "beta")
    deploy(manifest, folder, stable)
    if stable:
        api(f"repos/{REPO}/releases/{release['id']}", "PATCH", {"prerelease": False, "make_latest": "true"})
    cask_pr(manifest, stable)
    print(f"Published {version}, build {manifest['build']}. Cask PR awaits Kit's merge.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ["beta", "promote"]:
        child = sub.add_parser(command)
        child.add_argument("--dry-run", action="store_true")
        if command == "promote":
            child.add_argument("build", type=int)
    sub.add_parser("appcast-job")
    args = parser.parse_args()
    if args.command == "appcast-job":
        return appcast_job()
    if args.dry_run:
        return execute_release(args)
    STORE.mkdir(parents=True, exist_ok=True)
    with (STORE / "release.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        execute_release(args)


if __name__ == "__main__":
    main()
