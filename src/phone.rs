//! iPhone sources: a live cabled capture mirrored through `scrawl-helper`, and recordings made
//! in-app with TetherKit. Both become ordinary `.scrawl` bundles whose keyframes are decoded
//! from the phone's own video, never from a screenshot of the mirror window.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::capture::{LiveOverlayEvent, SharedCaptureTelemetry, scrawl_log};
use crate::helper::{self, FrameTimes, PhoneDevice};
use crate::model::{
    CaptureMode, DeviceInfo, InteractionEvent, RgbaFrame, ScrawlBundle, Stroke, TargetFrame,
    TargetInfo,
};

/// Keyframes are decoded from the native video and fit to this long edge (736×1600 for a
/// 1320×2868 phone): every UI point stays resolvable, and agents downsample anything larger.
pub const PHONE_FRAME_EDGE: u32 = 1600;
/// Candidate frames for keyframe selection only need enough detail for scene detection.
const CANDIDATE_EDGE: u32 = 320;
const MAX_CANDIDATES: u64 = 480;

#[derive(Default)]
struct MirrorState {
    device: Option<PhoneDevice>,
    rect: Option<[f32; 4]>,
    recording_epoch_ms: Option<u64>,
    recording_instant: Option<Instant>,
    markers_epoch_ms: Vec<u64>,
    finished: Option<(PathBuf, u64)>,
    ended: bool,
    error: Option<String>,
}

/// A running mirror: the helper's floating window shows the phone, records its native video
/// with the Mac's microphone into the bundle, and reports where the picture is on screen so
/// pointer and pen input normalize into phone-screen coordinates.
pub struct PhoneSession {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    state: Arc<Mutex<MirrorState>>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    started_at: Instant,
    session_epoch_ms: u64,
}

