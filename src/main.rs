mod capture;
mod handoff;
mod hex;
mod keyframes;
mod laser_overlay;
mod model;
mod render;
mod sketch;
mod ui;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::capture::{ScreenCaptureSession, discover_target, write_bundle_artifacts};
use crate::hex::{AudioRecorder, RecordedAudio};
use crate::keyframes::select_keyframes;
use crate::model::{
    CaptureMode, InteractionEvent, MAX_KEYFRAMES, RgbaFrame, SCALE, ScrawlBundle, Stroke,
    TargetFrame, TargetInfo, TranscriptData, TranscriptSegment, WordTiming, parse_clock_ms,
};
use crate::render::{
    annotate_keyframe_in_place, render_contact_sheet, render_synthetic_demo_frame,
};
use crate::sketch::{marks_of, summary_markdown};

#[derive(Parser)]
#[command(
    name = "scrawl",
    version,
    about = "Shortcut-triggered screen, voice, and drawing capture for agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Launch the GPUI menu-bar and review application (default).
    App {
        /// Start a capture session immediately on launch.
        #[arg(long)]
        record: bool,
    },
    /// Generate a realistic annotated demo bundle (contact sheet + keyframes + transcript).
    Demo {
        /// Output `.scrawl` bundle directory (defaults to Application Support).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Copy the generated bundle to the macOS clipboard.
        #[arg(long)]
        clipboard: bool,
    },
    /// Record a live screen + microphone capture from the terminal for a fixed duration.
    Record {
        /// Optional window title or app substring to target.
        #[arg(long)]
        window: Option<String>,
        /// Recording duration in seconds (default: 5).
        #[arg(long, default_value_t = 5)]
        duration: u64,
        /// Optional note to attach to the bundle.
        #[arg(long)]
        note: Option<String>,
        /// Output `.scrawl` bundle directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Print the agent-ready summary or JSON manifest for a `.scrawl` bundle (or `latest`).
    Inspect {
        /// Path to `.scrawl` bundle directory, `scrawl.json`, or `latest`.
        #[arg(default_value = "latest")]
        bundle: PathBuf,
        /// Emit raw `scrawl.json` instead of `summary.md`.
        #[arg(long)]
        json: bool,
    },
    /// Extract a dense burst of frames from a `.scrawl` bundle's time range (`M:SS` or seconds).
    Frames {
        /// Path to `.scrawl` bundle directory or `latest`.
        bundle: PathBuf,
        /// Start time (`0:02` or `2.0`).
        #[arg(long)]
        from: String,
        /// End time (`0:05` or `5.0`).
        #[arg(long)]
        to: String,
        /// Frames per second to extract when `recording.mp4` is present.
        #[arg(long, default_value_t = 4)]
        fps: u32,
    },
    /// Copy a `.scrawl` bundle to the clipboard or send it to an OpenCode V2 session.
    Send {
        /// Path to `.scrawl` bundle directory or `latest`.
        #[arg(default_value = "latest")]
        bundle: PathBuf,
        /// Target OpenCode session ID (`ses_...`). If omitted, uses the most recent active session.
        #[arg(long)]
        session: Option<String>,
        /// Copy to the macOS clipboard instead of posting to OpenCode.
        #[arg(long)]
        clipboard: bool,
    },
    /// Open a `.scrawl` bundle in the native GPUI review window.
    Preview {
        /// Path to `.scrawl` bundle directory or `latest`. Generates a demo bundle if none exists.
        #[arg(default_value = "latest")]
        bundle: PathBuf,
        /// Capture a window screenshot to this path and exit (for deterministic UI verification).
        #[arg(long)]
        capture_screenshot: Option<PathBuf>,
    },
}

pub fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn make_bundle_id() -> String {
    let ms = now_epoch_ms();
    let pid = std::process::id();
    format!("scrawl-{ms}-{pid:x}")
}

