use super::*;
use tessera_wayland::PresentedSurfaces;

// The render transaction outcome is a scheduler-domain value owned by
// `tessera-composite`; the transaction itself stays here because it is
// the composition root's synchronous orchestration of renderer, server,
// host, and shell.
pub(super) use tessera_composite::presentation::{PresentationOutcome, RetryWait};

mod capture;
use capture::FrameCapture;

impl CompositorRuntime<'_> {
    /// Invalidate gfx's per-image damage history across the union surface and
    /// every per-output target, and mark every target as owing a presentation.
    pub(super) fn invalidate_all_surface_damage(&mut self) {
        self.surface.invalidate_damage();
        for target in self.output_targets.values_mut() {
            target.surface.invalidate_damage();
            target.needs_present = true;
            target.last_source_export = None;
            target.last_mirror_cursor_rect = None;
            target.import_cache.retire_all();
        }
    }

    /// Land a pointer-motion cursor update on the hardware cursor plane
    /// without rendering a frame.
    ///
    /// This is the out-of-band half of the pointer fast path
    /// (ADR-0101): the backend's cursor-only atomic commit is independent
    /// of an in-flight primary-plane flip, so it is issued directly from
    /// the input path while a composite frame is still owned by KMS. The
    /// render transaction (`render_and_present`) performs the same commit
    /// when it reaches presentation with no other damage, which keeps the
    /// two halves consistent without a shared baseline protocol.
    ///
    /// Returns `Ok(false)` when the cursor plane was not updated (software
    /// cursor, session locked, sprite unavailable, or the plane hidden) —
    /// callers must not treat that as an error, the composite path repaints
    /// the software cursor as before.
    pub(super) fn present_pointer_cursor(
        &mut self,
        session_locked: bool,
        cursor_hidden: bool,
        cursor_shape: u32,
    ) -> bool {
        if session_locked || !self.host.supports_hardware_cursor() || !self.host.is_active() {
            return false;
        }
        // Re-arm a rejected cursor plane opportunistically; when the backoff
        // has elapsed, this out-of-band commit is the probe. A renewed
        // rejection disables the plane inside the backend and the render
        // path repaints the software cursor.
        self.host.poll_hardware_cursor_retry();
        let outputs = self.server.output_infos();
        let scale = outputs
            .first()
            .map(|o| o.geometry.scale.as_f32())
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| self.host.scale());
        let cursor_position = (
            (self.input_acc.cursor.0 * scale).round() as i32,
            (self.input_acc.cursor.1 * scale).round() as i32,
        );
        if cursor_hidden {
            let allow_client_hw_cursor = !self.streams.any_output_live()
                && self.pending_capture.is_none()
                && self.pending_interaction_domain_capture.is_none()
                && !self.screenshot_freeze.armed
                && !outputs.iter().any(|output| output.role.is_mirror())
                && !self
                    .server
                    .point_over_virtual_output(self.input_acc.cursor.0, self.input_acc.cursor.1);
            let prepared = allow_client_hw_cursor
                .then(|| {
                    let extent = self.host.hardware_cursor_extent()?;
                    let surface = self.server.client_cursor_surface()?;
                    cursor::prepare_client_hardware_cursor(&surface, scale, extent).ok()
                })
                .flatten();
            let Some(prepared) = prepared else {
                // Hiding is itself a cursor-plane commit the render path
                // performs (FB_ID 0); a hidden cursor needs no position update.
                return false;
            };
            let client_key = Some((prepared.id, prepared.generation));
            if self.damage.last_presented_cursor == Some((cursor_shape, cursor_hidden))
                && self.damage.last_presented_client_cursor == client_key
                && self.damage.last_presented_cursor_position == Some(cursor_position)
                && self.damage.last_presented_cursor_hotspot == Some(prepared.hotspot)
                && self.damage.last_presented_cursor_pixels == Some(prepared.size)
            {
                return true;
            }
            if !self.host.set_hardware_cursor(Some(HardwareCursor {
                pixels: &prepared.pixels,
                size: prepared.size,
                hotspot: prepared.hotspot,
                position: cursor_position,
            })) {
                return false;
            }
            return match self.host.present_cursor() {
                Ok(()) => {
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_client_cursor = client_key;
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot = Some(prepared.hotspot);
                    self.damage.last_presented_cursor_pixels = Some(prepared.size);
                    true
                }
                Err(_) => false,
            };
        }
        let Some((pixels, size, hotspot)) = self
            .cursor_cache
            .get(self.device, cursor_shape, scale)
            .map(|cursor| {
                (
                    std::sync::Arc::clone(&cursor.pixels),
                    (cursor.width as u32, cursor.height as u32),
                    (cursor.xhot as u32, cursor.yhot as u32),
                )
            })
        else {
            // Sprite rasterization failed: the render path owns the
            // fallback decision (disable + software cursor).
            return false;
        };
        // Same sprite, hotspot, and position as the last committed cursor:
        // the plane already shows exactly this state.
        if self.damage.last_presented_cursor == Some((cursor_shape, cursor_hidden))
            && self.damage.last_presented_client_cursor.is_none()
            && self.damage.last_presented_cursor_position == Some(cursor_position)
            && self.damage.last_presented_cursor_hotspot == Some(hotspot)
            && self.damage.last_presented_cursor_pixels == Some(size)
        {
            return true;
        }
        if !self.host.set_hardware_cursor(Some(HardwareCursor {
            pixels: &pixels,
            size,
            hotspot,
            position: cursor_position,
        })) {
            // The backend rejected the sprite or disabled the plane; the
            // render path repaints the software cursor.
            return false;
        }
        // Deliberately not vetoed by live output streams (ADR-0130): this
        // commit occupies no primary-plane flip, so it cannot delay a
        // stream-paced composite, and stream captures sample the cursor
        // at their own cadence either way. The physical pointer stops
        // riding the stream's frame rate.
        match self.host.present_cursor() {
            Ok(()) => {
                self.damage.last_present_minute = Some(wall_clock_minute());
                self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                self.damage.last_presented_client_cursor = None;
                self.damage.last_presented_cursor_position = Some(cursor_position);
                self.damage.last_presented_cursor_hotspot = Some(hotspot);
                self.damage.last_presented_cursor_pixels = Some(size);
                true
            }
            Err(_) => {
                // Busy/Inactive/Reconfigured: the plane state is unchanged
                // and the composite path restates cursor properties with its
                // next commit, so the delta is not lost. Fallback decisions
                // stay with the render path.
                false
            }
        }
    }

    pub(super) fn render_and_present(
        &mut self,
        state: FrameState,
    ) -> Result<PresentationOutcome, Box<dyn std::error::Error>> {
        let FrameState {
            input,
            session_locked,
            cursor_hidden,
            cursor_shape,
            had_input,
            input_pointer_only,
            cursor_fast_path: _,
            mut pending_screenshots,
        } = state;
        // Scene colors track the desktop appearance: the shell's design
        // snapshot carries the resolved scheme, refreshed by every
        // preferences reload, so re-derive the opaque clear from it.
        let color_scheme = self.shell.design().scheme;
        self.clear = clear_color(color_scheme);
        // Render scale and logical extent come from the server's output
        // geometry (backend + `[[output]]` overrides), not the host, so a
        // configured scale actually changes the desktop. Nested outputs
        // report the host scale, so the nested path is unchanged.
        let (scale, logical_size) = {
            let geometry = self.server.output_infos().first().map(|o| o.geometry);
            let scale = geometry
                .map(|g| g.scale.as_f32())
                .filter(|s| *s > 0.0)
                .unwrap_or_else(|| self.host.scale());
            let logical = geometry
                .map(|g| g.logical_size())
                .map(|s| (s.w.max(1) as u32, s.h.max(1) as u32))
                .unwrap_or_else(|| self.host.size_u32());
            (scale, logical)
        };
        let physical_size = self.surface.size();
        let cursor_position = (
            (self.input_acc.cursor.0 * scale).round() as i32,
            (self.input_acc.cursor.1 * scale).round() as i32,
        );
        // The largest theme-sprite edge, for pointer-only partial repaints
        // (damage assessment).
        let cursor_extent = self
            .cursor_cache
            .sprite_extent(cursor_shape, scale)
            .unwrap_or(48.0); // Upload and place compositor-owned theme cursors on dedicated KMS
        // planes before deciding whether a full-output client can scan out
        // directly. The cheap Arc clone ends the CursorCache borrow before
        // mutating the host; cursor buffers themselves are cached by exact
        // pixels in the DRM backend.
        // Bind capture requests before the hardware cursor and render/skip
        // decisions: a bound capture always forces a composited frame and
        // requires client cursor surfaces to be composited into the capture
        // rather than offloaded onto the KMS cursor plane.
        let mut frame_capture =
            self.prepare_frame_capture(session_locked, &mut pending_screenshots);
        // A due dmabuf stream (ADR-0130) forces a composite present exactly
        // like a bound readback: its capture-surface slot is filled from the
        // presented frame right after the commit. An active stream paces
        // presentation at its negotiated max-fps, so its consumer observes
        // real frames at that rate even on a static screen.
        let now = std::time::Instant::now();
        let dmabuf_stream_forcing_due =
            !session_locked && self.host.is_active() && self.streams.forcing_due_dmabuf(now);
        let shm_stream_forcing_due =
            !session_locked && self.host.is_active() && self.streams.forcing_due_shm(now);
        let stream_forcing_due = dmabuf_stream_forcing_due || shm_stream_forcing_due;
        let dmabuf_stream_capture_due =
            !session_locked && self.host.is_active() && self.streams.any_dmabuf_due(now);
        let mirror_present = self
            .server
            .output_infos()
            .iter()
            .any(|output| output.role.is_mirror());
        // Upload and place compositor-owned theme cursors — or eligible client
        // custom cursor surfaces (`wl_pointer.set_cursor`) — on dedicated KMS
        // planes before deciding whether a full-output client can scan out
        // directly. The cheap Arc clone ends the CursorCache borrow before
        // mutating the host; cursor buffers themselves are cached by exact
        // pixels in the DRM backend.
        // A previously rejected cursor plane re-arms itself once its failure
        // backoff elapses; the ordinary cursor commit below is the probe.
        let cursor_probe_armed = self.host.poll_hardware_cursor_retry();
        // Sprite identity this frame wants on the hardware plane. Written
        // whenever a cursor commit is actually attempted and read by the
        // present-side baseline updates below; the zero default means the
        // plane was hidden, disabled, or failed to load, in which case no
        // baseline may claim the plane still shows a particular sprite.
        let mut last_committed_hotspot: (u32, u32) = (0, 0);
        let mut last_committed_sprite_size: (u32, u32) = (0, 0);
        let mut client_cursor_on_hw_plane = false;
        let mut client_cursor_key: Option<(usize, u64)> = None;
        if self.host.supports_hardware_cursor() {
            if cursor_hidden {
                let allow_client_hw_cursor = !session_locked
                    && !mirror_present
                    && !self.streams.any_output_live()
                    && frame_capture.is_none()
                    && self.pending_capture.is_none()
                    && self.pending_interaction_domain_capture.is_none()
                    && !self.screenshot_freeze.armed
                    && !stream_forcing_due
                    && !self.server.point_over_virtual_output(
                        self.input_acc.cursor.0,
                        self.input_acc.cursor.1,
                    );
                let prepared_client = if allow_client_hw_cursor {
                    if let (Some(extent), Some(surface)) = (
                        self.host.hardware_cursor_extent(),
                        self.server.client_cursor_surface(),
                    ) {
                        match cursor::prepare_client_hardware_cursor(&surface, scale, extent) {
                            Ok(prepared) => {
                                self.damage.last_client_cursor_reject = None;
                                Some(prepared)
                            }
                            Err(reason) => {
                                let reject_key = (surface.id, surface.generation, reason);
                                if self.damage.last_client_cursor_reject != Some(reject_key) {
                                    log::debug!(
                                        "cursor: client cursor surface {:#x} ({}x{}, scale={}) \
                                         falling back to composited overlay: {reason}",
                                        surface.id,
                                        surface.width,
                                        surface.height,
                                        surface.buffer_scale,
                                    );
                                    self.damage.last_client_cursor_reject = Some(reject_key);
                                }
                                None
                            }
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(prepared) = prepared_client {
                    let key = Some((prepared.id, prepared.generation));
                    let unchanged = self.damage.last_presented_cursor
                        == Some((cursor_shape, cursor_hidden))
                        && self.damage.last_presented_client_cursor == key
                        && self.damage.last_presented_cursor_position == Some(cursor_position)
                        && self.damage.last_presented_cursor_hotspot == Some(prepared.hotspot)
                        && self.damage.last_presented_cursor_pixels == Some(prepared.size);
                    let uploaded = if !unchanged || cursor_probe_armed {
                        self.host.set_hardware_cursor(Some(HardwareCursor {
                            pixels: &prepared.pixels,
                            size: prepared.size,
                            hotspot: prepared.hotspot,
                            position: cursor_position,
                        }))
                    } else {
                        true
                    };
                    if uploaded {
                        if self.damage.last_presented_client_cursor != key {
                            log::debug!(
                                "cursor: client cursor surface {:#x} gen {} offloaded to KMS \
                                 cursor plane ({}x{} hot=({},{}))",
                                prepared.id,
                                prepared.generation,
                                prepared.size.0,
                                prepared.size.1,
                                prepared.hotspot.0,
                                prepared.hotspot.1,
                            );
                        }
                        client_cursor_on_hw_plane = true;
                        client_cursor_key = key;
                        last_committed_hotspot = prepared.hotspot;
                        last_committed_sprite_size = prepared.size;
                    }
                } else {
                    self.host.set_hardware_cursor(None);
                }
            } else {
                let loaded =
                    self.cursor_cache
                        .get(self.device, cursor_shape, scale)
                        .map(|cursor| {
                            (
                                std::sync::Arc::clone(&cursor.pixels),
                                (cursor.width as u32, cursor.height as u32),
                                (cursor.xhot as u32, cursor.yhot as u32),
                            )
                        });
                if let Some((pixels, size, hotspot)) = loaded {
                    // Same sprite, hotspot, and position as the last
                    // committed cursor: skip the backend cache lookup and
                    // the KMS property diff entirely. This is the steady
                    // state of a static pointer while the frame loop still
                    // runs (animations, streams) without any cursor change.
                    // A just-re-armed disabled plane must not skip: its
                    // probe is exactly this commit, and skipping it would
                    // leave the cursor invisible until the next change.
                    let unchanged = self.damage.last_presented_cursor
                        == Some((cursor_shape, cursor_hidden))
                        && self.damage.last_presented_client_cursor.is_none()
                        && self.damage.last_presented_cursor_position == Some(cursor_position)
                        && self.damage.last_presented_cursor_hotspot == Some(hotspot)
                        && self.damage.last_presented_cursor_pixels == Some(size);
                    if !unchanged || cursor_probe_armed {
                        self.host.set_hardware_cursor(Some(HardwareCursor {
                            pixels: &pixels,
                            size,
                            hotspot,
                            position: cursor_position,
                        }));
                    }
                    last_committed_hotspot = hotspot;
                    last_committed_sprite_size = size;
                } else {
                    log::warn!("cursor: theme rasterization failed; disabling hardware cursor");
                    self.host.disable_hardware_cursor();
                }
            }
        }
        let hw_cursor_visible = !cursor_hidden || client_cursor_on_hw_plane;
        let screenshot_include_cursor = self
            .config
            .as_ref()
            .is_none_or(|config| config.screenshot.include_cursor);
        let bound_saved_screenshot = frame_capture
            .as_ref()
            .is_some_and(|capture| matches!(&capture.target, CaptureTarget::Screenshot { .. }));
        let bound_screenshot_cursor = frame_capture
            .as_ref()
            .filter(|capture| matches!(&capture.target, CaptureTarget::Screenshot { .. }))
            .and_then(|capture| capture.cursor);
        // Print-key screenshots keep the freeze armed through confirmation.
        // Portal pick sessions also use the freeze, but retain their own
        // cursor-free capture contract and are not governed by this setting.
        let human_screenshot_session = self.screenshot_freeze.armed && self.pending_pick.is_none();
        let frozen_trigger_cursor = human_screenshot_session
            .then(|| self.screenshot_freeze.trigger_cursor())
            .flatten();
        let failed_human_freeze = human_screenshot_session && self.screenshot_freeze.failed;
        let mut capturing_frozen_screenshot = false;
        // Synchronize server model snapshots (windows, global strip,
        // workspaces, outputs, and Interaction Domains) AND the output scale
        // into the shell and IPC mirror BEFORE `Shell::prepare_frame`,
        // `assess_frame_damage`, and `plan_output_layers` run. Entering
        // fullscreen therefore puts HUD and Dock into dormancy on the very
        // same frame, allowing direct scanout without a one-frame composite
        // lag.
        self.sync_shell_and_ipc_snapshots();
        // Report the output scale so optics-ui rasterises chrome crisply on a
        // HiDPI host; layout and input stay in logical pixels.
        self.shell.set_scale(scale);
        self.enforce_backdrop_capability();
        // Create/destroy mirror targets before the union frame decides which
        // outputs it presents. A target that exists takes its mirror out of
        // the scaled union commit; a failed target re-enters this map only
        // after the topology or mode changes.
        self.sync_output_targets();
        // Resolve chrome's next-frame geometry before damage assessment and
        // plane assignment. An input-only hidden Dock can therefore turn a
        // bottom-edge entry into visible animation even while the previous
        // frames were direct-scanned-out and no canvas pass was running.
        self.shell.prepare_frame(&input);
        // The switcher is the one chrome surface that also needs the server's
        // frozen MRU order and live window list. Resolve it here, before damage
        // assessment, so `anim_pending`, hover, and the panel's damage footprint
        // describe the frame that is about to be painted instead of trailing it
        // by one frame.
        let switcher_state = self.server.window_switcher_snapshot();
        // Only clone the window list when the switcher can actually present:
        // `windows()` walks every live surface and clones each `Window`
        // (title/app_id Strings included) — pure per-frame heap churn in the
        // common no-switcher case. The shell check covers the closing
        // animation, which outlives the server-side session and still needs the
        // live set to keep its fading cards.
        let switcher_windows = if switcher_state.is_some() || self.shell.window_switcher_active() {
            self.server.windows()
        } else {
            Vec::new()
        };
        let (switcher_order, switcher_selected) = switcher_state
            .as_ref()
            .map(|(order, selected)| (order.as_slice(), *selected))
            .unwrap_or((&[], None));
        // Shell chrome lives in the output-local render frame, while the
        // window model keeps desktop-global coordinates. Use the server's
        // output-local view so a configured non-zero origin cannot shift the
        // switcher off the surface it is composited into.
        let switcher_display = self.shell.reserved().inset(self.server.output_local_rect());
        let window_switcher = self.shell.prepare_window_switcher(
            &input,
            switcher_display,
            &switcher_windows,
            switcher_order,
            switcher_selected,
        );
        let assessed_damage = self.assess_frame_damage(
            DamageAssessment {
                had_input,
                input_pointer_only,
                session_locked,
                cursor_hidden,
                cursor_shape,
                software_cursor: self.host.uses_software_cursor(),
                cursor_position: self.input_acc.cursor,
                cursor_extent,
                scale,
                physical_size,
            },
            !client_cursor_on_hw_plane,
        );
        let mut damage = assessed_damage.output;
        let mut primary_damage = assessed_damage.primary_output;
        let overlay_buffer_changed = assessed_damage.overlay_buffer_changed;
        let mut sampled_surface_ids = assessed_damage.sampled_surface_ids;
        let has_mirror_target =
            has_active_mirror_target(&self.server.output_infos(), |connector| {
                self.output_targets.contains_key(connector)
            });
        let cursor_plane_changed = self.host.supports_hardware_cursor()
            && (self.damage.last_presented_cursor != Some((cursor_shape, cursor_hidden))
                || self.damage.last_presented_client_cursor != client_cursor_key
                || (hw_cursor_visible
                    && (self.damage.last_presented_cursor_position != Some(cursor_position)
                        || self.damage.last_presented_cursor_hotspot
                            != Some(last_committed_hotspot)
                        || self.damage.last_presented_cursor_pixels
                            != Some(last_committed_sprite_size))));
        let cursor_only_eligible = matches!(damage, FrameDamage::None)
            && !has_mirror_target
            && cursor_plane_changed
            && frame_capture.is_none()
            && !stream_forcing_due
            && !self.streams.any_output_live()
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
            && !self.server.retired_buffers_pending();
        if cursor_only_eligible {
            match self.host.present_cursor() {
                Ok(()) => {
                    // A cursor-plane commit redraws no client content: it must
                    // pace clients via `wl_callback.done` without claiming any
                    // surface was presented.
                    self.server.send_frame_callbacks(
                        self.server.monotonic_ms(),
                        PresentedSurfaces::Nothing,
                    );
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_client_cursor = client_cursor_key;
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot =
                        hw_cursor_visible.then_some(last_committed_hotspot);
                    self.damage.last_presented_cursor_pixels =
                        hw_cursor_visible.then_some(last_committed_sprite_size);
                    self.renderer.gc(self.server.live_surface_ids());
                    self.renderer.begin_frame();
                    self.frame_count += 1;
                    self.drain_shell_chrome_events();
                    return Ok(PresentationOutcome::Submitted);
                }
                Err(HostError::Drm(DrmError::CursorFallback)) => {
                    // The backend disabled the cursor plane. Repaint this
                    // frame with the software cursor (and composited client
                    // cursor overlay if active) rather than showing one
                    // cursorless refresh.
                    client_cursor_on_hw_plane = false;
                    client_cursor_key = None;
                    self.damage.reset_cursor_baseline();
                    damage = FrameDamage::Full;
                }
                Err(HostError::Drm(
                    DrmError::Busy
                    | DrmError::FlipTimeout
                    | DrmError::Inactive
                    | DrmError::Reconfigured,
                )) => {
                    // Nothing was presented and no damage baseline was
                    // consumed, so the retry stays on the cheap cursor-only
                    // path: the unpresented position keeps
                    // `cursor_plane_changed` true next frame. The backend owns
                    // the flip that must retire before this commit can retry.
                    self.drain_shell_chrome_events();
                    return Ok(PresentationOutcome::Retry {
                        wait: RetryWait::BackendEvent,
                    });
                }
                Err(HostError::Drm(DrmError::ScanoutUnsupported)) => {
                    damage = FrameDamage::Full;
                }
                Err(error) => return Err(error.into()),
            }
        }
        // Cursor-only KMS commits are not enough when a mirror target is
        // active: source outputs own a hardware cursor plane, but a mirror
        // target has no cursor plane and must redraw its cursor. On the
        // per-output target path this routes through a localized mirror-only
        // pass without forcing full desktop redraws; only the legacy union
        // fallback needs full damage (ADR-0250, `[INV-ARCH-160]`).
        let mirror_cursor_dirty = cursor_plane_changed && has_mirror_target;
        if mirror_cursor_dirty && !self.can_render_output_targets() {
            damage = FrameDamage::Full;
            primary_damage = FrameDamage::Full;
        }
        if matches!(damage, FrameDamage::None)
            && !mirror_cursor_dirty
            && frame_capture.is_none()
            && !stream_forcing_due
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
        {
            // Nothing visible changed: skip the render and the atomic commit
            // — the scanout contents are already correct, so presenting
            // would be pure waste. Release any retired client buffers whose
            // prior composited GPU read is already fenced/complete so clients
            // never deadlock on `NoDamage` (ADR-0250, `[INV-ARCH-159]`).
            self.release_idle_retired_buffers();
            let callbacks_sent = self.presentation.frame_callbacks_allowed()
                && self
                    .server
                    .send_frame_callbacks(self.server.monotonic_ms(), PresentedSurfaces::Nothing)
                    > 0;
            self.renderer.gc(self.server.live_surface_ids());
            self.renderer.begin_frame();
            self.drain_shell_chrome_events();
            return Ok(PresentationOutcome::NoDamage { callbacks_sent });
        }
        // Direct-scanout fast path: a single opaque dma-buf client whose actual
        // geometry covers the whole output (and nothing else needing
        // compositing) can
        // be page-flipped directly onto the primary plane, skipping the Vulkan
        // composite. This is the primary-plane zero-GPU-cost path for games,
        // maximized video, and semantic fullscreen alike. A miss or
        // a kernel rejection falls through to the normal composite below.
        // Scanout stays disqualified while any output stream lives
        // (ADR-0130): a page-flipped frame never passes through the
        // compositor, so streams could not capture it.
        //
        // While scanout is active the renderer never composites, so the damage
        // tracker's per-surface generation baselines go stale: a client that
        // committed frames entirely through the scanout path looks "unchanged"
        // to it. Leaving scanout must therefore force a full redraw the next
        // composite, or the fallback frame could skip rendering and show a
        // frozen image.
        let was_scanout = self.primary_plane_state.is_direct();
        let output_layer_plans = self.plan_output_layers(
            physical_size,
            logical_size,
            scale,
            cursor_hidden,
            client_cursor_on_hw_plane,
            frame_capture.is_some() || self.streams.any_output_live(),
        );
        if let Some(overlay) = &output_layer_plans.overlay {
            self.overlay_telemetry
                .record(self.host.name(), &overlay.plan);
        }
        let wanted_overlay =
            if self.overlay_offload_enabled && !mirror_present && self.direct_outputs.is_empty() {
                output_layer_plans.overlay.as_ref().and_then(|audit| {
                    tessera_composite::presentation::OverlayPlacement::from_plan(
                        &audit.plan,
                        audit.output.clone()?,
                        audit.target,
                    )
                })
            } else {
                None
            };
        let overlay_change = self.overlay_state.reconcile(wanted_overlay);
        if overlay_change.requires_primary_repaint() {
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            damage = FrameDamage::Full;
            primary_damage = FrameDamage::Full;
            if matches!(
                overlay_change,
                tessera_composite::presentation::OverlayChange::Deactivated
            ) {
                log::info!(
                    "{}: overlay offload inactive; compositing",
                    self.host.name()
                );
            } else if let Some(overlay) = self.overlay_state.active() {
                log::info!(
                    "{}: overlay offload active for window {:?} on {} at {:?}",
                    self.host.name(),
                    overlay.window,
                    overlay.output,
                    overlay.target
                );
            }
        }
        if output_layer_plans.direct_targets.len() > 1 {
            let mut direct_fences = Vec::with_capacity(output_layer_plans.direct_targets.len());
            let mut all_ok = true;
            for (target, plan) in &output_layer_plans.direct_targets {
                let Some(candidate) = plan.direct_candidate() else {
                    all_ok = false;
                    break;
                };
                match self
                    .host
                    .present_scanout(candidate, target, damage.area_rects())
                {
                    Ok(fence) => fence.append_to(&mut direct_fences),
                    Err(error) => {
                        log::warn!(
                            "{}: multi-target direct scanout to {target} failed: {error}",
                            self.host.name()
                        );
                        all_ok = false;
                        break;
                    }
                }
            }
            if all_ok {
                if let Some((_, plan)) = output_layer_plans.direct_targets.first()
                    && let Some(candidate) = plan.direct_candidate()
                {
                    self.primary_plane_state.commit_direct(candidate.id);
                }
                let targets = output_layer_plans
                    .direct_targets
                    .iter()
                    .map(|(target, _)| target.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let target_refs = targets.iter().map(String::as_str).collect::<Vec<_>>();
                self.direct_outputs = targets.clone();
                self.overlay_state.deactivate();
                self.streams.accumulate_damage(&damage);
                self.server.acknowledge_presented_surface_damage_on_outputs(
                    sampled_surface_ids.iter(),
                    false,
                    Some(&target_refs),
                );
                let merged = tessera_platform::sync::merge_live_sync_fences(direct_fences);
                self.release_retired_buffers_after_direct_scanout(&target_refs, merged);
                self.server.send_frame_callbacks_on_outputs(
                    self.server.monotonic_ms(),
                    PresentedSurfaces::Sampled {
                        ids: &sampled_surface_ids,
                        drew_preview_sources: false,
                    },
                    Some(&target_refs),
                );
                self.damage.last_present_minute = Some(wall_clock_minute());
                self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                self.damage.last_presented_client_cursor = client_cursor_key;
                self.damage.last_presented_cursor_position = Some(cursor_position);
                self.damage.last_presented_cursor_hotspot =
                    hw_cursor_visible.then_some(last_committed_hotspot);
                self.damage.last_presented_cursor_pixels =
                    hw_cursor_visible.then_some(last_committed_sprite_size);
                self.damage.force_full_redraw = false;
                self.renderer.gc(self.server.live_surface_ids());
                self.renderer.begin_frame();
                self.frame_count += 1;
                self.scanout_telemetry.record_success(self.host.name());
                self.drain_shell_chrome_events();
                return Ok(PresentationOutcome::Submitted);
            }
            // Some targets may already be direct. Composite all on the
            // fallback; that reclaims every primary plane coherently.
            self.direct_outputs.clear();
        }

        let primary_plane_plan = &output_layer_plans.primary;
        if let Some(candidate) = primary_plane_plan.direct_candidate() {
            let scanout_target = output_layer_plans
                .scanout_target
                .clone()
                .unwrap_or_default();
            match self
                .host
                .present_scanout(candidate, &scanout_target, damage.area_rects())
            {
                Ok(completion_fence) => {
                    let entered_scanout = self.primary_plane_state.commit_direct(candidate.id);
                    self.scanout_telemetry.record_success(self.host.name());
                    if entered_scanout {
                        log::info!(
                            "{}: direct scanout active for {:#x} (mod {:#x}); composite bypassed",
                            self.host.name(),
                            candidate.drm_format,
                            candidate.modifier,
                        );
                    }
                    // The buffer was scanned out directly: mark the surface
                    // damage acknowledged so the server's damage baseline
                    // stays correct, fire client frame callbacks to keep
                    // pacing, and re-anchor the cursor/minute baselines the
                    // skip-render path consults. Captures and screenshots are
                    // disqualified by the primary-plane plan, so none run here.
                    // Live output streams disqualify scanout outright
                    // (ADR-0130), so this path never runs while one exists;
                    // a stopped stream's leftover damage still accumulates
                    // for the next captured frame (ADR-0127).
                    self.direct_outputs.insert(scanout_target.clone());
                    self.overlay_state.deactivate();
                    self.streams.accumulate_damage(&damage);
                    // Direct scanout samples exactly one buffer — the
                    // candidate that is now on the primary plane — so the
                    // damage pass's sampled set is authoritative here.
                    let direct_outputs = [scanout_target.as_str()];
                    self.server.acknowledge_presented_surface_damage_on_outputs(
                        sampled_surface_ids.iter(),
                        false,
                        Some(&direct_outputs),
                    );
                    self.release_retired_buffers_after_direct_scanout(
                        &direct_outputs,
                        completion_fence,
                    );
                    self.server.send_frame_callbacks_on_outputs(
                        self.server.monotonic_ms(),
                        PresentedSurfaces::Sampled {
                            ids: &sampled_surface_ids,
                            drew_preview_sources: false,
                        },
                        Some(&direct_outputs),
                    );
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_client_cursor = client_cursor_key;
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot =
                        hw_cursor_visible.then_some(last_committed_hotspot);
                    self.damage.last_presented_cursor_pixels =
                        hw_cursor_visible.then_some(last_committed_sprite_size);
                    self.damage.force_full_redraw = false;
                    self.renderer.gc(self.server.live_surface_ids());
                    self.renderer.begin_frame();
                    self.frame_count += 1;
                    self.drain_shell_chrome_events();
                    return Ok(PresentationOutcome::Submitted);
                }
                Err(error) => {
                    // Any rejection (unsupported format, EACCES, reconfigure,
                    // flip timeout) falls through to compositing. Do not change
                    // primary-plane state yet: the previous framebuffer still
                    // owns the plane until the fallback commit succeeds.
                    if matches!(error, HostError::Drm(DrmError::ScanoutUnsupported)) {
                        self.scanout_telemetry.record_rejection(
                            self.host.name(),
                            tessera_composite::presentation::ScanoutRejection::new(
                                ScanoutRejectReason::KmsRejected,
                                true,
                            ),
                        );
                    }
                    if matches!(error, HostError::Drm(DrmError::CursorFallback)) {
                        // The backend disabled the KMS cursor after rejecting
                        // this commit. The same frame must include the
                        // software cursor (or composited client cursor overlay),
                        // outside any client-only damage scissor. The plane is
                        // disabled until its retry backoff elapses; dropping
                        // the sprite baseline guarantees the re-arm probe
                        // commit is issued.
                        client_cursor_on_hw_plane = false;
                        client_cursor_key = None;
                        damage = FrameDamage::Full;
                        primary_damage = FrameDamage::Full;
                        self.damage.reset_cursor_baseline();
                    }
                    if matches!(
                        error,
                        HostError::Drm(
                            DrmError::Busy
                                | DrmError::FlipTimeout
                                | DrmError::Inactive
                                | DrmError::Reconfigured
                        )
                    ) {
                        if matches!(
                            &error,
                            HostError::Drm(DrmError::Busy | DrmError::FlipTimeout)
                        ) {
                            // No gfx frame was recorded on the direct-scanout
                            // path. Carry the assessed damage to the retry
                            // instead of forcing a full composite.
                            self.damage.pending_damage =
                                Some(match self.damage.pending_damage.take() {
                                    Some(pending) => union_frame_damage(pending, damage.clone()),
                                    None => damage.clone(),
                                });
                        } else {
                            self.damage.force_full_redraw = true;
                        }
                        self.drain_shell_chrome_events();
                        return Ok(PresentationOutcome::Retry {
                            wait: RetryWait::BackendEvent,
                        });
                    }
                    if !matches!(error, HostError::Drm(DrmError::ScanoutUnsupported)) {
                        log::warn!(
                            "{}: direct scanout failed; compositing instead: {error}",
                            self.host.name()
                        );
                    }
                }
            }
        } else if let Some(rejection) = primary_plane_plan.rejection() {
            self.scanout_telemetry
                .record_rejection(self.host.name(), *rejection);
        }
        // Scanout was active last frame but is no longer eligible (a window
        // appeared, the cursor moved, chrome opened…): force a full composite so
        // the resumed render is correct rather than skipped as "unchanged".
        if was_scanout {
            // Direct scanout bypassed gfx's epoch ledger entirely. The first
            // resumed composite has no trustworthy image history.
            self.damage.force_full_redraw = true;
            self.invalidate_all_surface_damage();
            damage = FrameDamage::Full;
            primary_damage = FrameDamage::Full;
        }
        // Zero-GPU overlay-only fast path (ADR-0268): when an active overlay
        // window only swaps its buffer in place and Primary has zero damage,
        // update only the KMS overlay plane without acquiring a Vulkan frame or
        // rewriting Primary.
        let single_physical_output = self
            .server
            .output_infos()
            .iter()
            .filter(|o| o.kind.is_physical())
            .count()
            <= 1;
        let overlay_only_eligible = matches!(
            overlay_change,
            tessera_composite::presentation::OverlayChange::Updated
        ) && matches!(primary_damage, FrameDamage::None)
            && overlay_buffer_changed
            && !was_scanout
            && !mirror_present
            && !mirror_cursor_dirty
            && (!cursor_plane_changed || single_physical_output)
            && frame_capture.is_none()
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && self.pending_window_capture.is_none()
            && self.pending_pick.is_none()
            && !session_locked
            && !self.screenshot_freeze.armed
            && !stream_forcing_due
            && !self.streams.any_output_live()
            && !self.server.lock_confirmation_pending();
        if overlay_only_eligible
            && let Some((active, overlay_surface)) = self.active_overlay_dmabuf()
        {
            match self
                .host
                .present_overlay_only(tessera_platform::host::OverlayLayer {
                    surface: &overlay_surface,
                    output: &active.output,
                    target: active.target,
                    zpos: active.zpos,
                }) {
                Ok(completion_fence) => {
                    let overlay_outputs = [active.output.as_str()];
                    let overlay_ids = [active.surface_id];
                    self.streams.accumulate_damage(&damage);
                    self.server.acknowledge_presented_surface_damage_on_outputs(
                        overlay_ids.iter(),
                        false,
                        Some(&overlay_outputs),
                    );
                    self.release_retired_buffers_after_direct_scanout(
                        &overlay_outputs,
                        completion_fence,
                    );
                    self.server.send_frame_callbacks_on_outputs(
                        self.server.monotonic_ms(),
                        PresentedSurfaces::Sampled {
                            ids: &overlay_ids,
                            drew_preview_sources: false,
                        },
                        Some(&overlay_outputs),
                    );
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_client_cursor = client_cursor_key;
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot =
                        hw_cursor_visible.then_some(last_committed_hotspot);
                    self.damage.last_presented_cursor_pixels =
                        hw_cursor_visible.then_some(last_committed_sprite_size);
                    self.damage.force_full_redraw = false;
                    self.renderer.gc(self.server.live_surface_ids());
                    self.renderer.begin_frame();
                    self.frame_count += 1;
                    self.drain_shell_chrome_events();
                    return Ok(PresentationOutcome::Submitted);
                }
                Err(HostError::Drm(DrmError::Busy | DrmError::FlipTimeout)) => {
                    self.damage.pending_damage = Some(match self.damage.pending_damage.take() {
                        Some(pending) => union_frame_damage(pending, damage),
                        None => damage,
                    });
                    self.drain_shell_chrome_events();
                    return Ok(PresentationOutcome::Retry {
                        wait: RetryWait::BackendEvent,
                    });
                }
                Err(HostError::Drm(DrmError::CursorFallback)) => {
                    client_cursor_on_hw_plane = false;
                    client_cursor_key = None;
                    damage = FrameDamage::Full;
                    primary_damage = FrameDamage::Full;
                    self.damage.reset_cursor_baseline();
                }
                Err(error) => {
                    log::warn!(
                        "{}: overlay-only commit rejected ({error}); falling back to composition",
                        self.host.name()
                    );
                    self.overlay_state.deactivate();
                    self.damage.force_full_redraw = true;
                    self.invalidate_all_surface_damage();
                    damage = FrameDamage::Full;
                    primary_damage = FrameDamage::Full;
                }
            }
        }
        // Normal DRM frames use one native target per physical output. The
        // union path remains for capture/screenshot frames and for complex
        // chrome states that still own union-coupled state.
        let target_live_previews = if window_switcher.is_none() {
            self.shell.live_preview_presentations()
        } else {
            Vec::new()
        };
        if frame_capture.is_none()
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && self.pending_window_capture.is_none()
            && self.pending_pick.is_none()
            && !session_locked
            && !self.screenshot_freeze.armed
            && !stream_forcing_due
            && !self.streams.any_output_live()
            && self.can_render_output_targets()
        {
            let outcome = self.render_output_targets(
                &damage,
                &primary_damage,
                overlay_buffer_changed,
                &sampled_surface_ids,
                cursor_shape,
                cursor_hidden,
                client_cursor_on_hw_plane,
                client_cursor_key,
                last_committed_hotspot,
                last_committed_sprite_size,
                mirror_cursor_dirty,
                window_switcher.as_ref(),
                &target_live_previews,
            );
            self.drain_shell_chrome_events();
            return outcome;
        }

        // Opportunistic stream capture: this frame composites for real
        // damage, so every due SHM stream may piggyback on it with the
        // shared readback — between the forced frames its cadence already
        // guarantees (ADR-0130). One-shot captures keep priority; a locked
        // or inactive session simply produces no stream frames (the streams
        // survive). When any due SHM stream negotiated the embedded cursor
        // mode, the readback carries a cursor snapshot along so the worker
        // can produce a composited twin next to the pristine frame
        // (ADR-0127).
        if frame_capture.is_none()
            && self.pending_capture.is_none()
            && !self.stream_job_in_flight
            && !session_locked
            && self.host.is_active()
            && !self.capture_worker.is_busy()
            && self.streams.any_shm_due(std::time::Instant::now())
        {
            frame_capture = Some(FrameCapture {
                crop: None,
                target: CaptureTarget::Stream,
                cursor: self.stream_shm_cursor_state(),
            });
        }
        // Bounded frame acquire (Optics ADR-0124 integration).
        //
        // On the direct DRM path the acquire waits for the slot's fence from
        // three frames ago; no presentation engine owns those images, so a
        // timeout here means the GPU is genuinely behind and dropping the
        // already-assessed frame would forfeit its damage baselines. Wait long
        // enough for the queue to retire (250 ms) instead of the 2 ms that
        // turned any burst of GPU work into a retry loop, a full-output repaint
        // and a visible frame-time jump.
        //
        // A host swapchain is different: `vkAcquireNextImageKHR` can block
        // because the outer compositor holds every image while occluded or
        // suspended. Keep that bound short (one frame at 60 Hz) so an invisible
        // nested window cannot park the loop, and let the scheduler retry at
        // the next presentation boundary.
        let acquire_timeout_ns = if self.host.presents_through_host_swapchain() {
            16_000_000
        } else {
            250_000_000
        };
        // A frosted or glass body shows a blur of what surrounds it, so damage
        // inside its sampling reach repaints the whole body (ADR-0253).
        if let Some(plan) = backdrop::plan(
            self.shell.design().material,
            self.shell.backdrop_bodies(),
            scale,
            physical_size,
        ) {
            damage = plan.widen(damage);
        }
        let current_damage = damage.to_gfx_damage();
        match self
            .surface
            .begin_frame_timeout_with_damage(acquire_timeout_ns, &current_damage)
        {
            Ok(mut frame) => {
                let frame_slot = frame.index() as usize;
                // Damage belongs to the swapchain image the presentation
                // engine handed us, not to the GPU frame slot. gfx owns that
                // ledger and answers with the exact scope this image must
                // repaint; the compositor no longer maintains a parallel
                // per-image history.
                let image_index = frame.image_index() as usize;
                let repaint = FrameDamage::from_gfx_damage(frame.repaint_scope()?);
                // Exact physical render area of the currently open output
                // pass. Full-output and offscreen-target passes leave it as
                // `None`. The merged desktop-and-chrome pass uses this scope
                // to decide whether visible optics-ui chrome intersects the
                // repaint.
                #[allow(unused_assignments)]
                let mut output_damage_scope: Vec<optics_gfx::CanvasRenderArea> = Vec::new();
                self.renderer.begin_frame();
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let live_previews = if window_switcher.is_none() {
                    self.shell.live_preview_presentations()
                } else {
                    Vec::new()
                };
                // When an unoccluded fullscreen surface owns the output with
                // no active transition, workspace slide, or overview, the
                // wallpaper (both 2D background and 3D model) cannot reach any
                // output pixel and is elided.
                let wallpaper_occluded = {
                    let overview_progress = self.shell.overview_progress();
                    let overview_live = self.shell.overview_active() || overview_progress > 0.001;
                    !overview_live
                        && !session_locked
                        && !self.screenshot_freeze.active()
                        && !self.server.workspace_slide_pending()
                        && !self.server.transitions_pending()
                        && self.space_use.client_owns_output()
                };
                let model_active = !wallpaper_occluded
                    && self
                        .wallpaper
                        .as_ref()
                        .is_some_and(wallpaper::Wallpaper::has_model);
                // Overview mode (M9) swaps the whole client scene for the
                // thumbnail grid and skips the backdrop graph capture path.
                // The grid stays up while the reveal animation runs in
                // either direction: keying the swap on `open` alone would
                // pop the desktop back the instant the close fade starts,
                // stranding the chrome's cell frames mid-flight.
                let overview_progress = self.shell.overview_progress();
                let overview_active = self.shell.overview_active() || overview_progress > 0.001;
                // A screenshot freeze session replaces the whole frame with
                // the trigger-frame snapshot: the capture frame renders the
                // desktop scene *and* the chrome into an offscreen target
                // below; later frames blit that snapshot and draw only the
                // selector on top. The backdrop graph stays off for the
                // whole session.
                let freeze_capturing = self.screenshot_freeze.needs_capture();
                // Compositor blurred shadows (ADR-0139): render masks and
                // record the shadow passes at this pass boundary, before any
                // output pass opens. The borrowed filter outputs are placed
                // by `tessera-composite` inside the client-scene pass below.
                // Exclude occluded windows so floating windows covered by a
                // fullscreen or maximized surface never record offscreen masks
                // or Gaussian blur compute dispatches. Overview replaces the
                // client scene with a thumbnail grid that draws every window
                // (occluded ones included), and it does not consume this layer
                // at all, so the occlusion filter applies only outside it.
                let shadow_style = self
                    .config
                    .as_ref()
                    .map(|c| c.ui.window_shadow)
                    .unwrap_or_default();
                let rendered_shadows =
                    if shadow_style == tessera_desktop::window::WindowShadowStyle::Soft {
                        let windows = if overview_active {
                            self.server.render_windows()
                        } else {
                            self.server.unoccluded_render_windows()
                        };
                        self.window_shadows.prepare(
                            self.device,
                            self.canvas,
                            &mut frame,
                            &windows,
                            scale,
                            shadow_style,
                            0,
                        )
                    } else {
                        Vec::new()
                    };
                // Borrow the raw effect outputs as draw-only entries for the
                // renderer. The images stay valid for this frame: the filter
                // slot is not applied again until the next rotation, after
                // this frame submits (ADR-0074 lifetime).
                let soft_shadow_entries: Vec<tessera_composite::SoftShadowEntry<'_>> =
                    rendered_shadows
                        .iter()
                        .map(|shadow| tessera_composite::SoftShadowEntry {
                            window: shadow.window,
                            raw: shadow.raw,
                            _borrow: std::marker::PhantomData,
                            x: shadow.x,
                            y: shadow.y,
                            w: shadow.w,
                            h: shadow.h,
                        })
                        .collect();
                let soft_shadow_layer = (!soft_shadow_entries.is_empty()).then_some(
                    tessera_composite::SoftShadowLayer {
                        entries: soft_shadow_entries.as_slice(),
                    },
                );
                output_damage_scope = frame_damage_scope(&repaint, physical_size);

                // [INV-ARCH-70]: the frame sampled client dma-bufs beyond the
                // visible desktop set whenever it drew overview thumbnails,
                // the window switcher's live cards, the workspace rail's live
                // miniatures, or a workspace-slide source page. Those buffers
                // are owned by this frame too, so the acknowledgement cannot be
                // restricted to the damage pass's own set (see
                // `Server::acknowledge_presented_surface_damage`).
                let drew_preview_sources = overview_active
                    || window_switcher.is_some()
                    || !live_previews.is_empty()
                    || self.server.workspace_slide_pending();

                // Minimize flight targets follow the dock's resting tile
                // layout; pushing them every composite keeps even
                // client-initiated minimizes aimed at the real icon.
                let targets = self.shell.minimize_targets(self.input_acc.display_size);
                self.server.set_minimize_targets(targets);
                // The do-not-disturb indicator is drawn by chrome and is part
                // of the composite; publish it here (after `assess_frame_damage`
                // consumed the change as full damage) so both the fan-out and
                // the repaint observe the same transition.
                let do_not_disturb = self
                    .notif_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .do_not_disturb();
                if self.system_status.do_not_disturb != do_not_disturb {
                    self.system_status.do_not_disturb = do_not_disturb;
                    publish_system_status_parts(
                        &self.system_status,
                        &mut self.shell,
                        &self.live,
                        &self.ipc,
                    );
                }

                let shell_requires_composition = self.shell.requires_composition();

                if freeze_capturing && !self.screenshot_freeze.failed {
                    if !self.screenshot_freeze.ensure_target(
                        self.device,
                        self.surface,
                        frame_slot,
                        physical_size,
                    ) {
                        self.screenshot_freeze.failed = true;
                    } else if let Some(target) = self.screenshot_freeze.target_mut(frame_slot) {
                        // The frozen frame shows chrome in its material too
                        // (ADR-0253). Its own executor: sharing the live
                        // backdrop's capture or filter slots in one frame would
                        // give their images a second writer.
                        let freeze_plan = if shell_requires_composition {
                            backdrop::plan(
                                self.shell.design().material,
                                self.shell.backdrop_bodies(),
                                scale,
                                physical_size,
                            )
                        } else {
                            None
                        };
                        let freeze_slab = match freeze_plan.as_ref() {
                            Some(plan) => {
                                let captured = match self.freeze_backdrop.capture_target(
                                    self.device,
                                    0,
                                    frame_slot,
                                    plan,
                                ) {
                                    Some(capture) => backdrop::capture_desktop(
                                        self.canvas,
                                        self.device,
                                        &mut frame,
                                        capture,
                                        &mut self.wallpaper,
                                        plan,
                                        logical_size,
                                        self.clear,
                                        true,
                                        |canvas| {
                                            draw_direct_desktop_scene(
                                                canvas,
                                                self.device,
                                                self.renderer,
                                                self.server,
                                                render_geometry,
                                                overview_active,
                                                overview_progress,
                                                window_switcher.as_ref(),
                                                color_scheme,
                                                soft_shadow_layer.as_ref(),
                                                shadow_style,
                                                None,
                                            )
                                        },
                                    ),
                                    None => Err(optics_gfx::Error(
                                        optics_gfx::sys::optics_gfx_result::OPTICS_GFX_ERROR_OUT_OF_MEMORY,
                                    )),
                                };
                                match captured {
                                    Ok(()) => {
                                        self.freeze_backdrop.compose(self.device, &frame, 0, plan)
                                    }
                                    Err(error) => {
                                        self.freeze_backdrop.capture_failed(error);
                                        None
                                    }
                                }
                            }
                            None => None,
                        };
                        let capture_ok = (|| -> Result<(), optics_gfx::Error> {
                            let s1 = self.canvas.begin_session(
                                optics_gfx::ImageTarget {
                                    frame: &mut frame,
                                    image: target,
                                },
                                optics_gfx::CanvasPassOptions {
                                    clear: Some(self.clear),
                                    damage: Vec::new(),
                                },
                            )?;
                            draw_wallpaper_background(
                                &s1,
                                self.device,
                                &mut self.wallpaper,
                                logical_size,
                                scale,
                            );
                            s1.end()?;

                            if model_active && let Some(wallpaper) = self.wallpaper.as_mut() {
                                wallpaper.draw_model_to(self.device, &mut frame, target);
                            }

                            let s2 = self.canvas.begin_session(
                                optics_gfx::ImageTarget {
                                    frame: &mut frame,
                                    image: target,
                                },
                                optics_gfx::CanvasPassOptions {
                                    clear: None,
                                    damage: Vec::new(),
                                },
                            )?;
                            draw_direct_desktop_scene(
                                &s2,
                                self.device,
                                self.renderer,
                                self.server,
                                render_geometry,
                                overview_active,
                                overview_progress,
                                window_switcher.as_ref(),
                                color_scheme,
                                soft_shadow_layer.as_ref(),
                                shadow_style,
                                None,
                            )?;
                            if let Some(slab) = freeze_slab.as_ref() {
                                slab.draw(&s2);
                            }
                            let (include_live_cursor, cursor_position) =
                                if !human_screenshot_session {
                                    (false, None)
                                } else {
                                    let client_cursor = screenshot_include_cursor
                                        && frozen_trigger_cursor
                                            .is_some_and(|cursor| cursor.client_surface);
                                    (
                                        client_cursor,
                                        client_cursor
                                            .then(|| {
                                                frozen_trigger_cursor.map(|cursor| cursor.position)
                                            })
                                            .flatten(),
                                    )
                                };
                            if shell_requires_composition {
                                let _uploads = self.device.uploads_begin();
                                let _ = unsafe { self.shell.submit(s2.as_raw() as *mut _) };
                            }
                            // Draw window-switcher and Dock live-preview thumbnails AFTER
                            // `self.shell.submit` so their client-surface miniatures sit on
                            // top of the opaque FloatingPanel background (`#111622`, alpha=242)
                            // inside `card.geometry.preview` (above the disjoint bottom label
                            // strip `card.geometry.label`).
                            if !session_locked {
                                if let Some(presentation) = window_switcher.as_ref() {
                                    draw_window_switcher_cards(
                                        &s2,
                                        self.device,
                                        self.renderer,
                                        self.server,
                                        scale,
                                        presentation,
                                    );
                                } else {
                                    draw_live_preview_scenes(
                                        &s2,
                                        self.device,
                                        self.renderer,
                                        self.server,
                                        scale,
                                        &live_previews,
                                    );
                                }
                                draw_client_overlays(
                                    &s2,
                                    self.device,
                                    self.renderer,
                                    self.server,
                                    scale,
                                    include_live_cursor,
                                    cursor_position,
                                );
                            }
                            if human_screenshot_session
                                && screenshot_include_cursor
                                && let Some(cursor) = frozen_trigger_cursor
                                && !cursor.hidden
                                && !cursor.client_surface
                            {
                                draw_software_cursor(
                                    &s2,
                                    self.device,
                                    &mut self.cursor_cache,
                                    cursor.position,
                                    cursor.shape,
                                    scale,
                                );
                            }
                            s2.end()?;
                            Ok(())
                        })();
                        if capture_ok.is_ok() {
                            self.screenshot_freeze.mark_captured(frame_slot);
                        } else {
                            self.screenshot_freeze.failed = true;
                        }
                    }
                }

                // When a participating full-screen surface (e.g. an open
                // Control Center at full reveal opacity) completely occludes
                // the desktop and no consumer of the desktop scene is live,
                // skip the redundant base desktop pass and render the shell
                // directly into a single clearing pass.
                //
                // The consumer list below is exhaustive and load-bearing: every
                // surface that draws scene-derived content (overview thumbnails, the interactive
                // screenshot picker's scrim, the window switcher, live window
                // previews) must appear here, or it renders over the clear
                // colour instead of the desktop (ADR-0181, [INV-ARCH-65]).
                let desktop_occluded = shell_requires_composition
                    && self.shell.occludes_desktop()
                    && !overview_active
                    && !self.shell.screenshot_active()
                    && !self.screenshot_freeze.active()
                    && !session_locked
                    && window_switcher.is_none()
                    && live_previews.is_empty();

                let overlay_session = if desktop_occluded {
                    let s = self.canvas.begin_session(
                        &mut frame,
                        optics_gfx::CanvasPassOptions {
                            clear: Some(self.clear),
                            /* Moved, not cloned: this branch is the mutually
                             * exclusive alternative to the overlay pass below,
                             * so the scope has exactly one consumer on each
                             * path. */
                            damage: output_damage_scope,
                        },
                    )?;
                    let _uploads = self.device.uploads_begin();
                    // A shell draw failure must not abort the frame: the canvas
                    // session is still open and the compositor can still present
                    // the desktop scene, exactly as the screenshot path below
                    // degrades. Propagating here would drop a live
                    // `CanvasSession` mid-pass and forfeit the whole frame.
                    if let Err(error) = unsafe { self.shell.submit(s.as_raw() as *mut _) } {
                        log::warn!("compositor: shell chrome render failed: {error}");
                    }
                    s
                } else {
                    // Determine upfront whether shell chrome participates in this
                    // pass so desktop surfaces and shell chrome share a single
                    // Canvas pass, eliminating the redundant base-session end
                    // and overlay reopen LOAD round-trip across the 8-bit
                    // surface.
                    let chrome_in_damage_scope = if output_damage_scope.is_empty() {
                        true
                    } else {
                        output_damage_scope.iter().any(|area| {
                            self.shell
                                .chrome_intersects_render_area(*area, scale, logical_size)
                        })
                    };
                    let draw_chrome = shell_requires_composition
                        && chrome_in_damage_scope
                        && (!self.screenshot_freeze.active() || self.screenshot_freeze.opened);

                    // The backdrop beneath frosted / glass chrome (ADR-0253):
                    // captured and filtered at this pass boundary, drawn
                    // between the desktop and the chrome below.
                    let backdrop_plan = if draw_chrome && !self.screenshot_freeze.active() {
                        backdrop::plan(
                            self.shell.design().material,
                            self.shell.backdrop_bodies(),
                            scale,
                            physical_size,
                        )
                        .and_then(|plan| plan.restrict_to(&output_damage_scope))
                    } else {
                        None
                    };
                    let backdrop_slab = match backdrop_plan.as_ref() {
                        Some(plan) => {
                            let captured = match self
                                .backdrop
                                .capture_target(self.device, 0, frame_slot, plan)
                            {
                                Some(capture) => backdrop::capture_desktop(
                                    self.canvas,
                                    self.device,
                                    &mut frame,
                                    capture,
                                    &mut self.wallpaper,
                                    plan,
                                    logical_size,
                                    self.clear,
                                    !wallpaper_occluded,
                                    |canvas| {
                                        draw_direct_desktop_scene(
                                            canvas,
                                            self.device,
                                            self.renderer,
                                            self.server,
                                            render_geometry,
                                            overview_active,
                                            overview_progress,
                                            window_switcher.as_ref(),
                                            color_scheme,
                                            soft_shadow_layer.as_ref(),
                                            shadow_style,
                                            self.overlay_state.active().map(|overlay| overlay.window),
                                        )
                                    },
                                ),
                                None => Err(optics_gfx::Error(
                                    optics_gfx::sys::optics_gfx_result::OPTICS_GFX_ERROR_OUT_OF_MEMORY,
                                )),
                            };
                            match captured {
                                Ok(()) => self.backdrop.compose(self.device, &frame, 0, plan),
                                Err(error) => {
                                    self.backdrop.capture_failed(error);
                                    None
                                }
                            }
                        }
                        None => None,
                    };

                    let active = begin_desktop_output(
                        self.canvas,
                        self.device,
                        &mut frame,
                        &mut self.wallpaper,
                        logical_size,
                        scale,
                        self.clear,
                        output_damage_scope,
                        model_active && !self.screenshot_freeze.active(),
                        wallpaper_occluded,
                    )?;
                    if self.screenshot_freeze.active() {
                        if let Some(image) = self.screenshot_freeze.image() {
                            active.draw_image(
                                image,
                                0.0,
                                0.0,
                                physical_size.0 as f32,
                                physical_size.1 as f32,
                            );
                        }
                    } else {
                        // A local capture has no pixels for the rest of the
                        // output (or the effect failed), so draw the normal
                        // desktop base once before overlaying the effect.
                        draw_direct_desktop_scene(
                            &active,
                            self.device,
                            self.renderer,
                            self.server,
                            render_geometry,
                            overview_active,
                            overview_progress,
                            window_switcher.as_ref(),
                            color_scheme,
                            soft_shadow_layer.as_ref(),
                            shadow_style,
                            self.overlay_state.active().map(|overlay| overlay.window),
                        )?;
                    }

                    if let Some(slab) = backdrop_slab.as_ref() {
                        slab.draw(&active);
                    }

                    if draw_chrome {
                        let _uploads = self.device.uploads_begin();
                        // See the occluded branch above: a shell chrome draw
                        // failure degrades to a chrome-less frame instead of
                        // dropping the still-open session.
                        if let Err(error) = unsafe { self.shell.submit(active.as_raw() as *mut _) }
                        {
                            log::warn!("compositor: shell chrome render failed: {error}");
                        }
                    }

                    // Draw window-switcher and Dock live-preview thumbnails AFTER
                    // `self.shell.submit` so their client-surface miniatures sit on
                    // top of the opaque FloatingPanel background (`#111622`, alpha=242)
                    // inside `card.geometry.preview` (above the disjoint bottom label
                    // strip `card.geometry.label`).
                    if !session_locked && !self.screenshot_freeze.active() {
                        if let Some(presentation) = window_switcher.as_ref() {
                            draw_window_switcher_cards(
                                &active,
                                self.device,
                                self.renderer,
                                self.server,
                                scale,
                                presentation,
                            );
                        } else {
                            draw_live_preview_scenes(
                                &active,
                                self.device,
                                self.renderer,
                                self.server,
                                scale,
                                &live_previews,
                            );
                        }
                    }

                    active
                };

                // Protocol overlays sit above ordinary shell chrome. A modal
                // screenshot/picker owns the whole frame and suppresses live
                // client overlays without changing Wayland keyboard focus.
                if !session_locked
                    && !self.shell.screenshot_active()
                    && !self.screenshot_freeze.active()
                {
                    let captured_cursor = if freeze_capturing || failed_human_freeze {
                        frozen_trigger_cursor
                    } else if bound_saved_screenshot {
                        bound_screenshot_cursor
                    } else {
                        None
                    };
                    let (include_live_cursor, cursor_position) = if freeze_capturing
                        && !human_screenshot_session
                    {
                        // Portal pick sessions are cursor-free.
                        (false, None)
                    } else if freeze_capturing || failed_human_freeze || bound_saved_screenshot {
                        let client_cursor = screenshot_include_cursor
                            && captured_cursor.is_some_and(|cursor| cursor.client_surface);
                        (
                            client_cursor,
                            client_cursor
                                .then(|| captured_cursor.map(|cursor| cursor.position))
                                .flatten(),
                        )
                    } else {
                        (!client_cursor_on_hw_plane, None)
                    };
                    draw_client_overlays(
                        &overlay_session,
                        self.device,
                        self.renderer,
                        self.server,
                        scale,
                        include_live_cursor,
                        cursor_position,
                    );
                }
                // Finish the freeze snapshot pass: the chrome above rendered
                // into the target as well; protocol overlays are included
                // above it. Resolve that whole trigger frame into the
                // on-screen blit, then open the selector over the frozen
                // screen. On failure the selector opens right away over the
                if self.screenshot_freeze.pending_open
                    && (self.screenshot_freeze.captured || self.screenshot_freeze.failed)
                {
                    // A pending IPC pick opens the selector in its picker
                    // mode (ADR-0054); otherwise this is the Print key.
                    match self.pending_pick_open.take() {
                        Some(kind) => self.shell.start_pick(picker_mode(kind)),
                        None => self.shell.start_screenshot(),
                    }
                    self.shell
                        .set_screenshot_freeze(self.screenshot_freeze.captured);
                    self.screenshot_freeze.mark_opened();
                }
                // Confirmation resets the selector before it draws, so this
                // same presentation frame contains the frozen desktop
                // without the selection overlay. Bind that exact frame to
                // the request: the saved pixels are the trigger frame's.
                //
                // A picker-mode confirm (ADR-0054) answers the waiting IPC
                // request instead of the Print-key file path below.
                let screenshot_region = self.shell.take_screenshot_region();
                let pick_consumed_region = if let Some(region) = screenshot_region
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(tessera_protocol::PickResult::Region { rect: region }));
                    true
                } else {
                    false
                };
                if let Some(region) = screenshot_region.filter(|_| !pick_consumed_region) {
                    capturing_frozen_screenshot = self.screenshot_freeze.active();
                    let path = screenshot_path(&self.screenshot_dir);
                    let ts = self.start.elapsed().as_millis() as u64;
                    let command = tessera_protocol::Command::Screenshot {
                        path: path.clone(),
                        region: Some(region),
                    };
                    if self.capture_worker.reserve() {
                        frame_capture = Some(FrameCapture {
                            crop: Some(region),
                            target: CaptureTarget::Screenshot {
                                path,
                                command,
                                ts_mono_ms: ts,
                                origin: tessera_protocol::Origin::Chrome,
                            },
                            cursor: self.screenshot_freeze.trigger_cursor(),
                        });
                    } else {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            tessera_protocol::Origin::Chrome,
                            command,
                            tessera_protocol::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    }
                }
                // The remaining picker results (ADR-0054): a pixel click
                // binds a 1x1 readback of this exact frame; window, whole-
                // output, and cancellation answer immediately.
                if let Some(point) = self.shell.take_picked_point()
                    && let Some(pick) = self.pending_pick.take()
                {
                    if pick.kind == tessera_protocol::PickKind::Pixel
                        && self.capture_worker.reserve()
                    {
                        frame_capture = Some(FrameCapture {
                            crop: Some(tessera_primitives::Rect::new(point.x, point.y, 1, 1)),
                            target: CaptureTarget::Pixel {
                                point,
                                reply: pick.reply,
                            },
                            cursor: None,
                        });
                    } else {
                        let _ = pick
                            .reply
                            .send(Err("another capture is still being processed".into()));
                    }
                }
                if let Some(id) = self.shell.take_picked_window()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let result = if self.server.windows().iter().any(|w| w.id == id) {
                        Ok(tessera_protocol::PickResult::Window { id })
                    } else {
                        Err("the picked window is gone".to_owned())
                    };
                    let _ = pick.reply.send(result);
                }
                if self.shell.take_pick_output()
                    && let Some(pick) = self.pending_pick.take()
                {
                    // The window-mode whole-output answer keeps the legacy
                    // bare shape (no connector) for wire compatibility
                    // (ADR-0128).
                    let _ = pick
                        .reply
                        .send(Ok(tessera_protocol::PickResult::Output { connector: None }));
                }
                if let Some(connector) = self.shell.take_picked_output()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let result = if self
                        .server
                        .output_infos()
                        .iter()
                        .any(|output| output.connector == connector)
                    {
                        Ok(tessera_protocol::PickResult::Output {
                            connector: Some(connector),
                        })
                    } else {
                        Err("the picked output is gone".to_owned())
                    };
                    let _ = pick.reply.send(result);
                }
                if self.shell.take_pick_cancelled()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick.reply.send(Ok(tessera_protocol::PickResult::Cancelled));
                }
                // Safety net: a picker overlay that closed without emitting
                // any event still answers its request (ADR-0054).
                if self.pending_pick.is_some()
                    && self.screenshot_freeze.opened
                    && !self.shell.screenshot_active()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick.reply.send(Ok(tessera_protocol::PickResult::Cancelled));
                }
                // The selector closed this frame (confirmed above or
                // cancelled). This frame still presented the frozen
                // snapshot — exactly what the bound readback captures — so
                // live rendering resumes from the next frame.
                if self
                    .screenshot_freeze
                    .should_disarm(self.shell.screenshot_active())
                {
                    self.screenshot_freeze.disarm();
                    self.shell.set_screenshot_freeze(false);
                }
                if session_locked {
                    draw_lock_scene(
                        &overlay_session,
                        self.device,
                        self.renderer,
                        self.server,
                        physical_size,
                        scale,
                    );
                }
                drain_shell_chrome_events_parts(ShellChromeDrainParts {
                    shell: &mut self.shell,
                    server: self.server,
                    host: self.host,
                    start: self.start,
                    notif_queue: &self.notif_queue,
                    quit_requested: &mut self.quit_requested,
                    ipc: &self.ipc,
                    journal: &self.journal,
                    live: &self.live,
                    screenshot_freeze: &mut self.screenshot_freeze,
                    input_acc: &mut self.input_acc,
                    pending_app_pick: &mut self.pending_app_pick,
                    pending_secret_prompt: &mut self.pending_secret_prompt,
                    pending_confirm_pick: &mut self.pending_confirm_pick,
                    pending_system_action: &mut self.pending_system_action,
                    pending_capability_pick: &mut self.pending_capability_pick,
                    system_confirm_requests: &mut self.system_confirm_requests,
                    system_status: &mut self.system_status,
                    ipc_idle_inhibits: &mut self.ipc_idle_inhibits,
                    idle_process: &mut self.idle_process,
                    network: &self.network,
                    bluetooth: &self.bluetooth,
                    casting: &self.casting,
                    host_system: &self.host_system,
                    status_refresh_tx: &self.status_refresh_tx,
                    config_path: self.config_path.as_deref(),
                    config_writer: &self.config_writer,
                    config: &mut self.config,
                    keymap: &mut self.keymap,
                    gesture_map: &mut self.gesture_map,
                    cursor_cache: &mut self.cursor_cache,
                    reload: &mut self.reload,
                    settings_revision: &mut self.settings_revision,
                    damage: &mut self.damage,
                    scan_req_tx: &self.scan_req_tx,
                    next_app_scan: &mut self.next_app_scan,
                    dock_state: &mut self.dock_state,
                    dock_state_path: &self.dock_state_path,
                    launcher_apps: &self.launcher_apps,
                    icon_cache: &self.icon_cache,
                    automatically_paused_interaction_domains: &mut self
                        .automatically_paused_interaction_domains,
                    capture_worker: &self.capture_worker,
                    pending_interaction_domain_capture: &mut self
                        .pending_interaction_domain_capture,
                    interaction_domain_render_targets: &mut self.interaction_domain_render_targets,
                    interaction_domain_processes: &mut self.interaction_domain_processes,
                    last_interaction_domain_revision: &mut self.last_interaction_domain_revision,
                });
                if self.host.uses_software_cursor() && !capturing_frozen_screenshot {
                    let saved_cursor = frame_capture
                        .as_ref()
                        .filter(|capture| {
                            matches!(&capture.target, CaptureTarget::Screenshot { .. })
                        })
                        .and_then(|capture| capture.cursor);
                    if let Some(cursor) = saved_cursor {
                        if screenshot_include_cursor && !cursor.hidden && !cursor.client_surface {
                            draw_software_cursor(
                                &overlay_session,
                                self.device,
                                &mut self.cursor_cache,
                                cursor.position,
                                cursor.shape,
                                scale,
                            );
                        }
                    } else if !cursor_hidden {
                        draw_software_cursor(
                            &overlay_session,
                            self.device,
                            &mut self.cursor_cache,
                            self.input_acc.cursor,
                            cursor_shape,
                            scale,
                        );
                    }
                }
                overlay_session.end()?;
                let mut capture_for_present = frame_capture.take().and_then(|capture| {
                    let FrameCapture {
                        crop,
                        target,
                        cursor: trigger_cursor,
                    } = capture;
                    let cursor = if matches!(&target, CaptureTarget::Stream) {
                        // Embedded cursor streams (ADR-0127): the binding
                        // carried a filtered cursor state; rasterize the
                        // theme sprite so the worker can blend a composited
                        // twin of this frame next to the pristine one.
                        trigger_cursor.and_then(|cursor| {
                            capture_cursor_snapshot(
                                self.device,
                                &mut self.cursor_cache,
                                cursor.position,
                                cursor.shape,
                                scale,
                            )
                        })
                    } else if screenshot_include_cursor
                        && matches!(&target, CaptureTarget::Screenshot { .. })
                    {
                        if capturing_frozen_screenshot {
                            // The trigger-time cursor is already part of the
                            // frozen image. Adding a readback overlay here
                            // would duplicate it in the saved PNG.
                            None
                        } else if !self.host.uses_software_cursor() {
                            trigger_cursor
                                .filter(|cursor| !cursor.hidden && !cursor.client_surface)
                                .and_then(|cursor| {
                                    capture_cursor_snapshot(
                                        self.device,
                                        &mut self.cursor_cache,
                                        cursor.position,
                                        cursor.shape,
                                        scale,
                                    )
                                })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let physical_crop = crop.map(|rect| {
                        logical_rect_to_physical(
                            rect,
                            render_geometry.scale,
                            physical_size.0,
                            physical_size.1,
                        )
                    });
                    match request_frame_readback(
                        &mut frame,
                        physical_size,
                        physical_crop,
                        cursor,
                        self.capture_worker.security_generation(),
                    ) {
                        Ok(readback) => Some(PendingCapture { readback, target }),
                        Err(reason) => {
                            refuse_capture_target(
                                &self.capture_worker,
                                target,
                                reason,
                                &self.journal,
                                &self.ipc,
                            );
                            None
                        }
                    }
                });
                let overlay_frame = self.active_overlay_dmabuf().map(|(_, frame)| frame);
                if self.overlay_state.active().is_some() && overlay_frame.is_none() {
                    self.overlay_state.deactivate();
                    self.damage.force_full_redraw = true;
                    self.surface.invalidate_damage();
                    log::warn!(
                        "{}: overlay candidate disappeared before present; falling back to composition",
                        self.host.name()
                    );
                    return Ok(PresentationOutcome::Retry {
                        wait: RetryWait::BackendEvent,
                    });
                }
                let submitted = match frame.submit() {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &self.capture_worker,
                                capture.target,
                                format!("captured frame submission failed: {error}"),
                                &self.journal,
                                &self.ipc,
                            );
                        }
                        return Err(error.into());
                    }
                };
                // The KMS hint is relative to the frame that is actually on
                // screen, not to this image's own last use. gfx's scanout
                // ledger includes every submitted-but-abandoned frame, so a
                // Busy retry cannot under-report changed pixels.
                let scanout_damage = FrameDamage::from_gfx_damage(self.surface.scanout_damage()?);
                let damage_rects = scanout_damage.area_rects();
                let render_scale = self
                    .server
                    .output_infos()
                    .first()
                    .map(|output| output.geometry.scale.as_f32())
                    .filter(|scale| *scale > 0.0)
                    .unwrap_or(1.0);
                let frame_size = self.surface.size();
                let direct_outputs = self.direct_outputs.clone();
                let direct_damaged = matches!(scanout_damage, FrameDamage::Full)
                    || direct_outputs.iter().any(|connector| {
                        self.server
                            .output_infos()
                            .iter()
                            .find(|output| output.connector == *connector)
                            .is_some_and(|output| {
                                let rect =
                                    tessera_composite::presentation::logical_rect_to_physical(
                                        output.geometry.logical_rect(),
                                        render_scale,
                                        frame_size.0,
                                        frame_size.1,
                                    );
                                damage_rects
                                    .unwrap_or(&[])
                                    .iter()
                                    .any(|damage| damage.intersect(rect).is_some())
                            })
                    });
                let present_targets = if direct_outputs.is_empty() || direct_damaged {
                    None
                } else {
                    let targets = self
                        .server
                        .output_infos()
                        .iter()
                        .filter(|output| {
                            output.kind.is_physical()
                                && !direct_outputs.contains(&output.connector)
                                && !(output.role.is_mirror()
                                    && self.output_targets.contains_key(&output.connector))
                                && !self.host.output_presentation_pending(&output.connector)
                        })
                        .map(|output| output.connector.clone())
                        .collect::<Vec<_>>();
                    (!targets.is_empty()).then_some(targets)
                };
                let target_presentation = self.can_present_union_targets();
                let present_result = if target_presentation {
                    self.direct_outputs.clear();
                    self.host
                        .compose_only(self.surface, submitted, damage_rects)
                } else if let (Some(active), Some(overlay_surface), None) = (
                    self.overlay_state.active(),
                    overlay_frame.as_ref(),
                    present_targets.as_deref(),
                ) {
                    self.host.present_with_overlay(
                        self.surface,
                        submitted,
                        damage_rects,
                        tessera_platform::host::OverlayLayer {
                            surface: overlay_surface,
                            output: &active.output,
                            target: active.target,
                            zpos: active.zpos,
                        },
                    )
                } else if let Some(targets) = present_targets.as_deref() {
                    let target_refs = targets.iter().map(String::as_str).collect::<Vec<_>>();
                    self.host
                        .present_to(self.surface, submitted, damage_rects, &target_refs)
                } else {
                    self.direct_outputs.clear();
                    self.host.present(self.surface, submitted, damage_rects)
                };
                let completion_fence = match present_result {
                    Ok(fence) => fence,
                    Err(error) => {
                        if self.overlay_state.active().is_some()
                            && matches!(&error, HostError::Drm(DrmError::ScanoutUnsupported))
                        {
                            self.overlay_state.deactivate();
                            self.damage.force_full_redraw = true;
                            self.invalidate_all_surface_damage();
                            log::warn!(
                                "{}: overlay presentation rejected; falling back to composition: {error}",
                                self.host.name()
                            );
                            return Ok(PresentationOutcome::Retry {
                                wait: RetryWait::BackendEvent,
                            });
                        }
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &self.capture_worker,
                                capture.target,
                                format!("captured frame was not presented: {error}"),
                                &self.journal,
                                &self.ipc,
                            );
                        }
                        // Transient direct-display conditions (VT switch,
                        // hotplug reconfigure, flip timeout): drop this frame
                        // and keep the session alive instead of exiting.
                        if matches!(
                            error,
                            HostError::Drm(
                                DrmError::Busy
                                    | DrmError::FlipTimeout
                                    | DrmError::Inactive
                                    | DrmError::Reconfigured
                                    | DrmError::CursorFallback
                            )
                        ) {
                            if matches!(&error, HostError::Drm(DrmError::Busy)) {
                                log::debug!(
                                    "{}: previous atomic commit still busy; coalescing frame",
                                    self.host.name()
                                );
                            } else {
                                log::warn!(
                                    "{}: transient present failure; skipping frame: {error}",
                                    self.host.name()
                                );
                            }
                            // Submission already committed this frame's
                            // pixels and history. Busy/timeout retries need no
                            // full repaint: the abandoned image is complete and
                            // the next scope is computed from its new epoch.
                            // Only a backend epoch/geometry/cursor change
                            // invalidates the framebuffer contract.
                            if matches!(
                                &error,
                                HostError::Drm(
                                    DrmError::Inactive
                                        | DrmError::Reconfigured
                                        | DrmError::CursorFallback
                                )
                            ) {
                                self.damage.force_full_redraw = true;
                            }
                            return Ok(PresentationOutcome::Retry {
                                wait: RetryWait::BackendEvent,
                            });
                        }
                        return Err(error.into());
                    }
                };
                let left_scanout = self.primary_plane_state.commit_composited();
                if left_scanout {
                    log::info!(
                        "{}: compositor reclaimed the primary plane",
                        self.host.name()
                    );
                }
                // Fold the changes the display actually had not seen into
                // every stream's accumulator before any stream-facing sampling
                // below, so a frame captured from this composite carries
                // exactly the damage that produced it (ADR-0127). This is the
                // scanout ledger, not this image's repaint scope: a retried
                // frame's pixels are part of the next captured delta.
                self.streams.accumulate_damage(&scanout_damage);
                if capture_for_present
                    .as_ref()
                    .is_some_and(|capture| matches!(capture.target, CaptureTarget::Stream))
                {
                    // A stream readback is bound to this frame: sample each
                    // due SHM stream's damage for its delivery.
                    self.stash_shm_stream_damage();
                }
                let repaint_label = match &repaint {
                    FrameDamage::Full => "full".to_owned(),
                    FrameDamage::None => "none".to_owned(),
                    FrameDamage::Area(rects) => format!("{} rect(s)", rects.len()),
                };
                let scanout_label = match &scanout_damage {
                    FrameDamage::Full => "full".to_owned(),
                    FrameDamage::None => "none".to_owned(),
                    FrameDamage::Area(rects) => format!("{} rect(s)", rects.len()),
                };
                log::trace!(
                    "present: slot={frame_slot} image={image_index} damage={repaint_label} scanout={scanout_label}"
                );
                if let Some(capture) = capture_for_present {
                    debug_assert!(self.pending_capture.is_none());
                    self.pending_capture = Some(capture);
                }
                // The frame is on its way to scanout: copy it into every due
                // dmabuf stream's capture-surface slot (IPC protocol 25). The
                // frame events go out once each slot's acquire fence signals.
                // Due-ness alone only rides this already-happening composite
                // (ADR-0126); it never caused the frame.
                if dmabuf_stream_capture_due {
                    self.blit_dmabuf_stream_frames(&completion_fence);
                }
                if let Some(overlay) = self.overlay_state.active()
                    && !sampled_surface_ids.contains(&overlay.surface_id)
                {
                    // The overlay surface bypassed the composite pass, but it
                    // is still owned by this presented frame until its page
                    // flip retires.
                    sampled_surface_ids.push(overlay.surface_id);
                }
                let (target_outputs, target_kms_fence) = if target_presentation {
                    let (outputs, kms_fence) =
                        self.present_union_to_targets(cursor_shape, cursor_hidden)?;
                    if outputs.is_empty() {
                        self.damage.pending_damage = Some(FrameDamage::Full);
                        return Ok(PresentationOutcome::Retry {
                            wait: RetryWait::BackendEvent,
                        });
                    }
                    (Some(outputs), kms_fence)
                } else {
                    (None, tessera_platform::sync::CompletionFence::Signaled)
                };
                let presented_outputs = target_outputs
                    .as_ref()
                    .map(|targets| targets.iter().map(String::as_str).collect::<Vec<_>>())
                    .or_else(|| {
                        present_targets
                            .as_ref()
                            .map(|targets| targets.iter().map(String::as_str).collect::<Vec<_>>())
                    });
                self.server.acknowledge_presented_surface_damage_on_outputs(
                    sampled_surface_ids.iter(),
                    drew_preview_sources,
                    presented_outputs.as_deref(),
                );
                if self.server.lock_confirmation_pending() {
                    match self.host.wait_presented(self.device) {
                        Ok(()) => self.server.presentation_complete(),
                        Err(error) => log::error!(
                            "session lock: secure frame was not confirmed; keeping lock request pending: {error}"
                        ),
                    }
                }
                let merged_completion_fence =
                    tessera_platform::sync::merge_completions([completion_fence, target_kms_fence]);
                self.release_retired_buffers_after_composited_frame(
                    presented_outputs.as_deref(),
                    merged_completion_fence,
                );

                // Pace clients: fire frame callbacks for this presentation and
                // complete `wp_presentation_feedback` for exactly the surfaces
                // this frame sampled (ADR-0122, `[INV-ARCH-84]`).
                self.server.send_frame_callbacks_on_outputs(
                    self.server.monotonic_ms(),
                    PresentedSurfaces::Sampled {
                        ids: &sampled_surface_ids,
                        drew_preview_sources,
                    },
                    presented_outputs.as_deref(),
                );

                // The frame landed: re-anchor the present-side damage
                // baselines (clock minute, cursor, post-resize full redraw).
                self.damage.last_present_minute = Some(wall_clock_minute());
                self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                self.damage.last_presented_client_cursor = client_cursor_key;
                self.damage.last_presented_cursor_position = Some(cursor_position);
                self.damage.last_presented_cursor_hotspot =
                    hw_cursor_visible.then_some(last_committed_hotspot);
                self.damage.last_presented_cursor_pixels =
                    hw_cursor_visible.then_some(last_committed_sprite_size);
                self.damage.force_full_redraw = false;

                // When `target_presentation` ran, `present_union_to_targets`
                // already blitted and committed every output target (including
                // mirrors). Running `present_output_targets` again on the same
                // frame would hit `DrmError::Busy` and evict the mirror target.
                if !target_presentation {
                    self.present_output_targets(cursor_shape, cursor_hidden);
                }
                self.frame_count += 1;
                if self.frame_count == 1 {
                    log::info!(
                        "{}: first frame presented (with shell chrome)",
                        self.host.name()
                    );
                }
            }
            Err(error) if error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY => {
                log::trace!("frame acquire deferred: image lease still held: {error}");
                if let Some(capture) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "image lease pending before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                self.damage.pending_damage = Some(match self.damage.pending_damage.take() {
                    Some(pending) => union_frame_damage(pending, damage.clone()),
                    None => damage.clone(),
                });
                return Ok(PresentationOutcome::Retry {
                    wait: RetryWait::BackendEvent,
                });
            }
            Err(error)
                if error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_TIMEOUT =>
            {
                // A capture bound by the pre-pass never got its frame; refuse
                // it so the worker lane is released for the next iteration.
                log::trace!("frame acquire timed out: {error}");
                if let Some(capture) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "output frame timed out before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                // The previous frame's GPU work did not retire inside the
                // frame timeout (or the presentation engine released no
                // swapchain image): transient. No gfx frame exists, so this
                // damage has no epoch entry yet. Carry it forward verbatim;
                // collapsing to Full here is what turned a transient acquire
                // miss into three full-output repaints.
                self.damage.pending_damage = Some(match self.damage.pending_damage.take() {
                    Some(pending) => union_frame_damage(pending, damage.clone()),
                    None => damage.clone(),
                });
                return Ok(PresentationOutcome::Retry {
                    wait: RetryWait::Deadline,
                });
            }
            Err(_) => {
                if let Some(capture) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "output changed before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                let (nw, nh) = self.host.physical_size();
                match self.surface.resize(nw, nh) {
                    Ok(()) => {}
                    Err(error)
                        if error.0 == optics_gfx_sys::optics_gfx_result::OPTICS_GFX_ERROR_BUSY =>
                    {
                        // A scanout lease still owns the old ring image. Keep
                        // the pending damage and retry once the corresponding
                        // page flip releases it.
                        self.damage.pending_damage =
                            Some(match self.damage.pending_damage.take() {
                                Some(pending) => union_frame_damage(pending, damage.clone()),
                                None => damage.clone(),
                            });
                        return Ok(PresentationOutcome::Retry {
                            wait: RetryWait::BackendEvent,
                        });
                    }
                    Err(error) => return Err(error.into()),
                }
                // The frame size a stream negotiated at start no longer
                // matches the output: freeze each affected stream with
                // `StreamGeometryChanged` so consumers renegotiate at the
                // new geometry instead of compositing mismatched frames
                // (ADR-0126). Runs after the resize so the new surface size
                // is the geometry streams are compared against.
                self.handle_output_geometry_change();
                // Out-of-date / lost: rebuild the swapchain at the current
                // physical size.
                if let Some(capture) = self.pending_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "output changed before the captured frame became readable".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                // The frozen snapshot no longer matches the output geometry;
                // end the session and close the selector rather than
                // presenting a stretched frame or cropping stale pixels.
                if self.screenshot_freeze.armed {
                    self.screenshot_freeze.disarm();
                    self.shell.set_screenshot_freeze(false);
                    if self.shell.screenshot_active() {
                        self.shell.start_screenshot();
                    }
                }
                self.invalidate_all_surface_damage();
                // Damage tracked against the old framebuffer does not
                // describe the rebuilt one; render the next frame in full.
                self.damage.force_full_redraw = true;
                if let Err(error) = self.surface.prepare_readback() {
                    log::warn!(
                        "capture: could not preallocate resized readback staging: {error}{}",
                        gfx_last_error_detail()
                    );
                }
                // The surface was rebuilt; no backend event exists for it.
                return Ok(PresentationOutcome::Retry {
                    wait: RetryWait::Deadline,
                });
            }
        }

        Ok(PresentationOutcome::Submitted)
    }

    /// Synchronize server model snapshots (windows, global window strip,
    /// workspaces, outputs, and Interaction Domains) into the shell and IPC
    /// mirror before `Shell::prepare_frame`, `assess_frame_damage`, and
    /// `plan_output_layers` run.
    ///
    /// Does not mutate `self.last_*` revision baselines — `assess_frame_damage`
    /// consumes the delta to flag full scene/backdrop damage and then advances
    /// those baselines for both direct-scanout and composited frames.
    fn sync_shell_and_ipc_snapshots(&mut self) {
        // Resolve presentation ownership before anything else in the frame
        // (ADR-0183). Chrome is told before it receives the window snapshot, so
        // a Dock released from ownership recomputes its obscuration against a
        // verdict that is already current. The verdict is scanned exactly once
        // and every consumer below reads `self.space_use`.
        self.space_use = self.server.space_use();
        if self.published_space_use != Some(self.space_use) {
            log::debug!(
                "space_use: {:?} -> {:?}",
                self.published_space_use,
                self.space_use
            );
            self.published_space_use = Some(self.space_use);
            self.shell.set_space_use(self.space_use);
            // The transition itself is unsigned: chrome standing down or back
            // up neither moves the window signature nor belongs to any client
            // commit, so without this the HUD's last pixels would survive in
            // the repaint history (and its returned pixels would never be
            // drawn) until unrelated damage arrived.
            self.damage.chrome_dirty = true;
            if let Some(s) = self.ipc.as_ref() {
                s.broadcast(tessera_protocol::Event::SpaceUseChanged {
                    state: self.space_use,
                });
            }
        }
        let windows_hash = self.server.windows_signature();
        if self.last_windows_hash != Some(windows_hash) {
            let win_snapshot = self.server.windows();
            let sig: WindowEventSignature = win_snapshot
                .iter()
                .map(|w| {
                    (
                        w.id,
                        w.state.activated,
                        w.state.maximized,
                        w.state.fullscreen,
                        w.minimized,
                        w.always_on_top,
                        w.title.clone(),
                    )
                })
                .collect();
            if self.last_win_sig.as_ref() != Some(&sig) {
                self.last_win_sig = Some(sig);
                if let Some(s) = self.ipc.as_ref() {
                    s.broadcast(tessera_protocol::Event::WindowsChanged);
                }
            }
            self.live.set_windows(win_snapshot.clone());
            self.shell.set_windows(win_snapshot);
        }
        let all_windows_hash = self.server.all_windows_signature();
        if self.last_all_windows_hash != Some(all_windows_hash) {
            let all_windows = self.server.all_windows();
            self.live.set_all_windows(all_windows.clone());
            self.shell.set_all_windows(all_windows);
        }
        let ws_sig = self.server.workspace_signature();
        if self.last_ws_sig != Some(ws_sig) {
            let ws_snap = self.server.workspace_snapshot();
            self.live.set_workspaces(ws_snap.clone());
            self.shell.set_workspaces(ws_snap);
            if let Some(s) = self.ipc.as_ref() {
                s.broadcast(tessera_protocol::Event::WorkspaceChanged);
            }
        }
        let outputs_revision = self.server.outputs_revision();
        if self.last_outputs_revision != Some(outputs_revision) {
            let output_snapshot = self.server.output_infos();
            self.live.set_outputs(output_snapshot.clone());
            let display_changed = self.system_status.display.configurable
                != (self.host.name() == "drm")
                || self.system_status.display.outputs != output_snapshot;
            if display_changed {
                self.system_status.display.configurable = self.host.name() == "drm";
                self.system_status.display.outputs = output_snapshot;
                publish_system_status_parts(
                    &self.system_status,
                    &mut self.shell,
                    &self.live,
                    &self.ipc,
                );
            }
        }
        let interaction_domain_revision = self.server.interaction_domain_revision();
        if self.last_interaction_domain_revision != Some(interaction_domain_revision) {
            let interaction_domain_snapshot = self.server.interaction_domain_snapshot();
            self.live
                .set_interaction_domains(interaction_domain_snapshot.clone());
            self.shell
                .set_interaction_domains(interaction_domain_snapshot);
            if let Some(s) = self.ipc.as_ref() {
                s.broadcast(tessera_protocol::Event::InteractionDomainsChanged {
                    revision: interaction_domain_revision,
                });
            }
        }
    }
}

