//! Notification toasts: a frameless, display-only strip stacked at the
//! top-right, newest on top — plain floating text in the VR/AR manner, no
//! background panel, no border, no pointer capture. Each toast is visible
//! for [`TOAST_TTL_MS`] and then simply gone; every interaction (dismissal)
//! lives in the command panel's Messages section, which reads the same
//! queue's much longer retention history.
//!
//! The component reads a shared [`NotificationQueue`] each frame, so it
//! needs no per-frame snapshot from the shell and adds no `Chrome` trait
//! parameter. The queue's retention TTL no longer drives presentation: the
//! toast applies its own window against the compositor clock the queue is
//! ticked with ([`NotificationQueue::now_ms`]).

use std::sync::{Arc, Mutex};

use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::component::{Chrome, ChromeEvents, ChromeUpdate, Localizer, ellipsize};
use tessera_design::Design;
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_desktop::notify::Notification;
use tessera_desktop::notify::NotificationQueue;
use tessera_desktop::window::Window;
use tessera_desktop::workspace::WorkspaceSnapshot;

const TOAST_W: f32 = 300.0;
const TOAST_H: f32 = 64.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_TOP_MARGIN: f32 = crate::component::SCREEN_CORNER_INSET;
const TOAST_RIGHT_MARGIN: f32 = crate::component::SCREEN_CORNER_INSET;
/// Cap the visible stack so a flood does not fill the screen.
const MAX_VISIBLE: usize = 5;
/// How long a toast stays on screen. Aging out is not a queue mutation, so
/// it bumps no revision — [`Self::anim_pending`] keeps the frames coming
/// until every visible toast has crossed the window.
const TOAST_TTL_MS: u64 = 3_000;
/// Frames the vacated strip keeps repainting after the last toast leaves.
///
/// A toast expiring is not a queue mutation, so it cannot rely on
/// `scene_changed`. The compositor's DRM ring may hold three images with
/// different vintages; one clear frame would leave the toast on the other
/// two. Four frames reproduces the strip on every image with margin, then
/// `anim_pending` goes false and the loop idles again. The band is small, so
/// the cost is bounded.
const TOAST_RETIRE_FRAMES: u8 = 4;
/// Start rebuilding this long before the oldest toast expires, so the
/// expiry frame is a real build and can arm the retirement window. Without
/// it the loop wakes at the deadline only to find that the component no
/// longer has a deadline (the toast is already past its window) and skips
/// the build. Two 60 Hz frames of a static strip is negligible.
const TOAST_EXPIRY_LEAD_MS: u64 = 34;

/// Whether `n` is still inside its presentation window at `now_ms`.
fn within_window(n: &Notification, now_ms: u64) -> bool {
    now_ms.saturating_sub(n.at_ms) <= TOAST_TTL_MS
}

/// The notification toast strip. Beyond its borrowed queue handle it caches
/// the rendered entries keyed by the queue's revision, so an unchanged queue
/// is not re-cloned every frame; the per-frame age filter runs on the cached
/// entries. The main loop pushes, expires, and dismisses entries — this
/// component only renders, and captures no input.
pub struct Toast {
    queue: Arc<Mutex<NotificationQueue>>,
    /// `(revision, entries)` of the last clone from the queue; `None` until
    /// the first render (or after do-not-disturb hid the stack).
    cache: Option<(u64, Vec<Notification>)>,
    /// The design snapshot the toast paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
    /// Band painted by the last render, used to arm the retirement window
    /// when the stack becomes empty.
    last_band: Option<Rect>,
    /// Band vacated by the stack that still needs a few repaints, and the
    /// remaining frame budget for it. Non-zero also keeps the component
    /// participating so `damage_regions` is asked for the clear.
    retire_band: Option<Rect>,
    retire_frames: u8,
}

impl Toast {
    /// Construct with a shared queue the main loop also pushes to.
    pub fn new(queue: Arc<Mutex<NotificationQueue>>) -> Toast {
        Toast {
            queue,
            cache: None,
            design: Design::dark(),
            last_band: None,
            retire_band: None,
            retire_frames: 0,
        }
    }

