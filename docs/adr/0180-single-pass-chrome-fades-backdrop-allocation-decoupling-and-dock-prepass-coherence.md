---
id: ADR-0180
title: "Single-Pass Chrome Fades, High-Water Backdrop Allocation Decoupling, and Dock Pre-Pass Coherence"
status: accepted
date: 2026-10-14
scope: shell/dock, shell/hud, shell/control_center, rendering, presentation, damage
superseded_by: null
negative_knowledge: true
---

# 0180. Single-Pass Chrome Fades, High-Water Backdrop Allocation Decoupling, and Dock Pre-Pass Coherence

- Status: Accepted
- Date: 2026-10-14
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0029](0029-animation-and-effect-policy.md), [ADR-0142](0142-layered-glass-backdrop-compositor.md), [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md), [ADR-0163](0163-decoupled-backdrop-blur-cache-and-continuous-fade.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0177](0177-dock-transit-spring-unification-and-anti-oscillation-latch.md), [ADR-0178](0178-optics-transit-smoothstep-unification-and-chrome-prepass-lifecycle.md), [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md)
- Extends: Optics [ADR-0109](https://github.com/ming2k/optics/blob/main/docs/adr/0109-premultiplied-command-opacity-scissor-bounded-layers-and-glyph-run-batching.md)
- Amended by: [ADR-0181](0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md) — the pre-pass method named `prepare_backdrop` in the Decision below is `prepare_frame` (the alias was removed); `[INV-ARCH-61]`–`[INV-ARCH-63]` remain in force.

---

## Context and Problem Statement

End-to-end profiling of desktop shell transitions and interactive Dock gestures across `tessera` and `optics` identified two categories of critical defects:

1. **Severe Frame Drops During Control Center Entry/Exit and HUD Proximity Fades**:
   - **Per-Node Offscreen Pass Explosion**: `ControlCenter::render`, `Hud::render`, and `Dock::render` use `Frame::set_opacity(alpha)` (`0.0 < alpha < 1.0`) to fade subtrees. In `optics`, `lensi_compile_node_body` wrapped every individual node with `opacity < 1.0` in `flux_encoder_save_layer`, and `vk_save_layer` performed a full-screen `RGBA16_SFLOAT` render-pass clear and full-screen quad composite per node (~200 full-screen passes per frame during Control Center transitions).
   - **HUD Partial-Fade Backdrop Reallocation Thrashing**: In `Hud::liquid_glass_regions`, filtering out individual chips as soon as `chip_fade <= 0.01` while the sibling chip remained visible mutated `BackdropCacheKey` mid-fade, invalidating all 3 swapchain slots' scene captures and triggering 18 GPU image reallocations when crossing 128px bucket boundaries.
   - **Control Center Mid-Pass VRM Submission**: `ControlCenter::render` invoked `avatar.advance(dt)` while the main output stencil overlay pass and deferred upload batch (`device.uploads_begin()`) were open, interleaving offscreen queue submissions with active output pass recording.

2. **Dock Trigger Deadlocks, Double-Edge Ghosting, and Swapchain Flickering**:
   - **Bucketed Backdrop Coordinate Distortion (`ADR-0179` Regression)**: `BackdropGraphExecutor::prepare` stored the 128px-bucketed allocation size in `BackdropCapture::size` and returned it from `capture_size(slot)`. `presentation.rs` divided `capture_size` by the unbucketed `capture_extent`, scaling the Dock's captured scene and Prism `LiquidGlassGroup` by `bucket_size / capture_extent` (e.g., `1280 / 1192 = 1.0738×` horizontally and `128 / 76 = 1.684×` vertically), while `draw_cached` cropped the unscaled `capture_extent` sub-rectangle onto the output—producing a persistent shifted double-image (`重影`) behind the Dock.
   - **Post-Obscurity Collapse Deadlock**: When a window overlapped the Dock (`dock_obscured = true`) or maximized (`SpaceUse::Maximized`), `collapse_pending` was set to `true`. However, `step_autohide_motion` guarded `self.collapse_pending = false` at `reveal == 0.0` behind `if !self.dock_obscured && self.space_use != SpaceUse::Maximized`, which is false by definition whenever a window is obscuring or maximized. Once collapsed, `collapse_pending` remained `true` forever, permanently blocking `step_hidden_reveal` and preventing the Dock from ever expanding again.
   - **Mid-Expansion Quick-Dismiss Abort**: Expanding from `reveal = 0.0` to `1.0` on `DOCK_REVEAL_SPRING` (`ω₀² = 320`) takes ~0.25s, whereas `AUTOHIDE_QUICK_DISMISS_TIMEOUT` is 0.15s. Because `pointer_keeps_revealed` for `reveal < 0.999` only checked the morphing `current_panel` and the 4px collapsed handle (excluding `rest_bounds`), moving the pointer slightly upward into `rest_bounds` to click an emerging icon exited `current_panel`, started the 0.15s timer, and collapsed the Dock mid-expansion.
   - **Pre-Pass Magnification Desync & Split Animation State**: `Dock` and `DockMotion` had duplicate `anim_active` fields (`self.motion.anim_active` was never updated), tile magnification springs (`self.sizes`) were advanced in `render()` *after* `liquid_glass_regions()` had already sampled the previous frame's widths, and dismissed tooltips/live-preview popovers cleared `hover_surface_bounds` without draining their damage region across the 3 swapchain slots.

---

## Decision

1. **Decouple Logical Content Size from High-Water Bucketed Allocation Size in `BackdropGraphExecutor`**:
   - `BackdropCapture` separates `size` (the exact logical content extent `capture_extent / BACKDROP_DOWNSAMPLE`, returned by `capture_size(slot)` for 1:1 coordinate mapping) from `allocated_size` (the 128px-bucketed high-water mark GPU texture extent used by `recompute_effects_internal`, `draw_cached`, and `draw_capture_opaque`).
   - `target_stale` only reallocates when the requested bucket exceeds `allocated_size` (capped at the bucketed `physical_size`), eliminating texture destruction/reallocation when regions shrink or fade out.

2. **Stabilize HUD Liquid-Glass Capture Bounds and Pre-Pass Control Center Avatar**:
   - While `Hud::requires_composition()` is true, `Hud::liquid_glass_regions` retains all layout-visible chips with explicit `.with_capture_bounds(bounds)` and clamps `opacity` to `0.0` when `fade <= 0.01`, preventing single-chip proximity fades from mutating `BackdropCacheKey`.
   - `ControlCenter::prepare_frame` advances animated VRM portraits during the pre-pass before any output canvas pass or upload batch opens.

3. **Uncompromised Dock Pre-Pass Coherence, Trigger Unblocking, and Hover Damage Draining**:
   - **Unblock Obscured/Maximized Re-Expansion**: In `Dock::step_autohide_motion`, reaching settled rest at `target_reveal == 0.0` unconditionally clears `self.collapse_pending = false` (while `dock_obscured` and `space_use` continue tracking window state), and any pointer position outside `collapsed_indicator_contains` immediately re-arms `hidden_trigger_armed = true`.
   - **Expansion Corridor Continuity**: While the Dock is actively expanding (`autohide_reveal > 0.001 && autohide_idle < idle_timeout && !collapse_pending && !dismiss_latched`), `expanded_trigger_contains(rest_bounds)` keeps the reveal alive so pointer entry into the emerging icon strip never aborts expansion.
   - **Pre-Pass Magnification & Unified Drain**: `step_autohide_motion` (invoked in `prepare_backdrop`) advances both `autohide_reveal` and `self.sizes` before `liquid_glass_regions` and `damage_region` are queried, synchronizes `self.motion.anim_active = self.anim_active`, and primes `SETTLED_DRAIN_FRAMES` (`3`) when either reveal or magnification settles.
   - **Hover Surface Damage Drain**: `Dock` tracks `hover_damage_rect` and `hover_drain_frames` (`3`) across tooltip and live-preview movement/dismissal, unioning the padded popover footprint into `damage_region()` until all swapchain slots are purged.
   - **Hardware Cursor Fast-Path Restoration**: `assess_frame_damage` in `crates/tessera/src/runtime/damage.rs` emits swept cursor damage rectangles only when `software_cursor` is active, allowing hardware-cursor pointer motion outside chrome to take the zero-composite `present_cursor()` fast path.

---

## Invariants & Behavioral Boundaries

- `[INV-ARCH-61] Backdrop Content vs. Allocation Extent Separation`: Bucketed render-target padding MUST NEVER participate in coordinate scaling (`capture_ratio`); shader and region mappings must use the unpadded logical capture extent while UV spans normalize against `allocated_size`.
- `[INV-ARCH-62] Single-Trip Collapse Pending Lifecycle`: `collapse_pending` guards only the single initial collapse transition into `reveal = 0.0` upon obscurity/maximization and MUST clear once `reveal = 0.0` settles so subsequent user dwell intent can reveal the Dock.
- `[INV-ARCH-63] Hover Popover Swapchain Drain`: Any transient chrome popover (tooltip or live preview) rendered outside its parent bar's `capture_footprint` MUST retain its damage rectangle for `FLUX_MAX_FRAMES_IN_FLIGHT` frames after dismissal.

---

## Rejected Alternatives & Negative Knowledge

### 1. Stretching Backdrop Captures to Fill the Padded 128px Bucket
- **Why considered**: Letting `capture_ratio = allocated_size / capture_extent` stretch the captured scene to fill the entire bucketed texture avoids computing `u_span = raw_w / img_w` during `draw_cached`.
- **Why rejected**: Non-uniform X/Y bucket padding ratios distort the aspect ratio of isotropic Gaussian blur kernels and SDF corner radii in `prism`, causing visible refraction misalignment and double-edge ghosting.

### 2. Keeping `collapse_pending = true` for the Entire Duration of Window Obscurity
- **Why considered**: Prevents any accidental Dock expansion while a window sits near the bottom edge.
- **Why rejected**: Completely breaks the core autohide UX contract when tiled or maximized windows are present—the collapsed stadium handle becomes permanently unresponsive until all windows are closed or moved away. `step_hidden_reveal` (180ms deliberate dwell on the handle) and `dismiss_latched` already prevent accidental skims and rebound oscillations.

---

## Consequences

### Positive
- Control Center enter/exit and HUD fade animations execute in a single Vulkan overlay render pass with zero per-node `save_layer` interruptions and zero mid-fade GPU texture reallocations.
- Eliminates Dock double-edge liquid-glass ghosting, post-animation swapchain flickering, mid-expansion collapse aborts, and post-obscurity expansion deadlocks.
- Hardware cursor motion over client windows commits directly to the KMS cursor plane without triggering redundant primary-plane composites.

### Negative / Trade-offs
- High-water mark backdrop textures retain their peak bucketed allocation (up to `physical_size`) across the session rather than shrinking when transient regions hide.
