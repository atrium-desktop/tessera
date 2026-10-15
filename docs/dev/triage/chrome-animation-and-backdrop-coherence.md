# Chrome Animation, Backdrop Coherence, and Dock State-Machine Triage

Engineering know-how and diagnostic guide for attributing and preventing frame drops, liquid-glass ghosting, swapchain flickering, and interaction state-machine deadlocks across `tessera-shell`, `tessera`, and `optics` (see [ADR-0171](../../adr/0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0177](../../adr/0177-dock-transit-spring-unification-and-anti-oscillation-latch.md), [ADR-0179](../../adr/0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md), and [ADR-0180](../../adr/0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md)).

---

## 1. System Invariants (The 6 Golden Rules)

### Rule 1: Pre-Pass Synchronization (`prepare_frame` / `prepare_backdrop` Authority)
Every chrome component that declares backdrop effects (`liquid_glass_regions`, `backdrop_regions`), live window thumbnails (`live_preview_presentation`), or localized animation damage (`damage_region`) **must advance all geometry-affecting springs and offscreen GPU jobs during `prepare_frame` / `prepare_backdrop`**, never lazily inside `render()`.

- **Why**: The compositor render loop executes in strict order:
  1. `shell.prepare_frame(&input)`
  2. `assess_frame_damage(...)` (queries `shell.anim_pending()` and `shell.anim_damage_region()`)
  3. `shell.backdrop_layers(...)` & `shell.live_preview_presentations()`
  4. Offscreen backdrop capture + blur + Prism liquid-glass compute dispatch
  5. Main output pass + `shell.render(...)`
- **Failure Mode**: If tile magnification springs (`Dock::sizes`) or VRM portrait advances (`ControlCenter::avatar`) run inside `render()`, the backdrop pass samples **Frame T-1** geometry while the Lens foreground pass draws **Frame T** geometry, producing a visible 1-frame double-edge ghosting (`重影`) or interleaving offscreen queue submissions inside an open output render pass.

### Rule 2: Logical Content Extent vs. Bucketed Allocation Extent (`[INV-ARCH-61]`)
When offscreen render targets (`BackdropCapture`) are rounded up to 128-pixel buckets (`flux::bucket_extent(raw, 128)`) or retained at a high-water mark to avoid `vkCreateImage` / `vkAllocateMemory` thrashing:

- `BackdropCapture::size` (returned by `capture_size(slot)`) **must store the unpadded logical content extent** (`capture_extent / BACKDROP_DOWNSAMPLE`).
- `BackdropCapture::allocated_size` **must store the physical padded texture dimensions**.
- **Failure Mode**: Returning the padded bucket dimensions from `capture_size(slot)` causes `capture_ratio = capture_size / capture_extent` to become non-unit (e.g., `1280 / 1192 = 1.0738×` in X and `128 / 76 = 1.684×` in Y), stretching the Prism liquid-glass shape and blur regions relative to the unscaled `draw_cached` UV crop (`u_span = raw_w / img_w`).

