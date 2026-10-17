---
id: ADR-0179
title: "Zero-Redundancy Presentation Pipeline, Sub-stepped Motion Dynamics, and Attachment Bucketing for 120Hz Animation Continuity"
status: accepted
date: 2026-10-06
scope: presentation, rendering, damage, motion, optics-interop, scheduler
superseded_by: null
negative_knowledge: true
---

# 0179. Zero-Redundancy Presentation Pipeline, Sub-stepped Motion Dynamics, and Attachment Bucketing for 120Hz Animation Continuity

- Status: Accepted
- Date: 2026-10-06
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0139](0139-animation-effect-placement.md), [ADR-0142](0142-layered-glass-backdrop-compositor.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0177](0177-dock-transit-spring-unification-and-anti-oscillation-latch.md), [ADR-0178](0178-optics-transit-smoothstep-unification-and-chrome-prepass-lifecycle.md)
- Extends: Optics ADR-0108 (Motion Sub-stepping and Render Target Bucket Allocation)
- Amended by: [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md) — window-shadow outputs are reused per frame slot keyed by window-local `(size, scale, activated)`, and occluded windows no longer record shadow/blur passes at all.

---

## Context and Problem Statement

Extensive performance testing on 60Hz and 120Hz/144Hz displays revealed severe, acute system stuttering and frame dropping during desktop animations (particularly during window minimize Genie flights, window open/close transitions, Dock reveal/magnification, and Control Center modal entry/exit). Profiling the compositor render loop and the Optics interface isolated four compounding architectural bottlenecks:

1. **Backdrop Cache Invalidation Overreach & Double-Draw Storm**:
   `assess_frame_damage` treated any in-flight window transition as global desktop damage (`self.server.transitions_pending() => backdrop_full = true`). This unconditionally forced `BackdropPlan::Refresh`, executing an offscreen desktop scene capture pass (`begin_capture` + `draw_client_scene`), multi-pass Dual-Kawase blur compute passes, Prism Liquid Glass passes, and immediately thereafter executing `draw_direct_desktop_scene` in the main output pass—**rendering the full client window list and background twice per frame**.
2. **Dynamic Geometry Sweeps & Vulkan Render Target Allocation Thrashing**:
   During window transitions and geometry sweeps, offscreen blur extents changed continuously. Because `BackdropGraphExecutor` and `WindowShadowRenderer` lacked extent bucketing, texture allocations were recreated every frame via `vkCreateImage` / `vkAllocateMemory`. Furthermore, `WindowShadowRenderer` maintained only a single mask image per frame slot, causing multiple shadow-casting windows to repeatedly destroy and reallocate each other's mask textures within the same frame.
3. **Atomic KMS Flip Congestion & 16.6ms EBUSY Retry Penalty**:
   When the compositor submitted continuous primary-plane atomic flips during animations, transient driver locks returned `-EBUSY` to asynchronous cursor or fallback commits. In response, `event_loop.rs` delayed retries by an entire `refresh_interval` (16.6ms), dropping whole refresh intervals and causing frame rates to collapse from 60fps to 30fps.
4. **Motion Integrator Discrete Leap on Cadence Hiccups**:
   Single-step spring integration under fluctuating frame delta times caused the harmonic oscillator to jump discontinuously across large displacement steps, producing visually jarring teleports following temporary load spikes.

---

## Decision Drivers

- **Zero-Redundant Double Draws**: Every scene element must be composited at most once per presented frame unless explicitly blurred.
- **Zero Allocations During Active Motion**: Size-sweeping animations and geometry morphs must never invoke `vkCreateImage` or `vkAllocateMemory` mid-flight.
- **Sub-Millisecond Non-Blocking Retries**: Transient driver page-flip lockups must not forfeit entire display refresh cycles.
- **Physical Trajectory Continuity**: Harmonic springs must preserve smooth continuous velocity and displacement integration across timing jitter.

---

## Decision Outcome

We enact four binding architectural solutions across `tessera`, `tessera-composite`, `tessera-wayland`, and `tessera-shell`:

