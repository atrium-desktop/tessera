//! # tessera-sched
//!
//! Realtime scheduling and CPU-pinning for the compositor frame loop.
//!
//! A Wayland compositor renders every frame on a single event-loop thread. If
//! that thread shares a runqueue with a CPU-saturating workload — a `cargo
//! build`, a desktop-app scan, an icon decode storm — the frame cadence
//! collapses and the *entire desktop* (cursor, windows, shell chrome) stutters.
//! Classical CPU scheduling favors the greedy worker, not the latency-sensitive
//! compositor, so well-designed display servers deliberately elevate the
//! frame thread above the crowd (KWin/Sway do this with `SCHED_FIFO`/nice and
//! CPU pinning).
//!
//! This crate encodes that contract precisely and defensively:
//!
//! 1. **Priority raise** — raise the frame thread to `SCHED_FIFO` at a bounded
//!    realtime priority, falling back to `SCHED_RR`, then to negative `nice`
//!    as privileges fade. Every failure is *logged, not fatal* (a compositor
//!    must start even unprivileged).
//! 2. **CPU pinning** — pin the frame loop to a dedicated logical CPU (or a
//!    pinned mask) and stage every background thread onto the remaining
//!    cores, so a compile storm's worker threads cannot steal the render core.
//! 3. **Lockdown** — `mlockall` the process so background-IO ballooning cannot
//!    push the frame path out to swap.
//!
//! All of it is Linux-only and degrades to a clean no-op elsewhere so the
//! workspace stays portable and unit-testable.
//!
//! ## Thread roles
//!
//! Threads that feed the frame loop get priority; pure-background work is
//! *de-prioritized* so it never competes with presentation. The taxonomy:
//!
//! - [`ThreadRole::Frame`] — the compositor's own event/render loop (must be
//!   elevated above every other process on the system).
//! - [`ThreadRole::FrameFeeder`] — threads whose output is consumed *by every
//!   frame* (capture readback/encode, wallpaper-video decode). Elevated, but
//!   below the frame thread.
//! - [`ThreadRole::Background`] — extension/scan/dbus work (app catalog scan,
//!   bluetooth, casting, tray, status probes). **De-prioritized** to keep it
//!   off the frame thread's pinned CPU.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::sync::OnceLock;

/// The process-wide scheduling plan, armed once at compositor startup by
/// [`arm_global_plan`]. Threads spawned anywhere in the process call
/// [`mark_background`] / [`mark_frame_feeder`] without threading a plan
/// through every spawn site; they read this global.
static GLOBAL_PLAN: OnceLock<SchedPlan> = OnceLock::new();

/// Install the process-wide scheduling plan. Called exactly once at
/// compositor startup, before any worker thread is spawned. Threads created
/// later consult it via the role helpers.
pub fn arm_global_plan(plan: SchedPlan) {
    let _ = GLOBAL_PLAN.set(plan);
}

/// The installed global plan, falling back to a disabled default when
/// nothing was armed (unit tests, non-compositor entry points).
fn global_plan() -> &'static SchedPlan {
    GLOBAL_PLAN.get_or_init(SchedPlan::disabled)
}

/// Which part of the compositor a thread serves. Priority is staged so the
/// frame loop always wins the runqueue, its feeders come second, and
/// background work is pushed to the leftover cores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadRole {
    /// The compositor's single event/render/present loop. Highest priority;
    /// pinned to a dedicated CPU.
    Frame,
    /// A thread whose output the frame loop consumes (capture encode,
    /// wallpaper-video decode). Elevated below [`ThreadRole::Frame`].
    FrameFeeder,
    /// Background extension/scan/dbus work. De-prioritized and kept off the
    /// frame thread's pinned CPU.
    Background,
}

/// The concrete scheduling policy applied to a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedPolicy {
    /// `SCHED_FIFO` — realtime, no round-robin. Used for the frame thread.
    Fifo,
    /// `SCHED_RR` — realtime with round-robin. Fallback for the frame thread.
    RoundRobin,
    /// Ordinary `SCHED_OTHER` with an adjusted nice value. Final fallback.
    Other,
}

