//! `scrawl autocrop`: an experiment in cropping full-screen captures to where the action was.
//!
//! Screen-mode captures log the on-screen windows and the cursor (`src/windows.rs`). This module
//! only exports a bundle's activity, windows and narration into `autocrop/data.json` and writes
//! a self-contained playground (`autocrop/index.html`) where the crop algorithm runs in JS,
//! recomputed live from sliders. Once Kit picks settings there ("Copy settings"), the chosen
//! algorithm gets ported to Rust (here, or in `src/keyframes.rs`) and applied when a Screen
//! capture finishes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::model::{InteractionEvent, KeyframeReason, RgbaFrame, ScrawlBundle, Shape};

const PLAYGROUND: &str = include_str!("autocrop.html");
const DATA_MARKER: &str = "/*SCRAWL_AUTOCROP_DATA*/null";

/// Writes `autocrop/data.json` and `autocrop/index.html` into the bundle and returns the page.
pub fn export(dir: &Path, bundle: &ScrawlBundle) -> Result<PathBuf> {
    let frames: Vec<Option<RgbaFrame>> = bundle
        .keyframes
        .iter()
        .map(|kf| {
            let file = std::fs::File::open(dir.join(&kf.file)).ok()?;
            crate::render::decode_png(std::io::BufReader::new(file)).ok()
        })
        .collect();
    let data = data(dir, bundle, &frames);
    let out = dir.join("autocrop");
    std::fs::create_dir_all(&out).with_context(|| format!("Could not create {}", out.display()))?;
    std::fs::write(out.join("data.json"), serde_json::to_vec_pretty(&data)?)?;
    // Inlined because browsers won't `fetch` a sibling file from a page opened from disk.
    let inline = serde_json::to_string(&data)?.replace("</", "<\\/");
    let page = out.join("index.html");
    std::fs::write(&page, PLAYGROUND.replace(DATA_MARKER, &inline))?;
    Ok(page)
}

fn r4(v: f32) -> f64 {
    (f64::from(v) * 10_000.0).round() / 10_000.0
}

