use super::*;

// The damage *value layer* lives in `tessera-presentation` (api tier): frame
// damage verdicts, the assessment split, the logical→physical mapping, and the
// `ClientDamageTracker` generation diff behind the narrow `SurfaceDamageFrame`
// seam. Per-image epoch history belongs to Flux, not to this layer. This module
// keeps only the composition-root orchestration: sampling the change signals
// spread across the server, shell, notifications, and wallpaper, and deciding
// what the current frame's delta is.
pub(super) use tessera_composite::presentation::{
    AssessedFrameDamage, ClientDamage, ClientDamageTracker, DamageAssessment, FrameDamage,
    SurfaceDamageFrame, logical_rects_to_frame, union_frame_damage,
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

/// How far a window's painted shadow reaches beyond its geometry, in logical
/// pixels. The damage path must repaint the whole visual footprint, not just
/// the window rect: moving a floating window otherwise leaves its old shadow
/// stroke (or blurred skirt) behind as a detached edge.
fn window_shadow_damage_margin(style: tessera_desktop::window::WindowShadowStyle) -> i32 {
    use tessera_desktop::window::WindowShadowStyle;
    match style {
        WindowShadowStyle::None => 0,
        WindowShadowStyle::Resize => 4,
        WindowShadowStyle::Soft => {
            // Must match `WindowShadowRenderer`: sigma * 3 + vertical offset
            // + 2 px of safety, rounded up. Over-repainting an unused side is
            // cheap; stranding a single shadow row is a visible tear.
            (super::rendering::SHADOW_BLUR_SIGMA * 3.0
                + super::rendering::SHADOW_OFFSET_Y.abs()
                + 2.0)
                .ceil() as i32
        }
    }
}

fn inflate_damage_rect(rect: tessera_primitives::Rect, margin: i32) -> tessera_primitives::Rect {
    if margin <= 0 {
        return rect;
    }
    tessera_primitives::Rect::new(
        rect.origin.x - margin,
        rect.origin.y - margin,
        rect.size.w + margin * 2,
        rect.size.h + margin * 2,
    )
}

fn transition_damage_with_shadow(
    rects: Vec<tessera_primitives::Rect>,
    style: tessera_desktop::window::WindowShadowStyle,
) -> Vec<tessera_primitives::Rect> {
    let margin = window_shadow_damage_margin(style);
    rects
        .into_iter()
        .map(|rect| inflate_damage_rect(rect, margin))
        .collect()
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
        // overview, window switcher, keyboard-grabbing chrome (launcher),
        // screenshot selector, modal/exclusive overlays, and Control Center.
        let chrome_mode = (
            self.shell.overview_active(),
            self.shell.window_switcher_active(),
            self.shell.captures_keyboard(),
            self.shell.screenshot_active(),
            self.shell.modal_or_exclusive_active(),
            self.shell.control_center_active(),
        );
        let windows_hash = self.server.windows_signature();
        let all_windows_hash = self.server.all_windows_signature();
        let ws_sig = self.server.workspace_signature();
        let outputs_revision = self.server.outputs_revision();
        let interaction_domain_revision = self.server.interaction_domain_revision();
        // Every "provably changed everything" signal is named and observed
        // individually. A single anonymous boolean left the loop with no way to
        // tell a forced full repaint from an over-conservative trigger, and the
        // latter is a standing performance defect: a 5 MP output repainting at
        // the panel rate because one term never turns off.
        let first_frame = self.frame_count == 0;
        let forced = self.damage.force_full_redraw;
        let freeze_capture = self.screenshot_freeze.needs_capture();
        let lock_changed = session_locked != self.damage.last_session_locked;
        let software_cursor_changed = software_cursor
            && self.damage.last_presented_cursor != Some((cursor_shape, cursor_hidden));
        // A workspace slide translates entire page layers across the output, so
        // no localized rect describes it: every sampled frame is a full-output
        // repaint by construction. Per-window transitions are different — each
        // record reports the union of the rect it occupied and the rect it now
        // occupies through `transition_damage_rects`, inflated by the shadow
        // reach below. Treating every animation as full made each window move
        // or open repaint a 5 MP output at the panel rate and cut a 120 Hz
        // session to 60 Hz.
        let workspace_slide = self.server.workspace_slide_pending();
        // Per-window transitions are localized; only the workspace slide above
        // remains a full-output cause. The transition scan is intentionally
        // evaluated only when trace logging is enabled: it is a diagnostic, not
        // a per-frame decision input.
        // A live 3D model changes on its own animation clock and paints outside
        // the canvas damage vocabulary (the model pass owns every pixel it
        // covers). Media wallpaper contributes damage only when its absolute
        // source deadline elapsed or a decoded video frame is already waiting;
        // the mere existence of an animated source must not turn every
        // unrelated client event into a full-output composite. A fullscreen
        // window covers the output completely, so the wallpaper cannot be
        // visible: advancing it would repaint pixels that never reach the
        // display. The source still advances its wall clock, so unfullscreening
        // resumes in step.
        let wallpaper_full = !session_locked
            && !self.space_use.client_owns_output()
            && self.wallpaper.as_ref().is_some_and(|w| {
                w.has_model()
                    || w.next_frame_in()
                        .is_some_and(|remaining| remaining.is_zero())
            });
        // Server-side topology: workspace, output, and Interaction Domain model
        // changes feed both the scene and the chrome in full.
        let workspace_changed = self.last_ws_sig != Some(ws_sig);
        let outputs_changed = self.last_outputs_revision != Some(outputs_revision);
        let interaction_domains_changed =
            self.last_interaction_domain_revision != Some(interaction_domain_revision);
        let chrome_mode_changed = self.damage.last_chrome_mode != Some(chrome_mode);
        // Shell mutations applied outside the signed paths (status poller,
        // config reload, app rescan, IPC settings/Interaction Domain control).
        let chrome_dirty = self.damage.chrome_dirty;
        let base_full = first_frame
            || forced
            || freeze_capture
            || lock_changed
            || software_cursor_changed
            || wallpaper_full
            || workspace_slide
            || workspace_changed
            || outputs_changed
            || interaction_domains_changed
            || chrome_mode_changed
            || chrome_dirty;
        log::trace!(
            "damage: base_full={base_full} first={first_frame} forced={forced} \
             freeze={freeze_capture} lock={lock_changed} \
             cursor={software_cursor_changed} wallpaper={wallpaper_full} \
             workspace={workspace_changed} outputs={outputs_changed} \
             domains={interaction_domains_changed} chrome_mode={chrome_mode_changed} \
             chrome_dirty={chrome_dirty} workspace_slide={workspace_slide} \
             live_transition={}",
            self.server.live_transitions_pending()
        );

        let shadow_style = self
            .config
            .as_ref()
            .map(|config| config.ui.window_shadow)
            .unwrap_or_default();
        let transition_rects =
            transition_damage_with_shadow(self.server.transition_damage_rects(), shadow_style);

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
        // A frame that never reached Flux's submit point (acquire timeout,
        // surface loss before recording) has no epoch entry. Carry its damage
        // forward instead of collapsing to Full: the next successful submit is
        // the transaction that commits it.
        let output = match self.damage.pending_damage.take() {
            Some(pending) => union_frame_damage(pending, output),
            None => output,
        };
        match &output {
            FrameDamage::Full => log::trace!("damage: output=Full"),
            FrameDamage::None => log::trace!("damage: output=None"),
            FrameDamage::Area(rects) => log::trace!(
                "damage: output=Area({} rects, area={})",
                rects.len(),
                rects
                    .iter()
                    .map(|r| (r.size.w.max(0) as i64) * (r.size.h.max(0) as i64))
                    .sum::<i64>()
            ),
        }
        AssessedFrameDamage {
            output,
            sampled_surface_ids,
        }
    }
}
