---
id: ADR-0171
title: "Chrome Animation Pre-Pass Synchronization and Surface Placement Invariants"
status: accepted
date: 2026-09-28
scope: shell, layout, animation, presentation, damage
superseded_by: null
negative_knowledge: true
---

# 0171. Chrome Animation Pre-Pass Synchronization and Surface Placement Invariants

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md), [ADR-0169](0169-control-center-bento-grid-and-mpris-absorption.md)

---

## Context and Problem Statement

Two distinct visual and spatial anomalies were identified during desktop shell usage:

1. **Dock Post-Animation Flickering & Ghost Scissor Artifact**: Following an autohide collapse or spring-settling animation, a prominent dark rectangle with 90° sharp corners cuts into the dock area, revealing the desktop wallpaper and collapsed capsule handle inside, while the old expanded dock bar remains frozen in the surrounding background. This artifact flickers between clean and ghosted states upon pointer movement or swapchain rotation.
2. **Control Center Quick Controls Layout Drift**: In the Control Center, all 2D Bento Grid toggles (Wi-Fi, Bluetooth, MPRIS Now Playing, DND, Dark Mode) and sliders (Brightness, Sound, Keyboard Backlight) drifted out of the central Main Panel into the screen's top-left corner `(0, 0)`, colliding directly with the User Persona avatar block while leaving the right half of the Main Panel completely blank.

Investigating both issues across the boundary between `Optics` (`flux`, `prism`, `lens`) and `tessera` revealed deep architectural synchronization gaps in chrome placement and multi-buffering damage assessment.

---

## Decision Drivers

- **Zero Tolerance for Visual Glitches**: Shell chrome must maintain flawless perceptual continuity across all animation states, resting transitions, and swapchain presentations.
- **Architectural Boundary Decoupling**: Optics provides generic vector UI, canvas recording, and material compilation; the compositor runtime owns the timing, damage accumulation, and presentation pipeline. Responsibilities must not bleed or be bypassed.
- **Immediate-Mode UI Predictability**: Flow containers must strictly inherit their container anchors and never escape to the global root origin.
- **Ring Buffer Damage Correctness**: Vulkan dynamic rendering scissors (`render_area`) depend on historical damage tracking. Any frame presenting partial damage must guarantee that no stale pixels linger in any slot of the swapchain ring (`FLUX_MAX_FRAMES_IN_FLIGHT = 3`).

---

## Root Cause Analysis

### 1. Control Center Grid Unanchored Root Escapes

Under ADR-0169, Quick Controls was refactored into a native 4-column Bento Grid via `f.grid(4)`. However, `render_quick_controls_section` received the target `area: Rect` but failed to open a placed surface container (`f.place`).

In the Lens UI architecture (Optics ADR-0028, ADR-0060), containers outside any active placed surface are added as children of the root frame's default flex flow, whose origin is fixed at `(0, 0)`. Consequently, the entire Bento Grid was laid out at screen `(0, 0)`, creating layout drift and collision with the top-left persona card.

### 2. Dock Animation Stepping vs. Damage Assessment Phase Inversion

In the compositor's presentation loop (`crates/tessera/src/runtime/presentation.rs`), frame generation follows a strict sequence:
1. `prepare_backdrop`: Query chrome pre-passes and resolve next-frame geometries.
2. `assess_frame_damage`: Poll `shell.anim_pending()` to determine whether chrome animations are in-flight; if so, union `damage_region` (`capture_footprint`) into the output damage.
3. Compute `output_render_area`: Scissor the Vulkan render pass to the bounding box of `repaint`.
4. `render`: Execute `shell.render()`.

Previously, `Dock` deferred its autohide progress stepping, idle timer progression, and settle detection (`was_animating && !is_animating`) to `render()` at step 4. When the dock completed its collapse, `(target - self.autohide_reveal).abs() <= 0.002` was reached before `render()` ran. As a result:
- At step 2, `assess_frame_damage` queried `anim_pending()`, which evaluated to `false` because `settled_drain_frames` had not yet been primed by `render()`.
- The compositor assessed the frame as having **zero dock damage** (or only localized cursor damage).
- The frame was rendered with an `output_render_area` restricted to a tiny rectangle around the cursor or collapsed indicator.
- Vulkan cleared and redrew wallpaper and the collapsed handle **only inside that tiny scissor**, leaving the historical expanded dock in the swapchain slot's framebuffer outside the scissor!
- When `render()` finally executed, it primed `settled_drain_frames = 3`, but this slot had already been committed with partial damage. When the swapchain ring rotated back to this un-purged slot, the expanded ghost flashed on screen.