/// Everything the playground needs, in capture-normalized coordinates (`0.0..=1.0`).
pub fn data(dir: &Path, bundle: &ScrawlBundle, frames: &[Option<RgbaFrame>]) -> Value {
    let target = &bundle.target;
    let (sw, sh) = (target.frame.width.max(1.0), target.frame.height.max(1.0));
    let frame_size = frames
        .iter()
        .flatten()
        .next()
        .map(|f| json!({"width": f.width, "height": f.height}));

    let strokes: Vec<Value> = bundle
        .strokes
        .iter()
        .map(|s| {
            let pts = s.timed_points();
            let (mut x0, mut y0, mut x1, mut y1) = (1.0_f32, 1.0_f32, 0.0_f32, 0.0_f32);
            for &(x, y, _) in &pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            json!({
                "t0": s.t0, "t1": s.t1, "laser": s.laser, "pin": s.pin, "on": s.on,
                "box": [r4(x0), r4(y0), r4(x1), r4(y1)],
                "pts": pts.iter().map(|&(x, y, t)| json!([r4(x), r4(y), t])).collect::<Vec<_>>(),
            })
        })
        .collect();

    let mut clicks = Vec::new();
    let mut drags = Vec::new();
    let mut scrolls = Vec::new();
    let mut points = Vec::new();
    let mut keys = Vec::new();
    for event in &bundle.events {
        match event {
            InteractionEvent::Click {
                t,
                x,
                y,
                button,
                label,
                ..
            } => clicks
                .push(json!({"t": t, "x": r4(*x), "y": r4(*y), "button": button, "label": label})),
            InteractionEvent::Drag {
                t,
                t1,
                x0,
                y0,
                x1,
                y1,
                path,
                label,
                ..
            } => drags.push(json!({
                "t": t, "t1": t1, "x0": r4(*x0), "y0": r4(*y0), "x1": r4(*x1), "y1": r4(*y1),
                "path": path.iter().map(|p| json!([r4(p[0]), r4(p[1])])).collect::<Vec<_>>(),
                "label": label,
            })),
            InteractionEvent::Scroll { t, x, y, .. } => {
                scrolls.push(json!({"t": t, "x": r4(*x), "y": r4(*y)}))
            }
            InteractionEvent::Point { t, x, y, .. } => {
                points.push(json!({"t": t, "x": r4(*x), "y": r4(*y)}))
            }
            InteractionEvent::Key { t, shortcut } => {
                keys.push(json!({"t": t, "shortcut": shortcut}))
            }
            InteractionEvent::AppChange { .. } | InteractionEvent::AppEvent { .. } => {}
        }
    }

    let windows: Vec<Value> = bundle
        .windows
        .iter()
        .map(|snap| {
            json!({
                "t": snap.t,
                "windows": snap.windows.iter().map(|w| {
                    let [x, y, width, height] = w.bounds.map(f64::from);
                    let n = |v: f64, d: f64| (v / d * 10_000.0).round() / 10_000.0;
                    json!({
                        "id": w.id, "app": w.app, "bundleId": w.bundle_id, "title": w.title,
                        "regular": w.regular,
                        "layer": w.layer, "frontmost": w.frontmost,
                        "box": [n(x, sw), n(y, sh), n(x + width, sw), n(y + height, sh)],
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let keyframes: Vec<Value> = bundle
        .keyframes
        .iter()
        .enumerate()
        .map(|(i, kf)| {
            let size = frames
                .get(i)
                .and_then(Option::as_ref)
                .map(|f| json!({"width": f.width, "height": f.height}));
            let changes = match (i.checked_sub(1).and_then(|p| frames.get(p)), frames.get(i)) {
                (Some(Some(prev)), Some(Some(cur))) => change_regions(prev, cur)
                    .iter()
                    .map(|b| json!(b.map(r4)))
                    .collect(),
                _ => Vec::new(),
            };
            json!({
                "index": kf.index,
                "t": kf.t,
                "reason": kf.reason,
                "event": kf.event,
                "said": kf.said,
                "markId": kf.mark_id,
                "cursor": kf.cursor.map(|c| c.map(r4)),
                "file": format!("../{}", kf.file),
                "path": dir.join(&kf.file),
                "size": size,
                "justifiedBy": justification(bundle, kf),
                "changes": changes,
            })
        })
        .collect();

    let marks: Vec<Value> = bundle
        .marks
        .iter()
        .map(|m| {
            json!({
                "id": m.id, "t0": m.t0, "t1": m.t1, "shape": m.shape, "strokes": m.strokes,
                "box": m.box_norm.map(r4), "description": m.description,
            })
        })
        .collect();

    json!({
        "version": 1,
        "id": bundle.id,
        "bundle": dir,
        "durationMs": bundle.duration_ms,
        "mode": target.mode,
        "source": bundle.source,
        "app": target.app,
        "screen": {
            "x": target.frame.x, "y": target.frame.y,
            "width": sw, "height": sh,
            "frame": frame_size,
        },
        "hasWindowLog": !bundle.windows.is_empty(),
        "keyframes": keyframes,
        "strokes": strokes,
        "marks": marks,
        "clicks": clicks,
        "drags": drags,
        "scrolls": scrolls,
        "points": points,
        "keys": keys,
        "cursor": bundle.cursor.iter().map(|c| json!([c.0, r4(c.1), r4(c.2)])).collect::<Vec<_>>(),
        "windows": windows,
        "words": bundle.transcript.words,
        "transcript": bundle.transcript.text,
    })
}

/// The activity a keyframe was cut for: its mark's strokes, or the click, drag or laser point
/// closest to it.
fn justification(bundle: &ScrawlBundle, kf: &crate::model::Keyframe) -> Value {
    let nearest = |pred: &dyn Fn(&InteractionEvent) -> bool| {
        bundle
            .events
            .iter()
            .enumerate()
            .filter(|(_, e)| pred(e))
            .map(|(i, e)| (i, e.timestamp_ms().abs_diff(kf.t)))
            .filter(|&(_, d)| d <= 800)
            .min_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
    };
    match kf.reason {
        KeyframeReason::Mark => {
            let mark = kf
                .mark_id
                .as_ref()
                .and_then(|id| bundle.marks.iter().find(|m| &m.id == id));
            match mark {
                Some(m) if m.shape == Shape::Flag => json!({"kind": "flag", "t": m.t0}),
                Some(m) => json!({"kind": "mark", "markId": m.id, "strokes": m.strokes}),
                None => json!({"kind": "mark"}),
            }
        }
        KeyframeReason::Click => {
            match nearest(&|e| {
                matches!(
                    e,
                    InteractionEvent::Click { .. } | InteractionEvent::Drag { .. }
                )
            }) {
                Some(i) => json!({"kind": "click", "event": bundle.events[i]}),
                None => json!({"kind": "click"}),
            }
        }
        KeyframeReason::Laser => match nearest(&|e| matches!(e, InteractionEvent::Point { .. })) {
            Some(i) => json!({"kind": "laser", "event": bundle.events[i]}),
            None => json!({"kind": "laser"}),
        },
        KeyframeReason::SceneChange => json!({"kind": "scene_change"}),
        KeyframeReason::Speech => json!({"kind": "speech"}),
        KeyframeReason::Initial => json!({"kind": "initial"}),
    }
}

/// Coarse regions that differ between two keyframes, as normalized `[x0, y0, x1, y1]` boxes,
/// largest first.
pub fn change_regions(prev: &RgbaFrame, cur: &RgbaFrame) -> Vec<[f32; 4]> {
    const COLS: usize = 64;
    if prev.width != cur.width || prev.height != cur.height || cur.width < COLS as u32 {
        return Vec::new();
    }
    let (w, h) = (cur.width as usize, cur.height as usize);
    let cell = w / COLS;
    let rows = (h / cell).max(1);
    let mut changed = vec![false; COLS * rows];
    for row in 0..rows {
        for col in 0..COLS {
            let (mut diff, mut n) = (0_u64, 0_u64);
            for y in (row * cell..((row + 1) * cell).min(h)).step_by(2) {
                for x in (col * cell..(col + 1) * cell).step_by(2) {
                    let i = (y * w + x) * 4;
                    let luma = |p: &[u8]| {
                        (u32::from(p[0]) * 3 + u32::from(p[1]) * 6 + u32::from(p[2])) / 10
                    };
                    diff +=
                        u64::from(luma(&prev.rgba[i..i + 3]).abs_diff(luma(&cur.rgba[i..i + 3])));
                    n += 1;
                }
            }
            // Scrawl's keyframe badge sits in a top corner and differs on every frame.
            let badge =
                row * cell < h * 6 / 100 && !(COLS * 6 / 100..COLS * 94 / 100).contains(&col);
            changed[row * COLS + col] = !badge && n > 0 && diff / n >= 10;
        }
    }
    // Connected changed cells (8-neighbour) become one box; lone cells are noise.
    let mut seen = vec![false; changed.len()];
    let mut boxes = Vec::new();
    for start in 0..changed.len() {
        if !changed[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let (mut c0, mut r0, mut c1, mut r1, mut count) = (COLS, rows, 0, 0, 0);
        while let Some(i) = stack.pop() {
            let (r, c) = (i / COLS, i % COLS);
            (c0, r0, c1, r1) = (c0.min(c), r0.min(r), c1.max(c), r1.max(r));
            count += 1;
            for dr in -1_i32..=1 {
                for dc in -1_i32..=1 {
                    let (nr, nc) = (r as i32 + dr, c as i32 + dc);
                    if nr < 0 || nc < 0 || nr >= rows as i32 || nc >= COLS as i32 {
                        continue;
                    }
                    let j = nr as usize * COLS + nc as usize;
                    if changed[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if count >= 2 {
            boxes.push((
                (c1 - c0 + 1) * (r1 - r0 + 1),
                [
                    (c0 * cell) as f32 / w as f32,
                    (r0 * cell) as f32 / h as f32,
                    (((c1 + 1) * cell).min(w)) as f32 / w as f32,
                    (((r1 + 1) * cell).min(h)) as f32 / h as f32,
                ],
            ));
        }
    }
    boxes.sort_by_key(|b| std::cmp::Reverse(b.0));
    boxes.into_iter().take(8).map(|(_, b)| b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CursorSample, WindowSnapshot};
    use crate::windows::WindowEntry;

    fn screen_bundle() -> ScrawlBundle {
        let mut bundle: ScrawlBundle = serde_json::from_value(json!({
            "version": 1, "id": "t", "createdAtMs": 0, "durationMs": 9_000,
            "target": {"mode": "display", "displayId": 1,
                "frame": {"x": 0.0, "y": 0.0, "width": 1000.0, "height": 500.0, "scale": 2.0}},
            "transcript": {"source": "test", "text": "look here", "segments": [],
                "words": [{"t0": 100, "t1": 300, "text": "look"}, {"t0": 300, "t1": 500, "text": "here"}]},
            "strokes": [{"t0": 2_000, "t1": 2_400, "fw": 1000, "fh": 500,
                "pts": [1_000, 2_000, 0, 3_000, 4_000, 400]}],
            "marks": [{"id": "m1", "strokes": [0], "t0": 2_000, "t1": 2_400,
                "boxNorm": [0.1, 0.2, 0.3, 0.4], "shape": "line", "description": "line"}],
            "events": [
                {"kind": "click", "t": 1_000, "x": 0.5, "y": 0.25},
                {"kind": "drag", "t": 3_000, "t1": 3_500, "x0": 0.1, "y0": 0.1, "x1": 0.2, "y1": 0.2},
                {"kind": "key", "t": 4_000, "shortcut": "⌘S"}
            ],
            "keyframes": [
                {"index": 1, "t": 1_000, "reason": "click", "file": "frames/frame-01.png", "event": "click"},
                {"index": 2, "t": 2_400, "reason": "mark", "file": "frames/frame-02.png", "event": "line", "markId": "m1"}
            ],
            "contactSheet": "contact-sheet.png"
        }))
        .unwrap();
        bundle.windows = vec![WindowSnapshot {
            t: 0,
            windows: vec![WindowEntry {
                id: 7,
                app: "Safari".into(),
                bundle_id: Some("com.apple.Safari".into()),
                regular: true,
                title: "Docs".into(),
                bounds: [100, 50, 500, 250],
                layer: 0,
                frontmost: true,
            }],
        }];
        bundle.cursor = vec![CursorSample(0, 0.5, 0.5), CursorSample(66, 0.52, 0.5)];
        bundle
    }

    #[test]
    fn export_data_has_the_shape_the_playground_reads() {
        let bundle = screen_bundle();
        let frames = vec![
            Some(RgbaFrame::new(0, 128, 64, vec![0; 128 * 64 * 4])),
            Some(RgbaFrame::new(0, 128, 64, {
                let mut rgba = vec![0; 128 * 64 * 4];
                for y in 0..16 {
                    for x in 64..96 {
                        rgba[(y * 128 + x) * 4..][..3].copy_from_slice(&[255, 255, 255]);
                    }
                }
                rgba
            })),
        ];
        let data = data(Path::new("/b.scrawl"), &bundle, &frames);
        assert_eq!(data["screen"]["width"], 1000.0);
        assert_eq!(data["screen"]["frame"], json!({"width": 128, "height": 64}));
        assert_eq!(data["hasWindowLog"], true);

        let kf = &data["keyframes"];
        assert_eq!(kf[0]["file"], "../frames/frame-01.png");
        assert_eq!(kf[0]["path"], "/b.scrawl/frames/frame-01.png");
        assert_eq!(kf[0]["justifiedBy"]["kind"], "click");
        assert_eq!(kf[0]["justifiedBy"]["event"]["t"], 1_000);
        assert_eq!(kf[0]["changes"], json!([]));
        assert_eq!(
            kf[1]["justifiedBy"],
            json!({"kind": "mark", "markId": "m1", "strokes": [0]})
        );
        assert_eq!(kf[1]["changes"], json!([[0.5, 0.0, 0.75, 0.25]]));

        assert_eq!(data["strokes"][0]["box"], json!([0.1, 0.2, 0.3, 0.4]));
        assert_eq!(data["strokes"][0]["pts"][1], json!([0.3, 0.4, 2_400]));
        assert_eq!(
            data["clicks"][0],
            json!({"t": 1_000, "x": 0.5, "y": 0.25, "button": null, "label": null})
        );
        assert_eq!(data["drags"][0]["x1"], 0.2);
        assert_eq!(data["keys"][0]["shortcut"], "⌘S");
        assert_eq!(data["cursor"][1], json!([66, 0.52, 0.5]));
        let window = &data["windows"][0]["windows"][0];
        assert_eq!(window["box"], json!([0.1, 0.1, 0.6, 0.6]));
        assert_eq!(window["app"], "Safari");
        assert_eq!(window["frontmost"], true);
        assert_eq!(data["words"][1]["text"], "here");
    }

    #[test]
    fn bundles_without_a_window_log_or_frames_still_export() {
        let mut bundle = screen_bundle();
        bundle.windows.clear();
        bundle.cursor.clear();
        let data = data(Path::new("/b.scrawl"), &bundle, &[None, None]);
        assert_eq!(data["hasWindowLog"], false);
        assert_eq!(data["windows"], json!([]));
        assert_eq!(data["cursor"], json!([]));
        assert_eq!(data["screen"]["frame"], Value::Null);
        assert_eq!(data["keyframes"][1]["size"], Value::Null);
    }

    #[test]
    fn exported_page_inlines_data_safely() {
        let dir = std::env::temp_dir().join(crate::make_bundle_id());
        let mut bundle = screen_bundle();
        bundle.transcript.text = "</script><b>".into();
        let page = export(&dir, &bundle).unwrap();
        let html = std::fs::read_to_string(&page).unwrap();
        assert!(!html.contains(DATA_MARKER));
        assert!(html.contains("<\\/script><b>"));
        assert!(dir.join("autocrop/data.json").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
