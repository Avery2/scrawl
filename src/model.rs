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
    /// Where the pictures came from when it isn't this Mac's screen (`"iphone"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceInfo>,
}

/// The phone behind an `"iphone"` bundle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    /// `"mirror"` for a cabled capture through Scrawl, `"tetherkit"` for an in-app SDK recording.
    pub capture: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Native video size in pixels.
    pub width: u32,
    pub height: u32,
    /// Screen size in points and its scale, when the recording says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
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
    /// A phone's own screen; normalized coordinates are phone-screen coordinates.
    Device,
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
    /// Word times were spread over detected speech rather than measured by the recognizer,
    /// so they can't place a spoken cue precisely.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
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
    /// Moves the stroke `ms` earlier onto a clock that started at `ms`, shifting point `dt_ms`
    /// by any clipped prefix and dropping points before `ms`. Returns `false` if the stroke
    /// ended before `ms` or has no remaining points.
    pub fn rebase(&mut self, ms: u64) -> bool {
        if self.t1 < ms {
            self.pts.clear();
            return false;
        }
        let lost = ms.saturating_sub(self.t0) as i64;
        self.t0 = self.t0.saturating_sub(ms);
        self.t1 = self.t1.saturating_sub(ms);
        if lost > 0 {
            let mut kept = Vec::with_capacity(self.pts.len());
            for chunk in self.pts.chunks_exact(3) {
                let dt = i64::from(chunk[2]) - lost;
                if dt >= 0 {
                    kept.extend_from_slice(&[chunk[0], chunk[1], dt as i32]);
                }
            }
            self.pts = kept;
        }
        !self.pts.is_empty()
    }

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
    /// A time-only "look here" marker (M in the mirror, or a TetherKit marker).
    Flag,
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
        /// `"touch"` for a finger on a phone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<String>,
        /// Intermediate positions, for drawing the trail.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        path: Vec<[f32; 2]>,
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
    /// Something the recorded app reported about itself (TetherKit `events[]`).
    AppEvent {
        t: u64,
        label: String,
    },
}

impl InteractionEvent {
    /// Moves the event `ms` earlier onto a clock that started at `ms`. Returns `false` if the
    /// event ended before `ms` and should be dropped.
    pub fn rebase(&mut self, ms: u64) -> bool {
        match self {
            Self::Drag { t, t1, .. } => {
                if *t1 < ms {
                    return false;
                }
                *t = t.saturating_sub(ms);
                *t1 -= ms;
                true
            }
            Self::Click { t, .. }
            | Self::Scroll { t, .. }
            | Self::Point { t, .. }
            | Self::Key { t, .. }
            | Self::AppChange { t, .. }
            | Self::AppEvent { t, .. } => {
                if *t < ms {
                    return false;
                }
                *t -= ms;
                true
            }
        }
    }

    pub fn timestamp_ms(&self) -> u64 {
        match self {
            Self::Click { t, .. }
            | Self::Drag { t, .. }
            | Self::Scroll { t, .. }
            | Self::Point { t, .. }
            | Self::Key { t, .. }
            | Self::AppChange { t, .. }
            | Self::AppEvent { t, .. } => *t,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stroke_and_event_rebase_shift_dt_and_drop_pre_offset_items() {
        let mut before = Stroke {
            t0: 100,
            t1: 400,
            fw: 100,
            fh: 100,
            on: None,
            pin: None,
            laser: false,
            pts: vec![10, 20, 0, 30, 40, 300],
        };
        assert!(!before.rebase(500));

        let mut spanning = Stroke {
            t0: 300,
            t1: 700,
            fw: 100,
            fh: 100,
            on: None,
            pin: None,
            laser: false,
            pts: vec![10, 20, 0, 20, 30, 150, 30, 40, 200, 50, 60, 400],
        };
        assert!(spanning.rebase(500));
        assert_eq!(spanning.t0, 0);
        assert_eq!(spanning.t1, 200);
        assert_eq!(spanning.pts, vec![30, 40, 0, 50, 60, 200]);

        let mut after = Stroke {
            t0: 800,
            t1: 1_000,
            fw: 100,
            fh: 100,
            on: None,
            pin: None,
            laser: false,
            pts: vec![10, 20, 0, 50, 60, 200],
        };
        assert!(after.rebase(500));
        assert_eq!(after.t0, 300);
        assert_eq!(after.t1, 500);
        assert_eq!(after.pts, vec![10, 20, 0, 50, 60, 200]);

        let mut early_click = InteractionEvent::Click {
            t: 450,
            x: 0.5,
            y: 0.5,
            button: None,
            label: None,
            role: None,
        };
        assert!(!early_click.rebase(500));

        let mut spanning_drag = InteractionEvent::Drag {
            t: 400,
            t1: 650,
            x0: 0.2,
            y0: 0.2,
            x1: 0.8,
            y1: 0.8,
            label: None,
            button: None,
            path: Vec::new(),
        };
        assert!(spanning_drag.rebase(500));
        assert_eq!(spanning_drag.timestamp_ms(), 0);
        if let InteractionEvent::Drag { t, t1, .. } = spanning_drag {
            assert_eq!((t, t1), (0, 150));
        }
    }
}
