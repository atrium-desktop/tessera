//! The redraw/presentation lifecycle state machine for one presentation
//! domain (ADR-0077/0101).
//!
//! Tessera submits one atomic KMS batch spanning every active CRTC, so the
//! domain is the host rather than an individual connector. Owning this
//! machine here makes the ownership rule explicit: input and Wayland
//! traffic may continue while a commit is in flight, but a second render
//! cannot start until the backend retires that batch. The composition root
//! drives the transitions; the state, the watchdogs, and the wait-timeout
//! policy live here.

use std::time::{Duration, Instant};

const PRESENTATION_WATCHDOG: Duration = Duration::from_secs(1);

/// What will make a retried render attempt possible.
///
/// The two cases are not interchangeable. A DRM flip rejection is owned by
/// the backend: the previous atomic batch retires and its page-flip event
/// wakes the loop, so the scheduler must wait for that event (bounded by the
/// watchdog). A renderer acquire timeout owns no event: the presentation
/// engine is simply holding every swapchain image, and the only useful retry
/// is at the next presentation boundary. Treating the latter as a backend
/// event either spins at millisecond cadence or sleeps for the full watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryWait {
    /// The backend will wake the loop when its in-flight batch retires.
    BackendEvent,
    /// No event will arrive; retry at `not_before`.
    Deadline,
}

/// The outcome of one render transaction, as reported back to the
/// composition root's iteration loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationOutcome {
    Submitted,
    NoDamage { callbacks_sent: bool },
    Retry { wait: RetryWait },
}

/// Redraw lifecycle for one presentation domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationState {
    /// No visible work is known.
    Idle,
    /// A redraw may start immediately.
    Queued,
    /// The runtime has consumed a queued redraw and is synchronously building
    /// its outcome. `estimated_deadline` preserves a no-damage callback cycle
    /// when real damage queued an early assessment inside that cycle.
    Rendering { estimated_deadline: Option<Instant> },
    /// A frame is owned by the presentation backend until vblank.
    WaitingForVblank {
        /// Visible state changed while the submitted frame was in flight.
        redraw_queued: bool,
        submitted_at: Instant,
        stall_reported: bool,
    },
    /// No backend vblank will arrive. When `paced` is `true` (entered after a
    /// nested frame submission or an active animation cycle), input and damage
    /// coalesce into `redraw_queued` until `deadline` expires, matching DRM's
    /// `WaitingForVblank` discipline instead of rendering at pointer-polling
    /// cadence. When `paced` is `false` (entered after a no-damage frame
    /// callback cycle), `redraw_queued` remains immediately renderable while
    /// `frame_callbacks_allowed` stays closed until `deadline`.
    WaitingForEstimatedVblank {
        deadline: Instant,
        redraw_queued: bool,
        paced: bool,
    },
    /// A transient rejection or an acquire timeout. `wait` decides how the
    /// state leaves this arm: `BackendEvent` renders when the owning atomic
    /// batch retires (bounded by `not_before` and the watchdog), while
    /// `Deadline` renders at `not_before` and never claims an event that does
    /// not exist. The distinction is what keeps a nested swapchain that is
    /// temporarily out of images from spinning or sleeping for a second.
    Retrying {
        not_before: Instant,
        wait: RetryWait,
    },
    /// Presentation is unavailable, with the reason retained so losing the
    /// backend after output power-off still invalidates the input epoch.
    Suspended(PresentationAvailability),
}