/// A per-thread CPU pin (logical CPU indices), reserved for a role.
#[derive(Debug, Clone)]
pub struct Pin {
    /// Logical CPU indices allowed for this thread, in ascending order.
    cpus: Vec<usize>,
}

impl Pin {
    fn new(cpus: Vec<usize>) -> Option<Self> {
        (!cpus.is_empty()).then_some(Self { cpus })
    }

    /// Logical CPUs this pin allows.
    pub fn cpus(&self) -> &[usize] {
        &self.cpus
    }
}

/// A parsed, validated scheduling plan for the process.
///
/// The plan is computed once at startup and then applied to each thread as it
/// is (re)configured, keeping the frame/feeder/background split coherent even
/// across worker restarts.
#[derive(Debug, Clone)]
pub struct SchedPlan {
    /// Whether realtime scheduling and pinning are enabled.
    enabled: bool,
    /// Dedicated logical CPU for the frame loop, when pinning is enabled.
    /// `None` means "leave the frame thread on whatever runqueue".
    frame_cpu: Option<usize>,
    /// Full set of logical CPUs the frame thread may run on (>= `frame_cpu`).
    frame_mask: Option<Vec<usize>>,
    /// All logical CPUs available to the process.
    available: Option<Vec<usize>>,
    /// Realtime priority requested for the frame thread (SCHED_FIFO/RR level).
    rt_priority: i32,
    /// Fallback nice for the frame thread when realtime is not permitted.
    frame_nice: i32,
    /// Fan-out of the remaining CPUs for feeder/background threads.
    rest: Vec<usize>,
}

impl SchedPlan {
    /// An inert plan where scheduling and pinning are explicitly disabled.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            frame_cpu: None,
            frame_mask: None,
            available: None,
            rt_priority: 0,
            frame_nice: 0,
            rest: Vec::new(),
        }
    }

    /// Build an active plan from the host's online CPU topology.
    ///
    /// - `pin_frame` pins the frame thread to a single dedicated logical CPU.
    /// - `rt_priority` is the SCHED_FIFO/RR level (1..=99; kept modest so the
    ///   compositor never stacks above kernel critical tasks).
    /// - `frame_nice` is the fallback nice (-20..=19) when realtime is denied.
    pub fn detect(pin_frame: bool, rt_priority: i32, frame_nice: i32) -> Self {
        let rt_priority = rt_priority.clamp(1, 99);
        let frame_nice = frame_nice.clamp(-20, 19);
        let available = detect_online_cpus();
        let available = available.filter(|cpus| !cpus.is_empty());
        // Pin to the first (lowest) online CPU; feeders/background fan out
        // across everything after it.
        let (frame_mask, rest) = match (&available, pin_frame) {
            (Some(cpus), true) => {
                let (head, tail) = cpus.split_first().unwrap_or((&0, &[]));
                (Some(vec![*head]), tail.to_vec())
            }
            (Some(cpus), false) => (None, cpus.clone()),
            (None, _) => (None, Vec::new()),
        };
        Self {
            enabled: true,
            frame_cpu: frame_mask.as_ref().and_then(|m| m.first().copied()),
            frame_mask,
            available,
            rt_priority,
            frame_nice,
            rest,
        }
    }

    /// The logical CPU reserved for the frame loop, if any.
    pub fn frame_cpu(&self) -> Option<usize> {
        self.frame_cpu
    }

    /// The configured RT priority level for the frame thread.
    pub fn rt_priority(&self) -> i32 {
        self.rt_priority
    }

    /// Whether this plan actually elevates/pins the frame loop.
    pub fn realtime_enabled(&self) -> bool {
        self.enabled
    }

    /// Override the dedicated frame CPU to a specific logical index. The
    /// frame mask becomes exactly `[cpu]` (if `cpu` is online) and the
    /// remaining CPUs are re-derived as the "rest" set.
    pub fn with_frame_pin(mut self, cpu: usize) -> Self {
        if !self.enabled {
            return self;
        }
        let available = self.available.clone().unwrap_or_default();
        if !available.contains(&cpu) {
            // The requested CPU is not in the affinity set (offline or
            // cpuset-forbidden); keep the auto-detected pin rather than
            // pinning to a forbidden core.
            return self;
        }
        self.frame_cpu = Some(cpu);
        self.frame_mask = Some(vec![cpu]);
        self.rest = available.into_iter().filter(|c| *c != cpu).collect();
        self
    }

    /// The pin for a given role.
    pub fn pin_for(&self, role: ThreadRole) -> Option<Pin> {
        if !self.enabled {
            return None;
        }
        match role {
            ThreadRole::Frame => Pin::new(self.frame_mask.clone()?),
            ThreadRole::FrameFeeder | ThreadRole::Background => {
                Pin::new(if self.rest.is_empty() {
                    self.available.clone()?
                } else {
                    self.rest.clone()
                })
            }
        }
    }

    /// The concrete scheduling policy for a role, in precedence order.
    pub fn policy_for(&self, role: ThreadRole) -> SchedPolicy {
        if !self.enabled {
            return SchedPolicy::Other;
        }
        match role {
            ThreadRole::Frame => SchedPolicy::Fifo,
            ThreadRole::FrameFeeder => SchedPolicy::RoundRobin,
            ThreadRole::Background => SchedPolicy::Other,
        }
    }

    /// Target nice for a role's `SCHED_OTHER` fallback path.
    pub fn nice_for(&self, role: ThreadRole) -> i32 {
        if !self.enabled {
            return 0;
        }
        match role {
            ThreadRole::Frame => self.frame_nice,
            ThreadRole::FrameFeeder => 0,
            ThreadRole::Background => 10,
        }
    }

    /// RT priority level for a role's realtime policy.
    pub fn rt_priority_for(&self, role: ThreadRole) -> i32 {
        if !self.enabled {
            return 0;
        }
        match role {
            // The frame thread owns the realtime level from the plan.
            ThreadRole::Frame => self.rt_priority,
            // Feeders sit one level below the frame thread.
            ThreadRole::FrameFeeder => (self.rt_priority - 1).max(1),
            ThreadRole::Background => 1,
        }
    }
}

