---
id: ADR-0210
title: "Performance-Core Affinity Allocation, Zero-Copy Strided Uploads, and Quiescent Lens Lifecycle"
status: accepted
date: 2026-11-03
scope: scheduling, rendering, composite, shell, optics-interop, architecture
superseded_by: null
negative_knowledge: true
---

# 0210. Performance-Core Affinity Allocation, Zero-Copy Strided Uploads, and Quiescent Lens Lifecycle

- Status: Accepted
- Date: 2026-11-03
- Deciders: Tessera Maintainers
- Amends: [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0196](0196-compositor-side-presentation-time-feedback.md), [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md), [ADR-0204](0204-frame-scheduling-reserves-a-runqueue-the-desktop-does-not-use.md)

---

## Context and Problem Statement

On high-refresh, high-resolution hardware (3072×1920 @ 120 Hz, Intel Core Ultra 9 285H Arrow Lake hybrid processor), the compositor presented severe performance regression under normal client workloads (e.g., interactive terminal editing in `foot`):

1. **CPU Saturation & Watchdog Violations**:
   The compositor main thread pegged at 100 % single-core load, triggering repeated watchdog stalls (`frame render/presentation took 127ms exceeding 100ms budget`) and input event starvation (`libinput error: client bug: event processing lagging behind by 94ms, your system is too slow`).
2. **Terminal Internal Latency & Starvation**:
   Typing and scrolling inside `foot` exhibited extreme latency and stutter. Frame callbacks (`wl_surface.frame` / `wl_callback.done`) were delayed by over 100 ms per cycle.
3. **Regional Damage Redraw Degraded to Full-Screen Repaints**:
   Incremental damage intended to redraw only changed terminal cells degraded into whole-frame GPU composites, saturating GPU memory bandwidth across double-pass RGBA16F intermediate blits.

Systemic root-cause analysis across `tessera` and the underlying `optics` engine isolated four compounding architectural defects:

### 1. Inverted Heterogeneous CPU Pinning (Amending ADR-0204)
[ADR-0204](0204-frame-scheduling-reserves-a-runqueue-the-desktop-does-not-use.md) correctly identified that CPU 0 carries legacy IRQ routes, but naively selected `available.iter().max()` as the dedicated frame CPU under the assumption that the highest-numbered CPU is an idle E-core.

On modern hybrid architectures (such as Intel Arrow Lake / Meteor Lake), the highest-numbered cores are **Low-Power Island SoC E-cores** (Crestmont cores 14 and 15, capped at 2.5 GHz with a synthetic CPU capacity score of 294 out of 1024). In contrast, CPUs 0–5 are Lion Cove Performance Cores (5.4 GHz, capacity 1024). ADR-0204 therefore pinned the latency-critical 120 Hz realtime frame loop to the slowest, lowest-IPC execution core on the die, causing complete execution saturation.

### 2. Lens Unlocalized Damage Rect Poisoning (Amending ADR-0203)
When any widget in `lens` undergoes an animated ease, Optics' `lens_damage_rect(ui)` computes the visual dirty bounding box. When internal deltas cannot be localized to a single node, `lens_damage_rect` falls back to `ui->root->final_rect` (`context.c:498`), returning the full-screen layout viewport.

Tessera's `anim_damage_regions` folded this fallback directly into the damage list. On an idle desktop with a static window, a minor hover ease or blink in a HUD or Dock widget expanded the output damage to the entire 3072×1920 output, forcing full GPU re-compositing and destroying partial repaint benefits.

### 3. Damage Degradation on Off-Screen Rectangles (Amending ADR-0203)
In `crates/tessera-composite/src/presentation/damage.rs`, `ClientDamage::into_frame_damage` and `logical_rects_to_frame` treated `logical_to_physical` returning `None` as a fatal failure, unconditionally degrading the entire frame to `FrameDamage::Full`.

However, `logical_to_physical` returns `None` whenever a rectangle lies entirely outside the physical framebuffer (e.g. an off-screen surface, or a window shadow skirt extending past a display boundary). An off-screen rectangle contributes zero physical pixels to the display. Degrading to a full-screen repaint because an off-screen rect touches no pixels directly inverted damage tracking goals.

### 4. CPU Staging Memcpy Storm on SHM Damage (Amending ADR-0179)
For Wayland SHM surfaces (e.g., `foot`), incremental damage updates performed a manual row-by-row CPU copy in Rust into an allocated `shm_staging` vector before calling `img.update_region`. On high-resolution displays, this CPU-bound staging loop executed thousands of slice copies per second, adding significant memory bus contention on the frame thread while Optics already provided a native `flux_image_update_region_strided` DMA interface.

---

## Decision Drivers

- **Maximum Performance P-Core Allocation**: Realtime frame scheduling must allocate full-capacity Performance Cores, strictly excluding Low-Power SoC Islands.
- **Zero Poisoning of Workspace Damage**: Persistent decoration animations (HUD, Dock) must never poison the central client workspace into full-output repaints.
- **Off-Screen Rect Neutrality**: Damage outside the framebuffer boundaries touches zero pixels and must never cause full-frame degradation.
- **Zero-Copy Host DMA**: SHM incremental updates must route directly to GPU staging via strided transfer APIs without CPU-side row-by-row staging buffers.

---

## Architectural Decisions

