//! The capsule's motion plumbing: one animation clock that can run slow, springs and durations
//! that can be tuned live from `~/.config/scrawl/tune.json` (debug builds), and a per-frame trace
//! in motion-lint's format (`SCRAWL_MOTION_TRACE=<file>`). Each costs nothing when it's off.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::Spring;

// ---------------------------------------------------------------------------------------------
// Clock

struct Clock {
    anchor: Instant,
    /// Animation seconds at `anchor`.
    base: f64,
    scale: f64,
}

thread_local! {
    static CLOCK: RefCell<Clock> = RefCell::new(Clock { anchor: Instant::now(), base: 0.0, scale: 1.0 });
}

/// Animation time in seconds. Every capsule age reads this, so a time scale slows them all.
pub(super) fn now() -> f64 {
    CLOCK.with_borrow(|c| c.base + c.anchor.elapsed().as_secs_f64() * c.scale)
}

/// Animation seconds since `since` (a `now()` reading).
pub(super) fn age(since: f64) -> f32 {
    (now() - since).max(0.0) as f32
}

pub(super) fn time_scale() -> f32 {
    CLOCK.with_borrow(|c| c.scale as f32)
}

/// Re-anchors so ages already running continue from where they are.
fn set_time_scale(scale: f64) {
    CLOCK.with_borrow_mut(|c| {
        let scale = scale.clamp(0.02, 4.0);
        if (scale - c.scale).abs() < 1e-9 {
            return;
        }
        c.base += c.anchor.elapsed().as_secs_f64() * c.scale;
        c.anchor = Instant::now();
        c.scale = scale;
    });
}

// ---------------------------------------------------------------------------------------------
// Tuning

/// Every value Kit judges by eye, for the capsule's island model (`super::island`). `DEFAULT` is
/// what ships (Kit Motion); `tune.json` overrides it live. Keys follow the KIT MOTION study's
/// `island.*` TuningStore names where one exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Tune {
    /// The outline growing (any change to a larger area), and shrinking.
    pub open: Spring,
    pub close: Spring,
    /// Opening, the width's spring is this much shorter; closing, this much longer (the height
    /// leads, so the sides close in last).
    pub width_lead: f32,
    pub height_lead: f32,
    /// The radius eases into its cap over this many points (0 = a hard minimum).
    pub radius_blend: f32,
    /// The tactile squish on a mode change: its spring and how far it widens (× width).
    pub squish: Spring,
    pub squish_amount: f32,
    /// Closing over leaving content, the outline waits this long for it.
    pub collapse_wait: f32,
    /// New content waits for this share of the outline's growth.
    pub content_delay: f32,
    /// Content fades in and out (zero-bounce springs, seconds).
    pub content_in: f32,
    pub content_out: f32,
    /// Coming and going together, content blurs from this many points and scales from this.
    pub content_blur: f32,
    pub content_scale: f32,
    /// Blur settles within this share of the fade.
    pub blur_lead: f32,
    /// Items follow each other outward by this much, travelling `content_push` points.
    pub stagger: f32,
    pub content_push: f32,
    /// Content clears the edge this far ahead of it (seconds of look-ahead).
    pub exit_lead: f32,
    /// Nothing shows within `clear` pt of the edge; it fades in over the next `band` pt.
    pub clear: f32,
    pub band: f32,
    /// Items in both layouts travel on this spring (`shared_travel` 1), else fade out and back
    /// (Native's swap, 0).
    pub shared: Spring,
    pub shared_travel: f32,
    /// Reduce Motion's dissolve.
    pub reduced_fade: f32,
    pub time_scale: f32,
}

const fn s(duration: f32, bounce: f32) -> Spring {
    Spring::visual(duration, bounce)
}

