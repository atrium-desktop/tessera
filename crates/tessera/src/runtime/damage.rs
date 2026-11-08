use super::*;

// The damage *value layer* lives in `tessera-presentation` (api tier): frame
// damage verdicts, the assessment split, slot-ring repaint history, the
// logical→physical mapping, and the `ClientDamageTracker` generation diff
// behind the narrow `SurfaceDamageFrame` seam. This module keeps only the
// composition-root orchestration: sampling the change signals spread across
// the server, shell, notifications, and wallpaper, and deciding what the
// frame's output and backdrop damage are.
pub(super) use tessera_composite::presentation::{
    AssessedFrameDamage, ClientDamage, ClientDamageTracker, DamageAssessment, FrameDamage,
    SurfaceDamageFrame, composite_repaint_for_slot, logical_rects_to_frame,
    record_composite_present, union_frame_damage,
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

impl CompositorRuntime<'_> {
    /// Damage from client surface commits since the previous assessment.
    /// Every presentation-visible surface class the scene draw pulls feeds
    /// the tracker: toplevels, subsurfaces, overlays (including the client
    /// cursor surface), and lock surfaces, in both SHM and dma-buf form.
    ///
    /// Also returns the resource ids of every surface sampled this pass. These
    /// are exactly the surfaces whose damage baseline the frame consumes, so
    /// they — and only they — may be marked presented after a successful
    /// submission (`[INV-ARCH-70]`).
    fn client_damage(&mut self) -> (ClientDamage, Vec<usize>) {
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
        let mut sampled = Vec::with_capacity(
            client_sets.shm.len() + overlay.len() + lock.len() + client_sets.dmabuf.len(),
        );
        let damage = self.damage.client_damage.assess(
            client_sets
                .shm
                .iter()
                .map(as_damage_frame)
                .chain(overlay.iter().map(as_damage_frame))
                .chain(lock.iter().map(as_damage_frame))
                .chain(client_sets.dmabuf.iter().map(as_damage_frame))
                .chain(overlay_dmabuf.iter().map(as_damage_frame))
                .chain(lock_dmabuf.iter().map(as_damage_frame))
                .inspect(|frame| sampled.push(frame.id())),
        );
        (damage, sampled)
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
        let notif_revision = self
            .notif_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revision();
        // Modal chrome states whose transitions are not otherwise signed:
        // overview, keyboard-grabbing chrome (launcher), and the screenshot
        // selector.
        let chrome_mode = (
            self.shell.overview_active(),
            self.shell.window_switcher_active(),
            self.shell.captures_keyboard(),
            self.shell.screenshot_active(),
        );
        let windows_hash = self.server.windows_signature();
        let all_windows_hash = self.server.all_windows_signature();
        let ws_sig = self.server.workspace_signature();
        let outputs_revision = self.server.outputs_revision();
        let interaction_domain_revision = self.server.interaction_domain_revision();
        let base_full = self.frame_count == 0
            || self.damage.force_full_redraw
            || self.screenshot_freeze.needs_capture()
            || session_locked != self.damage.last_session_locked
            || (software_cursor
                && self.damage.last_presented_cursor != Some((cursor_shape, cursor_hidden)))
            // Only *animating* transitions force a full assessment: a settled
            // ghost's hand-off frame changes exactly its own rect, which arrives
            // as local damage through `transition_damage_rects` below. Reporting
            // the hand-off as full here made the recorded rect dead on arrival
            // (ADR-0220).
            || self.server.live_transitions_pending()
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
                && !self.space_use.client_owns_output()
                && self
                    .wallpaper
                    .as_ref()
                    .is_some_and(|w| {
                        w.has_model()
                            || w.next_frame_in()
                                .is_some_and(|remaining| remaining.is_zero())
                    }))
            // Server-side topology: workspace, output, and Interaction Domain
            // model changes feed both the scene and the chrome in full.
            || self.last_ws_sig != Some(ws_sig)
            || self.last_outputs_revision != Some(outputs_revision)
            || self.last_interaction_domain_revision != Some(interaction_domain_revision)
            || self.damage.last_chrome_mode != Some(chrome_mode)
            // Shell mutations applied outside the signed paths (status poller,
            // config reload, app rescan, IPC settings/Interaction Domain control).
            || self.damage.chrome_dirty;

        let transition_rects = self.server.transition_damage_rects();

        // Localized extra damage rects for chrome and server state changes:
        let mut chrome_extra_rects = Vec::new();
        let display = self.input_acc.display_size;
        let reserved = self.shell.reserved();

        // 1. When the dock's global window strip changes (e.g. app mapped on another workspace):
        if self.last_all_windows_hash != Some(all_windows_hash) {
            let dock_h = reserved.bottom.max(64);
            chrome_extra_rects.push(tessera_primitives::Rect::new(
                0,
                (display.1 as i32) - dock_h,
                display.0 as i32,
                dock_h,
            ));
        }

        // 2. When DND status changes: the indicator is drawn inside the
        // command panel, which repaints as a whole while it is open, so nothing
        // outside it depends on the flag. The minute rollover used to repaint
        // the status strip; that strip is gone (ADR-0213) and the clock now
        // lives on the desktop layer, whose own damage report covers it, so
        // neither change schedules a chrome repaint here.

        // 3. When notification toast arrives/dismisses:
        if self.damage.last_notif_revision != Some(notif_revision) {
            chrome_extra_rects.push(tessera_primitives::Rect::new(
                (display.0 as i32).saturating_sub(420),
                0,
                420,
                360,
            ));
        }

        // 4. When window focus changes without client buffer commits:
        if self.last_windows_hash != Some(windows_hash) {
            let windows = self.server.windows();
            let focused_window = windows.iter().find(|w| w.state.activated);
            let focused_id = focused_window.map(|w| w.id);
            if focused_id != self.damage.last_focused_window {
                if let Some(win) = focused_window {
                    chrome_extra_rects.push(tessera_primitives::Rect::new(
                        win.position.x - 48,
                        win.position.y - 48,
                        win.size.w + 96,
                        win.size.h + 96,
                    ));
                }
                if let Some(prev_id) = self.damage.last_focused_window
                    && let Some(prev_win) = windows.iter().find(|w| w.id == prev_id)
                {
                    chrome_extra_rects.push(tessera_primitives::Rect::new(
                        prev_win.position.x - 48,
                        prev_win.position.y - 48,
                        prev_win.size.w + 96,
                        prev_win.size.h + 96,
                    ));
                }
                self.damage.last_focused_window = focused_id;
            }
        }

        // Chrome damage: in-flight animations, plus the one-shot structural
        // changes a component reported while building the scene (a dock tile's
        // context menu opening or closing). Both repaint only what the shell can
        // localize; `None` (a component without a footprint) keeps the
        // conservative full-output repaint. The regions are damage in their own
        // right — a ticking dock spring advances every frame with no input and no
        // client commit — so they join the output rects below, not just the
        // pointer-path union.
        //
        // A caret blink *flip* is a third trigger and is not an animation
        // (Optics ADR-0151): it is a two-state phase, so exactly the frames on
        // which it landed owe a paint, and every other frame of a focused text
        // field owes nothing. `Shell::lens_needs_repaint` is lens's own
        // transition query and is what carries it — folding a caret into
        // `anim_pending` instead would make every focused field render at the
        // panel rate, the defect that held a compositor at ~50% GPU on an idle
        // window.
        let scene_changed = self.shell.take_scene_changed();
        let chrome_repaint =
            scene_changed || self.shell.anim_pending() || self.shell.caret_flip_pending();
        let chrome_anim_region = if chrome_repaint {
            self.shell
                .chrome_damage_regions(self.input_acc.display_size, scene_changed)
        } else {
            None
        };
        let base_full = base_full || (chrome_repaint && chrome_anim_region.is_none());

        // Pointer-only input repaints the cursor sprite's swept area plus
        // the chrome region that reacts to the pointer (hover highlights,
        // the software cursor, magnify tooltips).
        //
        // Non-pointer input (keys, buttons, scroll, touch) never writes
        // desktop pixels itself. Its visual consequences are all carried by
        // signals that already exist on this exact frame: a client buffer
        // commit is `ClientDamage`; a focus/stacking/state change moves the
        // window signature into `base_full`; a chrome control activation
        // moves `chrome_dirty`/`chrome_mode`; an animation moves
        // `chrome_anim_region`. Collapsing every one of those into `Full`
        // here (the old behaviour) made a scroll inside one window — whose
        // actual footprint is a handful of damage rects on that window —
        // repaint the entire output at 120 Hz on high-resolution panels.
        // The input event itself therefore contributes no output damage.
        let input_damage: Option<Vec<tessera_primitives::Rect>> = if had_input && !base_full {
            if input_pointer_only {
                let display = self.input_acc.display_size;
                // The union of the previous and current cursor footprints —
                // a moving sprite leaves stale pixels at its old position
                // that must repaint even though nothing draws there anymore.
                // Only a software cursor paints into the primary framebuffer;
                // a hardware cursor plane commits out-of-band without dirtying
                // composite output pixels.
                let cursor_rect = if software_cursor {
                    let extent = cursor_extent.max(1.0);
                    let swept = |position: (f32, f32)| {
                        let x0 = (position.0 - extent).max(0.0).floor() as i32;
                        let y0 = (position.1 - extent).max(0.0).floor() as i32;
                        let x1 = (position.0 + extent).min(display.0).ceil() as i32;
                        let y1 = (position.1 + extent).min(display.1).ceil() as i32;
                        tessera_primitives::Rect::new(x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
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
                    Some(rect)
                } else {
                    None
                };
                // Chrome hover state can change anywhere along the pointer's
                // path; union the animated-chrome footprint when one is
                // known, and the pointer-capture band when it is not (the
                // shell reports no footprint only when it cannot localize,
                // which then falls back to full below).
                match (chrome_anim_region.as_ref(), cursor_rect) {
                    // Chrome animation footprints stay disjoint from each
                    // other and from the pointer sweep: a region that does not
                    // intersect the pointer simply travels alongside it, and
                    // the compositor's KMS hint takes the list as-is.
                    (Some(regions), Some(rect)) => {
                        let mut regions = regions.clone();
                        match regions
                            .iter_mut()
                            .find(|region| region.intersect(rect).is_some())
                        {
                            Some(region) => *region = region.union(rect),
                            None => regions.push(rect),
                        }
                        Some(regions)
                    }
                    (Some(regions), None) => Some(regions.clone()),
                    (None, cursor_rect) => {
                        // Chrome under the pointer reacts somewhere; ask it
                        // where. A component that cannot bound that reaction
                        // still answers `None`, which keeps the conservative
                        // full-output verdict.
                        match self.shell.pointer_damage_regions(
                            cursor_position.0,
                            cursor_position.1,
                            display,
                        ) {
                            Some(regions) => {
                                let mut regions = regions;
                                if let Some(rect) = cursor_rect {
                                    regions.push(rect);
                                }
                                Some(regions)
                            }
                            None => None,
                        }
                    }
                }
            } else if !self.shell.captures_keyboard() {
                // See the comment above: non-pointer input aimed at the
                // desktop/client scene has no compositor pixel footprint of
                // its own and its client-visible consequences arrive as a
                // buffer commit (`ClientDamage`) or a structural signal in
                // `base_full`. A scroll inside one window must not repaint
                // the whole output at the panel refresh rate.
                Some(Vec::new())
            } else {
                // Keyboard is captured by compositor chrome (launcher,
                // control center, command panel). The chrome drew into
                // compositor-owned pixels and its own change report is the
                // only reliable signal, but a captured key has unbounded
                // chrome reach (cursor keys, typed text that reshapes the
                // panel) — keep the conservative full repaint.
                None
            }
        } else {
            None
        };
        let output_full = base_full || input_damage.is_none() && had_input;

        self.last_windows_hash = Some(windows_hash);
        self.last_all_windows_hash = Some(all_windows_hash);
        self.last_ws_sig = Some(ws_sig);
        self.last_outputs_revision = Some(outputs_revision);
        self.last_interaction_domain_revision = Some(interaction_domain_revision);
        self.damage.last_session_locked = session_locked;
        self.damage.last_notif_revision = Some(notif_revision);
        self.damage.last_chrome_mode = Some(chrome_mode);
        self.damage.chrome_dirty = false;

        // Always collect client damage so surface baseline generations advance
        // in lockstep with client commits across all frames.
        let (client, sampled_surface_ids) = self.client_damage();
        let output = if output_full {
            FrameDamage::Full
        } else {
            // Pointer-only input, localized chrome animations, and chrome dirty
            // footprints join the client damage as additional physical rectangles.
            let mut extra: Vec<tessera_primitives::Rect> = Vec::new();
            if let Some(regions) = chrome_anim_region {
                // Already disjoint per component; do not union here.
                extra.extend(regions);
            }
            if let Some(rects) = input_damage {
                extra.extend(rects);
            }
            if !transition_rects.is_empty() {
                extra.extend(transition_rects);
            }
            extra.extend(chrome_extra_rects);
            let base = client.into_frame_damage(scale, physical_size);
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
            sampled_surface_ids,
        }
    }
}
