#![allow(dead_code)]
//!
//! Ported from Kit Motion study 03 (`island/model.js` and `island/NATIVE.md`):
//! - Springs are parameterized by `spring(duration, bounce)` with `ω = 2π / duration` and
//!   `ζ = 1 − bounce`, integrated at fixed 480 Hz sub-steps so retargets preserve velocity (`C1`).
//! - Only the outline's width (`L`, `R`) bounces (`0.50 / 0.28` open, `0.45 / 0.18` close,
//!   `0.35 / 0.45` tap squish); the top edge `TOP` is anchored so it never bounces upward into the
//!   top of the screen, and content / opacity / shared travel use `bounce = 0`.
//! - Room gating (`CLEAR = 2 pt` + `EDGE_BAND = 3 pt` smoothstep), `0.10 s` look-ahead exit cap
//!   sampled at 240 Hz, `0.10 s` collapse wait, `0.50` growth gate, `0.30 s` content-in fade from
//!   `8 pt` blur and `0.82` scale with blur on its own critically damped clock at `0.55×` lead,
//!   and `0.013 s` / `1.75 pt` directional stagger.
//! - Persistent shared elements (`lead` dot + timer across recording states, and mode chips + `✕`
//!   across `Recording (chips) -> Selecting`) travel on a `0.24 s` zero-bounce spring with up to
//!   `4 pt` blur in transit that clears before landing.

use crate::ui::SelectionMode;

// ─── Geometry (pt in the fixed 640 × 84 panel) ──────────────────────────────────────────────

pub const PANEL_W: f32 = 640.0;
pub const PANEL_H: f32 = 84.0;
pub const PANEL_Y: f32 = 34.0;
pub const CX: f32 = PANEL_W * 0.5;
/// Top edge of the resting 40 pt capsule inside the 84 pt panel (vertical centre y = 42, screen y = 76).
pub const TOP: f32 = 22.0;
pub const ISLAND_H: f32 = 40.0;
pub const ISLAND_RADIUS: f32 = ISLAND_H * 0.5;
pub const ISLAND_INSET: f32 = 4.0;
pub const ISLAND_OPEN_FROM_W: f32 = 120.0;
pub const IDLE_W: f32 = 96.0;
pub const IDLE_H: f32 = 24.0;

pub const CHIP_H: f32 = ISLAND_H - 2.0 * ISLAND_INSET; // 32.0
pub const CHIP_RADIUS: f32 = CHIP_H * 0.5; // 16.0
pub const CHIP_PX: f32 = 12.0;
pub const CHIP_GAP: f32 = 2.0;
pub const DIVIDER_MX: f32 = 6.0;
pub const DIVIDER_W: f32 = 1.0 + 2.0 * DIVIDER_MX; // 13.0
pub const LEAD_TRAIL: f32 = 8.0;
pub const SEG_PX: f32 = 10.0;
pub const SEG_ICON: f32 = 14.0;
pub const SEG_ICON_GAP: f32 = 5.0;
pub const HUD_TEXT: f32 = 13.0;

/// Core cap radius of a 32 pt chip for clearance checks (3 pt inside the 16 pt chip pill and 7 pt
/// inside the 20 pt outer capsule wall at rest, so at rest `d = 7 pt >= CLEAR + EDGE_BAND = 5 pt`).
pub const CHIP_CORE_ROUND: f32 = 13.0;

// ─── Physics & Timing Constants ─────────────────────────────────────────────────────────────

pub const CLEAR: f32 = 2.0;
pub const EDGE_BAND: f32 = 3.0;
pub const EXIT_LEAD_S: f32 = 0.10;
pub const COLLAPSE_WAIT_S: f32 = 0.10;
pub const CONTENT_DELAY: f32 = 0.50;
pub const CONTENT_BLUR_MAX: f32 = 8.0;
pub const BLUR_LEAD: f32 = 0.55;
pub const CONTENT_SCALE_FROM: f32 = 0.82;
pub const CONTENT_STAGGER_S: f32 = 0.013;
pub const CONTENT_PUSH_PT: f32 = 1.75;
pub const TRAVEL_BLUR_MAX: f32 = 6.0;
pub const REDUCED_FADE_S: f32 = 0.20;
pub const RADIUS_BLEND: f32 = 8.0;
pub const PRESS_GROW_PT: f32 = 2.5;
pub const TAP_SQUISH: f32 = 0.04;
const REF_DIST: f32 = 140.0;
const DISTANCE_FLOOR: f32 = 0.82;
const AHEAD_STEP: f32 = 1.0 / 240.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringCfg {
    pub duration: f32,
    pub bounce: f32,
}

impl SpringCfg {
    pub const fn new(duration: f32, bounce: f32) -> Self {
        Self { duration, bounce }
    }
}

pub const OPEN_SPRING: SpringCfg = SpringCfg::new(0.50, 0.28);
pub const CLOSE_SPRING: SpringCfg = SpringCfg::new(0.45, 0.18);
pub const TAP_SPRING: SpringCfg = SpringCfg::new(0.35, 0.45);
pub const PRESS_SPRING: SpringCfg = SpringCfg::new(0.25, 0.0);
pub const SHARED_SPRING: SpringCfg = SpringCfg::new(0.30, 0.0);
pub const CONTENT_IN_SPRING: SpringCfg = SpringCfg::new(0.30, 0.0);
pub const CONTENT_OUT_SPRING: SpringCfg = SpringCfg::new(0.15, 0.0);

#[inline]
pub fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringVal {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    pub duration: f32,
    pub bounce: f32,
}

impl SpringVal {
    pub const fn new(x: f32, cfg: SpringCfg) -> Self {
        Self {
            x,
            v: 0.0,
            target: x,
            duration: cfg.duration,
            bounce: cfg.bounce,
        }
    }

    #[inline]
    pub fn to(&mut self, target: f32, cfg: SpringCfg) {
        self.target = target;
        self.duration = cfg.duration;
        self.bounce = cfg.bounce;
    }

    #[inline]
    pub fn snap(&mut self, x: f32) {
        self.x = x;
        self.target = x;
        self.v = 0.0;
    }

    #[inline]
    pub fn is_resting(&self) -> bool {
        (self.x - self.target).abs() < 1e-4 && self.v.abs() < 4e-4
    }

    /// SwiftUI's `spring(duration:bounce:)`, using the exact analytical closed-form solution
    /// so integration is frame-rate independent and retargets are C1 continuous.
    pub fn step(&mut self, dt: f32) -> bool {
        if dt <= 0.0 {
            return false;
        }
        if self.is_resting() {
            self.x = self.target;
            self.v = 0.0;
            return false;
        }
        let w = std::f32::consts::TAU / self.duration.max(0.02);
        let zeta = (1.0 - self.bounce).clamp(0.05, 1.0);
        let d0 = self.x - self.target;
        let v0 = self.v;
        if zeta >= 0.9999 {
            let e = (-w * dt).exp();
            let c1 = v0 + w * d0;
            let next_x = self.target + (d0 + c1 * dt) * e;
            let next_v = (v0 - c1 * w * dt) * e;
            if d0 * (next_x - self.target) < 0.0 {
                self.x = self.target;
                self.v = 0.0;
            } else {
                self.x = next_x;
                self.v = next_v;
            }
        } else {
            let wd = w * (1.0 - zeta * zeta).sqrt();
            let e = (-zeta * w * dt).exp();
            let (s, c) = (wd * dt).sin_cos();
            let b = (v0 + zeta * w * d0) / wd;
            self.x = self.target + e * (d0 * c + b * s);
            self.v = e * ((b * wd - zeta * w * d0) * c - (d0 * wd + zeta * w * b) * s);
        }
        if self.is_resting() {
            self.x = self.target;
            self.v = 0.0;
        }
        true
    }

    /// Critically damped 0-bounce step that never crosses `target` (`status-roll.js`).
    pub fn step_critical(&mut self, dt: f32, visual_duration: f32, _unit: f32) {
        self.duration = (visual_duration * 0.95).max(0.04);
        self.bounce = 0.0;
        self.step(dt);
    }
}