impl Tune {
    /// Apple's system island as the study reads it (`NATIVE` in model.js): outline .5/.28 open,
    /// .45/.18 close, content after half the growth, in .3, out .15 from 8 pt blur and .82
    /// scale, the outline waiting .1 s for content, shared items swapping. The pill's 4 pt
    /// inset sets `clear` and `band`: full room 1 pt short of the resting inset, so a row as
    /// tall as the pill shows before the height's zero-bounce tail ends (the phone's 2 + 3 pt
    /// would never reach full room at rest).
    pub const NATIVE: Tune = Tune {
        open: s(0.5, 0.28),
        close: s(0.45, 0.18),
        width_lead: 0.0,
        height_lead: 0.0,
        radius_blend: 0.0,
        squish: s(0.35, 0.45),
        squish_amount: 0.03,
        collapse_wait: 0.1,
        content_delay: 0.5,
        content_in: 0.3,
        content_out: 0.15,
        content_blur: 8.0,
        content_scale: 0.82,
        blur_lead: 0.7,
        stagger: 0.0,
        content_push: 0.0,
        exit_lead: 0.08,
        clear: 1.0,
        band: 2.0,
        shared: s(0.4, 0.0),
        shared_travel: 0.0,
        reduced_fade: 0.2,
        time_scale: 1.0,
    };

    /// Kit Motion: Native's timings plus what Kit adds: content clears the edge .1 s ahead, blur
    /// resolves within .55 of the fade, items .013 s apart travelling 1.75 pt outward, shared
    /// items travel on .24 s zero bounce, the width leads opening and the height leads
    /// closing, and a smooth radius.
    pub const KIT_MOTION: Tune = Tune {
        width_lead: 0.04,
        height_lead: 0.04,
        radius_blend: 8.0,
        blur_lead: 0.55,
        stagger: 0.013,
        content_push: 1.75,
        exit_lead: 0.1,
        shared: s(0.24, 0.0),
        shared_travel: 1.0,
        ..Self::NATIVE
    };

    /// Kit Motion with no bounce anywhere.
    pub const STILL: Tune = Tune {
        open: s(0.5, 0.0),
        close: s(0.45, 0.0),
        squish: s(0.35, 0.0),
        ..Self::KIT_MOTION
    };

    /// The model run with today's (pre-island) values: the morph palette's springs, the chips'
    /// short fades and light blur, content as soon as there is room, no wait.
    pub const TODAY: Tune = Tune {
        open: s(0.36, 0.16),
        close: s(0.32, 0.08),
        width_lead: 0.0,
        height_lead: 0.0,
        radius_blend: 0.0,
        squish: s(0.22, 0.0),
        squish_amount: 0.03,
        collapse_wait: 0.0,
        content_delay: 0.0,
        content_in: 0.16,
        content_out: 0.13,
        content_blur: 2.5,
        content_scale: 1.0,
        blur_lead: 1.0,
        stagger: 0.0,
        content_push: 0.0,
        exit_lead: 0.08,
        clear: 1.0,
        band: 2.0,
        shared: s(0.36, 0.0),
        shared_travel: 1.0,
        reduced_fade: 0.18,
        time_scale: 1.0,
    };

    pub const DEFAULT: Tune = Self::KIT_MOTION;
}

type Field = fn(&mut Tune) -> &mut f32;

/// `tune.json` keys, in the order `scrawl tune list` prints them.
pub(super) const KEYS: &[(&str, &str, Field)] = &[
    ("open.duration", "Outline grows (s)", |t| {
        &mut t.open.response
    }),
    ("open.bounce", "Outline grows: bounce (width only)", |t| {
        &mut t.open.bounce
    }),
    ("close.duration", "Outline shrinks (s)", |t| {
        &mut t.close.response
    }),
    (
        "close.bounce",
        "Outline shrinks: bounce (width only)",
        |t| &mut t.close.bounce,
    ),
    ("widthLead", "Width leads the growth by (s)", |t| {
        &mut t.width_lead
    }),
    ("heightLead", "Height leads the collapse by (s)", |t| {
        &mut t.height_lead
    }),
    ("radius.blend", "Radius eases into its cap over (pt)", |t| {
        &mut t.radius_blend
    }),
    ("tap.duration", "Squish spring (s)", |t| {
        &mut t.squish.response
    }),
    ("tap.bounce", "Squish bounce", |t| &mut t.squish.bounce),
    ("tap.squish", "Squish amount (× width)", |t| {
        &mut t.squish_amount
    }),
    (
        "collapse.wait",
        "Outline waits for leaving content (s)",
        |t| &mut t.collapse_wait,
    ),
    (
        "content.delay",
        "Content waits for this share of the growth",
        |t| &mut t.content_delay,
    ),
    ("content.in", "Content fades in (s)", |t| &mut t.content_in),
    ("content.out", "Content fades out (s)", |t| {
        &mut t.content_out
    }),
    (
        "content.blur",
        "Whole-content blur coming and going (pt)",
        |t| &mut t.content_blur,
    ),
    (
        "content.scale",
        "Whole-content scale coming and going",
        |t| &mut t.content_scale,
    ),
    ("content.blurLead", "Blur settles within (× fade)", |t| {
        &mut t.blur_lead
    }),
    (
        "content.stagger",
        "Items follow each other outward by (s)",
        |t| &mut t.stagger,
    ),
    (
        "content.push",
        "Items arrive travelling outward by (pt)",
        |t| &mut t.content_push,
    ),
    (
        "content.exitLead",
        "Content clears the edge ahead by (s)",
        |t| &mut t.exit_lead,
    ),
    (
        "content.clear",
        "Nothing within this of the edge (pt)",
        |t| &mut t.clear,
    ),
    ("content.band", "Then fades in over (pt)", |t| &mut t.band),
    ("shared.duration", "Shared items travel (s)", |t| {
        &mut t.shared.response
    }),
    (
        "shared.travel",
        "Shared items travel (1) or swap (0)",
        |t| &mut t.shared_travel,
    ),
    ("reduced.fade", "Reduce Motion dissolve (s)", |t| {
        &mut t.reduced_fade
    }),
    (
        "timeScale",
        "Animation time scale (0.25 = slow motion)",
        |t| &mut t.time_scale,
    ),
];

