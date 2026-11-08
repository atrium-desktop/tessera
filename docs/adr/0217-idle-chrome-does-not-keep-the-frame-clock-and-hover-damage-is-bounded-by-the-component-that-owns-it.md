---
id: ADR-0217
title: "Idle Chrome Does Not Keep the Frame Clock, and Hover Damage Is Bounded by the Component That Owns It"
status: accepted
date: 2026-11-05
scope: shell/dock, shell/window-switcher, shell/component, runtime/damage, design/materials
superseded_by: null
negative_knowledge: true
---

# 0217. Idle Chrome Does Not Keep the Frame Clock, and Hover Damage Is Bounded by the Component That Owns It

- Status: Accepted
- Date: 2026-11-05
- Deciders: Tessera Maintainers
- Amends: [ADR-0021](0021-chrome-component-trait.md) (the `Chrome` contract gains a hover-damage query), [ADR-0171](0171-chrome-animation-pre-pass-synchronization-and-surface-placement-invariants.md) (the dock's frame clock), [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md) (chrome footprints are measured, not guessed — this extends the rule to the pointer)
- Related: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) (flat chrome: a painted fill must cost its area), [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md) (damage is a rectangle)

---

## Context and Problem Statement

Removing the analytic lens ([ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md))
made every chrome body a painted fill, and the reasoning was that a surface's
per-frame price should be proportional to its area. Two defects survived that
change and both break the rule from the other side: not by making a *painted*
pixel expensive, but by making an *unpainted* frame expensive.

**An open window switcher never stopped animating.** The switcher eases each
card's rect toward its layout slot with an exponential blend, and it reported
"still animating" whenever the eased rect differed from the target. The eased
state was stored in the layout's own integral `Rect`, so every frame quantised
it and re-derived the difference from the quantised value. The result was a
fixed point one pixel from the target: the blend resolved anything inside
0.75 px to the target, the next step landed on `target ± 1`, and `moving`
stayed true forever. A held-open switcher therefore drove the frame loop at the
panel refresh rate, and every one of those frames was a full-output composite
that re-sampled every card's live client buffer.

**A pointer move over chrome repainted the whole output.** A pointer move has no
pixel footprint of its own; it matters only because chrome under the cursor
reacts. The assessment asked "does any component capture the pointer here?" and
answered `None` — a full-output repaint — when the answer was yes. That premise
is true for a bar and false for a surface that owns most of the output. An open
command panel, Pivot, or the switcher made every mouse movement a full-screen
composite at the refresh rate — exactly the cost profile ADR-0213 deleted the
lens to avoid.

**A 4 px capsule cast a 12 px ring.** The elevation halo is the body's
silhouette pushed outward by a fraction of the role's nominal blur. That is
correct for a 74 px bar and wrong for the dock's collapsed capsule: a 12 px halo
around a 4 px pill reached six times the capsule's own thickness on every side,
which does not read as elevation beneath it — it reads as a dark painted ring
around it, visible as a grey edge that the hairline alone cannot explain.

**A timer advanced in `render` never drove the clock that advances it.** The
dock's dwell timer and the name bubble's fade were stepped inside `render`, and
the "still waiting / still fading" term was an `|=` into `anim_active` in the
same `render`. `Shell::prepare_frame` recomputes `anim_active` on every frame
before the damage assessment reads it, so a bit set during the previous
`render` is always gone by the time anything consumes it. The consequence was
not a slow animation but a *stopped* one: the bubble's appearance is gated on
`hover_elapsed >= TOOLTIP_DWELL`, and once the magnification wave had settled
nothing advanced `hover_elapsed` at all, so a pointer that came to rest on a
tile never raised the bubble.

**A hit test over a magnified bar was not a function of the pointer.** Two
neighbouring tiles magnify to the same size around the pointer, so a pointer
between them is equidistant from both centres to within float noise. The tile
that won was whichever one the noise favoured that frame, and it alternated
every frame or two. Because a hover *change* restarts the bubble's dwell, the
dwell was restarted faster than it could expire: the bubble took 121 frames to
fade in where its own dwell is 36. The fix is hysteresis — the incumbent keeps
the hover until a challenger is more than `HIT_HYSTERESIS_PX` closer — which is
what makes the answer a function of the pointer rather than of the noise.

All five are one class of defect: a value that should be *bounded by the object
that produces it* was instead bounded by a global constant, by a rounding grid,
or by a phase of the frame.

## Decision Drivers

- **Cost is per area, in both directions.** A surface that paints nothing must
  not cost a full-output composite, and a surface that paints 4 px must not cast
  a 12 px shadow.
- **"Still moving" must be a property of the animation.** Any predicate that
  drives the frame clock has to be able to become false, or it is a permanent
  full-rate repaint with extra steps.
- **A bound belongs to whoever knows it.** The host cannot know where a
  component's hover reaction lands; the component can. The contract should let
  it say so, and keep the conservative answer when it cannot.
