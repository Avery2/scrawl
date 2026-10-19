//! The capsule's motion, ported from Kit Motion's Dynamic Island study
//! (`~/code/home/research/2026-10-16-motion-v3/island`: `NATIVE.md`, `model.js`) to Scrawl's
//! horizontal pill. Pure: no window, no GPUI, so the tests below drive it frame by frame.
//!
//! - **One shape.** One rounded rectangle whose width and height are springs (`w`, `h`), hung from
//!   a fixed top edge and centred; its radius is a smooth minimum of `h / 2` and a cap. Every
//!   state (recording, picker, processing, done) is that outline retargeted, velocity kept.
//!   Opening, the width leads; closing, the height leads, and the outline first waits
//!   `collapse_wait` for the leaving content. Only the width bounces.
//! - **Content at its final place.** Each item (a chip, the lead, a divider) is laid out where the
//!   target layout puts it, never reflowed inside a moving width. It shows only where the outline
//!   has room for it: alpha ≤ room, `clear` pt clear of the edge, ramping over `band` pt.
//! - **In.** After `content_delay` of the growth and once its place is clear, an item fades in on
//!   a zero-bounce spring, `stagger` after the one inside it, travelling `push` pt outward.
//! - **Out.** Each frame the outline is predicted `exit_lead` ahead; an item the edge will reach
//!   fades linearly so it is gone exactly at contact. Nothing is ever cut by the edge.
//! - **Shared items.** An item in both the old and the new layout is the same element: it travels
//!   to its new place on `shared` (zero bounce), and the edge pushes it along rather than cutting
//!   it (`contain`).
//! - **Reduce Motion.** No travel: the old silhouette dissolves into the new over `reduced_fade`
//!   (coverage added, so the overlap stays solid) and content only fades.
//!
//! Coordinates are points from the capsule's resting centre: x right, y down. The outline's top
//! edge stays at `-ISLAND_H / 2`.

use super::motion::Tune;
use super::{CHIP_GAP, CHIP_H, ISLAND_H, ISLAND_INSET, Spring};

/// The outline a capsule opens from and lands into: a small lozenge under the menu bar.
pub const LAND_W: f32 = 64.0;
pub const LAND_H: f32 = 20.0;
/// The shell fades in over this much width above `LAND_W` (and out over it, landing).
const LAND_FADE_W: f32 = 40.0;
/// The largest corner radius; above `ISLAND_H / 2` by more than the blend, so the pill is an
/// exact capsule at rest.
const MAX_RADIUS: f32 = ISLAND_H / 2.0 + 10.0;
const TOP: f32 = -ISLAND_H / 2.0;
/// The look-ahead's sampling step.
const AHEAD_STEP: f32 = 1.0 / 240.0;

// ---------------------------------------------------------------------------------------------
// Springs

/// SwiftUI's `spring(duration:bounce:)`: ω = 2π / duration, ζ = 1 − bounce, integrated at 480 Hz
/// so a retarget keeps position and velocity (`stepSpring` in phone-stage.js).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct S1 {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    pub spring: Spring,
}

impl S1 {
    pub const fn at(x: f32, spring: Spring) -> Self {
        Self {
            x,
            v: 0.0,
            target: x,
            spring,
        }
    }

    pub fn to(&mut self, target: f32, spring: Spring) {
        self.target = target;
        self.spring = spring;
    }

    pub fn snap(&mut self, x: f32) {
        self.x = x;
        self.target = x;
        self.v = 0.0;
    }

    pub fn resting(&self) -> bool {
        (self.x - self.target).abs() < 1e-4 && self.v.abs() < 4e-4
    }

    pub fn step(&mut self, dt: f32) {
        if self.resting() {
            self.snap(self.target);
            return;
        }
        let w = std::f32::consts::TAU / self.spring.response.max(0.02);
        let zeta = 1.0 - self.spring.bounce;
        let mut left = dt;
        while left > 1e-9 {
            let h = left.min(1.0 / 480.0);
            self.v += (-w * w * (self.x - self.target) - 2.0 * zeta * w * self.v) * h;
            self.x += self.v * h;
            left -= h;
        }
        if self.resting() {
            self.snap(self.target);
        }
    }
}

const fn spring(duration: f32, bounce: f32) -> Spring {
    Spring {
        response: duration,
        bounce,
    }
}

fn smooth(a: f32, b: f32, v: f32) -> f32 {
    let t = ((v - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------------------------
// Geometry

/// The outline as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    pub l: f32,
    pub r: f32,
    pub t: f32,
    pub b: f32,
    pub radius: f32,
}

impl Shape {
    pub fn width(&self) -> f32 {
        self.r - self.l
    }

    pub fn height(&self) -> f32 {
        self.b - self.t
    }

    /// Signed distance from (x, y) to the rounded rectangle; negative inside. Exact.
    pub fn sdf(&self, x: f32, y: f32) -> f32 {
        let hw = (self.r - self.l) / 2.0;
        let hh = (self.b - self.t) / 2.0;
        let r = self.radius.min(hw).min(hh).max(0.0);
        let qx = (x - (self.l + self.r) / 2.0).abs() - (hw - r);
        let qy = (y - (self.t + self.b) / 2.0).abs() - (hh - r);
        qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
    }
}

/// The corner radius from the height alone: a polynomial smooth minimum of `h / 2` and the cap
/// (`radiusOf` in model.js), so the radius has no kink as it reaches the cap.
pub fn radius_of(h: f32, max_radius: f32, blend: f32) -> f32 {
    let a = h / 2.0;
    if blend <= 0.0 {
        return a.min(max_radius);
    }
    let t = (blend - (a - max_radius).abs()).max(0.0) / blend;
    a.min(max_radius) - t * t * blend / 4.0
}

/// Where an item's ink sits in its box: insets from its left and right, and a rounded box of
/// half-height `half_h` with corner radius `round` (a capsule when they are equal).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub left: f32,
    pub right: f32,
    pub half_h: f32,
    pub round: f32,
}

impl Ink {
    /// A chip: its background is a capsule of the row's height.
    pub const CHIP: Ink = Ink {
        left: 0.0,
        right: 0.0,
        half_h: CHIP_H / 2.0,
        round: CHIP_H / 2.0,
    };
}

/// One item of a layout: a stable identity, its width, and its ink.
#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub key: String,
    pub w: f32,
    pub ink: Ink,
}