impl Default for SchedPlan {
    fn default() -> Self {
        Self::detect(true, 10, -10)
    }
}

/// Probe the set of *online* logical CPUs from `sched_getaffinity`.
fn detect_online_cpus() -> Option<Vec<usize>> {
    #[cfg(target_os = "linux")]
    {
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::cpu_set_t>();
        if unsafe { libc::sched_getaffinity(0, size, &mut set) } != 0 {
            return None;
        }
        let max = CPU_SETSIZE.min(1024);
        let cpus = (0..max)
            .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &set) })
            .collect::<Vec<_>>();
        if cpus.is_empty() {
            None
        } else {
            Some(cpus)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ();
        None
    }
}

#[cfg(target_os = "linux")]
const CPU_SETSIZE: usize = 1024;

/// Apply `plan` to the calling thread for `role`, logging each outcome.
///
/// Order: try the house realtime policy for the role first; if the kernel
/// refuses (no `CAP_SYS_NICE` / `RLIMIT_RTPRIO`), fall back through `SCHED_OTHER`
/// with a role-appropriate nice. Then apply the role's CPU pin.
pub fn apply_role(plan: &SchedPlan, role: ThreadRole) {
    if !plan.enabled {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        apply_sched_policy(plan, role);
        if let Some(pin) = plan.pin_for(role) {
            apply_pin(&pin, role);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (plan, role);
    }
}

fn apply_sched_policy(plan: &SchedPlan, role: ThreadRole) {
    let policy = plan.policy_for(role);
    apply_sched_policy_fallback(plan, role, policy);
}

fn apply_sched_policy_fallback(plan: &SchedPlan, role: ThreadRole, policy: SchedPolicy) {
    match policy {
        SchedPolicy::Fifo => {
            let rt = plan.rt_priority_for(role);
            if try_set_sched(libc::SCHED_FIFO, rt) {
                log::info!("sched[{role}]: elevated to SCHED_FIFO priority {rt}");
            } else {
                apply_sched_policy_fallback(plan, role, SchedPolicy::RoundRobin);
            }
        }
        SchedPolicy::RoundRobin => {
            let rt = plan.rt_priority_for(role);
            if try_set_sched(libc::SCHED_RR, rt) {
                log::info!("sched[{role}]: elevated to SCHED_RR priority {rt}");
            } else {
                apply_sched_policy_fallback(plan, role, SchedPolicy::Other);
            }
        }
        SchedPolicy::Other => {
            let target_nice = plan.nice_for(role);
            set_other_nice(role, target_nice);
        }
    }
}

fn try_set_sched(policy: i32, priority: i32) -> bool {
    let param = libc::sched_param { sched_priority: priority };
    let rc = unsafe { libc::sched_setscheduler(0, policy, &param) };
    if rc == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error();
        log::debug!("sched: sched_setscheduler(policy={policy}) rejected: {err}");
        false
    }
}

