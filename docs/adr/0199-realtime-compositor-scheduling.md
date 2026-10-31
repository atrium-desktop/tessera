---
id: ADR-0199
title: "Realtime compositor scheduling: protecting the frame loop from CPU starvation"
status: accepted
date: 2026-10-31
scope: compositor, scheduling, presentation, launch-services, systemd
superseded_by: null
negative_knowledge: true
---

# 0199. Realtime compositor scheduling: protecting the frame loop from CPU starvation

- Status: Adopted
- Date: 2026-10-31
- Deciders: Tessera Maintainers
- Related: [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0196](0196-compositor-side-presentation-time-feedback.md)

---

## Context and Problem Statement

Tessera renders every frame on a **single event-loop thread**. Under a
CPU-saturating workload — a `cargo build`, a desktop-app scan, an icon-decode
storm — that thread shares a runqueue with greedy workers that outnumber the
logical CPUs. Classical `SCHED_OTHER` favors the throughput-heavy worker, not
the latency-sensitive compositor, so the frame thread loses the runqueue and
the **entire desktop stutters** (cursor, windows, shell chrome all fall behind
the refresh cadence).

Observations from the field confirmed the failure mode precisely:

- The compositor ran at `SCHED_OTHER`, nice `0`, with **no realtime priority, no
  CPU affinity, and no RT capability delegation** in the shipped systemd unit.
- While compiling, runqueue depth reached several times `nproc`; the frame loop
  had no defense and the desktop dropped frames under input motion.
- The unit granted none of `LimitRTPRIO` / `LimitNICE` / `LimitMEMLOCK`, so even
  a hypothetical in-process `sched_setscheduler` / `setpriority` / `mlockall`
  would have failed with `EPERM` and silently degraded.
- Optics (the rendering stack) exposes **no** scheduling/priority/affinity
  primitive of its own; its starvation defenses are MAILBOX present + a
  display-driven pacer + a dedicated presentation thread. It cannot lift the
  compositor's frame thread above a compile storm.

This record is the architecture decision to close the gap directly in
Tessera, owning the frame-loop scheduling contract rather than waiting for a
rendering-stack feature that does not exist.

## Decision

Add a **`tessera-sched` crate** that encodes the display-server scheduling
contract, a **`[compositor]` config table** to tune it, and a **systemd unit
delegation** so the realtime raise is actually permitted.

### 1. A dedicated scheduling crate (`tessera-sched`)

Linux-only (no-op on other targets). Centerpiece is a `SchedPlan` resolved once
at startup that splits the process's threads into a **frame / frame-feeder /
background** taxonomy and applies a staged schedule:

- **Frame** — the compositor's event/render/present loop. Raised to
  `SCHED_FIFO` at a bounded realtime priority (falling back to `SCHED_RR`, then
  to negative `nice`), and **pinned to a dedicated logical CPU** so the frame
  core's runqueue is disjoint from every compile/scan worker.
- **FrameFeeder** — capture readback/encode and wallpaper-video decode, which
  feed every frame. Elevated one level below the frame thread and pinned off
  the frame core.
- **Background** — app scan, bluetooth, casting, wireless, status probes,
  config writes, child reapers. **De-prioritized** and pinned off the frame core.

Every syscall is **best-effort and logged, never fatal**: a compositor must
start even unprivileged. A process-wide `OnceLock<SchedPlan>` (`arm_global_plan`)
lets worker threads in helper crates call `mark_background()` /
`mark_frame_feeder()` without threading a plan through every spawn site.

A `DeadlineWatchdog` on the frame thread logs stalls past a configured budget,
so a starved loop is observable instead of silent.

### 2. Configuration (`[compositor]`)

| Key | Default | Meaning |
|-----|---------|---------|
| `[compositor] realtime` | `true` | Master switch. `false` leaves the frame loop at the inherited scheduling class. |
| `scheduling.pin_frame` | `true` | Pin the frame loop to its own logical CPU. |
| `scheduling.pin_cpu` | lowest online | Specific logical CPU for the frame loop. |
| `scheduling.rt_priority` | `10` | `SCHED_FIFO`/`SCHED_RR` level, `1..=99`. |
| `scheduling.frame_nice` | `-10` | Fallback nice when realtime is not permitted. |
| `scheduling.watchdog_ms` | `100` | Stall budget before a warning is logged. `0` disables. |

