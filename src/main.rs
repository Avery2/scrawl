mod capture;
mod handoff;
mod helper;
mod hex;
mod keyframes;
mod laser_overlay;
mod model;
mod phone;
mod render;
mod sketch;
mod transcribe;
mod ui;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};

use crate::capture::{ScreenCaptureSession, discover_target, write_bundle_artifacts};
use crate::hex::{AudioRecorder, RecordedAudio};
use crate::keyframes::select_keyframes;
use crate::model::{
    CaptureMode, DeviceInfo, InteractionEvent, MAX_KEYFRAMES, Mark, RgbaFrame, SCALE, ScrawlBundle,
    Shape, Stroke, TargetFrame, TargetInfo, TranscriptData, TranscriptSegment, WordTiming,
    parse_clock_ms,
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
        /// Start capturing the cabled iPhone immediately on launch.
        #[arg(long, conflicts_with = "record")]
        iphone: bool,
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
        /// Capture the cabled iPhone instead of the screen (live mirror window + native video).
        #[arg(long, conflicts_with = "window")]
        iphone: bool,
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
    /// Build a bundle from a TetherKit in-app recording (`.mp4` + timeline `.json`).
    Import {
        /// The recording or its timeline; defaults to the newest pair in ~/Downloads.
        recording: Option<PathBuf>,
        /// Timeline JSON, when it isn't beside the recording with the same name.
        #[arg(long)]
        timeline: Option<PathBuf>,
        /// Optional note to attach to the bundle.
        #[arg(long)]
        note: Option<String>,
        /// Output `.scrawl` bundle directory.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Copy the summary to the clipboard.
        #[arg(long)]
        clipboard: bool,
    },
    /// List cabled iPhones and iPads that can be captured.
    Devices,
    /// Rebuild a bundle from its own video, audio, and input log with the current transcriber
    /// and keyframe rules.
    Reprocess {
        /// Path to `.scrawl` bundle directory or `latest`.
        bundle: PathBuf,
        /// Output `.scrawl` bundle directory (a new bundle; the original is untouched).
        #[arg(long)]
        out: PathBuf,
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
        /// Frames per second to extract when the bundle has a video.
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

/// What a source other than this Mac's screen adds to a bundle.
#[derive(Default)]
pub struct SourceExtras {
    pub source: Option<String>,
    pub device: Option<DeviceInfo>,
    /// Time-only markers (ms); each becomes a `Flag` mark with keyframe priority.
    pub flags: Vec<u64>,
    /// Decodes the chosen keyframes again at full quality, for sources that select on small
    /// candidate frames (phone movies).
    #[allow(clippy::type_complexity)]
    pub full_frames: Option<Box<dyn Fn(&[u64]) -> Result<Vec<RgbaFrame>>>>,
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
    let wav_path = bundle_dir.join("audio.wav");
    let transcript = transcribe::transcribe(
        wav_path.exists().then_some(wav_path.as_path()),
        recorded_audio.as_ref(),
    );

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
        SourceExtras::default(),
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
    extras: SourceExtras,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let mut marks = marks_of(&strokes);
    for (i, &t) in extras.flags.iter().enumerate() {
        marks.push(Mark {
            id: format!("f{}", i + 1),
            strokes: Vec::new(),
            t0: t,
            t1: t,
            box_norm: [0.0, 0.0, 1.0, 1.0],
            shape: Shape::Flag,
            from: None,
            to: None,
            pin: None,
            on: None,
            description: "flagged this moment".into(),
            frame: None,
        });
    }
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
    let selected = match &extras.full_frames {
        Some(decode) if !selected.is_empty() => {
            let times: Vec<u64> = selected.iter().map(|(k, _)| k.t).collect();
            match decode(&times) {
                Ok(full) if full.len() == selected.len() => selected
                    .into_iter()
                    .zip(full)
                    .map(|((key, _), mut frame)| {
                        frame.t_ms = key.t;
                        (key, frame)
                    })
                    .collect(),
                Ok(full) => {
                    capture::scrawl_log(format!(
                        "Full-quality keyframes: decoded {} of {}",
                        full.len(),
                        selected.len()
                    ));
                    selected
                }
                Err(error) => {
                    capture::scrawl_log(format!("Full-quality keyframes failed: {error:#}"));
                    selected
                }
            }
        }
        _ => selected,
    };
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
        source: extras.source,
        device: extras.device,
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
        estimated: false,
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
        SourceExtras::default(),
    )
}

fn main() -> Result<()> {
    if std::env::var_os("SCRAWL_BENCH_START").is_some() {
        ui::run_start_benchmark();
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::App {
        record: false,
        iphone: false,
    }) {
        Command::App { record, iphone } => {
            ui::run_app(None, record, iphone, None);
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
            window: _,
            iphone: true,
            duration,
            note,
            out,
        } => {
            let bundle_dir = out.unwrap_or_else(|| {
                handoff::default_captures_dir().join(format!("{}.scrawl", make_bundle_id()))
            });
            std::fs::create_dir_all(&bundle_dir)?;
            let session =
                phone::PhoneSession::start(&bundle_dir.join("recording.mov"), None, None)?;
            let started = std::time::Instant::now();
            while !session.has_recording() && started.elapsed() < Duration::from_secs(15) {
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(Duration::from_secs(duration.max(1)));
            let recording = session.stop()?;
            let (_dir, _bundle, summary) =
                phone::finalize_live_capture(&bundle_dir, recording, Vec::new(), Vec::new(), note)?;
            print!("{summary}");
            Ok(())
        }
        Command::Record {
            window,
            iphone: false,
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
        Command::Import {
            recording,
            timeline,
            note,
            out,
            clipboard,
        } => {
            let (movie, timeline) =
                phone::resolve_import(recording.as_deref(), timeline.as_deref())?;
            let bundle_dir = out.unwrap_or_else(|| {
                handoff::default_captures_dir().join(format!("{}.scrawl", make_bundle_id()))
            });
            let (dir, _bundle, summary) =
                phone::import(&movie, timeline.as_deref(), &bundle_dir, note)?;
            if clipboard {
                handoff::copy_bundle_to_clipboard(&dir)?;
            }
            print!("{summary}");
            Ok(())
        }
        Command::Reprocess { bundle, out } => {
            let (dir, old) = handoff::load_bundle(&bundle)?;
            let video = old
                .video
                .as_ref()
                .map(|v| dir.join(v))
                .filter(|p| p.is_file())
                .context("The bundle has no video to reprocess")?;
            ensure!(!out.exists(), "{} already exists", out.display());
            std::fs::create_dir_all(&out)?;
            let copied = out.join(video.file_name().context("Bad video name")?);
            std::fs::copy(&video, &copied)?;
            let flags: Vec<u64> = old
                .marks
                .iter()
                .filter(|m| m.shape == Shape::Flag)
                .map(|m| m.t0)
                .collect();
            let summary = if let Some(device) = old.device.clone() {
                phone::build_movie_bundle(
                    &out,
                    phone::MovieCapture {
                        movie: copied,
                        target: old.target,
                        device,
                        events: old.events,
                        strokes: old.strokes,
                        flags,
                        note: old.note,
                    },
                )?
                .2
            } else {
                let audio = old
                    .audio
                    .as_ref()
                    .map(|a| dir.join(a))
                    .filter(|p| p.is_file());
                let recorded = match &audio {
                    Some(path) => {
                        std::fs::copy(path, out.join("audio.wav"))?;
                        Some(hex::recorded_audio_from_wav(std::fs::read(path)?)?)
                    }
                    None => None,
                };
                let wav = out.join("audio.wav");
                let transcript = transcribe::transcribe(
                    recorded.as_ref().map(|_| wav.as_path()),
                    recorded.as_ref(),
                );
                let frames = helper::frames(&copied, helper::FrameTimes::Every(220), 1280)?;
                build_bundle_from_parts(
                    &out,
                    old.target,
                    old.duration_ms,
                    frames,
                    old.strokes,
                    old.events,
                    transcript,
                    Some(copied),
                    old.note,
                    SourceExtras::default(),
                )?
                .2
            };
            print!("{summary}");
            Ok(())
        }
        Command::Devices => {
            for device in helper::devices(Duration::from_secs(3))? {
                println!(
                    "{}\t{}\t{}×{}",
                    device.name, device.id, device.width, device.height
                );
            }
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
            ui::run_app(initial, false, false, capture_screenshot);
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