impl Spec {
    pub fn new(key: impl Into<String>, w: f32, ink: Ink) -> Self {
        Self {
            key: key.into(),
            w,
            ink,
        }
    }

    pub fn chip(key: impl Into<String>, w: f32) -> Self {
        Self::new(key, w, Ink::CHIP)
    }
}

/// How far inside the outline an item drawn at `x` sits: its ink box's worst corner, less its
/// rounding (the distance field is 1-Lipschitz, so that bounds the rounded box).
pub fn clearance(shape: &Shape, x: f32, w: f32, ink: Ink) -> f32 {
    let r = ink.round.min(ink.half_h).max(0.0);
    let mut x0 = x + ink.left + r;
    let mut x1 = x + w - ink.right - r;
    if x1 < x0 {
        let mid = (x0 + x1) / 2.0;
        x0 = mid;
        x1 = mid;
    }
    let y = ink.half_h - r;
    let worst = [(x0, -y), (x1, -y), (x0, y), (x1, y)]
        .iter()
        .map(|&(px, py)| shape.sdf(px, py))
        .fold(f32::NEG_INFINITY, f32::max);
    -worst - r
}

/// Layout widths: the outer width of a row of items, inset on both sides.
pub fn row_width(specs: &[Spec]) -> f32 {
    let inner: f32 = specs.iter().map(|s| s.w).sum::<f32>()
        + specs.len().saturating_sub(1) as f32 * CHIP_GAP;
    inner + 2.0 * ISLAND_INSET
}

