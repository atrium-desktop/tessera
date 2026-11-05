---
id: ADR-0203
title: "Damage is a placement-mapped rectangle, never a full-repaint verdict"
status: accepted
date: 2026-11-02
scope: core/rendering, presentation, damage
superseded_by: null
negative_knowledge: true
---

# 0203. Damage is a placement-mapped rectangle, never a full-repaint verdict

- Status: Accepted
- Date: 2026-11-02
- Deciders: Tessera Maintainers
- Amends: [ADR-0015](0015-damage-tracking.md), [ADR-0020](0020-buffer-scale-applied-at-composite.md), [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md)
- Related: [ADR-0202](0202-single-pass-full-output-backdrop-capture-and-glass-interior-compute-elision.md), [ADR-0204](0204-frame-scheduling-reserves-a-runqueue-the-desktop-does-not-use.md)

---

## Context and Problem Statement

The compositor kept the Intel GPU render engine 45–60 % busy on a completely
idle desktop (one static terminal plus a browser window), submitted 135–229
composites per second against a 120 Hz panel, and logged
`libinput: event processing lagging behind by 59 ms` on pointer motion. Field
measurement via `drm-engine-render` in `/proc/<pid>/fdinfo` isolated the cause
to a single predicate.

`ClientDamageTracker::accumulate_surface` classified a surface as *mappable*
only when it had no `wp_viewport` source, no `wp_viewport` destination, no
in-flight transition, an unchanged transform, and an unchanged buffer scale.
Anything else returned `true`, which the tracker turns into
`ClientDamage::Full` → `FrameDamage::Full` → a full-output composite. The
predicate was written when viewport and transition support was new and the
conservative answer was the only safe one.

Every modern HiDPI client sets `wp_viewport.set_destination`. GTK4, Chromium,
Electron, and foot all do it as a matter of course at
`fractional-scale-v1`-adjacent scale factors. On a 2× internal panel that
means **every mapped surface, every frame, forced a full-output repaint**. The
damage tracker's whole purpose — narrowing a repaint to what changed — was
inert on this machine.

Measured directly by instrumenting `accumulate_surface` in a nested instance:

```
trace: surface 94080949669712 UNMAPPABLE vp_src=None vp_dst=Some(Size { w: 1314, h: 780 })
   × 1154 in 24 s, always the same surface
```

The false premise is that a viewport or a transition destroys the surface →
output mapping. It does not. Both are *linear maps*: a surface-local rectangle
scales by `extent / local` and translates by the surface origin. A rectangle
stays a rectangle. Only the scale factor changes.

## Decision Drivers

- Damage must be a *decision*, not a fallback. A tracker that answers "full
  repaint" for the common case of a modern client is not tracking damage.
- The full-repaint tier exists for one reason: a region that genuinely cannot
  be expressed as rectangles. `wp_viewport` is not that. Preserving the tier
  for a provably expressible case converts every transient uncertainty into a
  permanent desktop-wide cost.
- Three code paths must agree on where a surface lands — the renderer's blit
  (`destination_size`), the ownership/occlusion query
  (`SurfacePresentation::logical_extent`), and the damage pass. They previously
  agreed by coincidence on three separate reimplementations of the same
  arithmetic. A damage rectangle that disagrees with the drawn rectangle is a
  stale pixel or a wasted repaint, and neither is observable in a test that
  only checks the damage list.
- Correctness of the narrowed repaint must be *testable per transformation*
  (identity, viewport destination, viewport source, transition extent, buffer
  scale, transform), not only in aggregate.

## Considered Options

- **Keep the unmappable tier and special-case `viewport_dst` only.** Rejected:
  `viewport_src` and transition extents are exactly as linear. Fixing one third
  of the predicate leaves the same class of bug reachable, and the next client
  that sets a source crop re-introduces it.
- **Map damage through the viewport but keep transitions unmappable.**
  Rejected: a transition's interpolated extent is *the* scale factor the
  renderer uses for that frame. Refusing to divide by it forces a full repaint
  for the entire duration of every window open, close, minimize, and workspace
  slide — precisely when the desktop is already at its busiest.