pub(super) fn capture_cursor_snapshot(
    device: &optics_gfx::Device,
    cache: &mut cursor::CursorCache,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) -> Option<CaptureCursor> {
    cache.get(device, shape, scale).map(|loaded| CaptureCursor {
        x: (position.0 * scale - loaded.xhot).round() as i32,
        y: (position.1 * scale - loaded.yhot).round() as i32,
        width: loaded.width.round().max(1.0) as u32,
        height: loaded.height.round().max(1.0) as u32,
        bgra: std::sync::Arc::clone(&loaded.pixels),
    })
}

#[allow(clippy::too_many_arguments)]
/// Whether any physical output currently has an active mirror target.
///
/// Only mirror targets lack an independent KMS cursor plane; single-output and
/// extended DRM targets own hardware cursor planes and stay eligible for the
/// zero-GPU cursor-only commit path.
pub(super) fn has_active_mirror_target(
    outputs: &[tessera_desktop::output::OutputInfo],
    has_target: impl Fn(&str) -> bool,
) -> bool {
    outputs
        .iter()
        .any(|output| output.role.is_mirror() && has_target(&output.connector))
}

pub(super) fn begin_desktop_output<'c, 'surface>(
    canvas: &'c mut optics_gfx::Canvas,
    device: &optics_gfx::Device,
    frame: &'c mut optics_gfx::Frame<'surface>,
    wallpaper: &mut Option<wallpaper::Wallpaper>,
    logical_size: (u32, u32),
    scale: f32,
    clear: u32,
    damage: Vec<optics_gfx::CanvasRenderArea>,
    model_active: bool,
    wallpaper_occluded: bool,
) -> Result<optics_gfx::CanvasSession<'c, &'c mut optics_gfx::Frame<'surface>>, optics_gfx::Error> {
    if model_active {
        let s = canvas.begin_session(
            &mut *frame,
            optics_gfx::CanvasPassOptions {
                clear: Some(clear),
                damage: damage.clone(),
            },
        )?;
        if !wallpaper_occluded {
            draw_wallpaper_background(&s, device, wallpaper, logical_size, scale);
        }
        s.end()?;
        if let Some(w) = wallpaper.as_mut() {
            w.draw_model(device, frame);
        }
        canvas.begin_session(
            frame,
            optics_gfx::CanvasPassOptions {
                clear: None,
                damage,
            },
        )
    } else {
        let s = canvas.begin_session(
            frame,
            optics_gfx::CanvasPassOptions {
                clear: Some(clear),
                damage,
            },
        )?;
        if !wallpaper_occluded {
            draw_wallpaper_background(&s, device, wallpaper, logical_size, scale);
        }
        Ok(s)
    }
}

