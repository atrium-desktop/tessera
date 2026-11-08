# Chrome Animation, Frame-Clock, and Dock State-Machine Triage

Engineering know-how and diagnostic guide for attributing and preventing frame
drops, ghosting, swapchain flickering, and interaction state-machine deadlocks
across `tessera-shell` and `tessera` (see
[ADR-0171](../../adr/0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md),
[ADR-0177](../../adr/0177-dock-transit-spring-unification-and-anti-oscillation-latch.md),
[ADR-0179](../../adr/0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md),
[ADR-0180](../../adr/0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md),
[ADR-0181](../../adr/0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md),
and [ADR-0217](../../adr/0217-idle-chrome-does-not-keep-the-frame-clock-and-hover-damage-is-bounded-by-the-component-that-owns-it.md)).

> **Scope note.** This guide described the analytic-glass era until ADR-0213. Every
> rule that referenced a capture, a blur cache key, or a material declaration has
> been removed rather than reworded: those mechanisms no longer exist, so a rule
> about them can only mislead. Chrome draws painted fills with a painted halo.

---

## 1. System Rules

### Rule 1: Pre-Pass Synchronization (`prepare_frame` Authority, `[INV-ARCH-64]`)
Every chrome component that publishes live window thumbnails
(`live_preview_presentation`), localized animation damage (`damage_regions`), or
pointer-hover damage (`pointer_damage_regions`) **must advance all
geometry-affecting state during `Chrome::prepare_frame`**, never lazily inside
`render()`.

- `Chrome::prepare_frame` / `Shell::prepare_frame` is the **only** pre-pass seam.
  There is no `prepare_backdrop`. Two spellings for one seam silently disable it
  for whichever spelling a component implements — that alias was removed in
  ADR-0181 after it left `ScreenshotSelector`, `Dock`, and `Hud` pre-passes dead
  (region screenshots could not be dragged, Dock autohide ran one frame behind).
- **Why**: the compositor render loop executes in strict order:
  1. `shell.prepare_frame(&input)`
  2. `assess_frame_damage(...)` — queries `shell.anim_pending()` and
     `shell.chrome_damage_regions(...)`
  3. `shell.live_preview_presentations()`
  4. Window-shadow effect passes, at a pass boundary
  5. Main output pass + `shell.render(...)`
- **Failure Mode**: if tile magnification springs (`Dock::sizes`) or VRM portrait
  advances (`ControlCenter::avatar`) run inside `render()`, step 2 localizes the
  frame from **Frame T-1** geometry while step 5 paints **Frame T** geometry. The
  repainted band is then one frame stale: a visible double edge (`重影`) at the
  leading edge of whatever moved, or a stale strip where a popover used to be.
- **Single layout source**: `Dock::resolve_strip_geometry` produces the bar
  length, tile centres, panel rect, and icon rects for *both* the pre-pass and
  `render`. Do not re-inline that math in either caller — a second copy is how
  the pre-pass and `render` end up disagreeing about where a tile is.
  `Dock::apply_pending_order` runs in both so an optimistic reorder commit is
  visible to the pre-pass.

### Rule 2: An Animation Predicate Must Be Able to Become False (`[INV-ARCH-56]`)
Anything that drives the frame clock — `Chrome::anim_pending`, a component's own
`anim_active` — must reach `false` with no input, or the compositor repaints at
the panel refresh rate forever.

- **The failure this rule exists for.** An eased value stored in an integral
  layout type is quantised every frame, and "still moving" derived from the
  quantised value never becomes false: the blend resolves anything within the
  rounding step to the target, the next step lands one grid unit off it, and the
  pair is a fixed point. The window switcher did exactly this and held the frame
  clock up for as long as it was open (ADR-0217).
- **The rule.** Animation state is floating point. Quantise where you *paint*,
  not where you *store*. Compare against the target with a tolerance that
  matches the paint's own resolution (the switcher uses half a pixel) and let the
  curve converge.
- **The other form of the same bug.** A monotone accumulator
  (`anim_active |= …`, `settled_drain_frames` folded into `anim_pending`) latches
  on and never clears. `settled_drain_frames` is swapchain-ring *damage*
  bookkeeping: it belongs in the damage footprint and must never gate the clock.
