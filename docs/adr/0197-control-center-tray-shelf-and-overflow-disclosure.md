---
id: ADR-0197
title: "The control-center tray is a bounded shelf with overflow disclosure, not a scrolling column"
status: accepted
date: 2026-10-29
scope: shell/control-center, shell/tray, shell/components
superseded_by: null
negative_knowledge: true
---

# 0197. The control-center tray is a bounded shelf with overflow disclosure, not a scrolling column

- Status: Accepted
- Date: 2026-10-29
- Amended by: [ADR-0221](0221-the-control-center-tray-is-a-horizontal-paging-lane.md) — the shelf becomes a horizontal lane and its overflow becomes bounded, reversible paging; `[INV-CHROME-04]` is retired and replaced by `[INV-CHROME-07]`.
- Deciders: Tessera Maintainers
- Amends: [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md)

---

## Context and Problem Statement

The control center presents the StatusNotifierItem (SNI) tray as a vertical
icon column anchored at the display's **left-middle**, laid out inside a
`f.scroll(...)` viewport with a fixed height cap
(`(TRAY_CELL + TRAY_GAP) * 8` = 400 logical px, further clamped to 66 % of the
main panel's height). When the icons exceed that viewport the column scrolls,
and a scrollbar fades in on wheel activity.

Three independent problems compound here:

1. **The anchor wastes the display.** The column is capped at 400 px on every
   output, including 1920×1080 and 2560×1440. Measured against the constants in
   `presentation/tray_panel.rs`, the cap admits **7 icons** at every one of
   1280×720, 1366×768, 1920×1080 and 2560×1440. The same display has ~16 icon
   slots of vertical room below the profile chip at 1080p, and ~23 at 1440p.
   The cap is not a layout necessity; it is a legacy number.

2. **Scrolling is the wrong interaction for a tray.** A tray's contract is
   *ambient availability*: every resident indicator is visible without an
   action. A scroll viewport breaks that contract and adds hidden state — the
   user cannot tell how many items are off-screen, and nothing signals it. It
   also makes the wheel a tray-reveal trigger, which is why the panel carries
   `tray_scrollbar_reveal`, a decay clock, and a damage-band entry purely for
   a scrollbar nobody should need.

3. **The scroll viewport is a hit-testing hazard.** `lensi_interact` rejects
   a widget the pointer reaches when `lensi_point_clipped_by_scroll` says the
   presented snapshot's clip excluded it. Every cell inside a scrolled tray is
   therefore conditional on scroll geometry that only exists because of the
   viewport. (The adjacent defect that motivated this record — a hover plate
   drawn as a sibling `place()` overlay that occluded its own column — is fixed
   separately; see `cb52a22` and the `tray_cells_are_clickable_and_open_their_menu`
   regression test. This record removes the *structural* reason the tray's
   geometry is fragile at all.)

The request that prompted this analysis proposed relocating the tray to the
**bottom-left as an unbounded surface**, dropping the scrollbar, and paging the
overflow with left/right buttons. That direction is correct on all three
counts; two details of its literal form do not survive contact with the layout.

### What the bottom-left corner actually holds

The dock is a `persistent_decoration()` that reports `visible_during_modal()`,
so it stays visible while the control center is open. It is **bottom-centred**
and its length grows with the tile count:

| Display | Dock length, 8 tiles | Dock length, 25 tiles | Left edge at 25 tiles |
|---|---|---|---|
| 1280×720 | 518 px | 1638 px | 78 px — inside the corner |
| 1366×768 | 518 px | 1638 px | 78 px — inside the corner |
| 1920×1080 | 518 px | 1638 px | 141 px — clear |

A shelf anchored flush to the bottom-left therefore overlaps the dock on small
displays with a full dock. The corner is not the empty region the proposal
assumed; it is the one edge that already has a dynamic occupant.

### What the capacity actually is

The tray also has a *horizontal* gradient: the main panel is centred and 820 px
wide, so the lane between the left margin and the panel's left edge is wide on
large displays and narrow on small ones:

| Display | Left lane width | Cells across |
|---|---|---|
| 1280×720 | 186 px | 3 |
| 1366×768 | 227 px | 4 |
| 1920×1080 | 490 px | 9 |
| 2560×1440 | 810 px | 16 |

A left/right **paged horizontal** shelf in that lane would show 3 icons per
page at 720p — the form with the most hidden state performs worst exactly where
space is scarcest.

Vertical room below the profile chip, held clear of the dock's reserved band:

| Display | Vertical slots |
|---|---|
| 1280×720 | 9 |
| 1366×768 | 10 |
| 1920×1080 | 16 |
| 2560×1440 | 23 |

## Decision Drivers

- **Ambient availability.** A resident indicator must be visible without a
  scroll, a page turn, or a hover. Hidden state in a tray is a defect, not a
  budget.
- **No overlap with persistent chrome.** The dock owns the bottom edge and
  changes length at runtime. The shelf must be placed by a rule, not by a fixed
  offset that happens to be clear on the developer's monitor.
- **One layout authority.** Bounds must be a pure function of the display and
  the shell's own reserved bands, so they are unit-testable and identical in
  test, preview and production.
- **No paging state.** Paging introduces "which page am I on", which must be
  persisted, reconciled when items appear and disappear, and re-anchored for
  popovers. The cost is permanent; the benefit is one extra icon.
- **Continuous monotonic capacity.** Doubling the display's vertical room
  should roughly double the visible icons; the current fixed cap makes capacity
  independent of the display, which is the tell that it is an accident.
- **Delete the interaction, delete its state.** The scrollbar reveal, its decay
  clock, and its damage band exist only to serve an interaction being removed.

## Considered Options

- **A. Keep the vertical column at left-middle, keep scrolling, raise the cap.**
- **B. Move the tray to a bottom-left unbounded surface with left/right paging.**
- **C. Vertical shelf anchored bottom-left, sized to the true available column
  height, with overflow disclosure (`+N` expands into a grid popover).**
- **D. Keep the tray in the side column but let it grow to the full available
  height, with overflow disclosure and no scrollbar.**

## Decision Outcome

Chosen option: **C**, because it is the only option that satisfies ambient
availability for the common case while degrading in a *disclosed* way, and it
is the only one whose geometry is defined by rule rather than by a constant.

### 1. The tray becomes a shelf with a rule-derived height

The tray is renamed and re-specified as the **tray shelf**. Its anchor becomes
the **bottom-left** corner, and its height is derived, not capped:

```
shelf_top    = profile.bottom + PANEL_GAP
shelf_bottom = display.h - margin_y - reserved.bottom
shelf_h      = shelf_bottom - shelf_top
```

`reserved.bottom` is the bottom band the shell's persistent decorations declare
through `Chrome::reserved()` and publish to every component via
`ChromeUpdate::ModalReserved`. That is the dock's own declaration, so an
auto-hidden or fullscreen-locked dock (which reserves nothing) and a side-docked
dock (which reserves a side edge) both fall out correctly — with no mirrored
constant to drift.

The shelf is **bottom-anchored**: it grows upward from `shelf_bottom`. This
keeps the icons nearest the pointer's common approach path (the bottom edge)
stable as items are added — a top-anchored shelf would shift every existing icon
down when one appears.

`[INV-CHROME-02] The tray shelf's height is derived, never capped.` The shelf
must fill the vertical span between the profile chip's bottom edge plus
`PANEL_GAP` and the display's bottom margin less the reserved bottom band. No
constant may bound the number of visible cells; only the display and the
reserved bands may. Capacity therefore scales monotonically with the display,
and `cluster_bounds` must take the reserved band as an explicit parameter rather
than reading a module constant, so the layout stays a pure function of
`(display, reserved)`.

### 2. There is no scroll viewport

`[INV-CHROME-03] The control-center tray is not a scroll area.` The shelf must
not be built inside `Frame::scroll`, must not declare a scroll id, and must not
carry a scrollbar reveal, decay clock, or scrollbar damage band. A tray cell
must never be rejected for hit-testing because it fell outside a scroll
viewport's clip.

This is a **clean break**: `tray_scrollbar_reveal`, its `advance` decay, its
`interaction_anim_pending` term, and its `animated_damage_region` band are
deleted, not defaulted. The `Chrome::update`/`advance` paths that feed them go
with them.

### 3. Overflow is disclosed, never scrolled or paged

`[INV-CHROME-04] Tray overflow is disclosed, never hidden behind navigation.`
*(Superseded by [ADR-0221](0221-the-control-center-tray-is-a-horizontal-paging-lane.md):
the shelf is now a horizontal paging lane and this tag is retired. The paragraph
below is preserved as the negative knowledge for why disclosure was chosen
first, and what it cost.)*
When the cell count exceeds the shelf's slot count, the shelf renders the first
`n - 1` cells followed by a final **disclosure cell** showing `+N` for the
remaining `N`. Activating the disclosure cell opens a grid popover listing every
overflowing item with the same activate/context-menu affordances as a shelf
cell.

The disclosure cell is a *cell*, not a control: it occupies the same slot
geometry, participates in the same hover/open treatment, and is the last item
in the shelf. No left/right or up/down navigation controls exist.

`[INV-CHROME-05] Slot capacity is a pure function.` The shelf's slot count must
be computed by a pure function of `(display, reserved)` with no reference to
the item list, so the layout is identical for every item set and the overflow
decision is testable without a render pass.

### 4. Capacity is measured in whole slots

The shelf's slot count is `floor((shelf_h - 2 * TRAY_PAD + TRAY_GAP) / (TRAY_CELL + TRAY_GAP))`,
clamped to at least 1. The subtraction of one slot for the disclosure cell
happens **only** when overflow is present, so a shelf that fits exactly `k`
icons shows `k` icons and no disclosure cell.

### 5. The tray surface is one recessed capsule

The shelf keeps a single recessed capsule surface sized to its *occupied* slot
count, not to the full available height, so a two-icon tray is a small pill at
the bottom-left rather than a tall empty column. The capsule is the shelf's own
placed node; no per-cell overlay plate exists (see the ADR-0060 hit-testing
note in `[INV-CHROME-06]`).

`[INV-CHROME-06] Interactive chrome is never occluded by its own decoration.`
A hover, focus or selected-state backing must be drawn as the owning widget's
own background or as a node in a band strictly *below* the widget's band. It
must never be a sibling `place()` node in the same or a higher band drawn after
the widget, because an absolutely-placed node occludes the flow widgets inside
its subtree for hit-testing (Optics [ADR-0060](https://github.com/ming2k/optics/blob/main/docs/adr/0060-lens-single-tree-placement-and-z-bands.md)).
This invariant is what the pre-existing tray defect violated.

## Rejected Alternatives & Negative Knowledge

### B. Bottom-left unbounded surface with left/right paging
- **Why considered**: It is the literal shape of the request, and paging has
  zero hidden-item cost per page — everything is "one page away", which reads
  as fair.
- **Why rejected**: Three disqualifying facts, all measured above.
  **(a)** The corner is occupied: the dock is bottom-centred, stays visible
  during the modal, and grows with the tile count — 1638 px at 25 tiles, whose
  left edge lands at 78 px on a 1280-wide output, inside the corner. Paging
  would need to reflow around a bar that changes length at runtime.
  **(b)** The horizontal lane is the *narrowest* axis on small displays: 3
  cells at 720p. Paging performs worst exactly where capacity is scarcest.
  **(c)** It introduces permanent state — current page, reconciliation when
  items appear and disappear, popover re-anchoring across page turns — to
  recover one icon beyond what a disclosed overflow already offers.

### A. Keep left-middle, keep scrolling, raise the cap
- **Why considered**: Smallest possible diff; no anchor move, no new surface.
- **Why rejected**: It preserves every structural defect. Scrolling stays the
  ambient-availability break; the scrollbar reveal and its damage band survive;
  cell hit-testing stays conditional on a scroll clip; and capacity stays a
  constant that ignores the display. Raising a cap is not a design.

### D. Full-height side column with overflow disclosure
- **Why considered**: Achieves the same capacity as C on wide displays while
  keeping the familiar left-middle anchor, so the visual language does not
  change.
- **Why rejected**: The side column's height is bounded by the main panel's
  band, not by the display, so on the small displays where capacity matters
  most it gains least. It also leaves the tray's vertical extent centred, which
  means the "bottom is stable" property C gains — icons do not move when the
  set grows — is unavailable. C is strictly better on capacity and on stability;
  the only thing D preserves is a habit.

### Paging with up/down controls instead of left/right
- **Why considered**: It resolves the direction mismatch between a vertical
  shelf and left/right page controls, which would be genuinely confusing.
- **Why rejected**: It fixes the cosmetic half of paging and none of the
  structural half. The page-index state, reconciliation and re-anchoring costs
  are identical, and the disclosure cell reaches the same icon count with no
  state at all. Direction correctness is not a reason to keep paging.

### Shrinking the cell to 32 px to raise capacity ~1.5×
- **Why considered**: `TRAY_CELL`/`TRAY_GAP` directly bound slots; 32 px cells
  with a 6 px gap take 1080p from 16 to 23 slots with no layout change at all.
- **Why rejected**: It trades capacity for pointer precision on a surface whose
  targets are already the smallest in the panel, and it is orthogonal to this
  decision — slot capacity is now a derived value, so the cell size can be
  revisited on its own merits without touching the shelf's rules.

## Consequences

### Positive

- Capacity scales with the display: 9 / 10 / 16 / 23 slots at
  1280×720 / 1366×768 / 1920×1080 / 2560×1440, versus a flat 7 everywhere
  before.
- No scroll viewport in the tray, so cell hit-testing no longer depends on a
  scroll clip, and the scrollbar's reveal/decay/damage machinery is deleted.
- Overflow is visible (`+N`) instead of invisible (off-screen below the fold).
- No paging state to persist or reconcile.
- Shelf geometry is a pure function of `(display, reserved)`, so it is
  exhaustively unit-testable across display classes without a GPU.
- Bottom-anchored growth means a new icon does not displace existing icons.

### Negative / Trade-offs

- The tray moves from the left-middle anchor to bottom-left, so the panel's
  spatial composition changes and existing screenshots/previews read
  differently. This is the intended clean break.
- The shelf must know the dock's reserved band. That coupling is real; it is
  expressed as an explicit parameter to the layout function rather than by
  reading dock state, keeping the layout pure.
- Overflow now needs a second surface (the grid popover) that must be placed,
  dismissed, and kept out of the shelf's own hit-testing path.
- On very short outputs (below ~9 slots) a tray of three or more icons will
  disclose overflow, which is a visible regression against "everything fits" on
  such a display. Ambient availability is preserved by the `+N` count itself,
  which states how many are hidden.

## Verification

- `cargo nextest run -p tessera-shell --lib -E 'test(tray_shelf)'` — the pure
  geometry: capacity is monotonic in the anchor and independent of the item
  list, `shelf_slots` inverts `shelf_height` exactly, overflow reserves exactly
  one disclosure slot, every item of every overflow size is accounted for, the
  capsule is bottom-aligned and sized to occupied slots, and a narrow anchor
  keeps a 24 px cell.
- `cargo nextest run -p tessera-shell --lib -E 'test(the_dock_reservation)'` —
  the shelf's bottom edge follows the dock's declared reservation: a
  bottom-docked dock shortens it by exactly its band, a side-docked one does
  not, and its top (below the profile chip) does not move.
- `cargo nextest run -p tessera-shell --lib -E 'test(the_tray_shelf)'` — the
  integrated bounds: capacity grows with the display across 720p/768p/1080p/1440p,
  the shelf never enters the dock's reserved band, and it always starts below the
  profile chip.
- `cargo nextest run -p tessera-shell --lib -E 'test(disclosure_cell)'` — the
  `+N` cell is a real pressable: a headless pointer sequence over a
  deterministically oversized tray opens the overflow popover.
- `cargo nextest run -p tessera-shell --lib -E 'test(outside_the_overflow_popover)'`
  — the popover light-dismisses on a press outside it, symmetric with the
  dbusmenu popover.
- `cargo nextest run -p tessera-shell --lib -E 'test(tray_cells_are_clickable)'`
  — a shelf cell's pressable still receives the pointer and opens its dbusmenu
  popover, with the cell centre derived from the same pure layout the renderer
  uses ([INV-CHROME-05]).
- `cargo run -p xtask -- check-adrs` — registry integrity for this record.