- **No guessing in the other direction.** Requiring every component to invent a
  band would trade one wrong answer for another. The default stays "repaint
  everything".

## Considered Options

- **Clamp the switcher's iteration count.** Rejected: it hides the non-convergent
  state instead of removing it, and the clamp value is a frame budget nobody can
  derive.
- **Loosen the switcher's equality test with an epsilon.** Rejected: it is the
  same bug with a wider fixed point. The state still never equals its target, so
  the next comparison still reports movement.
- **Keep the animation in `i32` and snap on a "close enough" distance.** Rejected
  as the same non-convergence in a different place, and it makes the presented
  geometry depend on which frame happened to be last rather than on the curve.
- **Answer the pointer question with the component's existing `damage_regions`.**
  Rejected as a blanket rule: a drag or an edge gesture deliberately reaches
  outside the strip its damage footprint describes, so reusing that footprint
  unconditionally would under-cover those frames.
- **Float animation state, quantised at presentation; a separate hover-damage
  query defaulting to `None`; halo reach capped by the body's thickness.**
  Chosen.

## Decision Outcome

### Animated geometry is floating point, quantised where it is painted

The window switcher keeps its eased card geometry as `f32` (`AnimCard` /
`AnimRect`) and quantises only the copy it hands to the painter and to hit
testing. The eased value converges on its layout target, so "still moving"
becomes false when the curve has actually finished. Integral layout stays
integral — the quantisation is at the presentation boundary, not in the state.

A card is "moving" while any edge is more than half a pixel from its target.
Half a pixel is the finest difference that can survive the quantisation the
paint applies, so a smaller one is not movement; it is the grid.

### A clock term belongs to the phase that can set it

Every term that drives the frame clock is resolved in `prepare_frame` — or, for
a discrete one-shot change, reported through `ChromeEvents::scene_changed`,
which the damage assessment drains *after* the frame has been drawn. Nothing in
`render` may write `anim_active`: the next `prepare_frame` replaces it wholesale
before any consumer reads it, so such a write is unreachable code that looks
like policy.

Time-based state (dwell timers, fades, decay) therefore advances in
`prepare_frame` alongside the geometry it gates. A value stepped in `render`
cannot be seen by the assessment that already ran, which means the compositor
never learns it needs to produce the next frame — an animation that is invisible
or, as with the bubble, never starts.

### The `Chrome` contract answers for hover damage

`Chrome::pointer_damage_regions(x, y, display, windows, workspaces)` returns the
regions a pointer move to `(x, y)` can repaint, or `None` when the component
cannot bound its own reaction. It is consulted only for a component that answers
`captures_pointer` at that position — a component that does not take the pointer
has no answer to give.

`Shell::pointer_damage_regions` collects the answers and the damage assessment
uses them where it previously asked `captures_pointer_at` and degraded to full.
The default is `None`, so third-party chrome keeps the conservative verdict.

The components that answer:

- **Dock**: the same footprint it publishes for its animations, except while a
  drag is in flight — a reorder drag previews across the strip and an edge drag
  travels to another screen edge, which the strip footprint does not contain.