pub struct PhoneRecording {
    pub movie: PathBuf,
    pub device: PhoneDevice,
    /// How long after Scrawl's session clock started the movie's first frame arrived.
    pub offset_ms: u64,
    /// M presses in the mirror, on the movie's clock.
    pub flags: Vec<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum MirrorEvent {
    Device(PhoneDevice),
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    Recording {
        #[serde(rename = "startedAt")]
        started_at: u64,
    },
    Marker {
        at: u64,
    },
    Finished {
        path: PathBuf,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
    },
    Cancelled,
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

impl PhoneSession {
    /// Opens the live iPhone mirror window in preview mode without recording to disk yet.
    pub fn start_preview(
        device_id: Option<&str>,
        hidden: bool,
        telemetry: Option<Arc<SharedCaptureTelemetry>>,
    ) -> Result<Self> {
        Self::spawn_mirror(None, hidden, device_id, telemetry)
    }

    /// `telemetry` (when capturing through the app) receives the mirror's on-screen rect as the
    /// capture target, and hears when the helper ends the capture on its own.
    pub fn start(
        movie: &Path,
        device_id: Option<&str>,
        telemetry: Option<Arc<SharedCaptureTelemetry>>,
    ) -> Result<Self> {
        Self::spawn_mirror(Some(movie), false, device_id, telemetry)
    }

    fn spawn_mirror(
        movie: Option<&Path>,
        hidden: bool,
        device_id: Option<&str>,
        telemetry: Option<Arc<SharedCaptureTelemetry>>,
    ) -> Result<Self> {
        let session_epoch_ms = crate::now_epoch_ms();
        let mut command = Command::new(helper::path()?);
        command.arg("mirror");
        if let Some(movie) = movie {
            command.arg("--out").arg(movie);
        } else {
            command.arg("--preview");
            if hidden {
                command.arg("--hidden");
            }
        }
        if let Some(id) = device_id
            && !id.is_empty()
        {
            command.arg("--device").arg(id);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Could not start the iPhone mirror")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("No mirror output")?;
        if let Some(stderr) = child.stderr.take() {
            let _ = std::thread::Builder::new()
                .name("scrawl-phone-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            scrawl_log(trimmed.to_owned());
                        }
                    }
                });
        }
        let state = Arc::new(Mutex::new(MirrorState::default()));
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_state = Arc::clone(&state);
        let reader_cancelled = Arc::clone(&cancelled);
        let reader = std::thread::Builder::new()
            .name("scrawl-phone".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(event) = serde_json::from_str::<MirrorEvent>(&line) else {
                        continue;
                    };
                    let mut state = reader_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match event {
                        MirrorEvent::Device(device) => {
                            state.device = Some(device);
                        }
                        MirrorEvent::Rect {
                            x,
                            y,
                            width,
                            height,
                        } => {
                            let rect = [x as f32, y as f32, width as f32, height as f32];
                            state.rect = Some(rect);
                            if let Some(telemetry) = &telemetry {
                                *telemetry
                                    .target_frame
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    TargetFrame {
                                        x,
                                        y,
                                        width,
                                        height,
                                        scale: 2.0,
                                    };
                                if telemetry
                                    .selecting
                                    .load(std::sync::atomic::Ordering::Acquire)
                                {
                                    telemetry.set_picker_rect(Some((rect, 3)));
                                }
                            }
                        }
                        MirrorEvent::Recording { started_at } => {
                            state.recording_epoch_ms = Some(started_at);
                            state.recording_instant = Some(Instant::now());
                            if let Some(telemetry) = &telemetry {
                                telemetry
                                    .active
                                    .store(true, std::sync::atomic::Ordering::Release);
                            }
                        }
                        MirrorEvent::Marker { at } => state.markers_epoch_ms.push(at),
                        MirrorEvent::Finished { path, duration_ms } => {
                            state.finished = Some((path, duration_ms));
                            state.ended = true;
                        }
                        MirrorEvent::Cancelled => {
                            state.ended = true;
                        }
                        MirrorEvent::Warning { message } => {
                            scrawl_log(format!("iPhone mirror: {message}"));
                        }
                        MirrorEvent::Error { message } => {
                            scrawl_log(format!("iPhone mirror: {message}"));
                            state.error = Some(message);
                            state.ended = true;
                        }
                    }
                }
                reader_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .ended = true;
                // The helper ended; if Scrawl didn't ask (phone unplugged, no phone, denied),
                // let the app finish or cancel the capture.
                if !reader_cancelled.load(std::sync::atomic::Ordering::Acquire)
                    && let Some(telemetry) = &telemetry
                {
                    let _ = telemetry.ui_sender.try_send(LiveOverlayEvent::PhoneEnded);
                }
            })
            .context("Could not watch the iPhone mirror")?;
        Ok(Self {
            child: Some(child),
            stdin,
            reader: Some(reader),
            state,
            cancelled,
            started_at: Instant::now(),
            session_epoch_ms,
        })
    }

    /// Transitions an open preview session into active movie recording.
    pub fn begin_recording(&mut self, movie: &Path) {
        self.started_at = Instant::now();
        self.session_epoch_ms = crate::now_epoch_ms();
        {
            let mut state = self.lock();
            state.recording_epoch_ms = None;
            state.recording_instant = None;
            state.markers_epoch_ms.clear();
        }
        self.send(&format!("start {}", movie.display()));
    }

    pub fn show_preview(&mut self) {
        self.send("show");
    }

    pub fn hide_preview(&mut self) {
        self.send("hide");
    }

    pub fn device(&self) -> Option<PhoneDevice> {
        self.lock().device.clone()
    }

    pub fn rect(&self) -> Option<[f32; 4]> {
        self.lock().rect
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.lock()
            .recording_instant
            .map_or(0, |t| t.elapsed().as_millis() as u64)
    }

    /// Whether the movie has started (or the capture already ended).
    pub fn has_recording(&self) -> bool {
        let state = self.lock();
        state.recording_epoch_ms.is_some() || state.finished.is_some()
    }

    /// Whether the helper process has finished, cancelled, or exited with an error.
    pub fn has_ended(&self) -> bool {
        let state = self.lock();
        state.ended || state.finished.is_some() || state.error.is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MirrorState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn send(&mut self, command: &str) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = writeln!(stdin, "{command}");
            let _ = stdin.flush();
        }
    }

    fn wait(&mut self, timeout: Duration) {
        self.stdin.take();
        if let Some(ref mut child) = self.child {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    fn cancel_async(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        self.send("cancel");
        self.stdin.take();
        let child = self.child.take();
        let reader = self.reader.take();
        if child.is_some() || reader.is_some() {
            std::thread::spawn(move || {
                if let Some(mut child) = child {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < deadline {
                        if matches!(child.try_wait(), Ok(Some(_))) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                }
                if let Some(reader) = reader {
                    let _ = reader.join();
                }
            });
        }
    }

    /// Stops recording and waits for the helper to finalize the movie.
    pub fn stop(mut self) -> Result<PhoneRecording> {
        self.send("stop");
        self.wait(Duration::from_secs(15));
        let state = std::mem::take(&mut *self.lock());
        let Some((movie, _)) = state.finished else {
            bail!(
                "{}",
                state
                    .error
                    .unwrap_or_else(|| "The iPhone recording did not finish".into())
            );
        };
        let device = state.device.context("No iPhone was attached")?;
        let started = state.recording_epoch_ms.unwrap_or(self.session_epoch_ms);
        Ok(PhoneRecording {
            movie,
            device,
            offset_ms: started.saturating_sub(self.session_epoch_ms),
            flags: state
                .markers_epoch_ms
                .iter()
                .map(|at| at.saturating_sub(started))
                .collect(),
        })
    }

    pub fn cancel(mut self) {
        self.cancel_async();
    }
}

impl Drop for PhoneSession {
    fn drop(&mut self) {
        self.cancel_async();
    }
}

/// Everything a phone movie needs to become a bundle, on the movie's clock.
pub struct MovieCapture {
    pub movie: PathBuf,
    pub target: TargetInfo,
    pub device: DeviceInfo,
    pub events: Vec<InteractionEvent>,
    pub strokes: Vec<Stroke>,
    pub flags: Vec<u64>,
    pub note: Option<String>,
}

pub fn phone_target(name: &str, app: Option<&str>, width: u32, height: u32) -> TargetInfo {
    TargetInfo {
        mode: CaptureMode::Device,
        app: Some(app.unwrap_or(name).to_owned()),
        bundle_id: app.map(str::to_owned),
        window_title: None,
        window_id: None,
        display_id: 0,
        frame: TargetFrame {
            x: 0.0,
            y: 0.0,
            width: f64::from(width),
            height: f64::from(height),
            scale: 1.0,
        },
    }
}

/// Builds a bundle from a phone movie already inside `bundle_dir`: narration transcribed with
/// word timings, keyframes chosen on small candidate frames and then decoded again from the
/// native video.
pub fn build_movie_bundle(
    bundle_dir: &Path,
    mut capture: MovieCapture,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let probe = helper::probe(&capture.movie)?;
    ensure!(probe.duration_ms > 0, "The recording is empty");
    if probe.width > 0 && probe.height > 0 {
        capture.device.width = probe.width;
        capture.device.height = probe.height;
        capture.target.frame.width = f64::from(probe.width);
        capture.target.frame.height = f64::from(probe.height);
    }
    let wav = bundle_dir.join("audio.wav");
    let audio = (probe.audio_tracks > 0)
        .then(|| helper::export_audio(&capture.movie, &wav))
        .and_then(|exported| {
            exported
                .and_then(|()| crate::hex::recorded_audio_from_wav(std::fs::read(&wav)?))
                .inspect_err(|error| scrawl_log(format!("Phone audio export failed: {error:#}")))
                .ok()
        });
    let every = (probe.duration_ms / MAX_CANDIDATES).max(250);
    let (transcript, frames) = std::thread::scope(|scope| {
        let frames = scope
            .spawn(|| helper::frames(&capture.movie, FrameTimes::Every(every), CANDIDATE_EDGE));
        let transcript =
            crate::transcribe::transcribe(audio.as_ref().map(|_| wav.as_path()), audio.as_ref());
        (transcript, frames.join())
    });
    let frames = frames.map_err(|_| anyhow::anyhow!("Frame decoding panicked"))??;
    ensure!(!frames.is_empty(), "No frames could be decoded");
    let movie = capture.movie.clone();
    let extras = crate::SourceExtras {
        source: Some("iphone".into()),
        device: Some(capture.device),
        flags: capture.flags,
        full_frames: Some(Box::new(move |times: &[u64]| -> Result<Vec<RgbaFrame>> {
            helper::frames(&movie, FrameTimes::At(times), PHONE_FRAME_EDGE)
        })),
    };
    crate::build_bundle_from_parts(
        bundle_dir,
        capture.target,
        probe.duration_ms,
        frames,
        capture.strokes,
        capture.events,
        transcript,
        Some(capture.movie),
        capture.note,
        extras,
    )
}

/// Finishes a live capture: re-bases Scrawl's pointer and pen input onto the movie's clock.
pub fn finalize_live_capture(
    bundle_dir: &Path,
    recording: PhoneRecording,
    mut events: Vec<InteractionEvent>,
    mut strokes: Vec<Stroke>,
    note: Option<String>,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    events.retain_mut(|event| event.rebase(recording.offset_ms));
    strokes.retain_mut(|stroke| stroke.rebase(recording.offset_ms));
    let device = &recording.device;
    let target = phone_target(&device.name, None, device.width, device.height);
    let info = DeviceInfo {
        name: device.name.clone(),
        capture: "mirror".into(),
        id: Some(device.id.clone()),
        model: Some(device.model.clone()),
        width: device.width,
        height: device.height,
        ..Default::default()
    };
    build_movie_bundle(
        bundle_dir,
        MovieCapture {
            movie: recording.movie,
            target,
            device: info,
            events,
            strokes,
            flags: recording.flags,
            note,
        },
    )
}

// MARK: TetherKit import

/// The `.json` TetherKit writes beside each in-app recording (seconds from the start, points in
/// screen coordinates).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timeline {
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub bundle_id: Option<String>,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub recorded_at: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub screen: Option<Screen>,
    #[serde(default)]
    pub markers: Vec<f64>,
    #[serde(default)]
    pub events: Vec<AppEvent>,
    #[serde(default)]
    pub touches: Vec<Touch>,
    #[serde(default)]
    pub strokes: Vec<TimelineStroke>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TimelineStroke {
    pub t0: f64,
    pub t1: f64,
    #[serde(default)]
    pub points: Vec<[f64; 3]>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    #[serde(default = "one")]
    pub scale: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct AppEvent {
    pub t: f64,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Touch {
    pub t: f64,
    pub id: i64,
    pub x: f64,
    pub y: f64,
    pub phase: String,
    #[serde(default)]
    pub label: Option<String>,
}

fn ms(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * 1_000.0).round() as u64
    } else {
        0
    }
}

/// Finger movement under this many points, held under this long, is a tap.
const TAP_SLOP_PT: f64 = 12.0;
const TAP_MS: u64 = 420;

/// Turns raw touch samples into taps and swipes, normalized to the screen.
pub fn touch_events(touches: &[Touch], screen: Screen) -> Vec<InteractionEvent> {
    let mut sorted = touches.to_vec();
    sorted.sort_by(|a, b| a.t.total_cmp(&b.t));
    let (w, h) = (screen.width.max(1.0), screen.height.max(1.0));
    let norm = |touch: &Touch| {
        [
            (touch.x / w).clamp(0.0, 1.0) as f32,
            (touch.y / h).clamp(0.0, 1.0) as f32,
        ]
    };
    let mut open: Vec<(i64, Vec<Touch>)> = Vec::new();
    let mut out = Vec::new();
    let close = |samples: Vec<Touch>, out: &mut Vec<InteractionEvent>| {
        let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
            return;
        };
        let travel = samples
            .iter()
            .map(|s| (s.x - first.x).hypot(s.y - first.y))
            .fold(0.0, f64::max);
        let (t0, t1) = (ms(first.t), ms(last.t));
        let [x0, y0] = norm(first);
        let hit_label = samples
            .iter()
            .find_map(|s| s.label.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if travel < TAP_SLOP_PT && t1.saturating_sub(t0) < TAP_MS {
            out.push(InteractionEvent::Click {
                t: t0,
                x: x0,
                y: y0,
                button: Some("touch".into()),
                label: hit_label.map(str::to_owned),
                role: None,
            });
        } else {
            let [x1, y1] = norm(last);
            let drag_label = if travel < TAP_SLOP_PT {
                Some(match hit_label {
                    Some(l) => format!("long press: {l}"),
                    None => "long press".into(),
                })
            } else {
                hit_label.map(str::to_owned)
            };
            out.push(InteractionEvent::Drag {
                t: t0,
                t1,
                x0,
                y0,
                x1,
                y1,
                label: drag_label,
                button: Some("touch".into()),
                path: samples.iter().map(norm).collect(),
            });
        }
    };
    for touch in sorted {
        let index = open.iter().position(|(id, _)| *id == touch.id);
        match (touch.phase.as_str(), index) {
            ("down", Some(i)) => {
                let (_, samples) = open.remove(i);
                close(samples, &mut out);
                open.push((touch.id, vec![touch]));
            }
            ("down", None) => open.push((touch.id, vec![touch])),
            ("up" | "cancel", Some(i)) => {
                let (_, mut samples) = open.remove(i);
                samples.push(touch);
                close(samples, &mut out);
            }
            ("up" | "cancel", None) => close(vec![touch], &mut out),
            (_, Some(i)) => open[i].1.push(touch),
            (_, None) => open.push((touch.id, vec![touch])),
        }
    }
    for (_, samples) in open {
        close(samples, &mut out);
    }
    out.sort_by_key(InteractionEvent::timestamp_ms);
    out
}

fn is_movie(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "mp4" | "mov" | "m4v"))
}

