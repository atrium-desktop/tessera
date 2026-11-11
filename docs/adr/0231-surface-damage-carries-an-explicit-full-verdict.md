---
id: ADR-0231
title: "Surface damage carries an explicit full verdict"
status: accepted
date: 2026-11-10
scope: primitives/geometry, composite/presentation, wayland/scene
superseded_by: null
negative_knowledge: true
---

# ADR-0231: Surface damage carries an explicit full verdict

- Status: Accepted
- Date: 2026-11-10
- Deciders: Tessera Architecture Working Group
- Consulted: `tessera-primitives` (surface frame values), `tessera-composite`
  (client damage tracker and SHM upload), `tessera-wayland` (scene frames)
- Amends: [ADR-0229](0229-the-compositor-supplies-current-damage-flux-owns-the-image-ledger.md),
  [ADR-0230](0230-damage-is-surface-local-and-viewport-is-a-buffer-transfer-inverse.md)

---

## Context and Problem Statement

ADR-0229 and Optics ADR-0155 made `Full` explicit in Flux's frame damage API.
Tessera's surface boundary still used the historical sentinel:

```rust
damage: &[Rect]   // empty means "unknown / whole surface"
```

Two consumers interpreted that sentinel:

- `ClientDamageTracker` treated a generation change with an empty list as a
  whole-surface repaint;
- the SHM texture upload treated an empty list as a full texture refresh.

The sentinel conflated three distinct facts:

1. the client reported no damage request at all;
2. the client reported damage, but every rectangle clipped away (a visible
   no-op);
3. the whole surface genuinely changed.

The first two need different behaviour at the snapshot and upload boundaries.
ADR-0230 already introduced `pending_damage_reported` and an empty
`SnapshotRepair::Rects { surface: &[], buffer: &[] }` for the snapshot. The
exposed surface value still forced the tracker and renderer to guess.

## Decision Outcome

### 1. `damage_full` is a first-class field

`SurfacePixels` and `SurfaceDmabuf` gain:

```rust
pub damage_full: bool,
```

`SurfaceDamageFrame` gains `damage_full()`. The server fills it from
`SurfaceRec::committed_damage_full`.

The exposed contract is:

| `damage_full` | `damage` | Meaning |
|---|---|---|
| `true` | empty | the whole surface changed |
| `false` | empty | no visible pixel changed |
| `false` | non-empty | exactly these surface-local rectangles changed |

An empty list is never interpreted as `Full`.

### 2. The tracker consumes the verdict, not a sentinel

`ClientDamageTracker` maps `damage_full == true` to the placement's drawn rect.
`false` maps only the listed rectangles; an empty list contributes nothing.
Placement changes still repaint both visual footprints independently of content
damage.

### 3. The uploader distinguishes full, region, and no-op

`plan_shm_upload` answers one of:

- `Full` — upload the whole texture;
- `Region(rect)` — upload the union of the mapped damage boxes;
- `None` — no visible pixel changed.

`None` is not a fallback to full. The renderer consumes the generation without
touching the texture, so a reported-but-clipped commit is not retried forever
and does not pay a whole-texture upload.

### 4. Full is never inferred

No consumer may derive `Full` from `damage.is_empty()`. The only producers of
the full verdict are the Wayland commit path (no damage on a new buffer,
unlocalizable buffer, geometry revision, overflow) and the tracker's first
sighting / placement change, which are separate facts.

## Invariants

- `[INV-ARCH-118]` **Full is explicit at the surface boundary.**
  `SurfacePixels::damage_full` and `SurfaceDmabuf::damage_full` carry the
  whole-surface verdict; an empty `damage` list never means `Full`.
- `[INV-ARCH-119]` **A no-op generation is consumed without an upload.**
  When `damage_full == false` and no rectangle maps to a visible buffer pixel,
  the renderer advances the cache generation without uploading and without
  falling back to a full refresh.

Enforced by:

- `tessera-composite`: `explicit_full_verdict_is_not_inferred_from_an_empty_list`,
  `shm_upload_plan_distinguishes_full_empty_and_region`;
- `tessera-wayland`: `commit_sequence_keeps_the_frame_ledger_monotonic`,
  `shm_snapshot_reported_empty_damage_copies_nothing`.

## Consequences

### Positive

- A reported-but-clipped commit is a real no-op: no full repaint, no full
  texture upload, no generation retry loop.
- The tracker and uploader no longer disagree about what an empty list means.
- Diagnostics can log `damage_full` as a fact instead of deriving it.

### Negative / Trade-offs

- `SurfacePixels` and `SurfaceDmabuf` grow one bool; every constructor must set
  it. This is a deliberate ABI/source break with no compatibility shim.
- A future consumer that forgets `damage_full` fails to compile rather than
  silently treating empty as full.

## Rejected Alternatives & Negative Knowledge

### Keep the sentinel and document it

- **Why considered**: zero churn.
- **Why rejected**: it is the same overloaded-value shape that produced the
  snapshot and viewport defects. Documentation is not a compiler check.

### Add a generic `DamageSet` framework

- **Why considered**: one value type across Flux, Wayland, and composition.
- **Why rejected**: the layers have different ownership and coordinate spaces;
  a universal container would hide the exact distinctions this record exists to
  preserve. A single boolean at the surface boundary is enough.

### Treat empty + false as `Full` defensively

- **Why considered**: avoids under-repaint if a producer forgets to set the
  flag.
- **Why rejected**: it recreates the sentinel and makes a no-op pay a full
  upload. The producer is in the same crate as the field and is covered by the
  sequence property test.
