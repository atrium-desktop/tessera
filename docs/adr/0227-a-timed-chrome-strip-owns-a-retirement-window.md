---
id: ADR-0227
title: "A Timed Chrome Strip Owns a Retirement Window"
status: accepted
date: 2026-11-09
scope: shell/chrome, shell/damage, runtime/damage
superseded_by: null
negative_knowledge: true
---

# ADR-0227: A Timed Chrome Strip Owns a Retirement Window

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-shell` (Toast, shell damage query, scene build), `crates/tessera` (damage assessment, offscreen ring)
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md), [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md), [ADR-0220](0220-a-close-ghost-dissolves-separate-alpha-and-geometry-clocks-and-an-owned-handoff-frame.md)

---

## Context and Problem Statement

The notification toast strip is a static, frameless chrome surface. Its life is
driven by wall-clock age (`TOAST_TTL_MS`), not by a queue mutation or a spring:
when the last toast crosses its three-second window the component simply stops
rendering. `Chrome::anim_pending` deliberately returned `false` for the whole
lifetime, because pumping the frame loop at the output rate for three static
seconds is exactly the kind of idle-power regression the damage pipeline exists
to prevent.

That left the removal unowned. On the frame the stack empties:

- `requires_composition` becomes false, so the component is excluded from the
  shell pass;
- `damage_regions` returns `None` (there is no presentable toast), and the
  shell's damage query only visits participating components, so the band the
  toast just vacated is not named;
- `animation_deadline_in` had scheduled a wake at the TTL, but by the time the
  loop wakes the component no longer has a deadline, and `prepare_frame` does
  not rebuild for a deadline that has already vanished.

On the direct DRM path the output is a rotation of three offscreen images. A
frame that does not name the vacated band repaints only its own image; the
other two keep the toast. As the ring rotates the strip appears, disappears,
and appears again — a flicker whose source is neither the notification queue
nor the client. On a windowed surface the same hole leaves the last strip on
the persistent content image until an unrelated full repaint.

## Decision Drivers

- A timed visual state change must schedule a real frame at the moment it
  happens, not merely wake the loop.
- The frame that removes pixels must own their repaint footprint for as long as
  the presentation rotation can still expose them.
- A static strip must not keep the loop at the output rate for its whole life.

## Decision Outcome

### 1. Expiry is a lead, not a single instant

`Toast::anim_pending` returns true during the last
`TOAST_EXPIRY_LEAD_MS` (34 ms) before the oldest toast crosses its window, in
addition to the retirement window below. The loop is already due to wake at the
TTL; the lead ensures the wake lands on a real build with the toast still
absent, rather than on a stale deadline the component can no longer report.

### 2. The vacated band owns four more paints

When `Toast::render` sees an empty stack immediately after a non-empty one, it
arms `retire_band = last_band` and `retire_frames = 4`:

- `requires_composition` stays true while the retirement window is armed;
- `damage_regions` returns the vacated band (and the current band, if a new
  toast arrived);
- `anim_pending` stays true, so the frame clock runs and each ring image is
  asked to repaint the band;
- `render` decrements the budget once per build. Four is one more than the
  deepest supported offscreen rotation (`FLUX_MAX_FRAMES_IN_FLIGHT + 1`), so
  every image is cleared with margin.

A new toast arriving during the window cancels the retirement: its band
supersedes the vacated one and the normal damage path owns it.

### 3. The ring is the reason four frames, not one

One clear frame is enough only for a single-buffered destination. The direct
DRM surface is a rotation; each acquired image is a separate complete frame.
The retirement window is the chrome analogue of the donut's hand-off frame in
ADR-0220: the owner of the disappearing pixels keeps naming them until every
consumer has seen the clear.

## Consequences

### Positive

- A toast expires without flickering back on the image ring.
- The toast still costs no frames during its three static seconds; the cost is
  a 34 ms lead plus four small-band paints.
- The mechanism is local to the component that knows its footprint; no
  shell-wide per-component footprint tracker is needed for a strip whose
  geometry is a single rectangle.

### Negative

- A timed component that forgets its retirement window can still strand pixels.
  The rule is now explicit: *removing a timed strip requires naming its
  footprint for more than one frame*.
- Four frames of a 300×~350 logical band are repainted; this is bounded and
  invisible against the cost of the full repaint it replaces.

## Rejected Alternatives

### Rely on `animation_deadline_in` alone

Measured: the deadline is computed before the sleep; after the sleep the
component no longer has a presentable toast and returns no deadline. The build
trigger has to exist on both sides of the boundary, which the lead supplies.

### Keep `requires_composition` true forever and rely on the queue mutation

The toast's TTL is a wall-clock property, not a queue mutation. The queue may
retain an entry for much longer than the strip is visible; tying removal to it
would leave the strip on screen past its contract.

### Track every component's previous footprint in the shell

That is the general form and would also cover animated popovers. It requires a
new `Chrome` query for static footprints, a per-component store, and a
participation diff on every frame. The toast's geometry is one known rectangle
and its removal is the only timed event without a scene change; the targeted
retirement window is smaller, mechanically testable, and leaves the general
tracker available for a future component that actually needs it.