/// Numbered presets, each a full set of values. `DEFAULT` is Kit Motion.
pub(super) const PRESETS: &[(&str, Tune)] = &[
    ("Today", Tune::TODAY),
    ("Native", Tune::NATIVE),
    ("Kit Motion", Tune::KIT_MOTION),
    ("Still", Tune::STILL),
];

impl Tune {
    pub fn get(&self, key: &str) -> Option<f32> {
        let mut copy = *self;
        KEYS.iter()
            .find(|(k, ..)| *k == key)
            .map(|(_, _, f)| *f(&mut copy))
    }

    fn set(&mut self, key: &str, value: f32) -> bool {
        match KEYS.iter().find(|(k, ..)| *k == key) {
            Some((_, _, f)) => {
                *f(self) = value;
                true
            }
            None => false,
        }
    }

    /// The preset the file names (1-based) or `DEFAULT`, then the file's own keys.
    pub(super) fn from_json(value: &serde_json::Value) -> Tune {
        let preset = value.get("preset").and_then(serde_json::Value::as_u64);
        let mut tune = preset
            .and_then(|n| PRESETS.get((n as usize).checked_sub(1)?))
            .map_or(Tune::DEFAULT, |(_, t)| *t);
        let scale = tune.time_scale;
        if let Some(map) = value.as_object() {
            for (key, v) in map {
                if let Some(v) = v.as_f64() {
                    tune.set(key, v as f32);
                }
            }
        }
        if value.get("timeScale").is_none() {
            tune.time_scale = scale;
        }
        tune.sanitize();
        tune
    }

    fn sanitize(&mut self) {
        for spring in [
            &mut self.open,
            &mut self.close,
            &mut self.squish,
            &mut self.shared,
        ] {
            spring.response = spring.response.clamp(0.04, 3.0);
            spring.bounce = spring.bounce.clamp(0.0, 0.9);
        }
        for v in [
            &mut self.width_lead,
            &mut self.height_lead,
            &mut self.collapse_wait,
        ] {
            *v = v.clamp(0.0, 0.5);
        }
        self.radius_blend = self.radius_blend.clamp(0.0, 16.0);
        self.squish_amount = self.squish_amount.clamp(0.0, 0.08);
        self.content_delay = self.content_delay.clamp(0.0, 0.95);
        self.content_in = self.content_in.clamp(0.02, 2.0);
        self.content_out = self.content_out.clamp(0.02, 2.0);
        self.content_blur = self.content_blur.clamp(0.0, 12.0);
        self.content_scale = self.content_scale.clamp(0.5, 1.0);
        self.blur_lead = self.blur_lead.clamp(0.1, 1.0);
        self.stagger = self.stagger.clamp(0.0, 0.1);
        self.content_push = self.content_push.clamp(0.0, 12.0);
        self.exit_lead = self.exit_lead.clamp(0.02, 0.3);
        self.clear = self.clear.clamp(0.0, 4.0);
        self.band = self.band.clamp(0.5, 6.0);
        self.shared_travel = if self.shared_travel >= 0.5 { 1.0 } else { 0.0 };
        self.reduced_fade = self.reduced_fade.clamp(0.05, 1.0);
        self.time_scale = self.time_scale.clamp(0.02, 4.0);
    }
}

