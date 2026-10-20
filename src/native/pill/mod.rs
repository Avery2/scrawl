//! Native AppKit `NSPanel` + `CAMetalLayer` + Metal renderer for the Scrawl recording pill.
//!
//! Replaces the GPUI `HudPillWindow` with a single fixed-size `NSPanel` (`640 × 92 pt` at `y = 34`
//! below the menu bar) that never resizes or moves during morphs. Supports two backdrop looks via
//! `pillStyle: "black" | "glass"`:
//! - `"black"`: Analytic SDF rounded-rect silhouette + shadow + specular border in Metal.
//! - `"glass"`: AppKit's `NSGlassEffectView` (inside `NSGlassEffectContainerView`) driven on the
//!   same spring silhouette inside a shared `CATransaction` with `CAMetalLayer` (`presentsWithTransaction`).

pub mod atlas;
pub mod model;
mod pacing;
mod types;

pub use self::types::{PillAction, PillSnapshot, PillStyle};

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use core_graphics_types::geometry::CGSize;
use core_video::base::{CVOptionFlags, CVTimeStamp};
use core_video::display_link::{CVDisplayLink, CVDisplayLinkRef};
use core_video::r#return::CVReturn;
use metal::foreign_types::ForeignType;
use metal::{
    MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLStoreAction, MTLTextureUsage, MetalLayer,
    RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState, TextureDescriptor,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScreen, NSStatusWindowLevel,
    NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CATransaction};

use self::atlas::{GlyphAtlas, IconKind};
use self::model::{
    CHIP_H, DONE_TEXT_X, ElemId, FrameState, HUD_CAP, LEAD_TEXT_X, PANEL_H, PANEL_W,
    PROCESSING_TEXT_X, PillModel, Pres, SEG_ICON, SEG_ICON_GAP, SEG_PX,
};
use crate::ui::motion;

