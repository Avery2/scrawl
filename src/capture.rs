use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::model::{
    CaptureMode, InteractionEvent, RgbaFrame, SCALE, Stroke, TargetFrame, TargetInfo,
};

// The operating system side of capture: global shortcuts, input monitoring, the frontmost app,
// displays and windows, and the screen recorder. Each platform module provides the same items.
#[cfg_attr(target_os = "macos", path = "capture/macos.rs")]
#[cfg_attr(target_os = "windows", path = "capture/windows.rs")]
mod platform;
pub use platform::*;

pub const FRAME_SAMPLE_INTERVAL: Duration = Duration::from_millis(220);
/// Candidate frames only feed scene diffs and keyframe selection; the keyframes themselves are
/// decoded from `recording.mp4` at full size afterwards.
pub const CANDIDATE_EDGE: f64 = 480.0;
/// Without a recording to decode from, the candidates are the keyframes, so keep them sharper.
pub(crate) const CANDIDATE_EDGE_NO_VIDEO: f64 = 1280.0;
/// The candidates' memory ceiling, however long the capture runs.
pub const CANDIDATE_BUDGET_BYTES: usize = 384 << 20;

/// The candidate frames of a capture, bounded in memory. When full it drops every other frame
/// (keeping the first and the ones taken for a click, stroke or shortcut) and samples half as
/// often from then on, so a long capture keeps its whole timeline, evenly, at a coarser step.
pub struct CandidateBuffer {
    frames: Vec<(RgbaFrame, bool)>,
    cap: usize,
    budget: usize,
    interval: Duration,
}

impl CandidateBuffer {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            frames: Vec::new(),
            cap: 0,
            budget: budget_bytes,
            interval: FRAME_SAMPLE_INTERVAL,
        }
    }

    /// How often to sample now.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn push(&mut self, frame: RgbaFrame, urgent: bool) {
        if self.cap == 0 {
            self.cap = (self.budget / frame.rgba.len().max(1)).clamp(60, 1_200);
        }
        self.frames.push((frame, urgent));
        if self.frames.len() >= self.cap {
            self.thin();
        }
    }

    fn thin(&mut self) {
        let mut regular = 0_usize;
        let mut kept: Vec<(RgbaFrame, bool)> = std::mem::take(&mut self.frames)
            .into_iter()
            .enumerate()
            .filter(|(i, (_, urgent))| {
                let keep = *i == 0 || *urgent || regular.is_multiple_of(2);
                if !urgent {
                    regular += 1;
                }
                keep
            })
            .map(|(_, f)| f)
            .collect();
        // Mostly urgent frames (a storm of clicks): thin those too.
        if kept.len() > self.cap * 3 / 4 {
            kept = kept
                .into_iter()
                .enumerate()
                .filter(|(i, _)| i.is_multiple_of(2))
                .map(|(_, f)| f)
                .collect();
        }
        self.frames = kept;
        self.interval *= 2;
    }

    pub fn into_frames(self) -> Vec<RgbaFrame> {
        self.frames.into_iter().map(|(f, _)| f).collect()
    }
}

/// What a finished screen capture leaves behind.
pub struct ScreenRecording {
    /// Small candidate frames for keyframe selection (see `CANDIDATE_EDGE`).
    pub frames: Vec<RgbaFrame>,
    pub video: Option<PathBuf>,
    pub duration_ms: u64,
    /// The capture time (on the frames' clock) of the video's start.
    pub video_offset_ms: u64,
}

/// Live UI events emitted by the global input monitor to drive Scrawl's HUD & Review windows.
#[derive(Clone, Debug)]
pub enum LiveOverlayEvent {
    /// ⌥⇧S (or the configured shortcut) went down; `hold::Switcher` decides what it means.
    CaptureShortcut,
    PasteLatestShortcut,
    UndoStroke,
    /// macOS's Esc hotkey; Windows polls Esc while the switcher is held.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    CancelCapture,
    RequestSnapshot,
    /// The iPhone mirror ended on its own (unplugged, never found, or access denied).
    PhoneEnded,
    /// The welcome window: a button, fresh permission states, or the helper exiting.
    Onboarding(crate::onboarding::Event),
    /// A ScrawlKit recording was ingested from an iPhone and compiled into a `.scrawl` bundle.
    IngestedBundle {
        dir: PathBuf,
        bundle: Box<crate::model::ScrawlBundle>,
        status: Option<String>,
    },
}

// Carbon HIToolbox modifier bits. The shortcut is stored with them on every platform.
pub const CARBON_CMD_KEY: u32 = 1 << 8;
pub const CARBON_SHIFT_KEY: u32 = 1 << 9;
pub const CARBON_OPTION_KEY: u32 = 1 << 11;
pub const CARBON_CONTROL_KEY: u32 = 1 << 12;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ShortcutConfig {
    pub key_code: u32,
    pub carbon_modifiers: u32,
    pub label: String,
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            key_code: u32::from(layout_s_keycode()),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: shortcut_label(CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "S"),
        }
    }
}