fn sibling(path: &Path, want_movie: bool) -> Option<PathBuf> {
    let stem = path.file_stem()?;
    let dir = path.parent()?;
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p != path && p.file_stem() == Some(stem))
        .find(|p| {
            if want_movie {
                is_movie(p)
            } else {
                p.extension().is_some_and(|e| e == "json")
            }
        })
}

/// Resolves what `scrawl import` was given into a movie and its timeline: a movie (timeline
/// beside it), a timeline (movie beside it), or nothing (the newest pair in ~/Downloads, where
/// AirDrop puts them).
pub fn resolve_import(
    path: Option<&Path>,
    timeline: Option<&Path>,
) -> Result<(PathBuf, Option<PathBuf>)> {
    let movie = match path {
        Some(path) if is_movie(path) => path.to_path_buf(),
        Some(path) => sibling(path, true)
            .with_context(|| format!("No .mp4 or .mov beside {}", path.display()))?,
        None => {
            let downloads = dirs::download_dir().context("No Downloads folder")?;
            std::fs::read_dir(&downloads)?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| is_movie(p) && sibling(p, false).is_some())
                .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
                .with_context(|| {
                    format!(
                        "No TetherKit recording (movie + .json) in {}",
                        downloads.display()
                    )
                })?
        }
    };
    ensure!(movie.is_file(), "{} does not exist", movie.display());
    let timeline = match (timeline, path) {
        (Some(t), _) => Some(t.to_path_buf()),
        (None, Some(p)) if !is_movie(p) => Some(p.to_path_buf()),
        _ => sibling(&movie, false),
    };
    Ok((movie, timeline))
}