const OFFSCREEN_W_PT: f32 = 960.0;
const OFFSCREEN_H_PT: f32 = 256.0;
const TILE_PAD_PT: f32 = 20.0;
/// Transparent padding around each rasterized label (`rasterize_text`'s `pad_px` at 1x and 2x).
const TEXT_PAD_PT: f32 = 2.0;

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PillUniforms {
    panel_size: [f32; 2],
    canvas_size: [f32; 2],
    scale: f32,
    elem_count: u32,
    is_completed: u32,
    has_ghost: u32,
    shape_rect: [f32; 4],
    shape_meta: [f32; 4],
    ghost_rect: [f32; 4],
    ghost_meta: [f32; 4],
    /// Selection highlight `l, t, r, b` and `radius, alpha, blur, 0`.
    hl_rect: [f32; 4],
    hl_meta: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TileUniforms {
    atlas_size: [f32; 2],
    canvas_size: [f32; 2],
    scale: f32,
    _pad0: f32,
    _pad1: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TilePrim {
    rect: [f32; 4],
    uv_rect: [f32; 4],
    color: [f32; 4],
    kind: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BlurBox {
    tile_rect: [f32; 4],
    sigma_pt: f32,
    _pad0: f32,
    _pad1: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CompositeElem {
    screen_rect: [f32; 4],
    tile_rect: [f32; 4],
    base_size: [f32; 2],
    scale: f32,
    alpha: f32,
    sigma_pt: f32,
    pad_pt: f32,
    /// Label level outside and inside the selection highlight (chips are drawn white and
    /// multiplied here, so the highlight reveals the bright label as it slides). `[1, 1]` = as drawn.
    levels: [f32; 2],
}

struct PillViewIvars {
    action_tx: flume::Sender<PillAction>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PillViewIvars]
    struct ScrawlPillView;

    unsafe impl NSObjectProtocol for ScrawlPillView {}

    impl ScrawlPillView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            true
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().action_tx.try_send(PillAction::Cancel);
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let key_code: u16 = unsafe { msg_send![event, keyCode] };
            if key_code == 53 {
                let _ = self.ivars().action_tx.try_send(PillAction::Cancel);
            }
        }

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn accessibility_role(&self) -> Retained<NSString> {
            NSString::from_str("AXToolbar")
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Retained<NSString> {
            NSString::from_str("Scrawl Recording Controls")
        }
    }
);

impl ScrawlPillView {
    fn new(mtm: MainThreadMarker, frame: NSRect, action_tx: flume::Sender<PillAction>) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(PillViewIvars { action_tx });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }
}

struct MetalPipelines {
    _device: metal::Device,
    queue: metal::CommandQueue,
    layer: MetalLayer,
    tile_pipeline: RenderPipelineState,
    blur_h_pipeline: RenderPipelineState,
    composite_pipeline: RenderPipelineState,
    offscreen_a: metal::Texture,
    offscreen_b: metal::Texture,
    atlas: GlyphAtlas,
    scale: f32,
}

impl MetalPipelines {
    fn new(scale: f32) -> Result<Self, String> {
        let device = metal::Device::system_default().ok_or("No Metal device available")?;
        let queue = device.new_command_queue();

        let metallib_bytes = include_bytes!(concat!(env!("OUT_DIR"), "/pill.metallib"));
        let library = device
            .new_library_with_data(metallib_bytes)
            .map_err(|e| format!("Failed to load pill.metallib: {e}"))?;

        let make_pipeline = |vs_name: &str, fs_name: &str, premul_blend: bool| -> Result<RenderPipelineState, String> {
            let vs = library
                .get_function(vs_name, None)
                .map_err(|e| format!("Missing vertex shader {vs_name}: {e}"))?;
            let fs = library
                .get_function(fs_name, None)
                .map_err(|e| format!("Missing fragment shader {fs_name}: {e}"))?;
            let desc = RenderPipelineDescriptor::new();
            desc.set_vertex_function(Some(&vs));
            desc.set_fragment_function(Some(&fs));
            let att = desc
                .color_attachments()
                .object_at(0)
                .ok_or("Missing color attachment 0")?;
            att.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            if premul_blend {
                att.set_blending_enabled(true);
                att.set_source_rgb_blend_factor(metal::MTLBlendFactor::One);
                att.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
                att.set_rgb_blend_operation(metal::MTLBlendOperation::Add);
                att.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
                att.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
                att.set_alpha_blend_operation(metal::MTLBlendOperation::Add);
            } else {
                att.set_blending_enabled(false);
            }
            device
                .new_render_pipeline_state(&desc)
                .map_err(|e| format!("Pipeline {vs_name}/{fs_name} error: {e}"))
        };

        let tile_pipeline = make_pipeline("pill_tile_vertex", "pill_tile_fragment", true)?;
        let blur_h_pipeline = make_pipeline("pill_blur_h_vertex", "pill_blur_h_fragment", false)?;
        let composite_pipeline = make_pipeline("pill_screen_vertex", "pill_composite_fragment", false)?;

        let make_offscreen = || {
            let desc = TextureDescriptor::new();
            desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            desc.set_width((OFFSCREEN_W_PT * scale).round() as u64);
            desc.set_height((OFFSCREEN_H_PT * scale).round() as u64);
            desc.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
            device.new_texture(&desc)
        };
        let offscreen_a = make_offscreen();
        let offscreen_b = make_offscreen();

        let layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        layer.set_presents_with_transaction(true);
        layer.set_opaque(false);
        layer.set_contents_scale(f64::from(scale));
        layer.set_drawable_size(CGSize::new(
            f64::from(PANEL_W * scale),
            f64::from(PANEL_H * scale),
        ));

        let atlas = GlyphAtlas::new(&device, scale);

        Ok(Self {
            _device: device,
            queue,
            layer,
            tile_pipeline,
            blur_h_pipeline,
            composite_pipeline,
            offscreen_a,
            offscreen_b,
            atlas,
            scale,
        })
    }
}

/// What a frame is drawn from: the model, Metal and the frame clock. The display link's thread
/// draws from it; the main thread changes it in `sync` under the same lock.
struct RenderState {
    model: PillModel,
    pipelines: MetalPipelines,
    style: PillStyle,
    /// An `NSGlassEffectView` exists (macOS 26+), so the glass style can be drawn.
    has_glass: bool,
    screen_origin_top_left: [f32; 2],
    /// Animation seconds per real second (`motion::time_scale`), copied in by the main thread.
    time_scale: f32,
    /// The refresh the last step was sampled for (host seconds); `None` after a pause.
    last_out: Option<f64>,
    dirty: bool,
    presented: Arc<Presented>,
    run: RunStats,
}

impl RenderState {
    fn use_glass(&self) -> bool {
        self.style == PillStyle::Glass && self.has_glass
    }
}

/// The panel and pointer state, touched only on the main thread.
struct MainState {
    window: Retained<NSPanel>,
    glass_view: Option<Retained<NSView>>,
    ignores_mouse: bool,
    was_mouse_down: bool,
    press_down_elem: Option<ElemId>,
    paste_drag_start_screen: Option<[f32; 2]>,
    action_tx: flume::Sender<PillAction>,
}

// Locked only on the main thread (`MainThreadMarker` checks at every use).
unsafe impl Send for MainState {}

struct SharedNativePill {
    render: Mutex<RenderState>,
    main: Mutex<MainState>,
    /// The glass style draws on the main thread, since its view moves in the same transaction
    /// as the Metal frame. Otherwise frames are drawn on the display link's thread.
    main_draws: AtomicBool,
    /// Low Power Mode is on, which holds a ProMotion display's compositor to 60 Hz while the
    /// display link still ticks at 120.
    low_power: AtomicBool,
    tick_queued: AtomicBool,
    alive: AtomicBool,
    /// Display link callbacks so far, and the latest one's now / output time / period (f64 bits).
    ticks: AtomicU64,
    link_now: AtomicU64,
    link_out: AtomicU64,
    link_period: AtomicU64,
    /// Whether the panel should let the pointer through (it's off the drawn outline), decided
    /// on whichever thread saw it last; `apply_ignore` makes the panel match on the main thread.
    want_ignore: AtomicBool,
    ignore_queued: AtomicBool,
    /// A press started on the capsule (keeps it taking events until the button comes up).
    pressing: AtomicBool,
}

/// What reached the screen, from `MTLDrawable.presentedTime`: the last time (f64 bits), the
/// longest gap between shown frames in the current run, and how many were shown or replaced.
#[derive(Default)]
struct Presented {
    last: AtomicU64,
    max_gap: AtomicU64,
    shown: AtomicU64,
    replaced: AtomicU64,
}

impl Presented {
    fn note(&self, at: f64) {
        if at <= 0.0 {
            self.replaced.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let last = f64::from_bits(self.last.swap(at.to_bits(), Ordering::Relaxed));
        let gap = at - last;
        if last > 0.0 && gap < 0.25 && gap > f64::from_bits(self.max_gap.load(Ordering::Relaxed)) {
            self.max_gap.store(gap.to_bits(), Ordering::Relaxed);
        }
        self.shown.fetch_add(1, Ordering::Relaxed);
    }

    /// Shown, replaced, worst gap (s) since the last call.
    fn take(&self) -> (u64, u64, f64) {
        (
            self.shown.swap(0, Ordering::Relaxed),
            self.replaced.swap(0, Ordering::Relaxed),
            f64::from_bits(self.max_gap.swap(0, Ordering::Relaxed)),
        )
    }
}

/// One run of frames (a morph, or a stretch of breathing), summarised in `scrawl.log` when it
/// ends so a janky moment in real use leaves numbers behind.
#[derive(Default)]
struct RunStats {
    frames: u32,
    worst_step: f64,
    max_draw: f64,
    max_wait: f64,
}

pub struct NativePill {
    shared: Arc<SharedNativePill>,
    display_link: Option<CVDisplayLink>,
    display_link_ctx: Option<*const SharedNativePill>,
    /// The link runs while anything on screen moves (`PillModel::is_static`), not while the
    /// capsule is hidden or still.
    link_running: std::cell::Cell<bool>,
    action_rx: flume::Receiver<PillAction>,
    window_num: u32,
}

impl NativePill {
    pub fn open() -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("NativePill::open requires the main thread")?;
        let screens = crate::screens::current();
        // Drawn at the sharpest scale of any display, so moving between them needs no new
        // textures (a 1x display just downsamples).
        let scale = screens
            .iter()
            .map(|s| s.scale as f32)
            .fold(2.0_f32, f32::max);
        let screen = panel_screen(&screens, crate::screens::pointer())
            .ok_or("No screen available for NativePill")?;
        let [top_left_x, top_left_y] = panel_origin(screen);
        let display_id = screen.id;
        let panel_rect = panel_appkit_rect(mtm, [top_left_x, top_left_y]);

        let pipelines = MetalPipelines::new(scale)?;
        let metrics = atlas::measure_metrics();
        let model = PillModel::new(metrics);
        let (action_tx, action_rx) = flume::unbounded();

        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            panel_rect,
            NSWindowStyleMask::Borderless
                | NSWindowStyleMask::UtilityWindow
                | NSWindowStyleMask::NonactivatingPanel
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe {
            let sel = sel!(_setPreventsActivation:);
            let responds: bool = msg_send![&*window, respondsToSelector: sel];
            if responds {
                let _: () = msg_send![&*window, _setPreventsActivation: true];
            }
            window.setReleasedWhenClosed(false);
            // No AppKit window animation: ordering a utility panel in plays a system zoom and
            // fade on the whole window, on the window server's timing, over the capsule's own
            // entry (seen as the window's bounds shrinking by up to 10 pt for a few frames).
            // 2 = `NSWindowAnimationBehaviorNone`.
            let _: () = msg_send![&*window, setAnimationBehavior: 2_isize];
        }

        let local_bounds = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(PANEL_W), f64::from(PANEL_H)),
        );
        let root_view = ScrawlPillView::new(mtm, local_bounds, action_tx.clone());
        root_view.setWantsLayer(true);

        // Optional AppKit Liquid Glass view (`NSGlassEffectView` on macOS 26+).
        let glass_view = create_glass_views(&root_view, local_bounds);

        let metal_view = NSView::initWithFrame(mtm.alloc(), local_bounds);
        metal_view.setWantsLayer(true);
        let cocoa_layer = unsafe { &*pipelines.layer.as_ptr().cast::<CALayer>() };
        cocoa_layer.setOpaque(false);
        metal_view.setLayer(Some(cocoa_layer));
        root_view.addSubview(&metal_view);

        window.setContentView(Some(&root_view));
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setHasShadow(false);
        window.setHidesOnDeactivate(false);
        window.setCanHide(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSStatusWindowLevel + 5);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::CanJoinAllApplications
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setFrame_display(panel_rect, true);
        window.orderFrontRegardless();
        // Force one initial AppKit display/layout pass while `glass_view` is non-hidden (alpha 0)
        // so `NSGlassEffectView` constructs its `SDFLayer` + `CABackdropLayer` + `CASDFElementLayer`
        // hierarchy on the main thread.
        window.displayIfNeeded();
        if let Some(ref gv) = glass_view {
            gv.setHidden(true);
        }
        let window_num = window.windowNumber().max(0) as u32;

        let style = PillStyle::current();
        let has_glass = glass_view.is_some();
        let render = RenderState {
            model,
            pipelines,
            style,
            has_glass,
            screen_origin_top_left: [top_left_x, top_left_y],
            time_scale: motion::time_scale(),
            last_out: None,
            dirty: true,
            presented: Arc::new(Presented::default()),
            run: RunStats::default(),
        };
        let main_draws = render.use_glass();
        let shared = Arc::new(SharedNativePill {
            render: Mutex::new(render),
            main: Mutex::new(MainState {
                window: window.clone(),
                glass_view,
                ignores_mouse: true,
                was_mouse_down: false,
                press_down_elem: None,
                paste_drag_start_screen: None,
                action_tx,
            }),
            main_draws: AtomicBool::new(main_draws),
            low_power: AtomicBool::new(low_power_mode()),
            tick_queued: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            ticks: AtomicU64::new(0),
            link_now: AtomicU64::new(0),
            link_out: AtomicU64::new(0),
            link_period: AtomicU64::new(0),
            want_ignore: AtomicBool::new(true),
            ignore_queued: AtomicBool::new(false),
            pressing: AtomicBool::new(false),
        });
        if pacing::enabled() {
            pacing::event("window-frame");
        }

        // Draw initial transparent frame synchronously.
        shared.draw_main(pacing::host_now());

        let mut display_link = None;
        let mut display_link_ctx = None;
        if let Ok(dl) = CVDisplayLink::from_active_cg_displays() {
            let ctx = Arc::into_raw(shared.clone());
            if unsafe {
                dl.set_output_callback(
                    pill_display_link_cb,
                    ctx.cast_mut().cast::<c_void>(),
                )
            }
            .is_ok()
            {
                // Tick with the display the capsule is on, not whichever the link picked.
                let _ = dl.set_current_cg_display(display_id);
                display_link = Some(dl);
                display_link_ctx = Some(ctx);
            } else {
                unsafe { drop(Arc::from_raw(ctx)) };
            }
        }

        Ok(Self {
            shared,
            display_link,
            display_link_ctx,
            link_running: std::cell::Cell::new(false),
            action_rx,
            window_num,
        })
    }

    pub fn window_number(&self) -> u32 {
        self.window_num
    }

    /// Moves the capsule to the screen holding `point` (global points, top-left origin), below
    /// that screen's menu bar and notch. A no-op when it's already there.
    pub fn place_on(&self, point: [f64; 2]) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let screens = crate::screens::current();
        let Some(screen) = panel_screen(&screens, Some(point)) else {
            return;
        };
        let origin = panel_origin(screen);
        let (Ok(mut st), Ok(main)) = (self.shared.render.lock(), self.shared.main.lock()) else {
            return;
        };
        if (st.screen_origin_top_left[0] - origin[0]).abs() < 0.5
            && (st.screen_origin_top_left[1] - origin[1]).abs() < 0.5
        {
            return;
        }
        st.screen_origin_top_left = origin;
        main.window
            .setFrame_display(panel_appkit_rect(mtm, origin), true);
        if pacing::enabled() {
            pacing::event("window-frame");
        }
        // On a 1x screen AppKit resets the layer's contentsScale; keep the scale it's drawn at.
        let scale = st.pipelines.scale;
        st.pipelines.layer.set_contents_scale(f64::from(scale));
        st.pipelines.layer.set_drawable_size(CGSize::new(
            f64::from(PANEL_W * scale),
            f64::from(PANEL_H * scale),
        ));
        st.dirty = true;
        if let Some(ref dl) = self.display_link
            && dl.get_current_cg_display() != screen.id
        {
            let _ = dl.set_current_cg_display(screen.id);
            // The new display's refreshes are on another timeline.
            st.last_out = None;
        }
    }

    /// Synchronizes coordinator state into the native pill model and polls the pointer. Frames
    /// are drawn by the display link (on its own thread, or for the glass style on the main
    /// thread), each sampled at the refresh it will be shown on.
    pub fn sync(&self, snapshot: PillSnapshot) {
        let Some(_mtm) = MainThreadMarker::new() else {
            return;
        };
        let mut became_visible = false;
        let idle;
        {
            let (Ok(mut st), Ok(mut main)) = (self.shared.render.lock(), self.shared.main.lock())
            else {
                return;
            };
            if st.style != snapshot.style {
                st.style = snapshot.style;
                st.dirty = true;
                let main_draws = st.use_glass();
                self.shared.main_draws.store(main_draws, Ordering::Release);
                if !main_draws && let Some(ref gv) = main.glass_view {
                    gv.setHidden(true);
                }
            }
            st.time_scale = motion::time_scale();
            self.shared.low_power.store(low_power_mode(), Ordering::Relaxed);
            st.model.reduced = snapshot.reduced_motion;
            st.model.set_opts(motion::tune().pill_opts());
            if st.model.clock != snapshot.clock {
                st.pipelines.atlas.update_clock(&snapshot.clock);
                st.model.set_clock(&snapshot.clock);
                st.dirty = true;
            }
            if let Pres::Completed { ref label } = snapshot.pres {
                let sp = st.pipelines.atlas.update_completed(label);
                st.model.set_completed_label(label, sp.size_pt[0]);
            }
            if st.model.pres != snapshot.pres {
                if pacing::enabled() {
                    pacing::event(&format!("{:?}", snapshot.pres));
                }
                let was_hidden = st.model.is_fully_hidden();
                st.model.set_pres(snapshot.pres);
                st.dirty = true;
                if was_hidden {
                    main.window.setLevel(NSStatusWindowLevel + 5);
                    main.window.orderFrontRegardless();
                    became_visible = true;
                }
            }
            main.poll_pointer_and_input(&mut st, &self.shared);
            self.shared
                .pressing
                .store(main.press_down_elem.is_some(), Ordering::Release);
            // Static: nothing would change on screen until the next `sync` changes something.
            // (A trace keeps the link going, so it records the frames at rest too.)
            idle = st.model.is_fully_hidden()
                || (st.model.is_static() && !st.dirty && !motion::tracing());
        }
        // Start and stop the link without holding the lock its callback takes.
        match &self.display_link {
            Some(dl) if idle && self.link_running.get() => {
                let _ = dl.stop();
                self.link_running.set(false);
            }
            Some(dl) if !idle && !self.link_running.get() => {
                if let Ok(mut st) = self.shared.render.lock() {
                    st.last_out = None;
                }
                let _ = dl.start();
                self.link_running.set(true);
            }
            Some(_) => {}
            None => {
                if !idle || became_visible {
                    self.shared.draw_main(pacing::host_now());
                }
            }
        }
    }

    pub fn drain_actions(&self) -> Vec<PillAction> {
        self.action_rx.try_iter().collect()
    }

    pub fn is_fully_hidden(&self) -> bool {
        self.shared
            .render
            .lock()
            .map_or(true, |st| st.model.is_fully_hidden())
    }

    pub fn pointer_on_pill(&self) -> bool {
        self.shared
            .main
            .lock()
            .map_or(false, |main| !main.ignores_mouse)
    }
}

