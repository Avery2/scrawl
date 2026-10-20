//! Frame-pacing log for the capsule (`SCRAWL_PILL_PACING=<file>`): one JSON line per drawn frame
//! with the display link's tick, when the frame was drawn, the moment its springs were sampled
//! at and the outline's edges, then one line per frame when it actually reached the glass
//! (`MTLDrawable.presentedTime`). `perf/pill/pacing.py` turns it into dropped frames, the worst
//! gap and how unevenly the motion advances. Host times are seconds on the
//! `CACurrentMediaTime` clock. Costs one `OnceLock` read when off.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use core_video::host_time::{get_current_host_time, get_host_clock_frequency};

static LOG: OnceLock<Option<Mutex<std::io::LineWriter<std::fs::File>>>> = OnceLock::new();
static FRAMES: AtomicU64 = AtomicU64::new(0);

fn log() -> Option<&'static Mutex<std::io::LineWriter<std::fs::File>>> {
    LOG.get_or_init(|| {
        let path = std::env::var_os("SCRAWL_PILL_PACING")?;
        let file = std::fs::File::create(path).ok()?;
        Some(Mutex::new(std::io::LineWriter::new(file)))
    })
    .as_ref()
}

pub fn enabled() -> bool {
    log().is_some()
}

/// Host ticks (`CVTimeStamp.hostTime`, `mach_absolute_time`) in seconds.
pub fn host_seconds(ticks: u64) -> f64 {
    static FREQ: OnceLock<f64> = OnceLock::new();
    ticks as f64 / *FREQ.get_or_init(get_host_clock_frequency)
}

pub fn host_now() -> f64 {
    host_seconds(get_current_host_time())
}

/// A frame number for `frame` and `presented` to share.
pub fn next_frame() -> u64 {
    FRAMES.fetch_add(1, Ordering::Relaxed) + 1
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Frame {
    pub n: u64,
    /// Display link callbacks so far (one per refresh).
    pub tick: u64,
    /// The link's "now" and the refresh this frame is for (`outputTime`), host seconds.
    pub link_now: f64,
    pub link_out: f64,
    pub period: f64,
    /// When drawing started and when the frame was handed to Core Animation.
    pub start: f64,
    pub end: f64,
    /// The model's clock after this frame's step, and the step.
    pub model_t: f32,
    pub dt: f32,
    pub l: f32,
    pub r: f32,
    pub main: bool,
}

fn write(line: &str) {
    if let Some(log) = log()
        && let Ok(mut out) = log.lock()
    {
        let _ = writeln!(out, "{line}");
    }
}

pub fn frame(f: &Frame) {
    write(&format!(
        "{{\"type\":\"frame\",\"n\":{},\"tick\":{},\"linkNow\":{:.6},\"linkOut\":{:.6},\"period\":{:.6},\"start\":{:.6},\"end\":{:.6},\"t\":{:.6},\"dt\":{:.6},\"l\":{:.4},\"r\":{:.4},\"main\":{}}}",
        f.n, f.tick, f.link_now, f.link_out, f.period, f.start, f.end, f.model_t, f.dt, f.l, f.r, f.main
    ));
}

pub fn presented(n: u64, at: f64) {
    write(&format!("{{\"type\":\"presented\",\"n\":{n},\"at\":{at:.6}}}"));
}

/// A presentation change (`Pres`) or anything else worth marking on the timeline.
pub fn event(name: &str) {
    write(&format!(
        "{{\"type\":\"event\",\"name\":{},\"at\":{:.6}}}",
        serde_json::Value::String(name.to_owned()),
        host_now()
    ));
}
