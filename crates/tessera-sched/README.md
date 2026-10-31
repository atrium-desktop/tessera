# tessera-sched

Realtime scheduling and CPU-pinning for the Tessera compositor frame loop.

A Wayland compositor renders every frame on a single event-loop thread. When
that thread shares a runqueue with a CPU-saturating workload — a `cargo build`,
a desktop-app scan, an icon-decode storm — the frame cadence collapses and the
entire desktop stutters. Classical CPU scheduling favors the greedy worker, not
the latency-sensitive compositor.

This crate encodes the display-server scheduling contract precisely and
defensively:

- **Priority raise** — the frame thread is raised to `SCHED_FIFO` at a bounded
  realtime priority, falling back to `SCHED_RR`, then to negative `nice` as
  privileges fade. Failure is logged, never fatal.
- **CPU pinning** — the frame loop is pinned to a dedicated logical CPU (or a
  restricted mask); background threads are fanned out across the remaining
  cores so a compile worker cannot steal the render core.
- **Lockdown** — `mlockall` keeps the frame path out of swap under memory
  pressure.
- **Observability** — a per-thread `DeadlineWatchdog` surfaces frame stalls so a
  starved loop is never silent.

## Thread roles

- `Frame` — the compositor's event/render/present loop. Elevated above every
  other process.
- `FrameFeeder` — capture readback/encode and wallpaper-video decode threads.
  Elevated below `Frame`.
- `Background` — extension/scan/dbus work. De-prioritized and kept off the frame
  thread's pinned core.

The crate is Linux-only; on other targets every call degrades to a no-op so the
workspace stays portable and unit-testable.