impl Drop for NativePill {
    fn drop(&mut self) {
        self.shared.alive.store(false, Ordering::SeqCst);
        if let Some(ref dl) = self.display_link {
            let _ = dl.stop();
        }
        if let Ok(main) = self.shared.main.lock() {
            main.window.orderOut(None);
            main.window.close();
        }
        if let Some(ctx) = self.display_link_ctx {
            unsafe { drop(Arc::from_raw(ctx)) };
        }
    }
}

/// The screen holding `point` (global points), else the primary one.
fn panel_screen(
    screens: &[crate::screens::Screen],
    point: Option<[f64; 2]>,
) -> Option<&crate::screens::Screen> {
    let index = point
        .and_then(|p| crate::screens::index_at(screens, p))
        .or(if screens.is_empty() { None } else { Some(0) })?;
    screens.get(index)
}

/// The panel's top-left (global points) on `screen`.
fn panel_origin(screen: &crate::screens::Screen) -> [f32; 2] {
    let [x, y] = crate::screens::pill_origin(
        screen,
        f64::from(PANEL_W),
        f64::from(model::PANEL_Y),
    );
    [x as f32, y as f32]
}

/// The panel's AppKit frame for a top-left origin in global points.
fn panel_appkit_rect(mtm: MainThreadMarker, [x, y]: [f32; 2]) -> NSRect {
    let primary_h = NSScreen::screens(mtm)
        .firstObject()
        .map_or(900.0, |s| s.frame().size.height);
    let [ax, ay, w, h] = crate::screens::flip(
        [
            f64::from(x),
            f64::from(y),
            f64::from(PANEL_W),
            f64::from(PANEL_H),
        ],
        primary_h,
    );
    NSRect::new(NSPoint::new(ax, ay), NSSize::new(w, h))
}

