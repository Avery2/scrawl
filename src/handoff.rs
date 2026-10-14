use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::model::ScrawlBundle;

pub fn default_captures_dir() -> PathBuf {
    std::env::var_os("SCRAWL_CAPTURES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/Application Support/scrawl/captures")
        })
}

pub fn resolve_bundle_dir(path_or_id: &Path) -> Result<PathBuf> {
    if path_or_id == Path::new("latest") {
        return newest_bundle(&default_captures_dir());
    }
    let mut path = path_or_id.to_owned();
    if path.file_name().is_some_and(|name| name == "scrawl.json") {
        path = path
            .parent()
            .context("Missing bundle directory")?
            .to_owned();
        if path.as_os_str().is_empty() {
            path = PathBuf::from(".");
        }
    }
    if !path.exists() && path.components().count() == 1 {
        path = default_captures_dir().join(&path);
        if path.extension().is_none() {
            path.set_extension("scrawl");
        }
    }
    ensure!(
        path.is_dir(),
        "Bundle directory not found: {}",
        path.display()
    );
    let path = path
        .canonicalize()
        .with_context(|| format!("Could not resolve {}", path.display()))?;
    ensure!(
        path.extension().is_some_and(|ext| ext == "scrawl"),
        "Expected a .scrawl directory: {}",
        path.display()
    );
    Ok(path)
}

fn newest_bundle(captures: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    for entry in
        fs::read_dir(captures).with_context(|| format!("Could not read {}", captures.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "scrawl") && path.is_dir() {
            // Bundle creation time is stable even after later zoom-frame extraction.
            let created = fs::read(path.join("scrawl.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ScrawlBundle>(&bytes).ok())
                .map(|bundle| bundle.created_at_ms)
                .unwrap_or_else(|| {
                    entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.created().or_else(|_| m.modified()).ok())
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |time| time.as_millis() as u64)
                });
            candidates.push((created, path));
        }
    }
    candidates.sort_unstable();
    candidates
        .pop()
        .context("No .scrawl captures found")?
        .1
        .canonicalize()
        .map_err(Into::into)
}

pub fn load_bundle(bundle_dir: &Path) -> Result<(PathBuf, ScrawlBundle)> {
    let directory = resolve_bundle_dir(bundle_dir)?;
    let path = directory.join("scrawl.json");
    let bundle = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?,
    )
    .with_context(|| format!("Invalid bundle manifest: {}", path.display()))?;
    Ok((directory, bundle))
}

