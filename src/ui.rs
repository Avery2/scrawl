use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Application, Bounds, BoxShadow, Context, Div, Entity, FocusHandle, FontWeight,
    Image, ImageFormat, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, SharedString, Size, TextRun, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, div, img, point,
    prelude::*, px, relative, rgb, rgba, size,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSStatusWindowLevel,
    NSWindow, NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::capture::{
    LiveOverlayEvent, ScreenCaptureSession, SharedCaptureTelemetry, discover_target,
    install_global_hotkeys, list_selectable_windows, region_target, scrawl_log,
    set_escape_hotkey_enabled, shortcut_from_key_and_mods, start_global_input_tap,
    update_global_shortcut,
};
use crate::hex::AudioRecorder;
use crate::laser_overlay::MetalLaserOverlay;
use crate::model::{ScrawlBundle, TargetInfo, format_clock};
use crate::sketch::timeline;

const SIDEBAR_BG: u32 = 0x111111;
const CANVAS_BG: u32 = 0x161616;
const SURFACE_BG: u32 = 0x1d1d1d;
const SURFACE_HOVER: u32 = 0x252525;
const SURFACE_SELECTED: u32 = 0x2e2e2e;
const LINE_COLOR: u32 = 0x2a2a2a;
const TEXT_COLOR: u32 = 0xf2f2f2;
const TEXT_SOFT: u32 = 0xd0d0d0;
const MUTED_COLOR: u32 = 0x969696;
const ACCENT_COLOR: u32 = 0x3b5cf6;
const ACCENT_HOVER: u32 = 0x4b6bff;
const INK_RED: u32 = 0xff3b30;
const POSITIVE_COLOR: u32 = 0x69d89f;

// Dynamic Island capsule geometry. GPUI boxes are border-box, so the hairline border is part of
// the inset: R_outer (20) = R_chip (16) + border (1) + pad (3), on every side.
const ISLAND_H: f32 = 40.0;
const ISLAND_RADIUS: f32 = ISLAND_H / 2.0;
const ISLAND_INSET: f32 = 4.0;
const ISLAND_PAD: f32 = ISLAND_INSET - 1.0;
const CHIP_H: f32 = ISLAND_H - 2.0 * ISLAND_INSET;
const CHIP_RADIUS: f32 = CHIP_H / 2.0;
const CHIP_PX: f32 = 12.0;
const CHIP_GAP: f32 = 2.0;
const DIVIDER_MX: f32 = 6.0;
const LEAD_TRAIL: f32 = 8.0;
const HUD_TEXT: f32 = 13.0;
const PILL_WINDOW_W: f32 = 440.0;
const PILL_WINDOW_H: f32 = 48.0;

#[derive(Clone, Copy, Debug)]
struct Spring {
    response: f32,
    damping_ratio: f32,
}

const ISLAND_SPRING: Spring = Spring {
    response: 0.42,
    damping_ratio: 0.78,
};

/// Advances a damped spring with fixed 4ms sub-steps so long frames stay stable.
fn spring_step(value: f32, velocity: f32, target: f32, dt: f32, spring: Spring) -> (f32, f32) {
    let omega = std::f32::consts::TAU / spring.response;
    let stiffness = omega * omega;
    let damping = 2.0 * spring.damping_ratio * omega;
    let steps = (dt / 0.004).ceil().max(1.0) as usize;
    let h = dt / steps as f32;
    let (mut x, mut v) = (value, velocity);
    for _ in 0..steps {
        v += ((target - x) * stiffness - v * damping) * h;
        x += v * h;
    }
    (x, v)
}

fn with_alpha(color: u32, alpha: f32) -> gpui::Rgba {
    rgba((color << 8) | (alpha.clamp(0.0, 1.0) * 255.0).round() as u32)
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn measure_text(window: &Window, text: &str, size: f32, weight: FontWeight) -> f32 {
    let mut font = window.text_style().font();
    font.weight = weight;
    let run = TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(size),
        &[run],
        None,
    );
    f32::from(line.width).ceil()
}

fn chip_width(window: &Window, label: &str, weight: FontWeight) -> f32 {
    measure_text(window, label, HUD_TEXT, weight) + 2.0 * CHIP_PX
}

fn clock_template(clock: &str) -> String {
    clock
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect()
}

fn completed_meta(duration_ms: u64, frame_count: usize) -> String {
    format!(
        "· {} · {} {}",
        format_clock(duration_ms),
        frame_count,
        if frame_count == 1 { "frame" } else { "frames" }
    )
}

/// Intrinsic outer width of the capsule for a phase, mirroring the layout in `HudPillWindow`.
fn island_content_width(window: &Window, phase: IslandPhase, clock: &str) -> f32 {
    let divider = 1.0 + 2.0 * DIVIDER_MX;
    let inner = match phase {
        IslandPhase::Selecting { .. } => {
            chip_width(window, "Area", FontWeight::MEDIUM)
                + chip_width(window, "Window", FontWeight::MEDIUM)
                + chip_width(window, "Full Screen", FontWeight::MEDIUM)
                + divider
                + CHIP_H
                + 4.0 * CHIP_GAP
        }
        IslandPhase::Recording => {
            CHIP_H
                + measure_text(
                    window,
                    &clock_template(clock),
                    HUD_TEXT,
                    FontWeight::SEMIBOLD,
                )
                + LEAD_TRAIL
                + divider
                + chip_width(window, "Undo", FontWeight::MEDIUM)
                + chip_width(window, "Cancel", FontWeight::MEDIUM)
                + chip_width(window, "Finish", FontWeight::SEMIBOLD)
                + 4.0 * CHIP_GAP
        }
        IslandPhase::Completed {
            frame_count,
            duration_ms,
            ..
        } => {
            CHIP_H
                + 4.0
                + measure_text(window, "Copied", HUD_TEXT, FontWeight::SEMIBOLD)
                + 5.0
                + measure_text(
                    window,
                    &completed_meta(duration_ms, frame_count),
                    HUD_TEXT,
                    FontWeight::NORMAL,
                )
                + LEAD_TRAIL
                + divider
                + chip_width(window, "Inspect", FontWeight::SEMIBOLD)
                + CHIP_H
                + 3.0 * CHIP_GAP
        }
        IslandPhase::Hidden | IslandPhase::Dismissing => return ISLAND_H,
    };
    inner + 2.0 * ISLAND_INSET
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    ToggleCapture,
    OpenLatest,
    RunDemo,
    Quit,
}

struct MenuTargetIvars {
    sender: flume::Sender<MenuAction>,
}

objc2::define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuTargetIvars]
    struct ScrawlMenuTarget;

    unsafe impl NSObjectProtocol for ScrawlMenuTarget {}

    impl ScrawlMenuTarget {
        #[unsafe(method(toggleCapture:))]
        fn toggle_capture(&self, _sender: &AnyObject) {
            let _ = self.ivars().sender.try_send(MenuAction::ToggleCapture);
        }

        #[unsafe(method(openLatest:))]
        fn open_latest(&self, _sender: &AnyObject) {
            let _ = self.ivars().sender.try_send(MenuAction::OpenLatest);
        }

        #[unsafe(method(runDemo:))]
        fn run_demo(&self, _sender: &AnyObject) {
            let _ = self.ivars().sender.try_send(MenuAction::RunDemo);
        }

        #[unsafe(method(quitApp:))]
        fn quit_app(&self, _sender: &AnyObject) {
            let _ = self.ivars().sender.try_send(MenuAction::Quit);
        }
    }
);

impl ScrawlMenuTarget {
    fn new(sender: flume::Sender<MenuAction>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuTargetIvars { sender });
        unsafe { msg_send![super(this), init] }
    }
}

struct StatusBarController {
    _item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
    _target: Retained<ScrawlMenuTarget>,
}

thread_local! {
    static STATUS_BAR: RefCell<Option<StatusBarController>> = const { RefCell::new(None) };
}

fn install_status_bar() -> Option<flume::Receiver<MenuAction>> {
    let mtm = MainThreadMarker::new()?;
    let (sender, receiver) = flume::bounded(16);
    let target = ScrawlMenuTarget::new(sender, mtm);
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Scrawl"));

    let add = |title: &str, action: objc2::runtime::Sel| {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::new(),
            )
        };
        unsafe { item.setTarget(Some(&target)) };
        menu.addItem(&item);
    };

    add("Record Scrawl  ⌃⌥⇧S", sel!(toggleCapture:));
    add("Open Latest Bundle", sel!(openLatest:));
    add("Generate Demo Bundle", sel!(runDemo:));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add("Quit Scrawl", sel!(quitApp:));

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
    if let Some(button) = item.button(mtm) {
        button.setTitle(&NSString::from_str("✎"));
    }
    item.setMenu(Some(&menu));

    STATUS_BAR.with(|slot| {
        *slot.borrow_mut() = Some(StatusBarController {
            _item: item,
            _menu: menu,
            _target: target,
        });
    });
    Some(receiver)
}

fn newest_ns_window() -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    windows.lastObject()
}