impl CompositorRuntime<'_> {
    /// Keep the backdrop executor and the shell's material in agreement
    /// (ADR-0253, [INV-MAT-05]).
    ///
    /// A material that does not sample the backdrop holds no filter and no
    /// capture target. A backdrop that failed demotes the shell to the
    /// translucent material, the most expensive one that needs no capture,
    /// rather than leaving glass-tuned bodies over an unblurred desktop.
    /// Runs before the shell builds its frame, so the demotion is drawn on
    /// the frame after the failure, and re-applies after a configuration
    /// reload asks for glass again.
    pub(super) fn enforce_backdrop_capability(&mut self) {
        let design = self.shell.design();
        if !design.material.samples_backdrop() {
            self.backdrop.release();
            self.freeze_backdrop.release();
            return;
        }
        if !self.backdrop.available() {
            let scheme = design.scheme;
            log::warn!(
                "appearance: material {} drawn as translucent (backdrop unavailable)",
                design.material.as_str()
            );
            self.backdrop.release();
            self.shell
                .set_appearance(scheme, tessera_desktop::settings::Material::Translucent);
            // Settings explains it the way it explains every other ceiling.
            if let Some(resolution) = self.shell.material_resolution() {
                self.shell
                    .set_material_resolution(tessera_desktop::settings::ResolvedMaterial {
                        effective: tessera_desktop::settings::Material::Translucent,
                        limited_by: Some(
                            tessera_desktop::settings::MaterialCeilingReason::Renderer,
                        ),
                        ..resolution
                    });
                self.publish_settings();
            }
        }
    }

    /// Drain all shell chrome events emitted during `prepare_frame`.
    ///
    /// Called on every presentation path (including `render_output_targets`
    /// and fast paths) so Dock/Pivot launches, pin edits, window actions, and
    /// portal answers are never dropped when the union render path is bypassed.
    pub(super) fn drain_shell_chrome_events(&mut self) {
        drain_shell_chrome_events_parts(ShellChromeDrainParts {
            shell: &mut self.shell,
            server: self.server,
            host: self.host,
            start: self.start,
            notif_queue: &self.notif_queue,
            quit_requested: &mut self.quit_requested,
            ipc: &self.ipc,
            journal: &self.journal,
            live: &self.live,
            screenshot_freeze: &mut self.screenshot_freeze,
            input_acc: &mut self.input_acc,
            pending_app_pick: &mut self.pending_app_pick,
            pending_secret_prompt: &mut self.pending_secret_prompt,
            pending_confirm_pick: &mut self.pending_confirm_pick,
            pending_system_action: &mut self.pending_system_action,
            pending_capability_pick: &mut self.pending_capability_pick,
            system_confirm_requests: &mut self.system_confirm_requests,
            system_status: &mut self.system_status,
            ipc_idle_inhibits: &mut self.ipc_idle_inhibits,
            idle_process: &mut self.idle_process,
            network: &self.network,
            bluetooth: &self.bluetooth,
            casting: &self.casting,
            host_system: &self.host_system,
            status_refresh_tx: &self.status_refresh_tx,
            config_path: self.config_path.as_deref(),
            config_writer: &self.config_writer,
            config: &mut self.config,
            keymap: &mut self.keymap,
            gesture_map: &mut self.gesture_map,
            cursor_cache: &mut self.cursor_cache,
            reload: &mut self.reload,
            settings_revision: &mut self.settings_revision,
            damage: &mut self.damage,
            scan_req_tx: &self.scan_req_tx,
            next_app_scan: &mut self.next_app_scan,
            dock_state: &mut self.dock_state,
            dock_state_path: &self.dock_state_path,
            launcher_apps: &self.launcher_apps,
            icon_cache: &self.icon_cache,
            automatically_paused_interaction_domains: &mut self
                .automatically_paused_interaction_domains,
            capture_worker: &self.capture_worker,
            pending_interaction_domain_capture: &mut self.pending_interaction_domain_capture,
            interaction_domain_render_targets: &mut self.interaction_domain_render_targets,
            interaction_domain_processes: &mut self.interaction_domain_processes,
            last_interaction_domain_revision: &mut self.last_interaction_domain_revision,
        });
    }
}

