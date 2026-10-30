---
id: ADR-0198
title: "The Collapsed Dock Capsule Hugs Its Edge Behind an Independent Reveal Corridor"
status: accepted
date: 2026-10-29
scope: shell/dock
superseded_by: null
negative_knowledge: true
---

# ADR-0198: The Collapsed Dock Capsule Hugs Its Edge Behind an Independent Reveal Corridor

- Status: Accepted
- Date: 2026-10-29
- Deciders: Tessera Maintainers
- Amends: [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md)

---

## Context and Problem Statement

`DockState::collapsed_panel_rect` resolved the whole Dock surface morph from a
single rectangle, and it reused `DOCK_EDGE_MARGIN` (12 px) as the gap for
*both* endpoints. The expanded bar genuinely wants that float: it is a
detached panel, and wallpaper showing beneath it is what makes it read as
floating chrome.

The collapsed endpoint does not. It is a 4 px stadium that is the *parked edge
affordance* — the thing that says "the Dock lives on this edge." Suspended
12 px above the screen edge it reads as a stray horizontal artifact hovering
mid-air, separated from both the edge it belongs to and the bar it will become.
The visible band was 4 px of capsule floating in 12 px of empty wallpaper.

The obvious correction — anchor the capsule to the edge — is not local. Two
consumers had silently coupled their geometry to that one constant:

1. **Reveal hit-testing.** `collapsed_indicator_contains` derived the
   pointer target by inflating the capsule's own rect to the display edge.
   Tucking the capsule to 2 px would shrink the reachable strip to a 6 px
   stripe, quietly making the Dock harder to summon.
2. **Capture footprint.** `Dock::capture_footprint` anchored the compositor's
   blur capture and damage envelope at the expanded bar's float. A body that
   morphed toward the edge would paint outside its own capture — precisely the
   ghost-scissor class of artifact ADR-0171 and ADR-0180 exist to prevent.

## Decision Drivers

- The collapsed capsule must read as an edge affordance, not a floating body.
- ADR-0146's dwell verification needs a *stable, deep enough* pointer strip. Its
  anti-skim guarantee is the only thing standing between "ambient edge handle"
  and "cursor passes trigger the Dock," so it must not become a side effect of
  a visual tweak.
- No frame of the reveal may paint outside its own capture envelope
  (ADR-0171, ADR-0180).
- Both morph endpoints must still land exactly on resting geometry, so the
  interaction model and the painted body never disagree.

## Decision Outcome

### 1. One rect, interpolated endpoints

`DockState::collapsed_panel_rect` interpolates the edge gap on the *same*
eased progress (`collapse_surface_progress`) that already drives length and
thickness:

```rust
let margin = edge_margin_for_surface_progress(progress);
```

`edge_margin_for_surface_progress` — `motion::edge_margin_for_surface_progress` —
clamps its input and returns exactly `AUTOHIDE_HANDLE_EDGE_MARGIN` (2 px) at
`progress == 0.0` and exactly `DOCK_EDGE_MARGIN` (12 px) at `progress == 1.0`.
The Dock therefore *lifts* off the edge as it opens rather than holding one
offset and then snapping to the other, and `reveal == 1.0` still reproduces
`DockState::panel_rect_for` geometry to the float.

### 2. The reveal corridor is a separate concept from the capsule

`AUTOHIDE_HANDLE_APPROACH_BAND` (16 px) is declared independently, and
`DockState::collapsed_reveal_corridor` builds the pointer-reachable rectangle
from it. `collapsed_indicator_contains` — the sole predicate ADR-0146's dwell
verification tests against — now reads the corridor, not the capsule.

The corridor grows along the edge **normal only**. Deriving its depth from the
capsule's long axis would inflate a bottom corridor to the handle's full 140 px
width, which is why the band is computed per-edge from the capsule's *thickness*
and then forced to at least the approach band.

### 3. The capture envelope spans both endpoints