#[cfg(target_os = "macos")]
pub fn copy_string_to_clipboard(content: &str) -> Result<()> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSString};

    let text = NSString::from_str(content);
    let pasteboard = NSPasteboard::generalPasteboard();
    unsafe {
        let types = NSArray::from_slice(&[NSPasteboardTypeString]);
        pasteboard.clearContents();
        pasteboard.declareTypes_owner(&types, None);
        ensure!(
            pasteboard.setString_forType(&text, NSPasteboardTypeString),
            "Could not copy text to clipboard"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn copy_png_to_clipboard(png_bytes: &[u8]) -> Result<()> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG};
    use objc2_foundation::{NSArray, NSData};

    let data = NSData::with_bytes(png_bytes);
    let pasteboard = NSPasteboard::generalPasteboard();
    unsafe {
        let types = NSArray::from_slice(&[NSPasteboardTypePNG]);
        pasteboard.clearContents();
        pasteboard.declareTypes_owner(&types, None);
        ensure!(
            pasteboard.setData_forType(Some(&data), NSPasteboardTypePNG),
            "Could not copy PNG image to clipboard"
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn copy_bundle_to_clipboard(bundle_dir: &Path) -> Result<()> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);
    copy_string_to_clipboard(&summary)
}

/// Pastes both the contact-sheet image and the transcript/keyframe text into the active app
/// as a two-step sequence (`⌘V` image, then `⌘V` text). If `click_target` is provided (from
/// dragging the Paste chip onto a window/input), clicks at that screen coordinate first to focus it.
#[cfg(target_os = "macos")]
pub fn paste_bundle_at_target(bundle_dir: &Path, click_target: Option<[f32; 2]>) -> Result<()> {
    if let Some([x, y]) = click_target {
        crate::capture::post_synthetic_click(f64::from(x), f64::from(y));
        std::thread::sleep(std::time::Duration::from_millis(65));
    } else {
        crate::capture::ensure_external_app_active();
    }
    paste_bundle_image_and_text(bundle_dir)
}

#[cfg(target_os = "macos")]
pub fn paste_bundle_image_and_text(bundle_dir: &Path) -> Result<()> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let sheet_path = directory.join(&bundle.contact_sheet);
    let image = fs::read(&sheet_path).context("Could not read contact sheet")?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);

    crate::capture::wait_for_modifiers_released(350);

    if crate::capture::is_frontmost_terminal() {
        // In terminals (Ghostty / OpenCode TUI / Claude Code), pasting the image path first
        // resolves and attaches `[Image 1]`, and the second paste inserts the text summary.
        copy_string_to_clipboard(&sheet_path.display().to_string())?;
    } else {
        // In GUI apps (OpenCode Desktop, Slack, Linear, browsers), PNG-only attaches the image.
        copy_png_to_clipboard(&image)?;
    }
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::capture::post_synthetic_cmd_v();

    std::thread::sleep(std::time::Duration::from_millis(160));
    copy_string_to_clipboard(&summary)?;
    std::thread::sleep(std::time::Duration::from_millis(20));
    crate::capture::post_synthetic_cmd_v();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn copy_bundle_to_clipboard(_bundle_dir: &Path) -> Result<()> {
    bail!("Image and text clipboard handoff requires macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn paste_bundle_image_and_text(_bundle_dir: &Path) -> Result<()> {
    bail!("Image and text paste handoff requires macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn paste_bundle_at_target(_bundle_dir: &Path, _click_target: Option<[f32; 2]>) -> Result<()> {
    bail!("Image and text paste handoff requires macOS")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OpenCodeSession {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub time: OpenCodeSessionTime,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OpenCodeSessionTime {
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub archived: Option<u64>,
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|path| path.join(name))
            .find(|path| executable(path))
    })
}

fn opencode_binary() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("OPENCODE_BIN").filter(|bin| !bin.is_empty()) {
        let path = PathBuf::from(bin);
        if executable(&path) {
            return Ok(path);
        }
        if let Some(path) = path.to_str().and_then(on_path) {
            return Ok(path);
        }
        bail!("OPENCODE_BIN is not an executable");
    }
    if let Some(path) = on_path("opencode2") {
        return Ok(path);
    }
    if let Some(path) = dirs::home_dir()
        .map(|home| home.join("bin/opencode2"))
        .filter(|path| executable(path))
    {
        return Ok(path);
    }
    bail!("Could not find opencode2; set OPENCODE_BIN or add it to PATH")
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    agent_host: Option<String>,
    #[serde(default)]
    agent_opencode: Option<String>,
}