- **Failure Mode**: a still surface costs a full-output composite per refresh
  interval, and everything inside it re-samples its live client buffers. This is
  the single largest steady-state cost a flat-chrome compositor can have, and it
  is invisible in a profile that only looks at draw calls — the draws are cheap,
  there is just one set per vblank forever.

### Rule 3: Damage Is Bounded by Whoever Knows the Bound (`[INV-ARCH-65]`)
A component reports its own repaint regions through `Chrome::damage_regions`
(animations and structural changes) and
`Chrome::pointer_damage_regions` (a pointer move over it). `None` means "cannot
localize" and stays a full-output repaint.

- The host cannot know where a component's hover reaction lands; the component
  can. The default is `None`, and it must stay the default — inventing a band is
  the guess ADR-0211 removed.
- Under-covering is the only unsafe direction. Over-covering a band is safe.
- A gesture that leaves the surface it started on (**a drag**, an edge-snap) must
  report `None`, not the resting footprint.
- A component whose body fades must report the painted halo until the fade
  reaches zero opacity, then stop reporting it.

### Rule 4: Swapchain Ring Draining Across All Animated Surfaces (`[INV-ARCH-63]`)
With triple-buffered swapchains (`FLUX_MAX_FRAMES_IN_FLIGHT = 3`) and regional
`render_area` repaints, an animation settling to rest — or a transient popover
(Dock tooltip or live-preview panel) closing — **must prime a 3-frame drain
counter** (`settled_drain_frames = 3` / `hover_drain_frames = 3`) and keep
unioning the vacated footprint into `damage_regions()` until all three slots have
been overwritten.

- The drain counter drives **damage**, never `anim_pending`: a counter in the
  clock predicate re-primes itself on the frame it requests (Rule 2).
- **Failure Mode**: clearing `hover_surface_bounds = None` immediately on tile
  exit without draining its damage rectangle leaves the closed tooltip or preview
  card burned into 2 of the 3 swapchain buffers, so it flashes every third frame
  whenever the Dock animates (`闪烁`).

### Rule 5: Single-Trip `collapse_pending` & Expansion Corridor Continuity (`[INV-ARCH-62]`)
In the Dock state machine (`crates/tessera-shell/src/components/dock/`):

1. `collapse_pending` exists solely to ensure **one uninterrupted collapse to
   `reveal = 0.0`** when a window first obscures the Dock or maximizes. Once
   `is_settled && target_reveal == 0.0`, `collapse_pending` **must
   unconditionally reset to `false`** (while `dock_obscured` and `space_use`
   remain `true`) so subsequent dwell on the collapsed stadium handle can re-open
   the Dock.
2. Whenever the pointer is outside `collapsed_indicator_contains`,
   `hidden_trigger_armed` must reset to `true` regardless of the current
   `autohide_reveal` value.
3. While the Dock is actively expanding (`autohide_reveal > 0.001 &&
   autohide_idle < idle_timeout && !collapse_pending && !dismiss_latched`),
   `expanded_trigger_contains(rest_bounds)` must keep the reveal alive.
   Otherwise, moving the pointer upward into the emerging icon strip exits the
   still-small morphing `current_panel`, triggers the `0.15 s` quick-dismiss
   timeout before the `~0.25 s` reveal spring finishes, and aborts the expansion
   mid-flight.

### Rule 6: Hardware Cursor Independence in `assess_frame_damage`
In `crates/tessera/src/runtime/damage.rs`, pointer-only motion
(`input_pointer_only`) must only emit swept cursor damage rectangles
(`cursor_rect`) when `software_cursor` is active. When the hardware cursor plane
is used and the pointer is neither over chrome nor driving a chrome animation,
`output` damage must resolve to `FrameDamage::None` so `render_and_present` takes
the `cursor_only_eligible` KMS cursor-plane fast path.

### Rule 7: Occlusion Elision Requires a Dead Scene (`[INV-ARCH-65]`)
`crates/tessera/src/runtime/presentation.rs` may skip `begin_desktop_output` and
draw the shell into a single clearing stencil pass **only** when
`shell.occludes_desktop()` is true *and* the desktop scene has no live consumer.