pub(super) fn tune_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SCRAWL_TUNE") {
        return Some(PathBuf::from(path));
    }
    dirs::home_dir().map(|home| home.join(".config/scrawl/tune.json"))
}

struct TuneState {
    tune: Tune,
    checked: Option<Instant>,
    stamp: Option<std::time::SystemTime>,
}

thread_local! {
    static TUNE: RefCell<TuneState> = const {
        RefCell::new(TuneState { tune: Tune::DEFAULT, checked: None, stamp: None })
    };
    /// Cached copy for the hot path.
    static CURRENT: Cell<Tune> = const { Cell::new(Tune::DEFAULT) };
}

/// The live values. One thread-local read; `DEFAULT` unless a tune file is loaded.
pub(super) fn tune() -> Tune {
    CURRENT.get()
}

/// Re-reads `tune.json` if it changed (checked at most four times a second). Debug builds only,
/// so a release never picks up a stray file.
pub(super) fn poll_tune() {
    if !cfg!(debug_assertions) {
        return;
    }
    TUNE.with_borrow_mut(|state| {
        let first = state.checked.is_none();
        if state
            .checked
            .is_some_and(|at| at.elapsed() < Duration::from_millis(250))
        {
            return;
        }
        state.checked = Some(Instant::now());
        let Some(path) = tune_path() else { return };
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if stamp == state.stamp && !first {
            return;
        }
        state.stamp = stamp;
        let mut tune = stamp
            .and_then(|_| std::fs::read_to_string(&path).ok())
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map_or(Tune::DEFAULT, |value| Tune::from_json(&value));
        if let Some(scale) = std::env::var("SCRAWL_TIME_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
        {
            tune.time_scale = scale.clamp(0.02, 4.0);
        }
        if tune != state.tune {
            crate::capture::scrawl_log(format!(
                "tune: {} ({})",
                if stamp.is_some() {
                    "loaded"
                } else {
                    "defaults"
                },
                path.display()
            ));
        }
        state.tune = tune;
        CURRENT.set(tune);
        set_time_scale(f64::from(tune.time_scale));
    });
}

/// `scrawl tune …`: edits `tune.json`, which debug builds (the lab, the installed app) re-read
/// within a quarter second.
pub(crate) enum TuneAction {
    List,
    Set(String, f32),
    Preset(usize),
    Reset,
    Rust,
}

pub(crate) fn tune_command(action: TuneAction) -> anyhow::Result<()> {
    let path = tune_path().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let read = || -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    };
    let write = |map: &serde_json::Map<String, serde_json::Value>| -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(map)? + "\n")?;
        Ok(())
    };
    match action {
        TuneAction::List => {
            let file = read();
            let tune = Tune::from_json(&serde_json::Value::Object(file.clone()));
            let preset = file.get("preset").and_then(serde_json::Value::as_u64);
            println!("{}", path.display());
            for (i, (name, _)) in PRESETS.iter().enumerate() {
                let default = PRESETS[i].1 == Tune::DEFAULT;
                let mark = if preset == Some(i as u64 + 1) || (preset.is_none() && default) {
                    "●"
                } else {
                    " "
                };
                print!("{mark} {} {name}   ", i + 1);
            }
            println!();
            for (key, label, _) in KEYS {
                let now = tune.get(key).unwrap_or_default();
                let default = Tune::DEFAULT.get(key).unwrap_or_default();
                let changed = if (now - default).abs() > 1e-6 {
                    "*"
                } else {
                    " "
                };
                println!("{changed} {key:<24} {now:<8} (default {default})  {label}");
            }
        }
        TuneAction::Set(key, value) => {
            anyhow::ensure!(
                Tune::DEFAULT.get(&key).is_some(),
                "unknown key {key}; `scrawl tune list` shows them"
            );
            let mut map = read();
            map.insert(key.clone(), serde_json::json!(value));
            write(&map)?;
            let tune = Tune::from_json(&serde_json::Value::Object(map));
            println!("{key} = {}", tune.get(&key).unwrap_or_default());
        }
        TuneAction::Preset(n) => {
            let (name, _) = PRESETS
                .get(n.wrapping_sub(1))
                .ok_or_else(|| anyhow::anyhow!("presets are 1–{}", PRESETS.len()))?;
            // A preset replaces individual keys, as picking one does on the phone tune page.
            let mut map = serde_json::Map::new();
            if let Some(scale) = read().get("timeScale") {
                map.insert("timeScale".into(), scale.clone());
            }
            map.insert("preset".into(), n.into());
            write(&map)?;
            println!("{n} {name}");
        }
        TuneAction::Reset => match std::fs::remove_file(&path) {
            Ok(()) => println!("removed {}", path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => println!("nothing to reset"),
            Err(e) => return Err(e.into()),
        },
        TuneAction::Rust => {
            let t = Tune::from_json(&serde_json::Value::Object(read()));
            // `Debug` prints a valid struct literal (`Spring`'s fields are visible in `ui`).
            println!("// Paste over `Tune::DEFAULT` in src/ui/motion.rs.");
            println!("pub const DEFAULT: Tune = {t:#?};");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Trace

/// Maps the capsule window's GPUI coordinates to what's drawn on screen: the content layer's
/// scale about the window centre and its drop, then the window's screen origin. `clip` is the
/// shell (the capsule clips its content), in screen points.
#[derive(Clone, Copy, Debug)]
pub(super) struct Xf {
    pub s: f32,
    pub cx: f32,
    pub cy: f32,
    pub ty: f32,
    pub ox: f32,
    pub oy: f32,
    pub alpha: f32,
    pub clip: [f32; 4],
}

impl Xf {
    pub fn map(&self, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
        [
            self.ox + self.cx + (x - self.cx) * self.s,
            self.oy + self.cy + (y - self.cy) * self.s + self.ty,
            w * self.s,
            h * self.s,
        ]
    }
}

/// A prepaint listener that traces a row's children under `keys` (`None` skips a child), each
/// at its own opacity times the capsule's.
pub(super) fn trace_children(
    keys: Vec<Option<(String, f32)>>,
    xf: Xf,
    parent: &'static str,
) -> impl Fn(Vec<gpui::Bounds<gpui::Pixels>>, &mut gpui::Window, &mut gpui::App) + 'static {
    move |bounds, _, _| {
        for (key, b) in keys.iter().zip(bounds) {
            let Some((key, alpha)) = key else { continue };
            let rect = xf.map([
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            ]);
            trace_element(
                key.clone(),
                rect,
                alpha * xf.alpha,
                0.0,
                Some(parent),
                Some(xf.clip),
            );
        }
    }
}

type PendingElement = (
    String,
    [f32; 4],
    f32,
    f32,
    Option<&'static str>,
    Option<[f32; 4]>,
);

#[derive(Default)]
struct Pending {
    t_ms: f64,
    elements: Vec<PendingElement>,
    channels: Vec<(&'static str, &'static str, f32)>,
    events: Vec<String>,
}

struct Trace {
    out: std::io::BufWriter<std::fs::File>,
    start: Instant,
    last_t: Option<f64>,
    ids: HashMap<String, u32>,
    described: HashSet<u32>,
    states: HashMap<u32, String>,
    channel_ids: HashMap<&'static str, u32>,
    channel_values: HashMap<u32, String>,
    pending: Option<Pending>,
    queued_events: Vec<String>,
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

fn trace_target() -> Option<&'static PathBuf> {
    static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
    TARGET
        .get_or_init(|| std::env::var_os("SCRAWL_MOTION_TRACE").map(PathBuf::from))
        .as_ref()
}

/// Whether a trace is being written. One `OnceLock` read when off.
pub(super) fn tracing() -> bool {
    trace_target().is_some()
}

/// Opens the trace (once): a motion-lint header sized to the main display in points.
pub(super) fn trace_open(launch: &str, display: [f32; 2]) {
    let Some(path) = trace_target() else { return };
    TRACE.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(file) = std::fs::File::create(path) else {
            crate::capture::scrawl_log(format!("motion trace: can't write {}", path.display()));
            return;
        };
        let mut out = std::io::BufWriter::new(file);
        let header = serde_json::json!({
            "type": "header", "launch": launch, "width": display[0], "height": display[1],
            "hz": 60, "scale": 2, "wall": crate::now_epoch_ms() as f64 / 1000.0,
            "platform": "gpui", "units": "pt", "clock": "wall",
        });
        let _ = writeln!(out, "{header}");
        *slot = Some(Trace {
            out,
            start: Instant::now(),
            last_t: None,
            ids: HashMap::new(),
            described: HashSet::new(),
            states: HashMap::new(),
            channel_ids: HashMap::new(),
            channel_values: HashMap::new(),
            pending: None,
            queued_events: Vec::new(),
        });
    });
}