fn sync_full_size_content_view(ns_window: &NSWindow) {
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
    if let Some(cv) = ns_window.contentView() {
        let bounds = cv.bounds();
        for sub in cv.subviews() {
            sub.setFrame(bounds);
        }
        if let Some(frame_view) = unsafe { cv.superview() } {
            for sub in frame_view.subviews() {
                if sub
                    .class()
                    .name()
                    .to_bytes()
                    .windows(8)
                    .any(|w| w == b"Titlebar")
                {
                    sub.setHidden(true);
                }
            }
        }
    }
}

fn configure_floating_window(ns_window: &NSWindow, ignores_mouse: bool, level_offset: isize) {
    ns_window.setHidesOnDeactivate(false);
    ns_window.setIgnoresMouseEvents(ignores_mouse);
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setHasShadow(false);
    ns_window.setLevel(NSStatusWindowLevel + level_offset);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    sync_full_size_content_view(ns_window);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionMode {
    Area,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IslandPhase {
    Hidden,
    Selecting {
        mode: SelectionMode,
    },
    Recording,
    Completed {
        finished_at: Instant,
        frame_count: usize,
        duration_ms: u64,
    },
    Dismissing,
}

struct ActiveRecording {
    target: TargetInfo,
    bundle_dir: PathBuf,
    screen_session: ScreenCaptureSession,
    audio_recorder: Option<AudioRecorder>,
    _metal_overlay: Option<MetalLaserOverlay>,
}

/// Shared coordinator owned by the GPUI app.
pub struct ScrawlCoordinator {
    telemetry: Arc<SharedCaptureTelemetry>,
    active_recording: Option<ActiveRecording>,
    island_phase: IslandPhase,
    island_phase_since: Instant,
    island_dismissed_from: IslandPhase,
    island_width: f32,
    island_target_width: f32,
    island_velocity: f32,
    island_opacity: f32,
    selectable_windows: Vec<TargetInfo>,
    hovered_window: Option<TargetInfo>,
    selection_drag: Option<([f32; 2], [f32; 2])>,
    recording_shortcut_key: bool,
    review_bundle: Option<(PathBuf, ScrawlBundle)>,
    review_status: Option<String>,
    selected_tab: ReviewTab,
    selected_keyframe: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewTab {
    ContactSheet,
    Keyframes,
}

impl ScrawlCoordinator {
    fn new(
        telemetry: Arc<SharedCaptureTelemetry>,
        initial_bundle: Option<(PathBuf, ScrawlBundle)>,
    ) -> Self {
        Self {
            telemetry,
            active_recording: None,
            island_phase: IslandPhase::Hidden,
            island_phase_since: Instant::now(),
            island_dismissed_from: IslandPhase::Hidden,
            island_width: 220.0,
            island_target_width: 220.0,
            island_velocity: 0.0,
            island_opacity: 0.0,
            selectable_windows: Vec::new(),
            hovered_window: None,
            selection_drag: None,
            recording_shortcut_key: false,
            review_bundle: initial_bundle,
            review_status: None,
            selected_tab: ReviewTab::ContactSheet,
            selected_keyframe: 0,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.active_recording.is_some()
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self.island_phase, IslandPhase::Selecting { .. })
    }

    pub fn is_island_visible(&self) -> bool {
        !matches!(self.island_phase, IslandPhase::Hidden)
    }

    pub fn shortcut_label(&self) -> String {
        self.telemetry
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .label
            .clone()
    }

    pub fn set_review_bundle(&mut self, bundle: (PathBuf, ScrawlBundle), status: Option<String>) {
        self.review_bundle = Some(bundle);
        self.review_status = status;
        self.selected_tab = ReviewTab::ContactSheet;
        self.selected_keyframe = 0;
    }

    pub fn set_selection_mode(&mut self, mode: SelectionMode, cx: &mut Context<Self>) {
        if matches!(self.island_phase, IslandPhase::Selecting { .. }) {
            self.island_phase = IslandPhase::Selecting { mode };
            self.selection_drag = None;
            cx.notify();
        }
    }

    pub fn toggle_selection_mode(&mut self, cx: &mut Context<Self>) {
        if let IslandPhase::Selecting { mode } = self.island_phase {
            let next = match mode {
                SelectionMode::Area => SelectionMode::Window,
                SelectionMode::Window => SelectionMode::Area,
            };
            self.set_selection_mode(next, cx);
        }
    }

    pub fn undo_stroke(&mut self, cx: &mut Context<Self>) {
        let mut strokes = self
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(idx) = strokes.iter().rposition(|s| !s.laser) {
            strokes.remove(idx);
        }
        drop(strokes);
        cx.notify();
    }

    pub fn request_immediate_frame(&self) {
        if let Some(ref rec) = self.active_recording {
            rec.screen_session.request_immediate_frame();
        }
    }

    /// Moves the island to `phase`, restarting the content cross-fade and giving the capsule a
    /// small inward impulse when the phase kind changes (mode toggles keep their content).
    fn set_island_phase(&mut self, phase: IslandPhase) {
        let changed = std::mem::discriminant(&self.island_phase) != std::mem::discriminant(&phase);
        if changed {
            if matches!(phase, IslandPhase::Dismissing) {
                self.island_dismissed_from = self.island_phase;
            } else if !matches!(self.island_phase, IslandPhase::Hidden) {
                self.island_velocity -= 140.0;
            }
            self.island_phase_since = Instant::now();
        }
        self.island_phase = phase;
    }

    pub fn dismiss_island(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.island_phase,
            IslandPhase::Hidden | IslandPhase::Dismissing
        ) {
            return;
        }
        self.selection_drag = None;
        self.hovered_window = None;
        self.set_island_phase(IslandPhase::Dismissing);
        cx.defer(close_picker_window);
        cx.notify();
    }

    pub fn inspect_completed_bundle(&mut self, cx: &mut Context<Self>) {
        self.set_island_phase(IslandPhase::Hidden);
        self.island_opacity = 0.0;
        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_capture_windows(cx);
            open_review_window(&entity, cx);
        });
        cx.notify();
    }

    pub fn step_island(&mut self, dt_sec: f32, cx: &mut Context<Self>) {
        let dt = dt_sec.clamp(0.001, 0.05);
        if let IslandPhase::Completed { finished_at, .. } = self.island_phase
            && finished_at.elapsed() >= Duration::from_millis(5_000)
        {
            self.set_island_phase(IslandPhase::Dismissing);
        }

        // `island_target_width` is the measured intrinsic width of the visible content, written
        // by `HudPillWindow::render`; dismissal contracts toward ~80% while fading out.
        let (target_w, target_opacity, fade_rate) = match self.island_phase {
            IslandPhase::Hidden => return,
            IslandPhase::Dismissing => (self.island_target_width * 0.8, 0.0, 12.0),
            _ => (self.island_target_width, 1.0, 16.0),
        };

        let (width, velocity) = spring_step(
            self.island_width,
            self.island_velocity,
            target_w,
            dt,
            ISLAND_SPRING,
        );
        self.island_width = width.clamp(ISLAND_H, PILL_WINDOW_W - 8.0);
        self.island_velocity = velocity;
        self.island_opacity += (target_opacity - self.island_opacity) * (fade_rate * dt).min(1.0);

        if matches!(self.island_phase, IslandPhase::Dismissing) && self.island_opacity < 0.04 {
            self.set_island_phase(IslandPhase::Hidden);
            self.island_opacity = 0.0;
            cx.defer(close_capture_windows);
        }
        cx.notify();
    }

    pub fn begin_target_selection(&mut self, cx: &mut Context<Self>) {
        if self.is_recording() {
            return;
        }
        scrawl_log("begin_target_selection invoked");
        self.recording_shortcut_key = false;
        self.selectable_windows = list_selectable_windows();
        self.hovered_window = self.selectable_windows.first().cloned();
        self.selection_drag = None;
        if matches!(
            self.island_phase,
            IslandPhase::Hidden | IslandPhase::Dismissing
        ) {
            // Bloom out of a compact capsule; a visible island morphs from its current width.
            self.island_width = (self.island_target_width * 0.62).max(ISLAND_H * 3.0);
            self.island_velocity = 0.0;
            self.island_opacity = 0.0;
            self.island_phase = IslandPhase::Hidden;
        }
        self.set_island_phase(IslandPhase::Selecting {
            mode: SelectionMode::Area,
        });

        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_review_window(cx);
            open_picker_windows(&entity, cx);
        });
        cx.notify();
    }

    pub fn start_recording_full_screen(&mut self, cx: &mut Context<Self>) {
        let target = discover_target(None);
        self.start_recording_with_target(target, cx);
    }

    pub fn start_recording(&mut self, cx: &mut Context<Self>) {
        self.begin_target_selection(cx);
    }

    pub fn start_recording_with_target(&mut self, target: TargetInfo, cx: &mut Context<Self>) {
        if self.is_recording() {
            return;
        }
        scrawl_log(format!(
            "start_recording_with_target invoked: mode={:?} frame={:?}",
            target.mode, target.frame
        ));
        self.recording_shortcut_key = false;
        self.selection_drag = None;
        self.hovered_window = None;
        let id = crate::make_bundle_id();
        let bundle_dir = crate::handoff::default_captures_dir().join(format!("{id}.scrawl"));
        let _ = std::fs::create_dir_all(&bundle_dir);
        let video_path = bundle_dir.join("recording.mp4");

        self.telemetry.reset_for_session(target.frame);
        set_escape_hotkey_enabled(true);
        let metal_overlay = MetalLaserOverlay::open(Arc::clone(&self.telemetry)).ok();
        if let Some(ref overlay) = metal_overlay {
            self.telemetry
                .excluded_window_ids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(overlay.window_number());
        }
        let audio_recorder = AudioRecorder::start().ok();
        let screen_session = ScreenCaptureSession::start(
            target.clone(),
            Some(video_path),
            Arc::clone(&self.telemetry.excluded_window_ids),
        );
        self.active_recording = Some(ActiveRecording {
            target,
            bundle_dir,
            screen_session,
            audio_recorder,
            _metal_overlay: metal_overlay,
        });
        if matches!(self.island_phase, IslandPhase::Hidden) {
            self.island_width = (self.island_target_width * 0.62).max(ISLAND_H * 3.0);
            self.island_velocity = 0.0;
        }
        self.set_island_phase(IslandPhase::Recording);

        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_review_window(cx);
            close_picker_window(cx);
            ensure_hud_pill_open(&entity, cx);
        });
        cx.notify();
    }

    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        scrawl_log("cancel_recording invoked");
        set_escape_hotkey_enabled(false);
        let Some(rec) = self.active_recording.take() else {
            self.dismiss_island(cx);
            return;
        };
        let _ = self.telemetry.finish_session();
        let _ = rec.screen_session.stop();
        if let Some(audio) = rec.audio_recorder {
            let _ = audio.stop();
        }
        let _ = std::fs::remove_dir_all(rec.bundle_dir);
        self.dismiss_island(cx);
    }

    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        scrawl_log("finish_recording invoked");
        set_escape_hotkey_enabled(false);
        let Some(rec) = self.active_recording.take() else {
            return;
        };
        let (events, strokes) = self.telemetry.finish_session();
        let bundle_dir = rec.bundle_dir;
        let target = rec.target;
        let screen_session = rec.screen_session;
        let audio_recorder = rec.audio_recorder;
        let ((frames, video_path, duration_ms), recorded_audio) = std::thread::scope(|s| {
            let screen_handle = s.spawn(|| screen_session.stop());
            let audio = audio_recorder.and_then(|r| r.stop().ok());
            (screen_handle.join().unwrap(), audio)
        });

        let mut frame_count = 1;
        if let Ok((dir, bundle, _)) = crate::finalize_capture_bundle(
            &bundle_dir,
            target,
            duration_ms,
            frames,
            strokes,
            events,
            recorded_audio,
            video_path,
            None,
        ) {
            frame_count = bundle.keyframes.len();
            let _ = crate::handoff::copy_bundle_to_clipboard(&dir);
            self.set_review_bundle((dir, bundle), None);
        }
        // Morph the floating HUD pill Dynamic Island-style into the "Copied" state
        // without opening the full Review window unless the user clicks "Inspect".
        self.set_island_phase(IslandPhase::Completed {
            finished_at: Instant::now(),
            frame_count,
            duration_ms,
        });
        cx.notify();
    }
}