/// Left edges of a centred row, from the capsule's centre.
pub fn row_lefts(specs: &[Spec]) -> Vec<f32> {
    let mut x = -(row_width(specs) / 2.0 - ISLAND_INSET);
    specs
        .iter()
        .map(|s| {
            let left = x;
            x += s.w + CHIP_GAP;
            left
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// State

#[derive(Clone, Debug)]
pub struct Item {
    pub spec: Spec,
    /// Left edge: its place in the layout, or travelling to it.
    pub x: S1,
    /// Fade, 0..1.
    pub a: S1,
    /// 1 = fully blurred. Runs ahead of the fade (`blur_lead`).
    pub blur: S1,
    /// The edge cap: how much the outline's look-ahead still lets it show.
    pub cap: f32,
    /// In the current layout.
    pub in_target: bool,
    want: bool,
    since: f64,
    /// Order outward from the centre, for the stagger; `last` is the layout's highest.
    rank: usize,
    last: usize,
    /// Travelling as a shared element (the edge pushes it along instead of cutting it).
    travelling: bool,
    /// Native's swap: fading out to jump to its new place while invisible.
    swap: Option<f32>,
    /// It has been seen in this layout's place (so it stays, contained, while the edge moves).
    settled: bool,
    /// It was showing when the current layout arrived: it stays through the change.
    kept: bool,
}

impl Item {
    fn new(spec: Spec, x: f32, t: f64) -> Self {
        Self {
            spec,
            x: S1::at(x, spring(0.24, 0.0)),
            a: S1::at(0.0, spring(0.3, 0.0)),
            blur: S1::at(1.0, spring(0.16, 0.0)),
            cap: 1.0,
            in_target: true,
            want: false,
            since: t,
            rank: 0,
            last: 0,
            travelling: false,
            swap: None,
            settled: false,
            kept: false,
        }
    }

    fn centre_sign(&self) -> f32 {
        let c = self.x.target + self.spec.w / 2.0;
        if c.abs() < 0.5 { 0.0 } else { c.signum() }
    }

    /// Drawn left edge: its place, less what is left of the push (outward as it fades in).
    fn drawn_x(&self, tune: &Tune, reduced: bool) -> f32 {
        if reduced {
            return self.x.x;
        }
        self.x.x - tune.content_push * (1.0 - self.a.x.clamp(0.0, 1.0)) * self.centre_sign()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Life {
    Hidden,
    Open,
    /// Content leaving, then the outline landing into `LAND_W × LAND_H` as the shell fades.
    Landing,
}

#[derive(Clone, Debug)]
struct Geometry {
    w: f32,
    h: f32,
    lefts: Vec<(String, f32)>,
}

#[derive(Clone, Debug)]
pub struct Island {
    pub w: S1,
    pub h: S1,
    pub squish: S1,
    /// Reduce Motion's shell fade (open and landing).
    fade: S1,
    pub items: Vec<Item>,
    applied: Geometry,
    pending: Option<(f64, Geometry)>,
    grow_from: (f32, f32),
    t: f64,
    pub reduced: bool,
    /// Reduce Motion: the old silhouette dissolving while `dis` runs 0 → 1.
    ghost: Option<Shape>,
    dis: f32,
    pub life: Life,
}

/// Everything a frame draws.
#[derive(Clone, Debug)]
pub struct Frame {
    pub shape: Shape,
    pub shell_alpha: f32,
    /// Reduce Motion's dissolving silhouette and its opacity.
    pub ghost: Option<(Shape, f32)>,
    pub items: Vec<ItemFrame>,
    /// The content layer's blur (pt) and scale. GPUI can't blur or scale one element, so these
    /// apply only while every visible item is coming or going together (open, dismiss, a whole
    /// new state); when something stays, items only fade and travel.
    pub content_blur: f32,
    pub content_scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemFrame {
    pub key: String,
    pub x: f32,
    pub w: f32,
    pub alpha: f32,
    /// In the layout and shown enough to take clicks.
    pub live: bool,
}

impl Default for Island {
    fn default() -> Self {
        Self::new()
    }
}

impl Island {
    pub fn new() -> Self {
        Self {
            w: S1::at(LAND_W, spring(0.5, 0.0)),
            h: S1::at(LAND_H, spring(0.5, 0.0)),
            squish: S1::at(0.0, spring(0.35, 0.45)),
            fade: S1::at(0.0, spring(0.2, 0.0)),
            items: Vec::new(),
            applied: Geometry {
                w: LAND_W,
                h: LAND_H,
                lefts: Vec::new(),
            },
            pending: None,
            grow_from: (LAND_W, LAND_H),
            t: 0.0,
            reduced: false,
            ghost: None,
            dis: 1.0,
            life: Life::Hidden,
        }
    }

    /// The width the capsule is heading for (hit-testing uses it, not the in-flight width).
    pub fn target_width(&self) -> f32 {
        self.pending.as_ref().map_or(self.applied.w, |(_, g)| g.w)
    }

    pub fn is_landed(&self) -> bool {
        self.life == Life::Landing
            && self.pending.is_none()
            && if self.reduced {
                self.fade.x < 0.02
            } else {
                (self.w.x - LAND_W).abs() < 0.5 && (self.h.x - LAND_H).abs() < 0.5
            }
    }

    /// From nothing: the outline starts as the landing lozenge, unseen, and the next `present`
    /// grows it. Already open, nothing changes.
    pub fn open(&mut self) {
        if self.life == Life::Open {
            return;
        }
        if self.life == Life::Hidden {
            self.w.snap(LAND_W);
            self.h.snap(LAND_H);
            self.squish.snap(0.0);
            self.items.clear();
            self.applied = Geometry {
                w: LAND_W,
                h: LAND_H,
                lefts: Vec::new(),
            };
            self.pending = None;
            self.ghost = None;
            self.dis = 1.0;
            self.fade.snap(0.0);
        }
        self.life = Life::Open;
    }

    /// Restarts an opening from its first seen frame (a new window shows a few frames late).
    pub fn restart_open(&mut self, tune: &Tune) {
        if self.life != Life::Open || self.shell_alpha() > 0.98 {
            return;
        }
        let target = self.target_width();
        self.w.snap(LAND_W);
        self.h.snap(LAND_H);
        self.fade.snap(0.0);
        for item in &mut self.items {
            item.a.snap(0.0);
            item.blur.snap(1.0);
            item.want = false;
            item.settled = false;
        }
        let lefts = self.applied.lefts.clone();
        self.pending = None;
        self.apply(
            Geometry {
                w: target,
                h: ISLAND_H,
                lefts,
            },
            tune,
        );
    }

    pub fn hide_now(&mut self) {
        *self = Self {
            reduced: self.reduced,
            ..Self::new()
        };
    }

    /// The tactile squish (a chip or key changed the mode): the outline widens and flattens
    /// for a moment on `Tune::squish`. That bounce is the material, so it stays.
    pub fn kick(&mut self, tune: &Tune) {
        if self.reduced || self.life != Life::Open {
            return;
        }
        let omega = std::f32::consts::TAU / tune.squish.response.max(0.02);
        // An impulse whose first swing peaks at `squish_amount` (ζ = 0.55: e^(−ζ/√(1−ζ²)·atan(√(1−ζ²)/ζ)) ≈ 0.52).
        self.squish.v += tune.squish_amount * omega / 0.52;
    }

    /// The target layout, every frame: new items join at their place, shared ones travel there,
    /// missing ones leave, and the outline retargets when the size changes.
    pub fn present(&mut self, specs: &[Spec], tune: &Tune) {
        if self.life != Life::Open {
            return;
        }
        let lefts = row_lefts(specs);
        let width = row_width(specs);
        let keys: Vec<&str> = specs.iter().map(|s| s.key.as_str()).collect();
        for item in &mut self.items {
            item.in_target = keys.contains(&item.spec.key.as_str());
        }
        for (spec, &left) in specs.iter().zip(&lefts) {
            match self.items.iter_mut().find(|i| i.spec.key == spec.key) {
                Some(item) => item.spec = spec.clone(),
                None => self.items.push(Item::new(spec.clone(), left, self.t)),
            }
        }
        // Rank outward from the centre: the inner items come first, leave last.
        let mut order: Vec<(usize, f32)> = specs
            .iter()
            .zip(&lefts)
            .enumerate()
            .map(|(i, (s, l))| (i, (l + s.w / 2.0).abs()))
            .collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        let last = specs.len().saturating_sub(1);
        for (rank, (i, _)) in order.iter().enumerate() {
            if let Some(item) = self.items.iter_mut().find(|it| it.spec.key == specs[*i].key) {
                item.rank = rank;
                item.last = last;
            }
        }
        let geometry = Geometry {
            w: width,
            h: ISLAND_H,
            lefts: specs
                .iter()
                .zip(&lefts)
                .map(|(s, &l)| (s.key.clone(), l))
                .collect(),
        };
        self.retarget(geometry, tune);
    }

    /// Content leaves, the outline waits for it, then lands as the shell fades.
    pub fn dismiss(&mut self, tune: &Tune) {
        if self.life == Life::Hidden {
            return;
        }
        for item in &mut self.items {
            item.in_target = false;
        }
        self.life = Life::Landing;
        if self.reduced {
            self.fade.to(0.0, spring(tune.reduced_fade, 0.0));
            self.pending = None;
            return;
        }
        self.retarget(
            Geometry {
                w: LAND_W,
                h: LAND_H,
                lefts: Vec::new(),
            },
            tune,
        );
    }

    fn same(a: &Geometry, b: &Geometry) -> bool {
        (a.w - b.w).abs() < 0.01
            && (a.h - b.h).abs() < 0.01
            && a.lefts.len() == b.lefts.len()
            && a
                .lefts
                .iter()
                .zip(&b.lefts)
                .all(|(x, y)| x.0 == y.0 && (x.1 - y.1).abs() < 0.01)
    }

    fn retarget(&mut self, geometry: Geometry, tune: &Tune) {
        if let Some((_, pending)) = &mut self.pending {
            *pending = geometry;
            return;
        }
        if Self::same(&self.applied, &geometry) {
            return;
        }
        // Closing over content that is still leaving: the outline waits for it (Apple's
        // collapse), so the content goes first and the edge never has to cut it.
        let shrinking = geometry.w * geometry.h < self.w.target * self.h.target - 1.0;
        let leaving = self.items.iter().any(|i| !i.in_target && i.a.x > 0.05);
        if shrinking && leaving && !self.reduced && tune.collapse_wait > 0.0 {
            self.pending = Some((self.t + f64::from(tune.collapse_wait), geometry));
            return;
        }
        self.apply(geometry, tune);
    }

    fn apply(&mut self, geometry: Geometry, tune: &Tune) {
        let landing = self.life == Life::Landing;
        if self.reduced {
            let current = self.shape(tune);
            if self.ghost.is_none() || self.dis >= 0.5 {
                self.ghost = Some(current);
                self.dis = 0.0;
            } else {
                self.dis = 1.0 - self.dis.max(1.0 - self.dis);
            }
            self.w.snap(geometry.w);
            self.h.snap(geometry.h);
            if self.life == Life::Open {
                self.fade.to(1.0, spring(tune.reduced_fade, 0.0));
            }
            for (key, left) in &geometry.lefts {
                if let Some(item) = self.items.iter_mut().find(|i| &i.spec.key == key) {
                    item.x.snap(*left);
                }
            }
            self.grow_from = (geometry.w, geometry.h);
            self.applied = geometry;
            return;
        }
        let (w0, h0) = (self.w.x, self.h.x);
        let opening = geometry.w * geometry.h >= self.w.target * self.h.target - 1.0;
        let base = if opening { tune.open } else { tune.close };
        let dw = (geometry.w - self.w.target).abs();
        let dh = (geometry.h - self.h.target).abs();
        // Opening, the width leads; closing, the height leads, so the sides close in last.
        let w_duration = if !opening && dw > 0.5 && dh > 0.5 {
            base.response + tune.height_lead
        } else if opening {
            (base.response - tune.width_lead).max(0.1)
        } else {
            base.response
        };
        // Only the width bounces, and landing has none (it eases onto the lozenge).
        let bounce = if landing { 0.0 } else { base.bounce };
        self.w.to(geometry.w, spring(w_duration, bounce));
        self.h.to(geometry.h, spring(base.response, 0.0));
        self.grow_from = (w0, h0);
        for item in &mut self.items {
            item.kept = item.in_target && (item.settled || item.travelling);
        }
        for (key, left) in &geometry.lefts {
            let Some(item) = self.items.iter_mut().find(|i| &i.spec.key == key) else {
                continue;
            };
            if (item.x.target - left).abs() < 0.01 {
                continue;
            }
            let visible = item.a.x > 0.02 || item.a.target > 0.0;
            if !visible {
                item.x.snap(*left);
            } else if tune.shared_travel >= 0.5 {
                item.x.to(*left, spring(tune.shared.response, 0.0));
                item.travelling = true;
            } else {
                item.swap = Some(*left);
            }
        }
        self.applied = geometry;
    }

    /// The outline from its springs: the squish widens it and flattens it from the bottom.
    fn shape_of(w: f32, h: f32, squish: f32, tune: &Tune) -> Shape {
        let w = w * (1.0 + squish);
        let h = (h * (1.0 - 1.2 * squish)).max(1.0);
        Shape {
            l: -w / 2.0,
            r: w / 2.0,
            t: TOP,
            b: TOP + h,
            radius: radius_of(h, MAX_RADIUS, tune.radius_blend),
        }
    }

    pub fn shape(&self, tune: &Tune) -> Shape {
        Self::shape_of(self.w.x, self.h.x, self.squish.x, tune)
    }

    fn room(d: f32, tune: &Tune) -> f32 {
        smooth(tune.clear, tune.clear + tune.band, d)
    }

    fn shell_alpha(&self) -> f32 {
        if self.reduced {
            self.fade.x.clamp(0.0, 1.0)
        } else {
            smooth(LAND_W, LAND_W + LAND_FADE_W, self.w.x)
        }
    }

    /// 'progress': the outline has done `content_delay` of its growth.
    fn grown_enough(&self, tune: &Tune) -> bool {
        let part = |s: &S1, from: f32| {
            let span = s.target - from;
            span <= 0.5 || (s.x - from) / span >= tune.content_delay
        };
        part(&self.w, self.grow_from.0) && part(&self.h, self.grow_from.1)
    }

    /// The outline over the next `lead` s on its current springs.
    fn predict(&self, lead: f32, tune: &Tune) -> Vec<(f32, Shape)> {
        let (mut w, mut h, mut sq) = (self.w, self.h, self.squish);
        let mut out = Vec::new();
        let mut t = AHEAD_STEP;
        while t <= lead + 1e-6 {
            w.step(AHEAD_STEP);
            h.step(AHEAD_STEP);
            sq.step(AHEAD_STEP);
            out.push((t, Self::shape_of(w.x, h.x, sq.x, tune)));
            t += AHEAD_STEP;
        }
        out
    }

    pub fn step(&mut self, dt: f32, tune: &Tune) {
        if dt <= 0.0 || self.life == Life::Hidden {
            return;
        }
        self.t += f64::from(dt);
        if let Some((at, _)) = &self.pending
            && *at <= self.t
            && let Some((_, geometry)) = self.pending.take()
        {
            self.apply(geometry, tune);
        }
        let reduced = self.reduced;
        if reduced {
            self.w.snap(self.w.target);
            self.h.snap(self.h.target);
            self.squish.snap(0.0);
            self.dis = (self.dis + dt / tune.reduced_fade.max(0.05)).min(1.0);
            if self.dis >= 1.0 {
                self.ghost = None;
            }
            self.fade.step(dt);
        } else {
            self.w.step(dt);
            self.h.step(dt);
            self.squish.spring = tune.squish;
            self.squish.step(dt);
        }
        let shape = self.shape(tune);
        let grown = self.grown_enough(tune);
        let settled_geo = self.pending.is_none();
        let inn = if reduced {
            tune.reduced_fade
        } else {
            tune.content_in
        };
        let out = if reduced {
            tune.reduced_fade
        } else {
            tune.content_out
        };
        let needs_ahead = !reduced
            && self
                .items
                .iter()
                .any(|i| i.a.x > 1e-3 && !i.settled && !i.travelling);
        let ahead = if needs_ahead {
            self.predict(tune.exit_lead, tune)
        } else {
            Vec::new()
        };
        let t = self.t;
        for item in &mut self.items {
            // Native's swap: out, jump while unseen, then in like any newcomer.
            if let Some(left) = item.swap
                && item.a.x < 0.03
            {
                item.x.snap(left);
                item.swap = None;
                item.settled = false;
            }
            let x = item.drawn_x(tune, reduced);
            let d = clearance(&shape, x, item.spec.w, item.spec.ink);
            let at_place = clearance(&shape, item.x.target, item.spec.w, item.spec.ink);
            // A newcomer waits for the outline to stop waiting, half its growth, and a clear place;
            // once wanted it stays wanted while it is in the layout (no flicker on a squish).
            let newcomer_ok =
                settled_geo && (reduced || grown && at_place >= tune.clear + tune.band - 1e-3);
            let want = item.in_target && item.swap.is_none() && (item.want || newcomer_ok);
            if want != item.want {
                item.want = want;
                item.since = t - f64::from(dt);
            }
            let turn = if reduced {
                0.0
            } else if want {
                tune.stagger * item.rank as f32
            } else {
                tune.stagger * item.last.saturating_sub(item.rank) as f32 * 0.5
            };
            if (t - item.since) as f32 >= turn - 1e-6 {
                let target = if want { 1.0 } else { 0.0 };
                item.a.to(target, spring(if want { inn } else { out }, 0.0));
            }
            item.a.step(dt);
            let on = item.a.target >= 1.0;
            let blur_duration = if reduced {
                0.05
            } else {
                ((if on { inn } else { out }) * tune.blur_lead).max(0.03)
            };
            item.blur
                .to(if on { 0.0 } else { 1.0 }, spring(blur_duration, 0.0));
            item.blur.step(dt);
            if reduced {
                item.x.snap(item.x.target);
                item.travelling = false;
            } else {
                item.x.step(dt);
                // Arrived (the sides may still hold it a hair inside), or leaving: then the edge
                // cap governs it like anything else that goes.
                if item.travelling
                    && (!item.in_target
                        || (item.x.x - item.x.target).abs() < 0.05 && item.x.v.abs() < 1.0)
                {
                    item.travelling = false;
                }
            }
            // Settled once it shows nearly in full with full room (so dropping the room limit
            // changes nothing): from then on the sides contain it instead.
            if item.in_target
                && item.a.target >= 1.0
                && item.a.x >= 0.97
                && item.cap >= 0.999
                && d >= tune.clear + tune.band - 1e-3
            {
                item.settled = true;
            }
            if !item.in_target || item.swap.is_some() {
                item.settled = false;
                item.kept = false;
            }
            // The edge pushes what stays along instead of cutting it (inelastic: no bounce).
            if !reduced && item.in_target && (item.settled || item.travelling) {
                Self::contain(item, &shape, tune);
            }
            // The edge cap for what comes and goes: gone exactly when the edge arrives.
            if !reduced && !item.settled && !item.travelling {
                let hidden = item.a.x <= 1e-3;
                if d < tune.clear {
                    item.cap = if hidden { 1.0 } else { 0.0 };
                } else if hidden {
                    item.cap = 1.0;
                } else {
                    let contact = ahead
                        .iter()
                        .find(|(_, sh)| {
                            let x = item.drawn_x(tune, reduced);
                            clearance(sh, x, item.spec.w, item.spec.ink) < tune.clear
                        })
                        .map(|(t, _)| *t);
                    match contact {
                        Some(c) => item.cap -= item.cap * (dt / c.max(dt)).min(1.0),
                        None => item.cap += dt / tune.exit_lead.max(0.01),
                    }
                }
                item.cap = item.cap.clamp(0.0, 1.0);
                if item.cap < 1e-4 {
                    item.cap = 0.0;
                }
            } else {
                item.cap = 1.0;
            }
        }
        self.items
            .retain(|i| i.in_target || i.a.x > 1e-3 || i.a.target > 0.0);
    }

    /// Keeps an item whole inside the outline's sides with full room: pushes it toward the
    /// centre and drops its outward velocity. Only the sides push; a squish's flattening never
    /// shoves anything sideways.
    fn contain(item: &mut Item, shape: &Shape, tune: &Tune) {
        let margin = tune.clear + tune.band;
        let h = shape.height().max(ISLAND_H);
        let sides = Shape {
            b: shape.t + h,
            radius: radius_of(h, MAX_RADIUS, tune.radius_blend),
            ..*shape
        };
        for _ in 0..3 {
            let x = item.drawn_x(tune, false);
            let d = clearance(&sides, x, item.spec.w, item.spec.ink);
            if d >= margin - 1e-4 {
                return;
            }
            let dir = item.centre_sign();
            if dir == 0.0 {
                return;
            }
            item.x.x -= (margin - d) * dir;
            if item.x.v * dir > 0.0 {
                item.x.v = 0.0;
            }
        }
    }

    pub fn frame(&self, tune: &Tune) -> Frame {
        let reduced = self.reduced;
        let shape = self.shape(tune);
        let ghost = (reduced && self.dis < 1.0)
            .then_some(self.ghost)
            .flatten()
            .map(|g| (g, 1.0 - self.dis));
        let mut items = Vec::with_capacity(self.items.len());
        let mut whole = true;
        let mut unsettled = false;
        let (mut blur_sum, mut alpha_sum, mut max_a) = (0.0f32, 0.0f32, 0.0f32);
        for item in &self.items {
            let x = item.drawn_x(tune, reduced);
            let d = clearance(&shape, x, item.spec.w, item.spec.ink);
            let a = item.a.x.clamp(0.0, 1.0);
            let allow = if reduced {
                if d >= tune.clear {
                    1.0
                } else {
                    ghost
                        .filter(|(g, _)| clearance(g, x, item.spec.w, item.spec.ink) >= tune.clear)
                        .map_or(0.0, |(_, ga)| ga)
                }
            } else if item.settled || item.travelling {
                1.0
            } else {
                item.cap.min(Self::room(d, tune))
            };
            let alpha = a.min(allow);
            if alpha > 0.01 {
                if item.kept {
                    whole = false;
                }
                if !item.settled {
                    unsettled = true;
                }
                let blur = item
                    .blur
                    .x
                    .clamp(0.0, 1.0)
                    .max(((1.0 - item.cap) / tune.blur_lead.max(0.05)).clamp(0.0, 1.0));
                blur_sum += alpha * blur;
                alpha_sum += alpha;
                max_a = max_a.max(a);
            }
            items.push(ItemFrame {
                key: item.spec.key.clone(),
                x,
                w: item.spec.w,
                alpha,
                live: item.in_target && alpha > 0.3,
            });
        }
        let changing = whole && unsettled && alpha_sum > 0.0 && !reduced;
        Frame {
            shape,
            shell_alpha: self.shell_alpha(),
            ghost,
            items,
            content_blur: if changing {
                tune.content_blur * blur_sum / alpha_sum
            } else {
                0.0
            },
            content_scale: if changing {
                mix(tune.content_scale, 1.0, max_a)
            } else {
                1.0
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tests: the model.test.js checks that carry over to one axis.

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 120.0;

    fn presets() -> Vec<(&'static str, Tune)> {
        super::super::motion::PRESETS
            .iter()
            .map(|(name, tune)| (*name, *tune))
            .collect()
    }

    fn recording(chips: bool) -> Vec<Spec> {
        let mut v = vec![
            Spec::new(
                "lead",
                CHIP_H + 34.0 + 8.0,
                Ink {
                    left: CHIP_H / 2.0 - 4.0,
                    right: 8.0,
                    half_h: 7.0,
                    round: 4.0,
                },
            ),
            divider("divider.lead"),
        ];
        if chips {
            v.extend([
                Spec::chip("chip:Area", 66.0),
                Spec::chip("chip:Window", 84.0),
                Spec::chip("chip:Screen", 80.0),
            ]);
        } else {
            v.extend([Spec::chip("undo", 52.0), Spec::chip("finish", 60.0)]);
        }
        v.extend([divider("divider.close"), Spec::chip("close", CHIP_H)]);
        v
    }

    fn picker(record: bool) -> Vec<Spec> {
        let mut v = vec![
            Spec::chip("chip:Area", 66.0),
            Spec::chip("chip:Window", 84.0),
            Spec::chip("chip:Screen", 80.0),
        ];
        if record {
            v.push(Spec::chip("record", 70.0));
        }
        v.extend([divider("divider.close"), Spec::chip("close", CHIP_H)]);
        v
    }

    fn processing() -> Vec<Spec> {
        vec![Spec::new(
            "processing",
            CHIP_H + 6.0 + 78.0 + 12.0,
            Ink {
                left: 6.0,
                right: 12.0,
                half_h: 9.0,
                round: 9.0,
            },
        )]
    }

    fn done() -> Vec<Spec> {
        vec![
            Spec::new(
                "done.lead",
                CHIP_H + 8.0 + 40.0 + 8.0,
                Ink {
                    left: 0.0,
                    right: 8.0,
                    half_h: CHIP_H / 2.0,
                    round: CHIP_H / 2.0,
                },
            ),
            divider("divider.lead"),
            Spec::chip("paste", 60.0),
            Spec::chip("inspect", 64.0),
            Spec::chip("close", CHIP_H),
        ]
    }

    fn divider(key: &str) -> Spec {
        Spec::new(
            key,
            13.0,
            Ink {
                left: 6.0,
                right: 6.0,
                half_h: 8.0,
                round: 0.5,
            },
        )
    }

    /// Steps `secs` presenting `specs` each frame (as the capsule does), checking each frame.
    fn run(
        island: &mut Island,
        tune: &Tune,
        specs: Option<&[Spec]>,
        secs: f32,
        mut check: impl FnMut(&Island, &Frame),
    ) {
        let mut t = 0.0;
        while t < secs {
            island.step(FRAME, tune);
            if let Some(specs) = specs {
                island.present(specs, tune);
            }
            let frame = island.frame(tune);
            check(island, &frame);
            t += FRAME;
        }
    }

    /// Nothing visible is cut: every item that shows is inside the outline (never sliced), and
    /// shows no more than the room the edge leaves it.
    fn no_cut(island: &Island, frame: &Frame, tune: &Tune, what: &str) {
        for (item, f) in island.items.iter().zip(&frame.items) {
            if f.alpha <= 0.02 {
                continue;
            }
            let d = clearance(&frame.shape, f.x, item.spec.w, item.spec.ink);
            if island.reduced {
                continue;
            }
            assert!(d >= 0.0, "{what}: {} sliced (d {d:.2}, alpha {:.2})", f.key, f.alpha);
            if item.settled || item.travelling {
                continue; // contained instead: the sides push it along
            }
            let room = Island::room(d, tune);
            assert!(
                f.alpha <= room + 0.02,
                "{what}: {} alpha {:.3} over room {room:.3} (d {d:.2})",
                f.key,
                f.alpha
            );
        }
    }

    fn opened(tune: &Tune, specs: &[Spec]) -> Island {
        let mut island = Island::new();
        island.open();
        island.present(specs, tune);
        run(&mut island, tune, Some(specs), 1.5, |_, _| {});
        island
    }

    #[test]
    fn springs_keep_velocity_on_retarget_and_settle() {
        let mut s = S1::at(0.0, spring(0.5, 0.28));
        s.to(100.0, spring(0.5, 0.28));
        for _ in 0..12 {
            s.step(FRAME);
        }
        let (x, v) = (s.x, s.v);
        s.to(-50.0, spring(0.45, 0.18));
        assert_eq!((s.x, s.v), (x, v), "a retarget changes the target only");
        s.step(FRAME);
        assert!(s.x > x, "the next frame continues the same direction");
        for _ in 0..600 {
            s.step(FRAME);
        }
        assert_eq!(s.x, -50.0);
    }

    #[test]
    fn the_radius_is_smooth_and_a_capsule_at_rest() {
        let blend = 8.0;
        assert_eq!(radius_of(ISLAND_H, MAX_RADIUS, blend), ISLAND_H / 2.0);
        let mut prev = radius_of(0.0, MAX_RADIUS, blend);
        let mut prev_slope = 0.5;
        let mut h = 0.25;
        while h < 120.0 {
            let r = radius_of(h, MAX_RADIUS, blend);
            let slope = (r - prev) / 0.25;
            assert!(r >= prev - 1e-5, "monotone at {h}");
            assert!((slope - prev_slope).abs() < 0.02, "no kink at {h}: {prev_slope} → {slope}");
            prev = r;
            prev_slope = slope;
            h += 0.25;
        }
        assert!((radius_of(120.0, MAX_RADIUS, blend) - MAX_RADIUS).abs() < 1e-5);
    }

    #[test]
    fn layouts_fit_and_rest_with_full_room() {
        for (name, tune) in presets() {
            for specs in [recording(true), recording(false), picker(true), processing(), done()] {
                let island = opened(&tune, &specs);
                let frame = island.frame(&tune);
                assert!((frame.shape.width() - row_width(&specs)).abs() < 0.05, "{name}");
                for f in &frame.items {
                    assert!((f.alpha - 1.0).abs() < 1e-3, "{name}: {} rests at {}", f.key, f.alpha);
                }
                assert_eq!(frame.content_blur, 0.0);
                assert_eq!(frame.content_scale, 1.0);
            }
        }
    }

    #[test]
    fn nothing_visible_is_cut_through_every_change() {
        let flows: Vec<(&str, Vec<Vec<Spec>>)> = vec![
            ("open recording", vec![recording(true)]),
            ("chips fold", vec![recording(true), recording(false)]),
            ("chip to picker", vec![recording(true), picker(false)]),
            ("picker record", vec![picker(false), picker(true)]),
            ("finish", vec![recording(false), processing(), done()]),
            ("picker to recording", vec![picker(false), recording(false)]),
        ];
        for (name, tune) in presets() {
            for (flow, steps) in &flows {
                let mut island = Island::new();
                island.open();
                for specs in steps {
                    run(&mut island, &tune, Some(specs), 1.2, |i, f| {
                        no_cut(i, f, &tune, &format!("{name}/{flow}"))
                    });
                }
                island.dismiss(&tune);
                run(&mut island, &tune, None, 1.5, |i, f| {
                    no_cut(i, f, &tune, &format!("{name}/{flow}/dismiss"))
                });
                assert!(island.is_landed(), "{name}/{flow} lands");
            }
        }
    }

    #[test]
    fn every_change_fired_mid_flight_of_every_other() {
        let layouts = [recording(true), recording(false), picker(false), processing(), done()];
        for (name, tune) in presets() {
            for a in 0..layouts.len() {
                for b in 0..layouts.len() {
                    for at in [0.03, 0.09, 0.16, 0.27] {
                        let mut island = Island::new();
                        island.open();
                        let mut last_w = None::<(f32, f32)>;
                        let mut check = |i: &Island, f: &Frame| {
                            no_cut(i, f, &tune, &format!("{name} {a}→{b}@{at}"));
                            // The outline never jumps: its edge moves at most what its spring allows.
                            if let Some((w, _)) = last_w {
                                assert!((f.shape.width() - w).abs() < 30.0, "{name} jump");
                            }
                            last_w = Some((f.shape.width(), 0.0));
                            for item in &f.items {
                                assert!(item.x.is_finite() && item.alpha.is_finite());
                            }
                        };
                        run(&mut island, &tune, Some(&layouts[a]), at, &mut check);
                        let (w, v) = (island.w.x, island.w.v);
                        island.present(&layouts[b], &tune);
                        assert_eq!((island.w.x, island.w.v), (w, v), "C1 at the retarget");
                        run(&mut island, &tune, Some(&layouts[b]), 2.0, &mut check);
                        let frame = island.frame(&tune);
                        assert!(
                            (frame.shape.width() - row_width(&layouts[b])).abs() < 0.05,
                            "{name} {a}→{b}@{at} settles on the new width"
                        );
                        assert_eq!(frame.items.len(), layouts[b].len());
                        assert!(frame.items.iter().all(|f| (f.alpha - 1.0).abs() < 1e-3));
                    }
                }
            }
        }
    }

    #[test]
    fn shared_items_travel_and_newcomers_wait_for_room() {
        let tune = Tune::DEFAULT;
        let mut island = opened(&tune, &recording(true));
        let start = island.items.iter().find(|i| i.spec.key == "chip:Area").unwrap().x.x;
        island.present(&picker(false), &tune);
        let mut xs = Vec::new();
        let mut lead_gone_at = None;
        let mut width_moved_at = None;
        let w0 = island.w.x;
        let mut t = 0.0;
        run(&mut island, &tune, Some(&picker(false)), 0.8, |i, f| {
            t += FRAME;
            let area = f.items.iter().find(|f| f.key == "chip:Area").unwrap();
            assert!(area.alpha > 0.99, "the chip stays: it is the same element");
            xs.push(area.x);
            if lead_gone_at.is_none()
                && f.items.iter().find(|f| f.key == "lead").is_none_or(|l| l.alpha < 0.05)
            {
                lead_gone_at = Some(t);
            }
            if width_moved_at.is_none() && (i.w.x - w0).abs() > 0.5 {
                width_moved_at = Some(t);
            }
        });
        // It travels: no step over 6 pt a frame at 120 Hz, and it lands where the picker puts it.
        let mut prev = start;
        for x in &xs {
            assert!((x - prev).abs() < 6.0, "{prev} → {x}");
            prev = *x;
        }
        assert!((xs.last().unwrap() - row_lefts(&picker(false))[0]).abs() < 0.05);
        // Apple's collapse: the lead is mostly gone before the outline moves.
        let (gone, moved) = (lead_gone_at.unwrap(), width_moved_at.unwrap());
        assert!(moved >= tune.collapse_wait - 1e-3, "the outline waits ({moved})");
        assert!(gone <= moved + 0.1, "content first ({gone} vs {moved})");
    }

    #[test]
    fn opening_content_follows_half_the_growth_inner_first() {
        let tune = Tune::DEFAULT;
        let mut island = Island::new();
        island.open();
        let specs = recording(true);
        island.present(&specs, &tune);
        let mut first: Vec<(String, f32)> = Vec::new();
        let mut t = 0.0;
        let target = row_width(&specs);
        run(&mut island, &tune, Some(&specs), 1.2, |i, f| {
            t += FRAME;
            for item in &f.items {
                if item.alpha > 0.02 && !first.iter().any(|(k, _)| k == &item.key) {
                    let grown = (i.w.x - LAND_W) / (target - LAND_W);
                    assert!(grown >= tune.content_delay - 0.02, "{} at {grown}", item.key);
                    first.push((item.key.clone(), t));
                }
            }
        });
        assert_eq!(first.len(), specs.len());
        let at = |k: &str| first.iter().find(|(key, _)| key == k).unwrap().1;
        assert!(at("chip:Window") <= at("close"), "the inner chip comes before the end");
    }

    #[test]
    fn the_outline_bounces_but_content_never_overshoots() {
        let tune = Tune::DEFAULT;
        assert!(tune.open.bounce > 0.0);
        let mut island = Island::new();
        island.open();
        let specs = recording(true);
        island.present(&specs, &tune);
        let target = row_width(&specs);
        let (mut over, mut alpha, mut h_over) = (0.0f32, 0.0f32, 0.0f32);
        run(&mut island, &tune, Some(&specs), 1.5, |i, f| {
            over = over.max(f.shape.width() - target);
            h_over = h_over.max(i.h.x - ISLAND_H);
            for item in &f.items {
                alpha = alpha.max(item.alpha);
            }
        });
        assert!(over > 0.5, "the width does bounce");
        assert!(h_over < 1e-3, "the height doesn't: no overshoot up into the menu bar");
        assert!(alpha <= 1.0);
    }

    #[test]
    fn blur_leads_opacity() {
        let tune = Tune::DEFAULT;
        let mut island = Island::new();
        island.open();
        let specs = processing();
        island.present(&specs, &tune);
        run(&mut island, &tune, Some(&specs), 1.0, |i, _| {
            for item in &i.items {
                if item.a.x >= 0.95 && item.a.target == 1.0 {
                    assert!(item.blur.x < 0.05, "sharp before opaque");
                }
            }
        });
    }

    #[test]
    fn reduce_motion_dissolves_without_travel() {
        let tune = Tune::DEFAULT;
        let mut island = Island::new();
        island.reduced = true;
        island.open();
        island.present(&recording(true), &tune);
        run(&mut island, &tune, Some(&recording(true)), 0.5, |_, _| {});
        let w0 = island.w.x;
        island.present(&picker(false), &tune);
        island.step(FRAME, &tune);
        let f = island.frame(&tune);
        assert_eq!(f.shape.width(), row_width(&picker(false)), "no width travel");
        assert!(f.ghost.is_some_and(|(g, a)| (g.width() - w0).abs() < 1e-3 && a > 0.9));
        assert_eq!(f.content_blur, 0.0);
        run(&mut island, &tune, Some(&picker(false)), 0.5, |_, f| {
            for item in &f.items {
                let x = row_lefts(&picker(false));
                let _ = x;
                assert!(item.x.is_finite());
            }
        });
        assert!(island.frame(&tune).ghost.is_none());
    }

    #[test]
    fn a_squish_never_cuts_or_dims_what_stays() {
        let tune = Tune::DEFAULT;
        let mut island = opened(&tune, &recording(true));
        island.kick(&tune);
        island.kick(&tune);
        run(&mut island, &tune, Some(&recording(true)), 1.0, |i, f| {
            no_cut(i, f, &tune, "squish");
            for item in &f.items {
                assert!(item.alpha > 0.99, "{} dims to {}", item.key, item.alpha);
            }
        });
    }
}
