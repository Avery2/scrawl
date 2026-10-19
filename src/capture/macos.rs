//! macOS capture: Carbon hotkeys, the CoreGraphics event tap and HID state poller, NSWorkspace
//! for the frontmost app, and ScreenCaptureKit for displays, windows and the recording.

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use super::{
    CANDIDATE_EDGE, CANDIDATE_EDGE_NO_VIDEO, CARBON_CMD_KEY, CARBON_CONTROL_KEY, CARBON_OPTION_KEY,
    CARBON_SHIFT_KEY, FRAME_SAMPLE_INTERVAL, LiveOverlayEvent, Sampler, SharedCaptureTelemetry,
    ShortcutConfig, ShortcutKind, capture_size, clean_label, hotkey_combos, layout_default_keycode,
    pick_display, poll_clicks, scrawl_log, shortcut_matches, track_shift_ink,
};
use crate::model::{CaptureMode, InteractionEvent, RgbaFrame, TargetFrame, TargetInfo};

/// Recordings may hold up to twice a frame's size in points (Retina pixels).
pub const MAX_CAPTURE_SCALE: f64 = 2.0;

/// `~/Library/Logs/Scrawl`.
pub fn log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Logs/Scrawl"))
}

pub fn main_display_id() -> u32 {
    unsafe { CGMainDisplayID() }
}

/// CGWindowList's windows, front to back, in an autorelease pool.
pub(super) fn raw_windows_now() -> Vec<crate::windows::RawWindow> {
    objc2::rc::autoreleasepool(|_| crate::windows::raw_windows())
}

type CfTypeRef = *const c_void;
type EventRef = *mut c_void;

const EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const EVENT_LEFT_MOUSE_UP: u32 = 2;
const EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const EVENT_RIGHT_MOUSE_UP: u32 = 4;
const EVENT_KEY_DOWN: u32 = 10;
const EVENT_SCROLL_WHEEL: u32 = 22;
const EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = u32::MAX - 1;
const EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = u32::MAX;

const KEYBOARD_EVENT_KEYCODE: u32 = 9;
const KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
/// `kCGEventTargetUnixProcessID`: the process an annotated session event is going to.
const EVENT_TARGET_UNIX_PROCESS_ID: u32 = 40;
const SCROLL_WHEEL_DELTA_AXIS_1: u32 = 11;
const SCROLL_WHEEL_DELTA_AXIS_2: u32 = 12;

const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

/// The S key on the ANSI layout (`kVK_ANSI_S`).
pub const KEY_S: u16 = 1;
const KEY_ESCAPE: u16 = 53;
const KEY_Z: u16 = 6;
pub const KEY_V: u16 = 9;

// Carbon HIToolbox global hotkey constants (requires zero TCC permissions).
const K_EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
const K_EVENT_PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
const HOTKEY_SIG: u32 = u32::from_be_bytes(*b"SCRW");
const HOTKEY_ID_TOGGLE: u32 = 1;
const HOTKEY_ID_ESCAPE: u32 = 2;
const HOTKEY_ID_ALT1: u32 = 3;
const HOTKEY_ID_PASTE_BASE: u32 = 100;

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn GetEventDispatcherTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i32,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        hot_key_code: u32,
        hot_key_modifiers: u32,
        hot_key_id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key: *mut c_void) -> i32;
    fn GetEventParameter(
        event: *mut c_void,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        buffer_size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
    fn TISCopyCurrentKeyboardLayoutInputSource() -> *const c_void;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: *const c_void;
    fn TISGetInputSourceProperty(
        input_source: *const c_void,
        property_key: *const c_void,
    ) -> *const c_void;
    fn UCKeyTranslate(
        layout: *const u8,
        key_code: u16,
        key_action: u16,
        modifier_state: u32,
        keyboard_type: u32,
        options: isize,
        dead_key_state: *mut u32,
        max_length: isize,
        actual_length: *mut isize,
        output: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u8;
}

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
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: EventTapCallback,
        user_info: *mut c_void,
    ) -> *mut c_void;
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    fn CGEventTapIsEnabled(tap: *mut c_void) -> bool;
    fn CGEventSourceButtonState(state_id: u32, button: u32) -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
    fn CGEventCreate(source: *const c_void) -> EventRef;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> EventRef;
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtual_key: u16,
        key_down: bool,
    ) -> EventRef;
    fn CGEventSetFlags(event: EventRef, flags: u64);
    fn CGEventPost(tap: u32, event: EventRef);
    fn CGEventSourceFlagsState(state_id: u32) -> u64;
    fn CGEventSourceKeyState(state_id: u32, key: u16) -> bool;
    fn CGEventGetFlags(event: EventRef) -> u64;
    fn CGEventGetLocation(event: EventRef) -> CGPoint;
    fn CGEventGetIntegerValueField(event: EventRef, field: u32) -> i64;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayPixelsWide(display: u32) -> usize;
    fn CGDisplayPixelsHigh(display: u32) -> usize;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
    fn CFMachPortCreateRunLoopSource(
        allocator: *const c_void,
        port: *mut c_void,
        order: isize,
    ) -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: *mut c_void);
    fn CFRetain(value: CfTypeRef) -> CfTypeRef;
    fn CFRelease(value: CfTypeRef);
    static kCFRunLoopCommonModes: *const c_void;
}

static LAYOUT_KEY_CODES: std::sync::OnceLock<std::collections::HashMap<char, u16>> =
    std::sync::OnceLock::new();