pub struct HudPillWindow {
    coordinator: Entity<ScrawlCoordinator>,
}

impl Render for HudPillWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let coord = self.coordinator.read(cx);
        let phase = coord.island_phase;
        let content_phase = match phase {
            IslandPhase::Dismissing => coord.island_dismissed_from,
            other => other,
        };
        let phase_age = coord.island_phase_since.elapsed().as_secs_f32();
        let width = coord.island_width;
        let opacity = coord.island_opacity.clamp(0.0, 1.0);
        let elapsed_ms = coord
            .active_recording
            .as_ref()
            .map_or(0, |r| r.screen_session.elapsed_ms());
        let stroke_count = coord
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|s| !s.laser)
            .count();

        let clock = format_clock(elapsed_ms);
        let target_width = island_content_width(window, content_phase, &clock);
        self.coordinator
            .update(cx, |coord, _| coord.island_target_width = target_width);

        // New content fades in once the capsule has started morphing toward its width.
        let content_alpha = if matches!(phase, IslandPhase::Dismissing) {
            1.0
        } else {
            ease_out_cubic((phase_age - 0.04) / 0.2)
        };

        let c_undo = self.coordinator.clone();
        let c_cancel = self.coordinator.clone();
        let c_finish = self.coordinator.clone();
        let c_inspect = self.coordinator.clone();
        let c_dismiss = self.coordinator.clone();
        let c_mode_area = self.coordinator.clone();
        let c_mode_win = self.coordinator.clone();
        let c_full = self.coordinator.clone();
        let c_sel_cancel = self.coordinator.clone();

        let row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(CHIP_GAP))
            .whitespace_nowrap()
            .opacity(content_alpha);

        let capsule_content = match content_phase {
            IslandPhase::Selecting { mode } => row
                .child(
                    mode_chip("sel-area", "Area", mode == SelectionMode::Area).on_click(
                        move |_, _, cx| {
                            c_mode_area.update(cx, |coord, cx| {
                                coord.set_selection_mode(SelectionMode::Area, cx);
                            });
                        },
                    ),
                )
                .child(
                    mode_chip("sel-window", "Window", mode == SelectionMode::Window).on_click(
                        move |_, _, cx| {
                            c_mode_win.update(cx, |coord, cx| {
                                coord.set_selection_mode(SelectionMode::Window, cx);
                            });
                        },
                    ),
                )
                .child(pill_chip("sel-fullscreen", "Full Screen", true).on_click(
                    move |_, _, cx| {
                        c_full.update(cx, |coord, cx| {
                            coord.start_recording_full_screen(cx);
                        });
                    },
                ))
                .child(island_divider())
                .child(
                    icon_circle_chip("sel-cancel", "✕").on_click(move |_, _, cx| {
                        c_sel_cancel.update(cx, |coord, cx| coord.cancel_recording(cx));
                    }),
                ),
            IslandPhase::Completed {
                frame_count,
                duration_ms,
                ..
            } => row
                .child(
                    island_lead()
                        .child(
                            div()
                                .size(px(CHIP_H))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .size(px(24.0))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(rgba(0x34c7592e))
                                        .text_color(rgb(POSITIVE_COLOR))
                                        .text_size(px(12.0))
                                        .font_weight(FontWeight::BOLD)
                                        .child("✓"),
                                ),
                        )
                        .child(
                            div()
                                .ml(px(4.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(TEXT_COLOR))
                                .child("Copied"),
                        )
                        .child(
                            div()
                                .ml(px(5.0))
                                .text_color(rgb(MUTED_COLOR))
                                .child(completed_meta(duration_ms, frame_count)),
                        ),
                )
                .child(island_divider())
                .child(
                    primary_chip("hud-inspect", "Inspect").on_click(move |_, _, cx| {
                        c_inspect.update(cx, |coord, cx| coord.inspect_completed_bundle(cx));
                    }),
                )
                .child(
                    icon_circle_chip("hud-dismiss", "✕").on_click(move |_, _, cx| {
                        c_dismiss.update(cx, |coord, cx| coord.dismiss_island(cx));
                    }),
                ),
            IslandPhase::Recording => {
                // A soft ripple leaves the dot every 1.6s while the dot itself breathes.
                let cycle = 1.6;
                let ripple = (phase_age / cycle).fract();
                let breath = 0.5 + 0.5 * (std::f32::consts::TAU * phase_age / cycle).cos();
                let halo = 8.0 + 12.0 * ease_out_cubic(ripple);
                let clock_w = measure_text(
                    window,
                    &clock_template(&clock),
                    HUD_TEXT,
                    FontWeight::SEMIBOLD,
                );
                row.child(
                    island_lead()
                        .child(
                            div()
                                .size(px(CHIP_H))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .size(px(halo))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(with_alpha(INK_RED, 0.34 * (1.0 - ripple).powi(2)))
                                        .child(
                                            div()
                                                .size(px(8.0))
                                                .rounded_full()
                                                .bg(with_alpha(INK_RED, 0.72 + 0.28 * breath)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .w(px(clock_w))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(clock),
                        ),
                )
                .child(island_divider())
                .child(pill_chip("hud-undo", "Undo", stroke_count > 0).when(
                    stroke_count > 0,
                    |d| {
                        d.on_click(move |_, _, cx| {
                            c_undo.update(cx, |coord, cx| coord.undo_stroke(cx));
                        })
                    },
                ))
                .child(
                    pill_chip("hud-cancel", "Cancel", true).on_click(move |_, _, cx| {
                        c_cancel.update(cx, |coord, cx| coord.cancel_recording(cx));
                    }),
                )
                .child(
                    primary_chip("hud-done", "Finish").on_click(move |_, _, cx| {
                        c_finish.update(cx, |coord, cx| coord.finish_recording(cx));
                    }),
                )
            }
            IslandPhase::Hidden | IslandPhase::Dismissing => row,
        };

        let is_completed = matches!(content_phase, IslandPhase::Completed { .. });
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .opacity(opacity)
            .child(
                div()
                    .w(px(width))
                    .h(px(ISLAND_H))
                    .flex_none()
                    .p(px(ISLAND_PAD))
                    .rounded(px(ISLAND_RADIUS))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgba(0x141416f5))
                    .border_1()
                    .border_color(if is_completed {
                        rgba(0x69d89f4d)
                    } else {
                        rgba(0xffffff1a)
                    })
                    .text_color(rgb(TEXT_COLOR))
                    .text_size(px(HUD_TEXT))
                    .whitespace_nowrap()
                    .child(capsule_content),
            )
    }
}