    /// The stacked strip's painted band for `count` visible toasts.
    fn band(display: (f32, f32), count: usize) -> Option<Rect> {
        (count > 0).then(|| {
            let width = (TOAST_W + TOAST_RIGHT_MARGIN).min(display.0.max(1.0));
            let height = TOAST_TOP_MARGIN
                + count as f32 * TOAST_H
                + (count.saturating_sub(1)) as f32 * TOAST_GAP;
            Rect {
                x: (display.0 - width).max(0.0),
                y: 0.0,
                w: width.ceil(),
                h: height.ceil(),
            }
        })
    }

    /// The entries eligible for presentation right now: do-not-disturb off,
    /// inside the toast window, newest first, capped. Reads the queue under
    /// a brief lock; `render` reuses the revision cache instead.
    fn presentable(&self) -> Vec<Notification> {
        let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.do_not_disturb() {
            return Vec::new();
        }
        let now_ms = queue.now_ms();
        queue
            .recent()
            .iter()
            .rev()
            .filter(|n| within_window(n, now_ms))
            .take(MAX_VISIBLE)
            .cloned()
            .collect()
    }
}

impl Chrome for Toast {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let design = self.design;
        // Re-clone only when the queue's revision moved; otherwise render
        // the cached entries. Newest on top: iterate the tail in reverse,
        // capped.
        let notifications: &[Notification] = {
            let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.do_not_disturb() {
                self.cache = None;
                &[]
            } else {
                let stale = self
                    .cache
                    .as_ref()
                    .map(|(revision, _)| *revision != queue.revision())
                    .unwrap_or(true);
                if stale {
                    let entries = queue
                        .recent()
                        .iter()
                        .rev()
                        .take(MAX_VISIBLE)
                        .cloned()
                        .collect();
                    self.cache = Some((queue.revision(), entries));
                }
                &self.cache.as_ref().unwrap().1
            }
        };
        let now_ms = self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .now_ms();
        let mut slot = 0;
        for n in notifications {
            // The age filter runs every frame: a toast crossing its window
            // is not a queue mutation and bumps no revision.
            if !within_window(n, now_ms) {
                continue;
            }
            let rect = Rect {
                x: disp.x - TOAST_W - TOAST_RIGHT_MARGIN,
                y: TOAST_TOP_MARGIN + slot as f32 * (TOAST_H + TOAST_GAP),
                w: TOAST_W,
                h: TOAST_H,
            };
            slot += 1;
            // Frameless: transparent background, no border, no radius —
            // floating text over the desktop.
            let opts = LayoutOpts {
                bg: Color::TRANSPARENT,
                border: Color::TRANSPARENT,
                border_width: 0.0,
                radius: 0.0,
                pad: 0.0,
                ..surface_layout()
            };
            let id: usize = n.id as usize;
            let overlay_id = format!("tessera-toast-{id}");
            f.place(&overlay_id, &chrome_place(rect, opts), |f| {
                let title = match &n.app_id {
                    Some(app) => format!("{} · {app}", n.summary),
                    None => n.summary.clone(),
                };
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: 3.0,
                        pad: 9.0,
                        cross: Align::Start,
                        ..Default::default()
                    },
                    |f| {
                        let text_width = (rect.w - 18.0).max(0.0);
                        let title = ellipsize(f, &title, design.typography.label, text_width);
                        f.label_compact_sized(&title, design.typography.label);
                        if !n.body.is_empty() {
                            let body =
                                ellipsize(f, &n.body, design.typography.footnote, text_width);
                            f.label_compact_sized(&body, design.typography.footnote);
                        }
                    },
                );
            });
        }

        // The strip is static; its only lifecycle event is "the last toast
        // left". Arm a short retirement window so every ring image receives
        // the clear, even though no queue revision or animation reported it.
        let band = Self::band((disp.x, disp.y), slot);
        let armed_now = band.is_none() && self.last_band.is_some();
        if armed_now {
            self.retire_band = self.last_band;
            self.retire_frames = TOAST_RETIRE_FRAMES;
        } else if band.is_some() {
            self.retire_band = None;
            self.retire_frames = 0;
        } else if self.retire_frames > 0 {
            self.retire_frames -= 1;
        }
        self.last_band = band;
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        if let ChromeUpdate::Appearance(design) = update {
            self.design = *design;
        }
    }

    fn anim_pending(&self) -> bool {
        // A toast is a static strip that vanishes when its window closes. It
        // has no fade, so it must not pump the frame loop for its whole
        // lifetime. `animation_deadline_in` schedules the expiry wake, and the
        // short lead below turns that wake into a build; the retirement window
        // then repaints the vacated band across the swapchain ring.
        if self.retire_frames > 0 {
            return true;
        }
        // The stack is empty but the last painted band is still owned: this is
        // the frame that must rebuild, observe the empty stack in `render`,
        // and arm the retirement window.
        if self.last_band.is_some() && self.presentable().is_empty() {
            return true;
        }
        let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.do_not_disturb() {
            return false;
        }
        let now_ms = queue.now_ms();
        queue
            .recent()
            .iter()
            .rev()
            .filter(|n| within_window(n, now_ms))
            .map(|n| TOAST_TTL_MS.saturating_sub(now_ms.saturating_sub(n.at_ms)))
            .min()
            .is_some_and(|remaining| remaining <= TOAST_EXPIRY_LEAD_MS)
    }

    fn animation_deadline_in(&self) -> Option<std::time::Duration> {
        // Wake exactly when the oldest presentable toast crosses its window:
        // that frame clears it, and no frame is needed before it.
        let queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.do_not_disturb() {
            return None;
        }
        let now_ms = queue.now_ms();
        queue
            .recent()
            .iter()
            .rev()
            .filter(|n| within_window(n, now_ms))
            .map(|n| {
                let age = now_ms.saturating_sub(n.at_ms);
                std::time::Duration::from_millis(TOAST_TTL_MS.saturating_sub(age).max(1))
            })
            .min()
    }

    fn damage_regions(&self, _windows: &[Window], display: (f32, f32)) -> Option<Vec<Rect>> {
        let current = Self::band(display, self.presentable().len());
        let mut regions = Vec::new();
        if let Some(rect) = current {
            regions.push(rect);
        }
        if self.retire_frames > 0
            && let Some(rect) = self.retire_band
        {
            regions.push(rect);
        }
        (!regions.is_empty()).then_some(regions)
    }

    fn requires_composition(&self) -> bool {
        // `last_band` keeps the component in the pass for the one build that
        // observes the empty stack and arms the retirement window. Without it
        // the component would be filtered out before `render`, so the band it
        // painted last frame would never be named as damage.
        self.retire_frames > 0 || self.last_band.is_some() || !self.presentable().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_copy_is_unicode_safe_and_bounded() {
        assert_eq!(
            crate::component::truncate("Fuji connected", 20),
            "Fuji connected"
        );
        assert_eq!(
            crate::component::truncate("真实通知已经联通", 6),
            "真实通知已…"
        );
    }

    #[test]
    fn presentation_window_is_three_seconds_inclusive() {
        let n = Notification {
            id: 0,
            summary: "title".into(),
            body: String::new(),
            app_id: None,
            external_id: None,
            at_ms: 10_000,
        };
        assert!(within_window(&n, 10_000), "fresh toast is visible");
        assert!(within_window(&n, 13_000), "exactly 3s is still visible");
        assert!(!within_window(&n, 13_001), "past 3s the toast is gone");
        // A clock reading older than the post time never underflows.
        assert!(within_window(&n, 5_000));
    }

    #[test]
    fn an_empty_stack_after_a_band_keeps_the_toast_participating() {
        // The expiry build must still call `render` so the component can arm
        // its retirement window; filtering it out because the stack is empty
        // would leave the last band unowned.
        let queue = Arc::new(Mutex::new(NotificationQueue::new(10_000)));
        let mut toast = Toast::new(queue);
        toast.last_band = Some(Rect {
            x: 10.0,
            y: 0.0,
            w: 300.0,
            h: 64.0,
        });
        assert!(toast.requires_composition());
        assert!(toast.anim_pending());
    }
}