/// Creates `NSGlassEffectView` dynamically on macOS 26+.
fn create_glass_views(root_view: &NSView, bounds: NSRect) -> Option<Retained<NSView>> {
    let glass_cls = AnyClass::get(c"NSGlassEffectView")?;
    unsafe {
        let raw_glass: *mut NSView = msg_send![glass_cls, alloc];
        if raw_glass.is_null() {
            return None;
        }
        let init_frame = NSRect::new(
            NSPoint::new((bounds.size.width - 200.0) * 0.5, 22.0),
            NSSize::new(200.0, 40.0),
        );
        let raw_glass: *mut NSView = msg_send![raw_glass, initWithFrame: init_frame];
        let glass_view = Retained::from_raw(raw_glass)?;
        glass_view.setWantsLayer(true);
        glass_view.setAlphaValue(0.0);

        if msg_send![&*glass_view, respondsToSelector: sel!(setStyle:)] {
            // 0 = NSGlassEffectViewStyleRegular (refractive Liquid Glass bottle + specular rim)
            let _: () = msg_send![&*glass_view, setStyle: 0_isize];
        }
        if msg_send![&*glass_view, respondsToSelector: sel!(setCornerRadius:)] {
            let _: () = msg_send![&*glass_view, setCornerRadius: 20.0_f64];
        }
        if msg_send![&*glass_view, respondsToSelector: sel!(setTintColor:)] {
            let tint = NSColor::colorWithSRGBRed_green_blue_alpha(0.06, 0.07, 0.10, 0.22);
            let _: () = msg_send![&*glass_view, setTintColor: Some(&*tint)];
        }

        root_view.addSubview(&glass_view);
        Some(glass_view)
    }
}

/// Notes when the drawable reaches the screen (0 if a newer one replaced it first) for the
/// run summary, and logs it for `SCRAWL_PILL_PACING` (`n > 0`).
fn watch_presented(drawable: &metal::MetalDrawableRef, n: u64, presented: &Arc<Presented>) {
    let presented = presented.clone();
    let block = block2::RcBlock::new(move |d: *mut AnyObject| {
        let at: f64 = unsafe { msg_send![d, presentedTime] };
        presented.note(at);
        if n > 0 {
            pacing::presented(n, at);
        }
    });
    let obj = metal::foreign_types::ForeignTypeRef::as_ptr(drawable).cast::<AnyObject>();
    unsafe {
        let _: () = msg_send![obj, addPresentedHandler: &*block];
    }
}

extern "C" fn pill_display_link_cb(
    _display_link: CVDisplayLinkRef,
    now: *const CVTimeStamp,
    output_time: *const CVTimeStamp,
    _flags_in: CVOptionFlags,
    _flags_out: *mut CVOptionFlags,
    context: *mut c_void,
) -> CVReturn {
    if context.is_null() {
        return 0;
    }
    let shared = unsafe { &*context.cast::<SharedNativePill>() };
    if !shared.alive.load(Ordering::Relaxed) {
        return 0;
    }
    shared.ticks.fetch_add(1, Ordering::Relaxed);
    let Some((now, out)) = (unsafe { now.as_ref().zip(output_time.as_ref()) }) else {
        return 0;
    };
    let out_s = pacing::host_seconds(out.hostTime);
    shared.link_now.store(pacing::host_seconds(now.hostTime).to_bits(), Ordering::Relaxed);
    shared.link_out.store(out_s.to_bits(), Ordering::Relaxed);
    let period = if out.videoTimeScale > 0 {
        out.videoRefreshPeriod as f64 / f64::from(out.videoTimeScale)
    } else {
        0.0
    };
    shared.link_period.store(period.to_bits(), Ordering::Relaxed);
    if !shared.draws_refresh(out_s, period) {
        return 0;
    }

    if !shared.main_draws.load(Ordering::Acquire) {
        // Drawn here, off the main thread, so a busy app can't delay or bunch frames.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            shared.draw(out_s, period, None);
        }));
    } else if !shared.tick_queued.swap(true, Ordering::AcqRel) {
        // Retain an Arc reference for the main-queue callback.
        unsafe {
            Arc::increment_strong_count(context.cast::<SharedNativePill>());
            dispatch_async_f(&_dispatch_main_q, context, pill_main_queue_trampoline);
        }
    }
    0
}

extern "C" fn pill_main_queue_trampoline(context: *mut c_void) {
    let shared = unsafe { Arc::from_raw(context.cast::<SharedNativePill>()) };
    if shared.alive.load(Ordering::Relaxed) && shared.main_draws.load(Ordering::Acquire) {
        let out = f64::from_bits(shared.link_out.load(Ordering::Relaxed));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            shared.draw_main(out);
        }));
    }
    shared.tick_queued.store(false, Ordering::Release);
}

/// `NSProcessInfo.lowPowerMode`.
fn low_power_mode() -> bool {
    let Some(cls) = AnyClass::get(c"NSProcessInfo") else {
        return false;
    };
    unsafe {
        let info: *mut AnyObject = msg_send![cls, processInfo];
        !info.is_null() && msg_send![info, isLowPowerModeEnabled]
    }
}