`desktop_occluded` is the conjunction of an occluder assertion and an empty
consumer set; each term is the guard for exactly one consumer class:

```rust
let desktop_occluded = shell_requires_composition
    && self.shell.occludes_desktop()
    && !overview_active                 // overview thumbnails + desktop snapshot
    && !self.shell.screenshot_active()  // interactive picker scrim over desktop
    && !self.screenshot_freeze.active() // frozen desktop snapshot image
    && !session_locked
    && window_switcher.is_none()
    && live_previews.is_empty();
```

- `Chrome::occludes_desktop` is asserted by the occluder itself (only
  `ControlCenter`, and only once `canvas_alpha(reveal) == 255`); it is never
  inferred from aggregate chrome alpha.
- There is deliberately no "declared material" term. Chrome draws no material any
  more (ADR-0213), so a component cannot declare a backdrop the compositor must
  keep alive beneath it; the consumer set above is the complete list.
- **Failure Mode**: eliding the base pass while *any* scene consumer is live
  renders uninitialized pixels (or the clear colour) where the desktop should be
  — a black or torn panel, not a subtle artefact. A new chrome surface that draws
  scene-derived content must be added to the predicate above and to this list.

### Rule 8: Cursor Plane Single Writer (`[INV-ARCH-66]`)
`crates/tessera/src/runtime/event_loop.rs` runs the out-of-band
`present_pointer_cursor` commit **only while `!presentation.can_redraw()`** (a
previous primary-plane flip is still in flight). With the scheduler gate open,
`render_and_present` performs the `cursor_only_eligible` cursor commit or the
combined primary+cursor atomic commit.

- The removed `cursor_plane_piggyback` flag issued a non-blocking cursor commit
  *and* let the render transaction commit the cursor in the same frame. The first
  commit occupies the CRTC's single non-blocking commit slot, so the primary flip
  immediately after it returns `EBUSY`, enters the 1 ms micro-retry loop, and
  overwrites buffers faster than the display drains them (`撕裂`, `闪烁`).
- **Failure Mode**: two cursor-plane writers per frame look like "cursor stutter
  over modal chrome" turning into full-frame tearing.

### Rule 9: Scanout-Before-Present Frame Ownership (`[INV-ARCH-67]`)
`crates/tessera-platform/src/drm/device.rs` (`DrmBackend::present`) must export
the dma-buf, import and commit scanout, and only then call `frame.present()`. If
`commit_scanout` fails or returns `EBUSY`, the `SubmittedFrame` is abandoned
(Optics `flux_frame_abandon`, Optics ADR-0110) so `current_frame` does not
advance.

- **Failure Mode**: calling `frame.present()` first rotates the offscreen ring
  into the buffer currently under scanout, so the retry composites into a live
  buffer (torn frame); dropping the frame instead of abandoning it wedges the
  surface permanently (`flux_surface_begin_frame` returns
  `FLUX_ERROR_INVALID_STATE` forever).

### Rule 10: Freeze Capture and Bucketed Offscreen Extents (`[INV-ARCH-68]`, `[INV-ARCH-69]`)
- `assess_frame_damage` includes `screenshot_freeze.needs_capture()` in
  `base_full`: the frame that freezes the desktop for a screenshot must assess as
  full damage even when it carries no other input.
- An offscreen target rounded up to 128-pixel buckets
  (`flux::bucket_extent(raw, 128)`) must keep the **logical** extent it paints
  and the **physical** extent it allocates as separate values, and express every
  sampling ratio against the pair. This is the one surviving form of the old
  capture-ratio rule: the window-shadow filter buckets its mask and output, so a
  ratio computed against one extent and applied to the other still stretches the
  result.
- **Failure Mode**: a 64 px origin floor or a 128 px extent bucket makes the
  effect draw content produced for one origin at another, and stretches it
  relative to the unscaled crop.

---

## 2. Quick Symptom-to-Root-Cause Matrix