fn translate_keycode(layout: *const u8, key_code: u16) -> Option<char> {
    if layout.is_null() {
        return None;
    }
    let mut dead_key_state = 0_u32;
    let mut output = [0_u16; 4];
    let mut length = 0_isize;
    let status = unsafe {
        UCKeyTranslate(
            layout,
            key_code,
            3, // kUCKeyActionDisplay
            0,
            u32::from(LMGetKbdType()),
            0,
            &raw mut dead_key_state,
            output.len() as isize,
            &raw mut length,
            output.as_mut_ptr(),
        )
    };
    if status == 0 && length > 0 {
        String::from_utf16(&output[..length as usize])
            .ok()?
            .chars()
            .next()
            .map(|c| c.to_ascii_lowercase())
    } else {
        None
    }
}

/// Builds the active keyboard layout character -> virtual keycode map (e.g. Dvorak `'s' -> 41`, `'z' -> 44`).
/// Must be called on the main thread during startup before background threads query it.
pub fn init_keyboard_layout_map() -> &'static std::collections::HashMap<char, u16> {
    LAYOUT_KEY_CODES.get_or_init(|| {
        let mut map = std::collections::HashMap::new();
        for source in unsafe {
            [
                TISCopyCurrentKeyboardLayoutInputSource(),
                TISCopyCurrentASCIICapableKeyboardLayoutInputSource(),
            ]
        } {
            if source.is_null() {
                continue;
            }
            let data =
                unsafe { TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) };
            if !data.is_null() {
                let bytes = unsafe { CFDataGetBytePtr(data) };
                for code in 0..128_u16 {
                    if let Some(ch) = translate_keycode(bytes, code) {
                        map.entry(ch).or_insert(code);
                    }
                }
            }
            unsafe { CFRelease(source) };
        }
        scrawl_log(format!(
            "keyboard layout resolved: 's'={:?} 'z'={:?}",
            map.get(&'s'),
            map.get(&'z')
        ));
        map
    })
}

pub fn key_code_for_char(ch: char) -> Option<u16> {
    init_keyboard_layout_map()
        .get(&ch.to_ascii_lowercase())
        .copied()
}

pub fn layout_s_keycode() -> u16 {
    key_code_for_char('s').unwrap_or(KEY_S)
}

pub fn layout_z_keycode() -> u16 {
    key_code_for_char('z').unwrap_or(KEY_Z)
}

pub fn layout_v_keycode() -> u16 {
    key_code_for_char('v').unwrap_or(KEY_V)
}

/// The registered Carbon hotkeys, so a change can replace all of them.
struct CarbonHotKeyState {
    refs: Vec<usize>,
}

static CARBON_HOTKEYS: std::sync::OnceLock<Mutex<CarbonHotKeyState>> = std::sync::OnceLock::new();

unsafe extern "C" fn carbon_hotkey_handler(
    _next_handler: *mut c_void,
    event: *mut c_void,
    user_data: *mut c_void,
) -> i32 {
    scrawl_log(format!(
        "carbon_hotkey_handler entered: event={event:p} user_data={user_data:p}"
    ));
    if event.is_null() || user_data.is_null() {
        return -1;
    }
    let mut hotkey_id = EventHotKeyID {
        signature: 0,
        id: 0,
    };
    let status = unsafe {
        GetEventParameter(
            event,
            K_EVENT_PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            ptr::null_mut(),
            std::mem::size_of::<EventHotKeyID>(),
            ptr::null_mut(),
            (&raw mut hotkey_id).cast(),
        )
    };
    let sig = hotkey_id.signature;
    let id = hotkey_id.id;
    scrawl_log(format!(
        "carbon_hotkey_handler GetEventParameter status={status} sig={sig:#x} id={id}"
    ));
    if status == 0 && sig == HOTKEY_SIG {
        let telemetry = unsafe { &*user_data.cast::<SharedCaptureTelemetry>() };
        match id {
            HOTKEY_ID_ESCAPE => {
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::CancelCapture);
            }
            id if id >= HOTKEY_ID_PASTE_BASE => {
                telemetry.emit_paste_shortcut();
            }
            _ => {
                telemetry.emit_capture_shortcut();
            }
        }
    }
    0
}

