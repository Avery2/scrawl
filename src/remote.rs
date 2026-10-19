//! Remote machines: copying captures to SSH hosts where agents run, and letting them zoom.
//!
//! - `"remotes": [{"host", "copy", "zoom"}]` in `~/.config/scrawl/config.json` (plus a per-host
//!   `token` and the host's `home` once zoom is on). Hosts are `Host` names from `~/.ssh/config`.
//! - Copying: after each capture (and `scrawl remote push`), the contact sheet, frames,
//!   `summary.md` and `scrawl.json` go to `<host>:~/.scrawl/captures/<bundle>/` over the user's own
//!   `ssh` (tar, so the remote needs nothing but `tar`). `BatchMode=yes` makes a host that wants a
//!   password fail at once, shown as "needs key-based login".
//! - Same path on both sides: `~/.scrawl/captures` on the Mac links to the real captures folder,
//!   and while any remote is set up, the summary and the pasted text name
//!   `~/.scrawl/captures/<bundle>`, which opens here and on every host.
//! - Zoom (opt-in per host): a `RemoteForward` of `<home>/.scrawl/scrawl.sock` to the ingest port,
//!   in a marked block in `~/.ssh/config`, plus `~/.scrawl/bin/scrawl` (`remote_scrawl.sh`) and its
//!   key on the host. The script posts to `/zoom` on the ingest server, which runs `zoom::zoom`.
//!
//! `SCRAWL_SSH_CONFIG` replaces `~/.ssh/config` (read, written, and passed to `ssh -F`) and
//! `SCRAWL_LINK_ROOT` replaces the Mac's `~/.scrawl`, so tests never touch the real ones.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::capture::scrawl_log;

/// Where captures live on every host, and where the Mac links to its own.
pub const REMOTE_CAPTURES: &str = ".scrawl/captures";
pub const REMOTE_SCRIPT: &str = ".scrawl/bin/scrawl";
pub const REMOTE_TOKEN: &str = ".scrawl/token";
pub const REMOTE_SOCKET: &str = ".scrawl/scrawl.sock";
/// The script installed on a host for zoom.
pub const SCRIPT: &str = include_str!("remote_scrawl.sh");
/// What a copy carries: everything an agent reads. Video and audio stay on the Mac.
pub const COPIED: [&str; 4] = ["contact-sheet.png", "frames", "summary.md", "scrawl.json"];

/// Removes the host's zoom socket if nothing listens on it. sshd leaves the file behind when a
/// connection ends (unless its own config has `StreamLocalBindUnlink yes`), and a leftover file
/// stops the next connection's forward. A live one (an open connection) answers and is kept.
const CLEAR_DEAD_SOCKET: &str = r#"s="$HOME"/.scrawl/scrawl.sock; if [ -S "$s" ] && command -v curl >/dev/null; then curl -s -o /dev/null --max-time 3 --unix-socket "$s" http://scrawl/ping; [ $? -ne 7 ] || rm -f "$s"; fi; true"#;

pub const BLOCK_BEGIN: &str = "# >>> scrawl (remove to undo) >>>";
pub const BLOCK_END: &str = "# <<< scrawl <<<";

// MARK: Config

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub host: String,
    #[serde(default = "yes")]
    pub copy: bool,
    #[serde(default)]
    pub zoom: bool,
    /// The key `~/.scrawl/token` on the host sends with `/zoom`. Never logged or shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `$HOME` on the host, for the forwarded socket's absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

fn yes() -> bool {
    true
}

/// An SSH host name safe to pass to `ssh` as one argument and to write into `~/.ssh/config`.
pub fn safe_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
}

/// A remote `$HOME` safe to put in `~/.ssh/config` unquoted.
fn safe_home(home: &str) -> bool {
    home.starts_with('/')
        && home
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./@".contains(&b))
}

pub fn remotes_in(path: &Path) -> Vec<Remote> {
    let config: serde_json::Value = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    config
        .get("remotes")
        .and_then(|r| serde_json::from_value::<Vec<Remote>>(r.clone()).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| safe_host(&r.host))
        .collect()
}

pub fn remotes() -> Vec<Remote> {
    if cfg!(test) {
        return Vec::new();
    }
    remotes_in(&crate::handoff::default_config_path())
}

pub fn save_remotes_in(path: &Path, remotes: &[Remote]) -> Result<()> {
    crate::handoff::set_config_value_in(path, "remotes", serde_json::to_value(remotes)?)
}

/// Changes one host's entry in `path` (adding it if `add`), keeping the rest.
pub fn update_remote_in(
    path: &Path,
    host: &str,
    add: bool,
    change: impl FnOnce(&mut Remote),
) -> Result<()> {
    let mut list = remotes_in(path);
    match list.iter_mut().find(|r| r.host == host) {
        Some(remote) => change(remote),
        None if add => {
            ensure!(safe_host(host), "“{host}” isn't a host name Scrawl can use");
            let mut remote = Remote {
                host: host.to_owned(),
                copy: true,
                ..Remote::default()
            };
            change(&mut remote);
            list.push(remote);
        }
        None => bail!("{host} isn't one of Scrawl's remote machines"),
    }
    save_remotes_in(path, &list)
}

pub fn remove_remote_in(path: &Path, host: &str) -> Result<()> {
    let list: Vec<Remote> = remotes_in(path)
        .into_iter()
        .filter(|r| r.host != host)
        .collect();
    save_remotes_in(path, &list)
}

// MARK: ~/.ssh/config