- **Clamp every damage rect to the surface's local extent and translate.**
  Rejected: ignores the viewport scale, so a HiDPI client's damage would land
  in the wrong place (a correct-looking rectangle covering the wrong pixels —
  the worst failure mode available, because nothing detects it).
- **Introduce a `SurfacePlacement` value computed once per sample and used by
  all three sites.** Chosen.

## Decision Outcome

Chosen option: **`SurfacePlacement` is the single authority for a surface's
surface-local → output-logical map, and damage is always mapped through it.**

`SurfacePlacement` resolves, per sampled frame:

| field           | meaning                                                                 |
| --------------- | ----------------------------------------------------------------------- |
| `origin`        | output-logical origin the visible content is drawn at                    |
| `local`         | post-transform buffer extent ÷ `set_buffer_scale` (the damage space)     |
| `source_origin` | where the visible content begins inside `local` (`viewport_src` origin)  |
| `source_extent` | how much of `local` is visible (`viewport_src` size, else all of it)     |
| `dest_extent`   | transition size → `viewport_dst` → `source_extent`                       |

The map is

```
p  ->  origin + (p - source_origin) * dest_extent / source_extent
```

The `- source_origin` term is the whole point. `wp_viewport.set_source` does
not merely scale — it *selects* which part of the buffer is visible and
therefore moves where the visible content begins. A crop at `(100, 100)` draws
surface pixel `(100, 100)` at the surface's output origin. Dropping the
subtraction (dividing by `local`, keeping `origin` as the base) yields a map
that is correct only when `source_origin == (0, 0)`, and displaces every
cropped surface's damage by `crop_origin * scale`.

`place()` clips a surface-local rectangle to the visible content
(`visible_local()`), applies the map in `f64`, and rounds **outward**, so every
touched output pixel is inside the damage; a rectangle that reaches no visible
content returns `None`. `place_damage()` maps the client's damage list (empty
means "no usable information" → the whole visible content). `bounds()` adds the
soft-shadow skirt for the vacated/occupied region when a placement moves.

`accumulate_surface` now pushes rectangles and returns nothing. The `bool`
return that carried "this surface forces a full repaint" is deleted, and with
it `ClientDamage::Full` — the variant that bool produced. Nothing can construct
it: a surface commit is always expressible through a placement, and a surface
that covers the output arrives as a full-output `Area` rect rather than a
blanket verdict. Leaving an unreachable variant in the type would let a future
caller reintroduce the blanket repaint without noticing the removal.

The predicate that forced full repaint is deleted, not narrowed. There is no
"unmappable viewport" tier left to regress into, and no `Full` arm to reach.

### Invariants & Behavioral Boundaries

Each invariant names its enforcement mechanism. The `test_inv_*` naming is the
repository's audit convention for the invariant set
(`docs/dev/invariants.md`).

- `[INV-ARCH-92] Placement is the single mapping authority.` The renderer's
  destination arithmetic (`tessera-composite` `destination_size` plus the
  transition override), the ownership query
  (`SurfacePresentation::logical_extent`), and `SurfacePlacement::resolve` must
  resolve the same `(origin, local, extent)` for the same geometry, viewport,
  scale, transform, and transition state. A change to one is a change to all
  three. Enforced by `placement_agreement` in `tessera-composite`:
  `test_inv_arch_92_renderer_and_damage_resolve_the_same_extent`,
  `test_inv_arch_92_placement_rect_matches_the_drawn_rect`, and
  `test_inv_arch_92_damage_at_the_visible_origin_lands_at_the_drawn_origin`.
  The last one is the load-bearing case: comparing extents alone passes a
  scale-only map, so the suite asserts the *landing position* too, over a
  matrix of viewport, scale, transform, and transition states including
  offset crops.