/// Leading status cluster: a 32px slot concentric with the capsule's left cap, then text.
/// Also acts as the drag handle for the floating island.
fn island_lead() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .pr(px(LEAD_TRAIL))
        .whitespace_nowrap()
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.start_window_move();
        })
}

fn island_divider() -> Div {
    div()
        .w(px(1.0))
        .h(px(16.0))
        .mx(px(DIVIDER_MX))
        .flex_none()
        .bg(rgba(0xffffff1a))
}

fn hud_chip(id: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(CHIP_H))
        .px(px(CHIP_PX))
        .rounded(px(CHIP_RADIUS))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .whitespace_nowrap()
}

fn pill_chip(id: &'static str, label: &'static str, enabled: bool) -> gpui::Stateful<Div> {
    hud_chip(id)
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled {
            rgb(TEXT_SOFT)
        } else {
            rgb(0x56565a)
        })
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(rgba(0xffffff14)).text_color(rgb(TEXT_COLOR)))
        })
        .child(label)
}

fn primary_chip(id: &'static str, label: &'static str) -> gpui::Stateful<Div> {
    hud_chip(id)
        .bg(rgb(ACCENT_COLOR))
        .text_color(rgb(TEXT_COLOR))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.bg(rgb(ACCENT_HOVER)))
        .child(label)
}

fn icon_circle_chip(id: &'static str, icon: &'static str) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .size(px(CHIP_H))
        .rounded_full()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .text_size(px(12.0))
        .text_color(rgb(MUTED_COLOR))
        .cursor_pointer()
        .hover(|s| s.bg(rgba(0xffffff14)).text_color(rgb(TEXT_COLOR)))
        .child(icon)
}

/// Area/Window mode toggle. Weight stays constant across states so toggling never shifts the
/// measured capsule width.
fn mode_chip(id: &'static str, label: &'static str, active: bool) -> gpui::Stateful<Div> {
    hud_chip(id)
        .font_weight(FontWeight::MEDIUM)
        .bg(if active {
            rgba(0xffffff26)
        } else {
            rgba(0x00000000)
        })
        .text_color(if active {
            rgb(TEXT_COLOR)
        } else {
            rgb(TEXT_SOFT)
        })
        .cursor_pointer()
        .hover(|s| {
            s.bg(if active {
                rgba(0xffffff2e)
            } else {
                rgba(0xffffff14)
            })
            .text_color(rgb(TEXT_COLOR))
        })
        .child(label)
}

pub struct TargetPickerWindow {
    coordinator: Entity<ScrawlCoordinator>,
    focus: FocusHandle,
}

impl Render for TargetPickerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let coord = self.coordinator.read(cx);
        let mode = match coord.island_phase {
            IslandPhase::Selecting { mode } => mode,
            _ => SelectionMode::Area,
        };
        let drag = coord.selection_drag;
        let hovered = coord.hovered_window.clone();

        let c_down = self.coordinator.clone();
        let c_move = self.coordinator.clone();
        let c_up = self.coordinator.clone();
        let c_key = self.coordinator.clone();

        let win_bounds = window.bounds();
        let origin = [
            f32::from(win_bounds.origin.x),
            f32::from(win_bounds.origin.y),
        ];
        let screen = [
            f32::from(win_bounds.size.width),
            f32::from(win_bounds.size.height),
        ];

        // Selection rect in window-local coordinates, clipped to the screen, plus its badge.
        let highlight: Option<([f32; 4], String)> = match mode {
            SelectionMode::Area => drag.map(|(a, b)| {
                let w = (a[0] - b[0]).abs().max(1.0);
                let h = (a[1] - b[1]).abs().max(1.0);
                (
                    [a[0].min(b[0]) - origin[0], a[1].min(b[1]) - origin[1], w, h],
                    format!("{} × {}", w.round() as i32, h.round() as i32),
                )
            }),
            SelectionMode::Window => hovered.map(|t| {
                let title = match (&t.app, &t.window_title) {
                    (Some(app), Some(win)) if !win.is_empty() && win != app => {
                        format!("{app} — {win}")
                    }
                    (Some(app), _) => app.clone(),
                    _ => format!(
                        "{} × {}",
                        t.frame.width.round() as i32,
                        t.frame.height.round() as i32
                    ),
                };
                (
                    [
                        t.frame.x as f32 - origin[0],
                        t.frame.y as f32 - origin[1],
                        t.frame.width as f32,
                        t.frame.height as f32,
                    ],
                    title,
                )
            }),
        }
        .map(|(rect, label)| (clip_rect(rect, screen), label));

        let badge = highlight.as_ref().map(|(rect, label)| {
            let max_text = (screen[0] - 2.0 * BADGE_MARGIN - 2.0 * BADGE_PX - 2.0).min(480.0);
            let text_w = measure_text(window, label, BADGE_TEXT, FontWeight::MEDIUM).min(max_text);
            let size = [text_w + 2.0 * BADGE_PX + 2.0, BADGE_H];
            let anchor = match mode {
                SelectionMode::Area => BadgeAnchor::BelowTrailing,
                SelectionMode::Window => BadgeAnchor::Center,
            };
            (
                place_badge(*rect, size, screen, anchor),
                text_w,
                label.clone(),
            )
        });

        let dim = rgba(0x00000052);
        let mut root = div()
            .id("scrawl-target-picker")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .when(highlight.is_none(), |d| d.bg(dim))
            .cursor_crosshair();

        if let Some(([x, y, w, h], _)) = highlight {
            // Dim everything around the selection so the target itself stays untouched.
            for [dx, dy, dw, dh] in [
                [0.0, 0.0, screen[0], y],
                [0.0, y + h, screen[0], screen[1] - y - h],
                [0.0, y, x, h],
                [x + w, y, screen[0] - x - w, h],
            ] {
                if dw > 0.0 && dh > 0.0 {
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(dx))
                            .top(px(dy))
                            .w(px(dw))
                            .h(px(dh))
                            .bg(dim),
                    );
                }
            }
            let frame = div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h));
            root = root.child(match mode {
                // Crisp 1px white edge with a dark hairline outside, legible on any content.
                SelectionMode::Area => {
                    frame
                        .border_1()
                        .border_color(rgba(0xffffffeb))
                        .shadow(vec![BoxShadow {
                            color: gpui::hsla(0.0, 0.0, 0.0, 0.35),
                            offset: point(px(0.0), px(0.0)),
                            blur_radius: px(0.0),
                            spread_radius: px(1.0),
                        }])
                }
                SelectionMode::Window => frame
                    .rounded(px(10.0))
                    .bg(rgba(0x3b5cf621))
                    .border_2()
                    .border_color(rgba(0x5b7cfaee)),
            });
        }

        root.when_some(badge, |d, ([bx, by], text_w, label)| {
            d.child(
                div()
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .h(px(BADGE_H))
                    .px(px(BADGE_PX))
                    .rounded(px(BADGE_H / 2.0))
                    .flex()
                    .items_center()
                    .bg(rgba(0x1c1c1ef2))
                    .border_1()
                    .border_color(rgba(0xffffff21))
                    .shadow(vec![BoxShadow {
                        color: gpui::hsla(0.0, 0.0, 0.0, 0.3),
                        offset: point(px(0.0), px(2.0)),
                        blur_radius: px(8.0),
                        spread_radius: px(0.0),
                    }])
                    .text_size(px(BADGE_TEXT))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT_COLOR))
                    .child(div().w(px(text_w)).truncate().child(label)),
            )
        })
        .on_key_down(move |event: &KeyDownEvent, _, cx| {
            let key = event.keystroke.key.as_str();
            if key == "space" || key == " " {
                c_key.update(cx, |coord, cx| coord.toggle_selection_mode(cx));
            } else if key == "escape" {
                c_key.update(cx, |coord, cx| coord.cancel_recording(cx));
            } else if key == "enter" || key.eq_ignore_ascii_case("s") {
                c_key.update(cx, |coord, cx| coord.start_recording_full_screen(cx));
            }
        })
        .on_mouse_down(
            MouseButton::Left,
            move |event: &MouseDownEvent, window, cx| {
                let win_bounds = window.bounds();
                let sx = f32::from(win_bounds.origin.x) + f32::from(event.position.x);
                let sy = f32::from(win_bounds.origin.y) + f32::from(event.position.y);
                c_down.update(cx, |coord, cx| {
                    let cur_mode = match coord.island_phase {
                        IslandPhase::Selecting { mode } => mode,
                        _ => return,
                    };
                    match cur_mode {
                        SelectionMode::Area => {
                            coord.selection_drag = Some(([sx, sy], [sx, sy]));
                            cx.notify();
                        }
                        SelectionMode::Window => {
                            if let Some(target) = coord.hovered_window.clone() {
                                coord.start_recording_with_target(target, cx);
                            } else {
                                coord.start_recording_full_screen(cx);
                            }
                        }
                    }
                });
            },
        )
        .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
            let win_bounds = window.bounds();
            let sx = f32::from(win_bounds.origin.x) + f32::from(event.position.x);
            let sy = f32::from(win_bounds.origin.y) + f32::from(event.position.y);
            c_move.update(cx, |coord, cx| {
                let cur_mode = match coord.island_phase {
                    IslandPhase::Selecting { mode } => mode,
                    _ => return,
                };
                match cur_mode {
                    SelectionMode::Area => {
                        if event.dragging()
                            && let Some((start, _)) = coord.selection_drag
                        {
                            coord.selection_drag = Some((start, [sx, sy]));
                            cx.notify();
                        }
                    }
                    SelectionMode::Window => {
                        let found = coord
                            .selectable_windows
                            .iter()
                            .find(|w| {
                                let f = w.frame;
                                f64::from(sx) >= f.x
                                    && f64::from(sx) <= f.x + f.width
                                    && f64::from(sy) >= f.y
                                    && f64::from(sy) <= f.y + f.height
                            })
                            .cloned();
                        if coord.hovered_window.as_ref().and_then(|w| w.window_id)
                            != found.as_ref().and_then(|w| w.window_id)
                        {
                            coord.hovered_window = found;
                            cx.notify();
                        }
                    }
                }
            });
        })
        .on_mouse_up(
            MouseButton::Left,
            move |event: &MouseUpEvent, window, cx| {
                let win_bounds = window.bounds();
                let sx = f32::from(win_bounds.origin.x) + f32::from(event.position.x);
                let sy = f32::from(win_bounds.origin.y) + f32::from(event.position.y);
                c_up.update(cx, |coord, cx| {
                    if let Some((start, _)) = coord.selection_drag.take() {
                        let x = f64::from(start[0].min(sx));
                        let y = f64::from(start[1].min(sy));
                        let w = f64::from((start[0] - sx).abs());
                        let h = f64::from((start[1] - sy).abs());
                        if w >= 36.0 && h >= 36.0 {
                            let target = region_target(x, y, w, h);
                            coord.start_recording_with_target(target, cx);
                        } else {
                            cx.notify();
                        }
                    }
                });
            },
        )
    }
}