/// How many display link ticks the compositor takes per refresh: 2 for a 120 Hz link while Low
/// Power Mode holds the compositor to 60 Hz, else 1.
fn refresh_stride(period: f64, low_power: bool) -> i64 {
    if !low_power || period <= 0.0 {
        return 1;
    }
    ((1.0 / 60.0) / period).round().max(1.0) as i64
}

impl SharedNativePill {
    /// Whether to draw for the refresh at `out`. When the compositor shows every `stride`th
    /// tick, drawing every tick makes it pick one of two frames, sometimes the earlier and
    /// sometimes the later, so the motion advances unevenly. Drawing every `stride`th tick, on
    /// a fixed phase of the display's clock, gives every frame exactly one refresh.
    fn draws_refresh(&self, out: f64, period: f64) -> bool {
        let stride = refresh_stride(period, self.low_power.load(Ordering::Relaxed));
        stride <= 1 || ((out / period).round() as i64).rem_euclid(stride) == 0
    }

    /// Draws on the main thread for the refresh at `out` (host seconds), moving the glass view
    /// with it when the glass style is on.
    fn draw_main(&self, out: f64) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let Ok(main) = self.main.lock() else {
            return;
        };
        let period = f64::from_bits(self.link_period.load(Ordering::Relaxed));
        self.draw(out, period, Some(&main));
    }

    /// Advances the springs to the refresh at `out` (host seconds; `period` apart) and, when
    /// anything moved, draws and presents that frame. Every step is the time between the
    /// refreshes it's drawn for, so the motion moves the same amount each frame, and a refresh
    /// that was missed is caught up rather than slowing the motion down.
    fn draw(&self, out: f64, period: f64, main: Option<&MainState>) {
        let asked = pacing::host_now();
        let Ok(mut st) = self.render.lock() else {
            return;
        };
        let wait = pacing::host_now() - asked;
        let step = match st.last_out {
            Some(prev) if out <= prev + 1e-4 => return,
            Some(prev) => out - prev,
            None if period > 0.0 => period,
            None => 1.0 / 60.0,
        };
        st.last_out = Some(out);
        let dt = step as f32 * st.time_scale;

        let animating = !st.model.is_static();
        if !animating && !st.dirty {
            if st.run.frames > 0 {
                let run = std::mem::take(&mut st.run);
                let (shown, replaced, gap) = st.presented.take();
                let stride = refresh_stride(period, self.low_power.load(Ordering::Relaxed));
                crate::capture::scrawl_log(format!(
                    "pill frames: {} drawn, link {:.0} Hz x{stride}, {shown} shown, {replaced} replaced, worst shown gap {:.1} ms, worst step {:.1} ms, draw max {:.2} ms, lock wait max {:.2} ms",
                    run.frames,
                    if period > 0.0 { 1.0 / period } else { 0.0 },
                    gap * 1000.0,
                    run.worst_step * 1000.0,
                    run.max_draw * 1000.0,
                    run.max_wait * 1000.0,
                ));
            }
            if motion::tracing() && !st.model.is_fully_hidden() {
                let frame = st.model.frame();
                st.emit_trace(&frame);
            }
            return;
        }

        let start = pacing::host_now();
        st.model.advance(dt);
        st.dirty = false;

        let frame = st.model.frame();
        if motion::tracing() && (!st.model.is_fully_hidden() || animating) {
            st.emit_trace(&frame);
        }
        let n = if pacing::enabled() { pacing::next_frame() } else { 0 };
        let glass = main.and_then(|m| m.glass_view.as_deref()).filter(|_| st.use_glass());
        st.render_frame(&frame, glass, n);
        let drawn = pacing::host_now() - start;
        if st.run.frames > 0 {
            st.run.worst_step = st.run.worst_step.max(step);
        } else {
            // A new run: what was shown before it is no part of it.
            st.presented.take();
        }
        st.run.frames += 1;
        st.run.max_draw = st.run.max_draw.max(drawn);
        st.run.max_wait = st.run.max_wait.max(wait);
        if main.is_none() {
            self.track_pointer(&st);
        }
        if n > 0 {
            let bits = |a: &AtomicU64| f64::from_bits(a.load(Ordering::Relaxed));
            pacing::frame(&pacing::Frame {
                n,
                tick: self.ticks.load(Ordering::Relaxed),
                link_now: bits(&self.link_now),
                link_out: out,
                period,
                start,
                end: pacing::host_now(),
                model_t: st.model.t,
                dt,
                l: st.model.l.x,
                r: st.model.r.x,
                main: main.is_some(),
            });
        }
    }
}

impl SharedNativePill {
    /// Checks the pointer against the outline just drawn, on the drawing thread, so the panel
    /// stops taking clicks the moment the shape leaves the pointer (and starts when it arrives)
    /// without waiting for the main thread's next poll to notice. Only a change goes to the main
    /// thread, which is the only place the panel itself can change.
    fn track_pointer(&self, st: &RenderState) {
        let Some([gx, gy]) = pointer_global() else {
            return;
        };
        let [ox, oy] = st.screen_origin_top_left;
        let inside = st.model.hit_island(gx - ox, gy - oy);
        let want = !inside && !self.pressing.load(Ordering::Relaxed);
        if self.want_ignore.swap(want, Ordering::AcqRel) != want
            && !self.ignore_queued.swap(true, Ordering::AcqRel)
        {
            let ctx = std::ptr::from_ref(self);
            // `self` lives in an `Arc` (the display link holds one); lend the main queue another.
            unsafe {
                Arc::increment_strong_count(ctx);
                dispatch_async_f(&_dispatch_main_q, ctx.cast_mut().cast(), apply_ignore_trampoline);
            }
        }
    }
}

extern "C" fn apply_ignore_trampoline(context: *mut c_void) {
    let shared = unsafe { Arc::from_raw(context.cast::<SharedNativePill>()) };
    shared.ignore_queued.store(false, Ordering::Release);
    if shared.alive.load(Ordering::Relaxed)
        && let Ok(mut main) = shared.main.lock()
    {
        main.apply_ignore(shared.want_ignore.load(Ordering::Acquire));
    }
}

/// The pointer in global top-left points (`CGEventGetLocation`), from any thread.
fn pointer_global() -> Option<[f32; 2]> {
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *mut c_void) -> core_graphics_types::geometry::CGPoint;
        fn CFRelease(cf: *const c_void);
    }
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        Some([p.x as f32, p.y as f32])
    }
}

impl MainState {
    fn apply_ignore(&mut self, ignore: bool) {
        if self.ignores_mouse != ignore {
            self.ignores_mouse = ignore;
            self.window.setIgnoresMouseEvents(ignore);
            if pacing::enabled() {
                pacing::event(if ignore { "pointer-through" } else { "pointer-taken" });
            }
        }
    }

