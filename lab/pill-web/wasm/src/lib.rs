//! The Metal pill's motion model (`src/native/pill/model.rs`), compiled unchanged to WebAssembly.
//!
//! `model.rs` is included by path, so the web playground and the Mac app run the same springs,
//! layout, gating and look-ahead code. Its only outside dependency is `crate::ui::SelectionMode`,
//! which `ui` below mirrors (`src/ui.rs`, macOS behaviour: iPhone is offered when cabled).
//!
//! The exports are a plain C ABI (no wasm-bindgen): JS calls `pill_*` and reads the frame from a
//! static `f32` buffer whose layout `FRAME_*` describes.

#[path = "../../../../src/native/pill/model.rs"]
pub mod model;

pub mod ui {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum SelectionMode {
        Area,
        Window,
        Screen,
        Iphone,
    }

    impl SelectionMode {
        const ALL: [SelectionMode; 4] = [Self::Area, Self::Window, Self::Screen, Self::Iphone];

        pub fn available(has_iphone: bool) -> &'static [SelectionMode] {
            if has_iphone { &Self::ALL } else { &Self::ALL[..3] }
        }

        pub fn shows_record(self) -> bool {
            matches!(self, Self::Screen | Self::Iphone)
        }

        pub fn from_index(i: u32) -> Self {
            Self::ALL[(i as usize).min(3)]
        }
    }
}

use std::cell::RefCell;

use model::{ElemId, Metrics, Origin, PillModel, PillOpts, Pres, SpringCfg};
use ui::SelectionMode;

/// Header floats before the elements.
pub const FRAME_HEADER: usize = 32;
/// Floats per element.
pub const FRAME_ELEM: usize = 14;
const FRAME_LEN: usize = FRAME_HEADER + FRAME_ELEM * ElemId::COUNT;

struct State {
    model: PillModel,
    metrics: Metrics,
    frame: [f32; FRAME_LEN],
    springs: [f32; 16],
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(|slot| {
        let st = slot.get_or_insert_with(|| State {
            model: PillModel::new(Metrics::default()),
            metrics: Metrics::default(),
            frame: [0.0; FRAME_LEN],
            springs: [0.0; 16],
        });
        f(st)
    })
}

const COMPLETED_LABELS: [&str; 4] = ["Copied", "Sent", "Saved", "Queued"];

fn pres_of(kind: u32, mode: u32, flag: u32, has_iphone: u32, n: u32) -> Pres {
    match kind {
        1 => Pres::Recording {
            chips: flag != 0,
            has_iphone: has_iphone != 0,
            stroke_count: n as usize,
        },
        2 => Pres::Selecting {
            mode: SelectionMode::from_index(mode),
            hold: flag != 0,
            has_iphone: has_iphone != 0,
        },
        3 => Pres::Processing,
        4 => Pres::Completed {
            label: COMPLETED_LABELS[(n as usize).min(COMPLETED_LABELS.len() - 1)].to_owned(),
        },
        _ => Pres::Hidden,
    }
}

/// A fresh model (time 0, hidden) with the last metrics and the given options kept.
#[unsafe(no_mangle)]
pub extern "C" fn pill_reset() {
    with(|st| {
        let opts = st.model.opts;
        let reduced = st.model.reduced;
        st.model = PillModel::new(st.metrics.clone());
        st.model.opts = opts;
        st.model.reduced = reduced;
    });
}

/// Label widths measured in the browser (13 pt SF Pro), in `Metrics` field order.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn pill_set_metrics(
    area: f32,
    window: f32,
    screen: f32,
    iphone: f32,
    record: f32,
    undo: f32,
    cancel: f32,
    finish: f32,
    processing: f32,
    paste: f32,
    inspect: f32,
    clock_000: f32,
    clock_0000: f32,
) {
    with(|st| {
        st.metrics = Metrics {
            area,
            window,
            screen,
            iphone,
            record,
            undo,
            cancel,
            finish,
            processing,
            paste,
            inspect,
            clock_000,
            clock_0000,
        };
        st.model.metrics = st.metrics.clone();
    });
}

