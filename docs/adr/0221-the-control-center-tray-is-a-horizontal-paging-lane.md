---
id: ADR-0221
title: "The Control-Center Tray Is a Horizontal Lane That Pages, and Both Ends of the Pager Are Real States"
status: accepted
date: 2026-11-06
scope: shell/control-center, shell/component, i18n
superseded_by: null
negative_knowledge: true
---

# 0221. The Control-Center Tray Is a Horizontal Lane That Pages, and Both Ends of the Pager Are Real States

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Maintainers
- Amends: [ADR-0197](0197-control-center-tray-shelf-and-overflow-disclosure.md) (the shelf becomes horizontal and its overflow becomes paging instead of disclosure), [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md)
- Related: [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md) (footprints are measured, not guessed)

---

## Context and Problem Statement

ADR-0197 moved the tray to the bottom-left and made it a **vertical** shelf that
grows upward, with overflow handled by a trailing `+N` disclosure cell that
expands into a grid popover. Its own rejection list dismissed horizontal
paging as "permanent state — current page, reconciliation when items appear and
disappear, popover re-anchoring across page turns — to recover one icon beyond
what a disclosed overflow already offers."

That rejection was measured against the *overflow* problem only, and the
measurement has not held up. Three things changed:

1. **The vertical anchor wastes the corner.** A bottom-left column one cell wide
   is the narrowest possible use of a corner that is mostly empty. Its height is
   derived (ADR-0197 fixed the cap), but on a 1080p output the lane's *width*
   span is 1,824 px against a derived height of ~908 px — the horizontal axis
   has twice the room, and unlike the vertical axis it is not contested by the
   profile chip.
2. **A disclosed overflow is a second surface for a first-class reading.** The
   tray's contract is ambient availability: a resident indicator should be
   visible without an action. The `+N` popover is a modal-ish grid the user has
   to open, read, and dismiss, and every item behind it is two clicks from
   activation instead of one. Paging costs one click per *page*, and the item
   itself is still one click.
3. **The popover was the panel's most fragile surface.** It had its own
   open/closed state, its own anchor rect kept fresh each frame, its own
   light-dismiss rule, an Escape rung, and a rebuild-everything hazard when the
   shelf re-laid-out under it. `tray_overflow_open` and `tray_overflow_anchor`
   existed only to serve it.

The request that prompted this record asks for exactly the shape ADR-0197
rejected: horizontal ordering, unbounded width, and paging with arrows that show
a disabled state at each end. On the evidence above, the request is right and
the rejection was scoped too narrowly.

## Decision Drivers

- Ambient availability: as many indicators as the display can hold, visible
  without an action.
- One click from the shelf to an indicator's own action, at any page.
- A pager whose ends are honest: a chevron that cannot go anywhere must look
  like it cannot go anywhere.
- No hidden state that can strand the shelf on a page that no longer exists.
- Geometry that is a pure function, testable without a GPU.

## Decision Outcome

### 1. The lane is horizontal, one row tall, bottom-left

The shelf occupies a strip from the left margin to the bottom band's left edge,
one cell row tall (`TRAY_CELL + 2 * TRAY_PAD`), with its bottom edge above the
dock's reserved band exactly as before. Its **capacity is a function of the
anchor's width**:

```
slots = floor((anchor_w - 2 * TRAY_PAD + TRAY_GAP) / (TRAY_CELL + TRAY_GAP))
```

`[INV-CHROME-02]` is unchanged in force and now reads on width: no constant may
bound the slot count, only the display and the reserved bands may.

Cells are laid out left to right from the capsule's padding. The capsule hugs
its *occupied* slots rather than filling the anchor, so a two-icon tray is a
small pill at the corner rather than a wide empty bar.

### 2. Overflow pages; it does not disclose

When `item_count > slots`, the lane spends one slot on each chevron and
paginates the remainder:

```
items_per_page = max(slots - 2, 1)
pages          = ceil(item_count / items_per_page)
page           = min(requested_page, pages - 1)
first_item     = page * items_per_page
```

`[INV-CHROME-03]` (no scroll viewport) survives verbatim: a page is a whole-number
slice of the lane, never a fractional scroll offset, and there is still no clip
to make a cell unreachable.