/// `scrawl import`: a TetherKit recording becomes a bundle. Touches become taps and swipes
/// (ripples and trails on keyframes), markers become flags, app events land in the summary.
pub fn import(
    movie: &Path,
    timeline_path: Option<&Path>,
    bundle_dir: &Path,
    note: Option<String>,
) -> Result<(PathBuf, ScrawlBundle, String)> {
    let timeline: Timeline = match timeline_path {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("Invalid TetherKit timeline {}", path.display()))?,
        None => Timeline::default(),
    };
    std::fs::create_dir_all(bundle_dir)?;
    let ext = movie
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_ascii_lowercase();
    let copied = bundle_dir.join(format!("recording.{ext}"));
    std::fs::copy(movie, &copied).context("Could not copy the recording")?;
    if let Some(path) = timeline_path {
        std::fs::copy(path, bundle_dir.join("timeline.json"))?;
    }

    let probe = helper::probe(&copied)?;
    let screen = timeline.screen.unwrap_or(Screen {
        width: f64::from(probe.width.max(1)),
        height: f64::from(probe.height.max(1)),
        scale: 1.0,
    });
    let mut events = touch_events(&timeline.touches, screen);
    events.extend(timeline.events.iter().map(|e| InteractionEvent::AppEvent {
        t: ms(e.t),
        label: e.label.clone(),
    }));
    events.sort_by_key(InteractionEvent::timestamp_ms);
    let (sw, sh) = (screen.width.max(1.0), screen.height.max(1.0));
    let strokes: Vec<Stroke> = timeline
        .strokes
        .iter()
        .filter_map(|s| {
            let t0 = ms(s.t0);
            let t1 = ms(s.t1).max(t0);
            let mut pts = Vec::with_capacity(s.points.len() * 3);
            for &[x, y, pt_t] in &s.points {
                let nx = ((x / sw).clamp(0.0, 1.0) * f64::from(crate::model::SCALE)).round() as i32;
                let ny = ((y / sh).clamp(0.0, 1.0) * f64::from(crate::model::SCALE)).round() as i32;
                let dt = ms(pt_t).saturating_sub(t0) as i32;
                pts.extend_from_slice(&[nx, ny, dt]);
            }
            (!pts.is_empty()).then_some(Stroke {
                t0,
                t1,
                fw: probe.width,
                fh: probe.height,
                on: None,
                pin: None,
                laser: false,
                pts,
            })
        })
        .collect();
    let name = timeline.device.clone().unwrap_or_else(|| "iPhone".into());
    let app = timeline.app.clone().filter(|a| !a.is_empty());
    let device = DeviceInfo {
        name: name.clone(),
        capture: "tetherkit".into(),
        system: timeline.system.clone(),
        width: probe.width,
        height: probe.height,
        screen: timeline.screen.map(|s| [s.width, s.height, s.scale]),
        app: app.clone(),
        app_version: timeline.version.clone(),
        ..Default::default()
    };
    build_movie_bundle(
        bundle_dir,
        MovieCapture {
            movie: copied,
            target: phone_target(&name, app.as_deref(), probe.width, probe.height),
            device,
            events,
            strokes,
            flags: timeline.markers.iter().map(|&t| ms(t)).collect(),
            note,
        },
    )
}