/// State-machine facade used by the compositor loop.
#[derive(Debug)]
pub struct PresentationScheduler {
    state: PresentationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationAvailability {
    Available,
    /// Scanout is deliberately dark, but the active input epoch remains valid
    /// so physical activity can wake a locked session.
    OutputsOff,
    /// No renderable output target currently exists, but the backend and
    /// active input epoch remain valid (for example, all connectors unplugged).
    TargetUnavailable,
    /// The backend/device epoch is gone (VT loss, seat revoke, or backend
    /// failure).
    BackendUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationChange {
    None,
    Suspended(PresentationAvailability),
    /// The backend epoch changed after presentation was already suspended for
    /// another reason, so input ownership still needs invalidation.
    BackendEpochInvalidated,
    Resumed,
}

impl Default for PresentationScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PresentationScheduler {
    pub fn new() -> Self {
        Self {
            // The first frame is compositor-owned work and must not wait for
            // an input event to make the desktop visible.
            state: PresentationState::Queued,
        }
    }

    pub fn set_availability(&mut self, availability: PresentationAvailability) -> ActivationChange {
        if availability == PresentationAvailability::Available {
            if matches!(self.state, PresentationState::Suspended(_)) {
                self.state = PresentationState::Queued;
                return ActivationChange::Resumed;
            }
            return ActivationChange::None;
        }

        if let PresentationState::Suspended(previous) = self.state {
            self.state = PresentationState::Suspended(availability);
            if previous != PresentationAvailability::BackendUnavailable
                && availability == PresentationAvailability::BackendUnavailable
            {
                return ActivationChange::BackendEpochInvalidated;
            }
            return ActivationChange::None;
        }

        self.state = PresentationState::Suspended(availability);
        ActivationChange::Suspended(availability)
    }

    pub fn reconcile_backend(&mut self, presentation_pending: bool) {
        if presentation_pending {
            return;
        }
        self.state = match self.state {
            PresentationState::WaitingForVblank { redraw_queued, .. } => {
                if redraw_queued {
                    PresentationState::Queued
                } else {
                    PresentationState::Idle
                }
            }
            // A rejected commit (EBUSY) was waiting for the batch that owned
            // scanout to retire. That batch just retired, so re-render now:
            // this is the event that replaces the old 1 ms polling retry, which
            // re-composited the whole frame ~15 times per vblank only for each
            // copy to be rejected again. A deadline retry deliberately ignores
            // this transition: no backend event exists for an acquire timeout,
            // and promoting it here would spin until an image happens to free.
            PresentationState::Retrying {
                not_before,
                wait: RetryWait::Deadline,
            } => PresentationState::Retrying {
                not_before,
                wait: RetryWait::Deadline,
            },
            PresentationState::Retrying {
                wait: RetryWait::BackendEvent,
                ..
            } => PresentationState::Queued,
            other => other,
        };
    }

    /// Advance an estimated-vblank timer.
    ///
    /// Returns `true` only when the boundary elapsed without a redraw waiting.
    /// The runtime can then complete callbacks directly instead of performing
    /// a no-damage render assessment solely to reopen the callback cycle.
    pub fn tick(&mut self, now: Instant) -> bool {
        if let PresentationState::Retrying { not_before, .. } = self.state
            && now >= not_before
        {
            // A parked retry promotes to a fresh render; this is not an
            // estimated-vblank callback boundary.
            self.state = PresentationState::Queued;
            return false;
        }
        if let PresentationState::WaitingForEstimatedVblank {
            deadline,
            redraw_queued,
            ..
        } = self.state
            && now >= deadline
        {
            self.state = if redraw_queued {
                PresentationState::Queued
            } else {
                PresentationState::Idle
            };
            return !redraw_queued;
        }
        false
    }

    pub fn queue_redraw(&mut self) {
        self.state = match self.state {
            PresentationState::Idle => PresentationState::Queued,
            PresentationState::Queued => PresentationState::Queued,
            PresentationState::Rendering { .. } => {
                panic!("redraw queued during a synchronous render transaction")
            }
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported,
                ..
            } => PresentationState::WaitingForVblank {
                redraw_queued: true,
                submitted_at,
                stall_reported,
            },
            PresentationState::WaitingForEstimatedVblank {
                deadline, paced, ..
            } => PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued: true,
                paced,
            },
            // A parked retry already implies a redraw; fresh damage simply
            // waits for the same deadline.
            PresentationState::Retrying { not_before, wait } => {
                PresentationState::Retrying { not_before, wait }
            }
            PresentationState::Suspended(availability) => {
                PresentationState::Suspended(availability)
            }
        };
    }

