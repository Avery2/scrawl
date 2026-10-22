//! Screenshots, behind `"experiments": {"screenshots": true}` (macOS). The screenshot shortcut
//! (⌥⇧4) opens the picker in a still mode: drag an area, click a window, or click the screen.
//! The pick becomes one PNG at the display's own scale in a small bundle (`screenshot.png`,
//! `scrawl.json` with `source: "screenshot"`, `summary.md`; no audio, video or transcript), so it
//! joins the capture stack like a recording, and goes on the clipboard as the image plus its
//! path as text. Smart routing never looks at screenshots.

use std::path::Path;

use crate::capture::ShortcutConfig;
use crate::model::{
    Keyframe, KeyframeReason, ScrawlBundle, TargetFrame, TargetInfo, TranscriptData,
};

pub const SCREENSHOTS: &str = "screenshots";
/// `ScrawlBundle::source` of a screenshot.
pub const SOURCE: &str = "screenshot";
pub const FILE: &str = "screenshot.png";

/// The screenshot shortcut to register: the stored one while the experiment is on, else none.
pub fn registered(on: bool, stored: &ShortcutConfig) -> ShortcutConfig {
    if on {
        stored.clone()
    } else {
        ShortcutConfig::unset()
    }
}

/// The still's size in pixels: the target's points at its display's scale.
pub fn native_size(frame: &TargetFrame) -> (u32, u32) {
    let scale = if frame.scale > 0.0 { frame.scale } else { 1.0 };
    let px = |points: f64| ((points * scale).round() as u32).max(1);
    (px(frame.width), px(frame.height))
}

pub fn is_screenshot(bundle: &ScrawlBundle) -> bool {
    bundle.source.as_deref() == Some(SOURCE)
}

/// The manifest of a screenshot of `target`, `size` pixels.
pub fn bundle_for(id: String, created_at_ms: u64, target: TargetInfo) -> ScrawlBundle {
    ScrawlBundle {
        version: 1,
        id,
        created_at_ms,
        duration_ms: 0,
        target,
        transcript: TranscriptData::default(),
        note: None,
        strokes: Vec::new(),
        marks: Vec::new(),
        events: Vec::new(),
        keyframes: vec![Keyframe {
            index: 1,
            t: 0,
            reason: KeyframeReason::Initial,
            file: FILE.into(),
            event: "screenshot".into(),
            said: None,
            mark_id: None,
            cursor: None,
        }],
        contact_sheet: FILE.into(),
        video: None,
        video_offset_ms: 0,
        audio: None,
        source: Some(SOURCE.into()),
        device: None,
        context: Default::default(),
        context_changes: Vec::new(),
        screens: Vec::new(),
        windows: Vec::new(),
        cursor: Vec::new(),
        autocrop: None,
        destination: None,
    }
}

/// Writes a screenshot bundle into `dir` (created if needed) from the still's PNG bytes.
pub fn write_bundle(dir: &Path, target: TargetInfo, png: &[u8]) -> anyhow::Result<ScrawlBundle> {
    std::fs::create_dir_all(dir)?;
    let id = dir
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scrawl-screenshot")
        .to_owned();
    let bundle = bundle_for(id, crate::now_epoch_ms(), target);
    std::fs::write(dir.join(FILE), png)?;
    std::fs::write(dir.join("summary.md"), summary(&bundle, dir))?;
    let mut json = serde_json::to_vec_pretty(&bundle)?;
    json.push(b'\n');
    // The manifest last: a bundle with `scrawl.json` is complete.
    std::fs::write(dir.join("scrawl.json"), json)?;
    Ok(bundle)
}

