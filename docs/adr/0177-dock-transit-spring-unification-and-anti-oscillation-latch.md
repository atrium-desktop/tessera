---
id: ADR-0177
title: "Dock Transit Spring Unification, Anti-Oscillation Dismiss Latching, and Non-Preemptible Swapchain Drain"
status: accepted
date: 2026-09-30
scope: shell/dock, motion, presentation, damage
superseded_by: null
negative_knowledge: true
---

# 0177. Dock Transit Spring Unification, Anti-Oscillation Dismiss Latching, and Non-Preemptible Swapchain Drain

- Status: Accepted
- Date: 2026-09-30
- Deciders: Tessera Maintainers & Shell Core Architects
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md), [ADR-0174](0174-universal-model-motion-view-chrome-architecture.md), [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md)

---

## Context and Problem Statement

Following the release of v0.0.72, users identified two severe, compounding desktop shell defects:

1. **Hardware Cursor Severe Stuttering & Page-Flip Congestion**: Moving the pointer across client windows or desktop surfaces suffered acute lag, frame dropping, and stuttering.
2. **Dock Retract Limit-Cycle Oscillation & Ghost Scissor Artifact**: When an autohidden Dock was expanded and the user clicked outside or focused a window, the Dock began to collapse but immediately entered an infinite limit-cycle oscillation ("chattering" animation every ~150ms). Visually, a sharp 90° dark rectangular cutout cleaved into the dock area displaying the collapsed stadium handle and desktop wallpaper, while the historical expanded dock bar remained frozen in the surrounding background, flickering across swapchain rotations.

Despite prior stabilization attempts in [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md) and [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), the defect recurred under standard desktop usage.

---

## Root Cause Analysis

Thorough architectural investigation across `tessera`, `tessera-shell`, and `optics::transit` isolated four systemic root causes:

### 1. Mid-Collapse Dynamic Hitbox Re-Capture (Limit-Cycle Oscillation)
Under ADR-0146, `pointer_keeps_revealed` evaluated `live_panel_contains(cursor, current_panel)` while `0.001 < reveal < 0.999`. When a user clicked outside or focused a window near the bottom edge:
- The click initiated an animated collapse (`autohide_idle = autohide_timeout`).
- The moment `reveal` dropped from `1.0` to `0.98`, the morphing panel bounds were still large.
- The user's cursor fell inside the shrinking `current_panel`, causing `pointer_keeps_revealed` to return `true` without intent dwell verification.
- `autohide_idle` was reset to `0.0`, reversing `target_reveal` back to `1.0`.
- Once re-expanded to `1.0`, `expanded_trigger_contains` evaluated to `false`, triggering `AUTOHIDE_QUICK_DISMISS_TIMEOUT` (150ms) and initiating collapse again.
- This created an infinite limit-cycle oscillation at 150ms intervals.

### 2. Secondary Animation Preemption of the Swapchain Drain Ring
In [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), `step_autohide_motion` primed `settled_drain_frames = 3` in `prepare_backdrop`. However, `Dock::render` retained legacy settle logic:
```rust
let is_animating = self.anim_active;
if was_animating && !is_animating {
    self.settled_drain_frames = 3;
} else if !is_animating && self.settled_drain_frames > 0 {
    self.settled_drain_frames -= 1;
} else if is_animating {
    self.settled_drain_frames = 0; // ★ FATAL PREEMPTION
}
```
If a secondary animation (such as an icon magnification spring settling, tooltip alpha fade, or dwell tick) remained active, `is_animating` was `true`, instantly wiping out `settled_drain_frames = 0`. Consequently, subsequent swapchain slots never received maximum footprint damage, rendering dynamic Vulkan scissors only around the collapsed capsule and freezing the historical expanded dock in the remaining framebuffer area.

### 3. Asymptotic Exponential Follow without Physical Settle Edge
The autohide reveal used `approach(reveal, target, 12.0, dt, reduced)`. Because an exponential follow approaches its asymptote without an intrinsic physical settle boundary, it depended on an arbitrary float cutoff (`abs() < 0.002`). Under fluctuating frame pacing, numerical jitter around this boundary caused settle detection and drain triggers to drop or desynchronize.