`[INV-CHROME-04]` is **retired**. Its declaration ("Tray overflow is disclosed,
never hidden behind navigation") is superseded here; the disclosure cell, its
popover, `overflow_bounds`, `tray_overflow_open`, `tray_overflow_anchor`, the
popover's light-dismiss rule and its Escape rung are deleted, not defaulted.

`[INV-CHROME-07] Paging is bounded and reversible.` The page index is clamped by
the resolver, never by a caller: an item leaving shrinks `pages` under the
reader's feet, and the shelf must land on a real page rather than an empty lane
with live chevrons. Each chevron's reachability is a property of the resolved
layout (`has_previous` / `has_next`), not a judgement the renderer makes, and
both are enforced by unit tests over the full item-count range.

### 3. Both ends render their own state

A chevron whose end is unreachable is drawn **dimmed and inert**: it occupies
the same slot geometry and keeps the same hit target, but it is not a pressable,
so it neither highlights on hover nor swallows a click. The alternative — a live
pressable that does nothing — is worse than no chevron at all: it promises a
page that is not there.

The chevrons are present on *every* page while the shelf is paged, including a
disabled end. The lane therefore keeps a constant length as the user turns pages,
so the icons do not slide under the pointer.

### 4. Geometry is one pure resolver

`tray_shelf::shelf_layout(anchor, item_count, page)` returns capacity, pages,
page, `items_per_page`, `first_item`, `paged`, and the capsule rect; `slot_at`
says what slot `i` draws (`Item(index)`, `Previous` or `Next`) **without
allocating** — the render path walks it once per slot per frame, so a `Vec` there
would be per-frame heap churn; `slot_rect` gives every slot's box from one
uniform pitch. The renderer, the pressable ids, the menu anchors and the fallback
rects all derive from that one value, so a slot is never drawn somewhere it
cannot be hit ([INV-CHROME-05], unchanged).

The resolved layout is cached in `tray_shelf` for the frame. Its *stated* purpose
is hit-testing parity; today the click-away walk tests the `anchor` band (which
encloses the capsule) rather than the per-slot rects, so the cache is the seam a
future per-slot hit test would read rather than a load-bearing reader. That is
recorded here rather than left as a comment claiming a consumer that does not
exist.

A cell's rect is `drawn_or_declared`: the pressable's resolved box when lens has
one, the layout's declared slot on the first frame. Deriving the menu anchor and
the damage band from different sources is exactly how a drawn shelf and an
interactive shelf drift apart.

### 5. A page turn is applied after the frame

The press is captured inside the layout closure and the page index changes after
it, so the icons the pointer is over cannot move mid-press. A popover anchored to
a cell that is no longer drawn on the new page is dismissed rather than
re-anchored: leaving it would float a menu over the wrong icon.

## Consequences

### Positive

- Capacity roughly doubles against the vertical shelf on every landscape
  display: the lane's usable span is the display width less the bottom band,
  where the column was bounded by the height left below the profile chip.
- Every resident indicator is one click from its own action on the current
  page, and one chevron click from its action on any other page — against two
  clicks behind the retired `+N` popover.
- Two state fields (`tray_overflow_open`, `tray_overflow_anchor`), a popover
  renderer, a popover bounds function, a light-dismiss rule and an Escape rung
  are deleted.
- The disabled ends make the pager's position legible without a page counter:
  two dead-looking chevrons mean "one page", a live right chevron on the first
  page means "more to the right".

### Negative / Trade-offs

- The shelf now has real paging state: `tray_page`, its clamp, and the rule that
  a popover whose cell left the page is dismissed. ADR-0197's rejection priced
  that state correctly; the price is paid deliberately because the capacity and
  reachability gains are larger.
- On a display narrow enough that the lane admits only a handful of slots, a
  tray of six or more items pages where the `+N` cell would have listed them in
  one grid. Both are two interactions from the sixth item; paging is one click
  per page and the `+N` grid was one click per item.
- The lane shares the corner's baseline with the work-mode/power band, so on a
  very narrow output the two compete for width. The lane yields first: it clamps
  to a single cell before the band is squeezed.

## Rejected Alternatives & Negative Knowledge

### Keep the vertical shelf and merely raise its capacity
- **Why considered**: Zero layout risk, no new state, and the derived-height rule
  already removed the cap.
- **Why rejected**: It leaves the corner's long axis unused. The vertical span
  is bounded above by the profile chip, which is a hard ceiling the horizontal
  span does not have; on a 1080p output it is ~908 px against ~1,824 px.

### Horizontal lane with the disclosure popover kept for a long tail
- **Why considered**: Paging for the common case and the complete grid for a
  very large tray would give both "one click per page" and "everything in one
  surface".
- **Why rejected**: Two overflow mechanisms that can both be present is two
  states to reconcile, two dismissal rules, and a user who cannot predict which
  one appears. One mechanism, consistently applied, is the smaller surface.

### A page counter (`2 / 5`) between the chevrons
- **Why considered**: It states the position unambiguously, which the chevrons
  only imply.
- **Why rejected**: It costs a slot — one resident indicator per page — and the
  disabled ends already answer "am I at an end". The count of pages is not an
  operation the user performs; the position is implied by the ends.

### Live chevrons that clamp at the ends instead of a disabled state
- **Why considered**: Simpler UI: every chevron is always pressable, and the
  clamp is already in the resolver.
- **Why rejected**: A pressable that provably does nothing is a lie about the
  state of the surface, and it also swallows the press so a click meant to
  dismiss something else becomes a no-op. The end state is a reading, so it is
  drawn as one.

### Wrapping the lane onto a second row instead of paging
- **Why considered**: No paging state at all, and more items visible at once.
- **Why rejected**: A second row grows the shelf upward into the profile chip's
  band on a short display, and the growth is unbounded: the third row pushes
  further. Paging keeps the shelf exactly one row tall, which is what makes its
  placement rule a constant rather than a function of the item count.

## Verification

- `cargo nextest run -p tessera-shell --lib -E 'test(tray_shelf)'` — the pure
  geometry: capacity is monotonic in the anchor width and inverts the occupied
  width exactly, the page index clamps into range, every item lands on exactly
  one page across the full range, page turns are exact inverses, the last page
  may be short but keeps both chevrons, both ends report their own reachability,
  slots advance left to right inside the capsule, and a one-slot anchor still
  pages forward instead of livelocking.
- `cargo nextest run -p tessera-shell --lib -E 'test(the_tray_lane)'` — the
  integrated bounds: capacity grows with the display, the lane never enters the
  dock's reserved band, it clears the profile chip, and it ends before the
  bottom band.
- `cargo nextest run -p tessera-shell --lib -E 'test(chevron)'` — the pager is a
  real pressable end to end: the trailing chevron turns forward, walks to the
  last page, and then does nothing; the leading chevron returns to the first
  page and then does nothing.
- `cargo nextest run -p tessera-shell --lib -E 'test(page)'` — a page that stops
  existing resolves to a real one, and a popover whose cell left the page is
  dismissed.
- `cargo nextest run -p tessera-shell --lib -E 'test(tray)'` — the pre-existing
  cell activation, right-click menu, and rollover regression tests still pass on
  the new lane.
- `cargo run -p xtask -- check-adrs` — registry integrity for this record.