    fn poll_pointer_and_input(&mut self, st: &mut RenderState, shared: &SharedNativePill) {
        if st.model.is_fully_hidden() {
            shared.want_ignore.store(true, Ordering::Release);
            self.apply_ignore(true);
            return;
        }

        let Some(cls) = AnyClass::get(c"NSEvent") else {
            return;
        };
        let mouse_ns: NSPoint = unsafe { msg_send![cls, mouseLocation] };
        let pressed_mask: usize = unsafe { msg_send![cls, pressedMouseButtons] };
        let mouse_down = (pressed_mask & 1) != 0;

        let win_frame = self.window.frame();
        let local_x = (mouse_ns.x - win_frame.origin.x) as f32;
        let local_y = (win_frame.size.height - (mouse_ns.y - win_frame.origin.y)) as f32;
        let screen_pt = [
            st.screen_origin_top_left[0] + local_x,
            st.screen_origin_top_left[1] + local_y,
        ];

        let inside_island = st.model.hit_island(local_x, local_y);
        let should_ignore = !inside_island && self.press_down_elem.is_none();
        shared.want_ignore.store(should_ignore, Ordering::Release);
        self.apply_ignore(should_ignore);

        let prev_hover = st.model.hovered;
        let prev_press = st.model.pressed;
        let pos_opt = inside_island.then_some([local_x, local_y]);
        st.model.set_pointer(pos_opt, mouse_down);
        if st.model.hovered != prev_hover || st.model.pressed != prev_press {
            st.dirty = true;
        }

        if mouse_down && !self.was_mouse_down {
            let hit = pos_opt.and_then(|[x, y]| st.model.hit_element(x, y));
            self.press_down_elem = hit;
            if hit == Some(ElemId::Paste) {
                self.paste_drag_start_screen = Some(screen_pt);
            } else {
                self.paste_drag_start_screen = None;
            }
        } else if !mouse_down && self.was_mouse_down {
            if let Some(down_elem) = self.press_down_elem.take() {
                let up_elem = pos_opt.and_then(|[x, y]| st.model.hit_element(x, y));
                if down_elem == ElemId::Paste {
                    if up_elem == Some(ElemId::Paste) {
                        let _ = self.action_tx.try_send(PillAction::Paste(None));
                    } else if let Some(start) = self.paste_drag_start_screen.take() {
                        let dist = (screen_pt[0] - start[0]).hypot(screen_pt[1] - start[1]);
                        if dist >= 24.0 {
                            let _ = self.action_tx.try_send(PillAction::Paste(Some(screen_pt)));
                        }
                    }
                } else if up_elem == Some(down_elem)
                    && let Some(action) = PillAction::for_click(&st.model.pres, down_elem)
                {
                    let _ = self.action_tx.try_send(action);
                }
            }
            self.paste_drag_start_screen = None;
        }
        self.was_mouse_down = mouse_down;
    }
}

impl RenderState {
    fn emit_trace(&self, frame: &FrameState) {
        motion::trace_frame();
        let [ox, oy] = self.screen_origin_top_left;
        let win = [ox, oy, PANEL_W, PANEL_H];
        let sh = &frame.shape;
        let shell = [ox + sh.l, oy + sh.t, sh.width(), sh.height()];
        motion::trace_element("window", win, 0.0, 0.0, None, None);
        motion::trace_element("shell", shell, sh.alpha, sh.radius, None, None);
        motion::trace_channel("content.blur", "level", frame.content_blur);
        motion::trace_channel("content.alpha", "level", frame.content_alpha);
        motion::trace_channel(
            "shell.width",
            "position",
            (self.model.r.x - self.model.l.x).max(0.0),
        );

        if let Some(hl) = frame.highlight {
            let [hx, hy, hw, hh] = hl.rect;
            motion::trace_element(
                "highlight",
                [ox + hx, oy + hy, hw, hh],
                hl.alpha,
                hh * 0.5,
                Some("indicator"),
                Some(shell),
            );
        }
        for elem in &frame.elems {
            let [ex, ey, ew, eh] = elem.rect;
            let screen_rect = [ox + ex, oy + ey, ew, eh];
            motion::trace_element(
                elem.id.trace_key(),
                screen_rect,
                elem.alpha,
                0.0,
                Some("row"),
                Some(shell),
            );
        }
    }

    fn render_frame(
        &mut self,
        frame: &FrameState,
        glass: Option<&NSView>,
        n: u64,
    ) {
        let use_glass = self.use_glass();

        let mut prims: Vec<TilePrim> = Vec::with_capacity(frame.elems.len() * 3);
        let mut blur_boxes: Vec<BlurBox> = Vec::with_capacity(frame.elems.len());
        let mut comp_elems: Vec<CompositeElem> = Vec::with_capacity(frame.elems.len());

        for (i, elem) in frame.elems.iter().take(16).enumerate() {
            let slot_x = (i % 5) as f32 * 192.0 + 4.0;
            let slot_y = (i / 5) as f32 * 112.0 + 4.0;
            let tile_w = (elem.base_size[0] + 2.0 * TILE_PAD_PT).min(184.0);
            let tile_h = (elem.base_size[1] + 2.0 * TILE_PAD_PT).min(104.0);
            let tile_rect = [slot_x, slot_y, tile_w, tile_h];

            self.emit_elem_prims(
                elem,
                [slot_x + TILE_PAD_PT, slot_y + TILE_PAD_PT],
                frame.t,
                &mut prims,
            );

            blur_boxes.push(BlurBox {
                tile_rect,
                sigma_pt: elem.blur,
                _pad0: 0.0,
                _pad1: [0.0; 2],
            });

            let levels = if elem.id.mode().is_some() {
                if use_glass {
                    [0.90 + 0.10 * elem.hover, 1.0]
                } else {
                    [0.608 + 0.353 * 0.65 * elem.hover, 0.961]
                }
            } else {
                [1.0, 1.0]
            };
            comp_elems.push(CompositeElem {
                screen_rect: elem.rect,
                tile_rect,
                base_size: elem.base_size,
                scale: elem.scale,
                alpha: elem.alpha,
                sigma_pt: elem.blur,
                pad_pt: TILE_PAD_PT,
                levels,
            });
        }

        let (has_ghost, ghost_rect, ghost_meta) = if let Some(ref g) = frame.ghost {
            (
                1_u32,
                [g.l, g.t, g.r, g.b],
                [g.radius, g.alpha, 0.0, 0.0],
            )
        } else {
            (0_u32, [0.0; 4], [0.0; 4])
        };

        let tile_uniforms = TileUniforms {
            atlas_size: [1024.0, 512.0],
            canvas_size: [OFFSCREEN_W_PT, OFFSCREEN_H_PT],
            scale: self.pipelines.scale,
            _pad0: 0.0,
            _pad1: [0.0; 2],
        };

        let uniforms = PillUniforms {
            panel_size: [PANEL_W, PANEL_H],
            canvas_size: [OFFSCREEN_W_PT, OFFSCREEN_H_PT],
            scale: self.pipelines.scale,
            elem_count: comp_elems.len() as u32,
            is_completed: u32::from(frame.is_completed),
            has_ghost,
            shape_rect: [
                frame.shape.l,
                frame.shape.t,
                frame.shape.r,
                frame.shape.b,
            ],
            shape_meta: [
                frame.shape.radius,
                frame.shape.alpha,
                frame.shape.blur,
                if use_glass { 1.0 } else { 0.0 },
            ],
            ghost_rect,
            ghost_meta,
            hl_rect: frame.highlight.map_or([0.0; 4], |h| {
                [h.rect[0], h.rect[1], h.rect[0] + h.rect[2], h.rect[1] + h.rect[3]]
            }),
            hl_meta: frame
                .highlight
                .map_or([0.0; 4], |h| [h.rect[3] * 0.5, h.alpha, h.blur, 0.0]),
        };

        let prim_count = prims.len() as u64;
        let box_count = blur_boxes.len() as u64;
        if prims.is_empty() {
            prims.push(TilePrim::default());
        }
        if blur_boxes.is_empty() {
            blur_boxes.push(BlurBox::default());
        }
        if comp_elems.is_empty() {
            comp_elems.push(CompositeElem::default());
        }

        let Some(drawable) = self.pipelines.layer.next_drawable() else {
            return;
        };

        let cmd = self.pipelines.queue.new_command_buffer();

        // Pass 1: Render crisp unblurred element primitives into `offscreen_a`.
        {
            let pass = RenderPassDescriptor::new();
            let att = pass.color_attachments().object_at(0).unwrap();
            att.set_texture(Some(&self.pipelines.offscreen_a));
            att.set_load_action(MTLLoadAction::Clear);
            att.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            att.set_store_action(MTLStoreAction::Store);
            let enc = cmd.new_render_command_encoder(&pass);
            if prim_count > 0 {
                enc.set_render_pipeline_state(&self.pipelines.tile_pipeline);
                enc.set_vertex_bytes(
                    0,
                    size_of::<TileUniforms>() as u64,
                    (&raw const tile_uniforms).cast::<c_void>(),
                );
                enc.set_vertex_bytes(
                    1,
                    (prims.len() * size_of::<TilePrim>()) as u64,
                    prims.as_ptr().cast::<c_void>(),
                );
                enc.set_fragment_bytes(
                    0,
                    size_of::<TileUniforms>() as u64,
                    (&raw const tile_uniforms).cast::<c_void>(),
                );
                enc.set_fragment_texture(0, Some(&self.pipelines.atlas.texture));
                enc.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, 6, prim_count);
            }
            enc.end_encoding();
        }

