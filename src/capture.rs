use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
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

const MAX_RETAINED_FRAMES: usize = 300;
const FRAME_SAMPLE_INTERVAL: Duration = Duration::from_millis(220);

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
const SCROLL_WHEEL_DELTA_AXIS_1: u32 = 11;
const SCROLL_WHEEL_DELTA_AXIS_2: u32 = 12;

const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_CONTROL: u64 = 1 << 18;
const FLAG_ALTERNATE: u64 = 1 << 19;
const FLAG_COMMAND: u64 = 1 << 20;

const KEY_S: u16 = 1;
const KEY_Z: u16 = 6;
const KEY_V: u16 = 9;

// Carbon HIToolbox global hotkey constants (requires zero TCC permissions).
pub const CARBON_CMD_KEY: u32 = 1 << 8;
pub const CARBON_SHIFT_KEY: u32 = 1 << 9;
pub const CARBON_OPTION_KEY: u32 = 1 << 11;
pub const CARBON_CONTROL_KEY: u32 = 1 << 12;
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
    fn CFRelease(value: CfTypeRef);
    static kCFRunLoopCommonModes: *const c_void;
}

/// Live UI events emitted by the global input monitor to drive Scrawl's HUD & Review windows.
#[derive(Clone, Debug)]
pub enum LiveOverlayEvent {
    ToggleCaptureShortcut,
    PasteLatestShortcut,
    UndoStroke,
    CancelCapture,
    RequestSnapshot,
    /// The iPhone mirror ended on its own (unplugged, never found, or access denied).
    PhoneEnded,
    /// A ScrawlKit recording was ingested from an iPhone and compiled into a `.scrawl` bundle.
    IngestedBundle {
        dir: PathBuf,
        bundle: Box<crate::model::ScrawlBundle>,
        status: Option<String>,
    },
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
            label: "⌥⇧S".into(),
        }
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
        if cfg.label == "⌃⌥⇧S" {
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

struct CarbonHotKeyState {
    toggle_ref: usize,
}

static CARBON_HOTKEYS: std::sync::OnceLock<Mutex<CarbonHotKeyState>> = std::sync::OnceLock::new();

pub fn scrawl_log(msg: impl AsRef<str>) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let line = format!("[{ts}] {}\n", msg.as_ref());
    eprint!("{line}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/private/var/folders/dd/5fz89drs5p9_r0fk7rwqqnbr0000gn/T/opencode/scrawl-debug.log")
    {
        let _ = f.write_all(line.as_bytes());
    }
}

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
                telemetry.emit_toggle_shortcut();
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
            let target = if !app_target.is_null() {
                app_target
            } else {
                dispatcher
            };
            let mut next_alt_id = HOTKEY_ID_ALT1;
            let mut codes = vec![s_code];
            if s_code != u32::from(KEY_S) {
                codes.push(u32::from(KEY_S));
            }
            for &code in &codes {
                for (mods, name) in [
                    (CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "⌥⇧S"),
                    (
                        CARBON_CONTROL_KEY | CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
                        "⌃⌥⇧S",
                    ),
                    (
                        CARBON_CMD_KEY | CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
                        "⌘⌥⇧S",
                    ),
                    (
                        CARBON_CONTROL_KEY | CARBON_OPTION_KEY | CARBON_CMD_KEY,
                        "⌃⌥⌘S",
                    ),
                ] {
                    let mut r: *mut c_void = ptr::null_mut();
                    let rc = RegisterEventHotKey(
                        code,
                        mods,
                        EventHotKeyID {
                            signature: HOTKEY_SIG,
                            id: next_alt_id,
                        },
                        target,
                        0,
                        &raw mut r,
                    );
                    scrawl_log(format!(
                        "RegisterEventHotKey({name}, code={code}, mods={mods}) -> rc={rc} ref={r:p}"
                    ));
                    next_alt_id += 1;
                }
            }
            // Register ⌥⇧V (plus ⌃⌥⇧V, ⌘⌥⇧V, ⌃⌥⌘V) for pasting both Image + Text of the latest capture.
            let v_code = u32::from(layout_v_keycode());
            let mut v_codes = vec![v_code];
            if v_code != u32::from(KEY_V) {
                v_codes.push(u32::from(KEY_V));
            }
            let mut paste_id = HOTKEY_ID_PASTE_BASE;
            for &code in &v_codes {
                for (mods, name) in [
                    (CARBON_OPTION_KEY | CARBON_SHIFT_KEY, "⌥⇧V"),
                    (
                        CARBON_CONTROL_KEY | CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
                        "⌃⌥⇧V",
                    ),
                    (
                        CARBON_CMD_KEY | CARBON_OPTION_KEY | CARBON_SHIFT_KEY,
                        "⌘⌥⇧V",
                    ),
                    (
                        CARBON_CONTROL_KEY | CARBON_OPTION_KEY | CARBON_CMD_KEY,
                        "⌃⌥⌘V",
                    ),
                ] {
                    let mut r: *mut c_void = ptr::null_mut();
                    let rc = RegisterEventHotKey(
                        code,
                        mods,
                        EventHotKeyID {
                            signature: HOTKEY_SIG,
                            id: paste_id,
                        },
                        target,
                        0,
                        &raw mut r,
                    );
                    scrawl_log(format!(
                        "RegisterEventHotKey({name}, code={code}, mods={mods}) -> rc={rc} ref={r:p}"
                    ));
                    paste_id += 1;
                }
            }
        }
        Mutex::new(CarbonHotKeyState { toggle_ref: 0 })
    });
    let shortcut = telemetry
        .shortcut
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let _ = state_mutex;
    update_global_shortcut(telemetry, shortcut);
}