### 1. Localized Transition Damage Tracking
- `Server::transition_damage_rects` calculates the exact bounding union rectangles of all active window geometry transitions (closing ghosts, minimize flights, and open animations).
- `damage.rs` stops treating `transitions_pending()` as an unconditional `backdrop_full = true`. Individual window transitions contribute their exact damage bounds to `backdrop_source`.
- As long as an animating window does not intersect a declared backdrop capture region (HUD or Dock), `backdrop_refresh_regions` returns empty, the backdrop cache remains `Cached`, and the entire offscreen capture pass, blur compute dispatch, and redundant scene drawing are completely eliminated.

### 2. Attachment Bucketing & Multi-Window Shadow Isolation
- Adopt Optics ADR-0108 `flux::bucket_extent(val, 128)`.
- `BackdropGraphExecutor::prepare` buckets capture dimensions to 128-pixel boundaries. `draw_cached` and `draw_capture_opaque` sample sub-rectangles via `draw_image_sub` and `draw_image_opaque_sub`, keeping allocations static while clipping exactly to geometric boundaries.
- `WindowShadowRenderer` replaces single-slot masks with `HashMap<(WindowId, usize), flux::Image>` bucketed to 128px, eliminating cross-window mask destruction and per-frame memory thrashing.

### 3. Sub-Millisecond Retry Pacing for Driver Congestion
- In `event_loop.rs`, `PresentationOutcome::Retry` replaces the 16.6ms delay with a 1ms micro-retry (`retry_at(now + Duration::from_millis(1))`).
- Transient `EBUSY` flips retry as soon as the previous vblank latch releases, preserving 60Hz and 120Hz refresh cadence without frame skipping.

### 4. Bounded Motion Sub-Stepping
- Adopt Optics ADR-0108 `transit_spring_advance_substep` / `Spring::advance_substep`.
- Re-export `spring_advance_substep` in `tessera-shell::widgets::motion` and integrate into Dock autohide, tile magnification, and Control Center reveal dynamics with a 1/60s sub-step clamp, ensuring mathematically continuous trajectory integration.

---

## Invariants & Behavioral Boundaries

- `[INV-ARCH-58] Zero-Allocation Geometry Sweeps`: Dynamic offscreen render targets and filter masks subjected to time-varying geometric sweeps MUST be bucketed to at least 128-pixel multiples to guarantee zero memory reallocations mid-animation.
- `[INV-ARCH-59] Localized Transition Damage Invalidation`: Window transitions (open, close, minimize, restore) MUST express their motion as localized bounding damage rectangles and MUST NOT force full-output backdrop invalidations unless they physically intersect a declared backdrop sampling region.
- `[INV-ARCH-60] Non-Dropping Driver Congestion Backoff`: Driver atomic commit retries triggered by transient `EBUSY` page-flip states MUST NOT delay the retry deadline by more than 2 milliseconds.

---

## Rejected Alternatives & Negative Knowledge

### 1. Bypassing Backdrop Blur Entirely on Low-End GPUs
- **Why considered**: Disabling all blur filters avoids GPU compute loads completely.
- **Why rejected**: Compromises the visual identity and aesthetic standard of Tessera and Optics Prism. The root cause was redundant full-screen double draws and unbucketed memory allocations, not the compute shaders themselves.

### 2. Caching Direct Desktop Scene into a Persistent Full-Screen Intermediate
- **Why considered**: Keep a full-screen intermediate texture of all windows and blit it to both capture and swapchain.
- **Why rejected**: Incurs an extra full-screen copy pass and additional memory bandwidth on mobile/iGPU architectures. Localizing transition damage avoids the offscreen pass altogether in the common case.

---

## Consequences

### Positive
- Eliminates redundant client scene double-draws and compute shader dispatches during window animations when windows do not touch chrome blur regions.
- Eliminates mid-animation `vkCreateImage` and `vkAllocateMemory` calls through 128px render-target bucketing and multi-window shadow isolation.
- Eradicates the 30fps drop caused by 16.6ms `EBUSY` delay penalties.
- Smooths out motion trajectory under irregular frame delivery through sub-stepping.

### Negative / Trade-offs
- Bucketed render targets allocate slightly more memory (rounded up to the next 128px step) than tight exact-fit allocations.