struct ShellChromeDrainParts<'a> {
    shell: &'a mut tessera_shell::Shell,
    server: &'a mut tessera_wayland::Server,
    host: &'a mut Host,
    start: std::time::Instant,
    notif_queue: &'a std::sync::Arc<std::sync::Mutex<tessera_desktop::notify::NotificationQueue>>,
    quit_requested: &'a mut bool,
    ipc: &'a Option<tessera_ipc::Server>,
    journal: &'a std::sync::Arc<
        std::sync::Mutex<tessera_audit::AuditLog<tessera_audit::journal::Entry>>,
    >,
    live: &'a std::sync::Arc<LiveState>,
    screenshot_freeze: &'a mut ScreenshotFreeze,
    input_acc: &'a mut InputAccumulator,
    pending_app_pick: &'a mut Option<PendingAppPick>,
    pending_secret_prompt: &'a mut Option<PendingSecretPrompt>,
    pending_confirm_pick: &'a mut Option<PendingConfirmPick>,
    pending_system_action: &'a mut Option<tessera_desktop::system::SystemAction>,
    pending_capability_pick: &'a mut Option<PendingCapabilityPick>,
    system_confirm_requests: &'a mut Vec<tessera_desktop::system::SystemAction>,
    system_status: &'a mut tessera_shell::component::SystemStatus,
    ipc_idle_inhibits: &'a mut IdleInhibits,
    idle_process: &'a mut session::IdleProcess,
    network: &'a crate::network::NetworkHandle,
    bluetooth: &'a crate::bluetooth::BluetoothHandle,
    casting: &'a crate::casting::CastHandle,
    host_system: &'a std::sync::Arc<dyn crate::host_system::HostSystem>,
    status_refresh_tx: &'a std::sync::mpsc::Sender<()>,
    config_path: Option<&'a std::path::Path>,
    config_writer: &'a ConfigWriter,
    config: &'a mut Option<tessera_config::Config>,
    keymap: &'a mut tessera_desktop::keybind::Keymap,
    gesture_map: &'a mut tessera_desktop::gesture::GestureMap,
    cursor_cache: &'a mut cursor::CursorCache,
    reload: &'a mut Option<tessera_config::ReloadWatcher>,
    settings_revision: &'a mut u64,
    damage: &'a mut DamageTracking,
    scan_req_tx: &'a std::sync::mpsc::Sender<AppScanRequest>,
    next_app_scan: &'a mut std::time::Instant,
    dock_state: &'a mut tessera_wayland::DockStateStore,
    dock_state_path: &'a std::path::Path,
    launcher_apps: &'a [tessera_desktop::app::Entry],
    icon_cache: &'a IconCache,
    automatically_paused_interaction_domains: &'a mut std::collections::BTreeSet<
        tessera_authority::interaction_domain::InteractionDomainId,
    >,
    capture_worker: &'a CaptureWorker,
    pending_interaction_domain_capture: &'a mut Option<PendingInteractionDomainCapture>,
    interaction_domain_render_targets: &'a mut std::collections::BTreeMap<
        tessera_authority::interaction_domain::InteractionDomainId,
        InteractionDomainRenderTarget,
    >,
    interaction_domain_processes: &'a mut InteractionDomainProcesses,
    last_interaction_domain_revision: &'a mut Option<u64>,
}