pub fn ssh_config_path() -> PathBuf {
    std::env::var_os("SCRAWL_SSH_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".ssh/config"))
}

/// The config path as shown to people: `~/.ssh/config` unless a test replaced it.
pub fn ssh_config_label() -> String {
    if std::env::var_os("SCRAWL_SSH_CONFIG").is_some() {
        ssh_config_path().display().to_string()
    } else {
        "~/.ssh/config".to_owned()
    }
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The keyword and arguments of one `ssh_config` line (`Host a b`, `Host=a`), without comments.
fn keyword(line: &str) -> Option<(String, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let split = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (key, rest) = line.split_at(split);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    Some((key.to_ascii_lowercase(), rest))
}

/// The concrete `Host` names in an `ssh_config`, in order, skipping patterns (`*`, `?`, `!`) and
/// the hosts inside Scrawl's own block. `Include`d files aren't followed.
pub fn parse_hosts(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        match line.trim() {
            BLOCK_BEGIN => inside = true,
            BLOCK_END => inside = false,
            _ => {}
        }
        if inside {
            continue;
        }
        let Some((key, rest)) = keyword(line) else {
            continue;
        };
        if key != "host" {
            continue;
        }
        for pattern in rest.split_whitespace() {
            let pattern = pattern.trim_matches('"');
            if pattern.contains(['*', '?', '!']) || !safe_host(pattern) {
                continue;
            }
            if !hosts.iter().any(|h| h == pattern) {
                hosts.push(pattern.to_owned());
            }
        }
    }
    hosts
}

pub fn suggested_hosts() -> Vec<String> {
    parse_hosts(&fs::read_to_string(ssh_config_path()).unwrap_or_default())
}

/// One forward: the host's socket to the Mac's ingest port.
pub fn forward_line(home: &str, port: u16) -> String {
    format!("RemoteForward {home}/{REMOTE_SOCKET} 127.0.0.1:{port}")
}

/// Scrawl's block for hosts with zoom, as `(host, home)`. Empty when there are none.
pub fn forward_block(hosts: &[(String, String)], port: u16) -> String {
    if hosts.is_empty() {
        return String::new();
    }
    let mut block = format!("{BLOCK_BEGIN}\n");
    for (host, home) in hosts {
        block.push_str(&format!("Host {host}\n    {}\n", forward_line(home, port)));
    }
    block.push_str(BLOCK_END);
    block.push('\n');
    block
}

/// `text` without Scrawl's block: the block's whole lines are cut and nothing else changes.
pub fn without_block(text: &str) -> String {
    let Some(start) = find_line(text, BLOCK_BEGIN, 0) else {
        return text.to_owned();
    };
    let Some(end_line) = find_line(text, BLOCK_END, start) else {
        return text.to_owned();
    };
    let end = text[end_line..]
        .find('\n')
        .map_or(text.len(), |i| end_line + i + 1);
    format!("{}{}", &text[..start], &text[end..])
}

/// The byte offset where a line equal to `line` (ignoring surrounding spaces) starts.
fn find_line(text: &str, line: &str, from: usize) -> Option<usize> {
    let mut offset = from;
    for chunk in text[from..].split_inclusive('\n') {
        if chunk.trim() == line {
            return Some(offset);
        }
        offset += chunk.len();
    }
    None
}

/// `text` with Scrawl's block replaced by `block` (none when empty). A new block goes just
/// before the first `Host` or `Match` line, so options above it stay global and the user's own
/// blocks are untouched; with no such line it goes at the end. Removing it again
/// ([`without_block`]) gives back the original bytes, except that a file with no `Host` line and
/// no final newline keeps the newline added before the block.
pub fn with_block(text: &str, block: &str) -> String {
    let base = without_block(text);
    if block.is_empty() {
        return base;
    }
    let mut offset = 0;
    for chunk in base.split_inclusive('\n') {
        if matches!(keyword(chunk), Some((ref key, _)) if key == "host" || key == "match") {
            return format!("{}{block}{}", &base[..offset], &base[offset..]);
        }
        offset += chunk.len();
    }
    if base.is_empty() || base.ends_with('\n') {
        format!("{base}{block}")
    } else {
        format!("{base}\n{block}")
    }
}

/// The port the forward points at: the app's ingest port, or `SCRAWL_INGEST_PORT` for a
/// `scrawl serve --port` under test.
pub fn ingest_port() -> u16 {
    std::env::var("SCRAWL_INGEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(crate::phone::DEFAULT_INGEST_PORT)
}

/// The block for every host in `remotes` with zoom on and a known home.
pub fn block_for(remotes: &[Remote]) -> String {
    let hosts: Vec<(String, String)> = remotes
        .iter()
        .filter(|r| r.zoom)
        .filter_map(|r| Some((r.host.clone(), r.home.clone().filter(|h| safe_home(h))?)))
        .collect();
    forward_block(&hosts, ingest_port())
}

/// Writes Scrawl's block into the ssh config, through a symlink if it is one, keeping its mode.
pub fn write_block(block: &str) -> Result<()> {
    let path = ssh_config_path();
    let path = fs::canonicalize(&path).unwrap_or(path);
    let before = fs::read_to_string(&path).unwrap_or_default();
    let after = with_block(&before, block);
    if after == before {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("scrawl-tmp");
    fs::write(&tmp, &after)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).map_or(0o600, |m| m.permissions().mode() & 0o777);
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    }
    fs::rename(&tmp, &path).with_context(|| format!("Could not write {}", path.display()))
}

// MARK: ssh

/// The user's `ssh`, failing fast instead of asking for a password, and never opening the
/// forwards in their config (Scrawl's own connections must not take the zoom socket).
pub fn ssh(host: &str) -> Command {
    let mut command = Command::new("/usr/bin/ssh");
    if let Some(config) = std::env::var_os("SCRAWL_SSH_CONFIG") {
        command.arg("-F").arg(config);
    }
    command.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ClearAllForwardings=yes",
        "--",
        host,
    ]);
    command
}

/// A command for the host's login shell that runs `script` in `/bin/sh`, whatever that shell is
/// (zsh aborts on an unmatched glob, fish has its own syntax). The POSIX `'\''` quoting also
/// reads correctly in fish.
pub fn posix(script: &str) -> String {
    format!("/bin/sh -c '{}'", script.replace('\'', "'\\''"))
}

/// How a host answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state", content = "detail")]
pub enum Reach {
    Reachable,
    /// It wants a password or a key it wasn't given.
    NeedsKey,
    /// Its host key isn't in `known_hosts` yet, or changed.
    HostKey,
    Unreachable(String),
}

/// Reads `ssh`'s exit and error output. Exit 255 is ssh's own failure; anything else came from
/// the remote command, so the host answered.
pub fn classify(code: Option<i32>, stderr: &str) -> Reach {
    if code != Some(255) {
        return Reach::Reachable;
    }
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("permission denied") || lower.contains("too many authentication failures") {
        Reach::NeedsKey
    } else if lower.contains("host key verification failed")
        || lower.contains("remote host identification has changed")
    {
        Reach::HostKey
    } else {
        let line = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty() && !l.starts_with("Warning:"))
            .unwrap_or("no answer")
            .trim_start_matches("ssh: ")
            .to_owned();
        Reach::Unreachable(line)
    }
}