Realtime is **on by default**, degrading gracefully (logged) where the kernel
or sandbox forbids it.

### 4. Optics RHI & presentation decoupling (`../optics`)

Fixing CPU starvation alone is insufficient if the graphics queue or swapchain
acquire stalls the event-loop thread. Three companion enhancements were landed
in `../optics`:

- **Vulkan hardware queue priority (`VK_EXT_global_priority` / `VK_KHR_global_priority`)**:
  In `libs/flux/src/core/device.c`, the graphics queue requests
  `VK_QUEUE_GLOBAL_PRIORITY_HIGH_KHR` when supported by the physical device,
  ensuring the GPU kernel scheduler prioritizes UI/presentation command
  buffers over background compute or compiler jobs. Gracefully retries
  without the extension if unprivileged.
- **Bounded frame acquire (`begin_frame_timeout`)**:
  In `libs/flux` and `flux-rs`, `flux_frame_begin_desc` now supports
  `non_blocking` and `timeout_ns`. Tessera's composite path
  (`presentation.rs`) calls `self.surface.begin_frame_timeout(1_000_000)` (1 ms
  bound) rather than blocking for up to 2 seconds. If the GPU is busy, it
  yields immediately with `PresentationOutcome::Retry`, allowing the event
  loop to pump input events and update pointer motion smoothly.
- **Iris presentation worker thread elevation**:
  In `libs/iris/src/app_wayland.c`, the presentation thread
  (`wp_presenter_thread_main`) is named `iris-presenter` and elevated to
  `SCHED_RR`/negative nice, with an adaptive timeout dynamically tracking
  display refresh rate rather than hardcoding 40 ms.

### 5. Systemd delegation

`contrib/systemd/user/tessera.service` now grants:

```
LimitRTPRIO=50
LimitNICE=-15
LimitMEMLOCK=infinity
```

`RLIMIT_RTPRIO`/`RLIMIT_NICE`/`RLIMIT_MEMLOCK` let the **process raise its own
threads** to realtime / negative-nice and `mlockall`, without a broad ambient
capability. This is the same delegation model KWin/Sway use.

## Consequences

### Positive

- The frame loop survives a compile storm: priority raise + CPU pin guarantee a
  dedicated core that `rustc` workers cannot steal.
- All frame-feeding threads (capture, wallpaper-video) are part of the tiering,
  so a screenshot encode cannot starve presentation either.
- Stall observability converts a silent desktop stutter into a logged,
  actionable diagnostic.
- Graceful degradation: unprivileged runs (nested dev, bare TTY) still start
  and still pin where permitted.

### Negative / Risks

- Real-time scheduling can, if misused, preempt kernel-irrigation tasks or
  hang the CPU. `rt_priority` is deliberately bounded (`1..=99` clamped, default
  `10`) well below Linux's internal critical band, and feed-back threads sit
  strictly one level below the frame thread.
- Pinning can under-utilize a small CPU set. `detect()` clamps to the cgroup's
  online/affinity set and `with_frame_pin` refuses a contradictory `pin_cpu`.
- `mlockall` pins the process's working set; `LimitMEMLOCK=infinity` trades
  some memory for deterministic swap-free frame timing. The default unit opts
  into this for a compositor, which is memory-stable by nature.
- No optics change is required; the rendering stack is unaffected.

## Alternatives Considered

- **Rely on Optics scheduling** — rejected: Optics exposes no scheduling
  primitive; its pacer/present-thread defenses address different stalls, not
  runqueue starvation.
- **Nice the whole process** — rejected: de-prioritizing the whole compositor
  is the opposite of what the frame loop needs, and `nice` does not isolate a
  dedicated core.
- **Config default off** — rejected: the requirement is "the desktop must not
  stutter under a build"; on-by-default with graceful degradation is strictly
  safer than shipping a defenseless default.