- **Window switcher**: the panel plus its painted halo. A *fade* is not
  localizable (the scrim's alpha covers the output), and it reports the full
  output while `visibility` is still travelling; the card slide, the selection
  indicator, and a hover wash all live inside the band.
- **Pivot**: its panel plus the halo under it, resolved at maximum content
  height so the band is a superset of every state a movement can occur in.

### Elevation reach is capped by the body's own thickness

`materials::elevation_halo_for_body` scales both the spread and the vertical
offset by `min(1, thickness / (2 × shadow_blur))`, where thickness is the
body's shorter edge. A body thinner than twice the nominal blur gets a
proportional contact shadow instead of a fixed reach; a body past the cap keeps
the role's full reach. The halo also takes the **body's** corner radius rather
than the role's styled one, so a body that morphs casts the silhouette it is
actually painting.

### Invariants & Behavioral Boundaries

- A component's `anim_pending` must be able to become `false` without input.
  Any animation whose state cannot represent its own target is a permanent
  full-rate repaint, and belongs in this ADR's rejected list, not in chrome.
- `render` must not write `anim_active`. The only channel from `render` to the
  damage assessment is `ChromeEvents::scene_changed`.
- Any state that advances with `dt` and gates an animation must advance in
  `prepare_frame`, not in `render`.
- **A hit test over an animated surface must be a function of the pointer.** If
  the pointer has not moved, the answer must not move either. A hover *change*
  is a discrete event everywhere downstream — it restarts a dwell, releases a
  popover, requests a repaint — so an answer that alternates with float noise
  makes every downstream timer unable to complete. The dock reads this as: the
  incumbent tile wins every tie within `HIT_HYSTERESIS_PX`.
- A hit target may be sized by a layout constant (the ceiling of an animation),
  never by the value that animation is currently producing. Sizing the target
  from the live geometry makes the hover a function of itself.
- Animated geometry that is quantised for painting is stored unquantised. Do not
  write an eased value back into an integral layout type and re-derive movement
  from it.
- One owner per piece of state. A `Dock`/`DockMotion` mirror pair is not two
  views of one value; it is two values that will eventually disagree, and the
  six hand-written copy sites needed to keep them aligned are the proof.
- `pointer_damage_regions` returns `None` unless the returned regions provably
  contain every pixel the component may repaint because the pointer arrived.
  Under-covering is the only unsafe direction; over-covering is safe and
  localized.
- A gesture that leaves the surface it started on (a drag, an edge-snap) must
  report [`None`], not the resting footprint.
- A painted halo is a function of the body it belongs to: its reach, its offset,
  and its corner radius all follow that body.

## Rejected Alternatives & Negative Knowledge

- **A frame budget for the switcher's easing.** Rejected: a cap that hides a
  non-convergent state is a slow-motion version of the same repaint, and it
  makes the final geometry depend on when the cap fired.
- **An epsilon on the switcher's target comparison.** Rejected: the eased value
  never reaches the target, so a tolerance only moves the fixed point.
- **Using `damage_regions` as the pointer answer for every component.**
  Rejected: it converts a correct conservative answer into an under-covering one
  for any component with a gesture that leaves its own band.
- **Keeping the halo reach constant and clipping it to the screen edge.**
  Rejected: the artefact is not at the edge, it is around the pill. Clipping
  moves the ring rather than removing it.
- **Suppressing the halo entirely for thin bodies.** Rejected: a chrome body
  that sits on arbitrary content still has to separate itself from it. The
  capsule keeps a shadow; it just stops being larger than the thing casting it.
- **Sizing a hit target from the geometry the hover is currently producing.**
  Rejected: it makes the hover a function of itself. A reach derived from the
  animated width chases the pointer into the seam between two tiles, the hover
  alternates, and every downstream timer that treats a hover change as an event
  is restarted before it can expire.
- **An `epsilon` on the "which tile is nearest" comparison.** Rejected, and this
  is the subtle one: an epsilon only widens the tie, so the answer still depends
  on which side the noise falls. Hysteresis is the right shape because it makes
  the *incumbent* the tie-breaker — the answer then depends on the previous
  answer, not on the noise.
- **Moving the hit test into the pre-pass to "match" the dwell timer.** Rejected
  in this codebase for a concrete reason: the dock resolves its strip geometry
  twice, once in the pre-pass for the springs and once in `render` for the
  paint, and the two resolutions disagree by a few pixels while the bar is
  morphing. A hit test in the pre-pass therefore answered about a bar that was
  not the one painted. The timer moved to the pre-pass (it is time, and the
  clock reads it there); the *hover* stayed in `render`, which is the pass that
  knows where the tiles actually were. Unifying the two geometry resolutions is
  the change that would let both live in one place, and it is not this ADR's.
- **Keeping a `|=` in `render` "so the clock notices".** Rejected, and this is
  the trap: it reads as intent and compiles as nothing. `prepare_frame`
  recomputes `anim_active` from scratch on the next frame, so the bit is
  discarded before the assessment runs. A one-shot change reported from `render`
  goes through `scene_changed`; a continuous one belongs in the pre-pass.
- **Keeping `Dock::settled_drain_frames` as a mirror of the motion state.**
  Rejected: it required six hand-written copy sites and gave two values the same
  name. `DockMotion` owns the counter; `Dock` reads it through `motion`.

## Consequences

- A held-open switcher costs one composite when it settles instead of one per
  refresh interval, and its settled frames localize to the strip.
- A pointer crossing the dock or an open panel repaints that surface's band
  instead of the output.
- The dock's resting capsule reads as a pill lying on the desktop rather than as
  a pill inside a grey ring.
- Chrome that cannot localize its hover reaction still repaints everything on a
  pointer move. That is deliberate and observable: the conservative path stays
  available, so the seam cannot silently become a guess.
- A future component that animates on a quantised grid has a worked example of
  why that is a frame-clock bug: the switcher's fixed point is recorded in
  `animated_card_geometry_reaches_its_target_exactly`.
- A component that advances a timer in `render` has a worked example too:
  `a_resting_pointer_raises_the_name_bubble_through_the_pre_pass` drives only the
  pre-pass and fails if the bubble needs `render` to make progress.
- A hover that is not a function of the pointer has one as well:
  `a_pointer_between_two_equal_tiles_keeps_one_hover` parks the pointer at the
  exact midpoint of two equally-magnified tiles and requires exactly one hover
  acquisition. Without hysteresis it reports seventeen.
- The two hover rules above pull against each other and both are required:
  stability says "do not change the answer on noise", reachability says "do not
  lose the popover when the pointer leaves the tile for it". `a_raised_popover_
  stays_while_the_pointer_enters_it` pins the second, so a future simplification
  toward a "pure" hit test fails loudly instead of silently making live previews
  unreachable.