// ─── Silhouette & Clearance Math ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Silhouette {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
    pub radius: f32,
    pub alpha: f32,
    pub blur: f32,
}

impl Silhouette {
    #[inline]
    pub fn width(&self) -> f32 {
        (self.r - self.l).max(0.0)
    }

    #[inline]
    pub fn height(&self) -> f32 {
        (self.b - self.t).max(0.0)
    }

    #[inline]
    pub fn rect(&self) -> [f32; 4] {
        [self.l, self.t, self.width(), self.height()]
    }
}

/// Polynomial smooth-min of `h / 2` and `max_r` (`radiusOf` in `island/model.js`).
#[inline]
pub fn radius_of(h: f32, max_r: f32, blend: f32) -> f32 {
    let a = h * 0.5;
    let b = max_r;
    if blend <= 0.0 {
        return a.min(b);
    }
    let t = (blend - (a - b).abs()).max(0.0) / blend;
    a.min(b) - t * t * blend * 0.25
}

/// Exact signed distance from `(x, y)` to a silhouette's rounded rectangle; negative inside.
#[inline]
pub fn outline_sdf(sh: &Silhouette, x: f32, y: f32) -> f32 {
    let hw = (sh.r - sh.l) * 0.5;
    let hh = (sh.b - sh.t) * 0.5;
    let r = sh.radius.min(hw).min(hh);
    let qx = (x - (sh.l + sh.r) * 0.5).abs() - (hw - r);
    let qy = (y - (sh.t + sh.b) * 0.5).abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// How far a rounded box `[x, y, w, h]` (with corner radius `round`) sits inside `sh`, when
/// scaled by `k` about `origin` and shifted vertically by `dy`.
#[inline]
pub fn box_clearance(
    sh: &Silhouette,
    [x, y, w, h]: [f32; 4],
    k: f32,
    round: f32,
    [ox, oy]: [f32; 2],
    dy: f32,
) -> f32 {
    let cap = (w * 0.5).min(h * 0.5).max(0.0);
    let r = round.min(cap).max(0.0);
    let x0 = x + cap;
    let y0 = y + cap;
    let x1 = (x + w - cap).max(x0);
    let y1 = (y + h - cap).max(y0);
    let mut worst = f32::NEG_INFINITY;
    for cx in [x0, x1] {
        for cy in [y0, y1] {
            let px = ox + (cx - ox) * k;
            let py = oy + (cy - oy) * k + dy;
            worst = worst.max(outline_sdf(sh, px, py));
        }
    }
    -worst - r * k
}

#[inline]
pub fn room(d: f32) -> f32 {
    smooth(CLEAR, CLEAR + EDGE_BAND, d)
}

#[inline]
pub fn scale_of(a: f32, reduced: bool) -> f32 {
    if reduced {
        1.0
    } else {
        mix(CONTENT_SCALE_FROM, 1.0, a.clamp(0.0, 1.0))
    }
}

#[inline]
pub fn push_of(a: f32, reduced: bool) -> f32 {
    if reduced {
        0.0
    } else {
        -CONTENT_PUSH_PT * (1.0 - a.clamp(0.0, 1.0))
    }
}

// ─── Elements & Presentations ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum ElemId {
    Lead = 0,
    DividerLead = 1,
    ChipArea = 2,
    ChipWindow = 3,
    ChipScreen = 4,
    ChipIphone = 5,
    Record = 6,
    DividerClose = 7,
    Close = 8,
    Undo = 9,
    Cancel = 10,
    Finish = 11,
    Processing = 12,
    DoneLead = 13,
    Paste = 14,
    Inspect = 15,
}

impl ElemId {
    pub const COUNT: usize = 16;
    pub const ALL: [ElemId; Self::COUNT] = [
        Self::Lead,
        Self::DividerLead,
        Self::ChipArea,
        Self::ChipWindow,
        Self::ChipScreen,
        Self::ChipIphone,
        Self::Record,
        Self::DividerClose,
        Self::Close,
        Self::Undo,
        Self::Cancel,
        Self::Finish,
        Self::Processing,
        Self::DoneLead,
        Self::Paste,
        Self::Inspect,
    ];

    pub const fn trace_key(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::DividerLead => "divider.lead",
            Self::ChipArea => "chip:Area",
            Self::ChipWindow => "chip:Window",
            Self::ChipScreen => "chip:Screen",
            Self::ChipIphone => "chip:iPhone",
            Self::Record => "record",
            Self::DividerClose => "divider.close",
            Self::Close => "close",
            Self::Undo => "undo",
            Self::Cancel => "cancel",
            Self::Finish => "finish",
            Self::Processing => "processing",
            Self::DoneLead => "done.lead",
            Self::Paste => "paste",
            Self::Inspect => "inspect",
        }
    }

    pub const fn from_mode(mode: SelectionMode) -> Self {
        match mode {
            SelectionMode::Area => Self::ChipArea,
            SelectionMode::Window => Self::ChipWindow,
            SelectionMode::Screen => Self::ChipScreen,
            SelectionMode::Iphone => Self::ChipIphone,
        }
    }

    pub const fn mode(self) -> Option<SelectionMode> {
        match self {
            Self::ChipArea => Some(SelectionMode::Area),
            Self::ChipWindow => Some(SelectionMode::Window),
            Self::ChipScreen => Some(SelectionMode::Screen),
            Self::ChipIphone => Some(SelectionMode::Iphone),
            _ => None,
        }
    }

    pub const fn is_interactive(self) -> bool {
        !matches!(
            self,
            Self::DividerLead | Self::DividerClose | Self::Processing
        )
    }

    pub const fn accessibility_label(self) -> &'static str {
        match self {
            Self::Lead => "Recording timer",
            Self::DividerLead | Self::DividerClose => "",
            Self::ChipArea => "Area",
            Self::ChipWindow => "Window",
            Self::ChipScreen => "Screen",
            Self::ChipIphone => "iPhone",
            Self::Record => "Record",
            Self::Close | Self::Cancel => "Cancel",
            Self::Undo => "Undo",
            Self::Finish => "Finish",
            Self::Processing => "Processing",
            Self::DoneLead => "Completed",
            Self::Paste => "Paste",
            Self::Inspect => "Inspect",
        }
    }

    /// Keyboard shortcut hint (`"Esc"` on the `✕` cancel chip and `"Cancel"` chip).
    pub const fn shortcut_hint(self) -> Option<&'static str> {
        match self {
            Self::Close | Self::Cancel => Some("Esc"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pres {
    Hidden,
    Recording {
        chips: bool,
        has_iphone: bool,
        stroke_count: usize,
    },
    Selecting {
        mode: SelectionMode,
        hold: bool,
        has_iphone: bool,
    },
    Processing,
    Completed {
        label: String,
    },
}

impl Pres {
    /// Whether two presentations share the same geometry/layout structure (ignoring minor state
    /// like `stroke_count` or active mode highlight when `shows_record` doesn't change).
    pub fn same_layout(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Hidden, Self::Hidden) | (Self::Processing, Self::Processing) => true,
            (
                Self::Recording {
                    chips: a,
                    has_iphone: ia,
                    ..
                },
                Self::Recording {
                    chips: b,
                    has_iphone: ib,
                    ..
                },
            ) => a == b && (ia == ib || !*a),
            (
                Self::Selecting {
                    mode: ma,
                    hold: ha,
                    has_iphone: ia,
                },
                Self::Selecting {
                    mode: mb,
                    hold: hb,
                    has_iphone: ib,
                },
            ) => {
                ha == hb
                    && (ia == ib || *ha)
                    && ((*ma).shows_record() && !*ha) == ((*mb).shows_record() && !*hb)
            }
            (Self::Completed { label: a }, Self::Completed { label: b }) => a == b,
            _ => false,
        }
    }
}

/// Measured text widths (in points at 13 pt SF Pro) used to lay out capsule rows.
#[derive(Clone, Debug)]
pub struct Metrics {
    pub area: f32,
    pub window: f32,
    pub screen: f32,
    pub iphone: f32,
    pub record: f32,
    pub undo: f32,
    pub cancel: f32,
    pub finish: f32,
    pub processing: f32,
    pub paste: f32,
    pub inspect: f32,
    pub clock_000: f32,
    pub clock_0000: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            area: 30.0,
            window: 50.0,
            screen: 44.0,
            iphone: 43.0,
            record: 46.0,
            undo: 34.0,
            cancel: 44.0,
            finish: 38.0,
            processing: 78.0,
            paste: 36.0,
            inspect: 47.0,
            clock_000: 29.0,
            clock_0000: 37.0,
        }
    }
}

