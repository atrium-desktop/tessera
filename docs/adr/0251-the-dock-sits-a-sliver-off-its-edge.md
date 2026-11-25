---
id: ADR-0251
title: "The Dock Sits a Sliver off Its Edge"
status: accepted
date: 2026-11-25
scope: shell/dock
superseded_by: null
negative_knowledge: true
---

# 0251. The Dock Sits a Sliver off Its Edge

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md), [ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md), [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md)
- Amended by: [ADR-0254](0254-dual-channel-reachability-and-component-mounted-hot-zones.md) — the collapsed stadium capsule is removed in favour of the unified eight-zone Hot Zone system; expanded dock `DOCK_EDGE_MARGIN` remains 4 px.

---

## Context and Problem Statement

[ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) gave the dock one
edge offset in both states, `DOCK_EDGE_MARGIN` = 12 px. The collapsed reveal
corridor ([ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md))
is composed *on top of* that offset — edge gap + capsule body + approach band —
so the parked dock's summon strip reached 12 + 4 + 16 = 32 px into the screen.

That is too high. Content near the bottom of a window (status bars, a terminal
prompt, a horizontal scrollbar) lives inside that strip, and a pointer working
there brushes the corridor and starts the reveal dwell. The dwell
([ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md)) rejects
most skims, but every false start is still a visible flinch of the capsule and
a chance to trap a click.

## Decision Drivers

- The parked dock should be summoned by reaching for the edge, not by working
  near it.
- The approach band (16 px) is the part of the corridor chosen *for* the
  pointer; the edge gap is the part that is only there because of where the
  pixels sit. Reduce the incidental part, not the deliberate one.
- Both states keep one offset (ADR-0213): lowering the dock must lower the bar
  and the capsule together.

## Decision Outcome

`DOCK_EDGE_MARGIN` drops from 12 px to **4 px** — the capsule's own thickness.
The bar still floats (a sliver of wallpaper and its painted halo separate it
from the edge), but it no longer hovers a tile-gap above it.

Consequences that follow from the existing composition, with no new mechanism:

- The collapsed reveal corridor is 4 + 4 + 16 = **24 px** deep (was 32).
- `Chrome::reserved()` on a non-autohide dock reserves `DOCK_PANEL_HEIGHT + 4`
  = 78 px (was 86), so tiled and maximized windows gain 8 px.
- `capture_footprint` and the expanded trigger move down with the body.

`AUTOHIDE_HANDLE_EDGE_MARGIN`, kept by ADR-0213 only as an alias of
`DOCK_EDGE_MARGIN`, is removed together with the always-zero tuck band in
`capture_footprint`.

### Invariants & Behavioral Boundaries

Domain contracts inside `tessera-shell`, enforced by unit tests (no `INV-` tag):

- The dock floats clear of its edge (`DOCK_EDGE_MARGIN > 0`), by no more than
  the parked capsule's thickness, and the collapsed reveal corridor reaches no
  more than 24 px into the screen. Enforced by
  `the_dock_sits_low_so_its_reveal_corridor_stays_out_of_the_content_path`.
- Both states still share one offset. Enforced by
  `both_dock_states_hold_the_same_distance_from_the_screen_edge`.

## Rejected Alternatives & Negative Knowledge

### Flush to the edge (0 px)

- **Why considered**: Lowest possible corridor (20 px) and the most screen for
  windows.
- **Why rejected**: The bar would read as a fused panel rather than floating
  chrome, its rounded bottom corners would meet the bezel, and the halo would
  have nothing to separate. The last 4 px buy the floating read for a small
  cost.

### Shrink the approach band instead

- **Why considered**: Same reduction in corridor depth without moving pixels.
- **Why rejected**: The approach band is the deliberate target ADR-0146 and
  ADR-0198 sized for summoning; the edge gap is incidental. Cutting the band
  makes the dock harder to summon on purpose while leaving the bar floating
  needlessly high.

### Revisiting ADR-0198's rejection of "a smaller global value"

ADR-0198 rejected a smaller global margin because the 12 px float was then
load-bearing for the expanded bar's floating read under glass, and because it
was fixing a *collapsed-only* problem. Neither holds now: ADR-0213 already made
the margin global to both states, and flat chrome paints its separation with an
elevation halo rather than relying on visible wallpaper. The reservation
coupling it cited is real and is accepted above as a benefit, not a cost.

## Consequences

### Positive

- Fewer accidental reveals: the summon strip shrinks by a quarter.
- Windows gain 8 px of work area on a pinned dock.

### Negative / Trade-offs

- The halo's lower edge is partly clipped by the screen edge at 4 px; the bar
  reads as floating mainly through its top and side halo.
