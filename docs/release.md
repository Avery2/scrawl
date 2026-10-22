# Releasing Scrawl

Merge → cut a beta → Kit uses it → promote that build. Beta is opt-in; the site, public download
aliases and agent files advertise stable only. Windows and Linux betas are GitHub prereleases,
installed manually. They do not use Sparkle channels.

## Cut a beta

Use a clean checkout of merged `main` on **Pro**, the release host. These scripts do not pull or
switch your checkout. Signing runs in a one-off GUI-session launchd job so it can use the login
keychain; Cargo builds go through HiveMind. The script waits for jobs with skulk.

```sh
./scripts/release-beta.sh --dry-run
./scripts/release-beta.sh
```

The script chooses the next `-beta.N` (for example `0.3.14-beta.2`) from GitHub tags and the durable
release records. It reserves a build number greater than every published feed item and every
reserved build, including failed builds. Re-running at the same commit resumes the same version
and build. Change the commit to cut another beta. `Cargo.toml` and `Cargo.lock` need no beta bump.

It builds, Developer ID signs, notarizes and staples the DMG and Sparkle zip with `release-app.sh`.
Missing signing or notarization credentials fail the release. The signed files and SHA-256s are
archived under `~/.local/share/scrawl-release/builds/<build>/release.json`; once archived, retries
reuse those exact bytes and reject any changed file. Keep that directory backed up.

The script creates `v<version>` and a GitHub **prerelease**, uploads the Mac artifacts, and the tag
starts the Windows packaging workflow. The Windows executable takes its identity from the tag;
workflow retries do not overwrite uploaded assets. Linux packaging, when available, must attach
its beta archive to that same prerelease. Never mark a beta latest.

The script adds the zip to Pro's existing `~/.local/share/scrawl-release/updates/`, then uses
`dev.kit.scrawl-appcast` to generate the signed item with Sparkle **2.9.4**:

```sh
generate_appcast --versions <build> --channel beta --maximum-versions 0 \
  --download-url-prefix https://scrawl.kitlangton.com/releases/ <updates>
```

The script checks the tool's `--help` for `--channel`. Only the specified build gets a new beta
item, with `<sparkle:channel>beta</sparkle:channel>`. Historical entries and archives are retained
for delta generation. The job reads the signing key directly from its login keychain. The first
run backs up the old `appcast.sh`, installs `scripts/appcast-job.sh` and its Python helper, and
sends the job a request in `appcast-request.json`. Its log stays at `appcast.log`.

The site deploy runs on Pro with its existing Wrangler login, through skulk's heavy slot. It first
hydrates prior GitHub downloads and feed enclosures so a fresh checkout cannot remove old assets.
Beta publication leaves `Scrawl-latest-*` pointing to stable. A Homebrew `scrawl@beta` update PR is
opened through REST. Kit merges cask PRs.

Wait for the beta Windows workflow before promotion. Inspect its fresh-profile screenshot and
test the beta on the target machines. For macOS, check startup, permissions, capture, paste and
update checks, including opting in and back out. Test the changed feature on real captures.

## Promote the tested build

```sh
./scripts/promote.sh 18 --dry-run
./scripts/promote.sh 18
```

Use the **build number**, not the beta sequence. The script verifies the archived hashes, creates
the stable version tag on the beta's commit, and uploads byte-identical artifacts under stable
filenames. Windows and Linux archives are copied from the beta prerelease, never rebuilt. Stable
tags do not trigger a Windows rebuild. Promotion requires the Windows beta archive to exist.

The feed gains a stable item without `sparkle:channel`, using the same build, enclosure, signature
and deltas as the beta. Sparkle's job runs again after the metadata change. The public download
aliases advance, the site deploys, GitHub marks the stable release latest, and a stable Homebrew
cask bump PR opens. Re-running is safe; promoting behind a newer stable build is refused.

In the promotion's appcast PR, change `"status": "main"` entries in `site/src/features.json` to
`"stable@<version>"` for features present in the promoted commit. Keep later main-only work as
`main`, and experiments behind their flags. Cutting a beta never marks features stable. The map's
release version, like the download buttons and agent files, ignores beta appcast items.

**The app's signed identity still says `0.3.14-beta.2`.** Changing its Info.plist would change the
tested artifact. The stable release listing and appcast display `0.3.14`, but About and issue
reports retain the original beta identity and build. Beta users already have those exact bytes
and receive no redundant update. Users who turn beta off keep their installed build until a
newer stable build appears; they never downgrade. The opt-in is `"updateChannel": "beta"` in
`~/.config/scrawl/config.json`; absent, null or unknown values mean stable.

## Recovery and checks

Both commands accept `--dry-run`, which reads the live feed and REST metadata but writes nothing,
launches no jobs and publishes nothing. Promotion dry-runs need the build archive on Pro.
Only one release can run at a time (`release.lock`). A retry permits the generated site appcast
to be dirty only if it matches the durable job output. Commit the resulting
`site/public/appcast.xml` in an appcast PR after publication.

Never delete or replace `~/.local/share/scrawl-release/updates/`; never move Sparkle's tools or
the release archive into `target/` or `/tmp`. `setup-sparkle.sh` verifies the pinned download's
checksum. `release-app.sh` is the low-level packager; use the channel scripts for published
releases so all beta and stable builds share one monotonic sequence.

After deploying, check the live appcast and download hashes, validate the Sparkle signature and
run `spctl` against the downloaded DMG and app. Check that the site's Mac and Windows buttons,
`llms.txt`, and feature map show stable. Check the Windows zip contains `scrawl.exe`, transcription
and CPU-backend DLLs, the Visual C++ runtime DLLs, README and LICENSE (see [windows.md](windows.md)).

Pure release checks:

```sh
python3 -m unittest discover -s scripts -p test_release_channel.py
bun test site/tests/appcast.test.ts
hivemind run --kind cargo -- cargo +stable test settings::tests::toggles_and_popups_write_their_keys
hivemind run --kind cargo -- cargo +stable test updates::tests
```