impl Metrics {
    pub fn mode_label(&self, mode: SelectionMode) -> f32 {
        match mode {
            SelectionMode::Area => self.area,
            SelectionMode::Window => self.window,
            SelectionMode::Screen => self.screen,
            SelectionMode::Iphone => self.iphone,
        }
    }

    pub fn segment_width(&self, mode: SelectionMode) -> f32 {
        self.mode_label(mode) + 2.0 * SEG_PX + SEG_ICON + SEG_ICON_GAP
    }

    pub fn chip_width(label_w: f32) -> f32 {
        label_w + 2.0 * CHIP_PX
    }

    pub fn clock_width(&self, clock: &str) -> f32 {
        if clock.len() <= 4 {
            self.clock_000
        } else {
            self.clock_0000
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutItem {
    pub id: ElemId,
    pub rect: [f32; 4],
    pub round: f32,
    pub rank: usize,
    pub last: usize,
    pub active: f32,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub l: f32,
    pub r: f32,
    pub t: f32,
    pub b: f32,
}

impl Layout {
    pub fn centered(w: f32, h: f32) -> Self {
        Self {
            l: CX - w * 0.5,
            r: CX + w * 0.5,
            t: TOP,
            b: TOP + h,
        }
    }

    pub fn width(&self) -> f32 {
        self.r - self.l
    }

    pub fn height(&self) -> f32 {
        self.b - self.t
    }
}

/// Computes the resting capsule bounds and item boxes for `pres`.
pub fn layout_of(
    pres: &Pres,
    clock: &str,
    completed_label_w: f32,
    m: &Metrics,
) -> (Layout, Vec<LayoutItem>) {
    let y0 = TOP + ISLAND_INSET;
    let mut raw: Vec<(ElemId, f32, f32, f32, bool)> = Vec::with_capacity(8);
    // (id, width, round, active_level, enabled)
    match pres {
        Pres::Hidden => {
            return (Layout::centered(IDLE_W, IDLE_H), Vec::new());
        }
        Pres::Selecting {
            mode,
            hold,
            has_iphone,
        } => {
            for &seg in SelectionMode::available(*has_iphone || *hold) {
                raw.push((
                    ElemId::from_mode(seg),
                    m.segment_width(seg),
                    CHIP_CORE_ROUND,
                    f32::from(u8::from(seg == *mode)),
                    true,
                ));
            }
            if mode.shows_record() && !*hold {
                raw.push((
                    ElemId::Record,
                    Metrics::chip_width(m.record),
                    CHIP_CORE_ROUND,
                    0.0,
                    true,
                ));
            }
            raw.push((ElemId::DividerClose, DIVIDER_W, 0.0, 0.0, false));
            raw.push((ElemId::Close, CHIP_H, CHIP_CORE_ROUND, 0.0, true));
        }
        Pres::Recording {
            chips,
            has_iphone,
            stroke_count,
        } => {
            let clock_w = m.clock_width(clock);
            let lead_w = CHIP_H + clock_w + LEAD_TRAIL;
            raw.push((ElemId::Lead, lead_w, CHIP_CORE_ROUND, 0.0, true));
            raw.push((ElemId::DividerLead, DIVIDER_W, 0.0, 0.0, false));
            if *chips {
                for &seg in SelectionMode::available(*has_iphone) {
                    raw.push((
                        ElemId::from_mode(seg),
                        m.segment_width(seg),
                        CHIP_CORE_ROUND,
                        f32::from(u8::from(seg == SelectionMode::Screen)),
                        true,
                    ));
                }
                raw.push((ElemId::DividerClose, DIVIDER_W, 0.0, 0.0, false));
                raw.push((ElemId::Close, CHIP_H, CHIP_CORE_ROUND, 0.0, true));
            } else {
                raw.push((
                    ElemId::Undo,
                    Metrics::chip_width(m.undo),
                    CHIP_CORE_ROUND,
                    0.0,
                    *stroke_count > 0,
                ));
                raw.push((
                    ElemId::Cancel,
                    Metrics::chip_width(m.cancel),
                    CHIP_CORE_ROUND,
                    0.0,
                    true,
                ));
                raw.push((
                    ElemId::Finish,
                    Metrics::chip_width(m.finish),
                    CHIP_CORE_ROUND,
                    0.0,
                    true,
                ));
            }
        }
        Pres::Processing => {
            let w = CHIP_H + 6.0 + m.processing + CHIP_PX;
            raw.push((ElemId::Processing, w, CHIP_CORE_ROUND, 0.0, false));
        }
        Pres::Completed { .. } => {
            let lead_w = CHIP_H + 8.0 + completed_label_w + LEAD_TRAIL;
            raw.push((ElemId::DoneLead, lead_w, CHIP_CORE_ROUND, 0.0, true));
            raw.push((ElemId::DividerLead, DIVIDER_W, 0.0, 0.0, false));
            raw.push((
                ElemId::Paste,
                Metrics::chip_width(m.paste),
                CHIP_CORE_ROUND,
                0.0,
                true,
            ));
            raw.push((
                ElemId::Inspect,
                Metrics::chip_width(m.inspect),
                CHIP_CORE_ROUND,
                0.0,
                true,
            ));
            raw.push((ElemId::Close, CHIP_H, CHIP_CORE_ROUND, 0.0, true));
        }
    }

    let inner_w: f32 =
        raw.iter().map(|(_, w, ..)| *w).sum::<f32>() + raw.len().saturating_sub(1) as f32 * CHIP_GAP;
    let outer_w = inner_w + 2.0 * ISLAND_INSET;
    let lay = Layout::centered(outer_w, ISLAND_H);
    let last = raw.len().saturating_sub(1);
    let mut x = lay.l + ISLAND_INSET;
    let items = raw
        .into_iter()
        .enumerate()
        .map(|(rank, (id, w, round, active, enabled))| {
            let item = LayoutItem {
                id,
                rect: [x, y0, w, CHIP_H],
                round,
                rank,
                last,
                active,
                enabled,
            };
            x += w + CHIP_GAP;
            item
        })
        .collect();
    (lay, items)
}

// ─── Model State ────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
struct ElemState {
    x: SpringVal,
    y: SpringVal,
    w: f32,
    h: f32,
    round: f32,
    rank: usize,
    last: usize,
    a: SpringVal,
    blur: SpringVal,
    go_blur: SpringVal,
    cap: f32,
    trip: f32,
    travelling: bool,
    want: bool,
    want_since: f32,
    active: SpringVal,
    hover: SpringVal,
    press: SpringVal,
    enabled: bool,
    initialized: bool,
}

impl ElemState {
    const fn new() -> Self {
        Self {
            x: SpringVal::new(CX, SHARED_SPRING),
            y: SpringVal::new(TOP + ISLAND_INSET, SHARED_SPRING),
            w: CHIP_H,
            h: CHIP_H,
            round: CHIP_CORE_ROUND,
            rank: 0,
            last: 0,
            a: SpringVal::new(0.0, CONTENT_IN_SPRING),
            blur: SpringVal::new(1.0, CONTENT_IN_SPRING),
            go_blur: SpringVal::new(0.0, CONTENT_OUT_SPRING),
            cap: 1.0,
            trip: 0.0,
            travelling: false,
            want: false,
            want_since: 0.0,
            active: SpringVal::new(0.0, SpringCfg::new(0.18, 0.0)),
            hover: SpringVal::new(0.0, SpringCfg::new(0.14, 0.0)),
            press: SpringVal::new(0.0, SpringCfg::new(0.12, 0.0)),
            enabled: true,
            initialized: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameElem {
    pub id: ElemId,
    /// Drawn bounding box `[x, y, w, h]` in panel points.
    pub rect: [f32; 4],
    /// Unscaled resting width and height `[w, h]` for texture tile layout.
    pub base_size: [f32; 2],
    pub alpha: f32,
    pub blur: f32,
    pub scale: f32,
    pub active: f32,
    pub hover: f32,
    pub press: f32,
    pub enabled: bool,
}

#[derive(Clone, Debug)]
pub struct FrameState {
    pub t: f32,
    pub shape: Silhouette,
    pub ghost: Option<Silhouette>,
    pub is_completed: bool,
    pub content_blur: f32,
    pub content_alpha: f32,
    pub elems: Vec<FrameElem>,
}

pub struct PillModel {
    pub t: f32,
    pub pres: Pres,
    pub geo_pres: Pres,
    pub geo_at: f32,
    pub reduced: bool,
    pub l: SpringVal,
    pub r: SpringVal,
    pub b: SpringVal,
    pub press: SpringVal,
    pub squish: SpringVal,
    pub shell_alpha: SpringVal,
    pub shell_blur: SpringVal,
    morph_start_w: f32,
    morph_start_h: f32,
    elems: [ElemState; ElemId::COUNT],
    ghost: Option<Silhouette>,
    dis: f32,
    pub hovered: Option<ElemId>,
    pub pressed: Option<ElemId>,
    pub clock: String,
    pub completed_label_w: f32,
    pub metrics: Metrics,
}

impl PillModel {
    pub fn new(metrics: Metrics) -> Self {
        let idle = Layout::centered(IDLE_W, IDLE_H);
        Self {
            t: 0.0,
            pres: Pres::Hidden,
            geo_pres: Pres::Hidden,
            geo_at: 0.0,
            reduced: false,
            l: SpringVal::new(idle.l, OPEN_SPRING),
            r: SpringVal::new(idle.r, OPEN_SPRING),
            b: SpringVal::new(idle.b, SpringCfg::new(0.32, 0.0)),
            press: SpringVal::new(0.0, PRESS_SPRING),
            squish: SpringVal::new(0.0, TAP_SPRING),
            shell_alpha: SpringVal::new(0.0, SpringCfg::new(0.18, 0.0)),
            shell_blur: SpringVal::new(1.0, SpringCfg::new(0.18, 0.0)),
            morph_start_w: IDLE_W,
            morph_start_h: IDLE_H,
            elems: [ElemState::new(); ElemId::COUNT],
            ghost: None,
            dis: 1.0,
            hovered: None,
            pressed: None,
            clock: "0:00".to_owned(),
            completed_label_w: 46.0,
            metrics,
        }
    }

    /// Sets the target presentation. Retargets are C1 continuous: positions and velocities carry over.
    pub fn set_pres(&mut self, next: Pres) {
        if self.pres == next {
            return;
        }
        let layout_changed = !self.pres.same_layout(&next);
        let was_visible = !matches!(self.pres, Pres::Hidden);
        let now_hidden = matches!(next, Pres::Hidden);
        let (prev_lay, prev_items) =
            layout_of(&self.geo_pres, &self.clock, self.completed_label_w, &self.metrics);
        let (next_lay, next_items) =
            layout_of(&next, &self.clock, self.completed_label_w, &self.metrics);

        // Update element metadata for the new presentation immediately (e.g. highlight or enabled state).
        if !now_hidden {
            for item in &next_items {
                let st = &mut self.elems[item.id as usize];
                st.enabled = item.enabled;
                if !st.initialized {
                    st.active.snap(item.active);
                } else {
                    st.active.to(item.active, SpringCfg::new(0.18, 0.0));
                }
            }
        }

        self.pres = next.clone();

        if !layout_changed {
            self.geo_pres = next;
            return;
        }

        // Collapse wait: whenever shrinking while leaving items are still visible (dismiss/cancel,
        // or shrinking into a narrower phase), outgoing content starts fading immediately while
        // the outline waits `COLLAPSE_WAIT_S` (0.10 s) so edge items never pop out in one frame.
        let shrinking_with_exit = was_visible
            && !self.reduced
            && (now_hidden
                || (next_lay.width() < prev_lay.width() - 1.0
                    && prev_items
                        .iter()
                        .any(|p| !next_items.iter().any(|n| n.id == p.id))));
        let wait = if shrinking_with_exit {
            COLLAPSE_WAIT_S
        } else {
            0.0
        };
        self.geo_at = self.t + wait;
        if wait == 0.0 {
            self.apply_geometry();
        }
    }

    fn apply_geometry(&mut self) {
        let next = self.pres.clone();
        let from_hidden = self.shell_alpha.x < 0.03;
        self.geo_pres = next.clone();
        let (lay, items) = layout_of(&next, &self.clock, self.completed_label_w, &self.metrics);

        if from_hidden && !matches!(next, Pres::Hidden) && !self.reduced {
            // Seed from the compact 120 × 40 capsule at top-center so the width springs out.
            let seed = Layout::centered(ISLAND_OPEN_FROM_W, ISLAND_H);
            self.l.snap(seed.l);
            self.r.snap(seed.r);
            self.b.snap(seed.b);
        }

        let before_l = self.l.x;
        let before_r = self.r.x;
        let before_b = self.b.x;
        self.morph_start_w = (before_r - before_l).max(1.0);
        self.morph_start_h = (before_b - TOP).max(1.0);

        if self.reduced {
            let current = self.silhouette();
            if self.ghost.is_none() || self.dis >= 0.5 {
                self.ghost = Some(current);
                self.dis = 0.0;
            } else {
                self.dis = 1.0 - self.dis.max(1.0 - self.dis);
            }
            self.l.snap(lay.l);
            self.r.snap(lay.r);
            self.b.snap(lay.b);
            self.shell_alpha.to(
                if matches!(next, Pres::Hidden) { 0.0 } else { 1.0 },
                SpringCfg::new(REDUCED_FADE_S, 0.0),
            );
            self.shell_blur.snap(0.0);
        } else {
            // Fold any active press swell into the outline's position & velocity (`foldPress`).
            if self.press.x.abs() > 1e-4 || self.press.v.abs() > 1e-4 {
                let swell = self.press.x * PRESS_GROW_PT;
                let rate = self.press.v * PRESS_GROW_PT;
                self.l.x -= swell;
                self.r.x += swell;
                self.b.x += swell * 0.5;
                self.l.v -= rate;
                self.r.v += rate;
                self.b.v += rate * 0.5;
                self.press.snap(0.0);
            }

            let dw = (lay.l - before_l).abs().max((lay.r - before_r).abs());
            let dh = (lay.b - before_b).abs();
            let opening = lay.width() * lay.height()
                >= (before_r - before_l) * (before_b - TOP) - 1.0;
            let base = if opening { OPEN_SPRING } else { CLOSE_SPRING };
            let f = (dw.hypot(dh) / REF_DIST).clamp(0.0, 1.0);
            let dur = (base.duration * mix(DISTANCE_FLOOR, 1.0, f)).max(0.14);
            let landing_idle = matches!(next, Pres::Hidden);
            // Since both L and R move opposite ways at once on symmetric width morphs, scale
            // each edge's bounce by 0.75 so total width overshoot matches a single-edge 0.28 spring.
            let w_bounce = if landing_idle { 0.0 } else { base.bounce * 0.75 };

            self.l.to(lay.l, SpringCfg::new(dur, w_bounce));
            self.r.to(lay.r, SpringCfg::new(dur, w_bounce));
            // Only the outline's width bounces; vertical bottom edge uses bounce = 0 so it never
            // bounces upward into the top of the screen.
            self.b.to(lay.b, SpringCfg::new(dur, 0.0));

            let show_shell = !landing_idle;
            self.shell_alpha.to(
                if show_shell { 1.0 } else { 0.0 },
                SpringCfg::new(if show_shell { 0.16 } else { dur }, 0.0),
            );
            self.shell_blur.to(
                if show_shell { 0.0 } else { 1.0 },
                SpringCfg::new(if show_shell { 0.14 } else { dur * 0.85 }, 0.0),
            );
        }

        // Configure elements for the new layout: elements already visible (`a.x > 0.03`) that
        // also exist in `items` travel on `SHARED_SPRING` (`0.24 s`, bounce 0) with `travelBlur`;
        // newly entering elements snap to their resting spot while invisible and wait for room.
        for item in &items {
            let st = &mut self.elems[item.id as usize];
            let [ix, iy, iw, ih] = item.rect;
            st.w = iw;
            st.h = ih;
            st.round = item.round;
            st.rank = item.rank;
            st.last = item.last;
            st.enabled = item.enabled;
            if !st.initialized || st.a.x <= 0.03 || self.reduced {
                st.x.snap(ix);
                st.y.snap(iy);
                st.trip = 0.0;
                st.travelling = false;
                st.active.snap(item.active);
                st.initialized = true;
            } else {
                let dist = (st.x.x - ix).hypot(st.y.x - iy);
                if dist > 0.5 {
                    st.trip = dist;
                    st.travelling = true;
                }
                st.x.to(ix, SHARED_SPRING);
                st.y.to(iy, SHARED_SPRING);
                st.active.to(item.active, SpringCfg::new(0.18, 0.0));
            }
        }
    }

    /// Updates the recording clock string (`M:SS`); if the digit count changed (e.g. `9:59 -> 10:00`),
    /// updates layout widths smoothly.
    pub fn set_clock(&mut self, clock: &str) {
        if self.clock == clock {
            return;
        }
        let old_w = self.metrics.clock_width(&self.clock);
        let new_w = self.metrics.clock_width(clock);
        self.clock.clear();
        self.clock.push_str(clock);
        if (old_w - new_w).abs() > 0.5 && matches!(self.pres, Pres::Recording { .. }) {
            self.apply_geometry();
        }
    }

    pub fn set_completed_label(&mut self, label: String, label_w: f32) {
        let width_changed = (self.completed_label_w - label_w).abs() > 0.5;
        self.completed_label_w = label_w;
        if let Pres::Completed { label: ref cur } = self.pres
            && (*cur != label || width_changed)
        {
            self.pres = Pres::Completed { label };
            self.apply_geometry();
        }
    }

    /// Pointer hover/press updates in panel coordinates `(x, y)` (top-left origin `[0..640, 0..84]`).
    pub fn set_pointer(&mut self, pos: Option<[f32; 2]>, down: bool) {
        let hit = pos.and_then(|[x, y]| self.hit_element(x, y));
        self.hovered = hit;
        if down {
            if self.pressed.is_none() {
                self.pressed = hit;
                if hit.is_some() && !self.reduced {
                    self.press.to(1.0, PRESS_SPRING);
                }
            }
        } else {
            if self.pressed.is_some() && !self.reduced {
                let w = std::f32::consts::TAU / TAP_SPRING.duration;
                self.squish.v = (self.squish.v + TAP_SQUISH * w * 1.4)
                    .clamp(-TAP_SQUISH * w * 3.0, TAP_SQUISH * w * 3.0);
            }
            self.pressed = None;
            self.press.to(0.0, PRESS_SPRING);
        }
    }

    /// Hit-tests against the model's target capsule shape (`hitIsland` in `island/model.js`),
    /// plus a `6 pt` slop margin so reaching for chips keeps mouse events enabled.
    pub fn hit_island(&self, x: f32, y: f32) -> bool {
        if matches!(self.pres, Pres::Hidden) {
            return false;
        }
        let (lay, _) = layout_of(&self.pres, &self.clock, self.completed_label_w, &self.metrics);
        let slop = 6.0;
        x >= lay.l - slop && x <= lay.r + slop && y >= lay.t - slop && y <= lay.b + slop
    }

    /// Hit-tests interactive elements at `(x, y)` in panel points.
    pub fn hit_element(&self, x: f32, y: f32) -> Option<ElemId> {
        if matches!(self.pres, Pres::Hidden) {
            return None;
        }
        let (_, items) = layout_of(&self.pres, &self.clock, self.completed_label_w, &self.metrics);
        for item in &items {
            if !item.id.is_interactive() || !item.enabled {
                continue;
            }
            let st = &self.elems[item.id as usize];
            if st.a.x < 0.15 {
                continue;
            }
            let [rx, ry, rw, rh] = [st.x.x, st.y.x, st.w, st.h];
            let slop = 2.0;
            if x >= rx - slop && x <= rx + rw + slop && y >= ry - slop && y <= ry + rh + slop {
                return Some(item.id);
            }
        }
        None
    }

    /// Current capsule silhouette (`L, T, R, B, radius, alpha, blur`), including press swell and
    /// tap squish. `T` is anchored at `TOP` so it never bounces upward into the top of the screen.
    pub fn silhouette(&self) -> Silhouette {
        self.silhouette_from(self.l.x, self.r.x, self.b.x)
    }

    fn silhouette_from(&self, mut l: f32, mut r: f32, mut b: f32) -> Silhouette {
        let swell = self.press.x * PRESS_GROW_PT;
        l -= swell;
        r += swell;
        b += swell * 0.5;
        let sq = self.squish.x;
        if sq.abs() > 1e-6 {
            let mid = (l + r) * 0.5;
            let w = (r - l) * (1.0 + sq);
            let h = (b - TOP) * (1.0 - sq * 1.2);
            l = mid - w * 0.5;
            r = mid + w * 0.5;
            b = TOP + h;
        }
        let w = (r - l).max(IDLE_W * 0.5);
        let mid = (l + r) * 0.5;
        l = mid - w * 0.5;
        r = mid + w * 0.5;
        b = b.max(TOP + IDLE_H * 0.5);
        let h = b - TOP;
        let radius = radius_of(h, ISLAND_RADIUS, RADIUS_BLEND);
        let alpha = self.shell_alpha.x.clamp(0.0, 1.0);
        let blur = if self.reduced {
            0.0
        } else {
            self.shell_blur.x.clamp(0.0, 1.0) * 3.5
        };
        Silhouette {
            l,
            t: TOP,
            r,
            b,
            radius,
            alpha,
            blur,
        }
    }

    /// Predicts the capsule silhouette over the next `lead` seconds on its current springs at 240 Hz.
    fn predict(&self, lead: f32) -> Vec<(f32, Silhouette)> {
        let mut l = self.l;
        let mut r = self.r;
        let mut b = self.b;
        let mut out = Vec::with_capacity(28);
        let mut t = AHEAD_STEP;
        while t <= lead + 1e-9 {
            l.step(AHEAD_STEP);
            r.step(AHEAD_STEP);
            b.step(AHEAD_STEP);
            out.push((t, self.silhouette_from(l.x, r.x, b.x)));
            t += AHEAD_STEP;
        }
        out
    }

    fn grown_enough(&self) -> bool {
        let target_w = self.r.target - self.l.target;
        let cur_w = self.r.x - self.l.x;
        let span = target_w - self.morph_start_w;
        if span <= 0.5 {
            return true;
        }
        (cur_w - self.morph_start_w) / span >= CONTENT_DELAY
    }

    /// Advances the simulation by `dt` seconds.
    pub fn advance(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        if dt <= 0.0 {
            return;
        }
        self.integrate(dt);
    }

    fn integrate(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        self.t += dt;
        if self.geo_at <= self.t + 1e-3 && self.geo_pres != self.pres {
            self.apply_geometry();
        }

        let reduced = self.reduced;
        if reduced {
            self.l.snap(self.l.target);
            self.r.snap(self.r.target);
            self.b.snap(self.b.target);
            self.press.snap(0.0);
            self.squish.snap(0.0);
            self.shell_alpha
                .step_critical(dt, REDUCED_FADE_S, 100.0);
            self.shell_blur.snap(0.0);
            self.dis = (self.dis + dt / REDUCED_FADE_S).min(1.0);
            if self.dis >= 1.0 {
                self.ghost = None;
            }
        } else {
            self.l.step(dt);
            self.r.step(dt);
            self.b.step(dt);
            self.press.step(dt);
            self.squish.step(dt);
            self.shell_alpha
                .step_critical(dt, self.shell_alpha.duration, 100.0);
            self.shell_blur
                .step_critical(dt, self.shell_blur.duration, 100.0);
        }

        let now = self.silhouette();
        let ahead = if reduced {
            Vec::new()
        } else {
            self.predict(EXIT_LEAD_S)
        };
        let settled_geo = self.geo_pres == self.pres;
        let grown = self.grown_enough();
        let (_, geo_items) =
            layout_of(&self.geo_pres, &self.clock, self.completed_label_w, &self.metrics);
        let (_, active_items) =
            layout_of(&self.pres, &self.clock, self.completed_label_w, &self.metrics);

        let inn = if reduced {
            SpringCfg::new(REDUCED_FADE_S, 0.0)
        } else {
            CONTENT_IN_SPRING
        };
        let out = if reduced {
            SpringCfg::new(REDUCED_FADE_S, 0.0)
        } else {
            CONTENT_OUT_SPRING
        };

        for id in ElemId::ALL {
            let idx = id as usize;
            let in_active = active_items.iter().any(|item| item.id == id);
            let in_geo = geo_items.iter().any(|item| item.id == id);
            // During `COLLAPSE_WAIT_S` (`!settled_geo`), shared elements that exist in both the
            // old and new presentation stay visible while outgoing elements fade out.
            let in_pres = in_active && (settled_geo || in_geo);
            let is_hovered = self.hovered == Some(id);
            let is_pressed = self.pressed == Some(id);

            let st = &mut self.elems[idx];
            st.hover.to(
                f32::from(u8::from(is_hovered)),
                SpringCfg::new(0.14, 0.0),
            );
            st.press.to(
                f32::from(u8::from(is_pressed)),
                SpringCfg::new(if is_pressed { 0.10 } else { 0.20 }, 0.0),
            );
            st.hover.step_critical(dt, 0.14, 100.0);
            st.press.step_critical(dt, 0.14, 100.0);
            st.active.step_critical(dt, 0.18, 100.0);

            if st.travelling && in_pres && !reduced {
                st.x.step(dt);
                st.y.step(dt);
                // Inelastic containment: keep travelling shared elements at least CLEAR inside the
                // current outline (`contain` in `island/model.js`).
                let min_x = now.l + ISLAND_INSET;
                let max_x = (now.r - ISLAND_INSET - st.w).max(min_x);
                if st.x.x < min_x {
                    st.x.x = min_x;
                    if st.x.v < 0.0 {
                        st.x.v = 0.0;
                    }
                } else if st.x.x > max_x {
                    st.x.x = max_x;
                    if st.x.v > 0.0 {
                        st.x.v = 0.0;
                    }
                }
                let left = (st.x.x - st.x.target).hypot(st.y.x - st.y.target);
                let going = st.trip > 2.0 && left > st.trip * 0.22;
                st.go_blur.target = f32::from(u8::from(going));
                let blur_dur = if going {
                    0.04
                } else {
                    (out.duration * BLUR_LEAD).max(0.03)
                };
                st.go_blur.step_critical(dt, blur_dur, 100.0);
                if left < 1e-3 && st.x.v.abs() < 1e-2 && st.go_blur.x < 1e-3 {
                    st.x.snap(st.x.target);
                    st.y.snap(st.y.target);
                    st.travelling = false;
                    st.trip = 0.0;
                }
            } else {
                st.x.snap(st.x.target);
                st.y.snap(st.y.target);
                st.travelling = false;
                st.go_blur.target = 0.0;
                st.go_blur.step_critical(dt, 0.08, 100.0);
            }

            let k = if st.travelling {
                1.0
            } else {
                scale_of(st.a.x, reduced)
            };
            let dy = if st.travelling {
                0.0
            } else {
                push_of(st.a.x, reduced)
            };
            let box_rect = [st.x.x, st.y.x, st.w, st.h];
            let d = box_clearance(&now, box_rect, k, st.round, [CX, TOP], dy);
            let d_rest = box_clearance(&now, box_rect, 1.0, st.round, [CX, TOP], 0.0);

            let gate = reduced
                || st.travelling
                || st.want
                || (settled_geo && grown && d_rest >= CLEAR + EDGE_BAND - 0.5);
            let want = in_pres && gate;
            if st.want != want {
                st.want = want;
                st.want_since = self.t - dt;
            }
            let turn = if reduced || st.travelling {
                0.0
            } else if want {
                CONTENT_STAGGER_S * st.rank as f32
            } else {
                // Outgoing items nearest either shrinking outer cap leave first (0 delay).
                let from_cap = st.rank.min(st.last.saturating_sub(st.rank));
                CONTENT_STAGGER_S * from_cap as f32 * 0.5
            };
            if self.t - st.want_since >= turn - 1e-6 {
                st.a.to(
                    f32::from(u8::from(want)),
                    if want { inn } else { out },
                );
            }
            let on = st.a.target >= 0.5;
            st.a.step(dt);

            // Blur runs on its own critically damped clock at `BLUR_LEAD` (0.55×) of the fade.
            st.blur.target = if on { 0.0 } else { 1.0 };
            let blur_dur = if reduced {
                0.05
            } else {
                ((if on { inn.duration } else { out.duration }) * BLUR_LEAD).max(0.03)
            };
            st.blur.step_critical(dt, blur_dur, 100.0);

            // Look-ahead exit cap (`stepCap` in `island/model.js`): only for non-travelling elements
            // (travelling shared elements are already clamped inside `now` by `contain`).
            let hidden = st.a.x <= 1e-3;
            if !reduced && !(st.travelling && in_pres) {
                let contact = if hidden {
                    f32::INFINITY
                } else {
                    let round = st.round;
                    ahead
                        .iter()
                        .find(|(_, sh)| {
                            box_clearance(sh, box_rect, k, round, [CX, TOP], dy) < CLEAR
                        })
                        .map_or(f32::INFINITY, |(at_t, _)| *at_t)
                };
                if d < CLEAR {
                    st.cap = if hidden { 1.0 } else { 0.0 };
                } else if contact.is_finite() {
                    st.cap -= st.cap * (dt / contact.max(dt)).min(1.0);
                } else if hidden {
                    st.cap = 1.0;
                } else {
                    st.cap += dt / EXIT_LEAD_S;
                }
                st.cap = st.cap.clamp(0.0, 1.0);
            } else {
                st.cap = 1.0;
            }
        }
    }

    /// Evaluates the complete frame state for rendering and motion-lint tracing.
    pub fn frame(&self) -> FrameState {
        let reduced = self.reduced;
        let mut shape = self.silhouette();
        let ghost = if reduced
            && let Some(mut g) = self.ghost
            && self.dis < 1.0
        {
            g.alpha *= 1.0 - self.dis;
            shape.alpha *= self.dis;
            Some(g)
        } else {
            None
        };

        let mut elems = Vec::with_capacity(10);
        let mut max_blur = 0.0_f32;
        let mut max_alpha = 0.0_f32;

        for id in ElemId::ALL {
            let st = &self.elems[id as usize];
            if !st.initialized {
                continue;
            }
            let base_k = if st.travelling {
                1.0
            } else {
                scale_of(st.a.x, reduced)
            };
            let press_k = 1.0 - 0.05 * st.press.x.clamp(0.0, 1.0);
            let dy = if st.travelling {
                0.0
            } else {
                push_of(st.a.x, reduced)
            };
            let box_rect = [st.x.x, st.y.x, st.w, st.h];
            let d = box_clearance(&shape, box_rect, base_k, st.round, [CX, TOP], dy);
            let edge = if !reduced {
                st.cap.clamp(0.0, 1.0).min(room(d))
            } else if d >= CLEAR {
                1.0
            } else if let Some(ref g) = ghost
                && box_clearance(g, box_rect, base_k, st.round, [CX, TOP], dy) >= CLEAR
            {
                g.alpha
            } else {
                0.0
            };
            let alpha = (st.a.x.clamp(0.0, 1.0).min(edge) * shape.alpha).clamp(0.0, 1.0);
            if alpha <= 1e-4 {
                continue;
            }
            let enter_exit_blur = if reduced {
                0.0
            } else {
                st.blur
                    .x
                    .clamp(0.0, 1.0)
                    .max(((1.0 - st.cap) / BLUR_LEAD).clamp(0.0, 1.0))
                    * CONTENT_BLUR_MAX
            };
            let travel_blur = if reduced {
                0.0
            } else {
                st.go_blur.x.clamp(0.0, 1.0) * TRAVEL_BLUR_MAX
            };
            let blur = enter_exit_blur.max(travel_blur);

            // Transform `[x, y, w, h]` by `base_k` about `(CX, TOP)` + `dy`, then `press_k` about item center.
            let sx = CX + (st.x.x - CX) * base_k;
            let sy = TOP + (st.y.x - TOP) * base_k + dy;
            let sw = st.w * base_k;
            let sh = st.h * base_k;
            let icx = sx + sw * 0.5;
            let icy = sy + sh * 0.5;
            let fw = sw * press_k;
            let fh = sh * press_k;
            let rect = [icx - fw * 0.5, icy - fh * 0.5, fw, fh];

            max_blur = max_blur.max(blur * alpha);
            max_alpha = max_alpha.max(alpha);
            elems.push(FrameElem {
                id,
                rect,
                base_size: [st.w, st.h],
                alpha,
                blur,
                scale: base_k * press_k,
                active: st.active.x.clamp(0.0, 1.0),
                hover: st.hover.x.clamp(0.0, 1.0),
                press: st.press.x.clamp(0.0, 1.0),
                enabled: st.enabled,
            });
        }

        FrameState {
            t: self.t,
            shape,
            ghost,
            is_completed: matches!(self.geo_pres, Pres::Completed { .. }),
            content_blur: max_blur,
            content_alpha: max_alpha,
            elems,
        }
    }

    /// True once every spring has settled and no delayed geometry transition is pending.
    pub fn is_at_rest(&self) -> bool {
        if self.geo_pres != self.pres {
            return false;
        }
        if self.reduced && self.dis < 1.0 {
            return false;
        }
        let outline_rest = [
            &self.l,
            &self.r,
            &self.b,
            &self.press,
            &self.squish,
            &self.shell_alpha,
            &self.shell_blur,
        ]
        .into_iter()
        .all(|s| (s.x - s.target).abs() < 1e-3 && s.v.abs() < 1e-2);
        if !outline_rest {
            return false;
        }
        self.elems.iter().all(|e| {
            !e.initialized
                || ((e.cap <= 1e-3 || e.cap >= 1.0 - 1e-3)
                    && [
                        &e.x, &e.y, &e.a, &e.blur, &e.go_blur, &e.active, &e.hover, &e.press,
                    ]
                    .into_iter()
                    .all(|s| (s.x - s.target).abs() < 1e-3 && s.v.abs() < 1e-2))
        })
    }

    /// True when the capsule is in `Pres::Hidden` and its outline and alpha have settled to rest.
    pub fn is_fully_hidden(&self) -> bool {
        matches!(self.pres, Pres::Hidden)
            && matches!(self.geo_pres, Pres::Hidden)
            && self.shell_alpha.x < 0.005
            && self.is_at_rest()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_DT: f32 = 1.0 / 120.0;

    /// Independent rounded-rect inside distance check (from `island/model.test.js`), written
    /// without sharing `outline_sdf` or `box_clearance` with the implementation under test.
    fn independent_inside(sh: &Silhouette, x: f32, y: f32) -> f32 {
        let r = sh
            .radius
            .min((sh.r - sh.l) * 0.5)
            .min((sh.b - sh.t) * 0.5);
        let nx = x.clamp(sh.l + r, sh.r - r);
        let ny = y.clamp(sh.t + r, sh.b - r);
        let from_core = (x - nx).hypot(y - ny);
        if from_core > 0.0 {
            r - from_core
        } else {
            (x - sh.l)
                .min(sh.r - x)
                .min(y - sh.t)
                .min(sh.b - y)
        }
    }

    fn assert_frame_room_invariants(model: &PillModel, f: &FrameState) {
        assert!(f.shape.l.is_finite() && f.shape.r.is_finite() && f.shape.b.is_finite());
        // Outline never bounces upward above TOP.
        assert!(
            (f.shape.t - TOP).abs() < 1e-5 && f.shape.b >= TOP,
            "outline bounced above TOP: {:?}",
            f.shape
        );
        for elem in &f.elems {
            assert!(
                elem.alpha >= 0.0 && elem.alpha <= 1.0 + 1e-5,
                "element {:?} alpha out of bounds: {}",
                elem.id,
                elem.alpha
            );
            if elem.alpha > 1e-3 && !model.reduced {
                let st = &model.elems[elem.id as usize];
                let cap = (elem.rect[2] * 0.5).min(elem.rect[3] * 0.5);
                let r = (st.round * elem.scale).min(cap);
                let x0 = elem.rect[0] + cap;
                let y0 = elem.rect[1] + cap;
                let x1 = (elem.rect[0] + elem.rect[2] - cap).max(x0);
                let y1 = (elem.rect[1] + elem.rect[3] - cap).max(y0);
                let mut min_inside = f32::INFINITY;
                for cx in [x0, x1] {
                    for cy in [y0, y1] {
                        min_inside = min_inside.min(independent_inside(&f.shape, cx, cy) - r);
                    }
                }
                assert!(
                    min_inside >= CLEAR - 0.15,
                    "element {:?} visible (alpha={:.3}) with clearance {:.2} < CLEAR at t={:.3}",
                    elem.id,
                    elem.alpha,
                    min_inside,
                    f.t
                );
            }
        }
    }

    #[test]
    fn content_alpha_never_exceeds_room_on_any_frame() {
        let scenes: Vec<Vec<(f32, Pres)>> = vec![
            // tap-then-area
            vec![
                (
                    0.0,
                    Pres::Recording {
                        chips: true,
                        has_iphone: false,
                        stroke_count: 0,
                    },
                ),
                (
                    1.2,
                    Pres::Selecting {
                        mode: SelectionMode::Area,
                        hold: false,
                        has_iphone: false,
                    },
                ),
                (2.4, Pres::Hidden),
            ],
            // tap-then-window
            vec![
                (
                    0.0,
                    Pres::Recording {
                        chips: true,
                        has_iphone: false,
                        stroke_count: 0,
                    },
                ),
                (
                    1.2,
                    Pres::Selecting {
                        mode: SelectionMode::Window,
                        hold: false,
                        has_iphone: false,
                    },
                ),
                (2.4, Pres::Hidden),
            ],
            // tap-then-screen
            vec![
                (
                    0.0,
                    Pres::Recording {
                        chips: true,
                        has_iphone: false,
                        stroke_count: 0,
                    },
                ),
                (
                    1.2,
                    Pres::Recording {
                        chips: false,
                        has_iphone: false,
                        stroke_count: 1,
                    },
                ),
                (2.4, Pres::Hidden),
            ],
            // hold-switcher
            vec![
                (
                    0.0,
                    Pres::Selecting {
                        mode: SelectionMode::Screen,
                        hold: true,
                        has_iphone: true,
                    },
                ),
                (
                    0.6,
                    Pres::Selecting {
                        mode: SelectionMode::Window,
                        hold: true,
                        has_iphone: true,
                    },
                ),
                (
                    1.2,
                    Pres::Selecting {
                        mode: SelectionMode::Iphone,
                        hold: true,
                        has_iphone: true,
                    },
                ),
                (
                    1.8,
                    Pres::Selecting {
                        mode: SelectionMode::Screen,
                        hold: true,
                        has_iphone: true,
                    },
                ),
                (
                    2.4,
                    Pres::Recording {
                        chips: false,
                        has_iphone: true,
                        stroke_count: 0,
                    },
                ),
                (3.6, Pres::Hidden),
            ],
            // finish
            vec![
                (
                    0.0,
                    Pres::Recording {
                        chips: true,
                        has_iphone: false,
                        stroke_count: 0,
                    },
                ),
                (
                    0.9,
                    Pres::Recording {
                        chips: false,
                        has_iphone: false,
                        stroke_count: 0,
                    },
                ),
                (2.0, Pres::Processing),
                (
                    2.8,
                    Pres::Completed {
                        label: "Copied".into(),
                    },
                ),
                (4.2, Pres::Hidden),
            ],
        ];

        for steps in scenes {
            let mut m = PillModel::new(Metrics::default());
            let mut step_idx = 0;
            let total_s = steps.last().unwrap().0 + 1.5;
            while m.t < total_s {
                while step_idx < steps.len() && m.t >= steps[step_idx].0 - 1e-6 {
                    m.set_pres(steps[step_idx].1.clone());
                    step_idx += 1;
                }
                m.advance(FRAME_DT);
                let f = m.frame();
                assert_frame_room_invariants(&m, &f);
            }
            assert!(m.is_at_rest());
            assert!(m.is_fully_hidden());
        }
    }

    #[test]
    fn retargeting_mid_flight_is_c1_continuous() {
        let mut m = PillModel::new(Metrics::default());
        m.set_pres(Pres::Recording {
            chips: true,
            has_iphone: false,
            stroke_count: 0,
        });
        for _ in 0..14 {
            m.advance(FRAME_DT);
        }
        let before = (m.l.x, m.l.v, m.r.x, m.r.v, m.b.x, m.b.v);
        assert!(before.3 > 50.0, "expected active outward velocity, got {}", before.3);

        // Retarget mid-expansion to Selecting(Area): position and velocity must carry over unchanged.
        m.set_pres(Pres::Selecting {
            mode: SelectionMode::Area,
            hold: false,
            has_iphone: false,
        });
        assert_eq!((m.l.x, m.l.v, m.r.x, m.r.v, m.b.x, m.b.v), before);

        // Next frame continues smoothly without a jump.
        m.advance(FRAME_DT);
        assert!((m.l.x - before.0).abs() < 10.0);
        assert!((m.r.x - before.2).abs() < 10.0);
    }

    #[test]
    fn every_scene_can_interrupt_every_other_and_settle() {
        let all_states = [
            Pres::Hidden,
            Pres::Recording {
                chips: true,
                has_iphone: false,
                stroke_count: 0,
            },
            Pres::Recording {
                chips: false,
                has_iphone: false,
                stroke_count: 2,
            },
            Pres::Selecting {
                mode: SelectionMode::Area,
                hold: false,
                has_iphone: false,
            },
            Pres::Selecting {
                mode: SelectionMode::Screen,
                hold: false,
                has_iphone: true,
            },
            Pres::Processing,
            Pres::Completed {
                label: "Copied".into(),
            },
        ];

        for from in &all_states {
            for to in &all_states {
                for interrupt_s in [0.04, 0.11, 0.22] {
                    let mut m = PillModel::new(Metrics::default());
                    m.set_pres(from.clone());
                    let mut elapsed = 0.0;
                    while elapsed < interrupt_s {
                        m.advance(FRAME_DT);
                        assert_frame_room_invariants(&m, &m.frame());
                        elapsed += FRAME_DT;
                    }
                    let prev_l = m.l.x;
                    let prev_r = m.r.x;
                    m.set_pres(to.clone());
                    m.advance(FRAME_DT);
                    assert!(
                        (m.l.x - prev_l).abs() <= 14.0 && (m.r.x - prev_r).abs() <= 14.0,
                        "jump on interrupt {:?} -> {:?} at {:.2}s",
                        from,
                        to,
                        interrupt_s
                    );
                    for _ in 0..180 {
                        m.advance(FRAME_DT);
                        assert_frame_room_invariants(&m, &m.frame());
                    }
                    assert!(
                        m.is_at_rest(),
                        "failed to settle after {:?} -> {:?}",
                        from,
                        to
                    );
                }
            }
        }
    }

    #[test]
    fn blur_resolves_ahead_of_opacity_and_shared_lead_travels_with_blur() {
        let mut m = PillModel::new(Metrics::default());
        m.set_pres(Pres::Recording {
            chips: true,
            has_iphone: false,
            stroke_count: 0,
        });
        let mut saw_sharp_before_opaque = false;
        for _ in 0..120 {
            m.advance(FRAME_DT);
            let st = &m.elems[ElemId::ChipArea as usize];
            if st.a.x > 0.35 && st.a.x < 0.90 && st.blur.x < 0.08 {
                saw_sharp_before_opaque = true;
            }
        }
        assert!(
            saw_sharp_before_opaque,
            "blur should resolve ahead of opacity (blurLead = 0.55)"
        );

        // Fold chips into RecordingControls: `Lead` is shared and travels with travelBlur > 0,
        // then clears its blur before landing.
        m.set_pres(Pres::Recording {
            chips: false,
            has_iphone: false,
            stroke_count: 0,
        });
        let mut peak_travel_blur = 0.0_f32;
        for _ in 0..120 {
            m.advance(FRAME_DT);
            let f = m.frame();
            if let Some(lead) = f.elems.iter().find(|e| e.id == ElemId::Lead) {
                peak_travel_blur = peak_travel_blur.max(lead.blur);
            }
        }
        assert!(
            peak_travel_blur > 1.0,
            "expected shared lead travel blur, got {peak_travel_blur}"
        );
        let settled = m.frame();
        let lead = settled
            .elems
            .iter()
            .find(|e| e.id == ElemId::Lead)
            .expect("lead visible");
        assert!(lead.blur < 1e-3 && (lead.alpha - 1.0).abs() < 1e-3);
    }

    #[test]
    fn outline_width_bounces_on_open_and_close_chip_has_esc_hint() {
        assert_eq!(ElemId::Close.shortcut_hint(), Some("Esc"));
        assert_eq!(ElemId::Cancel.shortcut_hint(), Some("Esc"));

        let mut m = PillModel::new(Metrics::default());
        m.set_pres(Pres::Recording {
            chips: true,
            has_iphone: false,
            stroke_count: 0,
        });
        let target_w = m.r.target - m.l.target;
        let mut max_w = 0.0_f32;
        for _ in 0..120 {
            m.advance(FRAME_DT);
            let f = m.frame();
            max_w = max_w.max(f.shape.width());
            assert_eq!(f.shape.t, TOP);
        }
        assert!(
            max_w > target_w + 2.0,
            "expected width overshoot on open (0.50 / 0.28), max_w={max_w} target_w={target_w}"
        );
    }

    #[test]
    fn opening_content_follows_half_the_growth_inner_first() {
        let mut m = PillModel::new(Metrics::default());
        m.set_pres(Pres::Recording {
            chips: true,
            has_iphone: false,
            stroke_count: 0,
        });
        let target_w = m.r.target - m.l.target;
        let mut first: Vec<(ElemId, f32)> = Vec::new();
        for _ in 0..140 {
            m.advance(FRAME_DT);
            let f = m.frame();
            for elem in &f.elems {
                if elem.alpha > 0.02 && !first.iter().any(|(id, _)| *id == elem.id) {
                    let grown = (f.shape.width() - ISLAND_OPEN_FROM_W)
                        / (target_w - ISLAND_OPEN_FROM_W);
                    assert!(
                        grown >= CONTENT_DELAY - 0.02,
                        "{:?} appeared at grown={grown}",
                        elem.id
                    );
                    first.push((elem.id, m.t));
                }
            }
        }
        let at = |want: ElemId| first.iter().find(|(id, _)| *id == want).unwrap().1;
        assert!(
            at(ElemId::ChipWindow) <= at(ElemId::Close),
            "inner chip enters before outer close chip"
        );
    }

    #[test]
    fn reduce_motion_dissolves_without_travel() {
        let mut m = PillModel::new(Metrics::default());
        m.reduced = true;
        m.set_pres(Pres::Recording {
            chips: true,
            has_iphone: false,
            stroke_count: 0,
        });
        for _ in 0..60 {
            m.advance(FRAME_DT);
        }
        let w0 = m.frame().shape.width();
        m.set_pres(Pres::Selecting {
            mode: SelectionMode::Area,
            hold: false,
            has_iphone: false,
        });
        m.advance(FRAME_DT);
        let f = m.frame();
        assert!(
            f.ghost
                .as_ref()
                .is_some_and(|g| (g.width() - w0).abs() < 1e-3 && g.alpha > 0.8),
            "expected dissolving ghost silhouette in Reduce Motion"
        );
        assert_eq!(f.content_blur, 0.0);
        for _ in 0..60 {
            m.advance(FRAME_DT);
        }
        assert!(m.frame().ghost.is_none());
    }
}
