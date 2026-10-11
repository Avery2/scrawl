use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::model::{
    CaptureMode, InteractionEvent, RgbaFrame, SCALE, Stroke, TargetFrame, TargetInfo,
};

const MAX_RETAINED_FRAMES: usize = 48;
const FRAME_SAMPLE_INTERVAL: Duration = Duration::from_millis(220);

type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type AxUiElementRef = *const c_void;
type EventRef = *mut c_void;

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

const EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const EVENT_LEFT_MOUSE_UP: u32 = 2;
const EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const EVENT_RIGHT_MOUSE_UP: u32 = 4;
const EVENT_MOUSE_MOVED: u32 = 5;
const EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_FLAGS_CHANGED: u32 = 12;
const EVENT_SCROLL_WHEEL: u32 = 22;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = u32::MAX - 1;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = u32::MAX;

const KEYBOARD_EVENT_KEYCODE: u32 = 9;
const SCROLL_WHEEL_DELTA_AXIS_1: u32 = 11;
const SCROLL_WHEEL_DELTA_AXIS_2: u32 = 12;

const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

const KEY_S: u16 = 1;
const KEY_D: u16 = 2;
const KEY_Z: u16 = 6;
const KEY_ESCAPE: u16 = 53;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}

type EventTapCallback = unsafe extern "C" fn(
    proxy: *mut c_void,
    event_type: u32,
    event: EventRef,
    user_info: *mut c_void,
) -> EventRef;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AxUiElementRef;
    fn AXUIElementCreateSystemWide() -> AxUiElementRef;
    fn AXUIElementCopyElementAtPosition(
        application: AxUiElementRef,
        x: f32,
        y: f32,
        element: *mut AxUiElementRef,
    ) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: AxUiElementRef,
        attribute: CfStringRef,
        value: *mut CfTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: AxUiElementRef, timeout_in_seconds: f32) -> i32;

    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: EventTapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventCreate(source: *const c_void) -> EventRef;
    fn CGEventSourceFlagsState(state_id: u32) -> u64;
    fn CGEventSourceKeyState(state_id: u32, key: u16) -> bool;
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> bool;
    fn CGEventGetFlags(event: EventRef) -> u64;
    fn CGEventGetLocation(event: EventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(event: EventRef, field: u32) -> i64;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFGetTypeID(value: CfTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetLength(value: CfStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> bool;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        string: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopRun();
    fn CFRelease(value: CfTypeRef);
    static kCFRunLoopCommonModes: *const c_void;
}

struct CfObject(CfTypeRef);

impl Drop for CfObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Live UI events emitted by the global input tap to drive the GPUI overlay & HUD pill.
#[derive(Clone, Debug)]
pub enum LiveOverlayEvent {
    ToggleCaptureShortcut,
    ToggleDrawMode,
    UndoStroke,
    CancelCapture,
    ShiftHeld,
    RequestSnapshot,
    ActionCaption(String),
}

/// Inspects the accessibility element at screen coordinates `(x, y)` in global points.
/// Queries the topmost normal (`window_layer == 0`) window's application directly so
/// Scrawl's own transparent overlay window never intercepts hit-testing.
pub fn ax_element_at(x: f64, y: f64) -> Option<(String, Option<String>)> {
    let our_pid = std::process::id() as i32;
    let hit_window = topmost_normal_window_at(x, y);
    let root = if let Some((pid, _, _)) = hit_window.as_ref()
        && *pid != our_pid
    {
        unsafe { AXUIElementCreateApplication(*pid) }
    } else {
        unsafe { AXUIElementCreateSystemWide() }
    };
    if root.is_null() {
        return hit_window.and_then(|(_, title, app)| title.or(app).map(|l| (l, None)));
    }
    let root = CfObject(root);
    let _ = unsafe { AXUIElementSetMessagingTimeout(root.0, 0.08) };

    let mut element: AxUiElementRef = ptr::null();
    let status =
        unsafe { AXUIElementCopyElementAtPosition(root.0, x as f32, y as f32, &mut element) };
    if status != 0 || element.is_null() {
        return hit_window.and_then(|(_, title, app)| title.or(app).map(|l| (l, None)));
    }
    let mut current = CfObject(element);
    let _ = unsafe { AXUIElementSetMessagingTimeout(current.0, 0.08) };

    let role = copy_ax_string(&current, c"AXRole");
    let subrole = copy_ax_string(&current, c"AXSubrole");
    if role.as_deref() == Some("AXSecureTextField")
        || subrole.as_deref() == Some("AXSecureTextField")
    {
        return Some(("Secure Text Field".into(), role));
    }

    for _ in 0..4 {
        let cur_role = copy_ax_string(&current, c"AXRole");
        let cur_subrole = copy_ax_string(&current, c"AXSubrole");
        let is_secure = cur_role.as_deref() == Some("AXSecureTextField")
            || cur_subrole.as_deref() == Some("AXSecureTextField");

        for attr in [c"AXTitle", c"AXDescription", c"AXIdentifier"] {
            if let Some(val) = copy_ax_string(&current, attr) {
                let cleaned = clean_label(&val);
                if !cleaned.is_empty() && !cleaned.eq_ignore_ascii_case("scrawl") {
                    return Some((cleaned, role));
                }
            }
        }
        if !is_secure
            && cur_role.as_deref() != Some("AXTextArea")
            && cur_role.as_deref() != Some("AXTextField")
            && let Some(val) = copy_ax_string(&current, c"AXValue")
        {
            let cleaned = clean_label(&val);
            if !cleaned.is_empty() && cleaned.len() <= 48 && !cleaned.eq_ignore_ascii_case("scrawl")
            {
                return Some((cleaned, role));
            }
        }

        let Some(parent) = copy_ax_object(&current, c"AXParent") else {
            break;
        };
        let _ = unsafe { AXUIElementSetMessagingTimeout(parent.0, 0.08) };
        current = parent;
    }

    if let Some((_, title, app)) = hit_window
        && let Some(fallback) = title.or(app)
    {
        return Some((fallback, role));
    }

    role.clone()
        .map(|r| (r.trim_start_matches("AX").to_string(), Some(r)))
}

fn topmost_normal_window_at(x: f64, y: f64) -> Option<(i32, Option<String>, Option<String>)> {
    let content = SCShareableContent::get().ok()?;
    for win in content.windows() {
        if !win.is_on_screen() || win.window_layer() != 0 {
            continue;
        }
        let frame = win.frame();
        if frame.size.width < 48.0 || frame.size.height < 48.0 {
            continue;
        }
        if x >= frame.origin.x
            && x <= frame.origin.x + frame.size.width
            && y >= frame.origin.y
            && y <= frame.origin.y + frame.size.height
        {
            let owner = win.owning_application()?;
            let pid = owner.process_id();
            let title = win
                .title()
                .map(|t| clean_label(&t))
                .filter(|t| !t.is_empty());
            let app = Some(clean_label(&owner.application_name())).filter(|a| !a.is_empty());
            return Some((pid, title, app));
        }
    }
    None
}

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

fn copy_ax_object(element: &CfObject, attr: &CStr) -> Option<CfObject> {
    let cf_attr =
        unsafe { CFStringCreateWithCString(ptr::null(), attr.as_ptr(), CF_STRING_ENCODING_UTF8) };
    if cf_attr.is_null() {
        return None;
    }
    let cf_attr = CfObject(cf_attr);
    let mut value: CfTypeRef = ptr::null();
    let status = unsafe { AXUIElementCopyAttributeValue(element.0, cf_attr.0, &mut value) };
    if status == 0 && !value.is_null() {
        Some(CfObject(value))
    } else {
        None
    }
}

fn copy_ax_string(element: &CfObject, attr: &CStr) -> Option<String> {
    let obj = copy_ax_object(element, attr)?;
    if unsafe { CFGetTypeID(obj.0) } != unsafe { CFStringGetTypeID() } {
        return None;
    }
    let length = unsafe { CFStringGetLength(obj.0) };
    if length <= 0 || length > 512 {
        return None;
    }
    let capacity =
        unsafe { CFStringGetMaximumSizeForEncoding(length, CF_STRING_ENCODING_UTF8) } + 1;
    let mut bytes = vec![0_u8; capacity as usize];
    let ok = unsafe {
        CFStringGetCString(
            obj.0,
            bytes.as_mut_ptr().cast(),
            capacity,
            CF_STRING_ENCODING_UTF8,
        )
    };
    if !ok {
        return None;
    }
    let nul = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    bytes.truncate(nul);
    String::from_utf8(bytes).ok()
}

/// Discovers the current capture target (either a specific window/app or the main display).
pub fn discover_target(window_query: Option<&str>) -> TargetInfo {
    let frontmost = frontmost_app_info();
    if let Ok(content) = SCShareableContent::get() {
        let our_pid = std::process::id() as i32;
        if let Some(query) = window_query {
            let q = query.to_lowercase();
            if let Some(win) = content.windows().into_iter().find(|w| {
                w.is_on_screen()
                    && w.window_layer() == 0
                    && w.owning_application()
                        .as_ref()
                        .is_none_or(|a| a.process_id() != our_pid)
                    && (w
                        .title()
                        .as_deref()
                        .is_some_and(|t| t.to_lowercase().contains(&q))
                        || w.owning_application()
                            .as_ref()
                            .is_some_and(|a| a.application_name().to_lowercase().contains(&q)))
            }) {
                let frame = win.frame();
                let owner = win.owning_application();
                let display_id = content
                    .displays()
                    .first()
                    .map(|d| d.display_id())
                    .unwrap_or_else(|| unsafe { CGMainDisplayID() });
                return TargetInfo {
                    mode: CaptureMode::Window,
                    app: owner.as_ref().map(|a| a.application_name()),
                    bundle_id: owner.as_ref().map(|a| a.bundle_identifier()),
                    window_title: win.title(),
                    window_id: Some(win.window_id()),
                    display_id,
                    frame: TargetFrame {
                        x: frame.origin.x,
                        y: frame.origin.y,
                        width: frame.size.width.max(1.0),
                        height: frame.size.height.max(1.0),
                        scale: 2.0,
                    },
                };
            }
        }

        if let Some(display) = content.displays().into_iter().next() {
            let frame = display.frame();
            let window_title = frontmost.as_ref().and_then(|(pid, _, _)| {
                content
                    .windows()
                    .into_iter()
                    .find(|w| {
                        w.is_on_screen()
                            && w.window_layer() == 0
                            && w.owning_application()
                                .as_ref()
                                .is_some_and(|a| a.process_id() == *pid)
                    })
                    .and_then(|w| w.title())
            });
            return TargetInfo {
                mode: CaptureMode::Display,
                app: frontmost.as_ref().map(|(_, name, _)| name.clone()),
                bundle_id: frontmost.and_then(|(_, _, bid)| bid),
                window_title,
                window_id: None,
                display_id: display.display_id(),
                frame: TargetFrame {
                    x: frame.origin.x,
                    y: frame.origin.y,
                    width: frame.size.width.max(1.0),
                    height: frame.size.height.max(1.0),
                    scale: 2.0,
                },
            };
        }
    }

    let display_id = unsafe { CGMainDisplayID() };
    let width = (unsafe { CGDisplayPixelsWide(display_id) } as f64).max(1440.0);
    let height = (unsafe { CGDisplayPixelsHigh(display_id) } as f64).max(900.0);
    TargetInfo {
        mode: CaptureMode::Display,
        app: frontmost.as_ref().map(|(_, name, _)| name.clone()),
        bundle_id: frontmost.and_then(|(_, _, bid)| bid),
        window_title: None,
        window_id: None,
        display_id,
        frame: TargetFrame {
            x: 0.0,
            y: 0.0,
            width,
            height,
            scale: 2.0,
        },
    }
}

fn frontmost_app_info() -> Option<(i32, String, Option<String>)> {
    use objc2_app_kit::NSWorkspace;
    let workspace = NSWorkspace::sharedWorkspace();
    let app = workspace.frontmostApplication()?;
    let pid = app.processIdentifier();
    if pid == std::process::id() as i32 {
        return None;
    }
    let name = app.localizedName()?.to_string();
    let bundle_id = app.bundleIdentifier().map(|b| b.to_string());
    Some((pid, name, bundle_id))
}

/// Active screen + optional MP4 recorder using `ScreenCaptureKit`.
pub struct ScreenCaptureSession {
    started_at: Instant,
    stopping: Arc<AtomicBool>,
    urgent_snapshot: Arc<AtomicBool>,
    frames: Arc<Mutex<VecDeque<RgbaFrame>>>,
    sampler_thread: Option<JoinHandle<()>>,
    stream: Option<SCStream>,
    recording_output: Option<SCRecordingOutput>,
    video_path: Option<PathBuf>,
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
        let frames = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_RETAINED_FRAMES)));

        let initial_excluded = excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (stream, recording_output, video_path, mut filter_and_config) =
            setup_sck_capture(&target, video_output_path, &initial_excluded);

        let thread_stopping = Arc::clone(&stopping);
        let thread_urgent = Arc::clone(&urgent_snapshot);
        let thread_frames = Arc::clone(&frames);
        let sampler_target = target;
        let sampler_thread = thread::Builder::new()
            .name("scrawl-frame-sampler".into())
            .spawn(move || {
                let mut last_sample = Instant::now()
                    .checked_sub(FRAME_SAMPLE_INTERVAL)
                    .unwrap_or_else(Instant::now);
                let mut phase = 0_u32;
                let mut last_excluded_len = initial_excluded.len();
                while !thread_stopping.load(Ordering::Acquire) {
                    let cur_excluded = excluded_window_ids
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if cur_excluded.len() != last_excluded_len {
                        last_excluded_len = cur_excluded.len();
                        if let Some((new_filter, new_cfg)) =
                            build_sck_filter_and_config(&sampler_target, &cur_excluded)
                        {
                            filter_and_config = Some((new_filter, new_cfg));
                        }
                    }
                    let urgent = thread_urgent.swap(false, Ordering::AcqRel);
                    if urgent || last_sample.elapsed() >= FRAME_SAMPLE_INTERVAL {
                        last_sample = Instant::now();
                        let t_ms = started_at.elapsed().as_millis() as u64;
                        if urgent {
                            phase = phase.saturating_add(1);
                        }
                        let frame = if let Some((ref filter, ref config)) = filter_and_config {
                            capture_sck_frame(filter, config, t_ms).unwrap_or_else(|| {
                                crate::render::render_synthetic_demo_frame(1280, 800, t_ms, phase)
                            })
                        } else {
                            crate::render::render_synthetic_demo_frame(1280, 800, t_ms, phase)
                        };
                        let mut guard = thread_frames
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if guard.len() >= MAX_RETAINED_FRAMES {
                            guard.pop_front();
                        }
                        guard.push_back(frame);
                    }
                    thread::sleep(Duration::from_millis(30));
                }
            })
            .ok();

        Self {
            started_at,
            stopping,
            urgent_snapshot,
            frames,
            sampler_thread,
            stream,
            recording_output,
            video_path,
        }
    }

    pub fn request_immediate_frame(&self) {
        self.urgent_snapshot.store(true, Ordering::Release);
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }

    pub fn stop(mut self) -> (Vec<RgbaFrame>, Option<PathBuf>, u64) {
        let duration_ms = self.elapsed_ms();
        self.stopping.store(true, Ordering::Release);
        if let Some(handle) = self.sampler_thread.take() {
            let _ = handle.join();
        }
        if let Some(stream) = self.stream.take() {
            let _ = stream.stop_capture();
            if let Some(ref output) = self.recording_output {
                let _ = stream.remove_recording_output(output);
            }
        }
        let mut collected: Vec<RgbaFrame> = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        if collected.is_empty() {
            collected.push(crate::render::render_synthetic_demo_frame(1280, 800, 0, 0));
        }
        let valid_video = self
            .video_path
            .filter(|path| path.exists() && path.metadata().is_ok_and(|m| m.len() > 0));
        (collected, valid_video, duration_ms.max(100))
    }
}