- `[INV-ARCH-93] Damage never widens because a surface is transformed.` A
  `wp_viewport` source, a `wp_viewport` destination, a buffer scale, a buffer
  transform, and an in-flight window transition each contribute a scale factor
  and nothing else. No combination of them may produce
  `FrameDamage::Full` — the variant does not exist. Enforced structurally by
  the type (see below) and by
  `test_inv_arch_93_viewport_destination_scales_damage_instead_of_forcing_full`,
  `test_inv_arch_93_transition_extent_overrides_the_viewport_destination`, and
  `test_inv_arch_93_full_surface_damage_covers_exactly_the_drawn_rect`.
- `[INV-ARCH-94] Outward rounding, never truncation.` Mapping a surface-local
  damage rect to the output must round the origin down and the far edge up.
  Truncation accumulates unpainted pixels along scaled edges. Enforced by
  `test_inv_arch_94_placement_maps_and_clamps_damage_to_the_surface`.
- A placement change repaints both the rect the surface vacated and the rect it
  now covers; the client's damage list is not a statement about the vacated
  pixels.
- The empty-damage convention is preserved: an empty list means "the whole
  surface changed", never "nothing changed".
- Damage stays a *list* of disjoint rectangles; nothing here unions them into a
  bounding box before the KMS `FB_DAMAGE_CLIPS` hand-off.

### Chrome animation damage must be nameable, not assumed

The same class of defect existed one layer up. `Shell::anim_damage_region`
returned `None` — a full-output repaint — whenever `lens` reported an eased
value in transit, on the grounds that "lens's internal eased values have no
exposed footprint". Lens has shipped `lens_damage_rect` (its own damage query,
covering base-tree damage, placed subtrees, tooltips, in-transit eased values,
and the text-caret blink cell) for some time; the Rust binding simply never
exposed it. So any settling lens widget — a hover fade on a button, a caret
blink in a focused text field — forced a full-output composite.

The binding now exposes `Ui::damage_rect`, and `anim_damage_regions` folds it
into the chrome damage exactly like a component's [`Chrome::damage_region`]. A
component that returns `None` still forces a full repaint, which is the correct
escape hatch; lens no longer uses it as a matter of routine.

The shell reports a **list**, not a bounding box. This is not a stylistic
choice: the HUD sits at the top of the output and the Dock at the bottom, so
unioning two independently animating chrome bodies yields the whole panel and
the narrow repaint is thrown away — the exact cost this ADR exists to remove.
The damage pipeline is a list all the way to the KMS `FB_DAMAGE_CLIPS` hint;
the only place that collapsed it was `anim_damage_region`'s single `Rect`
return. Enforced by `disjoint_chrome_animations_stay_disjoint`, which asserts
two edge-anchored footprints survive as two rectangles whose area is a small
fraction of the panel.

### The render area is clamped, not assumed

A narrowed damage rect reaches Vulkan as a dynamic `renderArea`, and both flux
backends reject one that is out of bounds, has a negative origin, or has exactly
one zero dimension (`canvas_pass_render_area`). `frame_damage_render_area`
previously converted the damage union straight to a descriptor, so a damage
rect that reached outside the framebuffer — a partially off-screen surface, a
history rect latched before a clamp, an `i32`-saturated widening — made
`begin_session` fail. The frame then lost its chrome, which is exactly the
`shell chrome render failed: Render(FLUX_ERROR_INVALID_ARGUMENT)` warning seen
eight times in a boot log.

The function now takes the framebuffer extent, intersects with it, and answers
`None` — a full-destination pass — when nothing usable survives. Clamping in the
damage domain keeps the failure where it is visible and recoverable instead of
turning it into a rejected pass. Enforced by
`test_inv_arch_94_render_area_is_clamped_to_the_framebuffer`.

### Building and drawing are separate phases

The first revision of this section claimed the ordering was already safe
because `assess_frame_damage` runs before the shell draws. That was wrong, and
instructively so: lens's `damage_rect` is only meaningful for a frame that has
been **built** (`lens_begin` … `lens_end`), and building used to happen inside
the draw call, after damage assessment. So the query described the *previous*
frame — which is precisely why `anim_damage_region` had to answer `None` for any
settling lens widget, and precisely why folding the query in without moving the
build would have narrowed repaints to the wrong region.

