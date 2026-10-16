# Chrome Animation, Backdrop Coherence, and Dock State-Machine Triage

Engineering know-how and diagnostic guide for attributing and preventing frame drops, liquid-glass ghosting, swapchain flickering, and interaction state-machine deadlocks across `tessera-shell`, `tessera`, and `optics` (see [ADR-0171](../../adr/0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0177](../../adr/0177-dock-transit-spring-unification-and-anti-oscillation-latch.md), [ADR-0179](../../adr/0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md), [ADR-0180](../../adr/0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md), and [ADR-0181](../../adr/0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md)).

---

## 1. System Invariants (The 10 Golden Rules)

### Rule 1: Pre-Pass Synchronization (`prepare_frame` Authority, `[INV-ARCH-64]`)
Every chrome component that declares backdrop effects (`liquid_glass_regions`, `backdrop_regions`), live window thumbnails (`live_preview_presentation`), or localized animation damage (`damage_region`) **must advance all geometry-affecting springs and offscreen GPU jobs during `Chrome::prepare_frame`**, never lazily inside `render()`.

- `Chrome::prepare_frame` / `Shell::prepare_frame` is the **only** pre-pass seam. There is no `prepare_backdrop`. Two spellings for one seam silently disable it for whichever spelling a component implements — that alias was removed in ADR-0181 after it left `ScreenshotSelector`, `Dock`, and `Hud` pre-passes dead (region screenshots could not be dragged, Dock autohide ran one frame behind).
- **Why**: The compositor render loop executes in strict order:
  1. `shell.prepare_frame(&input)`
  2. `assess_frame_damage(...)` (queries `shell.anim_pending()` and `shell.anim_damage_region()`)
  3. `shell.backdrop_layers(...)` & `shell.live_preview_presentations()`
  4. Offscreen backdrop capture + blur + Prism liquid-glass compute dispatch
  5. Main output pass + `shell.render(...)`
- **Failure Mode**: If tile magnification springs (`Dock::sizes`) or VRM portrait advances (`ControlCenter::avatar`) run inside `render()`, the backdrop pass samples **Frame T-1** geometry while the Lens foreground pass draws **Frame T** geometry, producing a visible 1-frame double-edge ghosting (`重影`) or interleaving offscreen queue submissions inside an open output render pass.
- **Single layout source**: `Dock::resolve_strip_geometry` produces the bar length, tile centres, panel rect, and icon rects for *both* the pre-pass and `render`. Do not re-inline that math in either caller — a second copy is how the pre-pass and `render` end up disagreeing about where a tile is. `Dock::apply_pending_order` runs in both so an optimistic reorder commit is visible to the pre-pass.


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

### Rule 7: Occlusion Elision Requires a Dead Scene (`[INV-ARCH-65]`)
`crates/tessera/src/runtime/presentation.rs` may skip `begin_desktop_output` and draw the shell into a single clearing stencil pass **only** when `shell.occludes_desktop()` is true *and* the desktop scene has no live consumer:

```rust
let desktop_occluded = shell_requires_composition
    && self.shell.occludes_desktop()
    && !capture_active
    && !self.screenshot_freeze.active()
    && !session_locked
    && window_switcher.is_none()
    && live_previews.is_empty();
```

- `desktop_occluded` is the conjunction of an occluder assertion and an empty
  consumer set; each term is the guard for one consumer class:

```rust
let desktop_occluded = shell_requires_composition
    && self.shell.occludes_desktop()
    && !capture_active          // declared backdrop/glass (participants filter)
    && !overview_active         // overview thumbnails + desktop snapshot
    && !self.shell.screenshot_active()  // interactive picker scrim over desktop
    && !self.screenshot_freeze.active() // frozen desktop snapshot image
    && !session_locked
    && window_switcher.is_none()
    && live_previews.is_empty();
```

- `Chrome::occludes_desktop` is asserted by the occluder itself (only `ControlCenter`, and only once `canvas_alpha(reveal) == 255`); it is never inferred from aggregate chrome alpha.
- `!capture_active` is the real guard for declared glass: it derives from the same `participates_in_shell_pass` filter that feeds `Shell::backdrop_layers`, so any component declaring frost/glass keeps the base pass.
- **Failure Mode**: Eliding the base pass while *any* scene consumer is live renders uninitialized pixels (or the clear colour) where the desktop should be — a black or torn panel, not a subtle artefact. A new chrome surface that draws scene-derived content outside `backdrop_layers` / `live_preview_presentation` must be added to the predicate above, and the invariant's consumer list extended with it.

### Rule 8: Cursor Plane Single Writer (`[INV-ARCH-66]`)
`crates/tessera/src/runtime/event_loop.rs` runs the out-of-band `present_pointer_cursor` commit **only while `!presentation.can_redraw()`** (a previous primary-plane flip is still in flight). With the scheduler gate open, `render_and_present` performs the `cursor_only_eligible` cursor commit or the combined primary+cursor atomic commit.

- The removed `cursor_plane_piggyback` flag issued a non-blocking cursor commit *and* let the render transaction commit the cursor in the same frame. The first commit occupies the CRTC's single non-blocking commit slot, so the primary flip immediately after it returns `EBUSY`, enters the 1 ms micro-retry loop, and overwrites buffers faster than the display drains them (`撕裂`, `闪烁`).
- **Failure Mode**: Two cursor-plane writers per frame look like "cursor stutter over modal chrome" turning into full-frame tearing.