/// An app event (a phase change, a chip click), written with the next frame.
pub(super) fn trace_event(event: impl Into<String>) {
    if !tracing() {
        return;
    }
    TRACE.with_borrow_mut(|slot| {
        if let Some(trace) = slot.as_mut() {
            trace.queued_events.push(event.into());
        }
    });
}

/// Starts a frame: the previous one (now complete, its prepaint bounds included) is written.
/// Ends the frame in progress without starting one: what is drawn until the next
/// `trace_frame` isn't recorded (a new window's first frame, drawn fully transparent).
pub(super) fn trace_skip_frame() {
    TRACE.with_borrow_mut(|slot| {
        if let Some(trace) = slot.as_mut() {
            trace.flush_pending();
        }
    });
}

pub(super) fn trace_frame() {
    TRACE.with_borrow_mut(|slot| {
        let Some(trace) = slot.as_mut() else { return };
        trace.flush_pending();
        trace.pending = Some(Pending {
            t_ms: trace.start.elapsed().as_secs_f64() * 1000.0,
            events: std::mem::take(&mut trace.queued_events),
            ..Pending::default()
        });
    });
}

/// An element's box in screen points (top-left origin), drawn opacity and corner radius, and
/// the clip it's drawn inside.
pub(super) fn trace_element(
    key: impl Into<String>,
    rect: [f32; 4],
    alpha: f32,
    radius: f32,
    parent: Option<&'static str>,
    clip: Option<[f32; 4]>,
) {
    TRACE.with_borrow_mut(|slot| {
        if let Some(pending) = slot.as_mut().and_then(|t| t.pending.as_mut()) {
            pending
                .elements
                .push((key.into(), rect, alpha, radius, parent, clip));
        }
    });
}