fn drain_shell_chrome_events_parts(parts: ShellChromeDrainParts<'_>) {
    // App-pick delivery (the AppChooser portal's compositor side).
    if let Some(id) = parts.shell.take_app_pick_confirmed()
        && let Some(pick) = parts.pending_app_pick.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::AppPickResult::App { id }));
    }
    if parts.shell.take_app_pick_cancelled()
        && let Some(pick) = parts.pending_app_pick.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::AppPickResult::Cancelled));
    }
    if parts.pending_app_pick.is_some()
        && !parts.shell.app_pick_active()
        && let Some(pick) = parts.pending_app_pick.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::AppPickResult::Cancelled));
    }
    // Secret-prompt delivery (the vault password unlock's compositor side).
    if let Some(value) = parts.shell.take_secret_prompt_confirmed()
        && let Some(pick) = parts.pending_secret_prompt.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::SecretPromptResult::Secret { value }));
    }
    if parts.shell.take_secret_prompt_cancelled()
        && let Some(pick) = parts.pending_secret_prompt.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::SecretPromptResult::Cancelled));
    }
    if parts.pending_secret_prompt.is_some()
        && !parts.shell.secret_prompt_active()
        && let Some(pick) = parts.pending_secret_prompt.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_protocol::SecretPromptResult::Cancelled));
    }
    // Confirmation delivery (portal consent dialogs and ADR-0088 runtime grants).
    let confirm_answer = parts.shell.take_confirm_pick_answered();
    if let Some(answer) = confirm_answer
        && let Some(pick) = parts.pending_confirm_pick.take()
    {
        let _ = pick.reply.send(Ok(answer));
    }
    if parts.pending_confirm_pick.is_some()
        && !parts.shell.confirm_pick_active()
        && let Some(pick) = parts.pending_confirm_pick.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_shell::component::ConfirmAnswer::Cancelled));
    }
    // Destructive system actions parked behind the same consent chrome.
    if let Some(answer) = confirm_answer
        && let Some(action) = parts.pending_system_action.take()
        && matches!(
            answer,
            tessera_shell::component::ConfirmAnswer::Confirmed
                | tessera_shell::component::ConfirmAnswer::AllowOnce
        )
        && let Err(reason) = apply_system_action(
            parts.server,
            parts.host,
            parts.notif_queue,
            parts.system_status,
            parts.ipc_idle_inhibits,
            parts.idle_process,
            parts.network,
            parts.bluetooth,
            parts.casting,
            &**parts.host_system,
            action,
        )
    {
        log::warn!("system control: {reason}");
    }
    if parts.pending_system_action.is_some() && !parts.shell.confirm_pick_active() {
        *parts.pending_system_action = None;
    }
    // Capability-checklist delivery (ADR-0088 agent pairing).
    if let Some(result) = parts.shell.take_capability_pick_answered()
        && let Some(pick) = parts.pending_capability_pick.take()
    {
        let _ = pick.reply.send(Ok(result));
    }
    if parts.pending_capability_pick.is_some()
        && !parts.shell.capability_pick_active()
        && let Some(pick) = parts.pending_capability_pick.take()
    {
        let _ = pick
            .reply
            .send(Ok(tessera_shell::component::CapabilityPickResult {
                approved: None,
            }));
    }
    // Drain chrome interactions and forward through the apply
    // chokepoint (ADR-0033) so the journal records them.
    let ts = parts.start.elapsed().as_millis() as u64;
    let origin = tessera_protocol::Origin::Chrome;
    if let Some(id) = parts.shell.take_window_switcher_pick() {
        parts.server.cancel_window_switcher();
        parts.shell.finish_window_switcher();
        apply_chrome_window_command(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            tessera_protocol::Command::Focus { id, reveal: true },
            parts.ipc,
            parts.journal,
            ts,
        );
    }
    if parts.shell.take_window_switcher_cancel() {
        parts.server.cancel_window_switcher();
        parts.shell.finish_window_switcher();
    }
    if let Some(id) = parts.shell.take_clicked_window() {
        apply_chrome_window_command(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            tessera_protocol::Command::Focus { id, reveal: true },
            parts.ipc,
            parts.journal,
            ts,
        );
    }
    // Overview intents (M9): a thumbnail pick focuses its window;
    // a rail tile switches workspace while the overview stays open.
    if let Some(id) = parts.shell.take_overview_pick() {
        apply_chrome_window_command(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            tessera_protocol::Command::Focus { id, reveal: true },
            parts.ipc,
            parts.journal,
            ts,
        );
    }
    if let Some(id) = parts.shell.take_overview_switch() {
        let cmd = tessera_protocol::Command::SwitchWorkspaceTo { id };
        apply_command_and_journal(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            cmd,
            parts.ipc,
            parts.journal,
            ts,
            origin.clone(),
        );
    }
    for action in parts.shell.take_window_actions() {
        let cmd = match action {
            tessera_shell::component::WindowAction::Focus(id) => {
                tessera_protocol::Command::Focus { id, reveal: true }
            }
            tessera_shell::component::WindowAction::Minimize(id) => {
                tessera_protocol::Command::Minimize { id }
            }
            tessera_shell::component::WindowAction::SetMaximized(id, maximized) => {
                tessera_protocol::Command::SetMaximized { id, maximized }
            }
            tessera_shell::component::WindowAction::SetAlwaysOnTop(id, on_top) => {
                tessera_protocol::Command::SetAlwaysOnTop { id, on_top }
            }
            tessera_shell::component::WindowAction::Close(id) => {
                tessera_protocol::Command::Close { id }
            }
        };
        apply_chrome_window_command(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            cmd,
            parts.ipc,
            parts.journal,
            ts,
        );
    }
    // Mirror-guard drags move a read-only mirror's presentation
    // position. Like a title-bar move grab this is a
    // chrome-owned gesture, not a journaled command.
    if let Some(mirror_move) = parts.shell.take_mirror_move() {
        parts
            .server
            .start_mirror_move(mirror_move.window, mirror_move.cursor);
    }
    if let Some(id) = parts.shell.take_switch_workspace() {
        let cmd = tessera_protocol::Command::SwitchWorkspaceTo { id };
        apply_command_and_journal(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            cmd,
            parts.ipc,
            parts.journal,
            ts,
            origin.clone(),
        );
    }
    for id in parts.shell.take_dismissed_notifications() {
        let cmd = tessera_protocol::Command::DismissNotification { id };
        apply_command_and_journal(
            parts.server,
            parts.notif_queue,
            parts.quit_requested,
            cmd,
            parts.ipc,
            parts.journal,
            ts,
            origin.clone(),
        );
    }
    if let Some(app) = parts.shell.take_open_builtin() {
        // The selector opens through the freeze session so the
        // trigger frame (chrome included) is snapshotted first.
        if app == tessera_desktop::app::BuiltInApplication::ScreenshotSelector {
            let cursor = CompositorRuntime::capture_cursor_state_parts(
                parts.input_acc.cursor,
                parts.input_acc.display_size,
                parts.server,
                parts.shell,
            );
            parts.screenshot_freeze.request_open(Some(cursor));
        } else {
            parts.shell.open_builtin(app);
        }
    }
    for intent in parts.shell.take_interaction_domain_intents() {
        let action = interaction_domain_intent_to_action(intent);
        let before_revision = parts.server.interaction_domain_revision();
        let result = apply_interaction_domain_action(parts.server, None, action.clone());
        match &result {
            Ok(_) => {
                for interaction_domain in interaction_domains_explicitly_stopped(&action) {
                    parts
                        .automatically_paused_interaction_domains
                        .remove(&interaction_domain);
                }
                let invalidated = interaction_domain_action_invalidates_capture(&action);
                if !invalidated.is_empty() {
                    parts.capture_worker.invalidate_security_context();
                    if parts
                        .pending_interaction_domain_capture
                        .as_ref()
                        .is_some_and(|pending| {
                            invalidated.contains(&pending.context.interaction_domain)
                        })
                    {
                        let pending = parts
                            .pending_interaction_domain_capture
                            .take()
                            .expect("capture invalidation predicate checked");
                        refuse_capture_target(
                            parts.capture_worker,
                            CaptureTarget::InteractionDomainReply {
                                context: pending.context,
                                reply: pending.reply,
                            },
                            "Interaction Domain authority changed before capture completed".into(),
                            parts.journal,
                            parts.ipc,
                        );
                    }
                }
                if let tessera_protocol::InteractionDomainAction::Revoke {
                    interaction_domain,
                    ..
                } = &action
                {
                    parts
                        .interaction_domain_render_targets
                        .remove(interaction_domain);
                }
                parts
                    .interaction_domain_processes
                    .apply_committed_action(&action);
                let snapshot = parts.server.interaction_domain_snapshot();
                *parts.last_interaction_domain_revision = Some(snapshot.revision);
                parts.live.set_interaction_domains(snapshot.clone());
                parts.shell.set_interaction_domains(snapshot.clone());
                if let Some(ipc) = parts.ipc {
                    ipc.broadcast(tessera_protocol::Event::InteractionDomainsChanged {
                        revision: snapshot.revision,
                    });
                }
            }
            Err(error) => {
                log::warn!("Interaction Domain action from shell refused: {error}");
                let notification = parts
                    .notif_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(
                        "Interaction Domain",
                        error.clone(),
                        Some("tessera".into()),
                        ts,
                    );
                if let Some(ipc) = parts.ipc {
                    ipc.broadcast(tessera_protocol::Event::Notified { notification });
                }
            }
        }
        let after_revision = parts.server.interaction_domain_revision();
        let effect = match result {
            Ok(_) => tessera_protocol::Effect::Applied,
            Err(reason) => tessera_protocol::Effect::Refused { reason },
        };
        journal_mutation_effect_and_broadcast(
            parts.journal,
            parts.ipc,
            ts,
            tessera_protocol::Origin::Chrome,
            tessera_protocol::JournalMutation::InteractionDomain {
                action,
                before_revision,
                after_revision,
            },
            effect,
        );
    }
    let system_actions = parts.shell.take_system_actions();
    if !system_actions.is_empty() {
        let mut applied = false;
        for action in system_actions {
            // The reading describes the request that succeeds, so it
            // needs the action after the call that consumes it.
            let applied_action = action.clone();
            // Destructive lifecycle transitions route through the
            // system-level confirmation first: the same consent
            // chrome the portal flows use, so a stray click on the
            // panel's power knob never powers the machine off.
            // The consent chrome opens in the iteration loop right
            // after this frame (the gfx frame's borrow below
            // forbids the `&mut parts` call here).
            if matches!(
                action,
                tessera_desktop::system::SystemAction::PowerOff
                    | tessera_desktop::system::SystemAction::Reboot
                    | tessera_desktop::system::SystemAction::Suspend
            ) && parts.pending_system_action.is_none()
            {
                parts.system_confirm_requests.push(action);
                continue;
            }
            let command = tessera_protocol::Command::System {
                action: action.clone(),
            };
            let effect = match apply_system_action(
                parts.server,
                parts.host,
                parts.notif_queue,
                parts.system_status,
                parts.ipc_idle_inhibits,
                parts.idle_process,
                parts.network,
                parts.bluetooth,
                parts.casting,
                &**parts.host_system,
                action,
            ) {
                Ok(()) => {
                    applied = true;
                    // A reading acknowledges a completed action, so
                    // it is raised against the value the machine
                    // reached rather than the one it was asked for.
                    if let Some(params) = super::system::osd_for_action(
                        &applied_action,
                        parts.system_status,
                        parts.shell.localizer(),
                    ) {
                        parts.shell.show_osd(params);
                    }
                    tessera_protocol::Effect::Applied
                }
                Err(reason) => {
                    log::warn!("system control: {reason}");
                    tessera_protocol::Effect::Refused { reason }
                }
            };
            journal_effect_and_broadcast(
                parts.journal,
                parts.ipc,
                ts,
                origin.clone(),
                command,
                effect,
            );
        }
        if applied {
            publish_system_status_parts(parts.system_status, parts.shell, parts.live, parts.ipc);
            // Reconcile the optimistic hardware state out of
            // cycle: `apply_system_action` already published
            // optimistic values, and the poller thread
            // re-probes the host right away so the main loop never
            // blocks on a subprocess.
            let _ = parts.status_refresh_tx.send(());
        }
    }
    // Persistent-settings mutations from compositor-owned settings
    // UI: the same commit/journal path as IPC settings requests.
    let settings_actions = parts.shell.take_settings_actions();
    for (expected_revision, action) in settings_actions {
        let appearance_changed = matches!(
            &action,
            tessera_protocol::SettingsAction::SetDesktopPreferences { .. }
        );
        let before_revision = *parts.settings_revision;
        let result = if parts.server.session_locked() {
            Err("session is locked".into())
        } else {
            // Disjoint-field form: a gfx frame borrows
            // `parts.surface` across this whole section.
            settings::commit_settings_parts(
                expected_revision,
                action.clone(),
                parts.settings_revision,
                parts.config_path,
                parts.config_writer,
                parts.config,
                parts.keymap,
                parts.gesture_map,
                parts.server,
                parts.shell,
                parts.cursor_cache,
                parts.host,
                parts.reload,
                parts.idle_process,
                parts.live,
                parts.system_status,
                parts.input_acc,
                parts.ipc,
            )
        };
        if result.is_ok() {
            // A committed settings action may redraw status chrome
            // outside the signed server-state paths.
            parts.damage.chrome_dirty = true;
            if appearance_changed {
                queue_app_scan_parts(
                    parts.server,
                    parts.host,
                    parts.config.as_ref(),
                    parts.scan_req_tx,
                    parts.next_app_scan,
                );
            }
        }
        let after_revision = *parts.settings_revision;
        let effect = match &result {
            Ok(_) => tessera_protocol::Effect::Applied,
            Err(reason) => tessera_protocol::Effect::Refused {
                reason: reason.clone(),
            },
        };
        journal_mutation_effect_and_broadcast(
            parts.journal,
            parts.ipc,
            ts,
            origin.clone(),
            tessera_protocol::JournalMutation::Settings {
                action,
                before_revision,
                after_revision,
            },
            effect,
        );
    }
    // The Control Center's Lock now row: lock immediately through
    // the same idle-process path as the Super+L binding.
    if parts.shell.take_lock() {
        parts.idle_process.lock_now();
    }
    // Unified eight-zone Hot Zone activation (ADR-0254).
    if let Some((slot, target)) = parts.shell.take_hot_zone_summon() {
        use tessera_desktop::hot_zone::HotZoneTarget;
        match target {
            HotZoneTarget::Dock => parts.shell.summon_dock(slot),
            HotZoneTarget::GlanceHud => {
                parts.shell.show_glance_hud();
                let _ = parts.status_refresh_tx.send(());
            }
            HotZoneTarget::Overview => parts.shell.toggle_overview(),
            HotZoneTarget::ControlCenter => {
                parts.shell.open_control_center();
                let _ = parts.status_refresh_tx.send(());
            }
            HotZoneTarget::Pivot => parts.shell.toggle_pivot(),
        }
    }
    // The dock's Pivot tile was clicked: toggle Pivot.
    if parts.shell.take_toggle_pivot() {
        parts.shell.toggle_pivot();
    }
    // Clipboard copy requests (e.g. from Pivot calculator)
    if let Some(text) = parts.shell.take_clipboard_copy() {
        log::info!("clipboard: copied expression from Pivot: {text}");
    }
    // Dispatched AI Agent prompts from Pivot
    if let Some(prompt) = parts.shell.take_agent_prompt() {
        log::info!("agent-broker: dispatched prompt from Pivot: {prompt}");
    }
    // Dock context-menu pin/unpin requests: apply the explicit,
    // idempotent action to dock state, persist to dock_state.json,
    // and refresh immediately.
    let pin_actions = parts.shell.take_dock_pin_actions();
    if !pin_actions.is_empty() {
        let mut pinned_list = parts.dock_state.pinned.clone();
        pinned_list = materialize_pins_for_manual_edit(
            parts.launcher_apps,
            &parts.icon_cache.map,
            &pinned_list,
            parts.dock_state.autopopulate,
        );
        pinned_list = apply_pin_actions(parts.launcher_apps, &pinned_list, &pin_actions);
        parts.dock_state.pinned = pinned_list.clone();
        parts.dock_state.autopopulate = false;
        if let Err(error) = parts.dock_state.save_to_path(parts.dock_state_path) {
            log::warn!("dock: pins not saved: {error}");
        }
        if let Some(c) = parts.config.as_mut() {
            c.dock.pinned = pinned_list.clone();
            c.dock.autopopulate = false;
        }
        let pinned = resolve_chrome_pins(
            parts.launcher_apps,
            &parts.icon_cache.map,
            &pinned_list,
            false,
        );
        parts
            .shell
            .set_app_catalog(tessera_shell::component::AppCatalog {
                apps: parts.launcher_apps.to_vec(),
                pinned,
                icons: parts.icon_cache.as_icon_set(),
                position: parts.dock_state.position,
            });
    }
    // A committed drag reorder carries the complete pinned list
    // in dock order (entry ids, already materialized from the
    // visible strip): persist it to dock_state.json. The dock
    // applied the order optimistically; this push reconciles it.
    if let Some(pinned_list) = parts.shell.take_dock_reorder() {
        parts.dock_state.pinned = pinned_list.clone();
        parts.dock_state.autopopulate = false;
        if let Err(error) = parts.dock_state.save_to_path(parts.dock_state_path) {
            log::warn!("dock: reordered pins not saved: {error}");
        }
        if let Some(c) = parts.config.as_mut() {
            c.dock.pinned = pinned_list.clone();
            c.dock.autopopulate = false;
        }
        let pinned = resolve_chrome_pins(
            parts.launcher_apps,
            &parts.icon_cache.map,
            &pinned_list,
            false,
        );
        parts
            .shell
            .set_app_catalog(tessera_shell::component::AppCatalog {
                apps: parts.launcher_apps.to_vec(),
                pinned,
                icons: parts.icon_cache.as_icon_set(),
                position: parts.dock_state.position,
            });
    }
    // A dock edge drag commits the edge it landed on: persist
    // it to dock_state.json and reconcile the catalog-carried position.
    // The dock already switched edges optimistically during the gesture.
    if let Some(position) = parts.shell.take_dock_position() {
        parts.dock_state.position = position;
        if let Err(error) = parts.dock_state.save_to_path(parts.dock_state_path) {
            log::warn!("dock: position not saved: {error}");
        }
        if let Some(c) = parts.config.as_mut() {
            c.dock.position = position;
        }
        let pinned = resolve_chrome_pins(
            parts.launcher_apps,
            &parts.icon_cache.map,
            &parts.dock_state.pinned,
            parts.dock_state.autopopulate,
        );
        parts
            .shell
            .set_app_catalog(tessera_shell::component::AppCatalog {
                apps: parts.launcher_apps.to_vec(),
                pinned,
                icons: parts.icon_cache.as_icon_set(),
                position,
            });
        let hot_zone_policy = parts
            .config
            .as_ref()
            .map(|c| c.hot_zones.resolve(position))
            .unwrap_or_else(|| {
                tessera_desktop::hot_zone::HotZonePolicy::with_dock_position(position)
            });
        parts.shell.set_hot_zones(hot_zone_policy);
    }
    // Launch the application the launcher's clicked row asked for.
    // The child is detached (setsid) and inherits the Wayland/XDG
    // environment, so it connects back to this compositor and
    // survives it exiting. See tessera-launch / ADR-0022.
    if let Some(entry) = parts.shell.take_spawn() {
        let opts = tessera_launch_services::LaunchOpts {
            wayland_display: Some(parts.server.socket().to_owned()),
            ..Default::default()
        };
        match tessera_launch_services::launch(&entry, &opts) {
            Ok(report) => {
                log::info!("launcher: spawned {} (pid {})", entry.id, report.pid)
            }
            Err(e) => log::warn!("launcher: failed to spawn {}: {e}", entry.id),
        }
    }
}
