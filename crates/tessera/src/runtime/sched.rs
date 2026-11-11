//! Scheduling and realtime integration for the compositor frame loop.
//!
//! The frame loop runs on the main thread of the compositor. If that thread
//! shares a runqueue with a CPU-saturating workload (a `cargo build`, a
//! desktop-app scan), the frame cadence collapses and the whole desktop
//! stutters. This module is the glue that turns the `[compositor]` config into
//! a [`tessera_sched::SchedPlan`], applies it to the frame thread at the exact
//! main-thread boundary, arms a [`tessera_sched::DeadlineWatchdog`] so a
//! starved loop is observable, and tags the background threads that feed the
//! frame loop.

/// Resolve the scheduling knobs from the `[compositor]` config into a
/// [`tessera_sched::SchedPlan`].
///
/// `compositor.realtime` is an opt-in master switch: when off (or when running
/// a nested development session inside an outer Wayland compositor), the frame
/// loop keeps the process's inherited scheduling class and multi-core CPU mask
/// so it never pins itself onto the single dedicated CPU core owned at
/// `SCHED_FIFO` by the outer host compositor. When on for a bare-metal DRM
/// session, the plan pins and elevates the frame thread. The plan is computed
/// **once at startup** so the frame/feeder/background split stays coherent
/// across worker restarts.
pub(super) fn plan_from_config(
    config: Option<&tessera_config::Config>,
    nested: bool,
) -> tessera_sched::SchedPlan {
    let cfg = config.map(|c| c.compositor.clone()).unwrap_or_default();
    if nested || !cfg.realtime {
        // Realtime off or nested preview session: report a plan that does not
        // pin/elevate, leaving scheduling and affinity untouched.
        return tessera_sched::SchedPlan::disabled();
    }
    let s = &cfg.scheduling;
    let pin_frame = s.pin_frame;
    let rt_priority = s.rt_priority as i32;
    let frame_nice = s.frame_nice;
    let mut plan = tessera_sched::SchedPlan::detect(pin_frame, rt_priority, frame_nice);
    // A user-specified pin CPU takes precedence over the auto-detected lowest
    // online CPU.
    if let Some(cpu) = s.pin_cpu {
        plan = plan.with_frame_pin(cpu);
    }
    plan
}

/// Arm and return a deadline watchdog for the frame loop. `watchdog_ms == 0`
/// disables the watchdog (returns `None`).
pub(super) fn watchdog_from_config(
    config: Option<&tessera_config::Config>,
) -> Option<tessera_sched::DeadlineWatchdog> {
    let cfg = config.map(|c| c.compositor.clone()).unwrap_or_default();
    if !cfg.realtime || cfg.scheduling.watchdog_ms == 0 {
        return None;
    }
    Some(tessera_sched::DeadlineWatchdog::new(
        std::time::Duration::from_millis(cfg.scheduling.watchdog_ms),
    ))
}

/// Apply the frame-loop realtime/pin schedule to the **calling** thread. Must
/// be called once at the top of `run_loop` so the frame thread is configured
/// before the first frame.
///
/// When the config has realtime disabled this is a no-op.
pub(super) fn arm_frame_thread(plan: &tessera_sched::SchedPlan) {
    if plan.realtime_enabled() {
        tessera_sched::make_current_frame_thread(plan);
    }
}

/// Lock the compositor's memory so background-IO pressure (and hence swap
/// latency) cannot push the frame path out to disk. Best-effort; needs
/// `CAP_IPC_LOCK` granted by the shipped systemd unit.
pub(super) fn lockdown_frame_memory() {
    tessera_sched::lockdown_memory();
}