`Dock::capture_footprint` keeps its strip envelope anchored at the bar's float
and grows it toward the edge by exactly the tuck band
(`DOCK_EDGE_MARGIN - AUTOHIDE_HANDLE_EDGE_MARGIN`). The envelope is still
derived from the two endpoints and never from the live reveal, so it stays
stable across the whole animation.

### Invariants & Behavioral Boundaries

This decision establishes **domain contracts** inside `tessera-shell`, not
system invariants: per [Invariants](../../dev/standards/invariants.md) §2, the
Dock autohide reveal is a within-crate state machine, and the edge gap is a
design constant. Neither carries an `INV-` tag; both are enforced by unit
tests.

- **Visual offset and pointer reach are separate contracts**: the collapsed
  capsule's visual gap (`AUTOHIDE_HANDLE_EDGE_MARGIN`) and its reveal corridor
  depth (`AUTOHIDE_HANDLE_APPROACH_BAND`) are independent constants. Changing
  one must never silently change the other. Enforced by
  `tucked_capsule_keeps_its_reveal_corridor_deeper_than_the_capsule`.
- Both morph endpoints reproduce resting geometry exactly: `reveal == 1.0` under
  `collapsed_panel_rect` equals `panel_rect_for`, and the gap interpolates
  monotonically between the two clamped endpoints. Enforced by
  `collapsed_capsule_hugs_the_screen_edge_instead_of_floating_with_the_bar` and
  `edge_margin_interpolates_between_the_capsule_tuck_and_the_bar_float`.
- Every morphed body stays inside `capture_footprint`. Enforced by
  `dock_morph_keeps_capture_and_shadow_damage_stable` and
  `settled_maximized_collapse_keeps_the_capsule_composited`.

## Rejected Alternatives & Negative Knowledge

### Reuse `DOCK_EDGE_MARGIN` with a smaller global value

- **Why considered**: One constant is simpler than two, and shrinking the bar's
  float would also close the capsule gap.
- **Why rejected**: The 12 px float is load-bearing for the *expanded* bar: it
  separates a detached panel from the edge it floats near. Shrinking it
  globally would flatten the resting Dock to fix a collapsed-state problem, and
  `Chrome::reserved()` (the work-area reservation consumed by tiling and by the
  control-center tray) is derived from it.

### Keep the capsule floating, only thinner

- **Why considered**: Strictly less code — no interpolation, no corridor, no
  capture change.
- **Why rejected**: It does not address the actual complaint. The capsule was
  unreadable as an edge affordance *because* of the gap, not because of its
  thickness; a thinner floating capsule is still a floating capsule.

### Tighten the capsule and let the reveal strip shrink with it

- **Why considered**: Conceptually tidy — one rect describes both the visual
  body and the pointer target, and the target always matches what the user sees.
- **Why rejected**: It trades a cosmetic fix for a real interaction regression.
  The reachable strip would drop to a 6 px stripe at the bottom edge, making
  the parked Dock materially harder to summon — the exact failure ADR-0146 was
  written to prevent, reintroduced sideways through a geometry change rather
  than a timing one.

## Consequences

### Positive

- The collapsed Dock reads as the edge's own handle instead of a hovering
  artifact, and the reveal now visibly lifts the bar off the edge instead of
  teleporting it.
- Pointer reach is now an explicit, tested contract rather than an incidental
  consequence of the visual offset, so the two can be tuned independently.
- The capture envelope is derived from endpoints only, so it remains
  animation-stable.

### Negative / Trade-offs

- Two constants and one pure function replace one constant. The cost is that a
  future tweak to either value is now a *two-place* conversation;
  `tucked_capsule_keeps_its_reveal_corridor_deeper_than_the_capsule` is what
  makes that visible instead of silent.
- `capture_footprint` grows 10 px toward the anchored edge, so the blur capture
  and damage region are marginally larger. This is bounded (one tuck band, not
  a per-frame quantity) and required for correctness.