pub fn update_global_shortcut(telemetry: &SharedCaptureTelemetry, config: ShortcutConfig) {
    config.save();
    *telemetry
        .shortcut
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = config.clone();

    let Some(state_mutex) = CARBON_HOTKEYS.get() else {
        return;
    };
    let mut state = state_mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.toggle_ref != 0 {
        unsafe { UnregisterEventHotKey(state.toggle_ref as *mut c_void) };
        state.toggle_ref = 0;
    }
    let mut new_ref: *mut c_void = ptr::null_mut();
    let target = unsafe {
        let a = GetApplicationEventTarget();
        if !a.is_null() {
            a
        } else {
            GetEventDispatcherTarget()
        }
    };
    let status = unsafe {
        RegisterEventHotKey(
            config.key_code,
            config.carbon_modifiers,
            EventHotKeyID {
                signature: HOTKEY_SIG,
                id: HOTKEY_ID_TOGGLE,
            },
            target,
            0,
            &raw mut new_ref,
        )
    };
    scrawl_log(format!(
        "update_global_shortcut({}, key={}, mods={}) -> status={status} ref={new_ref:p}",
        config.label, config.key_code, config.carbon_modifiers
    ));
    if status == 0 && !new_ref.is_null() {
        state.toggle_ref = new_ref as usize;
    }
}

/// No-op: do not register a global Escape hotkey because Karabiner maps CapsLock (Control)
/// with `to_if_alone: escape`, which would cancel recordings when releasing Control.
pub fn set_escape_hotkey_enabled(_enabled: bool) {}

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

pub fn selectable_window_at(windows: &[TargetInfo], x: f64, y: f64) -> Option<&TargetInfo> {
    windows.iter().find(|window| {
        let f = window.frame;
        x >= f.x && x <= f.x + f.width && y >= f.y && y <= f.y + f.height
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_stop_joins_worker_before_collecting_its_final_media() {
        let stopping = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(Mutex::new(VecDeque::new()));
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
                .push_back(RgbaFrame::new(42, 1, 1, vec![1, 2, 3, 255]));
            std::fs::write(&worker_path, b"finalized media").unwrap();
            Some(worker_path)
        });
        let session = ScreenCaptureSession {
            started_at: Instant::now(),
            stopping,
            urgent_snapshot: Arc::new(AtomicBool::new(false)),
            frames,
            sampler_thread: Some(worker),
        };
        let (frames, video, _) = session.stop();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].t_ms, 42);
        assert_eq!(frames[0].rgba, vec![1, 2, 3, 255]);
        assert_eq!(video.as_ref(), Some(&path));
        std::fs::remove_file(path).unwrap();
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
    let display_id = unsafe { CGMainDisplayID() };
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

fn frontmost_app_info() -> Option<(i32, String, Option<String>)> {
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

/// Active screen + optional MP4 recorder using `ScreenCaptureKit`.
pub struct ScreenCaptureSession {
    started_at: Instant,
    stopping: Arc<AtomicBool>,
    urgent_snapshot: Arc<AtomicBool>,
    frames: Arc<Mutex<VecDeque<RgbaFrame>>>,
    sampler_thread: Option<JoinHandle<Option<PathBuf>>>,
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

        let thread_stopping = Arc::clone(&stopping);
        let thread_urgent = Arc::clone(&urgent_snapshot);
        let thread_frames = Arc::clone(&frames);
        let sampler_target = target;
        let sampler_thread = thread::Builder::new()
            .name("scrawl-frame-sampler".into())
            .spawn(move || {
                let initial_excluded = excluded_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                // Keep SCK initialization, capture and teardown on the owning worker.
                // stop() joins even if it arrives while setup is still in progress.
                let (stream, recording_output, video_path, mut filter_and_config) =
                    setup_sck_capture(&sampler_target, video_output_path, &initial_excluded);
                crate::ui::start_metric("sck_ready_ms", started_at);
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
                if let Some(stream) = stream {
                    let _ = stream.stop_capture();
                    if let Some(ref output) = recording_output {
                        let _ = stream.remove_recording_output(output);
                    }
                }
                video_path
            })
            .ok();

        Self {
            started_at,
            stopping,
            urgent_snapshot,
            frames,
            sampler_thread,
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
        let video_path = self
            .sampler_thread
            .take()
            .and_then(|handle| handle.join().ok())
            .flatten();
        let mut collected: Vec<RgbaFrame> = self
            .frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        if collected.is_empty() {
            collected.push(crate::render::render_synthetic_demo_frame(1280, 800, 0, 0));
        }
        let valid_video =
            video_path.filter(|path| path.exists() && path.metadata().is_ok_and(|m| m.len() > 0));
        (collected, valid_video, duration_ms.max(100))
    }
}

impl Drop for ScreenCaptureSession {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(handle) = self.sampler_thread.take() {
            let _ = handle.join();
        }
    }
}