/// The part of `rect` inside `clip`, when the clip cuts it.
fn visible(rect: [f32; 4], clip: [f32; 4]) -> Option<[f32; 4]> {
    let x0 = rect[0].max(clip[0]);
    let y0 = rect[1].max(clip[1]);
    let x1 = (rect[0] + rect[2]).min(clip[0] + clip[2]);
    let y1 = (rect[1] + rect[3]).min(clip[1] + clip[3]);
    let vis = [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)];
    let cut = (0..4).any(|i| (vis[i] - rect[i]).abs() > 0.25);
    cut.then_some(vis)
}

/// A scalar per frame: `kind` is motion-lint's `level`, `phase` or `position`.
pub(super) fn trace_channel(name: &'static str, kind: &'static str, value: f32) {
    TRACE.with_borrow_mut(|slot| {
        if let Some(pending) = slot.as_mut().and_then(|t| t.pending.as_mut()) {
            pending.channels.push((name, kind, value));
        }
    });
}

/// Writes the last frame and closes the file.
pub(super) fn trace_close() {
    TRACE.with_borrow_mut(|slot| {
        if let Some(mut trace) = slot.take() {
            trace.flush_pending();
            // An empty closing frame so the last change has a frame after it.
            trace.pending = Some(Pending {
                t_ms: trace.start.elapsed().as_secs_f64() * 1000.0,
                ..Pending::default()
            });
            trace.flush_pending();
            let _ = trace.out.flush();
        }
    });
}

fn r2(v: f32) -> f64 {
    (f64::from(v) * 100.0).round() / 100.0
}

impl Trace {
    fn id(&mut self, key: &str) -> u32 {
        let next = self.ids.len() as u32 + 1;
        *self.ids.entry(key.to_owned()).or_insert(next)
    }