/// `PillOpts` from `Tune::pill_opts` (`origin` 0 centre, 1 top, 2 lead).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn pill_set_opts(
    origin: u32,
    hold_chips: u32,
    content_blur: f32,
    shell_blur: f32,
    enter_d: f32,
    enter_b: f32,
    exit_d: f32,
    exit_b: f32,
    open_d: f32,
    open_b: f32,
    close_d: f32,
    close_b: f32,
    lead_d: f32,
    lead_b: f32,
    trail_d: f32,
    trail_b: f32,
    punch: f32,
) {
    let cfg = |d: f32, b: f32| SpringCfg::new(d.max(0.02), b.clamp(0.0, 0.95));
    let opts = PillOpts {
        origin: match origin {
            0 => Origin::Centre,
            2 => Origin::Lead,
            _ => Origin::Top,
        },
        hold_chips: hold_chips != 0,
        content_blur,
        shell_blur,
        enter: cfg(enter_d, enter_b),
        exit: cfg(exit_d, exit_b),
        morph_open: cfg(open_d, open_b),
        morph_close: cfg(close_d, close_b),
        highlight_lead: cfg(lead_d, lead_b),
        highlight_trail: cfg(trail_d, trail_b),
        punch: punch.max(0.0),
    };
    with(|st| st.model.set_opts(opts));
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_set_reduced(on: u32) {
    with(|st| st.model.reduced = on != 0);
}

/// `kind`: 0 hidden, 1 recording (`flag` = chips, `n` = strokes), 2 selecting (`mode` 0–3,
/// `flag` = hold), 3 processing, 4 completed (`n` = label index).
#[unsafe(no_mangle)]
pub extern "C" fn pill_set_pres(kind: u32, mode: u32, flag: u32, has_iphone: u32, n: u32) {
    with(|st| st.model.set_pres(pres_of(kind, mode, flag, has_iphone, n)));
}

/// The completed label's measured width (pt) for label index `n`.
#[unsafe(no_mangle)]
pub extern "C" fn pill_set_completed_label(n: u32, width: f32) {
    let label = COMPLETED_LABELS[(n as usize).min(COMPLETED_LABELS.len() - 1)].to_owned();
    with(|st| st.model.set_completed_label(label, width));
}

/// The recording clock, `M:SS` from whole seconds (`10:00` and up is the wider layout).
#[unsafe(no_mangle)]
pub extern "C" fn pill_set_clock(secs: u32) {
    let clock = format!("{}:{:02}", secs / 60, secs % 60);
    with(|st| st.model.set_clock(&clock));
}

/// Pointer in panel points; `has` 0 when it's off the capsule.
#[unsafe(no_mangle)]
pub extern "C" fn pill_set_pointer(x: f32, y: f32, has: u32, down: u32) {
    with(|st| {
        let pos = (has != 0).then_some([x, y]);
        st.model.set_pointer(pos, down != 0);
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_hovered() -> i32 {
    with(|st| st.model.hovered.map_or(-1, |id| id as i32))
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_hit_element(x: f32, y: f32) -> i32 {
    with(|st| st.model.hit_element(x, y).map_or(-1, |id| id as i32))
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_hit_island(x: f32, y: f32) -> u32 {
    with(|st| u32::from(st.model.hit_island(x, y)))
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_advance(dt: f32) {
    with(|st| st.model.advance(dt));
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_is_at_rest() -> u32 {
    with(|st| u32::from(st.model.is_at_rest()))
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_is_fully_hidden() -> u32 {
    with(|st| u32::from(st.model.is_fully_hidden()))
}

#[unsafe(no_mangle)]
pub extern "C" fn pill_time() -> f32 {
    with(|st| st.model.t)
}

/// Writes `model.frame()` into the frame buffer and returns its address.
///
/// Header: `0 t`, `1..8 shape l t r b radius alpha blur`, `8 is_completed`, `9 content_blur`,
/// `10 content_alpha`, `11 has_highlight`, `12..18 highlight x y w h alpha blur`, `18 has_ghost`,
/// `19..25 ghost l t r b radius alpha`, `25 elem count`, `26 hovered id (-1)`, `27 pressed id`,
/// `28 at rest`. Each element: `id x y w h base_w base_h alpha blur scale active hover press enabled`.
#[unsafe(no_mangle)]
pub extern "C" fn pill_frame() -> *const f32 {
    with(|st| {
        let f = st.model.frame();
        let b = &mut st.frame;
        b.fill(0.0);
        b[0] = f.t;
        let s = f.shape;
        b[1..8].copy_from_slice(&[s.l, s.t, s.r, s.b, s.radius, s.alpha, s.blur]);
        b[8] = f32::from(u8::from(f.is_completed));
        b[9] = f.content_blur;
        b[10] = f.content_alpha;
        if let Some(h) = f.highlight {
            b[11] = 1.0;
            b[12..18].copy_from_slice(&[h.rect[0], h.rect[1], h.rect[2], h.rect[3], h.alpha, h.blur]);
        }
        if let Some(g) = f.ghost {
            b[18] = 1.0;
            b[19..25].copy_from_slice(&[g.l, g.t, g.r, g.b, g.radius, g.alpha]);
        }
        b[25] = f.elems.len() as f32;
        b[26] = st.model.hovered.map_or(-1.0, |id| id as usize as f32);
        b[27] = st.model.pressed.map_or(-1.0, |id| id as usize as f32);
        b[28] = f32::from(u8::from(st.model.is_at_rest()));
        for (i, e) in f.elems.iter().take(ElemId::COUNT).enumerate() {
            let o = FRAME_HEADER + i * FRAME_ELEM;
            b[o..o + FRAME_ELEM].copy_from_slice(&[
                e.id as usize as f32,
                e.rect[0],
                e.rect[1],
                e.rect[2],
                e.rect[3],
                e.base_size[0],
                e.base_size[1],
                e.alpha,
                e.blur,
                e.scale,
                e.active,
                e.hover,
                e.press,
                f32::from(u8::from(e.enabled)),
            ]);
        }
        b.as_ptr()
    })
}

/// The outline's raw springs for plotting: `l r b grow shell_alpha shell_blur press squish`
/// as position then velocity pairs.
#[unsafe(no_mangle)]
pub extern "C" fn pill_springs() -> *const f32 {
    with(|st| {
        let m = &st.model;
        let s = [m.l, m.r, m.b, m.grow, m.shell_alpha, m.shell_blur, m.press, m.squish];
        for (i, v) in s.iter().enumerate() {
            st.springs[i * 2] = v.x;
            st.springs[i * 2 + 1] = v.v;
        }
        st.springs.as_ptr()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wasm build is the same code: settle the finish scene and check the resting capsule.
    #[test]
    fn same_model_settles() {
        pill_reset();
        pill_set_pres(1, 0, 1, 0, 0);
        for _ in 0..240 {
            pill_advance(1.0 / 120.0);
        }
        assert_eq!(pill_is_at_rest(), 1);
        let f = unsafe { std::slice::from_raw_parts(pill_frame(), FRAME_LEN) };
        assert_eq!(f[4] - f[2], model::ISLAND_H);
    }
}
