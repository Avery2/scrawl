use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Application, Bounds, Context, Div, Entity, FocusHandle, FontWeight, Image,
    ImageFormat, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    PathBuilder, Pixels, Point, Render, SharedString, Size, Window, WindowBackgroundAppearance,
    WindowBounds, WindowKind, WindowOptions, canvas, div, img, point, prelude::*, px, rgb, rgba,
    size,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSStatusWindowLevel, NSWindow,
    NSWindowButton, NSWindowCollectionBehavior,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::capture::{
    LiveOverlayEvent, ScreenCaptureSession, SharedCaptureTelemetry, ax_element_at, discover_target,
    start_global_input_tap,
};
use crate::hex::AudioRecorder;
use crate::model::{LASER_MS, SCALE, ScrawlBundle, Stroke, TargetInfo, format_clock};
use crate::sketch::{FreehandOptions, bounds as points_bounds, freehand_outline, timeline};

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
const INK_RED: u32 = 0xff3b30;
const POSITIVE_COLOR: u32 = 0x69d89f;

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

    add("Record Scrawl  ⌃⌥⌘S", sel!(toggleCapture:));
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

fn configure_floating_window(ns_window: &NSWindow, ignores_mouse: bool) {
    ns_window.setHidesOnDeactivate(false);
    ns_window.setIgnoresMouseEvents(ignores_mouse);
    ns_window.setLevel(NSStatusWindowLevel);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns_window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
}

struct ActiveRecording {
    target: TargetInfo,
    bundle_dir: PathBuf,
    screen_session: ScreenCaptureSession,
    audio_recorder: Option<AudioRecorder>,
}

/// Shared coordinator owned by the GPUI app.
pub struct ScrawlCoordinator {
    telemetry: Arc<SharedCaptureTelemetry>,
    active_recording: Option<ActiveRecording>,
    active_stroke: Option<Stroke>,
    caption: String,
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
            active_stroke: None,
            caption: String::new(),
            review_bundle: initial_bundle,
            review_status: None,
            selected_tab: ReviewTab::ContactSheet,
            selected_keyframe: 0,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.active_recording.is_some()
    }

    pub fn is_drawing(&self) -> bool {
        self.telemetry
            .draw_mode
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn set_review_bundle(&mut self, bundle: (PathBuf, ScrawlBundle), status: Option<String>) {
        self.review_bundle = Some(bundle);
        self.review_status = status;
        self.selected_tab = ReviewTab::ContactSheet;
        self.selected_keyframe = 0;
    }

    pub fn toggle_draw_mode(&mut self, cx: &mut Context<Self>) {
        if !self.is_recording() {
            return;
        }
        let next = !self.is_drawing();
        self.telemetry.finish_active_laser(None);
        self.active_stroke = None;
        self.telemetry
            .draw_mode
            .store(next, std::sync::atomic::Ordering::Release);
        cx.notify();
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

    pub fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.is_recording() {
            return;
        }
        let target = discover_target(None);
        let id = crate::make_bundle_id();
        let bundle_dir = crate::handoff::default_captures_dir().join(format!("{id}.scrawl"));
        let _ = std::fs::create_dir_all(&bundle_dir);
        let video_path = bundle_dir.join("recording.mp4");

        self.telemetry.reset_for_session(target.frame);
        let audio_recorder = AudioRecorder::start().ok();
        let screen_session = ScreenCaptureSession::start(target.clone(), Some(video_path));
        self.active_recording = Some(ActiveRecording {
            target,
            bundle_dir,
            screen_session,
            audio_recorder,
        });
        self.active_stroke = None;
        self.caption.clear();

        let entity = cx.entity().clone();
        cx.defer(move |cx| open_capture_windows(&entity, cx));
        cx.notify();
    }

    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        let Some(rec) = self.active_recording.take() else {
            return;
        };
        let _ = self.telemetry.finish_session();
        let _ = rec.screen_session.stop();
        if let Some(audio) = rec.audio_recorder {
            let _ = audio.stop();
        }
        let _ = std::fs::remove_dir_all(rec.bundle_dir);
        self.active_stroke = None;
        cx.defer(close_capture_windows);
        cx.notify();
    }

    pub fn finish_recording(&mut self, cx: &mut Context<Self>) {
        let Some(rec) = self.active_recording.take() else {
            return;
        };
        if let Some(stroke) = self.active_stroke.take()
            && stroke.pts.len() >= 3
        {
            self.telemetry
                .strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(stroke);
        }
        let (events, strokes) = self.telemetry.finish_session();
        let (frames, video_path, duration_ms) = rec.screen_session.stop();
        let recorded_audio = rec.audio_recorder.and_then(|r| r.stop().ok());

        if let Ok((dir, bundle, _)) = crate::finalize_capture_bundle(
            &rec.bundle_dir,
            rec.target,
            duration_ms,
            frames,
            strokes,
            events,
            recorded_audio,
            video_path,
            None,
        ) {
            let status = match crate::handoff::copy_bundle_to_clipboard(&dir) {
                Ok(()) => "Copied contact sheet & summary to clipboard".into(),
                Err(e) => format!("Saved bundle (clipboard: {e})"),
            };
            self.set_review_bundle((dir, bundle), Some(status));
        }
        let entity = cx.entity().clone();
        cx.defer(move |cx| {
            close_capture_windows(cx);
            open_review_window(&entity, cx);
        });
        cx.notify();
    }
}