fn config() -> Config {
    dirs::home_dir()
        .and_then(|home| fs::read(home.join(".config/scrawl/config.json")).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Safe to put unquoted in a remote shell command (host names, `~/bin/tool` paths).
fn safe_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.@/~".contains(&b))
}

/// The Mac whose OpenCode server holds Kit's sessions when it isn't this one: an SSH host from
/// `SCRAWL_AGENT_HOST` or `~/.config/scrawl/config.json` (`{"agentHost": "studio"}`).
pub fn agent_host() -> Option<String> {
    std::env::var("SCRAWL_AGENT_HOST")
        .ok()
        .or_else(|| config().agent_host)
        .filter(|h| safe_word(h))
}

/// The `opencode2` command to run on the agent host (`agentOpencode`, default `opencode2`).
fn agent_opencode() -> String {
    config()
        .agent_opencode
        .filter(|c| safe_word(c))
        .unwrap_or_else(|| "opencode2".to_owned())
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn ssh(host: &str) -> Command {
    let mut command = Command::new("/usr/bin/ssh");
    command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=8", host]);
    command
}

/// Copies a bundle to the same path on `host`, so the agent there can open its files.
pub fn sync_bundle_to(host: &str, bundle_dir: &Path) -> Result<()> {
    use std::process::Stdio;
    let parent = bundle_dir.parent().context("Bundle has no parent")?;
    let name = bundle_dir.file_name().context("Bundle has no name")?;
    let mut tar = Command::new("/usr/bin/tar")
        .arg("-C")
        .arg(parent)
        .arg("-cf")
        .arg("-")
        .arg(name)
        .stdout(Stdio::piped())
        .spawn()
        .context("Could not start tar")?;
    let parent_q = shell_quote(&parent.to_string_lossy());
    let output = ssh(host)
        .arg(format!("mkdir -p {parent_q} && tar -C {parent_q} -xf -"))
        .stdin(tar.stdout.take().context("No tar output")?)
        .output()
        .context("Could not start ssh")?;
    let _ = tar.wait();
    ensure!(
        output.status.success(),
        "Copying the bundle to {host} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn opencode_api(method: &str, path: &str, data: Option<&serde_json::Value>) -> Result<Vec<u8>> {
    let mut command = if let Some(host) = agent_host() {
        let mut remote = format!(
            "PATH=\"$HOME/bin:$HOME/.opencode/bin:/opt/homebrew/bin:$PATH\" {} api {} {}",
            agent_opencode(),
            shell_quote(method),
            shell_quote(path)
        );
        if let Some(data) = data {
            remote.push_str(&format!(
                " --data {}",
                shell_quote(&serde_json::to_string(data)?)
            ));
        }
        let mut command = ssh(&host);
        command.arg(remote);
        command
    } else {
        let mut command = Command::new(opencode_binary()?);
        command.args(["api", method, path]);
        if let Some(data) = data {
            command.args(["--data", &serde_json::to_string(data)?]);
        }
        command
    };
    let output = command.output().context("Could not launch opencode2")?;
    ensure!(
        output.status.success(),
        "OpenCode API failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn query_escape(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

pub fn list_opencode_sessions() -> Result<Vec<OpenCodeSession>> {
    let mut sessions = Vec::new();
    let mut path = "/api/session".to_owned();
    let mut cursors = std::collections::HashSet::new();
    loop {
        let response: serde_json::Value =
            serde_json::from_slice(&opencode_api("get", &path, None)?)
                .context("Invalid OpenCode session response")?;
        let data = response
            .as_array()
            .or_else(|| response.get("data").and_then(|data| data.as_array()))
            .context("Missing OpenCode session list")?;
        for session in data {
            sessions.push(serde_json::from_value::<OpenCodeSession>(session.clone())?);
        }
        let Some(next) = response.pointer("/cursor/next").and_then(|v| v.as_str()) else {
            break;
        };
        ensure!(
            cursors.insert(next.to_owned()),
            "OpenCode returned a repeated session cursor"
        );
        path = format!("/api/session?cursor={}", query_escape(next));
    }
    sessions.sort_by(|a, b| {
        b.time
            .updated
            .cmp(&a.time.updated)
            .then_with(|| a.id.cmp(&b.id))
    });
    sessions.dedup_by(|a, b| a.id == b.id);
    Ok(sessions)
}

fn normalize_slug(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

pub fn send_to_opencode(bundle_dir: &Path, session_id: Option<&str>) -> Result<String> {
    let (id, _) = send_to_opencode_for_app(bundle_dir, session_id, None)?;
    Ok(id)
}

pub fn send_to_opencode_for_app(
    bundle_dir: &Path,
    session_id: Option<&str>,
    app_name: Option<&str>,
) -> Result<(String, String)> {
    let (_, bundle) = load_bundle(bundle_dir)?;
    let (session, title) =
        resolve_opencode_session(session_id, app_name.or(bundle.target.app.as_deref()))?;
    send_bundle_to_session(bundle_dir, &session)?;
    Ok((session, title))
}

/// Picks the session a capture goes to, and its title: an explicit ID, else the root session
/// whose title matches the app ("UpDown" → "UP DOWN APP"), else `OPENCODE_SESSION_ID`, else the
/// most recently updated root session.
pub fn resolve_opencode_session(
    session_id: Option<&str>,
    app_name: Option<&str>,
) -> Result<(String, String)> {
    let app_slug = app_name.map(normalize_slug).filter(|s| !s.is_empty());

    let (session, session_title) = if let Some(id) = session_id.filter(|s| !s.trim().is_empty()) {
        let title = list_opencode_sessions()
            .ok()
            .and_then(|list| list.into_iter().find(|s| s.id == id).map(|s| s.title))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| id.to_owned());
        (id.to_owned(), title)
    } else {
        let roots: Vec<OpenCodeSession> = list_opencode_sessions()?
            .into_iter()
            .filter(|session| session.parent_id.is_none() && session.time.archived.is_none())
            .collect();
        let matched = app_slug
            .as_deref()
            .and_then(|slug| {
                // Match against the last component if bundle identifier was passed (e.g. com.kitlangton.UpDown -> updown).
                let short = app_name
                    .and_then(|a| a.rsplit('.').next())
                    .map(normalize_slug)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| slug.to_owned());
                roots.iter().find(|s| {
                    let title_slug = normalize_slug(&s.title);
                    !title_slug.is_empty()
                        && (title_slug.contains(&short) || short.contains(&title_slug))
                })
            })
            .cloned()
            .or_else(|| {
                if let Some(id) = std::env::var("OPENCODE_SESSION_ID")
                    .ok()
                    .filter(|id| !id.trim().is_empty())
                {
                    roots.iter().find(|s| s.id == id).cloned()
                } else {
                    None
                }
            })
            .or_else(|| roots.into_iter().next())
            .context("No root OpenCode session found; pass a session ID")?;
        let title = if matched.title.is_empty() {
            matched.id.clone()
        } else {
            matched.title
        };
        (matched.id, title)
    };
    Ok((session, session_title))
}

/// Queues a bundle's summary and pointers into an OpenCode session.
pub fn send_bundle_to_session(bundle_dir: &Path, session: &str) -> Result<()> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);
    ensure!(
        session.starts_with("ses")
            && session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Invalid OpenCode session ID"
    );
    if let Some(host) = agent_host() {
        sync_bundle_to(&host, &directory)?;
    }
    let quoted_directory = shell_quote(&directory.to_string_lossy());
    let text = format!(
        "{}\n\nOpen the contact sheet at {} and read {} for timestamped transcript, marks, and interactions.\nTo look closely at a moment (an animation, a glitch, what the narration points at), run:\nscrawl zoom {} --at <M:SS or \"a phrase that was said\"> [--span 2] [--frames 12]\nIt saves a dense contact sheet of that moment with each frame's time and prints what was said; open the PNG before acting on the feedback. This capture is {} ms long.",
        summary.trim_end(),
        directory.join(&bundle.contact_sheet).display(),
        directory.join("scrawl.json").display(),
        quoted_directory,
        bundle.duration_ms,
    );
    opencode_api(
        "post",
        &format!("/api/session/{session}/prompt"),
        Some(&serde_json::json!({ "text": text, "delivery": "queue" })),
    )?;
    Ok(())
}

pub fn extract_frames_range(
    bundle_dir: &Path,
    from_ms: u64,
    to_ms: u64,
    fps: u32,
) -> Result<Vec<PathBuf>> {
    ensure!(from_ms <= to_ms, "Frame range ends before it starts");
    ensure!(fps > 0 && fps <= 240, "FPS must be between 1 and 240");
    let directory = resolve_bundle_dir(bundle_dir)?;
    let video = load_bundle(&directory)
        .ok()
        .and_then(|(_, bundle)| bundle.video)
        .map_or_else(|| directory.join("recording.mp4"), |v| directory.join(v));
    if video.is_file() && from_ms < to_ms {
        let output_dir = directory
            .join("zoom")
            .join(format!("range-{from_ms}-{to_ms}"));
        if let Some(ffmpeg) = on_path("ffmpeg") {
            fs::create_dir_all(&output_dir)?;
            // Only remove our generated files, so repeating at a lower FPS cannot
            // return leftover frames from a previous extraction.
            for entry in fs::read_dir(&output_dir)? {
                let entry = entry?;
                if generated_frame(&entry.path()) {
                    fs::remove_file(entry.path())?;
                }
            }
            let output = Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-nostdin",
                    "-y",
                    "-ss",
                ])
                .arg(format!("{:.3}", from_ms as f64 / 1_000.0))
                .args(["-threads", "1", "-filter_threads", "1"])
                .arg("-i")
                .arg(&video)
                .arg("-t")
                .arg(format!("{:.3}", (to_ms - from_ms) as f64 / 1_000.0))
                .arg("-vf")
                .arg(format!("fps={fps}"))
                .args(["-threads", "1"])
                .arg(output_dir.join("frame-%03d.png"))
                .output()
                .context("Could not start ffmpeg")?;
            ensure!(
                output.status.success(),
                "ffmpeg extraction failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            let mut frames = Vec::new();
            for entry in fs::read_dir(&output_dir)? {
                let path = entry?.path();
                if generated_frame(&path) {
                    frames.push(path);
                }
            }
            frames.sort();
            return Ok(frames);
        }

        let step_ms = (1_000 / u64::from(fps)).max(1);
        let times: Vec<u64> = (from_ms..=to_ms).step_by(step_ms as usize).collect();
        if !times.is_empty()
            && let Ok(decoded) =
                crate::helper::frames(&video, crate::helper::FrameTimes::At(&times), 0)
            && !decoded.is_empty()
        {
            fs::create_dir_all(&output_dir)?;
            for entry in fs::read_dir(&output_dir)? {
                let entry = entry?;
                if generated_frame(&entry.path()) {
                    fs::remove_file(entry.path())?;
                }
            }
            let mut written = Vec::with_capacity(decoded.len());
            for (idx, frame) in decoded.iter().enumerate() {
                let out_path = output_dir.join(format!("frame-{:03}.png", idx + 1));
                crate::render::save_png(frame, &out_path)?;
                written.push(out_path);
            }
            return Ok(written);
        }
    }
    existing_keyframes(&directory, from_ms, to_ms, fps)
}

fn generated_frame(path: &Path) -> bool {
    path.is_file()
        && path.extension().is_some_and(|ext| ext == "png")
        && path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("frame-"))
            .is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
}