fn set_other_nice(role: ThreadRole, nice: i32) {
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, nice) };
    if rc == 0 {
        if nice < 0 {
            log::info!("sched[{role}]: elevated to nice {nice}");
        } else {
            log::debug!("sched[{role}]: adjusted nice {nice}");
        }
    } else {
        let err = std::io::Error::last_os_error();
        if nice < 0 {
            log::warn!(
                "sched[{role}]: setpriority({nice}) rejected ({err}); run under tessera.service for realtime/nice privilege"
            );
        } else {
            log::debug!("sched[{role}]: setpriority({nice}) rejected: {err}");
        }
    }
}

fn apply_pin(pin: &Pin, role: ThreadRole) {
    #[cfg(target_os = "linux")]
    {
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        unsafe { libc::CPU_ZERO(&mut set) };
        for cpu in &pin.cpus {
            if *cpu < CPU_SETSIZE {
                unsafe { libc::CPU_SET(*cpu, &mut set) };
            }
        }
        let size = std::mem::size_of::<libc::cpu_set_t>();
        let rc = unsafe { libc::sched_setaffinity(0, size, &set) };
        if rc == 0 {
            if role == ThreadRole::Frame {
                log::info!("sched[{role}]: pinned to dedicated CPU {:?}", pin.cpus());
            } else {
                log::debug!("sched[{role}]: pinned to CPUs {:?}", pin.cpus());
            }
        } else {
            let err = std::io::Error::last_os_error();
            log::warn!("sched[{role}]: sched_setaffinity rejected: {err}");
        }
    }
}

/// Pin the calling thread (the frame loop) to the plan's dedicated CPU.
pub fn pin_current_thread(plan: &SchedPlan, role: ThreadRole) {
    if !plan.enabled {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(pin) = plan.pin_for(role) {
            apply_pin(&pin, role);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (plan, role);
    }
}

/// `mlockall` the compositor's future pages so background-IO card/balloon
/// pressure can't push the frame path to swap. Best-effort: needs
/// `CAP_IPC_LOCK` or `LimitMEMLOCK=infinity` in the systemd unit.
pub fn lockdown_memory() {
    #[cfg(target_os = "linux")]
    {
        let rc = unsafe { libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) };
        if rc == 0 {
            log::info!("sched: compositor memory locked (mlockall)");
        } else {
            let err = std::io::Error::last_os_error();
            log::warn!("sched: mlockall rejected ({err}); run under tessera.service for memory locking");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ();
    }
}

/// Convenience wrapper: apply the `Frame` role to the calling thread and
/// ensure the process-wide plan is armed.
pub fn make_current_frame_thread(plan: &SchedPlan) {
    if !plan.enabled {
        return;
    }
    arm_global_plan(plan.clone());
    pin_current_thread(plan, ThreadRole::Frame);
    apply_role(plan, ThreadRole::Frame);
}

/// Mark the calling thread as `Background`, using the process-wide plan armed
/// at startup.
pub fn mark_background() {
    apply_role(global_plan(), ThreadRole::Background);
}

/// Mark the calling thread as a `FrameFeeder` (capture/wallpaper), using the
/// process-wide plan armed at startup.
pub fn mark_frame_feeder() {
    apply_role(global_plan(), ThreadRole::FrameFeeder);
}

impl std::fmt::Display for ThreadRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ThreadRole::Frame => "frame",
            ThreadRole::FrameFeeder => "frame-feeder",
            ThreadRole::Background => "background",
        };
        f.write_str(s)
    }
}