fn build_sck_filter_and_config(
    target: &TargetInfo,
    excluded_window_ids: &[u32],
) -> Option<(SCContentFilter, SCStreamConfiguration)> {
    let content = SCShareableContent::get().ok()?;
    let our_pid = std::process::id() as i32;

    let filter = if let Some(window_id) = target.window_id
        && let Some(window) = content
            .windows()
            .into_iter()
            .find(|w| w.window_id() == window_id)
    {
        SCContentFilter::create().with_window(&window).build().ok()
    } else if let Some(display) = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == target.display_id)
        .or_else(|| content.displays().into_iter().next())
    {
        // Exclude Scrawl's floating HUD pill and Metal overlay windows by explicit window ID
        // (and any non-layer-0 Scrawl panels), while keeping Scrawl's normal Review window visible.
        let excluded_overlays: Vec<SCWindow> = content
            .windows()
            .into_iter()
            .filter(|w| {
                excluded_window_ids.contains(&w.window_id())
                    || (w
                        .owning_application()
                        .as_ref()
                        .is_some_and(|a| a.process_id() == our_pid)
                        && w.window_layer() != 0)
            })
            .collect();
        let excluded_refs: Vec<&SCWindow> = excluded_overlays.iter().collect();
        SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&excluded_refs)
            .build()
            .ok()
    } else {
        None
    }?;

    let aspect = (target.frame.height / target.frame.width.max(1.0)).clamp(0.3, 2.5);
    let capture_width = 1280_u32;
    let capture_height = ((capture_width as f64 * aspect).round() as u32).max(360) & !1;

    let config = SCStreamConfiguration::new()
        .with_width(capture_width)
        .with_height(capture_height)
        .with_shows_cursor(true);
    Some((filter, config))
}