/// A shortcut as this platform writes it: `⌥⇧S` on macOS, `Alt+Shift+S` on Windows.
pub fn shortcut_label(carbon_modifiers: u32, key: &str) -> String {
    let held = |bit: u32| carbon_modifiers & bit != 0;
    if cfg!(target_os = "windows") {
        let mut parts = Vec::new();
        for (bit, name) in [
            (CARBON_CONTROL_KEY, "Ctrl"),
            (CARBON_CMD_KEY, "Win"),
            (CARBON_OPTION_KEY, "Alt"),
            (CARBON_SHIFT_KEY, "Shift"),
        ] {
            if held(bit) {
                parts.push(name);
            }
        }
        parts.push(key);
        parts.join("+")
    } else {
        let mut label = String::new();
        for (bit, glyph) in [
            (CARBON_CONTROL_KEY, '⌃'),
            (CARBON_OPTION_KEY, '⌥'),
            (CARBON_SHIFT_KEY, '⇧'),
            (CARBON_CMD_KEY, '⌘'),
        ] {
            if held(bit) {
                label.push(glyph);
            }
        }
        label.push_str(key);
        label
    }
}

/// The paste shortcut's label: `⌥⇧V` on macOS, `Alt+Shift+V` on Windows.
pub fn paste_shortcut_label() -> String {
    shortcut_label(CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "V")
}

/// The system paste keystroke: `⌘V` on macOS, `Ctrl+V` on Windows.
pub fn system_paste_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Ctrl+V"
    } else {
        "⌘V"
    }
}

/// The keys of a shortcut label, one per keycap: `⌥⇧S` → `⌥`, `⇧`, `S`; `Alt+Shift+S` →
/// `Alt`, `Shift`, `S`.
pub fn shortcut_keys(label: &str) -> Vec<String> {
    if label.contains('+') && label.len() > 1 {
        let mut keys: Vec<String> = label
            .split('+')
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect();
        // `Ctrl++` binds the plus key itself.
        if label.ends_with("++") {
            keys.push("+".into());
        }
        keys
    } else {
        label.chars().map(String::from).collect()
    }
}

impl ShortcutConfig {
    fn config_path() -> PathBuf {
        crate::handoff::default_captures_dir()
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
            .join("config.json")
    }

