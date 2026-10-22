//! Screenshots in the app, behind `"experiments": {"screenshots": true}` (`crate::screenshot`).
//! The screenshot shortcut opens the picker in its still mode; whatever it picks goes to
//! `take_screenshot` instead of starting a recording.

use std::time::{Duration, Instant};

use gpui::Context;

use super::{IslandPhase, ScrawlCoordinator, SelectionMode, close_picker_window};
use crate::capture::scrawl_log;
use crate::model::TargetInfo;

/// How often the app looks whether the experiment (or the stored shortcut) changed.
const POLL: Duration = Duration::from_secs(2);

impl ScrawlCoordinator {
    /// The screenshot shortcut: opens the picker on Area in still mode. A second press while
    /// it's open cancels it.
    pub(super) fn press_screenshot(&mut self, cx: &mut Context<Self>) {
        if !crate::handoff::experiment(crate::screenshot::SCREENSHOTS) {
            return;
        }
        if self.still && self.is_selecting() {
            self.cancel_recording(cx);
            return;
        }
        if self.is_recording()
            || self.is_processing()
            || self.is_selecting()
            || self.shortcut.is_pending()
        {
            return;
        }
        if self.gate.is_pending() || !self.ready_to_record() {
            return;
        }
        scrawl_log("screenshot: opening the picker");
        self.still = true;
        self.open_picker(Some(SelectionMode::Area), cx);
    }

    /// The picker picked `target` in still mode: one frame of it becomes a screenshot bundle.
    pub(super) fn take_screenshot(&mut self, target: TargetInfo, cx: &mut Context<Self>) {
        self.still = false;
        let started = Instant::now();
        self.iphone_preview = None;
        self.selection_drag = None;
        self.hovered_window = None;
        self.animated_window_rect = None;
        self.telemetry
            .selecting
            .store(false, std::sync::atomic::Ordering::Release);
        self.telemetry.set_picker_rect(None);
        // Scrawl's windows are left out of the still by id and level, so the picker's dimming
        // can go now.
        self.picker_metal_overlay = None;
        self.completed_note = None;
        self.route_chips.clear();
        if let Some(ref pill) = self.native_pill {
            let f = target.frame;
            pill.place_on([f.x + f.width / 2.0, f.y + f.height / 2.0]);
        }
        self.set_island_phase(IslandPhase::Processing);
        cx.defer(close_picker_window);
        cx.notify();
        let excluded = self
            .telemetry
            .excluded_window_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let dir = crate::handoff::default_captures_dir()
            .join(format!("{}.scrawl", crate::make_bundle_id()));
        scrawl_log(format!(
            "screenshot: {:?} {:?} into {}",
            target.mode,
            target.frame,
            dir.display()
        ));
        cx.spawn(async move |entity, cx| {
            let shot = cx
                .background_executor()
                .spawn(async move {
                    let frame = crate::capture::capture_still(&target, &excluded)?;
                    let png = crate::render::encode_png(&frame)?;
                    let bundle = crate::screenshot::write_bundle(&dir, target, &png)?;
                    anyhow::Ok((dir, bundle, png))
                })
                .await;
            let _ = entity.update(cx, |coord, cx| {
                let (dir, bundle, png) = match shot {
                    Ok(shot) => shot,
                    Err(err) => {
                        scrawl_log(format!("screenshot failed: {err:#}"));
                        coord.dismiss_island(cx);
                        return;
                    }
                };
                let path = dir.join(crate::screenshot::FILE);
                if let Err(err) = crate::handoff::copy_png_and_text_to_clipboard(
                    &png,
                    &path.display().to_string(),
                ) {
                    scrawl_log(format!("screenshot: clipboard: {err:#}"));
                }
                scrawl_log(format!(
                    "screenshot: copied {} in {} ms",
                    path.display(),
                    started.elapsed().as_millis()
                ));
                coord.completed_dir = Some(dir.clone());
                coord.last_local_capture_ms = crate::now_epoch_ms();
                coord.stack_finished(&dir, &bundle);
                coord.set_review_bundle((dir, bundle), None);
                coord.set_island_phase(IslandPhase::Completed {
                    finished_at: Instant::now(),
                    frame_count: 1,
                    duration_ms: 0,
                    inbox_count: 0,
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Every UI tick: registers or drops the screenshot shortcut when the experiment or the
    /// stored shortcut changed (looked at every `POLL`).
    pub(super) fn tick_screenshot_shortcut(&mut self) {
        if self.screenshot_polled.elapsed() < POLL
            || self
                .telemetry
                .shortcuts_paused
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        self.screenshot_polled = Instant::now();
        let want = crate::screenshot::registered(
            crate::handoff::experiment(crate::screenshot::SCREENSHOTS),
            &crate::settings::Shortcuts::load().screenshot,
        );
        if want == self.telemetry.screenshot_shortcut() {
            return;
        }
        let label = if want.is_unset() {
            "off".to_owned()
        } else {
            want.label.clone()
        };
        match crate::capture::set_screenshot_shortcut(&self.telemetry, want) {
            Ok(()) => scrawl_log(format!("screenshot shortcut: {label}")),
            Err(kind) => scrawl_log(format!("screenshot shortcut {label}: {kind:?} refused")),
        }
    }
}