fn setup_sck_capture(
    target: &TargetInfo,
    video_output_path: Option<PathBuf>,
    excluded_window_ids: &[u32],
) -> (
    Option<SCStream>,
    Option<SCRecordingOutput>,
    Option<PathBuf>,
    Option<(SCContentFilter, SCStreamConfiguration)>,
) {
    let Some((filter, config)) = build_sck_filter_and_config(target, excluded_window_ids) else {
        return (None, None, None, None);
    };

    if let Some(video_path) = video_output_path {
        if let Ok(rec_cfg_builder) = SCRecordingOutputConfiguration::new()
            && let Ok(rec_cfg) = rec_cfg_builder.with_output_url(&video_path).map(|c| {
                c.with_video_codec(SCRecordingOutputCodec::H264)
                    .with_output_file_type(SCRecordingOutputFileType::MP4)
            })
            && let Some(rec_output) = SCRecordingOutput::new(&rec_cfg)
            && let Ok(stream) = SCStream::new(&filter, &config)
            && stream.add_recording_output(&rec_output).is_ok()
            && stream.start_capture().is_ok()
        {
            return (
                Some(stream),
                Some(rec_output),
                Some(video_path),
                Some((filter, config)),
            );
        }
    }

    (None, None, None, Some((filter, config)))
}