/// What the Add sheet's Test step learns: the host's home and system, and what's missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Probe {
    pub home: String,
    pub system: String,
    pub curl: bool,
}

pub fn probe(host: &str) -> std::result::Result<Probe, Reach> {
    let output = ssh(host)
        .arg(posix(&format!(
            r#"printf '%s\n' "$HOME"; uname -s; command -v curl >/dev/null && echo curl; command -v tar >/dev/null && echo tar; {CLEAR_DEAD_SOCKET}"#
        )))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Reach::Unreachable(format!("couldn't run ssh: {e}")))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    match classify(output.status.code(), &stderr) {
        Reach::Reachable if output.status.success() => {}
        Reach::Reachable => {
            return Err(Reach::Unreachable(format!(
                "the host answered but the check failed: {}",
                stderr.trim()
            )));
        }
        other => return Err(other),
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines().map(str::trim);
    let home = lines.next().unwrap_or_default().to_owned();
    let system = lines.next().unwrap_or_default().to_owned();
    let rest: Vec<&str> = lines.collect();
    if !rest.contains(&"tar") {
        return Err(Reach::Unreachable(format!("{host} has no tar")));
    }
    Ok(Probe {
        home,
        system,
        curl: rest.contains(&"curl"),
    })
}

pub fn check(host: &str) -> Reach {
    let reach = match probe(host) {
        Ok(_) => Reach::Reachable,
        Err(reach) => reach,
    };
    status_update(host, |s| {
        s.reach = Some(reach.clone());
        s.checked_at = crate::now_epoch_ms();
    });
    reach
}

/// One ssh command with `input` on stdin, as an error with ssh's own words on failure.
fn run(host: &str, remote: &str, input: &[u8]) -> Result<String> {
    use std::io::Write;
    let mut child = ssh(host)
        .arg(posix(remote))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start ssh")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match classify(output.status.code(), &stderr) {
            Reach::NeedsKey => bail!("{host} needs key-based login"),
            Reach::HostKey => {
                bail!("{host}'s host key isn't trusted yet; connect once in Terminal")
            }
            Reach::Unreachable(why) => bail!("{host} isn't reachable: {why}"),
            Reach::Reachable => bail!("{host}: {}", stderr.trim()),
        }
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// MARK: Status (what Settings shows per host)

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(default)]
    pub reach: Option<Reach>,
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default)]
    pub last_copy_at: u64,
    #[serde(default)]
    pub last_error: Option<String>,
}

fn status_path() -> PathBuf {
    crate::handoff::default_captures_dir()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
        .join("remote-status.json")
}