    fn flush_pending(&mut self) {
        let Some(frame) = self.pending.take() else {
            return;
        };
        let n = self.last_t.map_or(1, |last| {
            ((frame.t_ms - last) / (1000.0 / 60.0)).round().max(1.0) as u64
        });
        self.last_t = Some(frame.t_ms);
        let mut metas = Vec::new();
        let mut changed = Vec::new();
        let mut seen = HashSet::new();
        for (key, rect, a, r, parent, clip) in &frame.elements {
            let [x, y, w, h] = rect;
            let id = self.id(key);
            if !seen.insert(id) {
                continue;
            }
            if self.described.insert(id) {
                let mut meta =
                    serde_json::json!({ "id": id, "cls": "gpui", "nm": key, "host": "pill" });
                if let Some(parent) = parent {
                    meta["pv"] = self.id(parent).into();
                }
                metas.push(meta);
            }
            let mut e = vec![
                f64::from(id),
                r2(*x),
                r2(*y),
                r2(*w),
                r2(*h),
                (f64::from(*a) * 1000.0).round() / 1000.0,
                r2(*r),
                0.0,
            ];
            if let Some([vx, vy, vw, vh]) = clip.and_then(|clip| visible(*rect, clip)) {
                e.extend([r2(vx), r2(vy), r2(vw), r2(vh)]);
            }
            let e = serde_json::json!(e);
            let state = e.to_string();
            if self.states.get(&id) != Some(&state) {
                self.states.insert(id, state);
                changed.push(e);
            }
        }
        let gone: Vec<u32> = self
            .states
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in &gone {
            self.states.remove(id);
        }
        let mut channel_metas = Vec::new();
        let mut channel_values = Vec::new();
        for (name, kind, value) in &frame.channels {
            let next = self.channel_ids.len() as u32 + 1;
            let id = *self.channel_ids.entry(name).or_insert(next);
            if id == next {
                channel_metas.push(serde_json::json!({ "id": id, "name": name, "kind": kind }));
            }
            let v = (f64::from(*value) * 1e4).round() / 1e4;
            let text = v.to_string();
            if self.channel_values.get(&id) != Some(&text) {
                self.channel_values.insert(id, text);
                channel_values.push(serde_json::json!([id, v]));
            }
        }
        let mut line = serde_json::json!({ "t": (frame.t_ms * 1000.0).round() / 1000.0, "n": n });
        if !frame.events.is_empty() {
            line["ev"] = frame.events.into();
        }
        if !metas.is_empty() {
            line["m"] = metas.into();
        }
        if !changed.is_empty() {
            line["e"] = changed.into();
        }
        if !gone.is_empty() {
            line["g"] = gone.into();
        }
        if !channel_metas.is_empty() {
            line["cm"] = channel_metas.into();
        }
        if !channel_values.is_empty() {
            line["c"] = channel_values.into();
        }
        let _ = writeln!(self.out, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tune_json_layers_a_preset_and_keys_over_the_defaults() {
        let tune = Tune::from_json(&serde_json::json!({ "preset": 2, "content.blur": 3.0 }));
        assert_eq!(tune.open, Tune::NATIVE.open);
        assert_eq!(tune.content_blur, 3.0);
        assert_eq!(tune.shared_travel, 0.0);
        // Unknown keys are ignored and values are clamped.
        let tune = Tune::from_json(&serde_json::json!({ "nope": 1, "open.bounce": 5 }));
        assert_eq!(tune.open.bounce, 0.9);
        assert_eq!(Tune::from_json(&serde_json::json!({})), Tune::DEFAULT);
    }

    #[test]
    fn presets_are_sane_and_kit_motion_ships() {
        assert_eq!(Tune::DEFAULT, Tune::KIT_MOTION);
        for (name, tune) in PRESETS {
            let mut copy = *tune;
            copy.sanitize();
            assert_eq!(copy, *tune, "{name} is inside the clamps");
            for (key, ..) in KEYS {
                assert!(tune.get(key).is_some(), "{name}: {key}");
            }
        }
        // Only the outline bounces.
        let kit = Tune::KIT_MOTION;
        assert_eq!(kit.shared.bounce, 0.0);
        assert_eq!((kit.open.response, kit.open.bounce), (0.5, 0.28));
        assert_eq!((kit.close.response, kit.close.bounce), (0.45, 0.18));
    }
}