fn existing_keyframes(
    directory: &Path,
    from_ms: u64,
    to_ms: u64,
    fps: u32,
) -> Result<Vec<PathBuf>> {
    let (_, bundle) = load_bundle(directory)?;
    let margin = 1_000_u64.div_ceil(u64::from(fps)) / 2;
    let mut frames = Vec::new();
    for frame in bundle.keyframes {
        if frame.t < from_ms.saturating_sub(margin) || frame.t > to_ms.saturating_add(margin) {
            continue;
        }
        let relative = Path::new(&frame.file);
        ensure!(
            !relative.is_absolute()
                && relative.components().all(|part| matches!(
                    part,
                    std::path::Component::Normal(_) | std::path::Component::CurDir
                )),
            "Invalid keyframe path"
        );
        let path = directory.join(relative);
        if generated_frame(&path) {
            let canonical = path.canonicalize()?;
            ensure!(
                canonical.starts_with(directory.join("frames")),
                "Keyframe is outside bundle frames directory"
            );
            frames.push((frame.t, canonical));
        }
    }
    frames.sort_unstable();
    frames.dedup_by(|a, b| a.1 == b.1);
    Ok(frames.into_iter().map(|(_, path)| path).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "scrawl-handoff-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn bundle(&self, name: &str, created: u64) -> PathBuf {
            let path = self.0.join(format!("{name}.scrawl"));
            fs::create_dir_all(path.join("frames")).unwrap();
            let keyframes: Vec<_> = [0, 1_900, 2_500, 4_000, 5_100, 6_000].into_iter().enumerate().map(|(index, t)| {
                let file = format!("frames/frame-{index:03}.png");
                fs::write(path.join(&file), b"fixture").unwrap();
                serde_json::json!({"index": index, "t": t, "reason": "initial", "file": file, "event": ""})
            }).collect();
            let bundle = serde_json::json!({
                "version": 1, "id": name, "createdAtMs": created, "durationMs": 7_000,
                "target": {"mode": "display", "displayId": 1,
                    "frame": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 100.0, "scale": 1.0}},
                "transcript": {"source": "none", "text": "", "segments": []},
                "strokes": [], "marks": [], "events": [], "keyframes": keyframes,
                "contactSheet": "contact-sheet.png"
            });
            fs::write(
                path.join("scrawl.json"),
                serde_json::to_vec(&bundle).unwrap(),
            )
            .unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_directory_manifest_and_newest_capture() {
        let fixture = Fixture::new();
        let older = fixture.bundle("old", 100);
        let newer = fixture.bundle("new", 200);
        assert_eq!(
            resolve_bundle_dir(&older).unwrap(),
            older.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_bundle_dir(&older.join("scrawl.json")).unwrap(),
            older.canonicalize().unwrap()
        );
        assert_eq!(
            newest_bundle(&fixture.0).unwrap(),
            newer.canonicalize().unwrap()
        );
        assert!(resolve_bundle_dir(&fixture.0).is_err());
        assert!(resolve_bundle_dir(&fixture.0.join("missing.scrawl")).is_err());
    }

    #[test]
    fn loads_manifest_and_reports_invalid_json() {
        let fixture = Fixture::new();
        let path = fixture.bundle("example", 123);
        let (resolved, bundle) = load_bundle(&path.join("scrawl.json")).unwrap();
        assert_eq!(resolved, path.canonicalize().unwrap());
        assert_eq!(bundle.id, "example");
        assert_eq!(bundle.created_at_ms, 123);
        fs::write(path.join("scrawl.json"), b"not json").unwrap();
        assert!(load_bundle(&path).is_err());
    }

    #[test]
    fn frame_fallback_uses_manifest_times_not_file_indices() {
        let fixture = Fixture::new();
        let path = fixture.bundle("example", 123);
        let frames = extract_frames_range(&path, 2_000, 5_000, 4).unwrap();
        let names: Vec<_> = frames
            .iter()
            .map(|frame| frame.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "frame-001.png",
                "frame-002.png",
                "frame-003.png",
                "frame-004.png"
            ]
        );
        assert!(
            extract_frames_range(&path, 10_000, 11_000, 4)
                .unwrap()
                .is_empty()
        );
        assert!(extract_frames_range(&path, 5_000, 2_000, 4).is_err());
        assert!(extract_frames_range(&path, 0, 2_000, 0).is_err());
    }
}
