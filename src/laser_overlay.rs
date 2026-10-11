use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use core_graphics_types::geometry::CGSize;
use core_video::base::{CVOptionFlags, CVTimeStamp};
use core_video::display_link::{CVDisplayLink, CVDisplayLinkRef};
use core_video::r#return::CVReturn;
use metal::{
    MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLStoreAction, MetalLayer,
    RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSStatusWindowLevel, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::CALayer;

use crate::capture::SharedCaptureTelemetry;
use crate::sketch::{slice_laser_window, smooth_ink_points, smooth_laser_points};

const TAIL_DURATION_SEC: f32 = 0.42;
const MAX_SEGMENTS: usize = 1536;
const MAX_PINS: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LaserUniforms {
    resolution: [f32; 2],
    scale: f32,
    now_sec: f32,
    tail_duration: f32,
    tip_active: f32,
    tip_pos: [f32; 2],
    segment_count: u32,
    pin_count: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PathSegment {
    a: [f32; 2],
    b: [f32; 2],
    w0: f32,
    w1: f32,
    kind: f32,
    width: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PinMarker {
    pos: [f32; 2],
    radius: f32,
    _pad: f32,
}

pub struct MetalLaserOverlay {
    window: Retained<NSPanel>,
    display_link: CVDisplayLink,
    display_link_context: *const SharedMetalOverlay,
}

impl MetalLaserOverlay {
    pub fn open(telemetry: Arc<SharedCaptureTelemetry>) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Metal overlay requires the main thread")?;
        let screen = NSScreen::mainScreen(mtm)
            .or_else(|| NSScreen::screens(mtm).firstObject())
            .ok_or("No screen available for Metal overlay")?;
        let frame = screen.frame();
        let scale = (screen.backingScaleFactor() as f32).max(1.0);

        let shared = Arc::new(SharedMetalOverlay::new(
            telemetry,
            frame.size.width as f32,
            frame.size.height as f32,
            scale,
        )?);

        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Borderless
                | NSWindowStyleMask::UtilityWindow
                | NSWindowStyleMask::NonactivatingPanel
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        );
        let view =
            NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        view.setWantsLayer(true);
        let metal_layer = shared.layer_ptr();
        let cocoa_layer = unsafe { &*metal_layer.cast::<CALayer>() };
        cocoa_layer.setOpaque(false);
        view.setLayer(Some(cocoa_layer));
        window.setContentView(Some(&view));
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(false);
        window.setHidesOnDeactivate(false);
        window.setCanHide(false);
        window.setLevel(NSStatusWindowLevel + 1);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::CanJoinAllApplications
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        unsafe { window.setReleasedWhenClosed(false) };
        window.setFrame_display(frame, true);
        window.orderFrontRegardless();

        let display_link = CVDisplayLink::from_active_cg_displays()
            .map_err(|status| format!("Could not create CVDisplayLink: {status}"))?;
        let display_link_context = Arc::into_raw(shared);
        unsafe {
            display_link.set_output_callback(
                display_link_callback,
                display_link_context.cast_mut().cast::<c_void>(),
            )
        }
        .map_err(|status| format!("Could not configure CVDisplayLink callback: {status}"))?;
        let _ = display_link.start();

        Ok(Self {
            window,
            display_link,
            display_link_context,
        })
    }

    pub fn window_number(&self) -> u32 {
        self.window.windowNumber().max(0) as u32
    }
}

impl Drop for MetalLaserOverlay {
    fn drop(&mut self) {
        let _ = self.display_link.stop();
        unsafe { drop(Arc::from_raw(self.display_link_context)) };
        self.window.orderOut(None);
        self.window.close();
    }
}

extern "C" fn display_link_callback(
    _display_link: CVDisplayLinkRef,
    _now: *const CVTimeStamp,
    _output_time: *const CVTimeStamp,
    _flags_in: CVOptionFlags,
    _flags_out: *mut CVOptionFlags,
    context: *mut c_void,
) -> CVReturn {
    if !context.is_null() {
        let overlay = unsafe { &*context.cast::<SharedMetalOverlay>() };
        overlay.draw();
    }
    0
}

struct SharedMetalOverlay {
    telemetry: Arc<SharedCaptureTelemetry>,
    renderer: Mutex<MetalLaserRenderer>,
}

unsafe impl Send for SharedMetalOverlay {}
unsafe impl Sync for SharedMetalOverlay {}

struct MetalLaserRenderer {
    layer: MetalLayer,
    command_queue: metal::CommandQueue,
    pipeline: RenderPipelineState,
    width: f32,
    height: f32,
    scale: f32,
}

impl SharedMetalOverlay {
    fn new(
        telemetry: Arc<SharedCaptureTelemetry>,
        width: f32,
        height: f32,
        scale: f32,
    ) -> Result<Self, String> {
        let device = metal::Device::system_default().ok_or("Metal is unavailable")?;
        let layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        layer.set_opaque(false);
        layer.set_presents_with_transaction(false);
        layer.set_display_sync_enabled(true);
        layer.set_contents_scale(f64::from(scale));
        layer.set_drawable_size(CGSize::new(
            f64::from(width * scale),
            f64::from(height * scale),
        ));

        let library = device
            .new_library_with_data(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/laser_overlay.metallib"
            )))
            .map_err(|e| format!("Could not load laser shader library: {e}"))?;
        let vertex = library
            .get_function("laser_vertex", None)
            .map_err(|e| format!("Could not load laser_vertex: {e}"))?;
        let fragment = library
            .get_function("laser_fragment", None)
            .map_err(|e| format!("Could not load laser_fragment: {e}"))?;

        let descriptor = RenderPipelineDescriptor::new();
        descriptor.set_vertex_function(Some(&vertex));
        descriptor.set_fragment_function(Some(&fragment));
        let attachment = descriptor
            .color_attachments()
            .object_at(0)
            .ok_or("Missing color attachment")?;
        attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        attachment.set_blending_enabled(false);

        let pipeline = device
            .new_render_pipeline_state(&descriptor)
            .map_err(|e| format!("Could not create laser pipeline: {e}"))?;

        Ok(Self {
            telemetry,
            renderer: Mutex::new(MetalLaserRenderer {
                command_queue: device.new_command_queue(),
                layer,
                pipeline,
                width,
                height,
                scale,
            }),
        })
    }

    fn layer_ptr(&self) -> *const metal::MetalLayerRef {
        self.renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .layer
            .as_ref()
    }

    fn draw(&self) {
        if !self.telemetry.active.load(Ordering::Acquire) {
            return;
        }
        let now_ms = self.telemetry.elapsed_ms();
        let now_sec = now_ms as f32 / 1_000.0;
        let tail_ms = (TAIL_DURATION_SEC * 1_000.0) as u64;
        let min_laser_ms = now_ms.saturating_sub(tail_ms);

        let target_frame = *self
            .telemetry
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = target_frame.x as f32;
        let ty = target_frame.y as f32;
        let tw = (target_frame.width as f32).max(1.0);
        let th = (target_frame.height as f32).max(1.0);
        let to_screen = |nx: f32, ny: f32| -> [f32; 2] { [tx + nx * tw, ty + ny * th] };

        let mut strokes = self
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(ref active_laser) = *self
            .telemetry
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            strokes.push(active_laser.clone());
        }
        if let Some(ref active_ink) = *self
            .telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            strokes.push(active_ink.clone());
        }

        let mut segments = Vec::with_capacity(256);
        let mut pins = Vec::new();
        let ink_width = (tw.min(th) * 0.0085).clamp(4.5, 10.0);

        for stroke in &strokes {
            if stroke.pin.is_some() {
                if let Some([nx, ny]) = stroke.normalized_points().first().copied()
                    && pins.len() < MAX_PINS
                {
                    pins.push(PinMarker {
                        pos: to_screen(nx, ny),
                        radius: 11.5,
                        _pad: 0.0,
                    });
                }
                continue;
            }
            if stroke.laser {
                if stroke.t1 < min_laser_ms {
                    continue;
                }
                let raw = slice_laser_window(&stroke.timed_points(), now_ms, tail_ms, to_screen);
                let smoothed = smooth_laser_points(&raw);
                for pair in smoothed.windows(2) {
                    if segments.len() >= MAX_SEGMENTS {
                        break;
                    }
                    let (a, w0) = pair[0];
                    let (b, w1) = pair[1];
                    segments.push(PathSegment {
                        a,
                        b,
                        w0,
                        w1,
                        kind: 0.0,
                        width: 3.0,
                    });
                }
            } else {
                let pts: Vec<[f32; 2]> = stroke
                    .normalized_points()
                    .into_iter()
                    .map(|[nx, ny]| to_screen(nx, ny))
                    .collect();
                let smoothed = smooth_ink_points(&pts);
                for pair in smoothed.windows(2) {
                    if segments.len() >= MAX_SEGMENTS {
                        break;
                    }
                    segments.push(PathSegment {
                        a: pair[0].0,
                        b: pair[1].0,
                        w0: pair[0].1,
                        w1: pair[1].1,
                        kind: 1.0,
                        width: ink_width,
                    });
                }
            }
        }

        let shift_held = self.telemetry.shift_held.load(Ordering::Acquire);
        let pointer = *self
            .telemetry
            .pointer_screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if segments.is_empty() {
            segments.push(PathSegment::default());
        }
        if pins.is_empty() {
            pins.push(PinMarker::default());
        }

        let renderer = self
            .renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        objc2::rc::autoreleasepool(|_| {
            let Some(drawable) = renderer.layer.next_drawable() else {
                return;
            };
            let pass = RenderPassDescriptor::new();
            let attachment = pass.color_attachments().object_at(0).unwrap();
            attachment.set_texture(Some(drawable.texture()));
            attachment.set_load_action(MTLLoadAction::Clear);
            attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            attachment.set_store_action(MTLStoreAction::Store);

            let uniforms = LaserUniforms {
                resolution: [
                    renderer.width * renderer.scale,
                    renderer.height * renderer.scale,
                ],
                scale: renderer.scale,
                now_sec,
                tail_duration: TAIL_DURATION_SEC,
                tip_active: if shift_held { 1.0 } else { 0.0 },
                tip_pos: pointer,
                segment_count: if segments.len() == 1 && segments[0].width == 0.0 {
                    0
                } else {
                    segments.len().min(MAX_SEGMENTS) as u32
                },
                pin_count: if pins.len() == 1 && pins[0].radius == 0.0 {
                    0
                } else {
                    pins.len().min(MAX_PINS) as u32
                },
                _pad0: 0,
                _pad1: 0,
            };

            let cmd = renderer.command_queue.new_command_buffer();
            let enc = cmd.new_render_command_encoder(pass);
            enc.set_render_pipeline_state(&renderer.pipeline);
            enc.set_fragment_bytes(
                0,
                size_of::<LaserUniforms>() as u64,
                (&raw const uniforms).cast(),
            );
            enc.set_fragment_bytes(
                1,
                (segments.len() * size_of::<PathSegment>()) as u64,
                segments.as_ptr().cast(),
            );
            enc.set_fragment_bytes(
                2,
                (pins.len() * size_of::<PinMarker>()) as u64,
                pins.as_ptr().cast(),
            );
            enc.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
            enc.end_encoding();
            cmd.present_drawable(drawable);
            cmd.commit();
        });
    }
}