/// What a screenshot's `summary.md` and pasted text say: what it shows and where it is.
pub fn summary(bundle: &ScrawlBundle, dir: &Path) -> String {
    let t = &bundle.target;
    let mut what = t.app.clone().unwrap_or_else(|| "Screen".into());
    if let Some(title) = t.window_title.as_deref().filter(|s| !s.is_empty())
        && Some(title) != t.app.as_deref()
    {
        what.push_str(&format!(" “{title}”"));
    }
    let (w, h) = native_size(&t.frame);
    format!(
        "Screenshot · {what} · {w}×{h}\n\nImage: {}\nBundle: {}\n",
        dir.join(FILE).display(),
        dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::ShortcutKind;
    use crate::model::CaptureMode;

    fn target() -> TargetInfo {
        TargetInfo {
            mode: CaptureMode::Region,
            app: Some("Brave Browser".into()),
            bundle_id: None,
            window_title: Some("OpenCode".into()),
            window_id: None,
            display_id: 1,
            frame: TargetFrame {
                x: 100.0,
                y: 50.0,
                width: 400.5,
                height: 300.0,
                scale: 2.0,
            },
        }
    }

    #[test]
    fn the_shortcut_is_registered_only_while_the_experiment_is_on() {
        let stored = ShortcutConfig::default_screenshot();
        assert_eq!(stored.label, "⌥⇧4");
        let off = registered(false, &stored);
        assert!(off.is_unset());
        assert!(crate::capture::hotkey_combos(ShortcutKind::Screenshot, &off).is_empty());
        let on = registered(true, &stored);
        // Just ⌥⇧4: no ⌃/⌘ aliases (⌘⇧4 is macOS's own).
        assert_eq!(
            crate::capture::hotkey_combos(ShortcutKind::Screenshot, &on),
            vec![(on.key_code, on.carbon_modifiers)]
        );
        let hit = |cmd, ctrl, alt, shift| {
            let code = on.key_code as u16;
            crate::capture::shortcut_matches(
                ShortcutKind::Screenshot,
                &on,
                code,
                code,
                cmd,
                ctrl,
                alt,
                shift,
            )
        };
        assert!(hit(false, false, true, true), "⌥⇧4");
        assert!(!hit(true, true, true, false), "⌃⌥⌘4 is not an alias");
        assert!(!hit(true, false, true, true), "⌘⌥⇧4 is not an alias");
    }

    #[test]
    fn the_default_is_free_and_clashes_are_refused() {
        use crate::capture::{CARBON_CMD_KEY, CARBON_OPTION_KEY, CARBON_SHIFT_KEY};
        use crate::settings::{Pressed, Rejection, validate_against};
        let shot = ShortcutConfig::default_screenshot();
        let record = ShortcutConfig::default_record();
        let paste = ShortcutConfig::default_paste();
        let others = [
            (ShortcutKind::Record, &record),
            (ShortcutKind::Paste, &paste),
        ];
        let pressed = |code: u32, modifiers, key: &str| Pressed {
            key_code: code,
            modifiers,
            key: key.into(),
            reserved: false,
        };
        let ok = validate_against(
            ShortcutKind::Screenshot,
            &pressed(shot.key_code, CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "4"),
            &others,
        );
        assert_eq!(ok, Ok(shot.clone()));
        assert!(matches!(
            validate_against(
                ShortcutKind::Screenshot,
                &pressed(shot.key_code, CARBON_CMD_KEY | CARBON_SHIFT_KEY, "4"),
                &others,
            ),
            Err(Rejection::Reserved(_))
        ));
        assert_eq!(
            validate_against(
                ShortcutKind::Screenshot,
                &pressed(record.key_code, record.carbon_modifiers, "s"),
                &others,
            ),
            Err(Rejection::Taken(ShortcutKind::Record))
        );
        // Turned off, it takes nothing from the others.
        let off = ShortcutConfig::unset();
        let with_off = [(ShortcutKind::Screenshot, &off)];
        assert!(
            validate_against(
                ShortcutKind::Paste,
                &pressed(paste.key_code, paste.carbon_modifiers, "v"),
                &with_off,
            )
            .is_ok()
        );
    }

    #[test]
    fn a_screenshot_is_one_png_in_a_small_bundle() {
        let dir = std::env::temp_dir().join(format!("{}.scrawl", crate::make_bundle_id()));
        let png = crate::render::encode_png(&crate::model::RgbaFrame::new(
            0,
            801,
            600,
            vec![200; 801 * 600 * 4],
        ))
        .unwrap();
        let bundle = write_bundle(&dir, target(), &png).unwrap();
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, ["scrawl.json", "screenshot.png", "summary.md"]);
        let (_, loaded) = crate::handoff::load_bundle(&dir).unwrap();
        assert_eq!(loaded, bundle);
        assert!(is_screenshot(&loaded));
        assert_eq!(loaded.contact_sheet, FILE);
        assert_eq!(loaded.keyframes.len(), 1);
        assert_eq!(loaded.keyframes[0].file, FILE);
        assert!(loaded.video.is_none() && loaded.audio.is_none());
        assert!(loaded.transcript.text.is_empty());
        assert_eq!(native_size(&loaded.target.frame), (801, 600));
        let summary = std::fs::read_to_string(dir.join("summary.md")).unwrap();
        assert!(summary.starts_with("Screenshot · Brave Browser “OpenCode” · 801×600"));
        assert!(summary.contains(&dir.join(FILE).display().to_string()));
        assert_eq!(crate::sketch::summary_markdown(&loaded, &dir), summary);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
