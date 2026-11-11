---
id: ADR-0229
title: "The compositor supplies current damage; Flux owns the per-image ledger"
status: accepted
date: 2026-11-09
scope: composite/presentation, runtime, platform/drm
superseded_by: null
negative_knowledge: true
---

# 0229. The compositor supplies current damage; Flux owns the per-image ledger

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md),
  [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md),
  [ADR-0225](0225-damage-and-wakeups-follow-the-real-presentation-surface.md)
- Implements: Optics ADR-0155 (frame damage is a native epoch ledger)

---

## Context and Problem Statement

ADR-0203 made damage a placement-mapped rectangle set. ADR-0225 corrected the
ring key from the GPU frame slot to the acquired swapchain image. Those fixes
left the composition root owning a second per-image damage ledger:

```text
composite_image_damage: Vec<FrameDamage>
composite_repaint_for_image(...)
record_composite_present(...)
```

The engine already owned the images and, for windowed surfaces, an epoch
history. The caller-side ledger had to:

- initialise every image to `Full`;
- fold only the *current* delta into the other images after present;
- special-case a failed present because its damage had been assessed but not
  committed;
- collapse a retry to `force_full_redraw` when the ledger could not safely
  preserve the un-presented delta.

That is a second implementation of an engine invariant. The visible failure
modes — stale bands, a frame that rolls back, a toast that flickers during a
retry storm, and a single `Busy` becoming three full-output repaints — all sit
on this boundary.

## Decision Outcome

### 1. One ledger, owned by Flux

Tessera deletes `composite_image_damage`, `composite_repaint_for_image`, and
`record_composite_present`. A frame's repaint scope is asked of the engine:

```rust
let repaint = FrameDamage::from_flux_damage(frame.repaint_scope()?);
```

The composition root still owns the **current delta** — it is the only layer
that sees Wayland commits, chrome transitions, cursor sweeps, and the shell
scene. It passes that delta to `begin_frame_timeout_with_damage`; the engine
unions it with the history the acquired image missed.

### 2. Submission is the content transaction

`frame.submit()` commits the current delta and the acquired image's epoch.
`frame.present()` only publishes `presented_epoch`. A KMS `Busy` or flip
timeout abandons the flip, not the rendered image or its history.

Consequently:

- `Busy` / `FlipTimeout` no longer set `force_full_redraw`;
- the same image is reused on retry and asked only for its new delta;
- only a backend epoch/geometry change (`Inactive`, `Reconfigured`,
  `CursorFallback`, resize, lost flip) forces a full composite.

### 3. Scanout damage is a separate question

The KMS `FB_DAMAGE_CLIPS` hint comes from
`surface.scanout_damage()`: the union of every delta committed since the last
successful present. It includes abandoned submissions and is independent of
which ring image the next frame acquires. It is never the canvas repaint scope.

### 4. Pre-submit failures carry damage, not `Full`

A frame can fail before Flux records it: `begin_frame` timeout, direct-scanout
rejection, or a surface loss before submit. `DamageTracking.pending_damage`
carries that assessed delta into the next assessment. The next successful
submit is the transaction that commits it. `force_full_redraw` is reserved for
genuine content invalidation, not transient pacing.

### 5. Invalidation is explicit

`Surface::invalidate_damage()` is called when the framebuffer contract is
broken: surface resize/recreation, VT resume, lost flip, backend epoch change.
The next frame is full.

## Invariants

- **INV-1 — One image ledger.** No Tessera-side per-image damage vector exists.
- **INV-2 — Delta in, scope out.** The compositor supplies current damage; Flux
  returns the exact repaint scope and the scanout delta.
- **INV-3 — No full on transient.** `Busy`, `FlipTimeout`, and an acquire
  timeout never force a full repaint.
- **INV-4 — KMS hint is scanout damage.** A repaint scope is not a scanout hint.
- **INV-5 — Pre-submit damage is not lost.** It lives in `pending_damage` until
  a successful submit commits it.

## Consequences

### Positive

- A transient present rejection costs one delta, not a 5.9 MP full repaint.
- The same repair path serves SHM, dma-buf, and compositor chrome; the failure
  is no longer transport-specific.
- The KMS hint remains correct across abandoned flips and image rotation.
- The compositor no longer has a second answer to "which image is current?".

### Negative / Trade-offs

- `pending_damage` is one extra small value in `DamageTracking`.
- The compositor must supply a truthful current delta. It already computes one
  for every frame; the contract now states that it is the only input.
- The engine API grows two accessors and one invalidation call. That is the
  price of deleting a duplicated ledger, not an addition to the model.

## Rejected Alternatives & Negative Knowledge

### Keep the per-image vector and fix its edge cases

- **Why considered**: minimal diff; ADR-0225 had just stabilised the key.
- **Why rejected**: the edge cases *are* the second ledger. First-use `Full`,
  retry, and scanout hint semantics would each need a caller-side special case
  forever. The engine already owns the images.

### Force `Full` on every rejected present

- **Why considered**: conservative and simple.
- **Why rejected**: measured in the `tessera-trace2.log` sample as 23% full
  frames, with `Busy` clusters extending into six or more consecutive
  full-output repaints. The damage was already assessed; discarding it was the
  defect, not a safety property.

### Use the repaint scope as the KMS damage hint

- **Why considered**: one list, one consumer shape.
- **Why rejected**: the repaint scope is relative to one image's epoch; the KMS
  hint is relative to the frame currently on screen. They are different
  questions, and conflating them hides abandoned submissions.