/// Installs Carbon `RegisterEventHotKey` on both `GetEventDispatcherTarget()` and
/// `GetApplicationEventTarget()`. Works system-wide across all apps with zero TCC permissions.
pub fn install_global_hotkeys(telemetry: &Arc<SharedCaptureTelemetry>) {
    let _ = init_keyboard_layout_map();
    let s_code = u32::from(layout_s_keycode());
    let state_mutex = CARBON_HOTKEYS.get_or_init(|| {
        let spec = EventTypeSpec {
            event_class: K_EVENT_CLASS_KEYBOARD,
            event_kind: K_EVENT_HOT_KEY_PRESSED,
        };
        let user_data = Arc::into_raw(Arc::clone(telemetry))
            .cast_mut()
            .cast::<c_void>();
        unsafe {
            let mut handler_ref: *mut c_void = ptr::null_mut();
            let dispatcher = GetEventDispatcherTarget();
            let app_target = GetApplicationEventTarget();
            scrawl_log(format!(
                "install_global_hotkeys: dispatcher={dispatcher:p} app_target={app_target:p} s_code={s_code}"
            ));
            if !dispatcher.is_null() {
                let rc = InstallEventHandler(
                    dispatcher,
                    carbon_hotkey_handler,
                    1,
                    &raw const spec,
                    user_data,
                    &raw mut handler_ref,
                );
                scrawl_log(format!(
                    "InstallEventHandler(dispatcher) -> rc={rc} handler_ref={handler_ref:p}"
                ));
            }
            if !app_target.is_null() && app_target != dispatcher {
                let mut app_handler_ref: *mut c_void = ptr::null_mut();
                let rc = InstallEventHandler(
                    app_target,
                    carbon_hotkey_handler,
                    1,
                    &raw const spec,
                    user_data,
                    &raw mut app_handler_ref,
                );
                scrawl_log(format!(
                    "InstallEventHandler(app_target) -> rc={rc} handler_ref={app_handler_ref:p}"
                ));
            }
        }
        Mutex::new(CarbonHotKeyState { refs: Vec::new() })
    });
    let (record, paste) = telemetry.shortcuts();
    if let Err(kind) = register_shortcuts(state_mutex, &record, &paste) {
        scrawl_log(format!(
            "install_global_hotkeys: {kind:?} shortcut could not be registered"
        ));
    }
}

fn hotkey_target() -> *mut c_void {
    unsafe {
        let a = GetApplicationEventTarget();
        if a.is_null() {
            GetEventDispatcherTarget()
        } else {
            a
        }
    }
}

fn unregister_all(state: &mut CarbonHotKeyState) {
    for r in state.refs.drain(..) {
        unsafe { UnregisterEventHotKey(r as *mut c_void) };
    }
}