        // Pass 2: 1D Horizontal Gaussian blur from `offscreen_a` into `offscreen_b`.
        {
            let pass = RenderPassDescriptor::new();
            let att = pass.color_attachments().object_at(0).unwrap();
            att.set_texture(Some(&self.pipelines.offscreen_b));
            att.set_load_action(MTLLoadAction::Clear);
            att.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            att.set_store_action(MTLStoreAction::Store);
            let enc = cmd.new_render_command_encoder(&pass);
            if box_count > 0 {
                enc.set_render_pipeline_state(&self.pipelines.blur_h_pipeline);
                enc.set_vertex_bytes(
                    0,
                    size_of::<TileUniforms>() as u64,
                    (&raw const tile_uniforms).cast::<c_void>(),
                );
                enc.set_vertex_bytes(
                    1,
                    (blur_boxes.len() * size_of::<BlurBox>()) as u64,
                    blur_boxes.as_ptr().cast::<c_void>(),
                );
                enc.set_fragment_bytes(
                    0,
                    size_of::<TileUniforms>() as u64,
                    (&raw const tile_uniforms).cast::<c_void>(),
                );
                enc.set_fragment_texture(0, Some(&self.pipelines.offscreen_a));
                enc.draw_primitives_instanced(MTLPrimitiveType::Triangle, 0, 6, box_count);
            }
            enc.end_encoding();
        }

