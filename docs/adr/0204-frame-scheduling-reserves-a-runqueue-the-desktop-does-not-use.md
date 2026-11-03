---
id: ADR-0204
title: "Frame scheduling reserves a runqueue the desktop does not use"
status: accepted
date: 2026-11-02
scope: compositor, scheduling, launch-services, session
superseded_by: null
negative_knowledge: true
---

# 0204. Frame scheduling reserves a runqueue the desktop does not use

- Status: Accepted
- Date: 2026-11-02
- Deciders: Tessera Maintainers
- Amends: [ADR-0199](0199-realtime-compositor-scheduling.md)
- Related: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md)

---

## Context and Problem Statement

[ADR-0199](0199-realtime-compositor-scheduling.md) introduced realtime frame
scheduling: a `SchedPlan` pins the frame loop to a "dedicated" CPU and raises
its priority so a compile storm cannot starve presentation. The mechanism
worked as designed and the priority raise behaves correctly. The CPU it chose,
and the blast radius of choosing it, did not.

Two defects, both silent:

1. **The pin landed on the busiest CPU in the machine.** `SchedPlan::detect`
   took `cpus.split_first()` — the lowest-numbered online CPU — as the
   dedicated frame CPU. On x86 that is CPU 0, which carries the 8259/IO-APIC
   legacy routes, the i8042, the ACPI SCI, and every `kworker/0:*`. Measured
   on the reference machine: 79 processes confined to CPU 0, and
   `mpstat -P ALL` showing CPU 0 at 21.75 % user + 12.19 % system while every
   other core sat above 95 % idle. The "dedicated" core was the one CPU the
   kernel was already using for interrupt delivery.

2. **The pin escaped into every child process.** `apply_pin` calls
   `sched_setaffinity(0, …)`. On Linux that is *thread*-scoped — so it does not
   confine the compositor the way the ADR assumed — but a child process
   inherits the **forking thread's** affinity mask. Every client the compositor
   launched from a pinned thread was born single-CPU. The observed set on the
   reference machine included `foot`, `bwrap`, `chrome` (all zygotes and the
   GPU process), `xdg-dbus-proxy`, and `tmux`. Every application the user
   started was confined to the compositor's own core.

Together these inverted the ADR's stated purpose. The frame loop was pinned to
the interrupt CPU, and the entire desktop was pinned to the same core to
compete with it. The field symptom was
`libinput: event processing lagging behind by 59 ms` on pointer motion — input
delivery losing to compositor and client work on one saturated core.

The original ADR's own risk section warned that "pinning can under-utilize a
small CPU set" and that the plan "clamps to the cgroup's online/affinity set."
It did not consider that the chosen CPU is chosen by the *kernel* for its own
IRQ affinity, or that children inherit the mask.

## Decision Drivers

- A realtime thread on a shared runqueue is not isolated, whatever its
  priority. `SCHED_FIFO` loses to hardware interrupt delivery on the same
  physical core, and an SMT sibling shares the execution resources outright.
- The reservation must be a *reservation*: the core the frame loop owns should
  be one the rest of the system does not want. CPU 0 is the one CPU the rest of
  the system always wants.
- Affinity is inherited across `fork`. Any policy applied to the compositor's
  threads is a policy applied to every process it spawns unless explicitly
  reversed. A launch path that does not reverse it hands the desktop a
  one-CPU mask.
- The fix must not depend on privileges the compositor may not have. It ships
  to bare TTYs, nested sessions, and unprivileged development runs.

## Considered Options

- **Keep the lowest-CPU rule and rely on the realtime priority.** Rejected: the
  priority is exactly what does *not* help against interrupt delivery on the
  same core, and it does nothing for the SMT sibling.
- **Select the frame CPU from the `cpuset` cgroup's `cpuset.cpus.effective`.**
  Rejected as the *primary* mechanism: Tessera does not require a delegated
  cpuset cgroup (ADR-0199 was written to degrade gracefully without one), and
  the effective set is usually identical to the affinity set the plan already
  queries.