    pub fn can_redraw(&self) -> bool {
        matches!(
            self.state,
            PresentationState::Queued
                | PresentationState::WaitingForEstimatedVblank {
                    redraw_queued: true,
                    paced: false,
                    ..
                }
        )
    }

    /// No-damage callbacks are allowed once per estimated refresh cycle.
    pub fn frame_callbacks_allowed(&self) -> bool {
        !matches!(
            self.state,
            PresentationState::WaitingForEstimatedVblank { .. }
                | PresentationState::Rendering {
                    estimated_deadline: Some(_)
                }
        )
    }

    /// Consume the queued edge and enter the synchronous render transaction.
    pub fn begin_redraw(&mut self) {
        self.state = match self.state {
            PresentationState::Queued => PresentationState::Rendering {
                estimated_deadline: None,
            },
            PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued: true,
                paced: false,
            } => PresentationState::Rendering {
                estimated_deadline: Some(deadline),
            },
            _ => panic!("redraw began outside a queued presentation state"),
        };
    }

    pub fn submitted(
        &mut self,
        presentation_pending: bool,
        redraw_after_present: bool,
        pacing_anchor: Instant,
        refresh_interval: Duration,
    ) {
        assert!(
            matches!(self.state, PresentationState::Rendering { .. }),
            "submission completed outside a render transaction"
        );
        self.state = if presentation_pending {
            PresentationState::WaitingForVblank {
                redraw_queued: redraw_after_present,
                submitted_at: pacing_anchor,
                stall_reported: false,
            }
        } else {
            PresentationState::WaitingForEstimatedVblank {
                deadline: pacing_anchor + refresh_interval,
                redraw_queued: redraw_after_present,
                paced: true,
            }
        };
    }

    pub fn no_damage(
        &mut self,
        callbacks_sent: bool,
        redraw_after_cycle: bool,
        now: Instant,
        refresh_interval: Duration,
    ) {
        let PresentationState::Rendering { estimated_deadline } = self.state else {
            panic!("no-damage outcome completed outside a render transaction");
        };
        if let Some(deadline) = estimated_deadline {
            // A redraw queued during the closed callback cycle found no new
            // output damage. Keep the original deadline; replacing it would
            // let a frame-only client postpone the boundary forever.
            self.state = PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued: redraw_after_cycle,
                paced: redraw_after_cycle,
            };
        } else if redraw_after_cycle {
            self.state = PresentationState::WaitingForEstimatedVblank {
                deadline: now + refresh_interval,
                redraw_queued: true,
                paced: true,
            };
        } else if callbacks_sent {
            self.state = Self::estimated_callback_cycle(now, refresh_interval);
        } else {
            self.state = PresentationState::Idle;
        }
    }

    /// Close a callback cycle that was completed directly at an estimated
    /// boundary, without entering the renderer.
    pub fn callbacks_sent_at_estimated_vblank(
        &mut self,
        callbacks_sent: bool,
        now: Instant,
        refresh_interval: Duration,
    ) {
        assert!(
            matches!(self.state, PresentationState::Idle),
            "estimated-vblank callback completed outside the idle boundary"
        );
        if callbacks_sent {
            self.state = Self::estimated_callback_cycle(now, refresh_interval);
        }
    }

    /// How long a vblank wait may outlive its stall warning before the flip
    /// event is declared lost and scanout ownership is reclaimed.
    const RECOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    pub fn retry_at(&mut self, not_before: Instant, wait: RetryWait) {
        assert!(
            matches!(self.state, PresentationState::Rendering { .. }),
            "retry completed outside a render transaction"
        );
        self.state = PresentationState::Retrying { not_before, wait };
    }

    /// Report a page flip that exceeded the ownership watchdog once per
    /// submission. The state remains blocked at this tier: a late flip is
    /// not permission to reuse a scanout buffer that KMS may still own.
    /// `take_recovery_due` provides the second tier for a flip whose event
    /// never arrives at all.
    pub fn take_stall_warning(&mut self, now: Instant) -> Option<Duration> {
        let PresentationState::WaitingForVblank {
            submitted_at,
            stall_reported,
            ..
        } = &mut self.state
        else {
            return None;
        };
        let elapsed = now.saturating_duration_since(*submitted_at);
        if !*stall_reported && elapsed >= PRESENTATION_WATCHDOG {
            *stall_reported = true;
            Some(elapsed)
        } else {
            None
        }
    }

    /// Second watchdog tier: a flip event still missing
    /// `RECOVERY_TIMEOUT` after submission is treated as lost. Remaining
    /// blocked would freeze presentation forever, so the caller reclaims
    /// backend ownership and forces a full redraw. If KMS genuinely still
    /// owns the batch, the next commit comes back EBUSY and the paced retry
    /// keeps the loop alive instead. Fires once per submission.
    pub fn take_recovery_due(&mut self, now: Instant) -> bool {
        let due = matches!(
            self.state,
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported: true,
                ..
            } if now.saturating_duration_since(submitted_at) >= Self::RECOVERY_TIMEOUT
        );
        if due {
            self.state = PresentationState::Queued;
        }
        due
    }

    pub fn wait_timeout(&self, idle_timeout: Duration, now: Instant) -> Duration {
        match self.state {
            PresentationState::Queued
            | PresentationState::WaitingForEstimatedVblank {
                redraw_queued: true,
                paced: false,
                ..
            } => Duration::ZERO,
            PresentationState::Rendering { .. } => {
                panic!("event wait requested during a render transaction")
            }
            PresentationState::Retrying { not_before, wait } => match wait {
                // The retry is gated on the owning batch retiring, and the
                // card fd is in the backend's poll set, so the flip event
                // itself wakes the loop. Block up to the watchdog the way
                // `WaitingForVblank` does instead of polling every
                // millisecond: that poll is what used to drive a full
                // re-render per tick. `not_before` still bounds a rejection
                // that owns no flip (`Inactive`, `Reconfigured`, a lost event).
                RetryWait::BackendEvent => {
                    let until_floor = not_before.saturating_duration_since(now);
                    until_floor.max(PRESENTATION_WATCHDOG).min(idle_timeout)
                }
                // A deadline retry has no event source. Sleep exactly until
                // its boundary: a nested swapchain that is briefly out of
                // images must retry at the next presentation, not spin and not
                // sleep for the backend watchdog.
                RetryWait::Deadline => not_before.saturating_duration_since(now).min(idle_timeout),
            },
            PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued,
                paced,
            } => {
                if paced && !redraw_queued {
                    idle_timeout
                } else {
                    deadline.saturating_duration_since(now).min(idle_timeout)
                }
            }
            // Visual timers cannot make progress while KMS owns a submitted
            // batch. Ignoring them prevents expired wallpaper timers from
            // turning the vblank wait into a busy poll.
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported,
                ..
            } => {
                if stall_reported {
                    PRESENTATION_WATCHDOG
                } else {
                    (submitted_at + PRESENTATION_WATCHDOG).saturating_duration_since(now)
                }
            }
            PresentationState::Idle => idle_timeout,
            // The seat or output is unavailable, so visual timers are also
            // irrelevant. Backend fds still wake this bounded maintenance wait.
            PresentationState::Suspended(_) => PRESENTATION_WATCHDOG,
        }
    }

    fn estimated_callback_cycle(now: Instant, refresh_interval: Duration) -> PresentationState {
        PresentationState::WaitingForEstimatedVblank {
            deadline: now + refresh_interval,
            redraw_queued: false,
            paced: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn damage_coalesces_while_waiting_for_vblank() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);
        assert!(!state.can_redraw());

        state.queue_redraw();
        assert!(!state.can_redraw());
        state.reconcile_backend(false);
        assert!(state.can_redraw());
    }

    #[test]
    fn completed_frame_without_new_damage_returns_idle() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);
        state.reconcile_backend(false);
        assert!(!state.can_redraw());
        assert_eq!(
            state.wait_timeout(Duration::from_secs(1), now),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn no_damage_callback_closes_cycle_until_estimated_vblank() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        assert!(!state.frame_callbacks_allowed());

        state.queue_redraw();
        assert!(state.can_redraw());
        state.begin_redraw();
        state.no_damage(false, false, now + Duration::from_millis(1), FRAME);
        assert!(!state.can_redraw());

        assert!(state.tick(now + FRAME));
        assert!(!state.can_redraw());
        assert!(state.frame_callbacks_allowed());
    }

    #[test]
    fn nested_submission_coalesces_input_until_estimated_boundary() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(false, true, now, FRAME);
        assert!(!state.can_redraw());
        assert_eq!(state.wait_timeout(Duration::from_secs(1), now), FRAME);

        // Input arriving mid-frame coalesces rather than triggering an
        // unpaced sub-millisecond render before the refresh boundary.
        state.queue_redraw();
        assert!(!state.can_redraw());
        assert_eq!(state.wait_timeout(Duration::from_secs(1), now), FRAME);

        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(!state.can_redraw());
        assert!(!state.tick(now + FRAME));
        assert!(state.can_redraw());
    }

    #[test]
    fn nested_static_submission_sleeps_idle_unless_damage_arrives() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(false, false, now, FRAME);
        assert!(!state.can_redraw());
        assert_eq!(
            state.wait_timeout(Duration::from_secs(1), now),
            Duration::from_secs(1)
        );

        // Input arriving inside the same refresh period arms the deadline wait.
        state.queue_redraw();
        assert!(!state.can_redraw());
        assert_eq!(state.wait_timeout(Duration::from_secs(1), now), FRAME);
        assert!(!state.tick(now + FRAME));
        assert!(state.can_redraw());
    }

    #[test]
    fn suspend_blocks_and_resume_forces_one_redraw() {
        let mut state = PresentationScheduler::new();
        assert_eq!(
            state.set_availability(PresentationAvailability::OutputsOff),
            ActivationChange::Suspended(PresentationAvailability::OutputsOff)
        );
        assert_eq!(
            state.set_availability(PresentationAvailability::OutputsOff),
            ActivationChange::None
        );
        state.queue_redraw();
        assert!(!state.can_redraw());
        assert_eq!(
            state.set_availability(PresentationAvailability::Available),
            ActivationChange::Resumed
        );
        assert_eq!(
            state.set_availability(PresentationAvailability::Available),
            ActivationChange::None
        );
        assert!(state.can_redraw());
    }

    #[test]
    fn backend_loss_after_input_preserving_suspension_invalidates_the_epoch() {
        for reason in [
            PresentationAvailability::OutputsOff,
            PresentationAvailability::TargetUnavailable,
        ] {
            let mut state = PresentationScheduler::new();
            state.set_availability(reason);
            assert_eq!(
                state.set_availability(PresentationAvailability::BackendUnavailable),
                ActivationChange::BackendEpochInvalidated
            );
            assert_eq!(
                state.set_availability(PresentationAvailability::BackendUnavailable),
                ActivationChange::None
            );
        }
    }

    #[test]
    fn estimated_boundary_promotes_only_an_accumulated_redraw() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        state.queue_redraw();

        // Queued real work may render before the estimated boundary; if the
        // loop has not consumed it yet, the boundary keeps it queued.
        assert!(state.can_redraw());
        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(state.can_redraw());
        assert!(!state.tick(now + FRAME));
        assert!(state.can_redraw());
    }

    #[test]
    fn callback_sent_at_estimated_boundary_starts_the_next_cycle() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        assert!(state.tick(now + FRAME));

        state.callbacks_sent_at_estimated_vblank(true, now + FRAME, FRAME);
        assert!(!state.frame_callbacks_allowed());
        assert_eq!(
            state.wait_timeout(Duration::from_secs(1), now + FRAME),
            FRAME
        );
    }

    #[test]
    fn a_transient_rejection_blocks_on_the_flip_event_not_a_poll() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.retry_at(now + FRAME, RetryWait::BackendEvent);

        // A parked retry must not spin. The wait is far longer than the retry
        // floor because the loop blocks on the backend's flip event (the card
        // fd is in the poll set); polling every millisecond is what drove a
        // full re-composite per tick, each one rejected again.
        assert!(!state.can_redraw());
        let wait = state.wait_timeout(Duration::from_secs(30), now);
        assert!(
            wait >= PRESENTATION_WATCHDOG,
            "a parked retry blocks on the flip event, got {wait:?}"
        );

        // Fresh damage is absorbed by the parked retry rather than opening a
        // second render transaction.
        state.queue_redraw();
        assert!(!state.can_redraw());

        // The owning batch retiring is what re-opens the gate — one fresh
        // render per real vblank, not one per poll.
        state.reconcile_backend(false);
        assert!(state.can_redraw());
        state.begin_redraw();
    }

    #[test]
    fn a_parked_retry_waits_until_the_owning_batch_retires() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.retry_at(now + FRAME, RetryWait::BackendEvent);

        // Still owned: nothing to re-render yet.
        state.reconcile_backend(true);
        assert!(!state.can_redraw());

        // The floor alone also makes progress when the rejection owned no flip
        // at all (`Inactive`, `Reconfigured`, a lost event).
        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(!state.can_redraw());
        state.tick(now + FRAME);
        assert!(state.can_redraw());
    }

    #[test]
    fn a_deadline_retry_sleeps_until_its_boundary_and_ignores_backend_readiness() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.retry_at(now + FRAME, RetryWait::Deadline);

        // No backend event owns this retry. `reconcile_backend(false)` must not
        // promote it: the nested backend always reports no in-flight batch,
        // and promoting would turn an acquire timeout into a millisecond spin.
        state.reconcile_backend(false);
        assert!(!state.can_redraw());
        assert_eq!(state.wait_timeout(Duration::from_secs(30), now), FRAME);

        // The deadline is the only thing that reopens the gate.
        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(!state.can_redraw());
        state.tick(now + FRAME);
        assert!(state.can_redraw());
    }

    #[test]
    #[should_panic(expected = "redraw began outside a queued presentation state")]
    fn a_render_transaction_cannot_begin_twice() {
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.begin_redraw();
    }

    #[test]
    fn real_vblank_wait_ignores_an_expired_visual_timer() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        assert_eq!(
            state.wait_timeout(Duration::ZERO, now),
            PRESENTATION_WATCHDOG
        );
    }

    #[test]
    fn stalled_flip_warns_once_without_releasing_ownership() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        assert_eq!(
            state.take_stall_warning(now + PRESENTATION_WATCHDOG),
            Some(PRESENTATION_WATCHDOG)
        );
        assert_eq!(
            state.take_stall_warning(now + PRESENTATION_WATCHDOG * 2),
            None
        );
        assert!(!state.can_redraw());
        assert_eq!(
            state.wait_timeout(Duration::ZERO, now + PRESENTATION_WATCHDOG),
            PRESENTATION_WATCHDOG
        );
    }

    #[test]
    fn a_lost_flip_event_recovers_instead_of_freezing() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        // First tier warns once and stays blocked.
        assert!(
            state
                .take_stall_warning(now + PRESENTATION_WATCHDOG)
                .is_some()
        );
        assert!(!state.can_redraw());
        assert!(!state.take_recovery_due(now + PRESENTATION_WATCHDOG));

        // Second tier reclaims ownership and requeues exactly once.
        assert!(state.take_recovery_due(now + PresentationScheduler::RECOVERY_TIMEOUT));
        assert!(state.can_redraw());
        assert!(!state.take_recovery_due(now + PresentationScheduler::RECOVERY_TIMEOUT));
    }

    #[test]
    fn suspended_state_ignores_visual_timers() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.set_availability(PresentationAvailability::BackendUnavailable);
        assert_eq!(
            state.wait_timeout(Duration::ZERO, now),
            PRESENTATION_WATCHDOG
        );
    }
}