| Observed Symptom | Primary Suspect | Where to Inspect |
|------------------|-----------------|------------------|
| Severe frame drops during Control Center open/close or a chrome fade | Per-node `save_layer` pass explosion in `optics`, or a damage envelope that misses a fading body's painted halo | `../optics/libs/lens/src/render/replay.c` (`lensi_compile_node_body`), `crates/tessera-shell/src/chrome/system_osd.rs` (`damage_regions`) |
| A chrome surface looks shifted or doubled against its own body (`重影`) | A damage region localized from Frame T-1 geometry, or a halo whose reach is not proportional to the body that casts it | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`, `resolve_strip_geometry`), `crates/tessera-design/src/materials.rs` (`elevation_halo_for_body`) |
| Old magnified Dock tiles, tooltips, or preview cards flicker every 2–3 frames on cursor movement (`闪烁`) | `Dock::anim_pending()` returning `false` during magnification/hover drain, or `hover_damage_rect` not unioned into `Dock::damage_regions()` for 3 drain frames | `crates/tessera-shell/src/components/dock/rendering.rs` (`anim_pending`, `damage_regions`, `record_hover_surface_damage`) |
| GPU stays busy and frames keep presenting while chrome is visibly still | An animation predicate that cannot become false: eased state stored in an integral rect, a latched `anim_active`, or a drain counter in the clock | `crates/tessera-shell/src/chrome/window_switcher.rs` (`AnimCard`, `is_moving`), `crates/tessera-shell/src/components/dock/motion.rs` (`anim_pending`) |
| A pointer crossing chrome repaints the whole output | A component that captures the pointer without answering `pointer_damage_regions`, so the host falls back to full output | `crates/tessera/src/runtime/damage.rs` (`input_damage`), `crates/tessera-shell/src/lib.rs` (`Shell::pointer_damage_regions`) |
| Region screenshot selection cannot be dragged; Enter/Space closes the picker with no capture (`截图不可用`) | `Chrome::prepare_frame` default consumed the call, so `update_pointer` never ran and `confirmed` stayed `None` | `crates/tessera-shell/src/component/mod.rs` (`Chrome::prepare_frame`), `crates/tessera-shell/src/chrome/screenshot.rs` (`prepare_frame`, `update_pointer`) |
| Frame freezes for a screenshot but captures a stale/blank desktop | `assess_frame_damage` `base_full` missing `screenshot_freeze.needs_capture()` | `crates/tessera/src/runtime/damage.rs` (`base_full`) |
| Full-output tear/flicker while moving the cursor over a modal panel | `cursor_plane_piggyback` committing the cursor out-of-band *and* in the render transaction, forcing the primary flip into `EBUSY` retries | `crates/tessera/src/runtime/event_loop.rs` (`present_pointer_cursor` gate), `crates/tessera/src/runtime/input.rs` (`FrameState`) |
| Tear on the frame after an `EBUSY` atomic-flip retry | `frame.present()` called before `commit_scanout`, rotating the offscreen ring into the buffer under scanout | `crates/tessera-platform/src/drm/device.rs` (`DrmBackend::present`), `../optics/libs/flux/src/core/frame.c` (`flux_frame_abandon`) |
| Wallpaper and client windows composited beneath an open Control Center | `desktop_occluded` predicate missing `occludes_desktop` or a live scene consumer | `crates/tessera/src/runtime/presentation.rs` (`desktop_occluded`), `crates/tessera-shell/src/lib.rs` (`Shell::occludes_desktop`) |
| Dock collapses when a window maximizes or overlaps it, and can never be expanded again | `collapse_pending` not cleared when `is_settled && target_reveal == 0.0`, or `hidden_trigger_armed` not re-armed when pointer leaves handle | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
| Dock starts expanding after 180 ms dwell, then immediately collapses halfway up when moving the cursor toward the icons | `expanding` state not including `expanded_trigger_contains(rest_bounds)` in `over_dock_trigger`, causing the 0.15 s `AUTOHIDE_QUICK_DISMISS_TIMEOUT` to fire during the 0.25 s spring flight | `crates/tessera-shell/src/components/dock/rendering.rs` (`step_autohide_motion`) |