- **Consume `isolcpus=` / `nohz_full=` from `/sys/devices/system/cpu/isolated`.**
  Rejected as a dependency: it is the *right* answer for a machine tuned for a
  realtime workload, but it requires a boot-parameter change no compositor can
  make for itself, and it is empty on stock kernels. Left as future work; the
  selection hook is a single function so honoring it later is additive.
- **Detect and avoid the CPU that owns the most IRQ affinity.** Considered and
  rejected: it requires reading `/proc/irq/*/smp_affinity`, which is
  `CAP_SYS_ADMIN`-gated on modern kernels, and would fail closed on exactly the
  unprivileged runs the ADR must support.
- **Make `apply_pin` process-scoped with `sched_setaffinity` on every thread.**
  Rejected outright: confining the *whole process* to one CPU is the bug, not
  the fix.
- **Chosen: select the highest-numbered online CPU, and release the inherited
  mask in every spawn path.**

## Decision Outcome

Chosen option: **the frame loop takes the highest-numbered online CPU, and every
child process has its affinity widened back to the plan's full set before
`exec`.**

### 1. CPU selection

`preferred_frame_cpu` returns the maximum of the plan's available set.

On a hybrid Intel part (the reference machine is an Arrow Lake-P) the
highest-numbered logical CPUs are E-cores, which the scheduler already prefers
to leave idle and which carry none of the boot-strap IRQ affinity. On a uniform
part the highest CPU still avoids the CPU 0 legacy routes, the ACPI SCI, and
the `kworker/0:*` pool.

The choice is deliberately not "the lowest CPU": that is the value every naive
pin selects and the value the kernel uses for unmanaged IRQs, so it puts the
frame loop in the busiest available company. `[INV-ARCH-95]` records the rule,
and a test asserts CPU 0 is never selected while more than one CPU is online.