    pub fn load() -> Self {
        let mut cfg: Self = std::fs::read_to_string(Self::config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if cfg.label == "⌃⌥⇧S" || cfg.label.is_empty() {
            cfg = Self::default();
        } else if cfg.label.ends_with('S') && cfg.key_code == u32::from(KEY_S) {
            cfg.key_code = u32::from(layout_s_keycode());
        }
        cfg
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

pub fn scrawl_log(msg: impl AsRef<str>) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let line = format!("[{ts}] {}\n", msg.as_ref());
    eprint!("{line}");
    let Some(dir) = log_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("scrawl.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// A shortcut label split into its modifiers and key: `⌥⇧S` → (`⌥⇧`, `S`), and on Windows
/// `Alt+Shift+S` → (`Alt+Shift`, `S`).
pub fn shortcut_parts(label: &str) -> (&str, &str) {
    if let Some(plus) = label
        .get(..label.len().saturating_sub(1))
        .and_then(|l| l.rfind('+'))
    {
        return (&label[..plus], &label[plus + 1..]);
    }
    let split = label
        .char_indices()
        .find(|(_, c)| !matches!(c, '⌃' | '⌥' | '⇧' | '⌘'))
        .map_or(label.len(), |(i, _)| i);
    label.split_at(split)
}

/// Whether a key press is the capture shortcut: the configured one, or ⌥⇧S (also with ⌃ or ⌘)
/// and ⌃⌥⌘S on the active layout's S key or the ANSI one.
pub fn is_capture_shortcut(
    cfg: &ShortcutConfig,
    s_code: u16,
    keycode: u16,
    cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> bool {
    let want_cmd = (cfg.carbon_modifiers & CARBON_CMD_KEY) != 0;
    let want_ctrl = (cfg.carbon_modifiers & CARBON_CONTROL_KEY) != 0;
    let want_alt = (cfg.carbon_modifiers & CARBON_OPTION_KEY) != 0;
    let want_shift = (cfg.carbon_modifiers & CARBON_SHIFT_KEY) != 0;
    let custom = u32::from(keycode) == cfg.key_code
        && cmd == want_cmd
        && ctrl == want_ctrl
        && alt == want_alt
        && shift == want_shift;
    let is_s_key = keycode == s_code || keycode == KEY_S;
    custom || (is_s_key && alt && (shift || (ctrl && cmd)))
}

/// No-op: do not register a global Escape hotkey because Karabiner maps CapsLock (Control)
/// with `to_if_alone: escape`, which would cancel recordings when releasing Control.
pub fn set_escape_hotkey_enabled(_enabled: bool) {}

fn clean_label(input: &str) -> String {
    let squashed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = squashed.trim();
    if trimmed.chars().count() > 44 {
        let prefix: String = trimmed.chars().take(43).collect();
        format!("{}…", prefix.trim_end())
    } else {
        trimmed.to_string()
    }
}

/// Which display to record: the one under `point`, else the one holding the focused window's
/// centre, else the first. Frames and points are global, top-left origin.
pub fn pick_display(
    displays: &[[f64; 4]],
    point: Option<[f64; 2]>,
    focused: Option<[f64; 4]>,
) -> usize {
    let inside = |p: [f64; 2]| {
        displays
            .iter()
            .position(|&[x, y, w, h]| p[0] >= x && p[0] < x + w && p[1] >= y && p[1] < y + h)
    };
    point
        .and_then(inside)
        .or_else(|| focused.and_then(|[x, y, w, h]| inside([x + w / 2.0, y + h / 2.0])))
        .unwrap_or(0)
}

pub fn selectable_window_at(windows: &[TargetInfo], x: f64, y: f64) -> Option<&TargetInfo> {
    windows.iter().find(|window| {
        let f = window.frame;
        x >= f.x && x <= f.x + f.width && y >= f.y && y <= f.y + f.height
    })
}

/// Resolves region metadata from the picker's snapshot, without another SCK query.
pub fn region_target(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    windows: &[TargetInfo],
) -> TargetInfo {
    let mid_x = x + width * 0.5;
    let mid_y = y + height * 0.5;
    let hit = selectable_window_at(windows, mid_x, mid_y);
    let frontmost = frontmost_app_info();
    let display_id = main_display_id();
    TargetInfo {
        mode: CaptureMode::Region,
        app: hit
            .and_then(|window| window.app.clone())
            .or_else(|| frontmost.as_ref().map(|(_, name, _)| name.clone())),
        bundle_id: hit
            .and_then(|window| window.bundle_id.clone())
            .or_else(|| frontmost.and_then(|(_, _, bid)| bid)),
        window_title: hit.and_then(|window| window.window_title.clone()),
        window_id: None,
        display_id,
        frame: TargetFrame {
            x,
            y,
            width: width.max(32.0),
            height: height.max(32.0),
            scale: 2.0,
        },
    }
}

/// The capture size for `target` with its long edge at most `long_edge` px (and at most
/// `MAX_CAPTURE_SCALE`× its size: 2× its points on macOS, its own pixels on Windows).
pub fn capture_size(target: &TargetInfo, long_edge: f64) -> (u32, u32) {
    let max_dim = target.frame.width.max(target.frame.height).max(1.0);
    let scale = (long_edge / max_dim).min(MAX_CAPTURE_SCALE);
    (
        ((target.frame.width * scale).round() as u32).max(48) & !1,
        ((target.frame.height * scale).round() as u32).max(48) & !1,
    )
}

/// What the platform's sampler thread records into. It runs until `stopping`, pushes candidate
/// frames (at once when `urgent`), stores when the movie started into `video_offset_ms`, and
/// returns the movie it wrote.
pub(crate) struct Sampler {
    pub target: TargetInfo,
    pub video_output_path: Option<PathBuf>,
    /// Scrawl's own windows to leave out of the recording.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    pub started_at: Instant,
    pub stopping: Arc<AtomicBool>,
    pub urgent: Arc<AtomicBool>,
    pub frames: Arc<Mutex<CandidateBuffer>>,
    pub video_offset_ms: Arc<AtomicU64>,
}

/// Active screen + optional MP4 recorder (ScreenCaptureKit on macOS, Windows.Graphics.Capture
/// and Media Foundation on Windows).
pub struct ScreenCaptureSession {
    started_at: Instant,
    stopping: Arc<AtomicBool>,
    urgent_snapshot: Arc<AtomicBool>,
    frames: Arc<Mutex<CandidateBuffer>>,
    video_offset_ms: Arc<AtomicU64>,
    sampler_thread: Option<JoinHandle<Option<PathBuf>>>,
    _recording: crate::retention::Recording,
}

impl ScreenCaptureSession {
    pub fn start(
        target: TargetInfo,
        video_output_path: Option<PathBuf>,
        excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    ) -> Self {
        let started_at = Instant::now();
        let stopping = Arc::new(AtomicBool::new(false));
        let urgent_snapshot = Arc::new(AtomicBool::new(true));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(CANDIDATE_BUDGET_BYTES)));
        let video_offset_ms = Arc::new(AtomicU64::new(0));

        let thread_stopping = Arc::clone(&stopping);
        let thread_urgent = Arc::clone(&urgent_snapshot);
        let thread_frames = Arc::clone(&frames);
        let thread_offset = Arc::clone(&video_offset_ms);
        let sampler_target = target;
        let sampler_thread = thread::Builder::new()
            .name("scrawl-frame-sampler".into())
            .spawn(move || {
                platform::run_sampler(Sampler {
                    target: sampler_target,
                    video_output_path,
                    excluded_window_ids,
                    started_at,
                    stopping: thread_stopping,
                    urgent: thread_urgent,
                    frames: thread_frames,
                    video_offset_ms: thread_offset,
                })
            })
            .ok();

        Self {
            started_at,
            stopping,
            urgent_snapshot,
            frames,
            video_offset_ms,
            sampler_thread,
            _recording: crate::retention::Recording::start(),
        }
    }

    pub fn request_immediate_frame(&self) {
        self.urgent_snapshot.store(true, Ordering::Release);
        if let Some(ref handle) = self.sampler_thread {
            handle.thread().unpark();
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn stop(mut self) -> ScreenRecording {
        let duration_ms = self.elapsed_ms();
        self.stopping.store(true, Ordering::Release);
        let video_path = self
            .sampler_thread
            .take()
            .and_then(|handle| {
                handle.thread().unpark();
                handle.join().ok()
            })
            .flatten();
        let buffer = std::mem::replace(
            &mut *self
                .frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            CandidateBuffer::new(0),
        );
        let mut collected = buffer.into_frames();
        if collected.is_empty() {
            collected.push(crate::render::render_synthetic_demo_frame(1280, 800, 0, 0));
        }
        let valid_video =
            video_path.filter(|path| path.exists() && path.metadata().is_ok_and(|m| m.len() > 0));
        ScreenRecording {
            frames: collected,
            video: valid_video,
            duration_ms: duration_ms.max(100),
            video_offset_ms: self.video_offset_ms.load(Ordering::Acquire),
        }
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(handle) = self.sampler_thread.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

/// Shared state collected by the global input monitor during an active capture session.
pub struct SharedCaptureTelemetry {
    pub active: AtomicBool,
    pub selecting: AtomicBool,
    pub shift_held: AtomicBool,
    pub capture_mode: AtomicU64,
    pub shortcut: Mutex<ShortcutConfig>,
    pub started_at: Mutex<Instant>,
    pub picker_started_at: Mutex<Instant>,
    pub picker_rect: Mutex<Option<([f32; 4], u8)>>,
    pub target_frame: Mutex<TargetFrame>,
    pub pointer_screen: Mutex<[f32; 2]>,
    pub events: Mutex<Vec<InteractionEvent>>,
    pub strokes: Mutex<Vec<Stroke>>,
    pub active_laser: Mutex<Option<Stroke>>,
    pub live_ink: Mutex<Option<Stroke>>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    /// Raised when a click, ink stroke or shortcut starts, so the Screen-mode window log takes
    /// a snapshot right away.
    pub window_log_request: Arc<AtomicBool>,
    pub ui_sender: flume::Sender<LiveOverlayEvent>,
    /// Scrawl's windows that let clicks through (the ink overlay), skipped when deciding whether
    /// a click landed on Scrawl itself.
    pub passthrough_window_ids: Mutex<Vec<u32>>,
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    tap_port: AtomicU64,
    /// The event tap thread's run loop (retained), so `restart_input_tap` can stop it.
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    tap_run_loop: AtomicU64,
    /// A Screen recording started straight from the shortcut: the overlay flashes an edge glow.
    pub start_glow: AtomicBool,
    /// Whether the event tap is delivering. It can't be enabled without Input Monitoring, and
    /// macOS turns it off under Secure Input; the HID poller records clicks while it is off.
    tap_live: AtomicBool,
    last_shortcut_ms: AtomicU64,
    drag_start: Mutex<Option<(u64, f32, f32, Option<String>, TargetFrame)>>,
}

impl SharedCaptureTelemetry {
    pub fn new(ui_sender: flume::Sender<LiveOverlayEvent>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            selecting: AtomicBool::new(false),
            shift_held: AtomicBool::new(false),
            capture_mode: AtomicU64::new(0),
            shortcut: Mutex::new(ShortcutConfig::load()),
            started_at: Mutex::new(Instant::now()),
            picker_started_at: Mutex::new(Instant::now()),
            picker_rect: Mutex::new(None),
            target_frame: Mutex::new(TargetFrame {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
                scale: 2.0,
            }),
            pointer_screen: Mutex::new([0.0, 0.0]),
            events: Mutex::new(Vec::new()),
            strokes: Mutex::new(Vec::new()),
            active_laser: Mutex::new(None),
            live_ink: Mutex::new(None),
            excluded_window_ids: Arc::new(Mutex::new(Vec::new())),
            window_log_request: Arc::new(AtomicBool::new(false)),
            ui_sender,
            passthrough_window_ids: Mutex::new(Vec::new()),
            tap_port: AtomicU64::new(0),
            tap_run_loop: AtomicU64::new(0),
            tap_live: AtomicBool::new(false),
            start_glow: AtomicBool::new(false),
            last_shortcut_ms: AtomicU64::new(0),
            drag_start: Mutex::new(None),
        })
    }

    /// Each source (Carbon, the event tap, the HID poller) reports the same press within a few
    /// ms; a press after that is a new one, as when S is pressed again to switch modes.
    pub fn emit_capture_shortcut(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let prev = self.last_shortcut_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(prev) >= 90
            && self
                .last_shortcut_ms
                .compare_exchange(prev, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            scrawl_log("emit_capture_shortcut");
            let _ = self.ui_sender.try_send(LiveOverlayEvent::CaptureShortcut);
        }
    }

    pub fn emit_paste_shortcut(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let prev = self.last_shortcut_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(prev) >= 400
            && self
                .last_shortcut_ms
                .compare_exchange(prev, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            scrawl_log("emit_paste_shortcut -> sending PasteLatestShortcut");
            let _ = self
                .ui_sender
                .try_send(LiveOverlayEvent::PasteLatestShortcut);
        }
    }

    pub fn matches_shortcut(
        &self,
        keycode: u16,
        cmd: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> bool {
        let cfg = self
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        is_capture_shortcut(&cfg, layout_s_keycode(), keycode, cmd, ctrl, alt, shift)
    }

    pub fn set_capture_mode(&self, mode: CaptureMode) {
        let code = match mode {
            CaptureMode::Display => 0,
            CaptureMode::Region => 1,
            CaptureMode::Window => 2,
            CaptureMode::Device => 3,
        };
        self.capture_mode.store(code, Ordering::Release);
    }

    pub fn begin_picker_session(&self) {
        *self
            .picker_started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.active.store(false, Ordering::Release);
        self.selecting.store(true, Ordering::Release);
    }

    pub fn set_picker_rect(&self, rect: Option<([f32; 4], u8)>) {
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = rect;
    }

    pub fn reset_for_session(&self, frame: TargetFrame) {
        self.selecting.store(false, Ordering::Release);
        *self
            .picker_rect
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = frame;
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        *self
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.shift_held.store(false, Ordering::Release);
        self.window_log_request.store(false, Ordering::Release);
        self.active.store(true, Ordering::Release);
        let tap = if self.check_tap() {
            "event tap"
        } else {
            "polled mouse buttons (event tap off)"
        };
        scrawl_log(format!("session clicks from {tap}"));
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
            .as_millis() as u64
    }

    pub fn contains_screen_point(&self, screen_x: f64, screen_y: f64) -> bool {
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        frame.width >= 16.0
            && frame.height >= 16.0
            && frame.x > -10_000.0
            && screen_x >= frame.x
            && screen_x <= frame.x + frame.width
            && screen_y >= frame.y
            && screen_y <= frame.y + frame.height
    }

    /// A mouse button went down at a global point.
    fn pointer_down(&self, t_ms: u64, screen_x: f64, screen_y: f64) {
        if !self.contains_screen_point(screen_x, screen_y) {
            return;
        }
        let (nx, ny, _, _) = self.normalize_point(screen_x, screen_y);
        self.window_log_request.store(true, Ordering::Release);
        let start_frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((t_ms, nx, ny, None, start_frame));
    }

    /// A mouse button came up: records the click or drag that `pointer_down` began.
    fn pointer_up(&self, t_ms: u64, screen_x: f64, screen_y: f64, right: bool) {
        let Some((t0, x0, y0, start_label, start_frame)) = self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let end_frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !same_target_frame(start_frame, end_frame) {
            return;
        }
        let (nx, ny, fw, fh) = self.normalize_point(screen_x, screen_y);
        let dist_px = ((nx - x0) * fw as f32).hypot((ny - y0) * fh as f32);
        let _ = self.ui_sender.try_send(LiveOverlayEvent::RequestSnapshot);
        let event = if dist_px < 12.0 {
            InteractionEvent::Click {
                t: t0,
                x: x0,
                y: y0,
                button: Some(if right { "right" } else { "left" }.into()),
                label: start_label,
                role: None,
            }
        } else {
            InteractionEvent::Drag {
                t: t0,
                t1: t_ms,
                x0,
                y0,
                x1: nx,
                y1: ny,
                label: start_label,
                button: None,
                path: Vec::new(),
            }
        };
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    /// Whether the topmost window at a global point is Scrawl's own (ignoring the click-through
    /// ink overlay).
    fn on_scrawl_window(&self, x: f64, y: f64) -> bool {
        let passthrough = self
            .passthrough_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let windows = platform::raw_windows_now();
        crate::windows::owned_window_at(&windows, x, y, std::process::id() as i32, &passthrough)
    }

    /// Re-enables the input monitor if the system turned it off, and notes whether it is
    /// delivering.
    fn check_tap(&self) -> bool {
        platform::check_tap(self)
    }

    pub fn normalize_point(&self, screen_x: f64, screen_y: f64) -> (f32, f32, u32, u32) {
        let frame = *self
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nx = ((screen_x - frame.x) / frame.width.max(1.0)).clamp(0.0, 1.0) as f32;
        let ny = ((screen_y - frame.y) / frame.height.max(1.0)).clamp(0.0, 1.0) as f32;
        (
            nx,
            ny,
            frame.width.round().max(1.0) as u32,
            frame.height.round().max(1.0) as u32,
        )
    }

    pub fn finish_active_laser(&self, _ax_point: Option<(f64, f64)>) {
        if let Some(mut laser) = self
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && !laser.pts.is_empty()
        {
            if let Some(last) = laser.pts.chunks_exact(3).last() {
                let lx = last[0] as f32 / SCALE as f32;
                let ly = last[1] as f32 / SCALE as f32;
                laser.t1 = laser.t0.saturating_add(last[2].max(0) as u64);
                self.events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Point {
                        t: laser.t1,
                        x: lx,
                        y: ly,
                        label: None,
                    });
            }
            self.strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(laser);
        }
    }

    pub fn finish_session(&self) -> (Vec<InteractionEvent>, Vec<Stroke>) {
        self.active.store(false, Ordering::Release);
        self.selecting.store(false, Ordering::Release);
        *self
            .drag_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.finish_active_laser(None);
        if let Some(ink) = self
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && ink.pts.len() >= 3
        {
            self.strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(ink);
        }
        let events = std::mem::take(
            &mut *self
                .events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let strokes = std::mem::take(
            &mut *self
                .strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        (events, strokes)
    }
}

/// Clicks from the mouse buttons' polled state, while the event tap is off: a press starts a
/// click like the tap's mouse-down (unless it lands on Scrawl's own window), a release ends it.
fn poll_clicks(
    telemetry: &SharedCaptureTelemetry,
    prev: &mut [bool; 2],
    now: [bool; 2],
    (x, y): (f64, f64),
    pure_shift: bool,
    on_scrawl: impl Fn(f64, f64) -> bool,
) {
    let was = std::mem::replace(prev, now);
    if telemetry.tap_live.load(Ordering::Acquire) || pure_shift {
        return;
    }
    for (button, (&down, &before)) in now.iter().zip(&was).enumerate() {
        if down && !before && !on_scrawl(x, y) {
            telemetry.pointer_down(telemetry.elapsed_ms(), x, y);
        } else if !down && before {
            telemetry.pointer_up(telemetry.elapsed_ms(), x, y, button == 1);
        }
    }
}

fn same_target_frame(a: TargetFrame, b: TargetFrame) -> bool {
    (a.x - b.x).abs() <= 0.5
        && (a.y - b.y).abs() <= 0.5
        && (a.width - b.width).abs() <= 0.5
        && (a.height - b.height).abs() <= 0.5
}

/// One poll of the Shift ink: holding Shift alone and moving the cursor inside the capture
/// target draws a persistent stroke; letting go of Shift commits it. `pure_shift` is Shift
/// without other modifiers (and not left over from a chord such as the capture shortcut).
fn track_shift_ink(
    telemetry: &SharedCaptureTelemetry,
    (x, y): (f64, f64),
    pure_shift: bool,
    prev_shift_ink: bool,
) {
    let inside_target = telemetry.contains_screen_point(x, y);
    if pure_shift && inside_target {
        telemetry.shift_held.store(true, Ordering::Release);
        let t_ms = telemetry.elapsed_ms();
        let (nx, ny, fw, fh) = telemetry.normalize_point(x, y);
        let sx = (nx * SCALE as f32).round() as i32;
        let sy = (ny * SCALE as f32).round() as i32;
        let mut guard = telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.is_none() {
            telemetry.window_log_request.store(true, Ordering::Release);
        }
        let stroke = guard.get_or_insert_with(|| Stroke {
            t0: t_ms,
            t1: t_ms,
            fw,
            fh,
            on: None,
            pin: None,
            laser: false,
            pts: Vec::new(),
        });
        let dt = t_ms.saturating_sub(stroke.t0) as i32;
        let should_push = stroke.pts.chunks_exact(3).last().is_none_or(|prev| {
            let dx = (sx - prev[0]) as f32 / SCALE as f32 * fw as f32;
            let dy = (sy - prev[1]) as f32 / SCALE as f32 * fh as f32;
            dx.hypot(dy) >= 1.5
        });
        if should_push && stroke.pts.len() / 3 < 2_400 {
            stroke.pts.extend_from_slice(&[sx, sy, dt]);
            stroke.t1 = t_ms;
        }
    } else if !pure_shift && prev_shift_ink {
        telemetry.shift_held.store(false, Ordering::Release);
        if let Some(stroke) = telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            && stroke.pts.len() >= 6
        {
            let first = &stroke.pts[0..2];
            let max_span_px = stroke
                .pts
                .chunks_exact(3)
                .map(|p| {
                    let dx = (p[0] - first[0]) as f32 / SCALE as f32 * stroke.fw as f32;
                    let dy = (p[1] - first[1]) as f32 / SCALE as f32 * stroke.fh as f32;
                    dx.hypot(dy)
                })
                .fold(0.0_f32, f32::max);
            // Ignore stationary Shift taps (e.g. typing a capital letter without moving the mouse).
            if max_span_px >= 6.0 {
                telemetry
                    .strokes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(stroke);
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::RequestSnapshot);
            }
        }
    }
}

pub fn is_frontmost_terminal() -> bool {
    let Some((_pid, app, bundle_id)) = frontmost_app_info() else {
        return false;
    };
    let a = app.to_ascii_lowercase();
    let b = bundle_id.unwrap_or_default().to_ascii_lowercase();
    [
        "ghostty",
        "iterm",
        "terminal",
        "kitty",
        "wezterm",
        "alacritty",
        "warp",
        // Windows Terminal, the console host and the shells it runs.
        "windowsterminal",
        "conhost",
        "powershell",
        "pwsh",
        "cmd.exe",
        "mintty",
    ]
    .iter()
    .any(|needle| a.contains(needle) || b.contains(needle))
}

/// Writes all bundle artifacts (`scrawl.json`, `summary.md`, `contact-sheet.png`, `frames/frame-XX.png`)
/// into a new or specified `.scrawl` bundle directory.
pub fn write_bundle_artifacts(
    out_dir: &Path,
    bundle: &crate::model::ScrawlBundle,
    annotated_frames: &[(crate::model::Keyframe, RgbaFrame)],
    contact_sheet: &RgbaFrame,
    summary_md: &str,
) -> Result<()> {
    std::fs::create_dir_all(out_dir.join("frames"))
        .with_context(|| format!("could not create {}", out_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(out_dir, std::fs::Permissions::from_mode(0o700));
    }

    thread::scope(|scope| -> Result<()> {
        let workers: Vec<_> = annotated_frames
            .iter()
            .map(|(kf, frame)| {
                scope.spawn(move || -> Result<()> {
                    crate::render::save_png(frame, &out_dir.join(&kf.file))
                })
            })
            .collect();
        crate::render::save_png(contact_sheet, &out_dir.join(&bundle.contact_sheet))?;
        for worker in workers {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("PNG encoding worker panicked"))??;
        }
        Ok(())
    })?;

    let manifest_bytes = serde_json::to_vec_pretty(bundle)?;
    std::fs::write(out_dir.join("scrawl.json"), manifest_bytes)?;
    std::fs::write(out_dir.join("summary.md"), summary_md.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_stop_joins_worker_before_collecting_its_final_media() {
        let stopping = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(CandidateBuffer::new(1 << 20)));
        let worker_stopping = Arc::clone(&stopping);
        let worker_frames = Arc::clone(&frames);
        let path = std::env::temp_dir().join(crate::make_bundle_id());
        let worker_path = path.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker_stopping.load(Ordering::Acquire) && Instant::now() < deadline {
                thread::yield_now();
            }
            assert!(worker_stopping.load(Ordering::Acquire));
            worker_frames
                .lock()
                .unwrap()
                .push(RgbaFrame::new(42, 1, 1, vec![1, 2, 3, 255]), false);
            std::fs::write(&worker_path, b"finalized media").unwrap();
            Some(worker_path)
        });
        let session = ScreenCaptureSession {
            started_at: Instant::now(),
            stopping,
            urgent_snapshot: Arc::new(AtomicBool::new(false)),
            frames,
            video_offset_ms: Arc::new(AtomicU64::new(120)),
            sampler_thread: Some(worker),
            _recording: crate::retention::Recording::start(),
        };
        let recording = session.stop();
        let frames = recording.frames;
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].t_ms, 42);
        assert_eq!(frames[0].rgba, vec![1, 2, 3, 255]);
        assert_eq!(recording.video.as_ref(), Some(&path));
        assert_eq!(recording.video_offset_ms, 120);
        std::fs::remove_file(path).unwrap();
    }

    /// Kit's Screen capture on the Pro: the event tap was off (no Input Monitoring), so his
    /// clicks on OpenCode's composer never reached the bundle. Polled button state stands in.
    #[test]
    fn clicks_are_recorded_from_polled_buttons_while_the_event_tap_is_off() {
        let (sender, _receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(sender);
        telemetry.reset_for_session(TargetFrame {
            x: 0.0,
            y: 0.0,
            width: 1728.0,
            height: 1117.0,
            scale: 2.0,
        });
        assert!(!telemetry.tap_live.load(Ordering::Acquire));
        let mut prev = [false; 2];
        let not_scrawl = |_: f64, _: f64| false;
        let composer = (778.0, 1050.0);
        // A Shift-drag is ink, not a click.
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            true,
            not_scrawl,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            true,
            not_scrawl,
        );
        // A click on the composer.
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_scrawl,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_scrawl,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            false,
            not_scrawl,
        );
        // Then the stop button, which is Scrawl's own.
        let stop = (860.0, 20.0);
        poll_clicks(&telemetry, &mut prev, [true, false], stop, false, |_, _| {
            true
        });
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            stop,
            false,
            |_, _| true,
        );
        // While the tap is live it reports clicks itself.
        telemetry.tap_live.store(true, Ordering::Release);
        poll_clicks(
            &telemetry,
            &mut prev,
            [true, false],
            composer,
            false,
            not_scrawl,
        );
        poll_clicks(
            &telemetry,
            &mut prev,
            [false, false],
            composer,
            false,
            not_scrawl,
        );

        let (events, _) = telemetry.finish_session();
        assert_eq!(events.len(), 1, "{events:?}");
        let InteractionEvent::Click { x, y, button, .. } = &events[0] else {
            panic!("{events:?}");
        };
        assert!(
            (x - 0.45).abs() < 0.01 && (y - 0.94).abs() < 0.01,
            "{x}, {y}"
        );
        assert_eq!(button.as_deref(), Some("left"));
    }

    #[test]
    fn the_capture_shortcut_matches_its_aliases_and_a_custom_one() {
        let s = 1_u16; // the S key on this layout
        let default = ShortcutConfig {
            key_code: u32::from(s),
            carbon_modifiers: CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
            label: "⌥⇧S".into(),
        };
        // (cmd, ctrl, alt, shift)
        let hit = |cfg: &ShortcutConfig, key, (cmd, ctrl, alt, shift)| {
            is_capture_shortcut(cfg, s, key, cmd, ctrl, alt, shift)
        };
        assert!(hit(&default, s, (false, false, true, true)), "⌥⇧S");
        assert!(hit(&default, s, (true, false, true, true)), "⌥⇧⌘S");
        assert!(hit(&default, s, (false, true, true, true)), "⌃⌥⇧S");
        assert!(hit(&default, s, (true, true, true, false)), "⌃⌥⌘S");
        assert!(hit(&default, s, (true, true, true, true)), "hyper S");
        assert!(hit(&default, KEY_S, (false, false, true, true)), "ANSI S");
        assert!(!hit(&default, s, (false, false, false, true)), "⇧S");
        assert!(!hit(&default, s, (true, false, false, false)), "⌘S");
        assert!(!hit(&default, 2, (false, false, true, true)), "⌥⇧D");

        let custom = ShortcutConfig {
            key_code: 15,
            carbon_modifiers: CARBON_CONTROL_KEY | CARBON_OPTION_KEY,
            label: "⌃⌥R".into(),
        };
        assert!(hit(&custom, 15, (false, true, true, false)));
        assert!(!hit(&custom, 15, (true, true, true, false)));
        assert!(!hit(&custom, 15, (false, false, true, false)));

        assert_eq!(shortcut_parts("⌥⇧S"), ("⌥⇧", "S"));
        assert_eq!(shortcut_parts("⌃⌥⌘R"), ("⌃⌥⌘", "R"));
    }

    #[test]
    fn recordings_use_the_display_under_the_cursor() {
        let displays = [[0.0, 0.0, 1728.0, 1117.0], [1728.0, -200.0, 2560.0, 1440.0]];
        assert_eq!(pick_display(&displays, Some([2000.0, 300.0]), None), 1);
        assert_eq!(pick_display(&displays, Some([100.0, 100.0]), None), 0);
        // No cursor: the display holding the focused window's centre.
        let focused = Some([1900.0, 0.0, 800.0, 600.0]);
        assert_eq!(pick_display(&displays, None, focused), 1);
        assert_eq!(pick_display(&displays, Some([-50.0, 0.0]), focused), 1);
        assert_eq!(pick_display(&displays, None, None), 0);
    }

    #[test]
    fn candidate_buffer_stays_bounded_and_keeps_the_whole_timeline() {
        // A 1 MB budget of 4 KB frames: a 256-frame cap.
        let mut buffer = CandidateBuffer::new(1 << 20);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        let mut t = 0;
        let mut peak = 0;
        // Three hours at the sampler's pace, with a click frame every 10 s.
        while t < 3 * 3_600_000 {
            buffer.push(frame(t), t % 10_000 == 0);
            peak = peak.max(buffer.len());
            t += buffer.interval().as_millis() as u64;
        }
        assert!(peak <= 256, "peak {peak}");
        assert!(buffer.interval() >= FRAME_SAMPLE_INTERVAL * 64);
        let frames = buffer.into_frames();
        assert_eq!(frames[0].t_ms, 0, "the start is kept");
        assert!(frames.windows(2).all(|w| w[0].t_ms < w[1].t_ms));
        // Spread over the whole capture, not bunched at the end.
        let first_half = frames.iter().filter(|f| f.t_ms < 3 * 1_800_000).count();
        assert!(
            first_half * 3 > frames.len(),
            "{first_half} of {}",
            frames.len()
        );
    }

    #[test]
    fn candidate_buffer_keeps_urgent_frames_when_thinning() {
        let mut buffer = CandidateBuffer::new(40 * 4096);
        let frame = |t| RgbaFrame::new(t, 32, 32, vec![0; 32 * 32 * 4]);
        for t in 0..60 {
            buffer.push(frame(t * 220), t == 33);
        }
        assert!(buffer.len() < 60);
        assert_eq!(buffer.interval(), FRAME_SAMPLE_INTERVAL * 2);
        assert!(buffer.into_frames().iter().any(|f| f.t_ms == 33 * 220));
    }

    #[test]
    fn candidates_are_small_and_recordings_keep_full_size() {
        let target = |mode, width, height| TargetInfo {
            mode,
            app: None,
            bundle_id: None,
            window_title: None,
            window_id: None,
            display_id: 1,
            frame: TargetFrame {
                x: 0.0,
                y: 0.0,
                width,
                height,
                scale: 2.0,
            },
        };
        let display = target(CaptureMode::Display, 1728.0, 1117.0);
        assert_eq!(capture_size(&display, CANDIDATE_EDGE), (480, 310));
        let region = target(CaptureMode::Region, 480.0, 320.0);
        assert_eq!(capture_size(&region, CANDIDATE_EDGE), (480, 320));
        if cfg!(target_os = "windows") {
            // Frames are in pixels: never upscaled past the screen's own.
            assert_eq!(capture_size(&display, 2560.0), (1728, 1116));
            assert_eq!(capture_size(&region, 1280.0), (480, 320));
            let wide = target(CaptureMode::Display, 3440.0, 1440.0);
            assert_eq!(capture_size(&wide, 2560.0), (2560, 1072));
        } else {
            assert_eq!(capture_size(&display, 2560.0), (2560, 1654));
            assert_eq!(capture_size(&region, 1280.0), (960, 640));
        }
    }

    #[test]
    fn cached_window_hit_preserves_front_to_back_order_and_screen_coordinates() {
        let window = |id, x, y| TargetInfo {
            mode: CaptureMode::Window,
            app: Some(format!("App {id}")),
            bundle_id: None,
            window_title: Some(format!("Window {id}")),
            window_id: Some(id),
            display_id: 1,
            frame: TargetFrame {
                x,
                y,
                width: 200.0,
                height: 100.0,
                scale: 2.0,
            },
        };
        let windows = [window(1, -100.0, 20.0), window(2, 0.0, 0.0)];
        assert_eq!(
            selectable_window_at(&windows, 50.0, 50.0)
                .unwrap()
                .window_id,
            Some(1)
        );
        assert_eq!(
            selectable_window_at(&windows, 150.0, 50.0)
                .unwrap()
                .window_id,
            Some(2)
        );
        assert_eq!(
            selectable_window_at(&windows, -100.0, 20.0)
                .unwrap()
                .window_id,
            Some(1)
        );
        assert!(selectable_window_at(&windows, -101.0, 50.0).is_none());
        assert!(selectable_window_at(&[], 0.0, 0.0).is_none());
    }
}