A frame is therefore split into two phases:

| phase                | location                      | does                                        |
| -------------------- | ----------------------------- | ------------------------------------------- |
| `Shell::prepare_frame` | before damage assessment    | runs the prepass, **builds the scene**, stores it in `pending_scene` |
| `Shell::submit`        | inside an open canvas pass  | draws the stored scene, activates the generation |

`Shell::render` remains as the "do both" convenience for callers that have no
damage pass in between (headless paths, tests); it builds if nothing is pending
and then submits. The compositor calls `prepare_frame` early and `submit` inside
the pass, so all of a frame's state mutation happens in the prepass and the draw
is pure.

This is the architectural reason the lens damage query is trustworthy, and it is
enforced by `prepare_frame_builds_the_scene_damage_assessment_reads`.

## Consequences

### Positive

- A HiDPI client's incremental damage now reaches the compositor's repaint as
  a rectangle. The desktop stops re-compositing the full output for a blinking
  terminal cursor.
- Window transitions localize: an open, close, minimize, or workspace slide
  repaints the transition footprint instead of the panel.
- The three-place arithmetic agreement is now explicit, so a viewport bug shows
  up as a wrong-sized repaint in a unit test rather than as an intermittent
  visual artifact.

### Negative / Risks

- `SurfacePlacement` duplicates the resolution *policy* (which of destination,
  transition, source, or local wins) with the renderer and the ownership query.
  The values are shared by construction; the *precedence* is not. A future
  protocol addition (a second viewport-like rect) must update all three, and
  nothing mechanically enforces that. Accepted: the alternative was a damage
  pass that cannot express a viewport at all.
- Mapping in `f64` and rounding outward means a damage rect can be one pixel
  larger than the exact region. That is the correct direction to err.
- A surface whose placement changes every frame (a live resize) now reports
  `bounds() ∪ rect()` rather than `Full`. That is strictly narrower and still
  covers the vacated pixels, but a resize that also scrolls content inside the
  surface relies on the client's damage list being non-empty. Clients that
  commit a resize with an empty damage list already meant "everything changed",
  which `place_damage` honours.

## Rejected Alternatives & Negative Knowledge

### Keep the unmappable tier with a narrower predicate
- **Why considered**: it is the smallest possible diff, and the ADR that
  introduced the tier did so for a real reason — viewport support landed before
  the damage pass understood it.
- **Why rejected**: the tier's existence is what let a routine HiDPI client
  reach it on every frame for months without a single test failing. Deleting
  the tier is the only change that prevents its recurrence. A narrower
  predicate leaves the same shape of bug available to the next protocol
  feature.

### Map damage by `extent / local` and keep `origin` as the base
- **Why considered**: it is the obvious reading of "scale the rectangle", it
  passes an extent-only cross-check against the renderer, and it is correct for
  every surface without a source crop — which is the overwhelming majority.
- **Why rejected**: it is wrong for exactly the case the ADR exists to fix. A
  `wp_viewport.set_source` crop displaces the visible content's origin, so the
  map needs `- source_origin`. The first revision of this ADR shipped the
  scale-only form *and* a test that asserted its output as correct; the defect
  was found by re-deriving the map from the renderer's `viewport_uv` rather
  than from the damage pass's own assumptions. The failure mode is silent —
  a rectangle of the right size over the wrong pixels — so the invariant suite
  now asserts the *landing position* of damage at the visible origin, not just
  the extent. Retained here so the next simplification attempt meets the
  counter-example first.

### Treat a viewport as a full-surface damage every commit
- **Why considered**: avoids the scale arithmetic; a viewport commit usually
  accompanies a resize.
- **Why rejected**: `wp_viewport.set_destination` is routinely committed on a
  *content* frame, not a resize frame — a video player letterboxing into a
  destination rect, a fractional-scale client re-committing at the same
  destination every frame. Collapsing those to full-surface damage is only
  marginally better than full-output damage, and measurably worse than the
  correct rectangle.
