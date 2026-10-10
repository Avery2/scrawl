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
pub fn copy_bundle_to_clipboard(bundle_dir: &Path) -> Result<()> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSData, NSString};

    let (directory, bundle) = load_bundle(bundle_dir)?;
    let image =
        fs::read(directory.join(&bundle.contact_sheet)).context("Could not read contact sheet")?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);
    let data = NSData::with_bytes(&image);
    let text = NSString::from_str(&summary);
    let pasteboard = NSPasteboard::generalPasteboard();
    // These AppKit constants are immutable; no owner is needed for eagerly supplied data.
    unsafe {
        let types = NSArray::from_slice(&[NSPasteboardTypePNG, NSPasteboardTypeString]);
        pasteboard.clearContents();
        pasteboard.declareTypes_owner(&types, None);
        ensure!(
            pasteboard.setData_forType(Some(&data), NSPasteboardTypePNG),
            "Could not copy contact sheet"
        );
        ensure!(
            pasteboard.setString_forType(&text, NSPasteboardTypeString),
            "Could not copy capture summary"
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn copy_bundle_to_clipboard(_bundle_dir: &Path) -> Result<()> {
    bail!("Image and text clipboard handoff requires macOS")
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

fn opencode_api(method: &str, path: &str, data: Option<&serde_json::Value>) -> Result<Vec<u8>> {
    let mut command = Command::new(opencode_binary()?);
    command.args(["api", method, path]);
    if let Some(data) = data {
        command.args(["--data", &serde_json::to_string(data)?]);
    }
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

pub fn send_to_opencode(bundle_dir: &Path, session_id: Option<&str>) -> Result<String> {
    let (directory, bundle) = load_bundle(bundle_dir)?;
    let summary = crate::sketch::summary_markdown(&bundle, &directory);
    let session = if let Some(id) = session_id {
        id.to_owned()
    } else if let Some(id) = std::env::var("OPENCODE_SESSION_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
    {
        id
    } else {
        list_opencode_sessions()?
            .into_iter()
            .find(|session| session.parent_id.is_none() && session.time.archived.is_none())
            .context("No root OpenCode session found; pass a session ID")?
            .id
    };
    ensure!(
        session.starts_with("ses")
            && session
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Invalid OpenCode session ID"
    );
    let quoted_directory = format!("'{}'", directory.to_string_lossy().replace('\'', "'\\''"));
    let text = format!(
        "{}\n\nOpen the contact sheet at {} and read {} for timestamped transcript, marks, and interactions.\nFor a closer look at any interval, run:\nscrawl frames {} --from <start> --to <end> --fps 4\nReplace <start> and <end> with M:SS.mmm timestamps within this {} ms capture. Inspect the returned PNG files before acting on the feedback.",
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
    Ok(session)
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
    let video = directory.join("recording.mp4");
    if video.is_file() && from_ms < to_ms {
        if let Some(ffmpeg) = on_path("ffmpeg") {
            let output_dir = directory
                .join("zoom")
                .join(format!("range-{from_ms}-{to_ms}"));
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