const BADGE_H: f32 = 22.0;
const BADGE_PX: f32 = 8.0;
const BADGE_TEXT: f32 = 12.0;
const BADGE_GAP: f32 = 8.0;
const BADGE_MARGIN: f32 = 8.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BadgeAnchor {
    /// Just outside the bottom-right corner (Area drags), flipping above or inside near edges.
    BelowTrailing,
    /// Centered over the target (Window hover).
    Center,
}

fn clip_rect([x, y, w, h]: [f32; 4], [sw, sh]: [f32; 2]) -> [f32; 4] {
    let x0 = x.clamp(0.0, sw);
    let y0 = y.clamp(0.0, sh);
    let x1 = (x + w).clamp(0.0, sw);
    let y1 = (y + h).clamp(0.0, sh);
    [x0, y0, x1 - x0, y1 - y0]
}

/// Positions a badge of `size` relative to `rect`, keeping it fully on screen.
fn place_badge(
    [x, y, w, h]: [f32; 4],
    [bw, bh]: [f32; 2],
    [sw, sh]: [f32; 2],
    anchor: BadgeAnchor,
) -> [f32; 2] {
    let (bx, by) = match anchor {
        BadgeAnchor::BelowTrailing => {
            let below = y + h + BADGE_GAP;
            let above = y - BADGE_GAP - bh;
            let by = if below + bh <= sh - BADGE_MARGIN {
                below
            } else if above >= BADGE_MARGIN {
                above
            } else {
                y + h - BADGE_GAP - bh
            };
            (x + w - bw, by)
        }
        BadgeAnchor::Center => (x + (w - bw) / 2.0, y + (h - bh) / 2.0),
    };
    let max_x = (sw - BADGE_MARGIN - bw).max(BADGE_MARGIN);
    let max_y = (sh - BADGE_MARGIN - bh).max(BADGE_MARGIN);
    [
        bx.clamp(BADGE_MARGIN, max_x).round(),
        by.clamp(BADGE_MARGIN, max_y).round(),
    ]
}

pub struct BundleReviewWindow {
    coordinator: Entity<ScrawlCoordinator>,
    focus: FocusHandle,
    cached_sheet_path: Option<PathBuf>,
    cached_sheet_image: Option<Arc<Image>>,
    cached_frames: Vec<Option<Arc<Image>>>,
}

impl BundleReviewWindow {
    fn new(coordinator: Entity<ScrawlCoordinator>, focus: FocusHandle) -> Self {
        Self {
            coordinator,
            focus,
            cached_sheet_path: None,
            cached_sheet_image: None,
            cached_frames: Vec::new(),
        }
    }

    fn ensure_images(&mut self, dir: &std::path::Path, bundle: &ScrawlBundle) {
        let sheet_path = dir.join(&bundle.contact_sheet);
        if self.cached_sheet_path.as_ref() != Some(&sheet_path) {
            self.cached_sheet_path = Some(sheet_path.clone());
            self.cached_sheet_image = std::fs::read(&sheet_path)
                .ok()
                .map(|bytes| Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
            self.cached_frames = bundle
                .keyframes
                .iter()
                .map(|kf| {
                    let bytes = std::fs::read(dir.join(&kf.file)).ok()?;
                    Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
                })
                .collect();
        }
    }
}

impl Render for BundleReviewWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(mtm) = MainThreadMarker::new() {
            for ns_win in NSApplication::sharedApplication(mtm).windows() {
                if ns_win.frame().size.width >= 600.0 {
                    sync_full_size_content_view(&ns_win);
                }
            }
        }
        let stage_w = (f32::from(window.bounds().size.width) - 320.0 - 44.0).max(320.0);
        let coord_read = self.coordinator.read(cx);
        let state = coord_read.review_bundle.clone();
        let status = coord_read.review_status.clone();
        let tab = coord_read.selected_tab;
        let selected_kf = coord_read.selected_keyframe;
        let shortcut_label = coord_read.shortcut_label();
        let recording_key = coord_read.recording_shortcut_key;
        let (shortcut_chip_label, shortcut_hint) = if recording_key {
            ("Press shortcut…", None)
        } else {
            ("Shortcut", Some(shortcut_label.clone()))
        };

