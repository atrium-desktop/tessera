---
id: ADR-0178
title: "Optics Transit Smoothstep Unification, Chrome Prepass Lifecycle Decoupling, and Responsive Reveal Spring Tuning"
status: accepted
date: 2026-10-02
scope: optics/transit, shell/motion, shell/chrome-contract, shell/control-center, presentation
superseded_by: null
negative_knowledge: true
---

# 0178. Optics Transit Smoothstep Unification, Chrome Prepass Lifecycle Decoupling, and Responsive Reveal Spring Tuning

- Status: Accepted
- Date: 2026-10-02
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md), [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md)

---

## Context and Problem Statement

Following the retirement of the Control Center's full-screen 16σ backdrop blur in [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md), users and developers continued to analyze entry and exit performance on high-resolution displays. While GPU time per frame dropped from >15ms to sub-millisecond territory (because Dual-Kawase pyramid blur and Prism `Recompute` were eliminated), three structural and architectural discrepancies remained:

1. **Sluggish Reveal Spring Travel and Asymmetric Exit Tail**:
   `REVEAL_SPRING` was configured with `stiffness: 300.0` and damping `1.0` alongside an aggressive `REVEAL_SETTLE_VALUE_EPS: 0.002`. In practice, critical damping at $\omega_0 \approx 17.32\,\text{rad/s}$ required nearly $\approx 0.48\,\text{s}$ to settle within $0.002$. On exit, the side and main content tabs fade to zero opacity when reveal reaches $0.28$ and $0.18$ respectively, meaning the central UI had completely vanished while the spring spent another $\approx 200\,\text{ms}$ crawling through visually imperceptible values ($0.18 \to 0.002$). Because reveal animation marks damage conservative (`None` -> `FrameDamage::Full`), the compositor was forced to perform dozens of redundant full-screen desktop repaints.

2. **Hand-Rolled Smoothstep Easing Outside Optics Transit**:
   [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md) established invariant `[INV-ARCH-44] Mechanism in Transit, Policy in Tessera`, mandating that `tessera-shell` must not maintain hand-rolled easing math. However, cubic Hermite smoothstep ($f(t) = t^2(3 - 2t)$) remained implemented in-tree in `crates/tessera-shell/src/widgets/motion.rs` rather than being provided canonically by Optics's `transit` C library and Rust bindings.

3. **Conflated Lifecycle Semantics (`prepare_backdrop` for Zero-Blur Modals)**:
   [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md) introduced a pre-pass to advance animation clocks and geometry before damage assessment and backdrop capture. However, the trait method was named `Chrome::prepare_backdrop`. Surfaces such as the Control Center, which declare zero blur and no backdrop regions, were architecturally distorted by having to implement `prepare_backdrop` merely to update their reveal springs before damage evaluation.

## Decision Drivers

- **Zero-Mechanism Duplication**: All mathematical easing functions and spline interpolations belong in Optics `transit`, ensuring uniform feel and zero divergence across applications, tools, and shell chrome.
- **Crisp, Responsive Modal Dynamics**: Modal enter/exit transitions must feel responsive and snappy ($\le 220\,\text{ms}$), completely eliminating invisible long tails of full-output repaints.
- **Architectural Honesty in Lifecycle Contracts**: Lifecycle methods must reflect their actual invariant (`prepare_frame` before damage assessment) rather than coupling zero-blur surfaces to legacy backdrop terminology.

## Considered Options

- **Option 1 (Chosen)**:
  1. Add `transit_smoothstep` to Optics `transit` C library and re-export `transit::smoothstep` in `transit-rs`.
  2. Deprecate in-tree `smoothstep` in `tessera-shell::widgets::motion` in favour of re-exporting `transit::smoothstep`.
  3. Rename `prepare_backdrop` to `prepare_frame` across `Chrome` and `Shell` while retaining `prepare_backdrop` as an inline compatibility alias.
  4. Tune `ControlCenter`'s `REVEAL_SPRING` to `stiffness: 480.0`, damping `1.0`, and settle threshold `0.004` / `0.04`, settling cleanly in $\approx 220\,\text{ms}$.