pub struct HudPillWindow {
    coordinator: Entity<ScrawlCoordinator>,
}

impl Render for HudPillWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let coord = self.coordinator.read(cx);
        let elapsed_ms = coord
            .active_recording
            .as_ref()
            .map_or(0, |r| r.screen_session.elapsed_ms());
        let drawing = coord.is_drawing();
        let stroke_count = coord
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|s| !s.laser)
            .count();
        let caption = if !coord.caption.is_empty() {
            coord.caption.clone()
        } else if drawing {
            "Drawing ink · Shift-click pin · D to interact".into()
        } else {
            "Hold Shift to laser · D to draw".into()
        };

        let c_draw = self.coordinator.clone();
        let c_undo = self.coordinator.clone();
        let c_cancel = self.coordinator.clone();
        let c_finish = self.coordinator.clone();

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .rounded(px(22.0))
            .bg(rgba(0x161616e8))
            .border_1()
            .border_color(rgb(LINE_COLOR))
            .text_color(rgb(TEXT_COLOR))
            .text_size(px(12.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move();
                    })
                    .child(div().size(px(8.0)).rounded_full().bg(rgb(INK_RED)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format_clock(elapsed_ms)),
                    )
                    .child(
                        div()
                            .pl_2()
                            .max_w(px(210.0))
                            .overflow_x_hidden()
                            .text_color(if drawing {
                                rgb(ACCENT_COLOR)
                            } else {
                                rgb(MUTED_COLOR)
                            })
                            .child(caption),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        pill_chip(
                            "hud-draw",
                            if drawing { "✎ Draw On" } else { "✎ Draw" },
                            drawing,
                        )
                        .on_click(move |_, _, cx| {
                            c_draw.update(cx, |coord, cx| coord.toggle_draw_mode(cx));
                        }),
                    )
                    .child(
                        pill_chip(
                            "hud-undo",
                            &format!(
                                "↶ {}",
                                if stroke_count > 0 {
                                    stroke_count.to_string()
                                } else {
                                    String::new()
                                }
                            ),
                            false,
                        )
                        .on_click(move |_, _, cx| {
                            c_undo.update(cx, |coord, cx| coord.undo_stroke(cx));
                        }),
                    )
                    .child(
                        pill_chip("hud-cancel", "✕", false).on_click(move |_, _, cx| {
                            c_cancel.update(cx, |coord, cx| coord.cancel_recording(cx));
                        }),
                    )
                    .child(
                        div()
                            .id("hud-done")
                            .h(px(28.0))
                            .px_3()
                            .rounded(px(14.0))
                            .flex()
                            .items_center()
                            .bg(rgb(ACCENT_COLOR))
                            .text_color(rgb(TEXT_COLOR))
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .child("Finish")
                            .on_click(move |_, _, cx| {
                                c_finish.update(cx, |coord, cx| coord.finish_recording(cx));
                            }),
                    ),
            )
    }
}

fn pill_chip(id: &'static str, label: &str, active: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(28.0))
        .px_2p5()
        .rounded(px(14.0))
        .flex()
        .items_center()
        .border_1()
        .border_color(if active {
            rgb(ACCENT_COLOR)
        } else {
            rgb(LINE_COLOR)
        })
        .bg(if active {
            rgb(ACCENT_COLOR)
        } else {
            rgb(SURFACE_BG)
        })
        .text_color(if active {
            rgb(TEXT_COLOR)
        } else {
            rgb(TEXT_SOFT)
        })
        .cursor_pointer()
        .hover(|s| s.bg(rgb(SURFACE_HOVER)))
        .child(label.trim().to_string())
}