        let Some((dir, bundle)) = state else {
            let c_start = self.coordinator.clone();
            let c_demo = self.coordinator.clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .bg(rgb(CANVAS_BG))
                .text_color(rgb(TEXT_COLOR))
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Scrawl"),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            hint_button("empty-record", "Record", Some(shortcut_label), true)
                                .on_click(move |_, _, cx| {
                                    c_start.update(cx, |coord, cx| coord.start_recording(cx));
                                }),
                        )
                        .child(
                            action_button("empty-demo", "Load Demo Bundle", false).on_click(
                                move |_, _, cx| {
                                    if let Ok((dir, bundle, _)) = crate::generate_demo_bundle(None)
                                    {
                                        c_demo.update(cx, |coord, cx| {
                                            coord.set_review_bundle(
                                                (dir, bundle),
                                                Some("Generated demo bundle".into()),
                                            );
                                            cx.notify();
                                        });
                                    }
                                },
                            ),
                        ),
                )
                .into_any_element();
        };

        self.ensure_images(&dir, &bundle);

        let target_label = match (&bundle.target.app, &bundle.target.window_title) {
            (Some(app), Some(win)) if !win.is_empty() && win != app => {
                format!("{app} — “{win}”")
            }
            (Some(app), _) => app.clone(),
            _ => format!("Display {}", bundle.target.display_id),
        };
        let timeline = timeline(&bundle);

        let c_record = self.coordinator.clone();
        let c_shortcut = self.coordinator.clone();
        let c_keydown = self.coordinator.clone();
        let c_copy = self.coordinator.clone();
        let c_send = self.coordinator.clone();
        let c_tab_sheet = self.coordinator.clone();
        let c_tab_frames = self.coordinator.clone();
        let copy_dir = dir.clone();
        let send_dir = dir.clone();
        let focus = self.focus.clone();

        div()
            .id("scrawl-review-root")
            .track_focus(&self.focus)
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                c_keydown.update(cx, |coord, cx| {
                    let key = event.keystroke.key.as_str();
                    let mods = event.keystroke.modifiers;
                    if coord.recording_shortcut_key {
                        if key == "escape" {
                            coord.recording_shortcut_key = false;
                            cx.notify();
                            return;
                        }
                        if let Some(cfg) = shortcut_from_key_and_mods(
                            key,
                            mods.platform,
                            mods.control,
                            mods.alt,
                            mods.shift,
                        ) {
                            let new_label = cfg.label.clone();
                            update_global_shortcut(&coord.telemetry, cfg);
                            coord.recording_shortcut_key = false;
                            coord.review_status = Some(format!("Shortcut: {new_label}"));
                            cx.notify();
                        }
                        return;
                    }
                    if let Some(cfg) = shortcut_from_key_and_mods(
                        key,
                        mods.platform,
                        mods.control,
                        mods.alt,
                        mods.shift,
                    ) && coord.telemetry.matches_shortcut(
                        cfg.key_code as u16,
                        mods.platform,
                        mods.control,
                        mods.alt,
                        mods.shift,
                    ) {
                        coord.telemetry.emit_toggle_shortcut();
                    }
                });
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS_BG))
            .text_color(rgb(TEXT_COLOR))
            .child(
                div()
                    .h(px(52.0))
                    .px_4()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_4()
                    .bg(rgb(SIDEBAR_BG))
                    .border_b_1()
                    .border_color(rgb(LINE_COLOR))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Scrawl"),
                            )
                            .child(
                                // Segmented control: R_outer 8 = R_tab 6 + border 1 + pad 1.
                                div()
                                    .flex_none()
                                    .h(px(28.0))
                                    .p(px(1.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(2.0))
                                    .rounded(px(8.0))
                                    .bg(rgb(CANVAS_BG))
                                    .border_1()
                                    .border_color(rgb(LINE_COLOR))
                                    .child(
                                        tab_chip(
                                            "tab-contact-sheet",
                                            "Contact Sheet",
                                            tab == ReviewTab::ContactSheet,
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                c_tab_sheet.update(cx, |coord, cx| {
                                                    coord.selected_tab = ReviewTab::ContactSheet;
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    )
                                    .child(
                                        tab_chip(
                                            "tab-keyframes",
                                            "Keyframes",
                                            tab == ReviewTab::Keyframes,
                                        )
                                        .on_click(
                                            move |_, _, cx| {
                                                c_tab_frames.update(cx, |coord, cx| {
                                                    coord.selected_tab = ReviewTab::Keyframes;
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.0))
                                    .text_color(rgb(MUTED_COLOR))
                                    .child(format!(
                                        "{} · {} · {} {}",
                                        target_label,
                                        format_clock(bundle.duration_ms),
                                        bundle.keyframes.len(),
                                        if bundle.keyframes.len() == 1 {
                                            "keyframe"
                                        } else {
                                            "keyframes"
                                        }
                                    )),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                hint_button(
                                    "btn-shortcut",
                                    shortcut_chip_label,
                                    shortcut_hint,
                                    recording_key,
                                )
                                .on_click(move |_, window, cx| {
                                    window.focus(&focus);
                                    c_shortcut.update(cx, |coord, cx| {
                                        coord.recording_shortcut_key =
                                            !coord.recording_shortcut_key;
                                        cx.notify();
                                    });
                                }),
                            )
                            .child(action_button("btn-record", "Record", false).on_click(
                                move |_, _, cx| {
                                    c_record.update(cx, |coord, cx| coord.start_recording(cx));
                                },
                            ))
                            .child(
                                action_button("btn-reveal", "Show in Finder", false).on_click(
                                    move |_, _, cx| {
                                        let _ = std::process::Command::new("/usr/bin/open")
                                            .arg(&send_dir)
                                            .spawn();
                                        c_send.update(cx, |coord, cx| {
                                            coord.review_status = Some("Opened in Finder".into());
                                            cx.notify();
                                        });
                                    },
                                ),
                            )
                            .child(
                                action_button(
                                    "btn-copy",
                                    if status.as_deref() == Some("Copied ✓") {
                                        "Copied"
                                    } else {
                                        "Copy"
                                    },
                                    true,
                                )
                                .on_click(move |_, _, cx| {
                                    let res = crate::handoff::copy_bundle_to_clipboard(&copy_dir);
                                    c_copy.update(cx, |coord, cx| {
                                        coord.review_status = Some(match res {
                                            Ok(()) => "Copied ✓".into(),
                                            Err(e) => format!("Copy failed: {e}"),
                                        });
                                        cx.notify();
                                    });
                                }),
                            ),
                    ),
            )
            .child(
                // Main Workspace: Stage on Left + Timeline Sidebar on Right
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .id("review-stage-scroll")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .p_5()
                            .child(match tab {
                                ReviewTab::ContactSheet => self.render_contact_sheet_stage(stage_w),
                                ReviewTab::Keyframes => {
                                    self.render_keyframes_stage(&bundle, selected_kf, stage_w)
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("review-timeline-sidebar")
                            .w(px(320.0))
                            .h_full()
                            .flex_none()
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .bg(rgb(SIDEBAR_BG))
                            .border_l_1()
                            .border_color(rgb(LINE_COLOR))
                            .px_4()
                            .py(px(18.0))
                            .flex()
                            .flex_col()
                            .child(sidebar_heading("Transcript"))
                            .child(
                                div()
                                    .p_3()
                                    .rounded(px(8.0))
                                    .bg(rgb(SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(LINE_COLOR))
                                    .text_size(px(13.0))
                                    .line_height(relative(1.45))
                                    .map(|d| {
                                        let text = bundle.transcript.text.trim();
                                        if text.is_empty() {
                                            d.text_color(rgb(MUTED_COLOR)).child("No speech")
                                        } else {
                                            d.text_color(rgb(TEXT_COLOR)).child(format!("“{text}”"))
                                        }
                                    }),
                            )
                            .child(div().mt(px(20.0)).child(sidebar_heading("Timeline")))
                            .child(
                                div().mx(px(-8.0)).flex().flex_col().gap(px(1.0)).children(
                                    timeline
                                        .into_iter()
                                        .enumerate()
                                        .map(|(i, line)| timeline_row(i, &line)),
                                ),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl BundleReviewWindow {
    fn render_contact_sheet_stage(&self, stage_w: f32) -> AnyElement {
        if let Some(ref image) = self.cached_sheet_image {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .justify_center()
                .child(
                    img(Arc::clone(image))
                        .w(px(stage_w))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(rgb(LINE_COLOR)),
                )
                .into_any_element()
        } else {
            div()
                .p_6()
                .text_color(rgb(MUTED_COLOR))
                .child("Contact sheet image unavailable.")
                .into_any_element()
        }
    }

    fn render_keyframes_stage(
        &self,
        bundle: &ScrawlBundle,
        selected_kf: usize,
        stage_w: f32,
    ) -> AnyElement {
        // Card: border 1 + pad 8 around the frame, so R_card 12 = R_frame 3 + 9.
        let card_img_w = (stage_w - 18.0).max(240.0);
        let cards = bundle
            .keyframes
            .iter()
            .enumerate()
            .map(|(idx, kf)| {
                let is_selected = idx == selected_kf;
                let frame_img = self.cached_frames.get(idx).and_then(Clone::clone);
                let c_select = self.coordinator.clone();
                div()
                    .id(("kf-card", idx))
                    .p(px(8.0))
                    .rounded(px(12.0))
                    .bg(rgb(SURFACE_BG))
                    .border_1()
                    .border_color(if is_selected {
                        rgb(ACCENT_COLOR)
                    } else {
                        rgb(LINE_COLOR)
                    })
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .cursor_pointer()
                    .when(!is_selected, |d| d.hover(|s| s.border_color(rgb(0x3a3a3a))))
                    .on_click(move |_, _, cx| {
                        c_select.update(cx, |coord, cx| {
                            coord.selected_keyframe = idx;
                            cx.notify();
                        });
                    })
                    .when_some(frame_img, |d, image| {
                        d.child(img(image).w(px(card_img_w)).rounded(px(3.0)))
                    })
                    .child(
                        div()
                            .px(px(4.0))
                            .pb(px(2.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .h(px(20.0))
                                    .px(px(7.0))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .rounded(px(10.0))
                                    .bg(rgb(CANVAS_BG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(11.5))
                                    .text_color(rgb(TEXT_COLOR))
                                    .child(format!("#{} · {}", kf.index, format_clock(kf.t))),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(TEXT_SOFT))
                                    .child(kf.event.clone()),
                            ),
                    )
                    .when_some(kf.said.clone(), |d, said| {
                        d.child(
                            div()
                                .px(px(4.0))
                                .pb(px(2.0))
                                .text_size(px(12.0))
                                .line_height(relative(1.4))
                                .text_color(rgb(MUTED_COLOR))
                                .child(format!("“{said}”")),
                        )
                    })
            })
            .collect::<Vec<_>>();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .children(cards)
            .into_any_element()
    }
}

fn sidebar_heading(label: &'static str) -> Div {
    div()
        .mb(px(8.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(MUTED_COLOR))
        .child(label)
}

/// Splits a `sketch::timeline` line ("0:03 · Clicked…", "0:04 ✎ Circle…", "0:05 “said”") into
/// a time column and a body, flagging annotation marks.
fn split_timeline_line(line: &str) -> (&str, &str, bool) {
    let (time, rest) = line.split_once(' ').unwrap_or(("", line));
    if let Some(body) = rest.strip_prefix("✎ ") {
        (time, body, true)
    } else {
        (time, rest.strip_prefix("· ").unwrap_or(rest), false)
    }
}

fn timeline_row(index: usize, line: &str) -> gpui::Stateful<Div> {
    let (time, body, is_mark) = split_timeline_line(line);
    div()
        .id(("timeline-row", index))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .flex()
        .items_start()
        .gap(px(10.0))
        .text_size(px(12.5))
        .line_height(relative(1.4))
        .hover(|s| s.bg(rgb(SURFACE_BG)))
        .child(
            div()
                .w(px(32.0))
                .flex_none()
                .text_color(rgb(MUTED_COLOR))
                .child(time.to_string()),
        )
        .when(is_mark, |d| {
            d.child(div().flex_none().text_color(rgb(INK_RED)).child("✎"))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if is_mark {
                    rgb(TEXT_COLOR)
                } else {
                    rgb(TEXT_SOFT)
                })
                .child(body.to_string()),
        )
}

fn tab_chip(id: &'static str, label: &'static str, selected: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .font_weight(FontWeight::MEDIUM)
        .when(selected, |d| {
            d.bg(rgb(SURFACE_SELECTED)).shadow(vec![BoxShadow {
                color: gpui::hsla(0.0, 0.0, 0.0, 0.25),
                offset: point(px(0.0), px(1.0)),
                blur_radius: px(2.0),
                spread_radius: px(0.0),
            }])
        })
        .text_color(if selected {
            rgb(TEXT_COLOR)
        } else {
            rgb(MUTED_COLOR)
        })
        .when(!selected, |d| {
            d.cursor_pointer().hover(|s| s.text_color(rgb(TEXT_SOFT)))
        })
        .child(label)
}

fn button_base(id: &'static str, primary: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(7.0))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .whitespace_nowrap()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .border_1()
        .border_color(if primary {
            rgb(ACCENT_COLOR)
        } else {
            rgb(LINE_COLOR)
        })
        .bg(if primary {
            rgb(ACCENT_COLOR)
        } else {
            rgb(SURFACE_BG)
        })
        .text_color(rgb(TEXT_COLOR))
        .cursor_pointer()
        .hover(move |s| {
            if primary {
                s.bg(rgb(ACCENT_HOVER)).border_color(rgb(ACCENT_HOVER))
            } else {
                s.bg(rgb(SURFACE_HOVER))
            }
        })
}

fn action_button(id: &'static str, label: &str, primary: bool) -> gpui::Stateful<Div> {
    button_base(id, primary).child(SharedString::from(label.to_string()))
}

/// Button with a trailing muted hint, e.g. a keyboard shortcut.
fn hint_button(
    id: &'static str,
    label: &str,
    hint: Option<String>,
    primary: bool,
) -> gpui::Stateful<Div> {
    button_base(id, primary)
        .child(SharedString::from(label.to_string()))
        .when_some(hint, |d, hint| {
            d.child(
                div()
                    .text_color(if primary {
                        rgba(0xffffffb3)
                    } else {
                        rgb(MUTED_COLOR)
                    })
                    .child(hint),
            )
        })
}

fn open_picker_windows(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    close_picker_window(cx);
    let display = cx.primary_display();
    let display_bounds = display.as_ref().map_or_else(
        || Bounds {
            origin: Point::default(),
            size: size(px(1440.0), px(900.0)),
        },
        |d| d.bounds(),
    );
    let display_id = display.as_ref().map(|d| d.id());

    let picker_coord = coord_entity.clone();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: display_bounds.size,
            })),
            display_id,
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            if let Some(ns_win) = newest_ns_window() {
                ns_win.setStyleMask(
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                );
                if let Some(screen) = ns_win.screen() {
                    ns_win.setFrame_display(screen.frame(), true);
                }
                configure_floating_window(&ns_win, false, 1);
                let num = ns_win.windowNumber().max(0) as u32;
                if num > 0 {
                    picker_coord
                        .read(cx)
                        .telemetry
                        .excluded_window_ids
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(num);
                }
            }
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.new(|cx| {
                cx.observe(&picker_coord, |_, _, cx| cx.notify()).detach();
                TargetPickerWindow {
                    coordinator: picker_coord,
                    focus,
                }
            })
        },
    );

    ensure_hud_pill_open(coord_entity, cx);
}

fn ensure_hud_pill_open(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    for handle in cx.windows() {
        if let Some(pill) = handle.downcast::<HudPillWindow>() {
            let _ = pill.update(cx, |_, _, cx| cx.notify());
            return;
        }
    }
    let display = cx.primary_display();
    let display_bounds = display.as_ref().map_or_else(
        || Bounds {
            origin: Point::default(),
            size: size(px(1440.0), px(900.0)),
        },
        |d| d.bounds(),
    );
    let display_id = display.as_ref().map(|d| d.id());

    // Transparent Dynamic Island host positioned cleanly below the 37pt macOS menu bar
    let pill_size: Size<Pixels> = size(px(PILL_WINDOW_W), px(PILL_WINDOW_H));
    let pill_bounds = Bounds {
        origin: point(
            (display_bounds.size.width - pill_size.width) * 0.5,
            px(50.0),
        ),
        size: pill_size,
    };
    let pill_coord = coord_entity.clone();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(pill_bounds)),
            display_id,
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::Floating,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |_, cx| {
            if let Some(ns_win) = newest_ns_window() {
                configure_floating_window(&ns_win, false, 2);
                let num = ns_win.windowNumber().max(0) as u32;
                if num > 0 {
                    pill_coord
                        .read(cx)
                        .telemetry
                        .excluded_window_ids
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(num);
                }
            }
            cx.new(|cx| {
                cx.observe(&pill_coord, |_, _, cx| cx.notify()).detach();
                HudPillWindow {
                    coordinator: pill_coord,
                }
            })
        },
    );
}

fn close_picker_window(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<TargetPickerWindow>().is_some() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

fn close_capture_windows(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<HudPillWindow>().is_some()
            || handle.downcast::<TargetPickerWindow>().is_some()
        {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

fn close_review_window(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<BundleReviewWindow>().is_some() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }
}

fn open_review_window(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).activate();
    }
    for handle in cx.windows() {
        if let Some(review) = handle.downcast::<BundleReviewWindow>() {
            let _ = review.update(cx, |_, window, cx| {
                window.activate_window();
                cx.notify();
            });
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(1120.0), px(740.0)), cx);
    let coord_clone = coord_entity.clone();
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_resizable: true,
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        },
        move |window, cx| {
            if let Some(ns_win) = newest_ns_window() {
                sync_full_size_content_view(&ns_win);
            }
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.new(|cx| {
                cx.observe(&coord_clone, |_, _, cx| cx.notify()).detach();
                BundleReviewWindow::new(coord_clone, focus)
            })
        },
    );
}

/// Runs the GPUI desktop app, optionally opening an initial `.scrawl` bundle in the Review window
/// or starting recording immediately.
pub fn run_app(
    initial_bundle: Option<(PathBuf, ScrawlBundle)>,
    start_recording_immediately: bool,
    screenshot_out: Option<PathBuf>,
) {
    Application::new().run(move |cx: &mut App| {
        scrawl_log(format!("run_app started pid={}", std::process::id()));
        let (ui_sender, ui_receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(ui_sender);
        install_global_hotkeys(&telemetry);
        start_global_input_tap(Arc::clone(&telemetry));
        let menu_rx = install_status_bar();

        let has_initial = initial_bundle.is_some();
        let coordinator = cx.new(|_| ScrawlCoordinator::new(telemetry, initial_bundle));

        if has_initial {
            open_review_window(&coordinator, cx);
        }
        if start_recording_immediately {
            let coord_clone = coordinator.clone();
            coord_clone.update(cx, |coord, cx| coord.start_recording(cx));
        }

        if let Some(out_path) = screenshot_out {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(650))
                    .await;
                let window_num = cx
                    .update(|_| {
                        let mtm = MainThreadMarker::new()?;
                        let app = NSApplication::sharedApplication(mtm);
                        app.windows()
                            .iter()
                            .filter(|w| {
                                let size = w.frame().size;
                                size.width >= 600.0 && size.height >= 400.0
                            })
                            .max_by_key(|w| {
                                let size = w.frame().size;
                                (size.width as i64) * (size.height as i64)
                            })
                            .map(|w| {
                                w.orderFrontRegardless();
                                w.windowNumber()
                            })
                    })
                    .ok()
                    .flatten();
                let _ = cx
                    .background_executor()
                    .spawn(async move { capture_own_window_screenshot(window_num, &out_path) })
                    .await;
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        }

        let loop_coord = coordinator.clone();
        cx.spawn(async move |cx| {
            let mut last_frame = Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let dt = last_frame.elapsed().as_secs_f32();
                last_frame = Instant::now();
                let mut should_quit = false;
                let _ = cx.update(|cx| {
                    if let Some(ref rx) = menu_rx {
                        while let Ok(action) = rx.try_recv() {
                            match action {
                                MenuAction::ToggleCapture => {
                                    loop_coord.update(cx, |coord, cx| {
                                        if coord.is_recording() {
                                            coord.finish_recording(cx);
                                        } else if coord.is_selecting() {
                                            coord.start_recording_full_screen(cx);
                                        } else {
                                            coord.begin_target_selection(cx);
                                        }
                                    });
                                }
                                MenuAction::OpenLatest => {
                                    if let Ok((dir, bundle)) =
                                        crate::handoff::load_bundle(std::path::Path::new("latest"))
                                    {
                                        loop_coord.update(cx, |coord, cx| {
                                            coord.set_review_bundle((dir, bundle), None);
                                            cx.notify();
                                        });
                                        open_review_window(&loop_coord, cx);
                                    }
                                }
                                MenuAction::RunDemo => {
                                    if let Ok((dir, bundle, _)) = crate::generate_demo_bundle(None)
                                    {
                                        loop_coord.update(cx, |coord, cx| {
                                            coord.set_review_bundle(
                                                (dir, bundle),
                                                Some("Generated demo bundle".into()),
                                            );
                                            cx.notify();
                                        });
                                        open_review_window(&loop_coord, cx);
                                    }
                                }
                                MenuAction::Quit => {
                                    should_quit = true;
                                }
                            }
                        }
                    }

                    while let Ok(ev) = ui_receiver.try_recv() {
                        loop_coord.update(cx, |coord, cx| match ev {
                            LiveOverlayEvent::ToggleCaptureShortcut => {
                                if coord.is_recording() {
                                    coord.finish_recording(cx);
                                } else if coord.is_selecting() {
                                    coord.start_recording_full_screen(cx);
                                } else {
                                    coord.begin_target_selection(cx);
                                }
                            }
                            LiveOverlayEvent::UndoStroke => {
                                coord.undo_stroke(cx);
                            }
                            LiveOverlayEvent::CancelCapture => {
                                coord.cancel_recording(cx);
                            }
                            LiveOverlayEvent::RequestSnapshot => {
                                coord.request_immediate_frame();
                                cx.notify();
                            }
                        });
                    }

                    if loop_coord.read(cx).is_island_visible() {
                        loop_coord.update(cx, |coord, cx| coord.step_island(dt, cx));
                    }
                });
                if should_quit {
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        })
        .detach();
    });
}

fn capture_own_window_screenshot(
    window_num: Option<isize>,
    out_path: &std::path::Path,
) -> anyhow::Result<()> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(num) = window_num
        && num > 0
    {
        if let Some(frame) = capture_cg_window_rgba(num as u32) {
            crate::render::save_png(&frame, out_path)?;
            return Ok(());
        }
        if std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", "-l", &num.to_string()])
            .arg(out_path)
            .status()
            .is_ok_and(|s| s.success())
            && out_path.exists()
        {
            return Ok(());
        }
    }
    let pid = std::process::id() as i32;
    let content = SCShareableContent::get()
        .map_err(|e| anyhow::anyhow!("ScreenCaptureKit unavailable: {e}"))?;
    let window = content
        .windows()
        .into_iter()
        .filter(|w| {
            w.is_on_screen()
                && w.owning_application()
                    .as_ref()
                    .is_some_and(|a| a.process_id() == pid)
        })
        .max_by_key(|w| (w.frame().size.width as u64) * (w.frame().size.height as u64))
        .ok_or_else(|| anyhow::anyhow!("Could not find Scrawl window"))?;
    let frame = window.frame();
    let filter = SCContentFilter::create()
        .with_window(&window)
        .build()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let config = SCStreamConfiguration::new()
        .with_width((frame.size.width as u32 * 2).max(400))
        .with_height((frame.size.height as u32 * 2).max(300))
        .with_shows_cursor(false);
    let image =
        SCScreenshotManager::capture_image(&filter, &config).map_err(|e| anyhow::anyhow!("{e}"))?;
    image
        .save(
            out_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("invalid output path"))?,
            screencapturekit::screenshot_manager::ImageFormat::Png,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

fn capture_cg_window_rgba(window_id: u32) -> Option<crate::model::RgbaFrame> {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCreateImage(
            screen_bounds: CGRect,
            list_option: u32,
            window_id: u32,
            image_option: u32,
        ) -> *mut c_void;
        fn CGImageGetWidth(image: *const c_void) -> usize;
        fn CGImageGetHeight(image: *const c_void) -> usize;
        fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: *const c_void,
            bitmap_info: u32,
        ) -> *mut c_void;
        fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *const c_void);
        fn CFRelease(cf: *const c_void);
    }

    let null_rect = CGRect::new(
        &CGPoint::new(f64::INFINITY, f64::INFINITY),
        &CGSize::new(0.0, 0.0),
    );
    // kCGWindowListOptionIncludingWindow = 1 << 3 (8), kCGWindowImageBoundsIgnoreFraming = 1
    let cg_image = unsafe { CGWindowListCreateImage(null_rect, 8, window_id, 1) };
    if cg_image.is_null() {
        return None;
    }
    let width = unsafe { CGImageGetWidth(cg_image) };
    let height = unsafe { CGImageGetHeight(cg_image) };
    if width == 0 || height == 0 {
        unsafe { CFRelease(cg_image) };
        return None;
    }
    let mut rgba = vec![0_u8; width * height * 4];
    unsafe {
        let cs = CGColorSpaceCreateDeviceRGB();
        // kCGImageAlphaPremultipliedLast = 1
        let ctx =
            CGBitmapContextCreate(rgba.as_mut_ptr().cast(), width, height, 8, width * 4, cs, 1);
        if !ctx.is_null() {
            let draw_rect = CGRect::new(
                &CGPoint::new(0.0, 0.0),
                &CGSize::new(width as f64, height as f64),
            );
            CGContextDrawImage(ctx, draw_rect, cg_image);
            CFRelease(ctx);
        }
        if !cs.is_null() {
            CFRelease(cs);
        }
        CFRelease(cg_image);
    }
    Some(crate::model::RgbaFrame::new(
        0,
        width as u32,
        height as u32,
        rgba,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_geometry_is_concentric() {
        assert_eq!(ISLAND_RADIUS, CHIP_RADIUS + ISLAND_INSET);
        assert_eq!(ISLAND_PAD + 1.0, ISLAND_INSET);
        assert_eq!(CHIP_H + 2.0 * ISLAND_INSET, ISLAND_H);
        assert!(ISLAND_H <= PILL_WINDOW_H);
    }

    #[test]
    fn island_spring_settles_without_large_overshoot() {
        let (mut x, mut v) = (200.0_f32, 0.0_f32);
        let mut peak = x;
        for _ in 0..90 {
            (x, v) = spring_step(x, v, 300.0, 1.0 / 60.0, ISLAND_SPRING);
            peak = peak.max(x);
        }
        assert!((x - 300.0).abs() < 0.5, "settled at {x}");
        assert!(peak < 300.0 + 100.0 * 0.05, "overshoot peak {peak}");
    }

    #[test]
    fn island_spring_is_stable_for_long_frames() {
        let (x, _) = spring_step(200.0, 0.0, 300.0, 0.05, ISLAND_SPRING);
        assert!(x > 200.0 && x < 300.0);
    }

    #[test]
    fn area_badge_sits_below_and_flips_near_edges() {
        let screen = [1440.0, 900.0];
        let badge = [90.0, BADGE_H];
        let [bx, by] = place_badge(
            [100.0, 100.0, 300.0, 200.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!([bx, by], [310.0, 308.0]);

        let [_, by] = place_badge(
            [100.0, 600.0, 300.0, 290.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!(by, 600.0 - BADGE_GAP - BADGE_H);

        let [bx, by] = place_badge(
            [0.0, 0.0, 40.0, 900.0],
            badge,
            screen,
            BadgeAnchor::BelowTrailing,
        );
        assert_eq!(bx, BADGE_MARGIN);
        assert!(by + BADGE_H <= 900.0 - BADGE_MARGIN);
    }

    #[test]
    fn window_badge_centers_and_clamps_on_screen() {
        let screen = [1440.0, 900.0];
        let [bx, by] = place_badge(
            [200.0, 100.0, 600.0, 400.0],
            [200.0, BADGE_H],
            screen,
            BadgeAnchor::Center,
        );
        assert_eq!([bx, by], [400.0, 289.0]);

        let clipped = clip_rect([-300.0, -50.0, 400.0, 200.0], screen);
        assert_eq!(clipped, [0.0, 0.0, 100.0, 150.0]);
    }

    #[test]
    fn timeline_lines_split_into_time_and_body() {
        assert_eq!(
            split_timeline_line("0:04 ✎ Circle around button — frame #2"),
            ("0:04", "Circle around button — frame #2", true)
        );
        assert_eq!(
            split_timeline_line("0:05 · Clicked Save"),
            ("0:05", "Clicked Save", false)
        );
        assert_eq!(
            split_timeline_line("0:01 “hello”"),
            ("0:01", "“hello”", false)
        );
    }

    #[test]
    fn clock_template_is_width_stable() {
        assert_eq!(clock_template("12:34"), "00:00");
        assert_eq!(completed_meta(8_000, 1), "· 0:08 · 1 frame");
    }
}