fn capture_sck_frame(
    filter: &SCContentFilter,
    config: &SCStreamConfiguration,
    t_ms: u64,
) -> Option<RgbaFrame> {
    let image = SCScreenshotManager::capture_image(filter, config).ok()?;
    let width = u32::try_from(image.width()).ok()?;
    let height = u32::try_from(image.height()).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let rgba = image.rgba_data().ok()?;
    if rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    Some(RgbaFrame::new(t_ms, width, height, rgba))
}

/// Shared state collected by the global input monitor during an active capture session.
pub struct SharedCaptureTelemetry {
    pub active: AtomicBool,
    pub draw_mode: AtomicBool,
    pub shift_held: AtomicBool,
    pub started_at: Mutex<Instant>,
    pub target_frame: Mutex<TargetFrame>,
    pub pointer_screen: Mutex<[f32; 2]>,
    pub events: Mutex<Vec<InteractionEvent>>,
    pub strokes: Mutex<Vec<Stroke>>,
    pub active_laser: Mutex<Option<Stroke>>,
    pub live_ink: Mutex<Option<Stroke>>,
    pub excluded_window_ids: Arc<Mutex<Vec<u32>>>,
    pub ui_sender: flume::Sender<LiveOverlayEvent>,
    tap_port: AtomicU64,
    drag_start: Mutex<Option<(u64, f32, f32, Option<String>)>>,
}