/// Starts Scrawl's background TetherKit ingest: an HTTP server on `0.0.0.0:47777` the phone
/// uploads to, and a cabled-iPhone `devicectl` watcher for when the network can't reach the Mac.
/// Each recording becomes a `.scrawl` bundle, lands on the clipboard and is queued into the
/// OpenCode session matching the app. The phone is told as soon as the files arrive; the bundle
/// is built afterwards.
pub fn start_ingest_server(ui_sender: flume::Sender<LiveOverlayEvent>) {
    let http_sender = ui_sender.clone();
    let _ = std::thread::Builder::new()
        .name("scrawl-ingest-http".into())
        .spawn(move || {
            let Ok(listener) = std::net::TcpListener::bind("0.0.0.0:47777") else {
                scrawl_log("ingest server: port 47777 already in use");
                return;
            };
            scrawl_log("ingest server listening on 0.0.0.0:47777");
            for stream in listener.incoming().flatten() {
                let tx = http_sender.clone();
                std::thread::spawn(move || {
                    if let Err(err) = handle_ingest_connection(stream, &tx) {
                        scrawl_log(format!("ingest http: {err:#}"));
                    }
                });
            }
        });

    let _ = std::thread::Builder::new()
        .name("scrawl-ingest-usb".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(1500));
                // The USB registry check is instant, so `xcrun` only runs with an iPhone cabled.
                if helper::devices().map_or(true, |d| d.is_empty()) {
                    continue;
                }
                if let Err(err) = poll_usb_tether_containers(&ui_sender) {
                    scrawl_log(format!("ingest usb: {err:#}"));
                }
            }
        });
}