### Rule 9: Scanout-Before-Present Frame Ownership (`[INV-ARCH-67]`)
`crates/tessera-platform/src/drm/device.rs` (`DrmBackend::present`) must export the dma-buf, import and commit scanout, and only then call `frame.present()`. If `commit_scanout` fails or returns `EBUSY`, the `SubmittedFrame` is abandoned (Optics `flux_frame_abandon`, Optics ADR-0110) so `current_frame` does not advance.

- **Failure Mode**: Calling `frame.present()` first rotates the offscreen ring into the buffer currently under scanout, so the retry composites into a live buffer (torn frame); dropping the frame instead of abandoning it wedges the surface permanently (`flux_surface_begin_frame` returns `FLUX_ERROR_INVALID_STATE` forever).

### Rule 10: Freeze Capture and Exact Cache Geometry (`[INV-ARCH-68]`, `[INV-ARCH-69]`)
- `assess_frame_damage` includes `screenshot_freeze.needs_capture()` in `base_full`: the frame that freezes the desktop for a screenshot must assess as full damage even when it carries no other input.
- `BackdropCacheKey::new` stores `capture_origin` / `capture_extent` **unquantized**. `capture_origin` is baked into the capture contents via `session.translate(-origin * capture_ratio, …)` and `capture_extent` parameterizes `capture_ratio` and `draw_cached`'s `u_span`; quantizing either offsets the glass body or reintroduces the `[INV-ARCH-61]` `capture_ratio` distortion.
- **Failure Mode**: A 64 px origin floor or 128 px extent bucket makes the cached composite draw content captured for one origin at another, and stretches the glass relative to the unscaled `draw_cached` crop — the same `重影` as Rule 2.


---

## 2. Quick Symptom-to-Root-Cause Matrix

| Observed Symptom | Primary Suspect | Where to Inspect |
|------------------|-----------------|------------------|
| Severe frame drops during Control Center open/close or HUD fade | Per-node `save_layer` pass explosion in `optics`, or mid-fade `BackdropCacheKey` invalidation | `../optics/libs/lens/src/render/replay.c` (`lensi_compile_node_body`), `crates/tessera-shell/src/components/hud/hud.rs` (`liquid_glass_regions`) |
| Dock glass background appears wider/taller or shifted relative to the Dock border (`重影`) | `BackdropCapture::size` returning padded 128px bucket dimensions instead of logical content dimensions, `BackdropCacheKey` quantizing `capture_origin`/`capture_extent`, or `Dock::sizes` stepped in `render()` instead of `prepare_frame()` | `crates/tessera/src/runtime/rendering.rs` (`BackdropGraphExecutor::prepare`, `BackdropCacheKey::new`), `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`, `resolve_strip_geometry`) |
| Old magnified Dock tiles, tooltips, or preview cards flicker every 2–3 frames on cursor movement (`闪烁`) | `Dock::anim_pending()` returning `false` during magnification/hover drain, or `hover_damage_rect` not unioned into `Dock::damage_region()` for 3 drain frames | `crates/tessera-shell/src/components/dock/rendering.rs` (`anim_pending`, `damage_region`, `record_hover_surface_damage`) |
| Region screenshot selection cannot be dragged; Enter/Space closes the picker with no capture (`截图不可用`) | `ScreenshotSelector::prepare_backdrop` never dispatched (`Chrome::prepare_frame` default consumed the call), so `update_pointer` never ran and `confirmed` stayed `None` | `crates/tessera-shell/src/component/mod.rs` (`Chrome::prepare_frame`), `crates/tessera-shell/src/chrome/screenshot.rs` (`prepare_frame`, `update_pointer`) |
| Frame freezes for a screenshot but captures a stale/blank desktop | `assess_frame_damage` `base_full` missing `screenshot_freeze.needs_capture()` | `crates/tessera/src/runtime/damage.rs` (`base_full`) |
| Full-output tear/flicker while moving the cursor over a modal panel | `cursor_plane_piggyback` committing the cursor out-of-band *and* in the render transaction, forcing the primary flip into `EBUSY` retries | `crates/tessera/src/runtime/event_loop.rs` (`present_pointer_cursor` gate), `crates/tessera/src/runtime/input.rs` (`FrameState`) |
| Tear on the frame after an `EBUSY` atomic-flip retry | `frame.present()` called before `commit_scanout`, rotating the offscreen ring into the buffer under scanout | `crates/tessera-platform/src/drm/device.rs` (`DrmBackend::present`), `../optics/libs/flux/src/core/frame.c` (`flux_frame_abandon`) |
| Wallpaper and client windows composited beneath an open Control Center | `desktop_occluded` predicate missing `occludes_desktop` or a live scene consumer | `crates/tessera/src/runtime/presentation.rs` (`desktop_occluded`), `crates/tessera-shell/src/lib.rs` (`Shell::occludes_desktop`) |
| Dock collapses when a window maximizes or overlaps it, and can never be expanded again | `collapse_pending` not cleared when `is_settled && target_reveal == 0.0`, or `hidden_trigger_armed` not re-armed when pointer leaves handle | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
| Dock starts expanding after 180ms dwell, then immediately collapses halfway up when moving cursor toward icons | `expanding` state not including `expanded_trigger_contains(rest_bounds)` in `over_dock_trigger`, causing 0.15s `AUTOHIDE_QUICK_DISMISS_TIMEOUT` to fire during the 0.25s spring flight | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