pub struct OverlayCanvasWindow {
    coordinator: Entity<ScrawlCoordinator>,
    focus: FocusHandle,
    ns_window: Option<Retained<NSWindow>>,
}

impl Render for OverlayCanvasWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let coord = self.coordinator.read(cx);
        let drawing = coord.is_drawing();
        if let Some(ref ns_win) = self.ns_window {
            ns_win.setIgnoresMouseEvents(!drawing);
        }
        let now_ms = coord.telemetry.elapsed_ms();
        let mut strokes = coord
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(ref laser) = *coord
            .telemetry
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            strokes.push(laser.clone());
        }
        if let Some(ref active) = coord.active_stroke {
            strokes.push(active.clone());
        }

        let c_down = self.coordinator.clone();
        let c_move = self.coordinator.clone();
        let c_up = self.coordinator.clone();
        let c_key = self.coordinator.clone();
        let ns_win_down = self.ns_window.clone();
        let ns_win_up = self.ns_window.clone();

        div()
            .id("scrawl-overlay-canvas")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .when(drawing, |d| {
                d.border_2()
                    .border_color(rgba(0x3b5cf666))
                    .on_mouse_down(
                        MouseButton::Left,
                        move |event: &MouseDownEvent, window, cx| {
                            let bounds = window.bounds();
                            let fw = f32::from(bounds.size.width).max(1.0);
                            let fh = f32::from(bounds.size.height).max(1.0);
                            let nx = (f32::from(event.position.x) / fw).clamp(0.0, 1.0);
                            let ny = (f32::from(event.position.y) / fh).clamp(0.0, 1.0);
                            let sx = (nx * SCALE as f32).round() as i32;
                            let sy = (ny * SCALE as f32).round() as i32;
                            let ns_win_down = ns_win_down.clone();
                            c_down.update(cx, |coord, cx| {
                                let t_ms = coord.telemetry.elapsed_ms();
                                let screen_x = f64::from(
                                    f32::from(bounds.origin.x) + f32::from(event.position.x),
                                );
                                let screen_y = f64::from(
                                    f32::from(bounds.origin.y) + f32::from(event.position.y),
                                );
                                if event.modifiers.shift {
                                    if let Some(ref ns_win) = ns_win_down {
                                        ns_win.setIgnoresMouseEvents(true);
                                    }
                                    let on = ax_element_at(screen_x, screen_y).map(|(l, _)| l);
                                    if let Some(ref ns_win) = ns_win_down {
                                        ns_win.setIgnoresMouseEvents(false);
                                    }
                                    let mut guard = coord
                                        .telemetry
                                        .strokes
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    let pin_num =
                                        guard.iter().filter(|s| s.pin.is_some()).count() as u32 + 1;
                                    guard.push(Stroke {
                                        t0: t_ms,
                                        t1: t_ms,
                                        fw: fw.round() as u32,
                                        fh: fh.round() as u32,
                                        on,
                                        pin: Some(pin_num),
                                        laser: false,
                                        pts: vec![sx, sy, 0],
                                    });
                                    coord.request_immediate_frame();
                                } else {
                                    coord.active_stroke = Some(Stroke {
                                        t0: t_ms,
                                        t1: t_ms,
                                        fw: fw.round() as u32,
                                        fh: fh.round() as u32,
                                        on: None,
                                        pin: None,
                                        laser: false,
                                        pts: vec![sx, sy, 0],
                                    });
                                }
                                cx.notify();
                            });
                        },
                    )
                    .on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                        if !event.dragging() {
                            return;
                        }
                        let bounds = window.bounds();
                        let fw = f32::from(bounds.size.width).max(1.0);
                        let fh = f32::from(bounds.size.height).max(1.0);
                        let nx = (f32::from(event.position.x) / fw).clamp(0.0, 1.0);
                        let ny = (f32::from(event.position.y) / fh).clamp(0.0, 1.0);
                        let sx = (nx * SCALE as f32).round() as i32;
                        let sy = (ny * SCALE as f32).round() as i32;
                        c_move.update(cx, |coord, cx| {
                            let t_ms = coord.telemetry.elapsed_ms();
                            if let Some(ref mut stroke) = coord.active_stroke {
                                let dt = t_ms.saturating_sub(stroke.t0) as i32;
                                stroke.pts.extend_from_slice(&[sx, sy, dt]);
                                stroke.t1 = t_ms;
                                cx.notify();
                            }
                        });
                    })
                    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                        let bounds = window.bounds();
                        let ns_win_up = ns_win_up.clone();
                        c_up.update(cx, |coord, cx| {
                            if let Some(mut stroke) = coord.active_stroke.take() {
                                if stroke.pts.len() >= 3 {
                                    let [x0, y0, x1, y1] =
                                        points_bounds(&stroke.normalized_points());
                                    let mid_x = f64::from(
                                        f32::from(bounds.origin.x)
                                            + ((x0 + x1) * 0.5) * f32::from(bounds.size.width),
                                    );
                                    let mid_y = f64::from(
                                        f32::from(bounds.origin.y)
                                            + ((y0 + y1) * 0.5) * f32::from(bounds.size.height),
                                    );
                                    if let Some(ref ns_win) = ns_win_up {
                                        ns_win.setIgnoresMouseEvents(true);
                                    }
                                    stroke.on = ax_element_at(mid_x, mid_y).map(|(l, _)| l);
                                    if let Some(ref ns_win) = ns_win_up {
                                        ns_win.setIgnoresMouseEvents(!coord.is_drawing());
                                    }
                                    coord
                                        .telemetry
                                        .strokes
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .push(stroke);
                                    coord.request_immediate_frame();
                                }
                                cx.notify();
                            }
                        });
                    })
            })
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                let key = event.keystroke.key.as_str();
                if key.eq_ignore_ascii_case("d") && !event.keystroke.modifiers.platform {
                    c_key.update(cx, |coord, cx| coord.toggle_draw_mode(cx));
                } else if key == "escape" {
                    c_key.update(cx, |coord, cx| {
                        if coord.is_drawing() {
                            coord.toggle_draw_mode(cx);
                        } else {
                            coord.cancel_recording(cx);
                        }
                    });
                }
            })
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, _| {
                        paint_live_strokes(bounds, &strokes, now_ms, window);
                    },
                )
                .size_full(),
            )
    }
}