/// Recordings already taken in, by `recordedAt`, so one that arrives over both HTTP and USB is
/// built once.
static INGESTED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

/// Claims a recording for ingest; `false` if it was already claimed.
fn claim_recording(timeline: &Timeline) -> bool {
    let key = timeline
        .recorded_at
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(crate::make_bundle_id);
    INGESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(Default::default)
        .insert(key)
}

/// The OpenCode session a recording goes to (if one resolves) and the name the phone shows.
fn ingest_target(timeline: &Timeline) -> (Option<String>, String) {
    let app = timeline.app.as_deref().or(timeline.bundle_id.as_deref());
    match crate::handoff::resolve_opencode_session(timeline.session.as_deref(), app) {
        Ok((id, title)) => (Some(id), title),
        Err(err) => {
            scrawl_log(format!("ingest: no OpenCode session ({err:#})"));
            (None, "Mac".to_owned())
        }
    }
}

/// Builds the bundle for a received recording, copies it, queues it into `session` and shows it
/// in the HUD.
fn build_ingested(
    movie: &Path,
    timeline_path: &Path,
    session: Option<&str>,
    title: &str,
    ui_sender: &flume::Sender<LiveOverlayEvent>,
) -> Result<()> {
    let bundle_dir =
        crate::handoff::default_captures_dir().join(format!("{}.scrawl", crate::make_bundle_id()));
    let (dir, bundle, _) = import(movie, Some(timeline_path), &bundle_dir, None)?;
    let _ = crate::handoff::copy_bundle_to_clipboard(&dir);
    let status = match session {
        Some(session) => match crate::handoff::send_bundle_to_session(&dir, session) {
            Ok(()) => format!("Sent to {title}"),
            Err(err) => {
                scrawl_log(format!("ingest: OpenCode send failed ({err:#})"));
                "Copied to clipboard".to_owned()
            }
        },
        None => "Copied to clipboard".to_owned(),
    };
    let _ = ui_sender.send(LiveOverlayEvent::IngestedBundle {
        dir,
        bundle: Box::new(bundle),
        status: Some(status),
    });
    Ok(())
}