### 1. Heterogeneous Capacity-Aware CPU Selection (`[INV-ARCH-95]`)
In `crates/tessera-sched/src/lib.rs`, `preferred_frame_cpu` discovers Linux Energy Aware Scheduling (EAS) capacity via `/sys/devices/system/cpu/cpu{N}/cpu_capacity`, falling back to `/sys/devices/system/cpu/cpu{N}/cpufreq/cpuinfo_max_freq`:
- Excludes CPU 0 (preserving the legacy IRQ avoidance from ADR-0204).
- Filters out low-power island cores (capacity < 500 or max frequency < 60 % of die maximum).
- Selects the lowest-indexed Performance Core (e.g., CPU 1 with 1024 capacity at 5.4 GHz).

### 2. Persistent Decoration Damage Clamping (`[INV-ARCH-98]`)
In `crates/tessera-shell/src/lib.rs`, `Shell::anim_damage_regions` inspects the footprint returned by `self.ui.damage_rect()`:
- If `lens_damage` spans the full output viewport, but no modal or immersive component is active, the unlocalized root fallback is intercepted.
- Damage is constrained to the active persistent decoration strips (top HUD strip and bottom Dock strip).
- The central workspace (where client windows like `foot` reside) remains completely isolated from Lens UI easing noise.

### 3. Off-Screen Rectangle Discard Discipline (`[INV-ARCH-99]`)
In `crates/tessera-composite/src/presentation/damage.rs`:
- `ClientDamage::into_frame_damage` and `logical_rects_to_frame` only degrade to `FrameDamage::Full` when the coordinate system itself is invalid (non-finite or non-positive scale, or zero physical dimensions).
- Individual rectangles returning `None` from `logical_to_physical` (zero intersection with the physical output) are cleanly discarded without corrupting the frame damage verdict.

### 4. Direct Strided SHM Uploads (`[INV-ARCH-100]`)
In `crates/tessera-composite/src/lib.rs`:
- Removed `shm_staging: Vec<u8>` from `Renderer`.
- Replaced the CPU row-by-row extraction loop with `img.update_region_strided(x0, y0, bw, bh, stride, &f.pixels[start..])`.
- Uploads transfer directly to GPU staging buffers via Optics' optimized C kernel.

### 5. Optics Lens Dirty-Band Traversal and Root Fallback Gating (`[INV-ARCH-101]`)
In `../optics/libs/lens`:
- `lensi_mark_dirty` in `src/render/replay.c` traverses all placed bands (`ui->bands[b][i]`) so `lensi_mark_subtree_changed` computes `subtree_changed` and updates `visual_revision` for placed Chrome/Popup nodes.
- `lens_damage_rect` in `src/core/context.c` unions ghost bounds and only falls back to `ui->root->final_rect` when the root itself was actually modified.

### 6. Redundant Secondary Stencil Overlay Pass Elision (`[INV-ARCH-102]`)
In `crates/tessera-shell/src/lib.rs` and `crates/tessera/src/runtime/presentation.rs`:
- Implemented `chrome_intersects_render_area`: when physical `output_render_area` does not touch any participating shell chrome (e.g., terminal editing in the central workspace), the compositor avoids ending `base_session` and completely skips the secondary `begin_stencil_frame_overlay` pass.
- Eliminates two full-screen blits (Swapchain -> `linear` decode and `linear` -> Swapchain encode) and multiple pipeline barriers per terminal frame.

### 7. Quiescent Shell Scene Rebuild Elision (`[INV-ARCH-103]`)
In `crates/tessera-shell/src/lib.rs`:
- `prepare_frame` enforces a dirty-state gate over `self.dirty`, cursor motion, mouse/key input, and active animations.
- When chrome is quiescent and a presentable artifact already exists in `pending_scene`, redundant `ui.frame` traversals, text shaping, and DisplayList compilations are strictly elided.
- Eradicated dead legacy `Shell::render` method, achieving a clean break without backward compatibility baggage.

---

## Invariants & Behavioral Boundaries

- `[INV-ARCH-95] (Amended) Dedicated frame CPU is a high-capacity Performance Core.` The frame thread must never run on CPU 0 (IRQ core) and must never run on a Low-Power SoC Island (E-core score below threshold).
- `[INV-ARCH-98] Persistent decoration animations never poison client workspaces.` Unlocalized Lens root damage rects must split into persistent decoration bands when no immersive component is active.
- `[INV-ARCH-99] Off-screen damage contributes zero physical rects.` An off-screen or clipped-out rectangle must never degrade damage tracking to `FrameDamage::Full`.
- `[INV-ARCH-100] SHM incremental uploads are strided and zero-staging-copy.` Tightly packed SHM sub-rectangles must upload via `update_region_strided` without host-side intermediate staging vectors.
- `[INV-ARCH-101] Lens dirty tracking traverses all placed band nodes.` `lensi_mark_dirty` must visit `ui->bands` so placed subtrees accurately calculate visual revision and damage bounds.
- `[INV-ARCH-102] Chrome overlay pass is skipped when render area does not intersect chrome.` Stencil overlay sessions are elided when `chrome_intersects_render_area` reports false.
- `[INV-ARCH-103] Quiescent Shell Scene Elision.` When no input, animation, or model mutations touch shell chrome, `prepare_frame` must not recompile DisplayLists or drop unpresented snapshots.

---

## Negative Knowledge

- **Never assume the highest CPU index is a fast core**: In modern heterogeneous SoCs (Intel Arrow Lake, Meteor Lake, Snapdragon X Elite, Apple Silicon), the highest-indexed cores are often ultra-low-power efficiency cores located on the SoC island rather than the compute die. Always consult hardware topology or capacity registers.
- **Never treat `logical_to_physical == None` as an unmappable error**: When a damage rect is outside the screen, returning `None` indicates empty intersection, not an invalid mathematical transform. Demoting zero pixels to full damage destroys regional compositing.
