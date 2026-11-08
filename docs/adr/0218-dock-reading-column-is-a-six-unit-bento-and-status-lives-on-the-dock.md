---
id: ADR-0218
title: "The Dock's Reading Column Is a Six-Unit Bento, and Status Lives Where the Hand Already Is"
status: accepted
date: 2026-11-06
scope: shell/dock, shell/component, design/tokens, runtime/presentation, architecture
superseded_by: null
negative_knowledge: true
---

# 0218. The Dock's Reading Column Is a Six-Unit Bento, and Status Lives Where the Hand Already Is

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Maintainers
- Amends: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) (the dock's Bento cell vocabulary grows from one kind to two), [ADR-0214](0214-tiered-system-readings-and-a-barrier-only-at-the-end.md) (a level reading gains a persistent surface, so the OSD is no longer the only place a charge is visible), [ADR-0175](0175-bluetooth-peripheral-subsystem-and-bluez-bridge.md) (a Bluetooth reading reaches the dock)
- Related: [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md), [ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md) (the reveal corridor this column lives inside), [ADR-0217](0217-idle-chrome-does-not-keep-the-frame-clock-and-hover-damage-is-bounded-by-the-component-that-owns-it.md) (the damage discipline the column inherits)

---

## Context and Problem Statement

ADR-0213 removed the status HUD and answered "where do the readings go" with
three surfaces: the workspace pager became the dock's leading Bento cell, charge
and link state became transient system readings (ADR-0214), and everything
interactive moved into the control center. That split is right about the
*top* of the screen — it belongs to windows — and wrong about one thing.

**A transient reading answers "what just changed", never "what is it now".** An
OSD is raised when a link comes up or a charge crosses a threshold and is gone
in 2.6 seconds. The question a user actually asks a dozen times a day is the
static one: is this machine on the office Wi-Fi or a phone hotspot; are the
headphones still connected or did they drop to the speakers; how much charge is
left before I unplug. Answering that today costs `Super+S` and a full-screen
modal. That is a three-order-of-magnitude mismatch between the question and the
surface.

The dock is the only persistent chrome left on the screen, and it is already
where the hand goes. ADR-0213 said as much when it moved the workspace pager
there: *readings belong where the question is asked*. The pager proves the
shape works. What the dock has no vocabulary for is a second reading — and no
answer at all for how several readings coexist in a bar whose only axis is
length.

### The layout problem this exposes

The dock is a **one-dimensional strip**. Every cell it holds today is a span
along that axis: a tile is 56 px of length, the pager is a computed span of
length, the two sections are separated by a gap. Adding three readings to a
one-dimensional strip has no good answer: three more spans make the bar 100+ px
wider for information that is not an app, and a bar that grows with the number
of radios is a bar that jumps around when a radio is unplugged.

There *is* a second axis, and it is unused. The bar is 74 px tall and a resting
tile is 56 px, so a column as tall as a tile fits inside the bar with 9 px of
margin at each end. What the dock lacks is not space — it is **an algebra for
that space**, so that a second reading and a third reading are the same design
rather than two special cases.

## Decision Drivers

- **A glance must not cost a modal.** If reading the charge requires opening a
  full-screen panel, the charge will not be read.
- **The bar's length is not the place for machine state.** A strip that widens
  with the number of radios makes every dock position a function of hardware
  inventory.
- **Three readings must be one design, not three.** A layout that only works
  for exactly three cells is a layout that breaks the first time a machine has
  no Bluetooth radio.
- **A floor on cell size is a floor on legibility and on target size.** An
  unconstrained subdivision of a fixed box eventually produces a cell that is
  neither readable nor clickable.
- **No reading without a consumer, and no reading that is not true.** A glyph
  drawn for a radio the machine does not have is a lie; a threshold invented in
  the dock is policy the dock does not own.

## Considered Options

- **Keep the readings transient only.** Rejected: it answers the change and not
  the state, which is the question that is asked far more often.
- **Put the readings in the bar's length, as more leading cells.** Rejected:
  the bar widens with hardware inventory, the leading section becomes a
  horizontal jumble of unrelated statuses, and the section boundary that
  separates "machine" from "apps" (ADR-0213) stops reading as a boundary.
- **A separate small floating status widget beside the dock.** Rejected: a
  second persistent floating surface is a second thing competing with content,
  and it reintroduces exactly the detached-chip pattern ADR-0213 deleted.
- **A fixed three-row column with a row per reading, present or not.** Rejected:
  a machine with no battery would carry a permanently empty row, and the column
  would spend a third of its height saying nothing.
- **A cross-axis Bento grid inside the bar's thickness, with a minimum span of
  two units out of six and an exact tiling of whatever spans are present.**
  Chosen.

## Decision Outcome

Chosen option: "A cross-axis Bento grid inside the bar's thickness, with a
minimum span of two units out of six and an exact tiling of whatever spans are
present", because it gives the dock a second axis without touching the first,
and because the minimum-span rule makes every column that can exist legible by
construction instead of by inspection.

### The grid: six units, minimum two, at most three rows

The bar's thickness is divided into **six units**. A reading occupies a whole
number of units, and **a reading may not occupy fewer than two**. Two
consequences follow, and both are the reason for the rule:

- **The number of rows is bounded by the algebra, not by a second constant.**
  `COLUMN_UNITS / MIN_ROW_UNITS = 3`, so three readings is the ceiling. Three
  rows is also the limit of what the eye resolves without scanning, so the
  arithmetic bound and the perceptual bound agree — and the code derives the
  first from the second rather than asserting both.
- **Every span that can exist tiles the column exactly.** With `n` readings
  present, each takes `6 / n` units:

  | rows | units per row | tiling |
  |------|---------------|--------|
  | 1    | 6             | `6` |
  | 2    | 3             | `3 + 3` |
  | 3    | 2             | `2 + 2 + 2` |

  There is no remainder to distribute, no separator that consumes a unit, and
  no fractional pixel: 6 is divisible by every `n` the minimum-span rule
  permits. This is why the unit count is six and not, say, eight or five.

A **separator is not a unit**. Rows are separated by a hairline drawn *inside*
the boundary between two rows, snapped to the device pixel grid
(`snapped_hairline`), not by a gap that consumes a share of the height. Spending
one of six units on a divider would take 17% of the column to draw a 1 px line,
and — worse — would make the three-row case `2+1+2+1+2 = 8` units, which does
not fit in six. The grid is a *content* grid; the divider is paint.

### The column is one cell on the bar's long axis

The reading column occupies **one span of the bar's length** — `COLUMN_WIDTH`,
a narrow plate as tall as a resting tile — and the readings are distributed
across its *thickness*. So the bar grows by one fixed width when the column
appears, not by one width per reading: unplugging a Bluetooth radio changes what
is inside the column and never moves an icon.

This is the property the whole design rests on. The dock's length is a function
of *what the user pinned*; the dock's thickness was already paid for.

### A reading appears only when the machine has it

The resolver gates each reading on the capability, not on the value:

- **Wi-Fi** appears when the wireless radio service exists
  (`wifi_enabled.is_some()`).
- **Bluetooth** appears when the Bluetooth radio service exists
  (`bluetooth_enabled.is_some()`).
- **Battery** appears when the machine reports a battery (`battery.is_some()`).

A machine with none of the three resolves an empty column, which occupies zero
length and inserts no boundary gap — the same "auto-hide with zero footprint"
rule the single-workspace pager already follows. A machine with one reading
resolves a one-row column at full height. Nothing in between needs a special
case.

### Tone is a state, and the states are named

Each reading resolves one of three states, and the *glyph* and the *tone* are
both derived from it rather than from the raw snapshot fields:

- **`Linked`** — the radio is up and carrying a session (Wi-Fi `Connected`,
  Bluetooth `Connected`; a charging battery is `Linked` to power).
- **`Idle`** — the radio is on and not carrying a session (scanning,
  connecting, associated-but-down, idle).
- **`Off`** — the radio is off, or the service behind it is unavailable.

The distinction is the whole point: "Bluetooth is on but nothing is connected"
and "Bluetooth is off" are different answers and must not draw the same mark.

The mark itself is coarser than the tone, deliberately. `Linked` and `Idle` share
one glyph and differ only in the ink ramp, because both mean "the radio is on" and
the ramp already says which; `Off` is the struck-through glyph. That keeps the
vocabulary at two marks per radio rather than three, so the row reads at a glance
instead of requiring a comparison. The Bluetooth row's *count* of connected
peripherals is a dot in its corner, never a digit: at 11 px a numeral is a smudge,
and "something is connected" is the answer the row exists to give. A count the
dock cannot render legibly is a count the dock does not render.

### The battery is a gauge, not a number

A battery reading draws the level as a fill inside a track, because a charge is
a magnitude and a magnitude wants an extent. It does not print a percentage:
at a 30 px plate the number would be illegible, and the honest alternative to
legible digits is a legible proportion. It does not colour itself against a
threshold either — thresholds are `[battery] warn_at` policy owned by the
runtime (ADR-0214), and a dock that invented its own would disagree with the
alert that eventually fires. Charging is the one state the glyph carries
(`LENS_ICON_BATTERY_CHARGING`), because charging is a fact and not a policy.

**The gauge is a band along the row's own long axis, not a segment laid end to
end with the glyph.** The end-to-end arrangement is the obvious one and it does
not fit: the row is 24 px wide, the glyph takes 10–15 of them, and what is left
for the gauge is 5.1 px at one row and 9.7 px at three — so a 40% charge is a
2 px sliver in *every* geometry the column can produce. A band hugging the row's
trailing edge costs the row nothing on its long axis, giving the gauge 20 px at
three rows and 46 px at one, and the glyph is centred in the space the band
leaves.

**Orientation comes from the row's aspect, never from the dock's edge.** A bottom
dock with one reading has a *tall* row (24 × 50) because rows stack down the
plate's height; keying the gauge on "the dock is at the bottom" laid a horizontal
gauge inside a 24 px row. The row knows its own shape and the dock edge does not,
so the only input is `row.w >= row.h`.

### Pressing a reading opens the surface that owns it

A reading is a *shortcut*, not a control: pressing it opens the control center,
which already owns the toggles, the network list, and the device list. The dock
gains no new state, no new IPC, and no new system action — it emits one
`ChromeEvents::open_control_center`, and the host answers it with
`Shell::open_control_center`, which is deliberately **open-only**: pressing a
reading while the panel is already open must not close the panel the user is
looking at.

Hovering a reading tints its row, so the plate reads as three pressable rows
rather than as one flat capsule. The row hit test is the same algebra as the
tiling: `row_at` divides the plate by the row count it already knows.

### The dock's cell vocabulary becomes two shapes

`BentoCell`/`TileKind` described one thing (the pager) and could not describe a
second without a flag. It is replaced by `StripCell` — an enum of the two cells
that lead the bar:

- `StripCell::Readings(BentoColumn)` — the machine's own state, resolved from
  the status snapshot.
- `StripCell::WorkspacePager { workspaces }` — the interactive pager, unchanged
  in behaviour and geometry.

Order is fixed and semantic: **machine readings lead, then the pager, then the
icons**. The reading column is a `plate` the user presses to reach the machine;
the pager is navigation. Both are `Bento` in the sense that matters — cells
whose spans are computed rather than assumed — and the strip length is still
resolved by exactly one function that the paint, the hit test, the magnification
rest estimate, and the damage envelope all read.

### The column resolves to a paint plan before it paints

The layout is a pure function of `(plate, column, hovered, progress, orientation)`
returning the exact rects the paint will place — plate, row hovers, each reading's
glyph, each battery's track and fill, each separator — and the paint is a thin
loop over that list.

This is not decoration. The two obvious ways to test a renderer are both bad: a
test that re-runs the layout arithmetic passes on a renderer that places nothing,
and a test that runs a frame asserts against a GPU context the unit suite does not
have. Splitting the plan out means the layout's claims — rows tile the plate, the
gauge's fill is the charge's proportion, no planned rect escapes the plate, a
separator is a hairline and not a row — are ordinary assertions over data.

The plan is also what makes the *hit test* and the *paint* provably the same
algebra: both derive from `row_rects`, so a press cannot land on a row the paint
did not draw.

### Invariants & Behavioral Boundaries

- `[INV-DOCK-BENTO-1] The grid is six units and the floor is two.` A reading
  spans `COLUMN_UNITS / n` units for `n` readings present; no reading may span
  fewer than `MIN_ROW_UNITS`, and the maximum row count is *derived* as
  `COLUMN_UNITS / MIN_ROW_UNITS`, never written as a literal beside it.
- `[INV-DOCK-BENTO-2] Rows tile the plate exactly.` The union of the row rects
  is the plate, with no overlap and no remainder, for every row count the
  resolver can produce. A separator is a hairline inside the shared boundary and
  consumes no unit.
- `[INV-DOCK-BENTO-3] The column costs one span of length, whatever it holds.`
  Adding, removing, or re-toning a reading never changes the bar's length. Only
  the column's presence does, and its width is a constant.
- `[INV-DOCK-BENTO-4] A reading is drawn only for a capability the machine
  reports.` No glyph is drawn for a radio the snapshot says does not exist, and
  an empty column reserves zero length and inserts no boundary gap.
- `[INV-DOCK-BENTO-5] One strip-length source.` `strip_length` is the only
  arithmetic that resolves the bar's length; the paint, the hit test, the rest
  centre estimate, and the capture footprint all consume it. A second hand-rolled
  sum is how the leading section silently falls out of the layout.
- `[INV-DOCK-BENTO-6] A reading press opens, and only opens.` The dock emits
  `open_control_center`; the host answers with an open-only path. No reading
  press toggles a surface closed, and the dock itself never mutates system
  state.
- `[INV-DOCK-BENTO-7] The layout is a plan, not a side effect.` The rects a
  reading column paints are resolved by a pure function before any frame exists,
  and the hit test reads the same row geometry the paint places. A test asserting
  a column's layout must not need a rendering context, and a press must not be
  able to land in a row the paint did not draw.
- `[INV-DOCK-BENTO-8] A row's orientation comes from the row.` A reading's inner
  layout is chosen from `row.w`/`row.h`, never from the dock's edge or from the
  row count. Rows stack down the plate's height on a bottom dock and across its
  width on a side dock, so a bottom dock's *single*-reading row is tall — keying
  the inner layout on the edge produced a horizontal gauge inside a 24 px row.
- `[INV-DOCK-BENTO-9] Every reading stays legible in every geometry.` For each
  row count the algebra permits and each dock orientation, a reading's mark is at
  least 9 px and a battery's gauge band is at least 14 px on its long axis. A
  layout that shrinks an element below that has traded the reading for the
  layout.

## Rejected Alternatives & Negative Knowledge

### A row per reading with fixed rows

- **Why considered**: the simplest possible mapping — three readings, three
  rows, always.
- **Why rejected**: a desktop with no battery and no Bluetooth would carry two
  permanently empty rows, spending two thirds of a 56 px column on nothing. It
  also makes the row height a constant that has to be re-tuned whenever the
  reading count changes, which is precisely the coupling the unit algebra
  removes.

### A separator that consumes a unit

- **Why considered**: it keeps "everything in the column is a unit" as a single
  rule, with no exception for paint.
- **Why rejected**: one unit is 17% of the column, so a 1 px line would arrive
  with 8 px of blank on each side; and the three-row case would need eight
  units, which does not fit in six. The exception ("the divider is paint, not
  content") is cheaper than the layout it forces.

### Printing a battery percentage

- **Why considered**: a number is unambiguous, and the OSD already prints one.
- **Why rejected**: at a 30 px plate the digits are illegible, and an illegible
  number is worse than a legible proportion because it looks like information.
  The proportion is what the glance is for; the exact number is one press away.

### A battery threshold colour

- **Why considered**: a red battery at 5% is the conventional cue, and the dock
  already has a critical tone available.
- **Why rejected**: the thresholds are `[battery] warn_at` — user policy owned
  by the runtime and applied by `BatteryWarningLatches`. A dock that hard-coded
  its own would disagree with the alert that eventually fires, and two surfaces
  disagreeing about whether the battery is low is worse than one surface being
  quiet.

### Widening the bar for each reading

- **Why considered**: it needs no second axis, so the strip stays a strip.
- **Why rejected**: it makes the dock's length a function of hardware
  inventory. Plugging in a Bluetooth dongle would shift every icon in the bar
  and re-centre the whole dock — a chrome surface that moves because a radio
  appeared.

### Keeping `BentoCell`/`TileKind` and adding a variant per reading

- **Why considered**: additive, no churn at the call sites.
- **Why rejected**: `TileKind` is a one-dimensional cell vocabulary with a
  per-kind width function; a reading column is not a span of a per-kind width,
  it is a *grid* whose span is constant and whose interior is computed. Bolting
  a grid onto a width function would have made `rest_width` the wrong question
  for half the enum.

## Consequences

### Positive

- The three questions asked most often — which network, is the headset up, how
  much charge — are answered by a glance at the surface the hand is already
  moving toward, with no modal and no keybinding.
- The dock's length is now provably independent of hardware inventory: the
  column costs one constant span, and the row count changes only what is inside
  it.
- The reading vocabulary is a grid, so a fourth reading is an algebra problem
  with a known answer (the floor rises to three units and the ceiling stays
  three rows) rather than a layout rewrite.
- `StripCell` states the two things a leading cell can be, so the strip length,
  the hit test, and the paint each match on two cases instead of on a growing
  enum.

### Negative / Trade-offs

- The column consumes 30 px of the bar's length on every machine that has any
  radio at all. On a three-icon dock that is a visible share of the bar; the
  alternative is a bar that resizes with hardware, which is worse.
- At a two-row or three-row column each reading is 28 px or 18.7 px tall, which
  is a glanceable indicator and not a comfortable click target. The press is
  therefore a shortcut into the control center rather than a control, and the
  row hit test is generous (the full row width) to compensate.
- The battery is a proportion, so "4% or 8%" is not answerable from the dock.
  That is accepted: the alert is the surface for the low-charge case
  (ADR-0214), and the dock's job is the coarse answer.