fn http_json(stream: &mut std::net::TcpStream, status: &str, body: &serde_json::Value) {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn handle_ingest_connection(
    mut stream: std::net::TcpStream,
    ui_sender: &flume::Sender<LiveOverlayEvent>,
) -> Result<()> {
    use std::io::Read;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut header = Vec::with_capacity(2048);
    let mut byte = [0_u8; 1];
    while header.len() < 16_384 && !header.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte)? == 0 {
            break;
        }
        header.push(byte[0]);
    }
    let header = String::from_utf8_lossy(&header);
    let first_line = header.lines().next().unwrap_or("");
    if first_line.starts_with("GET ") {
        http_json(&mut stream, "200 OK", &serde_json::json!({ "ok": true }));
        return Ok(());
    }
    ensure!(
        first_line.starts_with("POST /ingest"),
        "Unsupported route: {first_line}"
    );
    let content_len: usize = header
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())
                .flatten()
        })
        .context("Missing Content-Length")?;
    ensure!(
        content_len > 4 && content_len <= 250 * 1024 * 1024,
        "Invalid payload length"
    );
    let mut payload = vec![0_u8; content_len];
    stream.read_exact(&mut payload)?;

    let json_len = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    ensure!(4 + json_len < payload.len(), "Invalid framed payload");
    let json_bytes = &payload[4..4 + json_len];
    let timeline: Timeline = serde_json::from_slice(json_bytes).unwrap_or_default();
    let (session, title) = ingest_target(&timeline);
    let staging = std::env::temp_dir().join(format!("scrawl-ingest-{}", crate::make_bundle_id()));
    std::fs::create_dir_all(&staging)?;
    let timeline_path = staging.join("timeline.json");
    let movie_path = staging.join("recording.mp4");
    std::fs::write(&timeline_path, json_bytes)?;
    std::fs::write(&movie_path, &payload[4 + json_len..])?;
    drop(payload);
    if !claim_recording(&timeline) {
        let _ = std::fs::remove_dir_all(&staging);
        http_json(
            &mut stream,
            "200 OK",
            &serde_json::json!({ "ok": true, "sessionTitle": title }),
        );
        return Ok(());
    }

    // The phone only needs to know the files arrived; building the bundle takes a while.
    http_json(
        &mut stream,
        "202 Accepted",
        &serde_json::json!({ "ok": true, "sessionTitle": title }),
    );
    let _ = stream.shutdown(std::net::Shutdown::Both);

    let built = build_ingested(
        &movie_path,
        &timeline_path,
        session.as_deref(),
        &title,
        ui_sender,
    );
    let _ = std::fs::remove_dir_all(&staging);
    built
}

/// Bundle IDs whose `Documents/Tether` the USB watcher checks.
const TETHER_APPS: &[&str] = &["com.kitlangton.UpDown"];

