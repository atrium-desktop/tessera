---
id: ADR-0187
title: "Offscreen Stream Damage Comes From the Stream's Own Surface Tree"
status: accepted
date: 2026-10-25
scope: composite/stream, runtime/stream
superseded_by: null
negative_knowledge: true
---

# 0187. Offscreen Stream Damage Comes From the Stream's Own Surface Tree

- Status: Accepted
- Date: 2026-10-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md), [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)

---

## Context and Problem Statement

A stream frame carries an optional `damage` list: a hint naming the regions
that changed since the previous delivered frame. The physical *output* stream
([ADR-0052](0052-scoped-output-frame-streaming.md), [ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md))
crops the presented desktop framebuffer, so the presented frame's own damage —
already accumulated by `OutputStreams::accumulate_damage` from the composition
root — describes exactly its coordinate space. Reusing that accumulator for the
*offscreen* stream classes was wrong:

- A **window stream** ([ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md))
  renders its own surface tree into its own target, positioned at the window's
  logical origin. It sampled the desktop accumulator clipped to the window rect
  — approximate, and blind to content changes that never reached the physical
  frame (an occluded, minimized, or foreign-workspace window).
- A **virtual-output stream** ([ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md))
  is worse. Its windows are deliberately excluded from the physical scene
  (`[INV-ARCH-76]`), so the physical accumulator can never contain their
  changes: the frame's pixels change while its `damage` hint reads "nothing
  changed". A hint that is not merely imprecise but *inverted* is a defect, not
  an optimisation.

The composition root also had to invent a physical `damage_origin` per frame
(`geometry.origin × scale`) so the desktop-space rects landed somewhere
plausible — a coordinate translation that only papered over the mismatch.

## Decision Drivers

- An offscreen frame's repair regions must describe the stream's own content in
  the stream's own target coordinates.
- The client-damage math (generation diff, geometry-move detection, transform
  axis swap, viewport/transition unmappability, per-surface committed damage)
  already exists once; a second copy is forbidden
  ([Single Source of Truth](../dev/standards/architecture.md)).
- Over-reporting damage is always safe; a *wrong* region is not.
- The tracker must advance exactly once per *rendered* frame, not once per
  main-loop iteration (an iteration that merely observes a change it does not
  render must not consume the delta).

## Considered Options

1. **Offscreen streams derive damage from their own surface tree through a
   per-stream `ClientDamageTracker`, advanced once per render, mapped to
   target pixels by `offscreen_damage_to_target`.**
2. **Report full target damage on every offscreen frame.**
3. **Keep sampling the physical accumulator, but clip it to the stream rect.**

## Decision Outcome

Chosen option: **Option 1**.

### 1. Offscreen streams are removed from the physical accumulator

`OutputStreams::accumulate_damage` skips every `is_offscreen()` stream
(window and virtual output). Only physical output streams inherit the presented
frame's damage.

### 2. A per-stream tracker owns the offscreen repair regions

`CompositorRuntime.offscreen_damage: HashMap<u64, ClientDamageTracker>` holds
one tracker per offscreen `stream_id`. The drive supplies a `tree_damage`
closure that runs `ClientDamageTracker::assess` over the stream's own visible
surface set (SHM + dma-buf, the same lists the painter draws). The closure is
called **once per render, after the due decision**, so the tracker advances
exactly once per rendered frame. Its verdict is the existing `ClientDamage`
(`Full` / `Area(rects)` / `None`) in compositor logical coordinates — reusing
the single client-damage implementation rather than a second one.

### 3. Logical damage maps through the stream origin, then the target scale

`offscreen_damage_to_target(damage, origin, scale, physical_size)` shifts each
logical rect by `-origin` (the stream's logical origin in the desktop layout)
and maps it with the existing `logical_rects_to_frame`. `Full` and `None` pass
through. This is the offscreen analogue of `ClientDamage::into_frame_damage`,
which assumes logical `(0, 0)` maps to the target origin — false for a window or
virtual output at a non-zero desktop position.

### 4. The seeded accumulator keeps the existing drop/fold semantics

`OutputStreams::seed_offscreen_damage(stream_id, own)` unions the own-tree
damage into the stream's `damage_since_delivery` and returns the sample with
origin `(0, 0)`. Delivery still clears the accumulator (`note_delivered`) and a
dropped frame still folds its sample back (`fold_damage_back`), so the next
delivery covers the missed regions — the same machinery physical streams use.
A tracker entry is dropped when its stream stops, freezes, or renegotiates, so
a rebuilt target reports full damage on its first frame.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-79] Offscreen Damage Isolation`: A window or virtual-output
  stream's frame damage MUST be derived from the stream's own surface tree and
  MUST NOT be sourced from the physical desktop frame's damage accumulator.
  Enforced by `OutputStreams::accumulate_damage` skipping offscreen streams
  plus the `tree_damage` drive seam, and verified by
  `test_inv_arch_79_offscreen_streams_exclude_physical_damage` and
  `offscreen_damage_translates_origin_then_maps_to_target`.

## Rejected Alternatives & Negative Knowledge

### 2. Full target damage on every offscreen frame
- **Why considered**: The target is fully redrawn every render, so reporting the
  whole target is trivially correct and needs no tracker.
- **Why rejected**: It discards a real, already-implemented optimisation for the
  common case (a small cursor or clock change) and throws away the
  `ClientDamageTracker` that exists precisely to compute these regions once.

### 3. Clip the physical accumulator to the stream rect
- **Why considered**: The pre-existing window-stream behaviour; the smallest
  change.
- **Why rejected**: For a virtual output it is not approximately wrong but
  *inverted* — the stream's windows are excluded from the physical scene, so the
  accumulator reports "no damage" for content that visibly changed. It also
  forces the synthetic physical `damage_origin` translation this ADR removes.

## Consequences

### Positive
- Offscreen frame damage is precise and in the correct coordinate space, so a
  consumer that trusts it repaints exactly the changed regions.
- One client-damage implementation serves the desktop, window streams, and
  virtual-output streams.
- The synthetic physical `damage_origin` computation disappears from both
  offscreen drives.

### Negative / Trade-offs
- The composition root now holds a per-stream `ClientDamageTracker` and resets
  it on stop/freeze/renegotiate; a missed reset would under-report one frame,
  so the resets are exercised by `[INV-ARCH-79]`'s test and the drive seam.
- Virtual-output streams fetch their visible surface set twice per rendered
  frame (once for the dirty snapshot, once for the tracker). This mirrors the
  existing dirty-check fetch and is bounded by the output's window count.