### 4. DRM/KMS Cursor Atomic Flip Congestion
Continuous compositor animation frames submitted page flips on every refresh cycle. When DRM/KMS has an atomic primary-plane page flip in-flight, out-of-band non-blocking cursor plane commits return `EBUSY`. The cursor was forced into the compositor's VBLANK render schedule, and because `captures_pointer_at` returned `true` during the oscillation, `cursor_plane_only` was disabled, turning all mouse motion into full compositor redraw commands.

---

## Decision Outcome

We enact three binding architectural solutions and complete the Model-Motion-View (MMV) refactoring of `tessera-shell::components::dock`:

### 1. Anti-Rebound Dismiss Latch (`dismiss_latched`)
When the Dock begins an explicit collapse (click-outside Escape hatch, Escape key press, window obscurity, or retreat vector), `dismiss_latched` is set to `true`.
- While `dismiss_latched` is active, `pointer_keeps_revealed` and `capsule_entry` unconditionally return `false`.
- The collapse proceeds without interruption to `0.0`.
- `dismiss_latched` clears ONLY after the reveal spring completely settles at `0.0` and the cursor has left the collapsed indicator area.

### 2. Parity with ADR-0176: Reveal Motion on Critically Damped `transit::Spring`
We eliminate `approach` for autohide reveal, migrating to a critically damped `transit::Spring`:
- `DOCK_REVEAL_SPRING: SpringParams = SpringParams::new(320.0, 1.0)`.
- Physical settle condition `spring.settled(target, 0.002, 0.02)` provides a deterministic rest edge.
- Settle management is centralized exclusively in `prepare_backdrop`.
- On the settle frame, `reveal.snap_to(target)` and `settled_drain_frames = 3` are primed.

### 3. Non-Preemptible Swapchain Ring Drain
`settled_drain_frames` is managed strictly in `prepare_backdrop`:
- It is never cleared or preempted by secondary tile or tooltip animations in `render()`.
- While `settled_drain_frames > 0`, `anim_pending()` reports `true` and damage is reported over `capture_footprint`.
- All 3 swapchain slots overwrite transient geometry with the clean resting desktop.

### 4. Full MMV Architecture
Conforming to [ADR-0174](0174-universal-model-motion-view-chrome-architecture.md):
- `motion.rs`: Owns `DockMotion`, `DOCK_REVEAL_SPRING`, `DOCK_TILE_SPRING`, and `transit` dynamics.
- `state.rs`: Owns `DockState`, pure headless state machine, and geometry math.
- `rendering.rs`: Pure presentation projection into Lens Frame and glass passes.
- `mod.rs`: `Dock` Chrome facade delegating cleanly to state and motion.

---

## Invariants & Behavioral Boundaries

- `[INV-ARCH-55] Dock Dismiss Latch Integrity`: An autohiding dock initiated into collapse by an explicit dismiss command or obscurity transition MUST latch its collapse and ignore all dynamic panel hover corridors until the surface has settled at `reveal = 0.0`.
- `[INV-ARCH-56] Non-Preemptible Settle Drain`: Settle drain counters primed to purge multi-buffered swapchains MUST NOT be reset to zero by secondary animations (tooltips, icon springs) in subsequent render passes.
- `[INV-ARCH-57] Zero-Approach Reveal Motion`: Chrome reveal transitions must delegate exclusively to `transit::Spring` with critically damped parameters, forbidding asymptotic `.approach()` loops.

---

## Rejected Alternatives & Negative Knowledge

### Retaining `approach` with Tighter Epsilon
- **Why rejected**: As shown in ADR-0176, narrowing epsilon in an exponential follow merely shifts the truncation point without solving the mathematical lack of a physical velocity rest edge.

### Forcing Full Redraws on All Dock Collapse Frames
- **Why rejected**: Defeated compositor dynamic scissoring, wasting GPU bandwidth and battery power on 120Hz/144Hz displays.

---

## Consequences

### Positive
- Zero limit-cycle oscillation upon clicking outside or launching applications from the Dock.
- Zero ghost scissors or rectangular wallpaper cutouts in any swapchain slot.
- KMS cursor plane commits cleanly out-of-band at 1000Hz with zero cursor stuttering.
- Clean Model-Motion-View separation with 100% headless state testability.