/// Core pipeline that turns raw frames, strokes, events, and audio/transcript into a complete `.scrawl` bundle on disk.
#[allow(clippy::too_many_arguments)]
pub fn finalize_capture_bundle(
    bundle_dir: &Path,
    target: TargetInfo,
    duration_ms: u64,
    frames: Vec<RgbaFrame>,
    strokes: Vec<Stroke>,
    events: Vec<InteractionEvent>,
    recorded_audio: Option<RecordedAudio>,
    video_path: Option<PathBuf>,
    note: Option<String>,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    if let Some(ref audio) = recorded_audio {
        let wav_path = bundle_dir.join("audio.wav");
        let _ = std::fs::create_dir_all(bundle_dir);
        let _ = std::fs::write(&wav_path, &audio.wav_bytes);
    }
    let transcript = hex::transcribe_with_hex(recorded_audio.as_ref());

    build_bundle_from_parts(
        bundle_dir,
        target,
        duration_ms,
        frames,
        strokes,
        events,
        transcript,
        video_path,
        note,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_bundle_from_parts(
    bundle_dir: &Path,
    target: TargetInfo,
    duration_ms: u64,
    frames: Vec<RgbaFrame>,
    strokes: Vec<Stroke>,
    events: Vec<InteractionEvent>,
    transcript: TranscriptData,
    video_path: Option<PathBuf>,
    note: Option<String>,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let mut marks = marks_of(&strokes);
    let bench = std::env::var_os("SCRAWL_BENCH").is_some();
    let started = std::time::Instant::now();
    let selected = select_keyframes(
        &frames,
        &strokes,
        &mut marks,
        &events,
        &transcript,
        MAX_KEYFRAMES,
    );
    let selected_at = std::time::Instant::now();

    let mut annotated_frames = Vec::with_capacity(selected.len());
    let mut keyframes = Vec::with_capacity(selected.len());
    for (kf, mut raw_frame) in selected {
        annotate_keyframe_in_place(&mut raw_frame, &kf, &strokes, &marks, &events);
        keyframes.push(kf.clone());
        annotated_frames.push((kf, raw_frame));
    }
    let annotated_at = std::time::Instant::now();

    let id = bundle_dir
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("scrawl-bundle")
        .to_string();

    let video_rel = video_path.and_then(|p| {
        p.exists()
            .then(|| p.file_name()?.to_str().map(str::to_owned))
            .flatten()
    });
    let audio_rel = bundle_dir
        .join("audio.wav")
        .exists()
        .then(|| "audio.wav".to_string());

    let bundle = ScrawlBundle {
        version: 1,
        id,
        created_at_ms: now_epoch_ms(),
        duration_ms,
        target,
        transcript,
        note,
        strokes,
        marks,
        events,
        keyframes,
        contact_sheet: "contact-sheet.png".into(),
        video: video_rel,
        audio: audio_rel,
    };

    let contact_sheet = render_contact_sheet(&bundle, &annotated_frames);
    let rendered_at = std::time::Instant::now();
    let summary = summary_markdown(&bundle, bundle_dir);
    write_bundle_artifacts(
        bundle_dir,
        &bundle,
        &annotated_frames,
        &contact_sheet,
        &summary,
    )?;
    if bench {
        let finished = std::time::Instant::now();
        for (name, elapsed) in [
            ("selection_ms", selected_at - started),
            ("annotation_ms", annotated_at - selected_at),
            ("contact_sheet_ms", rendered_at - annotated_at),
            ("artifacts_ms", finished - rendered_at),
            ("pipeline_ms", finished - started),
        ] {
            eprintln!("BENCH {name}={:.3}", elapsed.as_secs_f64() * 1000.0);
        }
    }

    Ok((bundle_dir.to_path_buf(), bundle, summary))
}

/// Generates a rich, deterministic multi-frame demo bundle showcasing clicks, circles, arrows, pins, and aligned speech.
pub fn generate_demo_bundle(out: Option<PathBuf>) -> Result<(PathBuf, ScrawlBundle, String)> {
    let bundle_dir = out.unwrap_or_else(|| {
        handoff::default_captures_dir().join(format!("{}.scrawl", make_bundle_id()))
    });
    let (fw, fh) = (1280_u32, 800_u32);
    let target = TargetInfo {
        mode: CaptureMode::Window,
        app: Some("OpenCode".into()),
        bundle_id: Some("ai.opencode.desktop".into()),
        window_title: Some("session/tool-running — OpenCode".into()),
        window_id: Some(4210),
        display_id: 1,
        frame: TargetFrame {
            x: 100.0,
            y: 60.0,
            width: f64::from(fw),
            height: f64::from(fh),
            scale: 2.0,
        },
    };

    let frames = vec![
        render_synthetic_demo_frame(fw, fh, 400, 0),
        render_synthetic_demo_frame(fw, fh, 1_500, 1),
        render_synthetic_demo_frame(fw, fh, 3_600, 1),
        render_synthetic_demo_frame(fw, fh, 6_400, 2),
        render_synthetic_demo_frame(fw, fh, 9_200, 2),
    ];

    // 1. Circle around the shifted header band (cx=0.52, cy=0.14, r_x=0.14, r_y=0.06)
    let mut circle_pts = Vec::new();
    for i in 0..=36 {
        let angle = (i as f32 / 36.0) * std::f32::consts::TAU;
        let x = ((0.52 + 0.13 * angle.cos()) * SCALE as f32).round() as i32;
        let y = ((0.15 + 0.055 * angle.sin()) * SCALE as f32).round() as i32;
        circle_pts.extend_from_slice(&[x, y, i * 18]);
    }
    let circle_stroke = Stroke {
        t0: 2_900,
        t1: 2_900 + 36 * 18,
        fw,
        fh,
        on: Some("Band header".into()),
        pin: None,
        laser: false,
        pts: circle_pts,
    };

    // 2. Arrow pointing from sidebar item to the tool card (shaft + arrowhead)
    let mut shaft_pts = Vec::new();
    for i in 0..=12 {
        let t = i as f32 / 12.0;
        let x = ((0.22 + (0.44 - 0.22) * t) * SCALE as f32).round() as i32;
        let y = ((0.48 + (0.36 - 0.48) * t) * SCALE as f32).round() as i32;
        shaft_pts.extend_from_slice(&[x, y, i * 16]);
    }
    let arrow_shaft = Stroke {
        t0: 5_800,
        t1: 5_800 + 12 * 16,
        fw,
        fh,
        on: Some("Used 12 tools".into()),
        pin: None,
        laser: false,
        pts: shaft_pts,
    };
    let arrow_head = Stroke {
        t0: 6_100,
        t1: 6_160,
        fw,
        fh,
        on: Some("Used 12 tools".into()),
        pin: None,
        laser: false,
        pts: vec![
            (0.41 * SCALE as f32) as i32,
            (0.35 * SCALE as f32) as i32,
            0,
            (0.44 * SCALE as f32) as i32,
            (0.36 * SCALE as f32) as i32,
            30,
            (0.42 * SCALE as f32) as i32,
            (0.40 * SCALE as f32) as i32,
            60,
        ],
    };

    // 3. Numbered pin on the composer send button
    let pin_stroke = Stroke {
        t0: 8_900,
        t1: 8_900,
        fw,
        fh,
        on: Some("Send button".into()),
        pin: Some(1),
        laser: false,
        pts: vec![
            (0.88 * SCALE as f32) as i32,
            (0.88 * SCALE as f32) as i32,
            0,
        ],
    };

    let strokes = vec![circle_stroke, arrow_shaft, arrow_head, pin_stroke];
    let events = vec![
        InteractionEvent::Click {
            t: 1_250,
            x: 0.46,
            y: 0.36,
            button: Some("left".into()),
            label: Some("Used 12 tools".into()),
            role: Some("AXButton".into()),
        },
        InteractionEvent::Scroll {
            t: 4_800,
            x: 0.55,
            y: 0.55,
            dx: 0.0,
            dy: -240.0,
            label: Some("Transcript".into()),
        },
    ];

    let transcript = TranscriptData {
        source: "hex".into(),
        model: Some("parakeet_unified_en".into()),
        text: "When I click Used 12 tools this band header jumps down by four pixels, and the row spacing under the card feels too tight.".into(),
        segments: vec![
            TranscriptSegment {
                t0: 600,
                t1: 4_100,
                text: "When I click Used 12 tools this band header jumps down by four pixels,".into(),
            },
            TranscriptSegment {
                t0: 5_400,
                t1: 9_400,
                text: "and the row spacing under the card feels too tight.".into(),
            },
        ],
        words: vec![
            WordTiming { t0: 600, t1: 850, text: "When".into() },
            WordTiming { t0: 850, t1: 980, text: "I".into() },
            WordTiming { t0: 980, t1: 1_320, text: "click".into() },
            WordTiming { t0: 1_320, t1: 1_650, text: "Used".into() },
            WordTiming { t0: 1_650, t1: 1_900, text: "12".into() },
            WordTiming { t0: 1_900, t1: 2_300, text: "tools".into() },
            WordTiming { t0: 2_450, t1: 2_700, text: "this".into() },
            WordTiming { t0: 2_700, t1: 3_050, text: "band".into() },
            WordTiming { t0: 3_050, t1: 3_400, text: "header".into() },
            WordTiming { t0: 3_400, t1: 3_700, text: "jumps".into() },
            WordTiming { t0: 3_700, t1: 3_900, text: "down".into() },
            WordTiming { t0: 3_900, t1: 4_100, text: "by four pixels,".into() },
            WordTiming { t0: 5_400, t1: 5_800, text: "and the".into() },
            WordTiming { t0: 5_800, t1: 6_300, text: "row spacing".into() },
            WordTiming { t0: 6_300, t1: 7_200, text: "under the card".into() },
            WordTiming { t0: 7_800, t1: 9_400, text: "feels too tight.".into() },
        ],
    };

    build_bundle_from_parts(
        &bundle_dir,
        target,
        9_800,
        frames,
        strokes,
        events,
        transcript,
        None,
        Some("Header inset shifts on expand".into()),
    )
}

fn main() -> Result<()> {
    if std::env::var_os("SCRAWL_BENCH_START").is_some() {
        ui::run_start_benchmark();
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::App { record: false }) {
        Command::App { record } => {
            ui::run_app(None, record, None);
            Ok(())
        }
        Command::Demo { out, clipboard } => {
            let (dir, _bundle, summary) = generate_demo_bundle(out)?;
            if clipboard {
                let _ = handoff::copy_bundle_to_clipboard(&dir);
            }
            print!("{summary}");
            Ok(())
        }
        Command::Record {
            window,
            duration,
            note,
            out,
        } => {
            let target = discover_target(window.as_deref());
            let bundle_dir = out.unwrap_or_else(|| {
                handoff::default_captures_dir().join(format!("{}.scrawl", make_bundle_id()))
            });
            std::fs::create_dir_all(&bundle_dir)?;
            let video_path = bundle_dir.join("recording.mp4");
            let audio = AudioRecorder::start().ok();
            let session = ScreenCaptureSession::start(
                target.clone(),
                Some(video_path),
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            );
            std::thread::sleep(Duration::from_secs(duration.max(1)));
            let (frames, video_out, elapsed_ms) = session.stop();
            let recorded_audio = audio.and_then(|a| a.stop().ok());
            let (_dir, _bundle, summary) = finalize_capture_bundle(
                &bundle_dir,
                target,
                elapsed_ms,
                frames,
                Vec::new(),
                Vec::new(),
                recorded_audio,
                video_out,
                note,
            )?;
            print!("{summary}");
            Ok(())
        }
        Command::Inspect { bundle, json } => {
            let (dir, loaded) = handoff::load_bundle(&bundle)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&loaded)?);
            } else {
                print!("{}", summary_markdown(&loaded, &dir));
            }
            Ok(())
        }
        Command::Frames {
            bundle,
            from,
            to,
            fps,
        } => {
            let from_ms =
                parse_clock_ms(&from).with_context(|| format!("invalid --from time: {from}"))?;
            let to_ms = parse_clock_ms(&to).with_context(|| format!("invalid --to time: {to}"))?;
            let extracted = handoff::extract_frames_range(&bundle, from_ms, to_ms, fps)?;
            for path in extracted {
                println!("{}", path.display());
            }
            Ok(())
        }
        Command::Send {
            bundle,
            session,
            clipboard,
        } => {
            let dir = handoff::resolve_bundle_dir(&bundle)?;
            if clipboard {
                handoff::copy_bundle_to_clipboard(&dir)?;
                println!("Copied {} to clipboard", dir.display());
            } else {
                let target_session = handoff::send_to_opencode(&dir, session.as_deref())?;
                println!(
                    "Sent {} to OpenCode session {target_session}",
                    dir.display()
                );
            }
            Ok(())
        }
        Command::Preview {
            bundle,
            capture_screenshot,
        } => {
            let initial = handoff::load_bundle(&bundle).ok().or_else(|| {
                generate_demo_bundle(None)
                    .ok()
                    .map(|(dir, loaded, _)| (dir, loaded))
            });
            ui::run_app(initial, false, capture_screenshot);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_failures_do_not_publish_a_manifest() {
        let root = std::env::temp_dir().join(make_bundle_id());
        let (_, mut bundle, _) = generate_demo_bundle(Some(root.join("demo.scrawl"))).unwrap();
        let frame = RgbaFrame::new(0, 1, 1, vec![20, 30, 40, 255]);
        let mut key = bundle.keyframes[0].clone();
        key.file = "frames/missing/frame.png".into();
        let frame_failure = root.join("frame-failure.scrawl");
        assert!(
            write_bundle_artifacts(
                &frame_failure,
                &bundle,
                &[(key, frame.clone())],
                &frame,
                "summary"
            )
            .is_err()
        );
        assert!(!frame_failure.join("scrawl.json").exists());

        bundle.contact_sheet = "missing/contact-sheet.png".into();
        let sheet_failure = root.join("sheet-failure.scrawl");
        assert!(write_bundle_artifacts(&sheet_failure, &bundle, &[], &frame, "summary").is_err());
        assert!(!sheet_failure.join("scrawl.json").exists());

        bundle.contact_sheet = "contact-sheet.png".into();
        let empty_frames = root.join("empty-frames.scrawl");
        write_bundle_artifacts(&empty_frames, &bundle, &[], &frame, "summary").unwrap();
        assert!(empty_frames.join("scrawl.json").exists());
        assert!(empty_frames.join("contact-sheet.png").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
