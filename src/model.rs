use serde::{Deserialize, Serialize};

/// Coordinates in `Stroke::pts` are scaled to `0..=10_000` (1/10000 of target width/height).
pub const SCALE: i32 = 10_000;

/// Pause longer than this (ms) between strokes starts a new `Mark`.
pub const MARK_GAP_MS: u64 = 1_200;

/// Maximum strokes grouped into a single `Mark`.
pub const MAX_MARK_STROKES: usize = 6;

/// How long a laser trail remains visible on screen (ms).
pub const LASER_MS: u64 = 420;

/// Maximum keyframes selected for a contact sheet.
pub const MAX_KEYFRAMES: usize = 6;

/// Formats milliseconds as `M:SS` (e.g. `0:04`, `1:23`).
pub fn format_clock(ms: u64) -> String {
    let total_seconds = ms / 1_000;
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

/// Parses a `M:SS` or `SS` or `SS.mmm` string into milliseconds.
pub fn parse_clock_ms(input: &str) -> Option<u64> {
    let trimmed = input.trim();
    if let Some((mins, secs)) = trimmed.split_once(':') {
        let m: u64 = mins.parse().ok()?;
        let s: f64 = secs.parse().ok()?;
        if !s.is_finite() || s < 0.0 {
            return None;
        }
        Some(m * 60_000 + (s * 1_000.0).round() as u64)
    } else if let Some(ms_str) = trimmed.strip_suffix("ms") {
        ms_str.trim().parse().ok()
    } else {
        let s: f64 = trimmed.strip_suffix('s').unwrap_or(trimmed).parse().ok()?;
        if !s.is_finite() || s < 0.0 {
            return None;
        }
        Some((s * 1_000.0).round() as u64)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScrawlBundle {
    pub version: u32,
    pub id: String,
    pub created_at_ms: u64,
    pub duration_ms: u64,
    pub target: TargetInfo,
    pub transcript: TranscriptData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub strokes: Vec<Stroke>,
    pub marks: Vec<Mark>,
    pub events: Vec<InteractionEvent>,
    pub keyframes: Vec<Keyframe>,
    pub contact_sheet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub mode: CaptureMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<u32>,
    pub display_id: u32,
    pub frame: TargetFrame,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Display,
    Window,
    Region,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub scale: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptData {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub text: String,
    pub segments: Vec<TranscriptSegment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordTiming>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub t0: u64,
    pub t1: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WordTiming {
    pub t0: u64,
    pub t1: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub t0: u64,
    pub t1: u64,
    pub fw: u32,
    pub fh: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub laser: bool,
    /// Flat triples `[x, y, dt_ms, ...]` where `x, y` are in `0..=SCALE` and `dt_ms` is ms since `t0`.
    pub pts: Vec<i32>,
}

impl Stroke {
    /// Returns normalized `(x, y)` points in `0.0..=1.0`.
    pub fn normalized_points(&self) -> Vec<[f32; 2]> {
        self.pts
            .chunks_exact(3)
            .map(|c| [c[0] as f32 / SCALE as f32, c[1] as f32 / SCALE as f32])
            .collect()
    }

    /// Returns `(x, y, absolute_t_ms)` points.
    pub fn timed_points(&self) -> Vec<(f32, f32, u64)> {
        self.pts
            .chunks_exact(3)
            .map(|c| {
                (
                    c[0] as f32 / SCALE as f32,
                    c[1] as f32 / SCALE as f32,
                    self.t0.saturating_add(c[2].max(0) as u64),
                )
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Circle,
    Underline,
    Line,
    Arrow,
    Tap,
    Pin,
    Mark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub id: String,
    pub strokes: Vec<usize>,
    pub t0: u64,
    pub t1: u64,
    /// Normalized bounding box `[x0, y0, x1, y1]` in `0.0..=1.0`.
    pub box_norm: [f32; 4],
    pub shape: Shape,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionEvent {
    Click {
        t: u64,
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },
    Drag {
        t: u64,
        t1: u64,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Scroll {
        t: u64,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Point {
        t: u64,
        x: f32,
        y: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    Key {
        t: u64,
        shortcut: String,
    },
    AppChange {
        t: u64,
        app: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_title: Option<String>,
    },
}

impl InteractionEvent {
    pub fn timestamp_ms(&self) -> u64 {
        match self {
            Self::Click { t, .. }
            | Self::Drag { t, .. }
            | Self::Scroll { t, .. }
            | Self::Point { t, .. }
            | Self::Key { t, .. }
            | Self::AppChange { t, .. } => *t,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyframeReason {
    Mark,
    Click,
    Laser,
    SceneChange,
    Speech,
    Initial,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    pub index: usize,
    pub t: u64,
    pub reason: KeyframeReason,
    pub file: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<[f32; 2]>,
}

/// Raw RGBA8 frame buffer in memory used for candidate selection and compositing.
#[derive(Clone, Debug)]
pub struct RgbaFrame {
    pub t_ms: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaFrame {
    pub fn new(t_ms: u64, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        debug_assert_eq!(rgba.len(), (width as usize) * (height as usize) * 4);
        Self {
            t_ms,
            width,
            height,
            rgba,
        }
    }
}