An explicit `compositor.scheduling.pin_cpu` still wins (validated against the
plan's available set by `with_frame_pin`), so a machine tuned with `isolcpus=`
can nominate its isolated core through configuration.

### 2. Child affinity release

`release_inherited_affinity(plan)` writes the plan's full available set to the
calling thread's affinity. It is called from a `pre_exec` hook, so it runs in
the forked child before `exec`, and it is async-signal-safe: one
`sched_setaffinity(2)`, no allocation.

Every spawn path installs the hook:

| spawn site                                  | seam                                    |
| ------------------------------------------- | --------------------------------------- |
| desktop-entry launch (`launch`)              | `detach_from_frame_pin`                 |
| Interaction Domain launch (`launch_managed`) | `ProcessCgroup::attach_on_exec`         |
| idle coordinator (`tessera-idle`)            | `session::detach_from_frame_pin`        |
| lock-now request (`tessera-idle --lock-now`) | `session::detach_from_frame_pin`        |
| direct lock fallback (`tessera-lock`)        | `session::detach_from_frame_pin`        |

The function returns `Result<(), io::Error>` and is always `Ok`. Widening a mask
to the set the process already ran on cannot fail in a way that matters; a
launch that refused to start because an affinity write returned `EPERM` would be
strictly worse than one that started with an inherited mask. The `Result` shape
exists solely so the function composes with `CommandExt::pre_exec` directly,
which keeps the hook one line at each site instead of a closure that cannot
fail.

### 3. What the pin still means

`sched_setaffinity(0, …)` is thread-scoped. `apply_pin` therefore confines the
calling *thread* — which for the frame role is exactly the intent — and the
rest of the compositor's threads retain whatever affinity they inherited.
`mark_background` / `mark_frame_feeder` apply the plan's `rest` set to the
threads that opt in. The plan is a thread-level policy, and the ADR now says so;
it was never a process-level cgroup.

### Invariants & Behavioral Boundaries

Each invariant names its enforcement mechanism. The `test_inv_*` naming is the
repository's audit convention for the invariant set
(`docs/dev/standards/invariants.md`).

- `[INV-ARCH-95] The frame CPU is never the IRQ CPU.` While more than one CPU is
  available, the dedicated frame CPU must not be CPU 0. Enforced by
  `test_inv_arch_95_frame_cpu_avoids_the_irq_cpu` (pure selection), and
  observed at startup in the `frame_pin=Some(n)` log line.
- `[INV-ARCH-96] No child inherits a thread pin.` Every `pre_exec` closure that
  spawns a process must widen the child's affinity to the plan's full available
  set before `exec`. The release must be async-signal-safe and must never fail
  the launch. Enforced by `test_inv_arch_96_a_pinned_threads_child_escapes_the_pin`
  (pins a thread, forks, compares masks),
  `test_inv_arch_96_launched_client_does_not_inherit_the_frame_pin` (drives the
  production `launch()` path), and
  `test_inv_arch_96_child_affinity_release_is_always_ok` (the `pre_exec`
  contract).
- `[INV-ARCH-97] The plan partitions the CPU set.` `frame_mask ∪ rest` is the
  available set, the two are disjoint, and `frame_mask` holds exactly one CPU
  when `pin_frame` is on. Enforced by
  `test_inv_arch_97_frame_mask_and_rest_partition_the_available_set`.
- The affinity release is a no-op when the plan is disabled or the platform has
  no affinity query; it is not a licence to touch affinity when realtime
  scheduling is off.
- The global plan remains the launch path's entry point (`OnceLock`), because
  spawn sites do not thread a plan through every call. `SchedPlan::available_cpus`
  is the public accessor that keeps callers from re-querying the kernel and
  racing the ambient mask.

## Consequences

### Positive

- The frame loop owns a core the kernel is not using for interrupt delivery, so
  the realtime priority from ADR-0199 finally buys what it was written to buy.
- The desktop's processes spread across the machine again. Measured on the
  reference machine after re-widening 68 confined processes: CPU 0 dropped from
  21.75 %/12.19 % to 14.46 %/8.09 %, and the compositor's observed frame period
  p50 went from 7.9 ms to 2.9 ms.
- The startup log names the real dedicated CPU, so an operator can see the plan
  sized itself correctly.

### Negative / Risks

- The highest-numbered CPU on some topologies is a low-clock E-core. That is
  acceptable and in fact desirable for a latency target measured in single-digit
  milliseconds: an idle E-core at 2.5 GHz delivers a frame sooner than a busy
  P-core at 5.4 GHz, and it leaves the P-cores to the workloads that need
  throughput. On a machine where this is wrong, `pin_cpu` overrides it.
- Reading the affinity set at startup means a CPU hotplug after startup is not
  reflected. The previous implementation had the same limitation and the plan
  was already computed once by design (ADR-0199).
- `pre_exec` closures run in a forked child where allocation and locks are
  forbidden. The release function obeys that; any future addition to those
  closures must too, which is a real constraint reviewers must know about.
- The release widens to `plan.available`, captured at startup. A child launched
  after a `sched_setaffinity` restriction applied by an external manager (a
  cgroup cpuset change) would be widened past that restriction — except
  `sched_setaffinity` cannot widen past a cpuset, so the kernel refuses and the
  hook silently leaves the child where the cgroup put it. Correct by
  construction, worth stating.

## Rejected Alternatives & Negative Knowledge

### Fix only the CPU choice, leave child inheritance alone
- **Why considered**: it is the smaller diff and the CPU choice is the more
  visible defect.
- **Why rejected**: measured directly, the client confinement was the larger
  contributor — 79 confined processes, including every application the user
  launched. A compositor alone on a good core, with the entire desktop on the
  same core, is still a stuttering desktop.

### Fix only the inheritance, leave CPU 0
- **Why considered**: the inheritance bug is the more surprising one.
- **Why rejected**: the frame loop on the IRQ CPU loses to interrupt delivery
  regardless of what else is pinned; the two defects compound and either alone
  leaves a measurable stall.

### Inherit the affinity `pre_exec` already implies and do nothing
- **Why considered**: `setsid --fork` re-execs, and one might expect a fresh
  affinity context.
- **Why rejected**: `execve` preserves the affinity mask by POSIX. The
  inheritance is real and observable, which is how the defect was found.

### Annul the pin by relaunching the compositor's threads unpinned
- **Why considered**: a single "don't pin" mode would be simpler than a release
  hook.
- **Why rejected**: it would fix the clients by giving up the frame loop's
  isolation, discarding ADR-0199 rather than repairing it.