---

## Decision Outcome

We enact three binding architectural invariants across `tessera-shell` and `tessera`:

### Invariants & Behavioral Boundaries

- `[INV-ARCH-01] Chrome Placement Enclosure`: Every container and sub-view rendered by a chrome component must be enclosed within an explicitly positioned placed surface (`f.place`), ensuring that no flow layout children escape into the global root flow at `(0, 0)`.
- `[INV-ARCH-02] Animation Pre-Pass Settle Integrity`: All chrome components undergoing geometric morphs, spring transitions, or reveal animations MUST advance their motion states and prime swapchain drain counters in `prepare_backdrop`. Pre-pass execution guarantees that `anim_pending` and `damage_region` are fully synchronized BEFORE `assess_frame_damage` computes the Vulkan scissor/render area.
- `[INV-ARCH-03] Swapchain Ring Purge Coverage`: When any chrome animation settles, the component MUST maintain `anim_pending() = true` and report its maximum capture footprint for exactly `FLUX_MAX_FRAMES_IN_FLIGHT` (3) frames, ensuring that every slot in the swapchain ring overwrites historical geometry with the final resting state.

### Implementation Specifics

1. **`tessera-shell::control_center`**:
   `render_quick_controls_section` now encloses its Bento Grid inside `f.place("tessera-hud-quick-controls-grid", &chrome_place(area, transparent()), |f| { f.column_ex(&sized(area.w, area.h), |f| { f.grid(4)... }) })`. All tiles and sliders reside strictly within the right content well of the Main Panel.
2. **`tessera-shell::dock`**:
   Autohide reveal progression, dwell timing, and settle detection are consolidated into `step_autohide_motion`, invoked during `prepare_backdrop`. The settle transition primes `settled_drain_frames = 3` before `assess_frame_damage`, guaranteeing that all 3 ring slots receive full `capture_footprint` damage and purge transient footprints completely.

---

## Rejected Alternatives & Negative Knowledge

### Discarded: Auto-Wrapping Top-Level Widgets in `optics::lens`
- **Why considered**: `GridBuilder` in `lens` could implicitly attempt to detect if it is being called without an enclosing container and synthesize an absolute placement.
- **Why rejected**: Violated the core mandate of Optics as a zero-assumption, general-purpose vector UI toolkit. Lens applications outside Tessera rely on root-level flow containers starting at `(0, 0)`. Injecting compositor-specific chrome placement logic into `optics` would introduce unmaintainable architectural coupling.

### Discarded: Disabling Partial Scissor / Dynamic Render Areas
- **Why considered**: Forcing full-screen repaints (`render_area = None`) on every frame would eliminate dirty-rect leaking.
- **Why rejected**: Massive performance degradation. Restricting Vulkan render passes and memory bandwidth to dirty rects is critical for meeting 120Hz/144Hz frame budgets and low-power battery consumption. The correct solution is strict damage assessment accuracy, not abandoning scissoring.

### Discarded: Deferring Settle Draining to Event Loop Polling
- **Why considered**: Polling a global timer in `event_loop.rs` to force damage after animations stop.
- **Why rejected**: Flaky and race-prone. Frame pacing and swapchain acquisition are driven by presentation fences and ring rotation. The component owning the animated geometry is the sole authority on when motion has rested.

---

## Consequences

### Positive
- Quick Controls Bento Grid and MPRIS Now Playing render cleanly anchored inside the Main Panel with zero top-left layout drift.
- Dock collapse, reveal, and magnification animations settle smoothly without ghosted expanded borders, sharp rectangular wallpaper cutouts, or swapchain ring flickering.
- Zero frame desync between `damage_region`, `liquid_glass_region`, and `render`.

### Negative / Trade-offs
- Chrome components must conscientiously execute motion progression during `prepare_backdrop` rather than lazily during `render`. Tests must account for pre-pass execution when validating damage state machines.
