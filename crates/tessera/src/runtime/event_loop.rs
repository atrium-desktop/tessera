use super::*;
use tessera_wayland::PresentedSurfaces;

const MAINTENANCE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const FALLBACK_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_667);
const MODEL_WALLPAPER_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_667);

impl CompositorRuntime<'_> {
    fn presentation_availability(&self) -> PresentationAvailability {
        if !self.host.is_active() {
            PresentationAvailability::BackendUnavailable
        } else if !self.host.outputs_powered() {
            PresentationAvailability::OutputsOff
        } else if !self.host.presentation_target_ready() {
            PresentationAvailability::TargetUnavailable
        } else {
            PresentationAvailability::Available
        }
    }

    fn reconcile_presentation_state(&mut self) {
        let now = std::time::Instant::now();
        let availability = self.presentation_availability();
        match self.presentation.set_availability(availability) {
            ActivationChange::None => {}
            ActivationChange::Suspended(reason) => {
                // Queued visual work belongs to the presentation epoch in
                // which it was accepted. Never replay chrome edges or a
                // screenshot across a target-availability boundary.
                self.refuse_suspended_frame();
                self.primary_plane_state.invalidate();
                // The hardware cursor plane is blanked when outputs go away;
                // drop the sprite baseline so the next commit reprograms it.
                self.damage.last_presented_cursor = None;
                self.damage.last_presented_cursor_position = None;
                self.damage.last_presented_cursor_hotspot = None;
                self.damage.last_presented_cursor_pixels = None;
                if reason == PresentationAvailability::BackendUnavailable {
                    self.invalidate_input_epoch();
                }
                self.previous_render_at = now;
            }
            ActivationChange::BackendEpochInvalidated => {
                // Presentation can already be suspended for a reason that
                // preserves input. Losing VT/session ownership afterwards
                // must still invalidate that older input epoch.
                self.refuse_suspended_frame();
                self.primary_plane_state.invalidate();
                self.damage.last_presented_cursor = None;
                self.damage.last_presented_cursor_position = None;
                self.damage.last_presented_cursor_hotspot = None;
                self.damage.last_presented_cursor_pixels = None;
                self.invalidate_input_epoch();
                self.previous_render_at = now;
            }
            ActivationChange::Resumed => {
                // VT resume replaces the DRM master fd, framebuffers, and
                // output topology. It may not reuse an earlier damage or
                // timing baseline. The hardware cursor plane was blanked at
                // suspend, so its presentation-side sprite baseline is void
                // and the first cursor commit must not be skipped.
                self.damage.force_full_redraw = true;
                self.damage.last_presented_cursor = None;
                self.damage.last_presented_cursor_position = None;
                self.damage.last_presented_cursor_hotspot = None;
                self.damage.last_presented_cursor_pixels = None;
                self.previous_render_at = now;
            }
        }
        if availability == PresentationAvailability::Available {
            self.presentation
                .reconcile_backend(self.host.presentation_pending());
            if let Some(elapsed) = self.presentation.take_stall_warning(now) {
                log::warn!(
                    "{}: presentation batch still owns scanout after {elapsed:?}; \
                     continuing event dispatch without submitting another frame",
                    self.host.name()
                );
            }
            if self.presentation.take_recovery_due(now) {
                log::error!(
                    "{}: page-flip event lost; reclaiming scanout ownership and forcing a full redraw",
                    self.host.name()
                );
                self.host.recover_lost_presentation();
                self.damage.force_full_redraw = true;
            }
            if self.presentation.tick(now) {
                if self.animating {
                    self.presentation.queue_redraw();
                    // An estimated-vblank boundary drives the client's draw
                    // clock, but no frame reached the display in this cycle.
                    // `wl_callback.done` pace clients; only a real presented
                    // frame may complete `wp_presentation_feedback`.
                    let _ = self.server.send_frame_callbacks(
                        self.start.elapsed().as_millis() as u32,
                        PresentedSurfaces::Nothing,
                    );
                } else {
                    let callbacks_sent = self.server.send_frame_callbacks(
                        self.start.elapsed().as_millis() as u32,
                        PresentedSurfaces::Nothing,
                    ) > 0;
                    let refresh_interval = self.presentation_interval();
                    self.presentation.callbacks_sent_at_estimated_vblank(
                        callbacks_sent,
                        now,
                        refresh_interval,
                    );
                }
            }
        }
    }

    fn invalidate_input_epoch(&mut self) {
        // A VT/session round-trip may omit release edges. Clear
        // compositor-side ownership and level state before a new backend
        // presentation epoch starts.
        self.input_acc.mouse_down.fill(false);
        self.keyboard_capture = Default::default();
        self.chrome_pointer_captured = false;
        self.synthetic_pointer_active = false;
    }

    fn refuse_suspended_frame(&mut self) {
        let Some(frame) = self.pending_frame.take() else {
            return;
        };
        self.refuse_unpresentable_frame(frame);
    }

    fn refuse_unpresentable_frame(&self, frame: FrameState) {
        for request in frame.pending_screenshots {
            let PendingScreenshot {
                command,
                ts_mono_ms,
                origin,
                ..
            } = request;
            journal_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                ts_mono_ms,
                origin,
                command,
                tessera_protocol::Effect::Refused {
                    reason: "presentation target became unavailable before capture".into(),
                },
            );
        }
    }

    fn idle_wait(&self) -> std::time::Duration {
        // A live output stream paces the loop at its negotiated max-fps
        // (ADR-0130): a due stream forces a presentation, so the loop wakes
        // at the stream's next frame deadline even on a static screen.
        // Window streams (ADR-0127) render independently of presentation:
        // the loop additionally wakes at a dirty window stream's max-fps
        // deadline. While a frame is already traversing the SHM readback
        // lane (readback bound, or the worker converting it), the capture
        // worker's completion eventfd is registered in the backend poll set
        // (the worker signals *before* sending through the channel), so the
        // completion itself wakes the loop — no poll quantum is needed and
        // none may be added: a floor here would re-introduce the 1 kHz
        // dispatch churn the eventfd was wired up to remove. Streams are
        // not served while the session is locked or the backend is
        // inactive, so they must not keep the loop awake then either.
        let stream_wait = if !self.server.session_locked() && self.host.is_active() {
            self.streams.next_stream_wake_in(std::time::Instant::now())
        } else {
            None
        };
        // Media wallpapers expose their next source frame directly. A 3D
        // wallpaper is intentionally capped at 60 Hz independently of shell
        // and client pacing; using the render timestamp retains that cadence
        // across an intervening DRM page flip.
        let Some(wallpaper) = self.wallpaper.as_ref() else {
            return stream_wait
                .unwrap_or(MAINTENANCE_INTERVAL)
                .min(MAINTENANCE_INTERVAL);
        };
        if self.server.session_locked() || self.space_use.client_owns_output() {
            return stream_wait
                .unwrap_or(MAINTENANCE_INTERVAL)
                .min(MAINTENANCE_INTERVAL);
        }
        let mut wait = wallpaper.next_frame_in().unwrap_or(MAINTENANCE_INTERVAL);
        if wallpaper.has_model() {
            wait = wait
                .min(MODEL_WALLPAPER_INTERVAL.saturating_sub(self.previous_render_at.elapsed()));
        }
        if let Some(stream_wait) = stream_wait {
            wait = wait.min(stream_wait);
        }
        wait.min(MAINTENANCE_INTERVAL)
    }

    fn wait_for_work(&mut self) -> bool {
        self.reconcile_presentation_state();
        let timeout = self
            .presentation
            .wait_timeout(self.idle_wait(), std::time::Instant::now());
        let alive = if timeout.is_zero() {
            self.host.dispatch_nonblocking()
        } else {
            self.host.dispatch_timeout(timeout)
        };
        self.reconcile_presentation_state();
        alive && !self.shell.should_quit() && !self.quit_requested
    }

    /// Refresh interval of the host's atomic presentation domain.
    ///
    /// Tessera currently commits every active CRTC as one ownership batch, so
    /// the slowest active output bounds when the whole batch retires. Nested
    /// mode has no authoritative output mode and uses a 60 Hz estimate.
    /// The result is cached against `outputs_revision` — this sits on the
    /// per-frame pacing path and a hotplug is the only event that can move
    /// it.
    fn presentation_interval(&mut self) -> std::time::Duration {
        let revision = self.server.outputs_revision();
        let (cached_revision, cached) = self.cached_presentation_interval;
        if cached_revision == revision {
            return cached;
        }
        let interval = presentation_domain_interval(self.server.output_infos().iter());
        self.cached_presentation_interval = (revision, interval);
        interval
    }

    fn update_animation_state(&mut self) {
        // Capture post-processing is background work with its own pollable
        // completion wakeup. Treating it as animation used to schedule full
        // compositor frames at the output refresh rate throughout PNG encode
        // and fsync, exactly when the machine was already under load.
        self.animating = self.shell.anim_pending() || self.server.transitions_pending();
    }

    fn queue_frame_state(&mut self, frame: FrameState) {
        if self.presentation_availability() != PresentationAvailability::Available {
            // The active session can still process input without a
            // presentation target, but visual edges from that interval must
            // not accumulate and replay after the target returns. Resume
            // forces a full redraw.
            self.refuse_unpresentable_frame(frame);
            return;
        }
        if let Some(pending) = self.pending_frame.as_mut() {
            pending.merge(frame);
        } else {
            self.pending_frame = Some(frame);
        }
        self.presentation.queue_redraw();
    }

    pub(super) fn run_loop(mut self) -> Result<(), Box<dyn std::error::Error>> {
        while self.wait_for_work() {
            let work = match self.prepare_iteration() {
                Ok(Some(work)) => work,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("compositor: iteration preparation error: {error}");
                    continue;
                }
            };
            let frame = match self.process_input(work) {
                Ok(frame) => frame,
                Err(error) => {
                    log::warn!("compositor: input processing error: {error}");
                    continue;
                }
            };
            // Pointer-motion fast path (ADR-0101): land the cursor on its
            // KMS plane out-of-band only while a previous primary-plane flip
            // is still in flight (`!self.presentation.can_redraw()`). When the
            // scheduler gate is open, `render_and_present` immediately below
            // executes either the `cursor_only_eligible` commit or a combined
            // primary+cursor atomic commit; issuing a separate NONBLOCK
            // cursor commit right before `render_and_present` would occupy the
            // CRTC's non-blocking commit slot and force the primary flip into
            // a spurious `EBUSY` retry.
            let is_cursor_fast_path = frame.cursor_fast_path;
            self.queue_frame_state(frame);
            if !self.presentation.can_redraw() {
                if is_cursor_fast_path && let Some(pending) = self.pending_frame.as_ref() {
                    let (session_locked, cursor_hidden, cursor_shape) = (
                        pending.session_locked,
                        pending.cursor_hidden,
                        pending.cursor_shape,
                    );
                    let _ =
                        self.present_pointer_cursor(session_locked, cursor_hidden, cursor_shape);
                }
                continue;
            }
            self.presentation.begin_redraw();
            let mut frame = self
                .pending_frame
                .take()
                .expect("queued redraw has accumulated frame state");
            let render_at = std::time::Instant::now();
            let frame_dt = (render_at - self.previous_render_at)
                .as_secs_f32()
                .clamp(0.0, 1.0 / 15.0);
            self.previous_render_at = render_at;
            frame.set_dt(frame_dt);

            let outcome = match self.render_and_present(frame) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log::warn!("compositor: presentation error: {error}");
                    self.damage.force_full_redraw = true;
                    PresentationOutcome::Retry
                }
            };
            self.update_animation_state();
            let now = std::time::Instant::now();
            let refresh_interval = self.presentation_interval();
            match outcome {
                PresentationOutcome::Submitted => {
                    let presentation_pending = self.host.presentation_pending();
                    let pacing_anchor =
                        submission_pacing_anchor(presentation_pending, render_at, now);
                    self.presentation.submitted(
                        presentation_pending,
                        self.animating,
                        pacing_anchor,
                        refresh_interval,
                    );
                }
                PresentationOutcome::NoDamage { callbacks_sent } => self.presentation.no_damage(
                    callbacks_sent,
                    self.animating,
                    now,
                    refresh_interval,
                ),
                PresentationOutcome::Retry => {
                    // `Busy` means KMS still owns the previous batch. The
                    // scheduler gates the retry on that batch retiring via
                    // `reconcile_backend`, so exactly one fresh render follows
                    // each real flip. The 1 ms floor only bounds a rejection
                    // that owns no flip (`Inactive`, `Reconfigured`); it is not
                    // a polling cadence.
                    self.presentation
                        .retry_at(now + std::time::Duration::from_millis(1))
                }
            }
        }

        log::info!(
            "tessera: {} session ended after {} frames",
            self.host.name(),
            self.frame_count
        );
        self.device.wait_idle();
        Ok(())
    }
}

