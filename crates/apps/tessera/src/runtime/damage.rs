use super::*;

// The damage *value layer* lives in `tessera-presentation` (api tier): frame
// damage verdicts, the assessment split, slot-ring repaint history, the
// logical→physical mapping, and the `ClientDamageTracker` generation diff
// behind the narrow `SurfaceDamageFrame` seam. This module keeps only the
// composition-root orchestration: sampling the change signals spread across
// the server, shell, notifications, and wallpaper, and deciding what the
// frame's output and backdrop damage are.
pub(super) use tessera_presentation::{
    composite_repaint_for_slot, logical_rects_to_frame, record_composite_present,
    union_frame_damage, AssessedFrameDamage, ClientDamage, ClientDamageTracker, DamageAssessment,
    FrameDamage, SurfaceDamageFrame,
};

/// Wall-clock minute, used to keep the status-bar clock honest: chrome draws
/// `HH:MM` from the system clock, so at least one frame must be presented
/// after each minute rollover even when nothing else changed.
pub(super) fn wall_clock_minute() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 60)
        .unwrap_or(0)
}

fn as_damage_frame<F: SurfaceDamageFrame>(frame: &F) -> &dyn SurfaceDamageFrame {
    frame
}

impl CompositorRuntime {
    /// Damage from client surface commits since the previous assessment.
    /// Every presentation-visible surface class the scene draw pulls feeds
    /// the tracker: toplevels, subsurfaces, overlays (including the client
    /// cursor surface), and lock surfaces, in both SHM and dma-buf form.
    fn client_damage(&mut self) -> ClientDamage {
        let server = &self.server;
        // Share one visibility/occlusion snapshot across the SHM and dma-buf
        // client lists (see `Server::desktop_frame_sets`): the damage pass
        // previously recomputed the O(windows × surfaces) occlusion walk once
        // per list, and the render/scanout paths repeat the same work again.
        let client_sets = server.desktop_frame_sets();
        let overlay = server.overlay_frames();
        let lock = server.lock_frames();
        let overlay_dmabuf = server.overlay_dmabuf_frames();
        let lock_dmabuf = server.lock_dmabuf_frames();
        self.damage.client_damage.assess(
            client_sets
                .shm
                .iter()
                .map(as_damage_frame)
                .chain(overlay.iter().map(as_damage_frame))
                .chain(lock.iter().map(as_damage_frame))
                .chain(client_sets.dmabuf.iter().map(as_damage_frame))
                .chain(overlay_dmabuf.iter().map(as_damage_frame))
                .chain(lock_dmabuf.iter().map(as_damage_frame)),
        )
    }

