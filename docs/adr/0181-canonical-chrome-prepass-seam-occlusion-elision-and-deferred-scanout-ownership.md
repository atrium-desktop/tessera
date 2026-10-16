---
id: ADR-0181
title: "Canonical Chrome Pre-Pass Seam, Occlusion-Eliding Base Pass, and Deferred-Scanout Frame Ownership"
status: accepted
date: 2026-10-15
scope: shell/component, shell/dock, shell/pivot, runtime/presentation, runtime/damage, runtime/event_loop, runtime/rendering, platform/drm
superseded_by: null
negative_knowledge: true
---

# 0181. Canonical Chrome Pre-Pass Seam, Occlusion-Eliding Base Pass, and Deferred-Scanout Frame Ownership

- Status: Accepted
- Date: 2026-10-15
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0101](0101-cursor-plane-commit-independence.md), [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md), [ADR-0178](0178-optics-transit-smoothstep-unification-and-chrome-prepass-lifecycle.md), [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md), [ADR-0180](0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md)
- Extends: Optics [ADR-0110](https://github.com/ming2k/optics/blob/main/docs/adr/0110-frame-slot-abandonment-without-ring-advance.md)

---

## Context and Problem Statement

Post-landing audit of ADR-0179/0180 exposed a class of defects distinct from the
frame-time regressions those records fixed: **correctness holes created by
half-applied abstractions**. Each was independently reachable in the shipped
build.

1. **The `prepare_backdrop` compatibility alias inverted the pre-pass.**
   ADR-0178 renamed the pre-pass to `Chrome::prepare_frame` "while retaining
   `prepare_backdrop` as an inline compatibility alias" — but the alias was
   installed the wrong way round: `Chrome::prepare_backdrop` defaulted to
   calling `prepare_frame`, while `Shell::prepare_frame` called
   `component.prepare_frame` (an empty default). Every component that had been
   migrated by *keeping* its `prepare_backdrop` implementation —
   `ScreenshotSelector`, `Dock`, `Hud` — was therefore **never invoked during a
   compositor frame**:
   - `ScreenshotSelector::update_pointer` never ran, so in `PickerMode::Region`
     the selection rectangle could not be dragged and `confirmed` stayed
     `None`; pressing Enter closed the picker without capturing anything
     (`截图不可用`).
   - `Dock::step_autohide_motion` never ran, so autohide reveal, magnification
     springs, and settled-drain priming all read frame T-1 state.
   - `Pivot` advanced its reveal spring inside `render()`, i.e. after
     `backdrop_regions`/`liquid_glass_regions` had already been sampled.

2. **The base desktop pass was drawn even when an opaque chrome surface
   covered it.** An open Control Center paints a full-display opaque canvas
   once its reveal reaches `alpha == 255`, yet the compositor still acquired
   every client dma-buf, drew the wallpaper and every window, and only then
   overdrew all of it.

3. **Two writers contended for the KMS cursor plane.** Pointer motion over
   modal chrome took a `cursor_plane_piggyback` fast path that issued a
   non-blocking `NONBLOCK` cursor commit *and* the render transaction also
   committed the cursor. The first commit occupied the CRTC's non-blocking
   commit slot, so the primary-plane atomic flip immediately after it failed
   with `EBUSY`, entered the 1 ms retry loop, and overwrote buffers faster than
   the display drained them (`撕裂`, `闪烁`).

4. **The offscreen frame ring rotated before KMS accepted the flip.**
   `DrmBackend::present` called `frame.present()` *before* exporting the
   dma-buf and committing scanout. On `EBUSY` the ring had already advanced
   into the buffer under scanout, so the retry composited into a live buffer.
   Optics had no way to release the submitted frame without advancing the ring
   (see Optics ADR-0110).

5. **A screenshot freeze frame could escape with no damage.** `assess_frame_damage`
   computed `base_full` without consulting `screenshot_freeze.needs_capture()`,
   so the single frame that must capture the frozen desktop could resolve to
   `FrameDamage::None` when it carried no other input.

6. **The backdrop cache key quantized away a coordinate the capture bakes in.**
   `BackdropCacheKey::new` floored `capture_origin` to 64 px and bucketed
   `capture_extent` to 128 px. `capture_origin` is *baked into the capture
   contents* (`session.translate(-origin * capture_ratio, …)`) and
   `capture_extent` parameterizes `capture_ratio` and `draw_cached`'s `u_span`.
   With `BACKDROP_DOWNSAMPLE == 1` the only correct `capture_ratio` is `1.0`;
   the bucketed key made it `bucket / extent` (e.g. `1280 / 1192 = 1.0738×`),
   scaling the captured scene while `draw_cached` cropped the *unbucketed*
   sub-rectangle — the persistent double image ADR-0180 had already attributed
   to `capture_size`.

## Decision Drivers

- A pre-pass whose dispatch depends on which of two spellings an implementor
  chose is not a seam; it is a coin flip.
- Redundant GPU work is a correctness problem when it also forces the
  presentation pipeline to own buffers it does not need.
- Exactly one writer may own the KMS cursor plane per frame (ADR-0101).
- A frame's ring slot must not advance until the external consumer has
  accepted the pixels.

## Considered Options

- **(A) Repair the alias by having both spellings dispatch through one
  function.**
- **(B) Delete the alias, make `prepare_frame` canonical, and migrate every
  component.**
- **(C) Keep drawing the base desktop pass and rely on overdraw being cheap.**
- **(D) Elide the base pass behind an explicit `occludes_desktop` contract.**

## Decision Outcome

Chosen options: **(B)** and **(D)**, plus the cursor/scanout/damage/cache
corrections in §3–§6.

1. **`prepare_frame` is the only pre-pass seam.** `Chrome::prepare_backdrop`
   and `Shell::prepare_backdrop` are deleted (`crates/tessera-shell/src/component/mod.rs`,
   `crates/tessera-shell/src/lib.rs`). `ScreenshotSelector`, `Dock`, and `Hud`
   implement `prepare_frame`; `Pivot` gains a `frame_prepared` latch and
   advances its reveal spring in `Chrome::prepare_frame`, consuming the
   pre-passed value in `render` without integrating `dt` twice.

2. **Occlusion elision is an explicit contract, not an inference.**
   `Chrome::occludes_desktop` (default `false`) is overridden by `ControlCenter`
   when `active() && canvas_alpha(reveal) == 255`. `Shell::occludes_desktop`
   evaluates it over exactly the components admitted by the same
   `participates_in_shell_pass` filter that feeds `Shell::backdrop_layers`.
   `render_and_present` skips `begin_desktop_output` and renders the shell into
   one clearing stencil pass only when the occluder is real **and** no consumer
   of the scene is live: `!capture_active && !overview_active &&
   !self.shell.screenshot_active() && !screenshot_freeze.active() &&
   !session_locked && window_switcher.is_none() && live_previews.is_empty()`.

3. **The cursor plane has exactly one writer per frame.** `FrameState::cursor_plane_piggyback`
   is deleted. The out-of-band `present_pointer_cursor` path runs only while
   `!presentation.can_redraw()` (a previous primary flip is still in flight);
   with the scheduler gate open, `render_and_present` performs the
   `cursor_only_eligible` cursor commit or the combined primary+cursor atomic
   commit.

4. **Scanout precedes presentation.** `DrmBackend::present` exports the
   dma-buf, imports and commits scanout, and only then calls `frame.present()`.
   If `commit_scanout` fails or returns `EBUSY`, the `SubmittedFrame` is
   abandoned (Optics ADR-0110), so `current_frame` does not advance and the
   retry reuses the same free backbuffer slot.

5. **A freeze capture always owns its frame.** `assess_frame_damage` includes
   `screenshot_freeze.needs_capture()` in `base_full`.

6. **The backdrop cache key is pixel-exact.** `BackdropCacheKey::new` stores
   `capture_origin`/`capture_extent` verbatim; `BackdropCapture::size` keeps the
   unbucketed logical extent while `allocated_size` keeps the bucketed
   high-water extent (`[INV-ARCH-61]`).

7. **The Dock resolves its strip once per frame.** `Dock::resolve_strip_geometry`
   is the single layout source for the pre-pass and `render`; `Dock::apply_pending_order`
   runs in both so the optimistic reorder commit is visible to the pre-pass.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-64] Single Pre-Pass Seam`: Chrome animation and geometry
  pre-passes are dispatched exclusively through `Chrome::prepare_frame` /
  `Shell::prepare_frame`. No compatibility spelling may exist, because a second
  name with a default body silently disables the seam for every component that
  uses it.
- `[INV-ARCH-65] Occlusion Elision Requires a Dead Scene`: The base desktop
  pass may be skipped only when a `participates_in_shell_pass` component reports
  `occludes_desktop() == true` **and** no live consumer of the desktop scene
  exists. The consumer list is exhaustive and load-bearing: declared backdrop
  capture (`capture_active`), overview thumbnails (`overview_active`), the
  interactive screenshot picker (`screenshot_active`) and its freeze
  (`screenshot_freeze`), session lock, the window switcher, and live window
  previews. An occluder that itself declares a scene-sampling effect, or any new
  surface drawing scene-derived content outside `backdrop_layers` /
  `live_preview_presentation`, must not be added without extending that
  predicate.
- `[INV-ARCH-66] Cursor Plane Single Writer`: Exactly one KMS cursor-plane
  commit is issued per frame. The out-of-band path is gated on
  `!can_redraw()`; `cursor_plane_piggyback` may not be reintroduced alongside
  the render transaction's own cursor commit.
- `[INV-ARCH-67] Scanout-Before-Present Frame Ownership`: `frame.present()` is
  called only after KMS has accepted the atomic flip for the exported dma-buf.
  A rejected or deferred flip abandons the frame so the offscreen ring slot is
  not rotated into an active scanout buffer.
- `[INV-ARCH-68] Freeze Capture Owns the Frame`: Any frame in which
  `screenshot_freeze.needs_capture()` is true must assess as full damage.
- `[INV-ARCH-69] Exact Backdrop Cache Geometry`: `BackdropCacheKey` identity
  uses the unquantized `capture_origin`/`capture_extent`. Quantizing either
  either offsets the glass body or reintroduces the `capture_ratio` distortion
  forbidden by `[INV-ARCH-61]`.

---

## Rejected Alternatives & Negative Knowledge

### 1. Repairing the `prepare_backdrop` alias instead of deleting it
- **Why considered**: Keeping the alias would have avoided touching five
  component implementations and their tests, and would preserve source
  compatibility for any out-of-tree chrome.
- **Why rejected**: The alias cannot be made safe by direction alone. If
  `prepare_backdrop` delegates to `prepare_frame`, every implementor that
  overrides `prepare_backdrop` is silently dead (the exact ADR-0178 failure). If
  `prepare_frame` delegates to `prepare_backdrop`, every implementor that
  overrides `prepare_frame` is silently dead. Two names for one seam guarantee
  that some subset of components is never called; only deletion removes the
  failure mode. The trait is in-tree and pre-1.0, so the compatibility cost is
  zero.

### 2. Inferring desktop occlusion from the shell's aggregate alpha
- **Why considered**: Summing declared chrome opacity and skipping the base pass
  above a threshold would avoid a new trait method and any per-component
  override.
- **Why rejected**: Aggregate alpha cannot distinguish "opaque canvas covers the
  output" from "several translucent glass bodies overlap into high coverage".
  The latter still samples the desktop in its backdrop pass, so eliding the base
  pass there renders uninitialized pixels. Occlusion must be a predicate the
  occluder itself asserts and the compositor verifies against its consumer set.

### 3. Keeping `cursor_plane_piggyback` and having the render transaction skip
its cursor commit when the piggyback already ran
- **Why considered**: Piggybacking keeps cursor motion at input cadence while a
  modal panel forces composite repaints, which is where cursor stutter is most
  visible.
- **Why rejected**: The non-blocking cursor commit and the primary-plane flip
  race for the CRTC's single non-blocking commit slot; resolving the race with a
  "did the piggyback run?" flag requires the render path to know whether an
  earlier out-of-band commit escaped, which is exactly the shared-baseline
  protocol ADR-0101 was written to avoid. Restricting the out-of-band path to
  `!can_redraw()` gives the same input-cadence cursor while the gate is closed
  and a single combined atomic commit when it is open.

### 4. Advancing the offscreen ring in an error path and letting the surface
recreate itself
- **Why considered**: `flux_frame_present` already existed, and `needs_recreate`
  already rebuilt a surface; an error-path `present()` needed no new API.
- **Why rejected**: Advancing the ring is precisely what puts the retry onto the
  buffer under scanout (torn frames), and recreating the offscreen pipeline on
  every 1 ms `EBUSY` micro-retry is an allocation storm for a transient
  condition. Correct behaviour required a new Optics transition; see Optics
  ADR-0110 §Alternatives.

### 5. Bucketing the backdrop cache key to suppress per-frame invalidation
- **Why considered**: Pixel-exact keys invalidate whenever an animated glass
  body moves, which reads as "thrashing".
- **Why rejected**: The key already carries pixel-exact absolute region rects
  (`capture_regions`, `layer_regions`) that move continuously with animated
  chrome, so quantization never made the identity stable — it only made it
  *wrong*. `capture_origin` is baked into the capture via `session.translate`,
  so a stale key draws content captured for one origin at another; and bucketed
  `capture_extent` makes `capture_ratio` non-unit, stretching the glass while
  `draw_cached` crops the unscaled extent. Re-capture is region-scissored, so
  the honest key's cost is bounded by the glass footprint.

### 6. Localizing the Dock pre-pass to the hover dismissal only
- **Why considered**: The pre-pass only needs the hit-test result; recomputing
  the full strip geometry (bar length, centres, panel rect, icon rects) there
  duplicates ~60 lines of `render`'s layout.
- **Why rejected as an accepted shape**: A second copy of the layout math is the
  defect, not the fix — the pre-pass and `render` can disagree about a tile's
  position, which produces exactly the one-frame tooltip flicker the pre-pass
  exists to prevent. Instead the layout was extracted into
  `Dock::resolve_strip_geometry`, which both callers share, and
  `Dock::apply_pending_order` was moved ahead of the pre-pass so a drag commit
  cannot leave the two looking at different tile orders.

---

## Consequences

### Positive
- Screenshot region selection works again: `ScreenshotSelector::update_pointer`
  runs every frame, so drag and confirm are live.
- Dock and Pivot publish frame-T geometry to damage assessment and backdrop
  capture; the ADR-0171 desync cannot silently return through a renamed method.
- An open Control Center no longer composites the wallpaper and every client
  window beneath an opaque canvas.
- Pointer motion over modal chrome no longer manufactures a spurious
  `EBUSY` retry loop; the KMS cursor plane has a single owner per frame.
- A rejected atomic flip retries on the same free backbuffer instead of a buffer
  under scanout.
- Glass captures no longer double-image after any camera-origin change.

### Negative / Trade-offs
- Removing `prepare_backdrop` is a source-breaking trait change for out-of-tree
  chrome; accepted pre-1.0 and mitigated by the triage guide.
- `Shell::occludes_desktop`'s consumer list (`window_switcher`, `live_previews`,
  `screenshot_freeze`, `session_locked`, `capture_active`) is hand-maintained.
  Any new chrome surface that draws scene-derived content outside
  `backdrop_layers`/`live_preview_presentation` must be added to
  `[INV-ARCH-65]`'s predicate or it will be elided with the base pass.
- Pixel-exact backdrop cache keys re-capture more often under animated glass.
  Bounded by region-scissored capture; if profiling ever shows it dominating,
  the correct lever is the region granularity, never the key.