/// Replaces every registered hotkey with `record`'s and `paste`'s. Fails with the shortcut whose
/// own combo macOS refused (another app may hold it); its aliases are best effort.
fn register_shortcuts(
    state_mutex: &Mutex<CarbonHotKeyState>,
    record: &ShortcutConfig,
    paste: &ShortcutConfig,
) -> Result<(), ShortcutKind> {
    let mut state = state_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    unregister_all(&mut state);
    let target = hotkey_target();
    let mut failed = None;
    for (kind, cfg) in [(ShortcutKind::Record, record), (ShortcutKind::Paste, paste)] {
        for (i, (code, mods)) in hotkey_combos(kind, cfg).into_iter().enumerate() {
            let mut r: *mut c_void = ptr::null_mut();
            let id = match (kind, i) {
                (ShortcutKind::Record, 0) => HOTKEY_ID_TOGGLE,
                (ShortcutKind::Record, i) => HOTKEY_ID_ALT1 + i as u32,
                (ShortcutKind::Paste, i) => HOTKEY_ID_PASTE_BASE + i as u32,
            };
            let rc = unsafe {
                RegisterEventHotKey(
                    code,
                    mods,
                    EventHotKeyID {
                        signature: HOTKEY_SIG,
                        id,
                    },
                    target,
                    0,
                    &raw mut r,
                )
            };
            scrawl_log(format!(
                "RegisterEventHotKey({kind:?} {}, code={code}, mods={mods}) -> rc={rc} ref={r:p}",
                if i == 0 { cfg.label.as_str() } else { "alias" }
            ));
            if rc == 0 && !r.is_null() {
                state.refs.push(r as usize);
            } else if i == 0 && failed.is_none() {
                failed = Some(kind);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

/// Sets both shortcuts and registers them. If macOS refuses either, the old ones are put back
/// and the refused one is returned. Nothing is saved here (`crate::settings` does that).
pub fn set_shortcuts(
    telemetry: &SharedCaptureTelemetry,
    record: ShortcutConfig,
    paste: ShortcutConfig,
) -> Result<(), ShortcutKind> {
    let (old_record, old_paste) = telemetry.shortcuts();
    if let Some(state) = CARBON_HOTKEYS.get()
        && !telemetry.shortcuts_paused.load(Ordering::Acquire)
        && let Err(kind) = register_shortcuts(state, &record, &paste)
    {
        let _ = register_shortcuts(state, &old_record, &old_paste);
        return Err(kind);
    }
    *telemetry
        .shortcut
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = record;
    *telemetry
        .paste_shortcut
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = paste;
    Ok(())
}

/// While Settings records a new shortcut, Scrawl's own stop answering, so pressing the current
/// combo reaches the recorder instead of starting a capture.
pub fn pause_shortcuts(telemetry: &SharedCaptureTelemetry, paused: bool) {
    if telemetry.shortcuts_paused.swap(paused, Ordering::AcqRel) == paused {
        return;
    }
    let Some(state) = CARBON_HOTKEYS.get() else {
        return;
    };
    if paused {
        unregister_all(
            &mut state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    } else {
        let (record, paste) = telemetry.shortcuts();
        let _ = register_shortcuts(state, &record, &paste);
    }
}

/// The review window's own recorder: sets and saves the record shortcut.
pub fn update_global_shortcut(telemetry: &SharedCaptureTelemetry, config: ShortcutConfig) {
    let (_, paste) = telemetry.shortcuts();
    match set_shortcuts(telemetry, config.clone(), paste) {
        Ok(()) => crate::settings::save_shortcut(ShortcutKind::Record, &config),
        Err(kind) => scrawl_log(format!("update_global_shortcut: {kind:?} refused")),
    }
}

/// Whether this running copy of Scrawl may record the screen. A grant made while it runs may
/// only show up here after a relaunch.
pub fn screen_capture_granted() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// The system prompt for Screen Recording (once; later calls do nothing).
pub fn request_screen_capture() {
    unsafe {
        let _ = CGRequestScreenCaptureAccess();
    }
}

/// Whether ⌥⇧V may post ⌘V into the app in front (System Settings › Accessibility). Unlike
/// Screen Recording, a grant shows up here while Scrawl runs.
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// The system prompt for Input Monitoring (once; later calls do nothing).
pub fn request_listen_access() {
    unsafe {
        let _ = CGRequestListenEventAccess();
    }
}

/// The ⌃⌥⇧⌘ modifier flags held right now (as `CGEventFlags` bits).
pub fn modifier_flags() -> u64 {
    let flags = unsafe { CGEventSourceFlagsState(0) | CGEventSourceFlagsState(1) };
    flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_SHIFT)
}

/// Whether Esc is down right now, whichever app has focus.
pub fn escape_down() -> bool {
    unsafe { CGEventSourceKeyState(0, KEY_ESCAPE) || CGEventSourceKeyState(1, KEY_ESCAPE) }
}

pub fn shortcut_from_key_and_mods(
    key: &str,
    cmd: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
) -> Option<ShortcutConfig> {
    let lower = key.to_ascii_lowercase();
    let (key_code, key_label): (u32, String) = if lower.chars().count() == 1
        && let Some(ch) = lower.chars().next()
        && ch.is_ascii_alphanumeric()
        && let Some(code) = key_code_for_char(ch)
    {
        (u32::from(code), ch.to_ascii_uppercase().to_string())
    } else {
        let (code, lbl) = match lower.as_str() {
            "a" => (0, "A"),
            "s" => (u32::from(layout_s_keycode()), "S"),
            "d" => (2, "D"),
            "f" => (3, "F"),
            "h" => (4, "H"),
            "g" => (5, "G"),
            "z" => (u32::from(layout_z_keycode()), "Z"),
            "x" => (7, "X"),
            "c" => (8, "C"),
            "v" => (9, "V"),
            "b" => (11, "B"),
            "q" => (12, "Q"),
            "w" => (13, "W"),
            "e" => (14, "E"),
            "r" => (15, "R"),
            "y" => (16, "Y"),
            "t" => (17, "T"),
            "1" => (18, "1"),
            "2" => (19, "2"),
            "3" => (20, "3"),
            "4" => (21, "4"),
            "6" => (22, "6"),
            "5" => (23, "5"),
            "9" => (25, "9"),
            "7" => (26, "7"),
            "8" => (28, "8"),
            "0" => (29, "0"),
            "o" => (31, "O"),
            "u" => (32, "U"),
            "i" => (34, "I"),
            "p" => (35, "P"),
            "l" => (37, "L"),
            "j" => (38, "J"),
            "k" => (40, "K"),
            "n" => (45, "N"),
            "m" => (46, "M"),
            "space" | " " => (49, "Space"),
            "f1" => (122, "F1"),
            "f2" => (120, "F2"),
            "f3" => (99, "F3"),
            "f4" => (118, "F4"),
            "f5" => (96, "F5"),
            "f6" => (97, "F6"),
            "f7" => (98, "F7"),
            "f8" => (100, "F8"),
            "f9" => (101, "F9"),
            "f10" => (109, "F10"),
            "f11" => (103, "F11"),
            "f12" => (111, "F12"),
            _ => return None,
        };
        (code, lbl.to_string())
    };
    let is_fn_key = key_label.starts_with('F') && key_label.len() >= 2;
    if !cmd && !ctrl && !alt && !is_fn_key {
        return None;
    }
    let mut carbon_modifiers = 0_u32;
    let mut label = String::new();
    if ctrl {
        carbon_modifiers |= CARBON_CONTROL_KEY;
        label.push('⌃');
    }
    if alt {
        carbon_modifiers |= CARBON_OPTION_KEY;
        label.push('⌥');
    }
    if shift {
        carbon_modifiers |= CARBON_SHIFT_KEY;
        label.push('⇧');
    }
    if cmd {
        carbon_modifiers |= CARBON_CMD_KEY;
        label.push('⌘');
    }
    label.push_str(&key_label);
    Some(ShortcutConfig {
        key_code,
        carbon_modifiers,
        label,
    })
}

/// Discovers the current capture target (either a specific window/app or the main display).
pub fn discover_target(window_query: Option<&str>) -> TargetInfo {
    discover_target_at(window_query, current_mouse_location().map(|p| [p.x, p.y]))
}

/// Like `discover_target`, recording the display under `point` (see `pick_display`).
pub fn discover_target_at(window_query: Option<&str>, point: Option<[f64; 2]>) -> TargetInfo {
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

        let displays = content.displays();
        let front_window = frontmost.as_ref().and_then(|(pid, _, _)| {
            content.windows().into_iter().find(|w| {
                w.is_on_screen()
                    && w.window_layer() == 0
                    && w.owning_application()
                        .as_ref()
                        .is_some_and(|a| a.process_id() == *pid)
            })
        });
        let rects: Vec<[f64; 4]> = displays
            .iter()
            .map(|d| {
                let f = d.frame();
                [f.origin.x, f.origin.y, f.size.width, f.size.height]
            })
            .collect();
        let focused = front_window.as_ref().map(|w| {
            let f = w.frame();
            [f.origin.x, f.origin.y, f.size.width, f.size.height]
        });
        if let Some(display) = displays.get(pick_display(&rects, point, focused)) {
            let frame = display.frame();
            let window_title = front_window.as_ref().and_then(|w| w.title());
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

/// Lists all on-screen normal (`window_layer == 0`) windows in front-to-back order for the Window picker.
pub fn list_selectable_windows() -> Vec<TargetInfo> {
    let Ok(content) = SCShareableContent::get() else {
        return Vec::new();
    };
    let our_pid = std::process::id() as i32;
    let display_id = content
        .displays()
        .first()
        .map(|d| d.display_id())
        .unwrap_or_else(|| unsafe { CGMainDisplayID() });

    content
        .windows()
        .into_iter()
        .filter(|w| {
            let f = w.frame();
            w.is_on_screen()
                && w.window_layer() == 0
                && f.size.width >= 80.0
                && f.size.height >= 80.0
                && w.owning_application()
                    .as_ref()
                    .is_some_and(|a| a.process_id() != our_pid)
        })
        .map(|w| {
            let f = w.frame();
            let owner = w.owning_application();
            TargetInfo {
                mode: CaptureMode::Window,
                app: owner
                    .as_ref()
                    .map(|a| clean_label(&a.application_name()))
                    .filter(|s| !s.is_empty()),
                bundle_id: owner.as_ref().map(|a| a.bundle_identifier()),
                window_title: w.title().map(|t| clean_label(&t)).filter(|t| !t.is_empty()),
                window_id: Some(w.window_id()),
                display_id,
                frame: TargetFrame {
                    x: f.origin.x,
                    y: f.origin.y,
                    width: f.size.width.max(1.0),
                    height: f.size.height.max(1.0),
                    scale: 2.0,
                },
            }
        })
        .collect()
}

static LAST_EXTERNAL_APP_PID: AtomicI32 = AtomicI32::new(0);

pub fn record_external_frontmost_app() {
    use objc2_app_kit::NSWorkspace;
    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        let pid = app.processIdentifier();
        if pid > 0 && pid != std::process::id() as i32 {
            LAST_EXTERNAL_APP_PID.store(pid, Ordering::Release);
        }
    }
}

pub fn ensure_external_app_active() {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};
    let our_pid = std::process::id() as i32;
    let workspace = NSWorkspace::sharedWorkspace();
    let front_pid = workspace
        .frontmostApplication()
        .map_or(0, |a| a.processIdentifier());
    if front_pid != 0 && front_pid != our_pid {
        LAST_EXTERNAL_APP_PID.store(front_pid, Ordering::Release);
        return;
    }
    let target_pid = LAST_EXTERNAL_APP_PID.load(Ordering::Acquire);
    if target_pid > 0
        && target_pid != our_pid
        && let Some(target_app) =
            NSRunningApplication::runningApplicationWithProcessIdentifier(target_pid)
    {
        #[allow(deprecated)]
        let _ = target_app
            .activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        let deadline = Instant::now() + Duration::from_millis(220);
        while Instant::now() < deadline {
            if workspace
                .frontmostApplication()
                .is_some_and(|a| a.processIdentifier() == target_pid)
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// The frontmost app's pid, or the last external one while Scrawl itself is in front.
pub fn frontmost_external_pid() -> Option<i32> {
    use objc2_app_kit::NSWorkspace;
    let our_pid = std::process::id() as i32;
    let pid = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .map_or(0, |a| a.processIdentifier());
    if pid > 0 && pid != our_pid {
        return Some(pid);
    }
    let fallback = LAST_EXTERNAL_APP_PID.load(Ordering::Acquire);
    (fallback > 0 && fallback != our_pid).then_some(fallback)
}

pub fn frontmost_app_info() -> Option<(i32, String, Option<String>)> {
    use objc2_app_kit::{NSRunningApplication, NSWorkspace};
    let our_pid = std::process::id() as i32;
    let workspace = NSWorkspace::sharedWorkspace();
    if let Some(app) = workspace.frontmostApplication() {
        let pid = app.processIdentifier();
        if pid != our_pid {
            LAST_EXTERNAL_APP_PID.store(pid, Ordering::Release);
            let name = app.localizedName()?.to_string();
            let bundle_id = app.bundleIdentifier().map(|b| b.to_string());
            return Some((pid, name, bundle_id));
        }
    }
    let fallback_pid = LAST_EXTERNAL_APP_PID.load(Ordering::Acquire);
    if fallback_pid > 0 && fallback_pid != our_pid {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(fallback_pid)?;
        let name = app.localizedName()?.to_string();
        let bundle_id = app.bundleIdentifier().map(|b| b.to_string());
        return Some((fallback_pid, name, bundle_id));
    }
    None
}

/// The ScreenCaptureKit sampler: the recording stream writes `recording.mp4` while candidate
/// screenshots are taken every `buffer.interval()` (or at once when urgent).
pub(super) fn run_sampler(sampler: Sampler) -> Option<PathBuf> {
    let Sampler {
        target: sampler_target,
        video_output_path,
        excluded_window_ids,
        started_at,
        stopping: thread_stopping,
        urgent: thread_urgent,
        frames: thread_frames,
        video_offset_ms: thread_offset,
    } = sampler;
    let initial_excluded = excluded_window_ids
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    // Keep SCK initialization, capture and teardown on the owning worker.
    // stop() joins even if it arrives while setup is still in progress.
    let (stream, recording_output, video_path, filter) = setup_sck_capture(
        &sampler_target,
        video_output_path,
        &initial_excluded,
        started_at,
        &thread_offset,
    );
    crate::ui::start_metric("sck_ready_ms", started_at);
    let shot_edge = if video_path.is_some() {
        CANDIDATE_EDGE
    } else {
        CANDIDATE_EDGE_NO_VIDEO
    };
    let shot_config = sck_config(&sampler_target, shot_edge);
    let mut filter_and_config = filter.map(|f| (f, shot_config));
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
            if let Some((new_filter, _)) =
                build_sck_filter_and_config(&sampler_target, &cur_excluded)
            {
                // The recording must hide the same windows as the candidates, or
                // keyframes decoded from it could show Scrawl's own overlays.
                if let Some(ref stream) = stream
                    && let Err(error) = stream.update_content_filter(&new_filter)
                {
                    scrawl_log(format!("Recording filter update failed: {error:?}"));
                }
                filter_and_config = Some((new_filter, sck_config(&sampler_target, shot_edge)));
            }
        }
        let urgent = thread_urgent.swap(false, Ordering::AcqRel);
        let interval = thread_frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .interval();
        if urgent || last_sample.elapsed() >= interval {
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
            thread_frames
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(frame, urgent);
        }
        thread::park_timeout(Duration::from_millis(30));
    }
    if let Some(stream) = stream {
        let _ = stream.stop_capture();
        if let Some(ref output) = recording_output {
            let _ = stream.remove_recording_output(output);
        }
    }
    video_path
}

fn build_sck_filter_and_config(
    target: &TargetInfo,
    excluded_window_ids: &[u32],
) -> Option<(SCContentFilter, SCStreamConfiguration)> {
    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(err) => {
            // Screen Recording is asked for in onboarding, never mid-capture.
            scrawl_log(format!(
                "SCShareableContent::get() failed: {err:?} (screen recording granted: {})",
                screen_capture_granted()
            ));
            return None;
        }
    };
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

    // Full-screen captures get auto-cropped to a window or region afterwards (`scrawl autocrop`),
    // so they need enough pixels for a crop to a quarter of the screen to stay sharp.
    let long_edge = if target.mode == CaptureMode::Display {
        2560.0
    } else {
        1280.0
    };
    Some((filter, sck_config(target, long_edge)))
}

fn sck_config(target: &TargetInfo, long_edge: f64) -> SCStreamConfiguration {
    let (width, height) = capture_size(target, long_edge);
    let mut config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_shows_cursor(true);
    if target.mode == CaptureMode::Region {
        config.set_source_rect(CGRect::new(
            target.frame.x,
            target.frame.y,
            target.frame.width.max(32.0),
            target.frame.height.max(32.0),
        ));
    }
    config
}

/// Starts the recording stream; returns it with the filter for candidate screenshots. Stores
/// when the recording started (ms since `started_at`) into `video_offset_ms`.
fn setup_sck_capture(
    target: &TargetInfo,
    video_output_path: Option<PathBuf>,
    excluded_window_ids: &[u32],
    started_at: Instant,
    video_offset_ms: &AtomicU64,
) -> (
    Option<SCStream>,
    Option<SCRecordingOutput>,
    Option<PathBuf>,
    Option<SCContentFilter>,
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
        {
            // The movie's clock starts while `start_capture` runs; its midpoint is the closest
            // estimate this side of the recording delegate.
            let before = started_at.elapsed();
            if stream.start_capture().is_ok() {
                let after = started_at.elapsed();
                video_offset_ms.store(((before + after) / 2).as_millis() as u64, Ordering::Release);
                return (
                    Some(stream),
                    Some(rec_output),
                    Some(video_path),
                    Some(filter),
                );
            }
        }
    }

    (None, None, None, Some(filter))
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

/// Re-enables the event tap if macOS turned it off, and notes whether it is delivering.
pub(super) fn check_tap(telemetry: &SharedCaptureTelemetry) -> bool {
    let tap = telemetry.tap_port.load(Ordering::Acquire) as *mut c_void;
    let live = !tap.is_null()
        && unsafe {
            CGEventTapIsEnabled(tap) || {
                CGEventTapEnable(tap, true);
                CGEventTapIsEnabled(tap)
            }
        };
    if telemetry.tap_live.swap(live, Ordering::AcqRel) != live {
        scrawl_log(if live {
            "event tap live".to_string()
        } else {
            format!(
                "event tap off (Input Monitoring {}, or Secure Input is on); clicks come from \
                 polling the mouse buttons",
                if unsafe { CGPreflightListenEventAccess() } {
                    "granted"
                } else {
                    "not granted"
                }
            )
        });
    }
    live
}

pub fn start_global_input_tap(telemetry: Arc<SharedCaptureTelemetry>) {
    let hid_telemetry = Arc::clone(&telemetry);
    let _ = thread::Builder::new()
        .name("scrawl-hid-poller".into())
        .spawn(move || run_global_hid_poller(hid_telemetry));
    spawn_event_tap(telemetry);
}

/// Makes the event tap again after Input Monitoring is granted while Scrawl runs, instead of
/// asking for a relaunch: a tap made without the grant never gets key events, and macOS doesn't
/// upgrade it.
pub fn restart_input_tap(telemetry: &Arc<SharedCaptureTelemetry>) {
    scrawl_log("Input Monitoring granted; making the event tap again");
    telemetry.tap_port.store(0, Ordering::Release);
    let old = telemetry.tap_run_loop.swap(0, Ordering::AcqRel) as *mut c_void;
    if !old.is_null() {
        unsafe {
            CFRunLoopStop(old);
            CFRelease(old.cast_const());
        }
    }
    spawn_event_tap(Arc::clone(telemetry));
}

fn spawn_event_tap(telemetry: Arc<SharedCaptureTelemetry>) {
    let _ = thread::Builder::new()
        .name("scrawl-event-tap".into())
        .spawn(move || {
            let mask = (1_u64 << EVENT_LEFT_MOUSE_DOWN)
                | (1_u64 << EVENT_LEFT_MOUSE_UP)
                | (1_u64 << EVENT_RIGHT_MOUSE_DOWN)
                | (1_u64 << EVENT_RIGHT_MOUSE_UP)
                | (1_u64 << EVENT_KEY_DOWN)
                | (1_u64 << EVENT_SCROLL_WHEEL);

            let user_info = Arc::into_raw(Arc::clone(&telemetry))
                .cast_mut()
                .cast::<c_void>();
            // tap = 2 (kCGAnnotatedSessionEventTap), options = 1 (kCGEventTapOptionListenOnly)
            let tap = unsafe { CGEventTapCreate(2, 0, 1, mask, tap_callback, user_info) };
            scrawl_log(format!(
                "CGEventTapCreate -> tap={tap:p} (Input Monitoring {})",
                if unsafe { CGPreflightListenEventAccess() } {
                    "granted"
                } else {
                    "not granted"
                }
            ));
            if tap.is_null() {
                let _ = unsafe { Arc::from_raw(user_info.cast::<SharedCaptureTelemetry>()) };
                return;
            }
            telemetry.tap_port.store(tap as u64, Ordering::Release);
            let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
            if source.is_null() {
                return;
            }
            let run_loop = unsafe { CFRunLoopGetCurrent() };
            let retained = unsafe { CFRetain(run_loop.cast_const()) } as u64;
            telemetry.tap_run_loop.store(retained, Ordering::Release);
            unsafe {
                CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                CGEventTapEnable(tap, true);
                CFRunLoopRun();
                // Stopped by `restart_input_tap`. The port and its callback's Arc are left
                // alive: `check_tap` may still be holding the old pointer.
                CGEventTapEnable(tap, false);
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
                CFRelease(source);
            }
            if telemetry
                .tap_run_loop
                .compare_exchange(retained, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                unsafe { CFRelease(retained as CfTypeRef) };
            }
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
    let pure_shift = shift && !cmd && !ctrl && !alt;

    if event_type == EVENT_KEY_DOWN {
        let keycode = unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_KEYCODE) } as u16;
        if telemetry.matches_shortcut(keycode, cmd, ctrl, alt, shift) {
            // Holding S repeats the key; only a fresh press switches modes.
            if unsafe { CGEventGetIntegerValueField(event, KEYBOARD_EVENT_AUTOREPEAT) } == 0 {
                telemetry.emit_capture_shortcut();
            }
            return event;
        }
    }

    if !telemetry.active.load(Ordering::Acquire) {
        return event;
    }

    let t_ms = telemetry.elapsed_ms();
    let loc = unsafe { CGEventGetLocation(event) };
    let (nx, ny, _, _) = telemetry.normalize_point(loc.x, loc.y);

    // Scrawl's own controls (the stop button, the HUD) aren't part of what is being shown.
    let on_scrawl = || {
        let target = unsafe { CGEventGetIntegerValueField(event, EVENT_TARGET_UNIX_PROCESS_ID) };
        target == i64::from(std::process::id())
    };
    match event_type {
        EVENT_LEFT_MOUSE_DOWN | EVENT_RIGHT_MOUSE_DOWN => {
            if pure_shift || on_scrawl() {
                return event;
            }
            telemetry.pointer_down(t_ms, loc.x, loc.y);
        }
        EVENT_LEFT_MOUSE_UP | EVENT_RIGHT_MOUSE_UP => {
            if pure_shift {
                return event;
            }
            telemetry.pointer_up(t_ms, loc.x, loc.y, event_type == EVENT_RIGHT_MOUSE_UP);
        }
        EVENT_SCROLL_WHEEL if on_scrawl() => {}
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
            if cmd && (keycode == layout_z_keycode() || keycode == KEY_Z) {
                let _ = telemetry.ui_sender.try_send(LiveOverlayEvent::UndoStroke);
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
                    telemetry.window_log_request.store(true, Ordering::Release);
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

pub fn current_mouse_screen() -> Option<[f32; 2]> {
    current_mouse_location().map(|p| [p.x as f32, p.y as f32])
}

pub fn wait_for_modifiers_released(max_wait_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(max_wait_ms);
    while Instant::now() < deadline {
        let flags = unsafe { CGEventSourceFlagsState(0) | CGEventSourceFlagsState(1) };
        let any_mod = (flags & (FLAG_COMMAND | FLAG_CONTROL | FLAG_ALTERNATE | FLAG_SHIFT)) != 0;
        if !any_mod {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn post_synthetic_cmd_v() {
    let v_code = layout_v_keycode();
    unsafe {
        let down = CGEventCreateKeyboardEvent(ptr::null(), v_code, true);
        if !down.is_null() {
            CGEventSetFlags(down, FLAG_COMMAND);
            CGEventPost(0, down);
            CFRelease(down.cast());
        }
        thread::sleep(Duration::from_millis(12));
        let up = CGEventCreateKeyboardEvent(ptr::null(), v_code, false);
        if !up.is_null() {
            CGEventSetFlags(up, FLAG_COMMAND);
            CGEventPost(0, up);
            CFRelease(up.cast());
        }
    }
}

pub fn post_synthetic_click(screen_x: f64, screen_y: f64) {
    let pt = CGPoint {
        x: screen_x,
        y: screen_y,
    };
    unsafe {
        let down = CGEventCreateMouseEvent(ptr::null(), EVENT_LEFT_MOUSE_DOWN, pt, 0);
        if !down.is_null() {
            CGEventPost(0, down);
            CFRelease(down.cast());
        }
        thread::sleep(Duration::from_millis(14));
        let up = CGEventCreateMouseEvent(ptr::null(), EVENT_LEFT_MOUSE_UP, pt, 0);
        if !up.is_null() {
            CGEventPost(0, up);
            CFRelease(up.cast());
        }
    }
}

/// 120Hz global HID & session state poller so shortcuts, Shift-laser (`⇧` + move), and
/// Shift-drag ink (`⇧` + drag) work across all apps (including Karabiner virtual HID).
fn run_global_hid_poller(telemetry: Arc<SharedCaptureTelemetry>) {
    let mut prev_shortcut = false;
    let mut prev_paste = false;
    let mut prev_shift_ink = false;
    let mut prev_undo = false;
    let mut await_shift_release = false;
    let mut prev_mod_mask = (0_u8, false, false);
    let mut prev_buttons = [false; 2];
    let mut last_tap_check = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(8));
        if last_tap_check.elapsed() >= Duration::from_millis(500) {
            telemetry.check_tap();
            last_tap_check = Instant::now();
        }

        // Combine COMBINED_SESSION_STATE (0, includes Karabiner virtual HID) and HID_SYSTEM_STATE (1).
        let flags = unsafe { CGEventSourceFlagsState(0) | CGEventSourceFlagsState(1) };
        let cmd = (flags & FLAG_COMMAND) != 0;
        let ctrl = (flags & FLAG_CONTROL) != 0;
        let alt = (flags & FLAG_ALTERNATE) != 0;
        let shift = (flags & FLAG_SHIFT) != 0;
        if !shift {
            await_shift_release = false;
        } else if cmd || ctrl || alt {
            await_shift_release = true;
        }
        let mod_mask =
            (u8::from(cmd) << 3) | (u8::from(ctrl) << 2) | (u8::from(alt) << 1) | u8::from(shift);

        let z_code = layout_z_keycode();
        let (record_cfg, paste_cfg) = telemetry.shortcuts();
        let key_down =
            |code: u16| unsafe { CGEventSourceKeyState(1, code) || CGEventSourceKeyState(0, code) };
        // Whether `kind`'s shortcut is down: its own key, or for a default its alias keys.
        let shortcut_down = |kind: ShortcutKind, cfg: &ShortcutConfig| {
            let mut codes = vec![cfg.key_code as u16];
            if cfg.is_default_for(kind) {
                codes.extend([layout_default_keycode(kind), kind.ansi_code()]);
            }
            codes.into_iter().any(|code| {
                key_down(code) && shortcut_matches(kind, cfg, code, code, cmd, ctrl, alt, shift)
            })
        };
        let shortcut_now = shortcut_down(ShortcutKind::Record, &record_cfg);
        let paste_now = shortcut_down(ShortcutKind::Paste, &paste_cfg);
        let key_state = (mod_mask, shortcut_now, paste_now);
        if key_state != prev_mod_mask {
            scrawl_log(format!(
                "hid_poller: cmd={cmd} ctrl={ctrl} alt={alt} shift={shift} record={shortcut_now} paste={paste_now}"
            ));
            prev_mod_mask = key_state;
        }
        if shortcut_now && !prev_shortcut {
            telemetry.emit_capture_shortcut();
        }
        prev_shortcut = shortcut_now;

        if paste_now && !prev_paste && !telemetry.active.load(Ordering::Acquire) {
            telemetry.emit_paste_shortcut();
        }
        prev_paste = paste_now;

        let buttons = unsafe {
            [
                CGEventSourceButtonState(1, 0) || CGEventSourceButtonState(0, 0),
                CGEventSourceButtonState(1, 1) || CGEventSourceButtonState(0, 1),
            ]
        };
        if !telemetry.active.load(Ordering::Acquire) {
            prev_shift_ink = false;
            prev_undo = false;
            prev_buttons = buttons;
            continue;
        }

        let key_z = unsafe {
            CGEventSourceKeyState(1, z_code)
                || CGEventSourceKeyState(0, z_code)
                || CGEventSourceKeyState(1, KEY_Z)
        };
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

        poll_clicks(
            &telemetry,
            &mut prev_buttons,
            buttons,
            (loc.x, loc.y),
            shift && !cmd && !ctrl && !alt,
            |x, y| telemetry.on_scrawl_window(x, y),
        );

        let pure_shift = shift && !cmd && !ctrl && !alt && !await_shift_release;
        // Hold Shift + move cursor inside the capture target = draw a persistent stroke.
        track_shift_ink(&telemetry, (loc.x, loc.y), pure_shift, prev_shift_ink);

        prev_shift_ink = pure_shift;
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