/// A per-thread deadline watchdog that observes frame execution and frame
/// pacing stalls under load.
///
/// Unlike naive timers that trip during normal idle sleeps, this watchdog
/// measures active frame execution time and inter-frame intervals while
/// animations or redraws are in-flight.
#[derive(Debug)]
pub struct DeadlineWatchdog {
    budget: std::time::Duration,
}

impl DeadlineWatchdog {
    /// Create a watchdog with the given stall budget.
    pub fn new(budget: std::time::Duration) -> Self {
        Self { budget }
    }

    /// Configured budget.
    pub fn budget(&self) -> std::time::Duration {
        self.budget
    }

    /// Check if a single frame iteration (processing input + layout + render)
    /// exceeded the stall budget.
    pub fn check_execution_stall(&self, elapsed: std::time::Duration) -> Option<std::time::Duration> {
        (elapsed > self.budget).then_some(elapsed)
    }

    /// Check if the interval between frames during active redraw/animation
    /// exceeded the stall budget.
    pub fn check_pacing_stall(
        &self,
        animating: bool,
        elapsed: std::time::Duration,
    ) -> Option<std::time::Duration> {
        (animating && elapsed > self.budget).then_some(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_plan_is_coherent() {
        let plan = SchedPlan::default();
        assert!(plan.realtime_enabled());
        if let Some(cpu) = plan.frame_cpu() {
            assert!(plan.frame_mask.is_some());
            assert!(plan.rest.iter().all(|c| *c != cpu), "frame cpu leaks into rest");
        }
        assert_eq!(plan.policy_for(ThreadRole::Frame), SchedPolicy::Fifo);
        assert_eq!(plan.policy_for(ThreadRole::FrameFeeder), SchedPolicy::RoundRobin);
        assert_eq!(plan.policy_for(ThreadRole::Background), SchedPolicy::Other);
    }

    #[test]
    fn disabled_plan_is_completely_inert() {
        let plan = SchedPlan::disabled();
        assert!(!plan.realtime_enabled());
        assert_eq!(plan.frame_cpu(), None);
        assert!(plan.pin_for(ThreadRole::Frame).is_none());
        assert!(plan.pin_for(ThreadRole::Background).is_none());
        assert_eq!(plan.policy_for(ThreadRole::Frame), SchedPolicy::Other);
        assert_eq!(plan.nice_for(ThreadRole::Frame), 0);
        assert_eq!(plan.rt_priority_for(ThreadRole::Frame), 0);
    }

    #[test]
    fn rt_priority_is_bounded() {
        let plan = SchedPlan::detect(true, 500, -10); // clamp 1..=99
        assert!((1..=99).contains(&plan.rt_priority));
        let plan2 = SchedPlan::detect(true, 0, -10);
        assert!((1..=99).contains(&plan2.rt_priority));
    }

    #[test]
    fn feeder_is_one_below_frame() {
        let plan = SchedPlan::detect(true, 10, -10);
        assert!(plan.rt_priority_for(ThreadRole::FrameFeeder) < plan.rt_priority_for(ThreadRole::Frame));
        assert_eq!(plan.rt_priority_for(ThreadRole::FrameFeeder), 9);
        assert_eq!(plan.rt_priority_for(ThreadRole::Frame), 10);
    }

    #[test]
    fn watchdog_evaluates_active_stalls_without_idle_false_positives() {
        let w = DeadlineWatchdog::new(std::time::Duration::from_millis(50));
        // Under budget: no stall
        assert!(w.check_execution_stall(std::time::Duration::from_millis(16)).is_none());
        // Over budget: stall reported
        assert_eq!(
            w.check_execution_stall(std::time::Duration::from_millis(80)),
            Some(std::time::Duration::from_millis(80))
        );
        // Idle (not animating): long interval does NOT trigger stall
        assert!(w.check_pacing_stall(false, std::time::Duration::from_millis(1000)).is_none());
        // Active animating: long interval DOES trigger stall
        assert_eq!(
            w.check_pacing_stall(true, std::time::Duration::from_millis(120)),
            Some(std::time::Duration::from_millis(120))
        );
    }

    #[test]
    fn no_pin_leaves_frame_unpinned() {
        let plan = SchedPlan::detect(false, 10, -10);
        assert!(plan.frame_cpu().is_none());
        assert!(plan.pin_for(ThreadRole::Frame).is_none());
    }
}
