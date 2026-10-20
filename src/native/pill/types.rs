//! The native pill's values shared with the coordinator, on every platform.

use super::model::Pres;
use crate::ui::{SelectionMode, motion};

/// Capsule visual style (`pillStyle: "black" | "glass"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PillStyle {
    #[default]
    Black,
    Glass,
}

#[allow(dead_code)]
impl PillStyle {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "glass" | "liquid" | "liquid-glass" | "1" | "true" => Some(Self::Glass),
            "black" | "dark" | "metal" | "0" | "false" => Some(Self::Black),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Glass => "glass",
        }
    }

    /// Resolves the active `PillStyle` from `SCRAWL_PILL_STYLE`, `tune.json`, or `config.json`,
    /// read at most twice a second: it's asked for on every capsule tick, and `config.json` is a
    /// file read and a JSON parse.
    pub fn current() -> Self {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, PillStyle)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, style)) = *cache
            && at.elapsed() < Duration::from_millis(500)
        {
            return style;
        }
        let style = Self::resolve();
        *cache = Some((Instant::now(), style));
        style
    }

    fn resolve() -> Self {
        if let Ok(env) = std::env::var("SCRAWL_PILL_STYLE")
            && let Some(style) = Self::from_str_opt(&env)
        {
            return style;
        }
        if let Some(style) = motion::tune().pill_style_override() {
            return style;
        }
        crate::handoff::pill_style()
    }
}

/// Which capsule draws (`pillRenderer: "metal" | "swiftui"`, `SCRAWL_PILL_RENDERER`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PillRenderer {
    #[default]
    Metal,
    SwiftUi,
}

#[allow(dead_code)]
impl PillRenderer {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "swiftui" | "swift" => Some(Self::SwiftUi),
            "metal" => Some(Self::Metal),
            _ => None,
        }
    }
}

/// User actions emitted by clicking or pressing keys on the native pill (macOS only, so
/// elsewhere nothing makes them).
#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum PillAction {
    SelectMode(SelectionMode),
    SwitchRecording(SelectionMode),
    RecordSelected,
    Undo,
    Cancel,
    Finish,
    Paste(Option<[f32; 2]>),
    Inspect,
    DismissCompleted,
}

/// Coordinator state snapshot synced to `NativePill`.
#[derive(Clone, Debug, PartialEq)]
pub struct PillSnapshot {
    pub pres: Pres,
    pub clock: String,
    pub reduced_motion: bool,
    pub style: PillStyle,
}