impl SharedCaptureTelemetry {
    pub fn new(ui_sender: flume::Sender<LiveOverlayEvent>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(false),
            draw_mode: AtomicBool::new(false),
            shift_held: AtomicBool::new(false),
            started_at: Mutex::new(Instant::now()),
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
            ui_sender,
            tap_port: AtomicU64::new(0),
            drag_start: Mutex::new(None),
        })
    }

    pub fn reset_for_session(&self, frame: TargetFrame) {
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
        self.draw_mode.store(false, Ordering::Release);
        self.shift_held.store(false, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
            .as_millis() as u64
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

    pub fn finish_active_laser(&self, ax_point: Option<(f64, f64)>) {
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
                let label = ax_point.and_then(|(x, y)| ax_element_at(x, y).map(|(l, _)| l));
                laser.on.clone_from(&label);
                self.events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Point {
                        t: laser.t1,
                        x: lx,
                        y: ly,
                        label,
                    });
            }
            self.strokes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(laser);
        }
    }

    pub fn record_laser_point(&self, screen_x: f64, screen_y: f64) {
        *self
            .pointer_screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            [screen_x as f32, screen_y as f32];
        let t_ms = self.elapsed_ms();
        let (nx, ny, fw, fh) = self.normalize_point(screen_x, screen_y);
        let mut guard = self
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stroke = guard.get_or_insert_with(|| Stroke {
            t0: t_ms,
            t1: t_ms,
            fw,
            fh,
            on: None,
            pin: None,
            laser: true,
            pts: Vec::new(),
        });
        let sx = (nx * SCALE as f32).round() as i32;
        let sy = (ny * SCALE as f32).round() as i32;
        let dt = t_ms.saturating_sub(stroke.t0) as i32;
        let should_push = stroke.pts.chunks_exact(3).last().is_none_or(|prev| {
            let dx = (sx - prev[0]) as f32 / SCALE as f32 * fw as f32;
            let dy = (sy - prev[1]) as f32 / SCALE as f32 * fh as f32;
            dx.hypot(dy) >= 1.2 || (dt - prev[2]) >= 16
        });
        if should_push && stroke.pts.len() / 3 < 1_800 {
            stroke.pts.extend_from_slice(&[sx, sy, dt]);
            stroke.t1 = t_ms;
        }
    }

    pub fn finish_session(&self) -> (Vec<InteractionEvent>, Vec<Stroke>) {
        self.active.store(false, Ordering::Release);
        self.draw_mode.store(false, Ordering::Release);
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

pub fn start_global_input_tap(telemetry: Arc<SharedCaptureTelemetry>) {
    let hid_telemetry = Arc::clone(&telemetry);
    let _ = thread::Builder::new()
        .name("scrawl-hid-poller".into())
        .spawn(move || run_global_hid_poller(hid_telemetry));

    let _ = thread::Builder::new()
        .name("scrawl-event-tap".into())
        .spawn(move || {
            let mask = (1_u64 << EVENT_LEFT_MOUSE_DOWN)
                | (1_u64 << EVENT_LEFT_MOUSE_UP)
                | (1_u64 << EVENT_RIGHT_MOUSE_DOWN)
                | (1_u64 << EVENT_RIGHT_MOUSE_UP)
                | (1_u64 << EVENT_MOUSE_MOVED)
                | (1_u64 << EVENT_LEFT_MOUSE_DRAGGED)
                | (1_u64 << EVENT_RIGHT_MOUSE_DRAGGED)
                | (1_u64 << EVENT_KEY_DOWN)
                | (1_u64 << EVENT_FLAGS_CHANGED)
                | (1_u64 << EVENT_SCROLL_WHEEL);

            let user_info = Arc::into_raw(Arc::clone(&telemetry))
                .cast_mut()
                .cast::<c_void>();
            // tap = 2 (kCGAnnotatedSessionEventTap), options = 1 (kCGEventTapOptionListenOnly)
            let tap = unsafe { CGEventTapCreate(2, 0, 1, mask, tap_callback, user_info) };
            if tap.is_null() {
                let _ = unsafe { Arc::from_raw(user_info.cast::<SharedCaptureTelemetry>()) };
                return;
            }
            telemetry.tap_port.store(tap as u64, Ordering::Release);
            let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
            if !source.is_null() {
                let run_loop = unsafe { CFRunLoopGetCurrent() };
                unsafe {
                    CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                    CGEventTapEnable(tap, true);
                    CFRunLoopRun();
                    CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
                    CFRelease(source);
                }
            }
            unsafe { CFRelease(tap) };
            let _ = unsafe { Arc::from_raw(user_info.cast::<SharedCaptureTelemetry>()) };
        });
}

unsafe extern "C" fn tap_callback(
    _proxy: *mut c_void,
    event_type: u32,
    event: EventRef,
    user_info: *mut c_void,
) -> EventRef {
    if user_info.is_null() || event.is_null() {
        return event;
    }
    let telemetry = unsafe { &*user_info.cast::<SharedCaptureTelemetry>() };
    if event_type == EVENT_TAP_DISABLED_BY_TIMEOUT || event_type == EVENT_TAP_DISABLED_BY_USER_INPUT
    {
        let tap = telemetry.tap_port.load(Ordering::Acquire) as *mut c_void;
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, true) };
        }
        return event;
    }

    let flags = unsafe { CGEventGetFlags(event) };
    let cmd = (flags & FLAG_COMMAND) != 0;
    let ctrl = (flags & FLAG_CONTROL) != 0;
    let alt = (flags & FLAG_ALTERNATE) != 0;
    let shift = (flags & FLAG_SHIFT) != 0;

    if event_type == EVENT_KEY_DOWN {
        let keycode = unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_KEYCODE) } as u16;
        // Global shortcut: Ctrl + Option + Cmd + S
        if ctrl && alt && cmd && keycode == KEY_S {
            let _ = telemetry
                .ui_sender
                .try_send(LiveOverlayEvent::ToggleCaptureShortcut);
            return event;
        }
    }

    if !telemetry.active.load(Ordering::Acquire) {
        return event;
    }

    if matches!(
        event_type,
        EVENT_MOUSE_MOVED | EVENT_LEFT_MOUSE_DRAGGED | EVENT_RIGHT_MOUSE_DRAGGED
    ) && (!shift || telemetry.draw_mode.load(Ordering::Acquire))
    {
        return event;
    }

    let t_ms = telemetry.elapsed_ms();
    let loc = unsafe { CGEventGetLocation(event) };
    let (nx, ny, fw, fh) = telemetry.normalize_point(loc.x, loc.y);

    match event_type {
        EVENT_FLAGS_CHANGED => {
            let was_shift = telemetry.shift_held.swap(shift, Ordering::AcqRel);
            if was_shift != shift {
                let _ = telemetry.ui_sender.try_send(LiveOverlayEvent::ShiftHeld);
            }
            if !shift {
                telemetry.finish_active_laser(Some((loc.x, loc.y)));
            }
        }
        EVENT_MOUSE_MOVED | EVENT_LEFT_MOUSE_DRAGGED | EVENT_RIGHT_MOUSE_DRAGGED => {
            telemetry.shift_held.store(true, Ordering::Release);
            telemetry.record_laser_point(loc.x, loc.y);
        }
        EVENT_LEFT_MOUSE_DOWN | EVENT_RIGHT_MOUSE_DOWN => {
            if telemetry.draw_mode.load(Ordering::Acquire) {
                return event;
            }
            let label_and_role = ax_element_at(loc.x, loc.y);
            let label = label_and_role.as_ref().map(|(l, _)| l.clone());
            *telemetry
                .drag_start
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((t_ms, nx, ny, label));
        }
        EVENT_LEFT_MOUSE_UP | EVENT_RIGHT_MOUSE_UP => {
            if telemetry.draw_mode.load(Ordering::Acquire) {
                return event;
            }
            let start = telemetry
                .drag_start
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let (t0, x0, y0, start_label) = start.unwrap_or((t_ms, nx, ny, None));
            let dist_px = ((nx - x0) * fw as f32).hypot((ny - y0) * fh as f32);
            if dist_px < 12.0 {
                let (label, role) = ax_element_at(loc.x, loc.y)
                    .map(|(l, r)| (Some(l), r))
                    .unwrap_or((start_label, None));
                if let Some(ref l) = label {
                    let _ = telemetry
                        .ui_sender
                        .try_send(LiveOverlayEvent::ActionCaption(format!("Clicked “{l}”")));
                } else {
                    let _ = telemetry
                        .ui_sender
                        .try_send(LiveOverlayEvent::RequestSnapshot);
                }
                telemetry
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Click {
                        t: t0,
                        x: x0,
                        y: y0,
                        button: Some(if event_type == EVENT_RIGHT_MOUSE_UP {
                            "right".into()
                        } else {
                            "left".into()
                        }),
                        label,
                        role,
                    });
            } else {
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::RequestSnapshot);
                telemetry
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Drag {
                        t: t0,
                        t1: t_ms,
                        x0,
                        y0,
                        x1: nx,
                        y1: ny,
                        label: start_label,
                    });
            }
        }
        EVENT_SCROLL_WHEEL => {
            let dy =
                unsafe { CGEventGetIntegerValueField(event, SCROLL_WHEEL_DELTA_AXIS_1) } as f32;
            let dx =
                unsafe { CGEventGetIntegerValueField(event, SCROLL_WHEEL_DELTA_AXIS_2) } as f32;
            if dx.abs() > 0.0 || dy.abs() > 0.0 {
                telemetry
                    .events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(InteractionEvent::Scroll {
                        t: t_ms,
                        x: nx,
                        y: ny,
                        dx,
                        dy,
                        label: None,
                    });
            }
        }
        EVENT_KEY_DOWN => {
            let keycode =
                unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_KEYCODE) } as u16;
            if keycode == KEY_ESCAPE {
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::CancelCapture);
            } else if cmd && keycode == KEY_Z {
                let _ = telemetry.ui_sender.try_send(LiveOverlayEvent::UndoStroke);
            } else if !cmd
                && !ctrl
                && !alt
                && keycode == KEY_D
                && telemetry.draw_mode.load(Ordering::Acquire)
            {
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::ToggleDrawMode);
            } else if cmd || ctrl {
                // Only log modifier-chord shortcuts, never ordinary typing or secrets.
                if let Some(key_name) = shortcut_key_name(keycode) {
                    let mut chord = String::new();
                    if ctrl {
                        chord.push('⌃');
                    }
                    if alt {
                        chord.push('⌥');
                    }
                    if shift {
                        chord.push('⇧');
                    }
                    if cmd {
                        chord.push('⌘');
                    }
                    chord.push_str(key_name);
                    telemetry
                        .events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(InteractionEvent::Key {
                            t: t_ms,
                            shortcut: chord,
                        });
                }
            }
        }
        _ => {}
    }

    event
}