fn presentation_domain_interval<'a>(
    outputs: impl IntoIterator<Item = &'a tessera_desktop::output::OutputInfo>,
) -> std::time::Duration {
    // One canonical derivation of *both* which outputs form the domain and the
    // period they present at, shared with the `wp_presentation` feedback that
    // clients pace against (`presentation_domain_period_ns`). Keeping a second
    // copy here is how the compositor's cadence and its advertised refresh
    // drifted apart — twice: once in the arithmetic (`min` vs `max`) and once
    // in the input set (virtual outputs included only here).
    // `FALLBACK_REFRESH_INTERVAL` is only the "no real rate" estimate for a
    // nested/headless domain that owns no vblank.
    tessera_desktop::output::presentation_domain_period_ns(outputs)
        .map(std::time::Duration::from_nanos)
        .unwrap_or(FALLBACK_REFRESH_INTERVAL)
}

fn submission_pacing_anchor(
    presentation_pending: bool,
    render_started_at: std::time::Instant,
    submitted_at: std::time::Instant,
) -> std::time::Instant {
    if presentation_pending {
        // The watchdog measures ownership after KMS accepted the batch.
        submitted_at
    } else {
        // Nested FIFO presentation may already have waited for the outer
        // compositor. Anchor estimated pacing to the frame start so that wait
        // is credited instead of charging a second refresh interval.
        render_started_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_desktop::output::{OutputGeometry, OutputInfo, OutputKind, OutputMode};

    fn info(connector: &str, refresh_mhz: u32, kind: OutputKind) -> OutputInfo {
        OutputInfo {
            connector: connector.to_owned(),
            geometry: OutputGeometry {
                mode: OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz,
                },
                ..Default::default()
            },
            kind,
            available_modes: Vec::new(),
            color_caps: Default::default(),
        }
    }

    #[test]
    fn atomic_domain_is_paced_by_its_slowest_crtc() {
        let outputs = [
            info("DP-1", 120_000, OutputKind::External),
            info("DP-2", 60_000, OutputKind::External),
        ];
        assert_eq!(
            presentation_domain_interval(&outputs),
            std::time::Duration::from_nanos(16_666_666)
        );
    }

    #[test]
    fn a_slow_virtual_output_never_paces_the_physical_domain() {
        // The compositor's own cadence and the advertised `wp_presentation`
        // refresh must agree, so both exclude virtual outputs. A 24 Hz virtual
        // capture target on a 60 Hz panel presents at 60 Hz, not 24 Hz.
        let outputs = [
            info("eDP-1", 60_000, OutputKind::Internal),
            info("VIRTUAL-1", 24_000, OutputKind::Virtual),
        ];
        assert_eq!(
            presentation_domain_interval(&outputs),
            std::time::Duration::from_nanos(16_666_666)
        );
    }

    #[test]
    fn nested_domain_uses_a_sixty_hertz_estimate() {
        let outputs = [info("nested", 0, OutputKind::External)];
        assert_eq!(
            presentation_domain_interval(&outputs),
            FALLBACK_REFRESH_INTERVAL
        );
        // A headless desktop driving only virtual outputs owns no vblank.
        let virtual_only = [info("VIRTUAL-1", 60_000, OutputKind::Virtual)];
        assert_eq!(
            presentation_domain_interval(&virtual_only),
            FALLBACK_REFRESH_INTERVAL
        );
    }

    #[test]
    fn nested_fifo_wait_counts_toward_the_estimated_interval() {
        let render_started_at = std::time::Instant::now();
        let submitted_at = render_started_at + FALLBACK_REFRESH_INTERVAL;
        assert_eq!(
            submission_pacing_anchor(false, render_started_at, submitted_at),
            render_started_at
        );
        assert_eq!(
            submission_pacing_anchor(true, render_started_at, submitted_at),
            submitted_at
        );
    }
}
