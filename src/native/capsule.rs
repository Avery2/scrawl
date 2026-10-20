//! The recording capsule the coordinator talks to: the Metal one (`pill::NativePill`, in this
//! process) or, with `"pillRenderer": "swiftui"`, the SwiftUI one in `scrawl-helper pill`
//! (`swift_pill`), which falls back to Metal when the helper won't start or goes away.

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

pub struct Capsule {
    inner: std::cell::RefCell<Inner>,
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

impl Capsule {
    pub fn open() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        if crate::handoff::pill_renderer() == Renderer::SwiftUi {
            match super::swift_pill::SwiftPill::open() {
                Ok(pill) => {
                    crate::capture::scrawl_log(format!(
                        "SwiftUI pill started (scrawl-helper pill, window {})",
                        pill.window_number()
                    ));
                    return Ok(Self::with(Inner::Swift(pill)));
                }
                Err(err) => crate::capture::scrawl_log(format!(
                    "SwiftUI pill failed to start ({err}); using the Metal pill"
                )),
            }
        }
        NativePill::open().map(|p| Self::with(Inner::Metal(p)))
    }

    fn with(inner: Inner) -> Self {
        Self {
            inner: std::cell::RefCell::new(inner),
        }
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
                match NativePill::open() {
                    Ok(metal) => {
                        if let Some(point) = crate::screens::pointer() {
                            metal.place_on(point);
                        }
                        *self.inner.borrow_mut() = Inner::Metal(metal);
                    }
                    Err(err) => crate::capture::scrawl_log(format!("NativePill::open error: {err}")),
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
