---
id: ADR-0172
title: "Adopt Optics Transit Motion Vocabulary in Shell Chrome"
status: accepted
date: 2026-09-28
scope: shell, motion, animation, optics-integration
superseded_by: null
negative_knowledge: true
---

# 0172. Adopt Optics Transit Motion Vocabulary in Shell Chrome

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Consulted: Optics Graphics & Motion Subsystem Team
- Informed: Compositor Runtime & Accessibility Working Group

---

## Context and Problem Statement

Optics provides a dedicated, shared motion vocabulary library named `transit` (`libs/transit`, Rust bindings in `transit` / `transit-sys`, established in Optics ADR-0077 and unified in ADR-0106). `transit` provides provably non-divergent analytic closed-form damped harmonic oscillator springs (`transit_spring`), exponential approach/decay (`transit_approach`, `transit_decay`), cubic and back easing curves (`transit_ease_*`), Schmitt-trigger hysteresis latching (`transit_hysteresis`), and motion-adaptive critically damped signal smoothing (`transit_smoother`).

Previously, under ADR-0139, Tessera (formerly Aegis) eliminated three divergent in-tree spring integrators by consolidating them into `crates/tessera-shell/src/widgets/motion.rs`. While this successfully prevented Euler divergence on long frame stalls, `tessera-shell` continued to maintain an in-tree duplicate of the spring physics and easing math rather than consuming Optics's canonical `transit` library. Optics ADR-0077 explicitly specified that consumer hosts—specifically citing the compositor's motion module—should link and re-export `transit`, allowing the shared motion library to mature through production dogfooding and ensuring uniform physical feel across apps, shell chrome, and showcase examples.

We need to formalize the adoption of Optics's `transit` motion library in `tessera-shell`, define the boundary between Optics mechanism and Tessera policy, and address performance expectations rigorously.

## Decision Drivers

- **Dogfooding and Maturing the Shared Ecosystem**: Tessera is the primary production consumer of Optics. Dogfooding `transit` exercises the C ABI and safe Rust bindings under real compositor display loops, verifying standard presets (`snappy`, `gentle`, `bouncy`) and edge conditions (NaN resistance, adverse $\Delta t$, multi-slot swapchain drain).
- **Zero Physics Divergence**: Ensuring that UI components across the desktop environment (shell chrome, standalone lens-rs applications, preview harnesses) share the exact same mathematical definitions and physical tuning constants without algorithmic drift.
- **Architectural Separation of Concerns**: Optics owns *mechanism* (scalar mathematical formulas, closed-form analytic solutions, delta-time clamping, de-jitter latches); Tessera owns *policy* (choreography, activation triggers, window lifecycles, and desktop-wide reduced-motion enforcement).
- **Realistic Performance Expectations**: Clarifying that adopting a C-ABI motion library does not magically accelerate scalar math (which already costs $<0.05\%$ of frame budgets), but rather provides frame-pacing stability, provable bounded energy decay, and mathematical robustness.

## Considered Options

- **Option 1: Adopt `transit` in `tessera-shell` and Re-export Motion Primitives (Chosen)**: Add `transit` and `transit-sys` to the workspace dependencies, update `tessera-shell` to depend on `transit`, re-export canonical motion types (`Spring`, `SpringParams`, `Smoother`, `Hysteresis`, easing curves), and refactor chrome consumers (Dock, Control Center) to utilize `SpringParams`.
- **Option 2: Retain Private In-Tree Math in `tessera-shell`**: Keep `widgets::motion.rs` completely independent, ignoring Optics's `transit`.
- **Option 3: Pull `transit` into Low-Level Model Crates (`tessera-desktop`)**: Force `tessera-desktop` to link `transit`, replacing `tessera-desktop::transition::ease_out_cubic`.

## Decision Outcome

Chosen option: **Option 1: Adopt `transit` in `tessera-shell` and Re-export Motion Primitives**, because it realizes the architectural convergence intended in ADR-0077/ADR-0139, establishes true production dogfooding for Optics, and completely eliminates redundant physics implementations.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-44] Mechanism in Transit, Policy in Tessera`: `tessera-shell` MUST NOT maintain hand-rolled spring integration or easing math. All continuous spring dynamics, exponential decays, and hysteresis gates MUST delegate to `transit`.
- `[INV-ARCH-45] Bounded Integration & Reduced Motion Guarantees`: All chrome motion state advances MUST honor `transit`'s clamped $\Delta t \in [0, 1/30]\,\text{s}$ integration domain. When the global `reduced_motion` accessibility switch is active, transitions MUST immediately resolve to their target in $\le 1$ frame via `transit_spring_snap_to` or the `reduced_motion` parameter.
- `[INV-ARCH-46] Pure Model Independence`: `tessera-desktop` remains a pure, dependency-light model crate that does NOT link C dynamic libraries or FFI crates. Optics rendering and motion libraries are consumed at the presentation and shell chrome tiers (`tessera-shell`, `tessera-render`, `tessera`).

## Rejected Alternatives & Negative Knowledge

### Retain Private In-Tree Math in `tessera-shell` (Option 2)
- **Why considered**: Avoided adding an additional dependency edge (`tessera-shell -> transit`) and slightly simplified the build graph.
- **Why rejected**: Perpetuates duplicate maintenance of mathematical logic, leaves Optics's `transit` under-tested in primary production workloads, and risks future tuning drift between the compositor and external applications.

### Link `transit` into `tessera-desktop` (Option 3)
- **Why considered**: Allowed a single source of truth for window transition easing (`lerp_rect`, `ease_out_cubic`) across the model.
- **Why rejected**: Violates ADR-0110 and `[INV-ARCH-46]`. `tessera-desktop` is required by background daemons, protocol journals, and auditing crates that must compile without requiring C compilation toolchains or dynamically linking Vulkan/Optics shared libraries. The simple normalized easing curves in `tessera-desktop` remain pure inline Rust functions.

### Expecting Micro-Performance Acceleration from FFI
- **Negative Knowledge**: Moving scalar calculations ($1 - (1 - t)^3$ or $\exp(-\zeta \omega_0 t)$) from inlined Rust into a C shared library (`libtransit.so`) crosses an FFI / dynamic link boundary. It does not reduce CPU cycles; in micro-benchmarks, it introduces minimal function call overhead. The rationale for adopting `transit` is **mathematical provability, non-divergence, and ecosystem convergence**, not micro-benchmark speedup. Frame pacing is bounded by GPU composition and swapchain presentation, where `transit`'s guaranteed convergence prevents pathological frame bursts.

## Consequences

### Positive
- `tessera-shell` components share the canonical `transit` types (`Spring`, `SpringParams`, `Smoother`, `Hysteresis`).
- Optics's `transit` library receives Tier-1 real-world validation and dogfooding.
- Tuning parameters for chrome components are cleanly organized via `SpringParams`.
- Boundary conformity is strictly verified via `tooling/xtask`.

### Negative / Trade-offs
- `tessera-shell` gains a direct dependency on `transit`. Build workflows requiring local development must maintain `transit` in `.cargo/config.toml` (already standard in `.cargo/optics-local.toml`).