fn build_sck_filter_and_config(
    target: &TargetInfo,
    excluded_window_ids: &[u32],
) -> Option<(SCContentFilter, SCStreamConfiguration)> {
    let content = match SCShareableContent::get() {
        Ok(c) => c,
        Err(err) => {
            scrawl_log(format!("SCShareableContent::get() failed: {err:?}"));
            unsafe {
                if !CGPreflightScreenCaptureAccess() {
                    scrawl_log(
                        "Screen capture access not granted; calling CGRequestScreenCaptureAccess()",
                    );
                    let _ = CGRequestScreenCaptureAccess();
                }
            }
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

    let max_dim = target.frame.width.max(target.frame.height).max(1.0);
    // Full-screen captures get auto-cropped to a window or region afterwards (`scrawl autocrop`),
    // so they need enough pixels for a crop to a quarter of the screen to stay sharp.
    let long_edge = if target.mode == CaptureMode::Display { 2560.0 } else { 1280.0 };
    let scale = (long_edge / max_dim).min(2.0);
    let capture_width = ((target.frame.width * scale).round() as u32).max(48) & !1;
    let capture_height = ((target.frame.height * scale).round() as u32).max(48) & !1;

    let mut config = SCStreamConfiguration::new()
        .with_width(capture_width)
        .with_height(capture_height)
        .with_shows_cursor(true);
    if target.mode == CaptureMode::Region {
        config.set_source_rect(CGRect::new(
            target.frame.x,
            target.frame.y,
            target.frame.width.max(32.0),
            target.frame.height.max(32.0),
        ));
    }
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
    tap_port: AtomicU64,
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
            tap_port: AtomicU64::new(0),
            last_shortcut_ms: AtomicU64::new(0),
            drag_start: Mutex::new(None),
        })
    }

    pub fn emit_toggle_shortcut(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let prev = self.last_shortcut_ms.load(Ordering::Acquire);
        if now_ms.saturating_sub(prev) >= 280
            && self
                .last_shortcut_ms
                .compare_exchange(prev, now_ms, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            scrawl_log("emit_toggle_shortcut -> sending ToggleCaptureShortcut");
            let _ = self
                .ui_sender
                .try_send(LiveOverlayEvent::ToggleCaptureShortcut);
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
        let want_cmd = (cfg.carbon_modifiers & CARBON_CMD_KEY) != 0;
        let want_ctrl = (cfg.carbon_modifiers & CARBON_CONTROL_KEY) != 0;
        let want_alt = (cfg.carbon_modifiers & CARBON_OPTION_KEY) != 0;
        let want_shift = (cfg.carbon_modifiers & CARBON_SHIFT_KEY) != 0;
        let custom_match = u32::from(keycode) == cfg.key_code
            && cmd == want_cmd
            && ctrl == want_ctrl
            && alt == want_alt
            && shift == want_shift;
        // Support ⌥⇧S, ⌃⌥⇧S, ⌘⌥⇧S, and ⌃⌥⌘S on both the active layout's 's' key and ANSI 's'.
        let s_code = layout_s_keycode();
        let is_s_key = keycode == s_code || keycode == KEY_S;
        let builtin_s = is_s_key && alt && (shift || (ctrl && cmd));
        custom_match || builtin_s
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
                | (1_u64 << EVENT_KEY_DOWN)
                | (1_u64 << EVENT_SCROLL_WHEEL);

            let user_info = Arc::into_raw(Arc::clone(&telemetry))
                .cast_mut()
                .cast::<c_void>();
            // tap = 2 (kCGAnnotatedSessionEventTap), options = 1 (kCGEventTapOptionListenOnly)
            let tap = unsafe { CGEventTapCreate(2, 0, 1, mask, tap_callback, user_info) };
            scrawl_log(format!("CGEventTapCreate -> tap={tap:p}"));
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

fn same_target_frame(a: TargetFrame, b: TargetFrame) -> bool {
    (a.x - b.x).abs() <= 0.5
        && (a.y - b.y).abs() <= 0.5
        && (a.width - b.width).abs() <= 0.5
        && (a.height - b.height).abs() <= 0.5
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
            telemetry.emit_toggle_shortcut();
            return event;
        }
    }

    if !telemetry.active.load(Ordering::Acquire) {
        return event;
    }

    let t_ms = telemetry.elapsed_ms();
    let loc = unsafe { CGEventGetLocation(event) };
    let (nx, ny, fw, fh) = telemetry.normalize_point(loc.x, loc.y);

    match event_type {
        EVENT_LEFT_MOUSE_DOWN | EVENT_RIGHT_MOUSE_DOWN => {
            if pure_shift || !telemetry.contains_screen_point(loc.x, loc.y) {
                return event;
            }
            telemetry.window_log_request.store(true, Ordering::Release);
            let start_frame = *telemetry
                .target_frame
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *telemetry
                .drag_start
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((t_ms, nx, ny, None, start_frame));
        }
        EVENT_LEFT_MOUSE_UP | EVENT_RIGHT_MOUSE_UP => {
            if pure_shift {
                return event;
            }
            let Some((t0, x0, y0, start_label, start_frame)) = telemetry
                .drag_start
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            else {
                return event;
            };
            let end_frame = *telemetry
                .target_frame
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !same_target_frame(start_frame, end_frame) {
                return event;
            }
            let dist_px = ((nx - x0) * fw as f32).hypot((ny - y0) * fh as f32);
            if dist_px < 12.0 {
                let _ = telemetry
                    .ui_sender
                    .try_send(LiveOverlayEvent::RequestSnapshot);
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
                        label: start_label,
                        role: None,
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
                        button: None,
                        path: Vec::new(),
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
    ]
    .iter()
    .any(|needle| a.contains(needle) || b.contains(needle))
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
    loop {
        thread::sleep(Duration::from_millis(8));

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

        let s_code = layout_s_keycode();
        let z_code = layout_z_keycode();
        let v_code = layout_v_keycode();
        let cfg_keycode = telemetry
            .shortcut
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .key_code as u16;
        let key_s = unsafe {
            CGEventSourceKeyState(1, s_code)
                || CGEventSourceKeyState(0, s_code)
                || CGEventSourceKeyState(1, KEY_S)
        };
        let key_v = unsafe {
            CGEventSourceKeyState(1, v_code)
                || CGEventSourceKeyState(0, v_code)
                || CGEventSourceKeyState(1, KEY_V)
        };
        let key_cfg = if cfg_keycode == s_code {
            key_s
        } else {
            unsafe {
                CGEventSourceKeyState(1, cfg_keycode) || CGEventSourceKeyState(0, cfg_keycode)
            }
        };
        let key_state = (mod_mask, key_s, key_cfg);
        if key_state != prev_mod_mask {
            scrawl_log(format!(
                "hid_poller: cmd={cmd} ctrl={ctrl} alt={alt} shift={shift} key_s={key_s} key_cfg={key_cfg}"
            ));
            prev_mod_mask = key_state;
        }
        let shortcut_now = (key_cfg
            && telemetry.matches_shortcut(cfg_keycode, cmd, ctrl, alt, shift))
            || (key_s && telemetry.matches_shortcut(s_code, cmd, ctrl, alt, shift));
        if shortcut_now && !prev_shortcut {
            telemetry.emit_toggle_shortcut();
        }
        prev_shortcut = shortcut_now;

        let paste_now = key_v && alt && (shift || (ctrl && cmd));
        if paste_now && !prev_paste && !telemetry.active.load(Ordering::Acquire) {
            telemetry.emit_paste_shortcut();
        }
        prev_paste = paste_now;

        if !telemetry.active.load(Ordering::Acquire) {
            prev_shift_ink = false;
            prev_undo = false;
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

        let inside_target = telemetry.contains_screen_point(loc.x, loc.y);
        let pure_shift = shift && !cmd && !ctrl && !alt && !await_shift_release;

        // Hold Shift + move cursor inside the capture target = draw a persistent stroke.
        if pure_shift && inside_target {
            telemetry.shift_held.store(true, Ordering::Release);
            let t_ms = telemetry.elapsed_ms();
            let (nx, ny, fw, fh) = telemetry.normalize_point(loc.x, loc.y);
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

    // Two frame workers plus the calling thread keep CPU and encoder memory bounded.
    thread::scope(|scope| -> Result<()> {
        let workers: Vec<_> = annotated_frames
            .chunks(annotated_frames.len().div_ceil(2).max(1))
            .map(|frames| {
                scope.spawn(move || -> Result<()> {
                    for (kf, frame) in frames {
                        crate::render::save_png(frame, &out_dir.join(&kf.file))?;
                    }
                    Ok(())
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