fn paint_live_strokes(
    bounds: Bounds<Pixels>,
    strokes: &[Stroke],
    now_ms: u64,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width).max(1.0);
    let h = f32::from(bounds.size.height).max(1.0);
    let ox = f32::from(bounds.origin.x);
    let oy = f32::from(bounds.origin.y);
    let base_size = (w.min(h) * 0.012).max(5.0);

    for stroke in strokes {
        if let Some(pin) = stroke.pin {
            if let Some([nx, ny]) = stroke.normalized_points().first().copied() {
                let cx = ox + nx * w;
                let cy = oy + ny * h;
                paint_circle(window, cx, cy, 13.0, rgba(0xffffffdd));
                paint_circle(window, cx, cy, 11.0, rgba(0xff3b30ff));
                let _ = pin;
            }
            continue;
        }

        if stroke.laser && stroke.t1 < now_ms.saturating_sub(LASER_MS) {
            continue;
        }
        let min_t = if stroke.laser {
            now_ms.saturating_sub(LASER_MS)
        } else {
            0
        };
        let screen_pts: Vec<[f32; 2]> = stroke
            .timed_points()
            .into_iter()
            .filter(|&(_, _, t)| t >= min_t && t <= now_ms.saturating_add(50))
            .map(|(nx, ny, _)| [ox + nx * w, oy + ny * h])
            .collect();

        if screen_pts.len() < 2 {
            continue;
        }

        let (base_opts, passes) = if stroke.laser {
            (
                FreehandOptions {
                    size: base_size,
                    thinning: 0.5,
                    smoothing: 0.5,
                    streamline: 0.4,
                    simulate_pressure: false,
                    taper_start: true,
                    taper_end: false,
                },
                [
                    (base_size * 0.9, rgba(0xff3b30cc)),
                    (base_size * 0.35, rgba(0xffffffff)),
                ],
            )
        } else {
            (
                FreehandOptions {
                    size: base_size,
                    thinning: 0.55,
                    smoothing: 0.6,
                    streamline: 0.45,
                    simulate_pressure: true,
                    taper_start: false,
                    taper_end: false,
                },
                [
                    (base_size + 4.0, rgba(0xffffffc0)),
                    (base_size, rgba(0xff3b30ff)),
                ],
            )
        };
        for (size, color) in passes {
            let outline = freehand_outline(&screen_pts, &FreehandOptions { size, ..base_opts });
            paint_polygon(window, &outline, color);
        }
    }
}