fn current_mouse_location() -> Option<CGPoint> {
    let ev = unsafe { CGEventCreate(ptr::null()) };
    if ev.is_null() {
        return None;
    }
    let loc = unsafe { CGEventGetLocation(ev) };
    unsafe { CFRelease(ev.cast()) };
    Some(loc)
}

/// 120Hz global HID state poller so Shift-laser (`⇧` + move), Shift-drag ink (`⇧` + drag),
/// and shortcuts work across all apps even when Scrawl is in the background.
fn run_global_hid_poller(telemetry: Arc<SharedCaptureTelemetry>) {
    let mut prev_shortcut = false;
    let mut prev_shift_laser = false;
    let mut prev_shift_ink = false;
    let mut prev_undo = false;
    loop {
        thread::sleep(Duration::from_millis(8));
        // HID_SYSTEM_STATE = 1
        let flags = unsafe { CGEventSourceFlagsState(1) };
        let cmd = (flags & FLAG_COMMAND) != 0;
        let ctrl = (flags & FLAG_CONTROL) != 0;
        let alt = (flags & FLAG_ALTERNATE) != 0;
        let shift = (flags & FLAG_SHIFT) != 0;
        let key_s = unsafe { CGEventSourceKeyState(1, KEY_S) };
        let key_z = unsafe { CGEventSourceKeyState(1, KEY_Z) };

        let shortcut_now = ctrl && alt && cmd && key_s;
        if shortcut_now && !prev_shortcut {
            let _ = telemetry
                .ui_sender
                .try_send(LiveOverlayEvent::ToggleCaptureShortcut);
        }
        prev_shortcut = shortcut_now;

        if !telemetry.active.load(Ordering::Acquire) {
            prev_shift_laser = false;
            prev_shift_ink = false;
            prev_undo = false;
            continue;
        }

        let undo_now = cmd && !ctrl && !alt && !shift && key_z;
        if undo_now && !prev_undo {
            let _ = telemetry.ui_sender.try_send(LiveOverlayEvent::UndoStroke);
        }
        prev_undo = undo_now;

        let Some(loc) = current_mouse_location() else {
            continue;
        };
        *telemetry
            .pointer_screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = [loc.x as f32, loc.y as f32];

        let drawing = telemetry.draw_mode.load(Ordering::Acquire);
        let left_down = unsafe { CGEventSourceButtonState(1, 0) };
        let pure_shift = shift && !cmd && !ctrl && !alt;

        if !drawing {
            // Hold Shift + drag = draw persistent ink in any app; Hold Shift + move = optical laser
            let shift_ink = pure_shift && left_down;
            let shift_laser = pure_shift && !left_down;

            if shift_ink {
                if prev_shift_laser {
                    telemetry.finish_active_laser(None);
                }
                telemetry.shift_held.store(false, Ordering::Release);
                let t_ms = telemetry.elapsed_ms();
                let (nx, ny, fw, fh) = telemetry.normalize_point(loc.x, loc.y);
                let sx = (nx * SCALE as f32).round() as i32;
                let sy = (ny * SCALE as f32).round() as i32;
                let mut guard = telemetry
                    .live_ink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                if should_push && stroke.pts.len() / 3 < 2_000 {
                    stroke.pts.extend_from_slice(&[sx, sy, dt]);
                    stroke.t1 = t_ms;
                }
            } else if prev_shift_ink {
                if let Some(mut stroke) = telemetry
                    .live_ink
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    && stroke.pts.len() >= 3
                {
                    stroke.on = ax_element_at(loc.x, loc.y).map(|(l, _)| l);
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

            if shift_laser {
                telemetry.shift_held.store(true, Ordering::Release);
                telemetry.record_laser_point(loc.x, loc.y);
            } else if prev_shift_laser && !shift_ink {
                telemetry.shift_held.store(false, Ordering::Release);
                telemetry.finish_active_laser(Some((loc.x, loc.y)));
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::RequestSnapshot);
            }

            prev_shift_ink = shift_ink;
            prev_shift_laser = shift_laser;
        } else {
            telemetry.shift_held.store(false, Ordering::Release);
            prev_shift_laser = false;
            prev_shift_ink = false;
        }
    }
}

fn shortcut_key_name(keycode: u16) -> Option<&'static str> {
    match keycode {
        0 => Some("A"),
        1 => Some("S"),
        2 => Some("D"),
        3 => Some("F"),
        6 => Some("Z"),
        7 => Some("X"),
        8 => Some("C"),
        9 => Some("V"),
        11 => Some("B"),
        12 => Some("Q"),
        13 => Some("W"),
        14 => Some("E"),
        15 => Some("R"),
        17 => Some("T"),
        31 => Some("O"),
        35 => Some("P"),
        36 => Some("↩"),
        40 => Some("K"),
        45 => Some("N"),
        48 => Some("⇥"),
        49 => Some("Space"),
        51 => Some("⌫"),
        _ => None,
    }
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

    for (kf, frame) in annotated_frames {
        let frame_path = out_dir.join(&kf.file);
        crate::render::save_png(frame, &frame_path)?;
    }

    let sheet_path = out_dir.join(&bundle.contact_sheet);
    crate::render::save_png(contact_sheet, &sheet_path)?;

    let manifest_bytes = serde_json::to_vec_pretty(bundle)?;
    std::fs::write(out_dir.join("scrawl.json"), manifest_bytes)?;
    std::fs::write(out_dir.join("summary.md"), summary_md.as_bytes())?;

    Ok(())
}