static STATUS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn statuses() -> BTreeMap<String, Status> {
    fs::read(status_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn status_update(host: &str, change: impl FnOnce(&mut Status)) {
    let _guard = STATUS_LOCK.lock();
    let mut all = statuses();
    change(all.entry(host.to_owned()).or_default());
    let path = status_path();
    if let Ok(body) = serde_json::to_vec_pretty(&all) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, body).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

fn status_forget(host: &str) {
    let _guard = STATUS_LOCK.lock();
    let mut all = statuses();
    if all.remove(host).is_some()
        && let Ok(body) = serde_json::to_vec_pretty(&all)
    {
        let _ = fs::write(status_path(), body);
    }
}

/// "just now", "4 min ago", "2 h ago", "3 days ago".
pub fn ago(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ if secs / 86_400 == 1 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

// MARK: Copying captures

/// Copies the light part of a bundle to `host:~/.scrawl/captures/<bundle>/`. The files are staged
/// beside the bundle on the host and moved into place whole, so an agent never reads half of one.
pub fn push(host: &str, bundle_dir: &Path) -> Result<()> {
    ensure!(safe_host(host), "“{host}” isn't a host name Scrawl can use");
    let name = bundle_dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| safe_name(n))
        .context("The bundle's folder name isn't one Scrawl can copy")?
        .to_owned();
    // Staged locally so the copy carries a summary that names the shared path (bundles made
    // before a remote was added name the Mac's own folder). `fs::copy` clones on APFS.
    let staging = std::env::temp_dir().join(format!("scrawl-push-{}", crate::make_bundle_id()));
    let staged = staging.join(&name);
    let result = (|| -> Result<()> {
        fs::create_dir_all(&staged)?;
        for rel in COPIED {
            let from = bundle_dir.join(rel);
            if rel == "summary.md" {
                continue;
            }
            if from.is_dir() {
                copy_dir(&from, &staged.join(rel))?;
            } else if from.is_file() {
                fs::copy(&from, staged.join(rel))?;
            }
        }
        let summary = match crate::handoff::load_bundle(bundle_dir) {
            Ok((dir, bundle)) => crate::handoff::summary_text(&bundle, &dir),
            Err(_) => fs::read_to_string(bundle_dir.join("summary.md")).unwrap_or_default(),
        };
        fs::write(staged.join("summary.md"), summary)?;
        let tar = Command::new("/usr/bin/tar")
            .arg("-C")
            .arg(&staging)
            .args(["-cf", "-"])
            .arg(&name)
            .stderr(Stdio::null())
            .output()
            .context("Could not run tar")?;
        ensure!(tar.status.success(), "tar failed");
        let dest = format!("\"$HOME\"/{REMOTE_CAPTURES}");
        let incoming = format!("{dest}/.{name}.incoming");
        run(
            host,
            &format!(
                "umask 077; mkdir -p {dest} && rm -rf {incoming} && mkdir {incoming} && \
                 tar -C {incoming} -xf - && rm -rf {dest}/{name}.old && \
                 {{ [ ! -e {dest}/{name} ] || mv {dest}/{name} {dest}/{name}.old; }} && \
                 mv {incoming}/{name} {dest}/{name} && rm -rf {incoming} {dest}/{name}.old && {{ {CLEAR_DEAD_SOCKET}; }}"
            ),
            &tar.stdout,
        )?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging);
    result
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()))?;
        } else {
            fs::copy(&path, to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// A bundle folder name safe in a remote shell command and a URL form value.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// Pushes to one host and records the outcome for Settings.
pub fn push_and_record(host: &str, bundle_dir: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let result = push(host, bundle_dir);
    match &result {
        Ok(()) => {
            scrawl_log(format!(
                "remote: copied {} to {host} in {} ms",
                bundle_dir.file_name().unwrap_or_default().to_string_lossy(),
                started.elapsed().as_millis()
            ));
            status_update(host, |s| {
                s.reach = Some(Reach::Reachable);
                s.last_copy_at = crate::now_epoch_ms();
                s.checked_at = crate::now_epoch_ms();
                s.last_error = None;
            });
        }
        Err(error) => {
            let text = format!("{error:#}");
            scrawl_log(format!("remote: copying to {host} failed: {text}"));
            let reach = if text.contains("needs key-based login") {
                Reach::NeedsKey
            } else if text.contains("host key") {
                Reach::HostKey
            } else if text.contains("isn't reachable") {
                Reach::Unreachable(text.clone())
            } else {
                Reach::Reachable
            };
            status_update(host, |s| {
                s.reach = Some(reach);
                s.checked_at = crate::now_epoch_ms();
                s.last_error = Some(text);
            });
        }
    }
    result
}

/// Copies a finished capture to every host with copying on, all at once, off the caller's
/// thread. Never holds up finishing a capture.
pub fn push_all_in_background(bundle_dir: &Path) {
    if cfg!(test) {
        return;
    }
    let hosts: Vec<String> = remotes()
        .into_iter()
        .filter(|r| r.copy)
        .map(|r| r.host)
        .collect();
    for host in hosts {
        let dir = bundle_dir.to_path_buf();
        let _ = std::thread::Builder::new()
            .name(format!("scrawl-remote-{host}"))
            .spawn(move || {
                let _ = push_and_record(&host, &dir);
            });
    }
}

// MARK: The same path on both sides

/// The Mac's `~/.scrawl` (or `SCRAWL_LINK_ROOT`).
pub fn link_root() -> PathBuf {
    std::env::var_os("SCRAWL_LINK_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".scrawl"))
}

/// Makes `~/.scrawl/captures` a link to the captures folder if nothing is there, and with zoom
/// on somewhere, `~/.scrawl/bin/scrawl` a link to this Scrawl, so the pasted zoom command also
/// works on the Mac. Never replaces anything that isn't a dangling link.
pub fn ensure_local_links(zoom: bool) {
    if cfg!(test) {
        return;
    }
    let root = link_root();
    let captures = crate::handoff::default_captures_dir();
    let _ = fs::create_dir_all(&captures);
    let link = root.join("captures");
    if fs::symlink_metadata(&link).is_err() || (is_symlink(&link) && !link.exists()) {
        let _ = fs::create_dir_all(&root);
        let _ = fs::remove_file(&link);
        if let Err(error) = std::os::unix::fs::symlink(&captures, &link) {
            scrawl_log(format!("remote: couldn't link {}: {error}", link.display()));
        }
    }
    if zoom && let Ok(exe) = std::env::current_exe() {
        let bin = root.join("bin/scrawl");
        if fs::symlink_metadata(&bin).is_err() || (is_symlink(&bin) && !bin.exists()) {
            let _ = fs::create_dir_all(root.join("bin"));
            let _ = fs::remove_file(&bin);
            let _ = std::os::unix::fs::symlink(exe, &bin);
        }
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Whether `~/.scrawl/captures` on the Mac opens the captures folder.
fn linked(captures: &Path) -> bool {
    match (
        fs::canonicalize(link_root().join("captures")),
        fs::canonicalize(captures),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// How a bundle's folder is named in text: `~/.scrawl/captures/<bundle>` when it's in the
/// captures folder, some remote is set up and the Mac's link is in place; else its real path.
pub fn shown_path(dir: &Path, captures: &Path, remotes: bool, linked: bool) -> PathBuf {
    let inside = dir.parent().is_some_and(|parent| {
        parent == captures
            || fs::canonicalize(parent)
                .ok()
                .zip(fs::canonicalize(captures).ok())
                .is_some_and(|(a, b)| a == b)
    });
    match dir.file_name() {
        Some(name) if remotes && linked && inside => {
            Path::new("~").join(REMOTE_CAPTURES).join(name)
        }
        _ => dir.to_path_buf(),
    }
}

pub fn shown_dir(dir: &Path) -> PathBuf {
    let list = remotes();
    if list.is_empty() {
        return dir.to_path_buf();
    }
    let captures = crate::handoff::default_captures_dir();
    if !linked(&captures) {
        ensure_local_links(list.iter().any(|r| r.zoom));
    }
    shown_path(dir, &captures, true, linked(&captures))
}

/// The one line the summary gains while some host can zoom.
pub fn zoom_line(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_str()?;
    remotes().iter().any(|r| r.zoom).then(|| {
        format!(
            "To look closely at a moment, run ~/{REMOTE_SCRIPT} zoom {name} --at <M:SS or \"words said\">"
        )
    })
}

// MARK: Zoom setup

/// What turning zoom on for `host` changes, for the confirmation: the lines it adds to the ssh
/// config (the whole new block) and the script.
pub fn zoom_preview(config: &Path, host: &str, home: Option<&str>) -> String {
    let mut list = remotes_in(config);
    let placeholder = format!("/home/<you on {host}>");
    match list.iter_mut().find(|r| r.host == host) {
        Some(remote) => {
            remote.zoom = true;
            if remote.home.is_none() {
                remote.home = Some(home.unwrap_or(&placeholder).to_owned());
            }
        }
        None => list.push(Remote {
            host: host.to_owned(),
            copy: true,
            zoom: true,
            token: None,
            home: Some(home.unwrap_or(&placeholder).to_owned()),
        }),
    }
    let hosts: Vec<(String, String)> = list
        .iter()
        .filter(|r| r.zoom)
        .filter_map(|r| Some((r.host.clone(), r.home.clone()?)))
        .collect();
    forward_block(&hosts, ingest_port())
}

/// How many lines turning zoom on adds to the ssh config: the block's markers the first time, and
/// a `Host` line and a `RemoteForward` line per host.
pub fn lines_added(config: &Path) -> usize {
    if remotes_in(config).iter().any(|r| r.zoom) {
        2
    } else {
        4
    }
}

/// Turns zoom on for `host`: installs the script and its key there, then adds the forward.
pub fn enable_zoom(config: &Path, host: &str) -> Result<()> {
    let probe = probe(host).map_err(|reach| match reach {
        Reach::NeedsKey => anyhow::anyhow!("{host} needs key-based login"),
        Reach::HostKey => anyhow::anyhow!("{host}'s host key isn't trusted yet"),
        Reach::Unreachable(why) => anyhow::anyhow!("{host} isn't reachable: {why}"),
        Reach::Reachable => anyhow::anyhow!("{host} didn't answer the check"),
    })?;
    ensure!(probe.curl, "Zoom needs curl on {host}, and it has none");
    ensure!(
        safe_home(&probe.home),
        "{host}'s home folder ({}) has characters Scrawl can't put in ~/.ssh/config",
        probe.home
    );
    let existing = remotes_in(config)
        .into_iter()
        .find(|r| r.host == host)
        .and_then(|r| r.token);
    let token = match existing {
        Some(token) => token,
        None => crate::handoff::random_hex_token_32()?,
    };
    // The script and the key go up on stdin, never on a command line.
    run(
        host,
        &format!(
            "umask 077; mkdir -p \"$HOME\"/.scrawl/bin && cat > \"$HOME\"/{REMOTE_SCRIPT}.tmp && \
             chmod 755 \"$HOME\"/{REMOTE_SCRIPT}.tmp && mv \"$HOME\"/{REMOTE_SCRIPT}.tmp \"$HOME\"/{REMOTE_SCRIPT}"
        ),
        SCRIPT.as_bytes(),
    )?;
    run(
        host,
        &format!(
            "umask 077; cat > \"$HOME\"/{REMOTE_TOKEN}.tmp && mv \"$HOME\"/{REMOTE_TOKEN}.tmp \"$HOME\"/{REMOTE_TOKEN}"
        ),
        format!("Authorization: Bearer {token}\n").as_bytes(),
    )?;
    update_remote_in(config, host, true, |r| {
        r.zoom = true;
        r.token = Some(token);
        r.home = Some(probe.home.clone());
    })?;
    write_block(&block_for(&remotes_in(config)))?;
    ensure_local_links(true);
    scrawl_log(format!("remote: zoom on for {host}"));
    Ok(())
}

/// Turns zoom off for `host`: its forward leaves the ssh config at once (the block goes when no
/// host is left), then the script and key are deleted there if it answers.
pub fn disable_zoom(config: &Path, host: &str) -> Result<()> {
    update_remote_in(config, host, false, |r| {
        r.zoom = false;
        r.token = None;
    })?;
    write_block(&block_for(&remotes_in(config)))?;
    scrawl_log(format!("remote: zoom off for {host}"));
    uninstall_zoom(host)
}

fn uninstall_zoom(host: &str) -> Result<()> {
    run(
        host,
        &format!(
            "rm -f \"$HOME\"/{REMOTE_SCRIPT} \"$HOME\"/{REMOTE_TOKEN} \"$HOME\"/{REMOTE_SOCKET}; rmdir \"$HOME\"/.scrawl/bin \"$HOME\"/.scrawl 2>/dev/null; true"
        ),
        b"",
    )
    .map(|_| ())
}

/// Removes `host` from Scrawl: its forward and entry at once, then (if it answers) the zoom
/// script, and with `delete_copies` the captures copied there.
pub fn remove_host(config: &Path, host: &str, delete_copies: bool) -> Result<()> {
    let had_zoom = remotes_in(config).iter().any(|r| r.host == host && r.zoom);
    remove_remote_in(config, host)?;
    write_block(&block_for(&remotes_in(config)))?;
    status_forget(host);
    let mut steps = Vec::new();
    if had_zoom {
        steps.push(format!(
            "rm -f \"$HOME\"/{REMOTE_SCRIPT} \"$HOME\"/{REMOTE_TOKEN} \"$HOME\"/{REMOTE_SOCKET}; rmdir \"$HOME\"/.scrawl/bin 2>/dev/null"
        ));
    }
    if delete_copies {
        steps.push(format!(
            "rm -rf \"$HOME\"/{REMOTE_CAPTURES}/*.scrawl \"$HOME\"/{REMOTE_CAPTURES}/.*.incoming; \
             rmdir \"$HOME\"/{REMOTE_CAPTURES} 2>/dev/null"
        ));
    }
    if steps.is_empty() {
        return Ok(());
    }
    steps.push("rmdir \"$HOME\"/.scrawl 2>/dev/null; true".to_owned());
    run(host, &steps.join("; "), b"").map(|_| ())
}

// MARK: Retention

/// Deletes copies older than `bundle_days` on every host with copying on, as the Mac's own
/// cleanup does (copies carry no video, so only the whole-bundle age applies).
pub fn prune(bundle_days: Option<u32>) {
    let Some(days) = bundle_days else {
        return;
    };
    for remote in remotes().into_iter().filter(|r| r.copy) {
        let command = format!(
            "[ -d \"$HOME\"/{REMOTE_CAPTURES} ] || exit 0; find \"$HOME\"/{REMOTE_CAPTURES} -mindepth 1 -maxdepth 1 \
             -type d -name '*.scrawl' -mtime +{days} -print -exec rm -rf {{}} +"
        );
        match run(&remote.host, &command, b"") {
            Ok(out) => {
                let n = out.lines().filter(|l| !l.trim().is_empty()).count();
                if n > 0 {
                    scrawl_log(format!("remote: removed {n} old copies on {}", remote.host));
                }
            }
            Err(error) => scrawl_log(format!(
                "remote: cleanup on {} failed: {error:#}",
                remote.host
            )),
        }
    }
}

// MARK: /zoom on the ingest server

/// What `/zoom` was asked.
#[derive(Clone, Debug, PartialEq)]
pub struct ZoomRequest {
    pub capture: String,
    pub at: String,
    pub span_ms: u64,
    pub frames: usize,
    pub full: bool,
}

/// What `/zoom` answers with.
pub struct ZoomOut {
    pub png: Vec<u8>,
    pub file: String,
    pub cells: String,
    pub said: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    fn text(status: &'static str, message: &str) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/plain; charset=utf-8".to_owned())],
            body: message.as_bytes().to_vec(),
        }
    }
}

pub fn is_zoom_request(header: &str) -> bool {
    let first = header.lines().next().unwrap_or("");
    first.starts_with("POST /zoom ") || first.starts_with("POST /zoom?")
}

/// The host whose key the request carries, if it has zoom on. Compared in constant time.
pub fn authorize(header: &str, remotes: &[Remote]) -> Option<String> {
    let provided = header.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if !k.trim().eq_ignore_ascii_case("authorization") {
            return None;
        }
        let v = v.trim();
        v.strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
            .map(str::trim)
    })?;
    let mut matched = None;
    for remote in remotes.iter().filter(|r| r.zoom) {
        if let Some(token) = remote.token.as_deref()
            && crate::phone::constant_time_eq(provided, token)
        {
            matched = Some(remote.host.clone());
        }
    }
    matched
}

/// `application/x-www-form-urlencoded`, as curl's `--data-urlencode` sends it.
pub fn parse_form(body: &str) -> BTreeMap<String, String> {
    fn decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < bytes.len() => {
                    let hex = |b: u8| (b as char).to_digit(16);
                    match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                        (Some(hi), Some(lo)) => {
                            out.push((hi * 16 + lo) as u8);
                            i += 2;
                        }
                        _ => out.push(b'%'),
                    }
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    body.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

pub fn parse_request(body: &str) -> std::result::Result<ZoomRequest, String> {
    let form = parse_form(body);
    let capture = form
        .get("capture")
        .cloned()
        .unwrap_or_else(|| "latest".into());
    let capture = if capture.ends_with(".scrawl") || capture == "latest" {
        capture
    } else {
        format!("{capture}.scrawl")
    };
    if capture != "latest" && !safe_name(&capture) {
        return Err(format!("“{capture}” isn't a capture name"));
    }
    let at = form
        .get("at")
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty())
        .ok_or("Pass --at with a time (0:12) or words that were said")?;
    let span: f64 = form
        .get("span")
        .map_or(Ok(2.0), |s| s.parse())
        .map_err(|_| "--span must be a number of seconds".to_owned())?;
    if !(span.is_finite() && span > 0.0 && span <= 120.0) {
        return Err("--span must be between 0 and 120 seconds".into());
    }
    let frames: usize = form
        .get("frames")
        .map_or(Ok(12), |s| s.parse())
        .map_err(|_| "--frames must be a whole number".to_owned())?;
    Ok(ZoomRequest {
        capture,
        at,
        span_ms: (span * 1000.0) as u64,
        frames: frames.clamp(1, 48),
        full: form.get("full").is_some_and(|f| f == "true" || f == "1"),
    })
}

/// Answers one `/zoom` request: checks the key, finds the capture in `captures` and runs `zoom`.
pub fn zoom_reply(
    header: &str,
    body: &str,
    remotes: &[Remote],
    captures: &Path,
    zoom: impl FnOnce(&Path, &ZoomRequest) -> Result<ZoomOut>,
) -> Reply {
    let Some(host) = authorize(header, remotes) else {
        return Reply::text(
            "401 Unauthorized",
            "Scrawl on the Mac didn't accept this machine's key. Turn Allow zoom off and on again in Scrawl's settings.",
        );
    };
    let request = match parse_request(body) {
        Ok(request) => request,
        Err(message) => return Reply::text("400 Bad Request", &message),
    };
    let dir = if request.capture == "latest" {
        crate::handoff::resolve_bundle_dir_in(captures, Path::new("latest"))
    } else {
        Ok(captures.join(&request.capture))
    };
    let dir = match dir {
        Ok(dir) if dir.join("scrawl.json").is_file() => dir,
        _ => {
            return Reply::text(
                "404 Not Found",
                &format!("The Mac has no capture called {}", request.capture),
            );
        }
    };
    scrawl_log(format!(
        "remote: zoom from {host} into {} at {}",
        request.capture, request.at
    ));
    match zoom(&dir, &request) {
        Ok(out) => {
            let clean = |s: &str| -> String {
                s.chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .take(4000)
                    .collect()
            };
            Reply {
                status: "200 OK",
                headers: vec![
                    ("Content-Type", "image/png".to_owned()),
                    ("X-Scrawl-File", clean(&out.file)),
                    ("X-Scrawl-Cells", clean(&out.cells)),
                    ("X-Scrawl-Said", clean(&out.said)),
                ],
                body: out.png,
            }
        }
        Err(error) => Reply::text("422 Unprocessable Entity", &format!("{error:#}")),
    }
}

/// The real zoom: `zoom::zoom` on the Mac's copy, which has the video.
pub fn run_zoom(dir: &Path, request: &ZoomRequest) -> Result<ZoomOut> {
    let (dir, bundle) = crate::handoff::load_bundle(dir)?;
    let sheet = crate::zoom::zoom(
        &dir,
        &bundle,
        &request.at,
        request.span_ms,
        request.frames,
        !request.full,
    )?;
    Ok(ZoomOut {
        png: fs::read(&sheet.path)?,
        file: sheet
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "zoom.png".into()),
        cells: crate::zoom::cells_line(&sheet),
        said: sheet.said.clone(),
    })
}

/// Reads the body after `header` and answers `/zoom` on `stream`.
pub fn serve_zoom(stream: &mut std::net::TcpStream, header: &str) -> Result<()> {
    use std::io::{Read, Write};
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    let reply = if length > 64 * 1024 {
        Reply::text("413 Payload Too Large", "Request too large")
    } else {
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body)?;
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(60)));
        zoom_reply(
            header,
            &String::from_utf8_lossy(&body),
            &remotes(),
            &crate::handoff::default_captures_dir(),
            run_zoom,
        )
    };
    let mut head = format!("HTTP/1.1 {}\r\n", reply.status);
    for (key, value) in &reply.headers {
        head.push_str(&format!("{key}: {value}\r\n"));
    }
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        reply.body.len()
    ));
    stream.write_all(head.as_bytes())?;
    stream.write_all(&reply.body)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "# my config\nServerAliveInterval 30\nInclude ~/.orbstack/ssh/config\n\nHost pro\n    HostName 100.1.2.3\n\nHost=github.com\n  User git\n\nhost mini   kitbox.exe.xyz\n\tUser administrator\n\nHost *.internal !bastion web-?\n    User me\n\nMatch host \"ci\"\n    User ci\n\nHost *\n    AddKeysToAgent yes\n";

    #[test]
    fn ssh_config_hosts_skip_patterns_and_keep_order() {
        assert_eq!(
            parse_hosts(CONFIG),
            ["pro", "github.com", "mini", "kitbox.exe.xyz"]
        );
        // Scrawl's own block doesn't suggest its hosts twice, and duplicates collapse.
        let with = with_block(
            CONFIG,
            &forward_block(&[("mini".into(), "/Users/a".into())], 47777),
        );
        assert_eq!(parse_hosts(&with), parse_hosts(CONFIG));
        assert_eq!(parse_hosts("Host a\nHost a b\n# Host c\n"), ["a", "b"]);
        assert!(parse_hosts("").is_empty());
    }

    #[test]
    fn block_goes_before_the_first_host_and_comes_out_byte_identical() {
        let block = forward_block(
            &[
                ("mini".into(), "/Users/administrator".into()),
                ("kitbox.exe.xyz".into(), "/home/exedev".into()),
            ],
            47777,
        );
        assert_eq!(
            block,
            "# >>> scrawl (remove to undo) >>>\nHost mini\n    RemoteForward /Users/administrator/.scrawl/scrawl.sock 127.0.0.1:47777\nHost kitbox.exe.xyz\n    RemoteForward /home/exedev/.scrawl/scrawl.sock 127.0.0.1:47777\n# <<< scrawl <<<\n"
        );
        let added = with_block(CONFIG, &block);
        // Global options and the Include stay above it, so they stay global.
        assert!(added.starts_with(
            "# my config\nServerAliveInterval 30\nInclude ~/.orbstack/ssh/config\n\n# >>> scrawl"
        ));
        assert!(added.contains("# <<< scrawl <<<\nHost pro\n"));
        // Idempotent, replaceable, removable.
        assert_eq!(with_block(&added, &block), added);
        assert_eq!(without_block(&added), CONFIG);
        assert_eq!(with_block(&added, ""), CONFIG);
        let one = forward_block(&[("mini".into(), "/Users/administrator".into())], 47777);
        assert_eq!(without_block(&with_block(&added, &one)), CONFIG);
        assert_eq!(with_block(&added, &one).matches(BLOCK_BEGIN).count(), 1);
    }

    #[test]
    fn block_edges_empty_files_and_files_without_hosts() {
        let block = forward_block(&[("mini".into(), "/Users/a".into())], 47777);
        assert_eq!(with_block("", &block), block);
        assert_eq!(without_block(&block), "");
        // No Host line: appended, and the user's text is untouched before it.
        let globals = "ServerAliveInterval 30\n";
        assert_eq!(with_block(globals, &block), format!("{globals}{block}"));
        assert_eq!(without_block(&with_block(globals, &block)), globals);
        // No final newline and no Host line: the one documented change is the added newline.
        assert_eq!(
            without_block(&with_block("Compression yes", &block)),
            "Compression yes\n"
        );
        // No final newline after a Host block: the block went before it, so nothing changes.
        assert_eq!(
            without_block(&with_block("Host a\n  User b", &block)),
            "Host a\n  User b"
        );
        // CRLF files keep their line endings.
        let crlf = "Host a\r\n  User b\r\n";
        assert_eq!(without_block(&with_block(crlf, &block)), crlf);
        // A half-deleted block (end marker gone) is left alone rather than eating the file.
        let broken = format!("{BLOCK_BEGIN}\nHost x\nHost a\n");
        assert_eq!(without_block(&broken), broken);
        assert!(forward_block(&[], 47777).is_empty());
    }

    #[test]
    fn paths_use_the_shared_name_only_when_set_up() {
        let captures = Path::new("/Users/k/Library/Application Support/scrawl/captures");
        let dir = captures.join("scrawl-1.scrawl");
        assert_eq!(
            shown_path(&dir, captures, true, true),
            Path::new("~/.scrawl/captures/scrawl-1.scrawl")
        );
        assert_eq!(shown_path(&dir, captures, false, true), dir);
        assert_eq!(shown_path(&dir, captures, true, false), dir);
        let elsewhere = Path::new("/tmp/out/scrawl-1.scrawl");
        assert_eq!(shown_path(elsewhere, captures, true, true), elsewhere);
    }

    #[test]
    fn summary_header_names_the_shared_path() {
        let bundle: crate::model::ScrawlBundle = serde_json::from_value(serde_json::json!({
            "version": 1, "id": "t", "createdAtMs": 0, "durationMs": 5_000,
            "target": {"mode": "display", "displayId": 0,
                "frame": {"x": 0.0, "y": 0.0, "width": 10.0, "height": 10.0, "scale": 1.0}},
            "transcript": {"source": "test", "text": "hello", "segments": []},
            "strokes": [], "marks": [], "events": [], "keyframes": [], "contactSheet": "c.png"
        }))
        .unwrap();
        let captures = Path::new("/c");
        let shown = shown_path(&captures.join("scrawl-9.scrawl"), captures, true, true);
        let summary = crate::sketch::summary_markdown(&bundle, &shown);
        assert!(
            summary.starts_with("[scrawl 0:05 · Display · ~/.scrawl/captures/scrawl-9.scrawl]\n")
        );
    }

    fn remote(host: &str, zoom: bool, token: &str) -> Remote {
        Remote {
            host: host.into(),
            copy: true,
            zoom,
            token: Some(token.into()),
            home: Some("/home/x".into()),
        }
    }

    fn header(token: Option<&str>) -> String {
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        format!("POST /zoom HTTP/1.1\r\nHost: scrawl\r\n{auth}Content-Length: 10\r\n\r\n")
    }

    fn captures_with(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(crate::make_bundle_id());
        fs::create_dir_all(root.join(name)).unwrap();
        fs::write(root.join(name).join("scrawl.json"), "{}").unwrap();
        root
    }

    fn fake(png: &[u8]) -> impl FnOnce(&Path, &ZoomRequest) -> Result<ZoomOut> + '_ {
        move |_, request| {
            Ok(ZoomOut {
                png: png.to_vec(),
                file: "zoom-1-2.png".into(),
                cells: format!("{} cells", request.frames),
                said: "it's\nweird".into(),
            })
        }
    }

    #[test]
    fn zoom_needs_the_key_of_a_host_with_zoom_on() {
        let captures = captures_with("scrawl-1.scrawl");
        let remotes = [
            remote("mini", true, "k-mini"),
            remote("box", false, "k-box"),
        ];
        let body = "capture=scrawl-1.scrawl&at=0%3A03&frames=6";
        let called = std::cell::Cell::new(false);
        let never = |_: &Path, _: &ZoomRequest| -> Result<ZoomOut> {
            called.set(true);
            bail!("must not run")
        };
        assert!(is_zoom_request(&header(None)));
        assert!(!is_zoom_request("POST /ingest HTTP/1.1\r\n"));
        for token in [
            None,
            Some("wrong"),
            Some("k-box"),
            Some(""),
            Some("k-mini2"),
            Some("k-min"),
        ] {
            let reply = zoom_reply(&header(token), body, &remotes, &captures, never);
            assert_eq!(reply.status, "401 Unauthorized", "{token:?}");
        }
        // The ingest token is not a zoom key either.
        assert!(authorize(&header(Some("ingest")), &[remote("mini", true, "k")]).is_none());
        assert!(!called.get());

        let reply = zoom_reply(
            &header(Some("k-mini")),
            body,
            &remotes,
            &captures,
            fake(b"PNG"),
        );
        assert_eq!(reply.status, "200 OK");
        assert_eq!(reply.body, b"PNG");
        assert!(
            reply
                .headers
                .contains(&("X-Scrawl-File", "zoom-1-2.png".into()))
        );
        assert!(
            reply
                .headers
                .contains(&("X-Scrawl-Cells", "6 cells".into()))
        );
        // Header values never carry a line break.
        assert!(
            reply
                .headers
                .contains(&("X-Scrawl-Said", "it's weird".into()))
        );
        fs::remove_dir_all(captures).unwrap();
    }

    #[test]
    fn zoom_only_opens_captures_by_name() {
        let captures = captures_with("scrawl-1.scrawl");
        let remotes = [remote("mini", true, "k")];
        let h = header(Some("k"));
        for body in [
            "capture=..%2F..%2Fetc&at=1",
            "capture=%2Fetc%2Fpasswd&at=1",
            "capture=.hidden.scrawl&at=1",
        ] {
            let reply = zoom_reply(&h, body, &remotes, &captures, fake(b""));
            assert_eq!(reply.status, "400 Bad Request", "{body}");
        }
        let missing = zoom_reply(&h, "capture=scrawl-2&at=1", &remotes, &captures, fake(b""));
        assert_eq!(missing.status, "404 Not Found");
        let no_at = zoom_reply(&h, "capture=scrawl-1", &remotes, &captures, fake(b""));
        assert_eq!(no_at.status, "400 Bad Request");
        let latest = zoom_reply(&h, "capture=latest&at=1", &remotes, &captures, fake(b"x"));
        assert_eq!(latest.status, "200 OK");
        fs::remove_dir_all(captures).unwrap();
    }

    #[test]
    fn form_decoding_matches_curl() {
        let form =
            parse_form("at=%E2%80%9Cwidth+changes%E2%80%9D&span=2.5&bad=%zz&capture=scrawl-1");
        assert_eq!(form["at"], "“width changes”");
        assert_eq!(form["bad"], "%zz");
        let request = parse_request("at=width+changes&span=2.5&frames=99&full=true").unwrap();
        assert_eq!(request.capture, "latest");
        assert_eq!(request.span_ms, 2500);
        assert_eq!(request.frames, 48);
        assert!(request.full);
        assert!(parse_request("at=1&span=-1").is_err());
    }

    #[test]
    fn ssh_failures_are_told_apart() {
        assert_eq!(classify(Some(0), ""), Reach::Reachable);
        assert_eq!(classify(Some(1), "tar: oops"), Reach::Reachable);
        assert_eq!(
            classify(
                Some(255),
                "administrator@mini: Permission denied (publickey,password)."
            ),
            Reach::NeedsKey
        );
        assert_eq!(
            classify(Some(255), "Host key verification failed."),
            Reach::HostKey
        );
        assert_eq!(
            classify(
                Some(255),
                "ssh: connect to host box port 22: Operation timed out\r\n"
            ),
            Reach::Unreachable("connect to host box port 22: Operation timed out".into())
        );
        assert!(safe_host("kitbox.exe.xyz") && safe_host("me@box"));
        assert!(!safe_host("-oProxyCommand=x") && !safe_host("a b") && !safe_host("a;b"));
    }

    #[test]
    fn remotes_round_trip_through_the_config_keeping_other_keys() {
        let path = std::env::temp_dir().join(format!("{}.json", crate::make_bundle_id()));
        fs::write(
            &path,
            r#"{"ingestToken":"abc","remotes":[{"host":"bad host"}]}"#,
        )
        .unwrap();
        assert!(remotes_in(&path).is_empty());
        update_remote_in(&path, "mini", true, |r| r.home = Some("/Users/a".into())).unwrap();
        update_remote_in(&path, "mini", false, |r| r.zoom = true).unwrap();
        assert!(update_remote_in(&path, "other", false, |_| {}).is_err());
        let list = remotes_in(&path);
        assert_eq!(list.len(), 1);
        assert!(list[0].copy && list[0].zoom);
        assert_eq!(block_for(&list).lines().count(), 4);
        let config: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["ingestToken"], "abc");
        remove_remote_in(&path, "mini").unwrap();
        assert!(remotes_in(&path).is_empty());
        assert_eq!(lines_added(&path), 4);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn remote_commands_survive_any_login_shell() {
        let command = posix(r#"printf '%s\n' "$HOME" && echo "it's""#);
        assert_eq!(
            command,
            r#"/bin/sh -c 'printf '\''%s\n'\'' "$HOME" && echo "it'\''s"'"#
        );
        let out = Command::new("/bin/zsh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("{}\nit's\n", std::env::var("HOME").unwrap())
        );
    }

    #[test]
    fn the_script_is_posix_sh() {
        let status = Command::new("/bin/sh")
            .args(["-n", "-c", SCRIPT])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            !SCRIPT.contains("Bearer"),
            "the key stays in ~/.scrawl/token"
        );
    }
}
