//! The recording capsule the coordinator talks to: the Metal one (`pill::NativePill`, in this
//! process) or, with `"pillRenderer": "swiftui"`, the SwiftUI one in `scrawl-helper pill`
//! (`swift_pill`), which falls back to Metal when the helper won't start or goes away. Changing
//! `pillRenderer` switches renderer without a relaunch, the next time the capsule opens.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::pill::{NativePill, PillAction, PillSnapshot};

/// Which capsule draws (`pillRenderer: "metal" | "swiftui"`, `SCRAWL_PILL_RENDERER`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Renderer {
    #[default]
    Metal,
    SwiftUi,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl Renderer {
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "swiftui" | "swift" => Some(Self::SwiftUi),
            "metal" => Some(Self::Metal),
            _ => None,
        }
    }
}

/// The SwiftUI capsule's window (0 when Metal draws). It belongs to `scrawl-helper`, so the
/// capture filter's "Scrawl's own floating windows" rule (by pid) misses it, and a session clears
/// `excluded_window_ids` when it starts; captures check this too.
static HELPER_WINDOW: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Whether `id` is the SwiftUI capsule's window, which recordings must leave out.
pub fn is_helper_window(id: u32) -> bool {
    id != 0 && HELPER_WINDOW.load(std::sync::atomic::Ordering::Relaxed) == id
}

/// How often `sync` (called every few ms) re-reads `pillRenderer` from `config.json`.
const RENDERER_POLL: Duration = Duration::from_millis(500);

pub struct Capsule {
    inner: std::cell::RefCell<Inner>,
    /// Captures leave these windows out; each renderer's window is added as it opens.
    excluded: Arc<Mutex<Vec<u32>>>,
    /// The configured renderer last seen, and when it was read. A switch happens when it
    /// changes (not on every read, so a helper that failed isn't retried every poll).
    wanted: std::cell::Cell<(Renderer, Instant)>,
}

enum Inner {
    Metal(NativePill),
    #[cfg(target_os = "macos")]
    Swift(super::swift_pill::SwiftPill),
}

macro_rules! each {
    ($self:ident, $p:ident => $e:expr) => {
        match &*$self.inner.borrow() {
            Inner::Metal($p) => $e,
            #[cfg(target_os = "macos")]
            Inner::Swift($p) => $e,
        }
    };
}

fn configured() -> Renderer {
    if cfg!(target_os = "macos") {
        crate::handoff::pill_renderer()
    } else {
        Renderer::Metal
    }
}

/// Opens `renderer`, or Metal if that fails.
fn open_inner(renderer: Renderer) -> Result<Inner, String> {
    #[cfg(target_os = "macos")]
    if renderer == Renderer::SwiftUi {
        match super::swift_pill::SwiftPill::open() {
            Ok(pill) => {
                crate::capture::scrawl_log(format!(
                    "SwiftUI pill started (scrawl-helper pill, window {})",
                    pill.window_number()
                ));
                return Ok(Inner::Swift(pill));
            }
            Err(err) => crate::capture::scrawl_log(format!(
                "SwiftUI pill failed to start ({err}); using the Metal pill"
            )),
        }
    }
    let _ = renderer;
    let pill = NativePill::open()?;
    crate::capture::scrawl_log("Metal pill started");
    Ok(Inner::Metal(pill))
}

impl Capsule {
    pub fn open(excluded: Arc<Mutex<Vec<u32>>>) -> Result<Self, String> {
        let renderer = configured();
        let capsule = Self {
            inner: std::cell::RefCell::new(open_inner(renderer)?),
            excluded,
            wanted: std::cell::Cell::new((renderer, Instant::now())),
        };
        capsule.exclude_window();
        Ok(capsule)
    }

    fn exclude_window(&self) {
        let num = self.window_number();
        HELPER_WINDOW.store(
            if self.is_swift() { num } else { 0 },
            std::sync::atomic::Ordering::Relaxed,
        );
        if num > 0 {
            let mut ids = self
                .excluded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !ids.contains(&num) {
                ids.push(num);
            }
        }
    }

    fn replace(&self, renderer: Renderer) {
        // Only called while the old capsule shows nothing; it closes as the new one takes over.
        let placeholder = match open_inner(renderer) {
            Ok(inner) => inner,
            Err(err) => {
                crate::capture::scrawl_log(format!("capsule switch failed: {err}"));
                return;
            }
        };
        *self.inner.borrow_mut() = placeholder;
        if let Some(point) = crate::screens::pointer() {
            self.place_on(point);
        }
        self.exclude_window();
    }

    fn is_swift(&self) -> bool {
        #[cfg(target_os = "macos")]
        return matches!(&*self.inner.borrow(), Inner::Swift(_));
        #[cfg(not(target_os = "macos"))]
        false
    }

    pub fn window_number(&self) -> u32 {
        each!(self, p => p.window_number())
    }

    pub fn place_on(&self, point: [f64; 2]) {
        each!(self, p => p.place_on(point));
    }

    pub fn sync(&self, snapshot: PillSnapshot) {
        #[cfg(target_os = "macos")]
        {
            let dead = matches!(&*self.inner.borrow(), Inner::Swift(p) if !p.is_alive());
            if dead {
                crate::capture::scrawl_log("SwiftUI pill went away; using the Metal pill");
                self.replace(Renderer::Metal);
            }
        }
        let (wanted, read_at) = self.wanted.get();
        if read_at.elapsed() >= RENDERER_POLL {
            let now = configured();
            self.wanted.set((now, Instant::now()));
            let current = if self.is_swift() {
                Renderer::SwiftUi
            } else {
                Renderer::Metal
            };
            if now != wanted && now != current {
                // Switch only while the current capsule shows nothing (the coordinator stops
                // syncing while hidden, so this is usually as the next capture opens it, and the
                // new one enters from hidden); until then keep asking.
                if self.is_fully_hidden() {
                    crate::capture::scrawl_log(format!("pillRenderer changed to {now:?}"));
                    self.replace(now);
                } else {
                    self.wanted.set((wanted, Instant::now()));
                }
            }
        }
        each!(self, p => p.sync(snapshot));
    }

    pub fn drain_actions(&self) -> Vec<PillAction> {
        each!(self, p => p.drain_actions())
    }

    pub fn is_fully_hidden(&self) -> bool {
        each!(self, p => p.is_fully_hidden())
    }

    pub fn pointer_on_pill(&self) -> bool {
        each!(self, p => p.pointer_on_pill())
    }
}