        // Pass 3: Capsule SDF silhouette + 1D Vertical Gaussian blur + hairline rim into drawable.
        {
            let pass = RenderPassDescriptor::new();
            let att = pass.color_attachments().object_at(0).unwrap();
            att.set_texture(Some(drawable.texture()));
            att.set_load_action(MTLLoadAction::Clear);
            att.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            att.set_store_action(MTLStoreAction::Store);
            let enc = cmd.new_render_command_encoder(&pass);
            enc.set_render_pipeline_state(&self.pipelines.composite_pipeline);
            enc.set_fragment_bytes(
                0,
                size_of::<PillUniforms>() as u64,
                (&raw const uniforms).cast::<c_void>(),
            );
            enc.set_fragment_bytes(
                1,
                (comp_elems.len() * size_of::<CompositeElem>()) as u64,
                comp_elems.as_ptr().cast::<c_void>(),
            );
            enc.set_fragment_texture(0, Some(&self.pipelines.offscreen_b));
            enc.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
            enc.end_encoding();
        }

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(gv) = glass {
            if frame.shape.alpha > 0.005 {
                gv.setHidden(false);
                let gx = f64::from(frame.shape.l);
                let gy = f64::from(PANEL_H - frame.shape.b);
                let gw = f64::from(frame.shape.width());
                let gh = f64::from(frame.shape.height());
                gv.setFrame(NSRect::new(NSPoint::new(gx, gy), NSSize::new(gw, gh)));
                gv.setAlphaValue(f64::from(frame.shape.alpha));
                unsafe {
                    if msg_send![gv, respondsToSelector: sel!(setCornerRadius:)] {
                        let _: () = msg_send![gv, setCornerRadius: f64::from(frame.shape.radius)];
                    }
                    if msg_send![gv, respondsToSelector: sel!(setTintColor:)] {
                        let tint = if frame.is_completed {
                            NSColor::colorWithSRGBRed_green_blue_alpha(0.04, 0.14, 0.09, 0.28)
                        } else {
                            NSColor::colorWithSRGBRed_green_blue_alpha(0.06, 0.07, 0.10, 0.22)
                        };
                        let _: () = msg_send![gv, setTintColor: Some(&*tint)];
                    }
                }
                gv.setNeedsLayout(true);
                gv.layoutSubtreeIfNeeded();
            } else {
                gv.setHidden(true);
            }
        }
        watch_presented(drawable, n, &self.presented);
        cmd.commit();
        cmd.wait_until_scheduled();
        drawable.present();
        CATransaction::commit();
    }

    fn emit_elem_prims(
        &self,
        elem: &model::FrameElem,
        [ox, oy]: [f32; 2],
        t: f32,
        out: &mut Vec<TilePrim>,
    ) {
        let atlas = &self.pipelines.atlas;
        let [bw, bh] = elem.base_size;
        let push_shape = |out: &mut Vec<TilePrim>, rect: [f32; 4], radius: f32, color: [f32; 4]| {
            out.push(TilePrim {
                rect,
                uv_rect: [radius, 0.0, 0.0, 0.0],
                color,
                kind: 0,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            });
        };
        let push_sprite = |out: &mut Vec<TilePrim>, rect: [f32; 4], uv: [f32; 4], color: [f32; 4]| {
            out.push(TilePrim {
                rect,
                uv_rect: uv,
                color,
                kind: 1,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
            });
        };
        // Labels sit on one baseline that centres SF Pro's caps on the chip, snapped to the pixel
        // grid so the sprite (whose baseline is a whole pixel row) samples 1:1.
        let px = self.pipelines.scale;
        let snap = |v: f32| (v * px).round() / px;
        let baseline = snap(oy + CHIP_H * 0.5 + HUD_CAP * 0.5);
        let text_at = |sp: atlas::Sprite, glyph_x: f32| -> [f32; 4] {
            [
                snap(glyph_x - TEXT_PAD_PT),
                baseline - sp.baseline_pt,
                sp.size_pt[0],
                sp.size_pt[1],
            ]
        };
        let center_text = |sp: atlas::Sprite| -> [f32; 4] {
            text_at(sp, ox + (bw - sp.size_pt[0]) * 0.5 + TEXT_PAD_PT)
        };

        let is_glass = self.use_glass();

        match elem.id {
            ElemId::ChipArea | ElemId::ChipWindow | ElemId::ChipScreen | ElemId::ChipIphone => {
                let Some(mode) = elem.id.mode() else {
                    unreachable!()
                };
                // The selected chip's fill is the sliding highlight drawn in the composite pass,
                // which also sets these white glyphs' level (`CompositeElem::levels`).
                if is_glass {
                    let smoke_a = 0.20 + 0.10 * elem.hover;
                    push_shape(out, [ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.10, smoke_a]);
                } else {
                    let bg_a = 0.08 * elem.hover * (1.0 - elem.active) + 0.06 * elem.press;
                    if bg_a > 0.002 {
                        push_shape(out, [ox, oy, bw, bh], bh * 0.5, [1.0, 1.0, 1.0, bg_a]);
                    }
                }
                let ic = atlas.icon(IconKind::from_mode(mode));
                let tx = atlas.label(elem.id.label());
                push_sprite(
                    out,
                    [ox + SEG_PX, oy + (CHIP_H - SEG_ICON) * 0.5, SEG_ICON, SEG_ICON],
                    ic.uv,
                    [1.0, 1.0, 1.0, 1.0],
                );
                push_sprite(
                    out,
                    text_at(tx, ox + SEG_PX + SEG_ICON + SEG_ICON_GAP),
                    tx.uv,
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            ElemId::Record | ElemId::Finish | ElemId::Paste => {
                let tx = atlas.label(elem.id.label());
                // `ACCENT_COLOR = 0x3b5cf6`, `ACCENT_HOVER = 0x4b6bff`
                let r = (0.231 + 0.063 * elem.hover - 0.03 * elem.press).clamp(0.0, 1.0);
                let g = (0.361 + 0.059 * elem.hover - 0.04 * elem.press).clamp(0.0, 1.0);
                let b = (0.965 + 0.035 * elem.hover - 0.04 * elem.press).clamp(0.0, 1.0);
                push_shape(out, [ox, oy, bw, bh], bh * 0.5, [r, g, b, 1.0]);
                push_sprite(out, center_text(tx), tx.uv, [0.95, 0.95, 0.96, 1.0]);
            }
            ElemId::Lead => {
                if is_glass {
                    push_shape(out, [ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.10, 0.24]);
                }
                let tx = atlas.clock_sprite;
                let cycle = 1.6_f32;
                let ripple = (t / cycle).fract();
                let breath = 0.5 + 0.5 * (std::f32::consts::TAU * t / cycle).cos();
                let halo_r = 4.0 + 6.0 * ripple;
                let halo_a = 0.34 * (1.0 - ripple).powi(2);
                push_shape(
                    out,
                    [
                        ox + 16.0 - halo_r,
                        oy + 16.0 - halo_r,
                        halo_r * 2.0,
                        halo_r * 2.0,
                    ],
                    halo_r,
                    [0.941, 0.267, 0.220, halo_a],
                );
                push_shape(
                    out,
                    [ox + 12.0, oy + 12.0, 8.0, 8.0],
                    4.0,
                    [0.941, 0.267, 0.220, 0.78 + 0.22 * breath],
                );
                push_sprite(
                    out,
                    text_at(tx, ox + LEAD_TEXT_X),
                    tx.uv,
                    [0.961, 0.961, 0.969, 1.0],
                );
            }
            ElemId::Undo | ElemId::Cancel | ElemId::Inspect => {
                if is_glass {
                    let smoke_a = if elem.enabled {
                        0.20 + 0.12 * elem.hover + 0.06 * elem.press
                    } else {
                        0.12
                    };
                    push_shape(out, [ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.10, smoke_a]);
                } else if elem.enabled && (elem.hover > 0.01 || elem.press > 0.01) {
                    let bg_a = 0.08 * elem.hover + 0.06 * elem.press;
                    push_shape(out, [ox, oy, bw, bh], bh * 0.5, [1.0, 1.0, 1.0, bg_a]);
                }
                let tx = atlas.label(elem.id.label());
                let fg = if !elem.enabled {
                    if is_glass { 0.78 } else { 0.56 }
                } else if is_glass {
                    0.92 + 0.08 * elem.hover
                } else {
                    0.74 + 0.22 * elem.hover
                };
                let fg_a = if !elem.enabled { 0.90 } else { 1.0 };
                push_sprite(out, center_text(tx), tx.uv, [fg, fg, fg, fg_a]);
            }
            ElemId::Close => {
                if is_glass {
                    let smoke_a = 0.20 + 0.12 * elem.hover + 0.06 * elem.press;
                    push_shape(
                        out,
                        [ox + 2.0, oy + 2.0, bh - 4.0, bh - 4.0],
                        (bh - 4.0) * 0.5,
                        [0.06, 0.07, 0.10, smoke_a],
                    );
                } else if elem.hover > 0.01 || elem.press > 0.01 {
                    let bg_a = 0.10 * elem.hover + 0.06 * elem.press;
                    push_shape(
                        out,
                        [ox + 2.0, oy + 2.0, bh - 4.0, bh - 4.0],
                        (bh - 4.0) * 0.5,
                        [1.0, 1.0, 1.0, bg_a],
                    );
                }
                let ic = atlas.icon(IconKind::Close);
                let fg = if is_glass {
                    0.90 + 0.10 * elem.hover
                } else {
                    0.61 + 0.35 * elem.hover
                };
                push_sprite(
                    out,
                    [ox + (CHIP_H - 12.0) * 0.5, oy + (CHIP_H - 12.0) * 0.5, 12.0, 12.0],
                    ic.uv,
                    [fg, fg, fg, 1.0],
                );
            }
            ElemId::Processing => {
                let head = (t * 8.0) % 8.0;
                for k in 0..8 {
                    let ang =
                        (k as f32) * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
                    let dcx = ox + 16.0 + 6.2 * ang.cos();
                    let dcy = oy + 16.0 + 6.2 * ang.sin();
                    let phase_d = (head - k as f32).rem_euclid(8.0);
                    let da = (1.0 - phase_d / 7.5).clamp(0.18, 1.0);
                    push_shape(
                        out,
                        [dcx - 1.35, dcy - 1.35, 2.7, 2.7],
                        1.35,
                        [0.961, 0.961, 0.969, da],
                    );
                }
                let tx = atlas.label(elem.id.label());
                push_sprite(
                    out,
                    text_at(tx, ox + PROCESSING_TEXT_X),
                    tx.uv,
                    [0.961, 0.961, 0.969, 1.0],
                );
            }
            ElemId::DoneLead => {
                push_shape(
                    out,
                    [ox + 5.0, oy + 5.0, 22.0, 22.0],
                    11.0,
                    [0.412, 0.847, 0.624, 0.22],
                );
                let ic = atlas.icon(IconKind::Check);
                push_sprite(
                    out,
                    [ox + (CHIP_H - 13.0) * 0.5, oy + (CHIP_H - 13.0) * 0.5, 13.0, 13.0],
                    ic.uv,
                    [0.412, 0.847, 0.624, 1.0],
                );
                let tx = atlas.completed_sprite;
                push_sprite(
                    out,
                    text_at(tx, ox + DONE_TEXT_X),
                    tx.uv,
                    [0.961, 0.961, 0.969, 1.0],
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_pipelines_compile_and_initialize() {
        let pipelines = MetalPipelines::new(2.0).expect("MetalPipelines::new failed");
        assert_eq!(pipelines.scale, 2.0);
    }
}