    /// Assess this frame's output damage from every change signal the
    /// compositor already tracks. Anything not provably unchanged resolves
    /// to [`FrameDamage::Full`]. [`FrameDamage::None`] is the only verdict
    /// that lets the caller skip rendering entirely, so the burden of proof
    /// is on "nothing changed".
    pub(super) fn assess_frame_damage(
        &mut self,
        assessment: DamageAssessment,
    ) -> AssessedFrameDamage {
        let DamageAssessment {
            had_input,
            input_pointer_only,
            session_locked,
            cursor_hidden,
            cursor_shape,
            software_cursor,
            cursor_position,
            cursor_extent,
            scale,
            physical_size,
        } = assessment;
        let (notif_revision, do_not_disturb) = {
            let queue = self.notif_queue.lock().unwrap();
            (queue.revision(), queue.do_not_disturb())
        };
        // Modal chrome states whose transitions are not otherwise signed:
        // overview, keyboard-grabbing chrome (launcher), and the screenshot
        // selector.
        let chrome_mode = (
            self.shell.overview_active(),
            self.shell.window_switcher_active(),
            self.shell.captures_keyboard(),
            self.shell.screenshot_active(),
        );
        let base_full = self.frame_count == 0
            || self.damage.force_full_redraw
            || session_locked != self.damage.last_session_locked
            || (software_cursor
                && self.damage.last_presented_cursor != Some((cursor_shape, cursor_hidden)))
            || self.server.transitions_pending()
            // A live 3D model changes on its own animation clock. Media
            // wallpaper contributes damage only when its absolute source
            // deadline elapsed or a decoded video frame is already waiting;
            // the mere existence of an animated source must not turn every
            // unrelated client event into a full-output composite.
            // A fullscreen window covers the output completely, so the
            // wallpaper cannot be visible: advancing it would repaint (and
            // re-blur, via the backdrop term below) pixels that never reach
            // the display at the animation's frame rate. The source still
            // advances its wall clock, so unfullscreening resumes in step.
            || (!session_locked
                && !self.server.visible_fullscreen_window()
                && self
                    .wallpaper
                    .as_ref()
                    .is_some_and(|w| {
                        w.has_model()
                            || w.next_frame_in()
                                .is_some_and(|remaining| remaining.is_zero())
                    }))
            // Server-side topology: window list/geometry, workspace, output,
            // and Interaction Domain model changes all feed both the scene and the chrome.
            || self.last_windows_hash != Some(self.server.windows_signature())
            // The dock's workspace-global strip follows its own hash; a change
            // on a hidden workspace (map/unmap/title) moves only this one.
            || self.last_all_windows_hash != Some(self.server.all_windows_signature())
            || self.last_ws_sig != Some(self.server.workspace_signature())
            || self.last_outputs_revision != Some(self.server.outputs_revision())
            || self.last_interaction_domain_revision != Some(self.server.interaction_domain_revision())
            // Toasts and the do-not-disturb indicator follow the notification
            // queue (arrivals, dismissals, expiry).
            || self.damage.last_notif_revision != Some(notif_revision)
            || self.damage.last_chrome_mode != Some(chrome_mode)
            // Shell mutations applied outside the signed paths (status poller,
            // config reload, app rescan, IPC settings/Interaction Domain control).
            || self.damage.chrome_dirty
            // The fanout pushes these into chrome when they drift.
            || self.system_status.do_not_disturb != do_not_disturb
            // The status-bar clock is read from wall time at render; force a
            // frame after each minute rollover.
            || self.damage.last_present_minute != Some(wall_clock_minute());

        // Only conditions that change pixels in the desktop scene sampled by
        // a backdrop belong here. Pure shell/input/clock/notification damage
        // still repaints the output, but must not invalidate the expensive
        // capture + blur cache.
        let backdrop_full = self.frame_count == 0
            || self.damage.force_full_redraw
            || session_locked != self.damage.last_session_locked
            || self.server.transitions_pending()
            || (!session_locked
                && !self.server.visible_fullscreen_window()
                && self.wallpaper.as_ref().is_some_and(|w| {
                    w.has_model()
                        || w.next_frame_in()
                            .is_some_and(|remaining| remaining.is_zero())
                }))
            || self.last_windows_hash != Some(self.server.windows_signature())
            || self.last_ws_sig != Some(self.server.workspace_signature())
            || self.last_outputs_revision != Some(self.server.outputs_revision())
            || self.last_interaction_domain_revision != Some(self.server.interaction_domain_revision())
            // The switcher/live-preview scene is captured below chrome and is
            // therefore backdrop source content, unlike ordinary shell UI.
            || self.shell.window_switcher_active()
                != self
                    .damage
                    .last_chrome_mode
                    .map(|(_, switcher, _, _)| switcher)
                    .unwrap_or(false);

        // Chrome animations repaint only the band they animate in when the
        // shell can state it; `None` (an animated component without a
        // footprint, or lens's own eased state) keeps the conservative
        // full-output repaint. The region is damage in its own right — a
        // ticking dock spring advances every frame with no input and no
        // client commit — so it joins the output rects below, not just the
        // pointer-path union.
        let chrome_anim_region = if self.shell.anim_pending() {
            self.shell.anim_damage_region(self.input_acc.display_size)
        } else {
            None
        };
        let base_full = base_full || (self.shell.anim_pending() && chrome_anim_region.is_none());

        // Pointer-only input repaints the cursor sprite's swept area plus
        // the chrome region that reacts to the pointer (hover highlights,
        // the software cursor, magnify tooltips). Any other input keeps the
        // conservative full damage: a click can press a chrome control, a
        // key can change focus, and those effects are not localized.
        let input_damage: Option<Vec<tessera_model::Rect>> = if had_input && !base_full {
            if input_pointer_only {
                let display = self.input_acc.display_size;
                // The union of the previous and current cursor footprints —
                // a moving sprite leaves stale pixels at its old position
                // that must repaint even though nothing draws there anymore.
                let extent = cursor_extent.max(1.0);
                let swept = |position: (f32, f32)| {
                    let x0 = (position.0 - extent).max(0.0).floor() as i32;
                    let y0 = (position.1 - extent).max(0.0).floor() as i32;
                    let x1 = (position.0 + extent).min(display.0).ceil() as i32;
                    let y1 = (position.1 + extent).min(display.1).ceil() as i32;
                    tessera_model::Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
                };
                let mut rect = swept(cursor_position);
                if let Some(previous) = self.damage.last_presented_cursor_position {
                    // The baseline stores physical pixels; the swept
                    // footprint here is logical. Convert back.
                    let logical = (
                        previous.0 as f32 / scale.max(0.001),
                        previous.1 as f32 / scale.max(0.001),
                    );
                    rect = rect.union(swept(logical));
                }
                // Chrome hover state can change anywhere along the pointer's
                // path; union the animated-chrome footprint when one is
                // known, and the pointer-capture band when it is not (the
                // shell reports no footprint only when it cannot localize,
                // which then falls back to full below).
                match chrome_anim_region {
                    Some(region) => Some(vec![rect.union(region)]),
                    None => {
                        if self.shell.captures_pointer_at(
                            cursor_position.0,
                            cursor_position.1,
                            display,
                        ) {
                            // The pointer is over chrome that could not
                            // localize its animation: conservative full.
                            None
                        } else {
                            Some(vec![rect])
                        }
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let output_full = base_full || input_damage.is_none() && had_input;

        self.damage.last_session_locked = session_locked;
        self.damage.last_notif_revision = Some(notif_revision);
        self.damage.last_chrome_mode = Some(chrome_mode);
        self.damage.chrome_dirty = false;

        // Even when output chrome forces a full repaint, collect client damage
        // unless the backdrop source itself is already known to be full. This
        // keeps the effect invalidation precise and advances surface baselines
        // instead of rediscovering old client commits on a later frame.
        let client = (!backdrop_full).then(|| self.client_damage());
        let backdrop_source = if backdrop_full {
            FrameDamage::Full
        } else {
            client
                .clone()
                .unwrap_or(ClientDamage::None)
                .into_frame_damage(scale, physical_size)
        };
        let output = if output_full {
            FrameDamage::Full
        } else {
            // Pointer-only input and localized chrome animations join the
            // client damage as additional physical rectangles. The chrome
            // animation region is independent damage: a ticking spring
            // advances the frame with no input and no client commit.
            let mut extra: Vec<tessera_model::Rect> = Vec::new();
            if let Some(region) = chrome_anim_region {
                extra.push(region);
            }
            if let Some(rects) = input_damage {
                extra.extend(rects);
            }
            let base = client
                .unwrap_or(ClientDamage::None)
                .into_frame_damage(scale, physical_size);
            if extra.is_empty() {
                base
            } else {
                let extra = logical_rects_to_frame(extra, scale, physical_size);
                match base {
                    FrameDamage::Full => FrameDamage::Full,
                    other => union_frame_damage(extra, other),
                }
            }
        };
        AssessedFrameDamage {
            output,
            backdrop_source,
        }
    }
}