### Rule 3: Stable Capture Footprints Across Partial Opacity Fades
When a component hosts multiple glass bodies (such as the HUD's left status chip and center workspace chip), individual bodies fading to `0.0` while sibling bodies remain visible **must not drop their `capture_bounds` from `liquid_glass_regions`**.

- Keep visible chips in `liquid_glass_regions` with `.with_capture_bounds(bounds)` and clamp `opacity` to `0.0` once `fade <= 0.01`.
- `glass_region_active(region)` (`opacity > 0.0`) automatically skips Prism compute dispatch for the transparent chip while keeping `BackdropCacheKey` invariant across single-chip proximity fades.

### Rule 4: Swapchain Ring Draining Across All Animated Surfaces (`[INV-ARCH-56]`, `[INV-ARCH-63]`)
With triple-buffered swapchains (`FLUX_MAX_FRAMES_IN_FLIGHT = 3`) and partial scissor rendering (`render_area`), any animation settling to rest—or any transient popover (Dock tooltip or live-preview panel) closing outside its parent's `capture_footprint`—**must prime a 3-frame drain counter** (`settled_drain_frames = 3` / `hover_drain_frames = 3`) and keep unioning its footprint into `damage_region()` and reporting `anim_pending() == true` until all 3 slots have been overwritten.

- **Failure Mode**: Clearing `hover_surface_bounds = None` immediately on tile exit without draining its damage rectangle leaves the closed tooltip or preview card burned into 2 of the 3 swapchain buffers, causing it to flash every 3rd frame whenever the Dock animates (`闪烁`).

### Rule 5: Single-Trip `collapse_pending` & Expansion Corridor Continuity (`[INV-ARCH-62]`)
In the Dock state machine (`crates/tessera-shell/src/components/dock/`):

1. `collapse_pending` exists solely to ensure **one uninterrupted collapse to `reveal = 0.0`** when a window first obscures the Dock or maximizes. Once `is_settled && target_reveal == 0.0`, `collapse_pending` **must unconditionally reset to `false`** (while `dock_obscured` and `space_use` remain `true`) so subsequent user dwell on the collapsed stadium handle can re-open the Dock.
2. Whenever the pointer is outside `collapsed_indicator_contains`, `hidden_trigger_armed` must reset to `true` regardless of the current `autohide_reveal` value.
3. While the Dock is actively expanding (`autohide_reveal > 0.001 && autohide_idle < idle_timeout && !collapse_pending && !dismiss_latched`), `expanded_trigger_contains(rest_bounds)` must keep the reveal alive. Otherwise, moving the pointer upward into the emerging icon strip exits the still-small morphing `current_panel`, triggers the `0.15s` quick-dismiss timeout before the `~0.25s` reveal spring finishes, and aborts the expansion mid-flight.

### Rule 6: Hardware Cursor Independence in `assess_frame_damage`
In `crates/tessera/src/runtime/damage.rs`, pointer-only motion (`input_pointer_only`) must only emit swept cursor damage rectangles (`cursor_rect`) when `software_cursor` is active. When the hardware cursor plane is used and the pointer is neither over chrome nor driving a chrome animation, `output` damage must resolve to `FrameDamage::None` so `render_and_present` takes the `cursor_only_eligible` KMS cursor-plane fast path.

---

## 2. Quick Symptom-to-Root-Cause Matrix

| Observed Symptom | Primary Suspect | Where to Inspect |
|------------------|-----------------|------------------|
| Severe frame drops during Control Center open/close or HUD fade | Per-node `save_layer` pass explosion in `optics`, or mid-fade `BackdropCacheKey` invalidation | `../optics/libs/lens/src/render/replay.c` (`lensi_compile_node_body`), `crates/tessera-shell/src/components/hud/hud.rs` (`liquid_glass_regions`) |
| Dock glass background appears wider/taller or shifted relative to the Dock border (`重影`) | `BackdropCapture::size` returning padded 128px bucket dimensions instead of logical content dimensions, or `Dock::sizes` stepped in `render()` instead of `prepare_backdrop()` | `crates/tessera/src/runtime/rendering.rs` (`BackdropGraphExecutor::prepare`), `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
| Old magnified Dock tiles, tooltips, or preview cards flicker every 2–3 frames on cursor movement (`闪烁`) | `Dock::anim_pending()` returning `false` during magnification/hover drain, or `hover_damage_rect` not unioned into `Dock::damage_region()` for 3 drain frames | `crates/tessera-shell/src/components/dock/rendering.rs` (`anim_pending`, `damage_region`, `record_hover_surface_damage`) |
| Dock collapses when a window maximizes or overlaps it, and can never be expanded again | `collapse_pending` not cleared when `is_settled && target_reveal == 0.0`, or `hidden_trigger_armed` not re-armed when pointer leaves handle | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
| Dock starts expanding after 180ms dwell, then immediately collapses halfway up when moving cursor toward icons | `expanding` state not including `expanded_trigger_contains(rest_bounds)` in `over_dock_trigger`, causing 0.15s `AUTOHIDE_QUICK_DISMISS_TIMEOUT` to fire during the 0.25s spring flight | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