- **Option 2: Localize Damage During Full-Screen Canvas Fade**:
  Attempt to clip damage during canvas fade. Rejected because the canvas is full-screen and alpha-blends over the entire desktop; every pixel mathematically changes colour during the fade, so damage is inherently full-screen. The correct optimization is bounding the temporal duration, not faking damage bounds.
- **Option 3: Leave Smoothstep in `tessera-shell`**:
  Rejected as an ongoing violation of `[INV-ARCH-44]`.

## Decision Outcome

Chosen option: **Option 1**.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-55] Canonical Spline & Easing Mechanism in Transit`: All normalized $[0, 1]$ easing curves and spline interpolations (including cubic Hermite smoothstep) MUST reside in Optics `transit` (`transit_smoothstep` / `transit::smoothstep`). Shell crates must never write in-tree mathematical equivalents.
- `[INV-ARCH-56] Chrome Prepass Decoupled From Backdrop Legacy`: The per-frame animation and geometry prepass is canonically named `Chrome::prepare_frame` and `Shell::prepare_frame`. Components without backdrop effects implement `prepare_frame` directly without referencing `prepare_backdrop`.
- `[INV-ARCH-57] Responsive Modal Reveal Bounded Convergence`: Full-screen modal canvas reveals MUST converge within $250\,\text{ms}$ under critical damping ($\zeta = 1.0$) to avoid scheduling redundant full-output damage frames after foreground elements have faded.

## Implementation Specifics

1. **Optics `libs/transit` & `bindings/transit-rs`**:
   - `transit.h` & `transit.c`: Add `TRANSIT_API float transit_smoothstep(float t);`.
   - `transit` Rust crate: Expose `pub fn smoothstep(t: f32) -> f32`.
   - Unit tests: Add C and Rust checks verifying endpoints ($0 \to 0, 1 \to 1$), symmetry at $0.5$, clamping, and monotonicity.
2. **`tessera-shell::widgets::motion`**:
   - Remove private `pub fn smoothstep` and re-export `transit::smoothstep`.
3. **`tessera-shell::component` & `tessera::runtime`**:
   - Introduce `Chrome::prepare_frame` with default no-op.
   - Retain `Chrome::prepare_backdrop` as an inline alias delegating to `prepare_frame`.
   - Update `Shell::prepare_frame` and call sites in `tessera::runtime::presentation`.
4. **`tessera-shell::control_center`**:
   - Update `REVEAL_SPRING` to `stiffness: 480.0`, `damping: 1.0`.
   - Set settle tolerances to `REVEAL_SETTLE_VALUE_EPS: 0.004` and `REVEAL_SETTLE_VELOCITY_EPS: 0.04`.
   - Implement `prepare_frame` instead of `prepare_backdrop`.

## Rejected Alternatives & Negative Knowledge

### Retaining 0.002 Settle Threshold with Stiff Springs
- **Why considered**: Trying to make the spring settle at 99.8% precision.
- **Why rejected**: A settle threshold of $0.002$ in an 8-bit color space ($255 \times 0.002 \approx 0.51$) represents half a single quantization step, which cannot be represented in standard sRGB framebuffers. Holding the compositor in `FrameDamage::Full` for multiple extra frames to resolve sub-LSB alpha values wastes GPU memory bandwidth with zero perceptual benefit.

## Consequences

### Positive
- Optics `transit` motion library becomes fully comprehensive, covering standard Hermite smoothstep across C and Rust consumers.
- Modal canvas enter and exit animations settle cleanly in $\approx 220\,\text{ms}$, removing the lingering $200\,\text{ms}$ tail of full-output redraws.
- Clear separation between frame animation pre-pass (`prepare_frame`) and backdrop declarations (`backdrop_layers`).

### Negative / Trade-offs
- Calling code updating local `optics` submodule/checkout must recompile `transit` and its bindings (handled automatically by Cargo and Meson).