fn paint_polygon(window: &mut Window, pts: &[[f32; 2]], color: gpui::Rgba) {
    if pts.len() < 3 {
        return;
    }
    let gpui_pts: Vec<Point<Pixels>> = pts.iter().map(|&[x, y]| point(px(x), px(y))).collect();
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&gpui_pts, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_circle(window: &mut Window, cx: f32, cy: f32, radius: f32, color: gpui::Rgba) {
    let mut pts = Vec::with_capacity(24);
    for i in 0..24 {
        let angle = (i as f32 / 24.0) * std::f32::consts::TAU;
        pts.push([cx + radius * angle.cos(), cy + radius * angle.sin()]);
    }
    paint_polygon(window, &pts, color);
}

pub struct BundleReviewWindow {
    coordinator: Entity<ScrawlCoordinator>,
    cached_sheet_path: Option<PathBuf>,
    cached_sheet_image: Option<Arc<Image>>,
    cached_frames: Vec<Option<Arc<Image>>>,
}

impl BundleReviewWindow {
    fn new(coordinator: Entity<ScrawlCoordinator>) -> Self {
        Self {
            coordinator,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.coordinator.read(cx).review_bundle.clone();
        let status = self.coordinator.read(cx).review_status.clone();
        let tab = self.coordinator.read(cx).selected_tab;
        let selected_kf = self.coordinator.read(cx).selected_keyframe;

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
                        .text_size(px(13.0))
                        .text_color(rgb(MUTED_COLOR))
                        .child("Press ⌃⌥⌘S anywhere to record screen, voice, and marks."),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            action_button("empty-record", "Record  ⌃⌥⌘S", true).on_click(
                                move |_, _, cx| {
                                    c_start.update(cx, |coord, cx| coord.start_recording(cx));
                                },
                            ),
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
            (Some(app), Some(win)) if !win.is_empty() => format!("{app} — “{win}”"),
            (Some(app), _) => app.clone(),
            _ => format!("Display {}", bundle.target.display_id),
        };
        let timeline = timeline(&bundle);

        let c_record = self.coordinator.clone();
        let c_copy = self.coordinator.clone();
        let c_send = self.coordinator.clone();
        let c_tab_sheet = self.coordinator.clone();
        let c_tab_frames = self.coordinator.clone();
        let copy_dir = dir.clone();
        let send_dir = dir.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(CANVAS_BG))
            .text_color(rgb(TEXT_COLOR))
            .child(
                // Top Header Bar
                div()
                    .h(px(52.0))
                    .px_5()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(SIDEBAR_BG))
                    .border_b_1()
                    .border_color(rgb(LINE_COLOR))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Scrawl"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(rgb(MUTED_COLOR))
                                    .child(format!(
                                        "{} · {} · {} keyframes",
                                        target_label,
                                        format_clock(bundle.duration_ms),
                                        bundle.keyframes.len()
                                    )),
                            )
                            .child(
                                div()
                                    .ml_2()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .p_0p5()
                                    .rounded(px(7.0))
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
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when_some(status, |d, msg| {
                                d.child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(rgb(POSITIVE_COLOR))
                                        .mr_2()
                                        .child(msg),
                                )
                            })
                            .child(action_button("btn-record", "New  ⌃⌥⌘S", false).on_click(
                                move |_, _, cx| {
                                    c_record.update(cx, |coord, cx| coord.start_recording(cx));
                                },
                            ))
                            .child(action_button("btn-copy", "Copy Bundle", false).on_click(
                                move |_, _, cx| {
                                    let res = crate::handoff::copy_bundle_to_clipboard(&copy_dir);
                                    c_copy.update(cx, |coord, cx| {
                                        coord.review_status = Some(match res {
                                            Ok(()) => "Copied to clipboard".into(),
                                            Err(e) => format!("Copy failed: {e}"),
                                        });
                                        cx.notify();
                                    });
                                },
                            ))
                            .child(
                                action_button("btn-send", "Send to OpenCode", true).on_click(
                                    move |_, _, cx| {
                                        let res = crate::handoff::send_to_opencode(&send_dir, None);
                                        c_send.update(cx, |coord, cx| {
                                            coord.review_status = Some(match res {
                                                Ok(ses) => format!("Sent to {ses}"),
                                                Err(e) => format!("OpenCode handoff: {e}"),
                                            });
                                            cx.notify();
                                        });
                                    },
                                ),
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
                                ReviewTab::ContactSheet => self.render_contact_sheet_stage(),
                                ReviewTab::Keyframes => {
                                    self.render_keyframes_stage(&bundle, selected_kf)
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
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2p5()
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(MUTED_COLOR))
                                    .child("TRANSCRIPT"),
                            )
                            .child(
                                div()
                                    .p_3()
                                    .rounded(px(8.0))
                                    .bg(rgb(SURFACE_BG))
                                    .border_1()
                                    .border_color(rgb(LINE_COLOR))
                                    .text_size(px(13.0))
                                    .text_color(rgb(TEXT_COLOR))
                                    .child(if bundle.transcript.text.trim().is_empty() {
                                        "(No speech recorded)".to_string()
                                    } else {
                                        format!("“{}”", bundle.transcript.text.trim())
                                    }),
                            )
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(MUTED_COLOR))
                                    .child("TIMELINE"),
                            )
                            .children(timeline.into_iter().enumerate().map(|(i, line)| {
                                let is_mark = line.contains('✎');
                                div()
                                    .id(("timeline-row", i))
                                    .p_2p5()
                                    .rounded(px(7.0))
                                    .bg(rgb(SURFACE_BG))
                                    .border_1()
                                    .border_color(if is_mark {
                                        rgba(0xff3b3066)
                                    } else {
                                        rgb(LINE_COLOR)
                                    })
                                    .text_size(px(12.0))
                                    .text_color(if is_mark {
                                        rgb(TEXT_COLOR)
                                    } else {
                                        rgb(TEXT_SOFT)
                                    })
                                    .child(line)
                            }))
                            .child(
                                div()
                                    .mt_2()
                                    .text_size(px(11.0))
                                    .text_color(rgb(MUTED_COLOR))
                                    .child(format!("Bundle: {}", dir.display())),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl BundleReviewWindow {
    fn render_contact_sheet_stage(&self) -> AnyElement {
        if let Some(ref image) = self.cached_sheet_image {
            div()
                .w_full()
                .min_w_0()
                .flex()
                .justify_center()
                .child(
                    img(Arc::clone(image))
                        .w(px(740.0))
                        .max_w_full()
                        .object_fit(gpui::ObjectFit::Contain)
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

    fn render_keyframes_stage(&self, bundle: &ScrawlBundle, selected_kf: usize) -> AnyElement {
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
                    .p_3()
                    .rounded(px(10.0))
                    .bg(rgb(SURFACE_BG))
                    .border_1()
                    .border_color(if is_selected {
                        rgb(ACCENT_COLOR)
                    } else {
                        rgb(LINE_COLOR)
                    })
                    .flex()
                    .flex_col()
                    .gap_2()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        c_select.update(cx, |coord, cx| {
                            coord.selected_keyframe = idx;
                            cx.notify();
                        });
                    })
                    .when_some(frame_img, |d, image| {
                        d.child(img(image).w_full().rounded(px(6.0)))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.5))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(5.0))
                                    .bg(rgb(CANVAS_BG))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT_COLOR))
                                    .child(format!("#{} · {}", kf.index, format_clock(kf.t))),
                            )
                            .child(div().text_color(rgb(TEXT_SOFT)).child(kf.event.clone())),
                    )
                    .when_some(kf.said.clone(), |d, said| {
                        d.child(
                            div()
                                .text_size(px(12.0))
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

fn tab_chip(id: &'static str, label: &'static str, selected: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(26.0))
        .px_2p5()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .text_size(px(12.0))
        .bg(if selected {
            rgb(SURFACE_SELECTED)
        } else {
            rgb(CANVAS_BG)
        })
        .text_color(if selected {
            rgb(TEXT_COLOR)
        } else {
            rgb(MUTED_COLOR)
        })
        .cursor_pointer()
        .child(label)
}

fn action_button(id: &'static str, label: &'static str, primary: bool) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .px_3()
        .rounded(px(6.0))
        .flex()
        .items_center()
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
        .hover(|s| {
            s.bg(if primary {
                rgb(0x4b6bff)
            } else {
                rgb(SURFACE_HOVER)
            })
        })
        .child(SharedString::from(label))
}

fn open_capture_windows(coord_entity: &Entity<ScrawlCoordinator>, cx: &mut App) {
    close_capture_windows(cx);
    let display = cx.primary_display();
    let display_bounds = display.as_ref().map_or_else(
        || Bounds {
            origin: Point::default(),
            size: size(px(1440.0), px(900.0)),
        },
        |d| d.bounds(),
    );
    let display_id = display.as_ref().map(|d| d.id());

    // 1. Full-screen transparent overlay window (ignoresMouseEvents = true initially)
    let overlay_coord = coord_entity.clone();
    if let Ok(_) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: display_bounds.size,
            })),
            display_id,
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            let ns_window = newest_ns_window();
            if let Some(ref w) = ns_window {
                configure_floating_window(w, true);
            }
            let focus = cx.focus_handle();
            window.focus(&focus);
            cx.new(|cx| {
                cx.observe(&overlay_coord, |_, _, cx| cx.notify()).detach();
                OverlayCanvasWindow {
                    coordinator: overlay_coord,
                    focus,
                    ns_window,
                }
            })
        },
    ) {}

    // 2. Compact floating HUD pill at top-center
    let pill_size: Size<Pixels> = size(px(520.0), px(44.0));
    let pill_bounds = Bounds {
        origin: point(
            (display_bounds.size.width - pill_size.width) * 0.5,
            px(18.0),
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
            window_background: WindowBackgroundAppearance::Blurred,
            ..Default::default()
        },
        move |_, cx| {
            if let Some(ns_win) = newest_ns_window() {
                configure_floating_window(&ns_win, false);
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

fn close_capture_windows(cx: &mut App) {
    for handle in cx.windows() {
        if handle.downcast::<HudPillWindow>().is_some()
            || handle.downcast::<OverlayCanvasWindow>().is_some()
        {
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
        move |_, cx| {
            if let Some(ns_win) = newest_ns_window() {
                for kind in [
                    NSWindowButton::CloseButton,
                    NSWindowButton::MiniaturizeButton,
                    NSWindowButton::ZoomButton,
                ] {
                    if let Some(button) = ns_win.standardWindowButton(kind) {
                        button.setHidden(true);
                    }
                }
            }
            cx.new(|cx| {
                cx.observe(&coord_clone, |_, _, cx| cx.notify()).detach();
                BundleReviewWindow::new(coord_clone)
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
        let (ui_sender, ui_receiver) = flume::unbounded();
        let telemetry = SharedCaptureTelemetry::new(ui_sender);
        start_global_input_tap(Arc::clone(&telemetry));
        let menu_rx = install_status_bar();

        let has_initial = initial_bundle.is_some();
        let coordinator = cx.new(|_| ScrawlCoordinator::new(telemetry, initial_bundle));

        if has_initial || !start_recording_immediately {
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
            let mut last_tick = Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                let mut should_quit = false;
                let _ = cx.update(|cx| {
                    if let Some(ref rx) = menu_rx {
                        while let Ok(action) = rx.try_recv() {
                            match action {
                                MenuAction::ToggleCapture => {
                                    loop_coord.update(cx, |coord, cx| {
                                        if coord.is_recording() {
                                            coord.finish_recording(cx);
                                        } else {
                                            coord.start_recording(cx);
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
                                } else {
                                    coord.start_recording(cx);
                                }
                            }
                            LiveOverlayEvent::ToggleDrawMode => {
                                coord.toggle_draw_mode(cx);
                            }
                            LiveOverlayEvent::UndoStroke => {
                                coord.undo_stroke(cx);
                            }
                            LiveOverlayEvent::CancelCapture => {
                                if coord.is_drawing() {
                                    coord.toggle_draw_mode(cx);
                                } else {
                                    coord.cancel_recording(cx);
                                }
                            }
                            LiveOverlayEvent::ShiftHeld | LiveOverlayEvent::PointerMoved => {
                                cx.notify();
                            }
                            LiveOverlayEvent::RequestSnapshot => {
                                coord.request_immediate_frame();
                            }
                            LiveOverlayEvent::ActionCaption(text) => {
                                coord.caption = text;
                                coord.request_immediate_frame();
                                cx.notify();
                            }
                        });
                    }

                    if last_tick.elapsed() >= Duration::from_millis(200) {
                        last_tick = Instant::now();
                        if loop_coord.read(cx).is_recording() {
                            loop_coord.update(cx, |_, cx| cx.notify());
                        }
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