fn devicectl_copy(
    udid: &str,
    bundle_id: &str,
    direction: &str,
    source: &str,
    destination: &str,
) -> bool {
    Command::new("xcrun")
        .args([
            "devicectl",
            "device",
            "copy",
            direction,
            "--device",
            udid,
            "--domain-type",
            "appDataContainer",
            "--domain-identifier",
            bundle_id,
            "--source",
            source,
            "--destination",
            destination,
        ])
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Takes in recordings the phone marked `.ready` but has no `.sent` for, and writes `.sent`
/// (the session's name) back as soon as the files are on the Mac.
fn poll_usb_tether_containers(ui_sender: &flume::Sender<LiveOverlayEvent>) -> Result<()> {
    let list = Command::new("xcrun")
        .args(["devicectl", "list", "devices"])
        .output()?;
    let list = String::from_utf8_lossy(&list.stdout);
    let Some(udid) = list.lines().find_map(|line| {
        (line.contains("physical")
            && line.contains("available (paired)")
            && !line.contains("unavailable")
            && line.contains("iPhone"))
        .then(|| {
            line.split_whitespace()
                .find(|tok| {
                    tok.len() >= 20 && tok.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                })
                .map(str::to_owned)
        })
        .flatten()
    }) else {
        return Ok(());
    };

    for &bundle_id in TETHER_APPS {
        let files = Command::new("xcrun")
            .args([
                "devicectl",
                "device",
                "info",
                "files",
                "--device",
                &udid,
                "--domain-type",
                "appDataContainer",
                "--domain-identifier",
                bundle_id,
                "--subdirectory",
                "Documents/Tether",
            ])
            .output()?;
        if !files.status.success() {
            continue;
        }
        let files = String::from_utf8_lossy(&files.stdout);
        let names: Vec<&str> = files
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .collect();
        for stem in names.iter().filter_map(|n| n.strip_suffix(".ready")) {
            if names.contains(&format!("{stem}.sent").as_str()) {
                continue;
            }
            let staging =
                std::env::temp_dir().join(format!("scrawl-usb-{}", crate::make_bundle_id()));
            std::fs::create_dir_all(&staging)?;
            let local_json = staging.join(format!("{stem}.json"));
            let local_mp4 = staging.join(format!("{stem}.mp4"));
            let sent = staging.join(format!("{stem}.sent"));
            let remote = |ext: &str| format!("Documents/Tether/{stem}.{ext}");
            if !devicectl_copy(
                &udid,
                bundle_id,
                "from",
                &remote("json"),
                &local_json.to_string_lossy(),
            ) {
                let _ = std::fs::remove_dir_all(&staging);
                continue;
            }
            let timeline: Timeline = std::fs::read(&local_json)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            if !devicectl_copy(
                &udid,
                bundle_id,
                "from",
                &remote("mp4"),
                &local_mp4.to_string_lossy(),
            ) {
                let _ = std::fs::remove_dir_all(&staging);
                continue;
            }
            let (session, title) = ingest_target(&timeline);
            let fresh = claim_recording(&timeline);
            // Answer the phone first: it is waiting, and the bundle takes a while to build.
            std::fs::write(&sent, title.as_bytes())?;
            devicectl_copy(
                &udid,
                bundle_id,
                "to",
                &sent.to_string_lossy(),
                &remote("sent"),
            );
            if fresh {
                if let Err(err) = build_ingested(
                    &local_mp4,
                    &local_json,
                    session.as_deref(),
                    &title,
                    ui_sender,
                ) {
                    scrawl_log(format!("ingest usb {stem}: {err:#}"));
                }
            }
            let _ = std::fs::remove_dir_all(&staging);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(t: f64, id: i64, x: f64, y: f64, phase: &str) -> Touch {
        Touch {
            t,
            id,
            x,
            y,
            phase: phase.into(),
            label: None,
        }
    }

    #[test]
    fn touches_become_taps_swipes_and_long_presses() {
        let screen = Screen {
            width: 400.0,
            height: 800.0,
            scale: 3.0,
        };
        let events = touch_events(
            &[
                touch(1.0, 1, 200.0, 400.0, "down"),
                touch(1.08, 1, 202.0, 401.0, "up"),
                touch(2.0, 2, 200.0, 700.0, "down"),
                touch(2.1, 2, 200.0, 500.0, "move"),
                touch(2.2, 2, 200.0, 200.0, "up"),
                touch(3.0, 3, 100.0, 100.0, "down"),
                touch(3.9, 3, 101.0, 100.0, "up"),
                // A touch the system took over still ends.
                touch(5.0, 4, 300.0, 400.0, "down"),
                touch(5.1, 4, 380.0, 400.0, "cancel"),
            ],
            screen,
        );
        assert_eq!(events.len(), 4);
        assert!(matches!(
            &events[0],
            InteractionEvent::Click { t: 1000, x, y, button: Some(b), .. }
                if *x == 0.5 && *y == 0.5 && b == "touch"
        ));
        match &events[1] {
            InteractionEvent::Drag {
                t,
                t1,
                y0,
                y1,
                path,
                ..
            } => {
                assert_eq!((*t, *t1), (2000, 2200));
                assert!(*y0 > 0.85 && *y1 < 0.3);
                assert_eq!(path.len(), 3);
            }
            other => panic!("expected a swipe, got {other:?}"),
        }
        assert!(matches!(
            &events[2],
            InteractionEvent::Drag { label: Some(l), .. } if l == "long press"
        ));
        assert!(matches!(
            &events[3],
            InteractionEvent::Drag { t1: 5100, .. }
        ));
    }

    #[test]
    fn timeline_parses_tetherkit_output() {
        let json = r#"{"app":"dev.kitlangton.UpDown","version":"42","device":"iPhone","system":"27.0",
            "screen":{"width":440,"height":956,"scale":3},"recordedAt":"2026-10-13T16:40:00Z",
            "duration":12.5,"markers":[3.2],"events":[{"t":4.1,"label":"Done tapped"}],
            "touches":[{"t":4.0,"id":7,"x":220,"y":900,"phase":"down"},{"t":4.05,"id":7,"x":220,"y":900,"phase":"up"}]}"#;
        let timeline: Timeline = serde_json::from_str(json).unwrap();
        assert_eq!(timeline.markers, vec![3.2]);
        assert_eq!(timeline.events[0].label, "Done tapped");
        let events = touch_events(&timeline.touches, timeline.screen.unwrap());
        assert!(matches!(events[0], InteractionEvent::Click { t: 4000, .. }));
    }

    #[test]
    fn import_pairs_movie_and_timeline_by_name() {
        let dir = std::env::temp_dir().join(crate::make_bundle_id());
        std::fs::create_dir_all(&dir).unwrap();
        let movie = dir.join("20261013T164000.mp4");
        let json = dir.join("20261013T164000.json");
        std::fs::write(&movie, b"x").unwrap();
        std::fs::write(&json, b"{}").unwrap();
        assert_eq!(
            resolve_import(Some(&movie), None).unwrap(),
            (movie.clone(), Some(json.clone()))
        );
        assert_eq!(
            resolve_import(Some(&json), None).unwrap(),
            (movie.clone(), Some(json.clone()))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
