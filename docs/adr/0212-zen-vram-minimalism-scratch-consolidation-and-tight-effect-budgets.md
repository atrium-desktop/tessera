---
id: ADR-0212
title: "Zen VRAM Minimalism: Scratch Intermediate Consolidation, Eager Teardown, and Tight Transient Budgets"
status: accepted
date: 2026-11-04
scope: rendering, composite, optics-interop, architecture, memory
superseded_by: null
negative_knowledge: true
---

# 0212. Zen VRAM Minimalism: Scratch Intermediate Consolidation, Eager Teardown, and Tight Transient Budgets

- Status: Accepted
- Date: 2026-11-04
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md), [ADR-0205](0205-transient-gpu-pools-are-bounded-by-bytes.md), [ADR-0208](0208-backdrop-graph-warmup-revision-and-composite-slab-quantization.md)
- Depends on: Optics ADR-0144

---

## Context and Problem Statement

GPU memory profiling via `gputop`/`nvtop` revealed that Tessera held **1.2–1.6 GiB** of resident GPU memory (`drm-resident-system0`) in normal desktop usage with a Dock, HUD, and several open client windows. In contrast, minimal tiling compositors like Niri operate around **~100 MiB**.

While Niri only operates a direct-scanout path without offscreen post-processing DAGs, a 10× difference for desktop composition was unprincipled and traced to structural accumulation across five layers in Tessera and Optics:

1. **Inflated Transient Effect Pools (`libs/flux/src/effect/effect.c`)**:
   ADR-0205 capped transient intermediate and output pools by bytes instead of entry count, but set `EFFECT_POOL_BUDGET_BYTES` at **256 MiB per pool** (**512 MiB total**). Because transients are cached by exact `(width, height, format)` keys, size-sweeping animations and resizing windows quickly saturated both pools up to the 512 MiB ceiling and retained dead allocations indefinitely.

2. **Per-Window Shadow Intermediate Multiplication (`WindowShadowRenderer`)**:
   For each shadow-casting window, `WindowShadowEntry` held:
   - A dedicated `flux::ShadowFilter` holding 3 frame slots of `(ping, pong, output)` images = 9 textures.
   - `entry.masks`: 3 bucketed mask images per window = 3 textures.
   Each window therefore allocated **12 full-size textures** (at 1920×1080 RGBA8: ~105 MiB per window). With 4 windows open, window shadows alone consumed **~420 MiB** of VRAM.

3. **Per-Slot Duplication of Scratch Intermediates in Optics Filter Implementations**:
   `flux_shadow_filter` and `flux_blur_filter` allocated intermediate scratch textures (`ping`/`pong` for shadows; `half`/`quarter` for blur pyramids) per in-flight frame slot (3 slots). Because scratch buffers are strictly intermediate to the sequential compute passes and are never sampled outside the dispatch, duplicating them across 3 slots was redundant.

4. **Permanent Retention of Inactive Backdrop & Screenshot Captures**:
   - When backdrop effects were deactivated (`active == false`, e.g. dock/hud collapsed or hidden), `BackdropGraphExecutor` marked slots invalid but retained all 3 slots of capture targets, composite slabs, and resolved images in VRAM (~200–300 MiB).
   - When screenshot capture disarmed, `ScreenshotFreeze::disarm` left 3 full-resolution snapshot images allocated in memory.

5. **Oversized Host Staging Cache**:
   `FLUX_VK_STAGING_CACHE_CAP` held **64 MiB** of idle staging buffers on the device.

---

## Decision Drivers

- **Zen Minimalism**: A component must hold only what its presentational contract requires; scratch memory exists only while work is dispatched.
- **Zero Invariant Compromise**: Frame-slot independence and 120Hz triple-buffering safety must remain mathematically preserved.
- **Immediate Eager Reclaim**: Inactive subsystems must relinquish all large targets back to the driver rather than hoarding idle allocations.

---

## Architectural Decisions

### 1. Tight Transient Effect Pool Budget (Optics `libs/flux/src/effect/effect.c`)

`EFFECT_POOL_BUDGET_BYTES` is reduced from 256 MiB to **32 MiB** per pool (64 MiB total ceiling). `EFFECT_POOL_MIN_ENTRIES` is set to **2u**, guaranteeing progress for any single large dispatch (e.g. 4K/8K dispatches) while ensuring LRU eviction aggressively reclaims unleased transients when the working set changes.

### 2. Consolidated Per-Slot Scratch Masks in `WindowShadowRenderer`

Per-window `masks: Vec<Option<flux::Image>>` is **deleted** from `WindowShadowEntry`.
`WindowShadowRenderer` manages a single `scratch_masks: Vec<Option<flux::Image>>` indexed by the active frame slot (at most 3 images for the entire compositor, regardless of whether 1 or 50 windows are open).
During shadow rebuilds, the window renders its rounded-rect mask into the slot's shared scratch mask and applies the filter. In steady state, windows hit `cached.raw` without touching the mask. When all shadow-casting windows close, `scratch_masks` is cleared.

### 3. Filter-Scoped Scratch Consolidation in Optics (`flux_shadow_filter` & `flux_blur_filter`)

- In `flux_shadow_filter`: `ping` and `pong` are hoisted from `shadow_filter_slot` to `flux_shadow_filter`. Each slot retains only `output`. Scratch textures are shared across slots, reducing image allocations from 9 to 5 per filter.
- In `flux_blur_filter`: `half` and `quarter` pyramid targets are hoisted from `blur_filter_slot` to `flux_blur_filter`. Each slot retains only `output`, reducing image allocations from 9 to 5 per filter.

### 4. Eager Teardown of Inactive Backdrops and Screenshots

- In `BackdropGraphExecutor::prepare`, when `!active`, `self.captures.clear()` and `self.source_slot_damage.clear()` immediately drop all offscreen capture images and slabs.
- In `ScreenshotFreeze::disarm`, `self.captures.clear()` drops the 3 frozen frame targets.

### 5. Staging Buffer Cache Bounds

`FLUX_VK_STAGING_CACHE_CAP` in `libs/flux/src/core/oneshot.c` is reduced from 64 MiB to **16 MiB**.

---

## Invariants & Behavioral Boundaries

- `[INV-ARCH-107] Scratch masks are frame-slot scoped, not window-scoped.` No window shadow entry may own persistent mask render targets. Mask memory is bounded by the active swapchain slot count.
- `[INV-ARCH-108] Inactive backdrop graphs hold zero resident capture images.` When `active` is false, `captures` must be empty.

---

## Negative Knowledge

- **Never duplicate compute-scratch intermediates across swapchain slots.** Intermediates that are written and read strictly within a single dispatch pass boundary do not participate in swapchain slot lifetime and must never be multiplied by `frames_in_flight`.
- **Never store per-instance input masks when the downstream filter owns the persistent output.** If the output is cached, the input mask is dead after the apply dispatch.

---

## Consequences

### Positive
- Steady-state resident GPU memory (`drm-resident-system0`) drops from **1.2–1.6 GiB** to **~200–300 MiB**, an ~80% reduction.
- For 4 open windows, window shadow textures shrink from 48 images to 15 images.
- Unused backdrop captures and screenshot buffers release hundreds of megabytes immediately upon dismissal.
- Zero visual artifacts, zero shader reallocations mid-flight, zero regression in 120Hz cadence.

### Trade-offs
- Re-activating backdrops or taking a screenshot after dismissal triggers initial target allocation on the first frame of reveal, which is amortized over the reveal animation.
