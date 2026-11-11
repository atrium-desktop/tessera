---
id: ADR-0230
title: "Damage is surface-local; viewport is a buffer-transfer inverse"
status: accepted
date: 2026-11-09
scope: wayland/damage, composite/presentation, primitives/geometry
superseded_by: null
negative_knowledge: true
---

# ADR-0230: Damage is surface-local; viewport is a buffer-transfer inverse

- Status: Accepted
- Date: 2026-11-09
- Deciders: Tessera Architecture Working Group
- Consulted: `tessera-wayland` (commit/snapshot), `tessera-primitives`
  (geometry), `tessera-composite` (damage assessment and SHM upload),
  `tessera-desktop` (logical extent)
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md),
  [ADR-0228](0228-a-commit-repairs-its-own-buffer-not-the-frame-s-damage-union.md),
  [ADR-0229](0229-the-compositor-supplies-current-damage-flux-owns-the-image-ledger.md)
- Depends on: Optics ADR-0155 (frame damage is a native epoch ledger)

---

## Context and Problem Statement

ADR-0203 made damage a placement-mapped rectangle set. That was the right
ownership boundary, but it left two coordinate-space defects in the SHM path.

1. **The damage rectangle space was modelled as pre-viewport.** `SurfacePlacement`
   stored the full post-scale buffer extent (`local`), the viewport source
   origin, and the destination extent, then mapped damage as
   `origin + (p - source_origin) * dest / source`. The server, however, exposes
   `wl_surface.damage` in surface-local coordinates (the post-viewport extent),
   and `wl_surface.damage_buffer` is converted to that same space at commit.
   Applying the viewport scale again shrank and displaced every fractional-scale
   client's repaint region. A 2× viewport destination repainted a quarter of the
   changed area; a source crop could drop the left/top strip entirely.

2. **The SHM snapshot and texture upload inverted the pipeline with a bare
   scale.** `logical_to_buffer_scale` returned `buffer / destination` or
   `buffer_scale`. It ignored `viewport_src.origin` and used the full buffer
   rather than the source extent. A cropped surface copied the wrong buffer
   rows into the snapshot and uploaded the wrong texture region. Over-covering
   a per-commit snapshot repair is not safe: pixels outside that commit's
   damage still belong to an older frame in an alternating buffer, so a wrong
   inverse can reintroduce the rollback ADR-0228 removed.

The two defects are independent of the frame ledger. ADR-0228 fixed *which*
damage a snapshot repair may consume; this record fixes *which coordinate
space* every damage rectangle lives in and how it crosses the surface/buffer
boundary.

## Decision Outcome

### 1. One damage space: the stable surface-local extent

Every damage list the compositor consumes is in the surface-local logical
extent after `wp_viewport` has selected the surface's size:

- `wl_surface.damage` is already in that space;
- `wl_surface.damage_buffer` is converted to it at commit;
- `SurfacePixels::damage` and `SurfaceDmabuf::damage` expose that space.

`SurfaceGeometry` names the pieces explicitly:

- `local_extent()`: post-transform buffer extent ÷ `buffer_scale`, before the
  viewport crop;
- `surface_source()`: `viewport_src`, else the full local extent;
- `surface_extent()`: `viewport_dst`, else the source size.

A damage rectangle never carries a viewport offset. The source origin belongs
only to the surface → buffer conversion.

### 2. The geometry owns both directions of the conversion

`SurfaceGeometry::buffer_rect_to_logical` and
`SurfaceGeometry::surface_rect_to_buffer` are exact inverses over the complete
renderer pipeline:

```text
raw buffer px
  -> buffer transform
  -> buffer_scale
  -> viewport source crop + destination stretch
  -> surface-local logical px
```

Both directions are rectangle-aware. The inverse applies the source origin,
the source extent, and `buffer_scale` before inverting the 8-way transform.
No caller re-derives a scale factor. The SHM snapshot and the texture upload
both consume `surface_rect_to_buffer`.

### 3. `SurfacePlacement` maps damage 1:1, with only transitions scaling it

`SurfacePlacement` retains the source rectangle as placement identity, because
a crop change alters the buffer pixels even when the destination extent does
not. But `place()` maps damage as:

```text
output = origin + surface_rect * (dest_extent / surface_extent)
```

`surface_extent` is the stable surface size, so a viewport destination or
source crop contributes no second scale. Only an in-flight transition replaces
`dest_extent` and legitimately scales the same content. `place_damage(&[], true)`
answers the whole drawn rect; `place_damage(&[], false)` answers nothing.

### 4. Snapshot repair is an explicit two-state contract

`SnapshotRepair` distinguishes:

- `Full`: the commit could not describe itself; copy the whole buffer;
- `Rects { surface, buffer }`: copy exactly this commit's rectangles.
  `surface` is mapped through the viewport; `buffer` is raw and therefore also
  reaches pixels hidden by a source crop. Both empty means the commit is a
  visible no-op: copy nothing.

`SurfaceRec` records whether a damage request was issued, so a buffer attach
with no damage request is full while a reported-but-clipped commit is a no-op.
This preserves ADR-0228's monotonic snapshot and avoids turning a visible
no-op into a whole-buffer rollback.

### 5. A coordinate-space change latches the frame interval to full

`committed_damage` accumulates across several commits before one present. If a
later commit changes the viewport, transform, or buffer scale, the earlier
rectangles were expressed under a different sampling map and cannot be
reinterpreted through the new geometry. A buffer extent change that moves the
surface extent has the same problem. The commit that changes the map clears the
accumulated list, sets `committed_damage_full`, and — when it attaches a new
buffer — copies that whole buffer into the SHM snapshot. The latest buffer is
the authority for the new map; the next present acknowledges the interval. A
buffer resample behind a fixed `wp_viewport.set_destination` with an unchanged
map leaves the surface extent — and therefore the accumulated damage — valid.

### 6. Raw buffer damage survives the viewport crop

A `wp_viewport.set_source` rectangle smaller than the raw buffer hides raw
pixels. Those pixels are not addressable in surface-local damage coordinates,
but `wl_surface.damage_buffer` can address them. The SHM snapshot therefore
keeps **two** per-commit damage lists:

- `surface` damage, mapped through the viewport like every surface-local
  rectangle;
- raw `buffer` damage, copied directly into the raw snapshot.

The frame-level damage list still contains only the visible surface
intersection of buffer damage, because that is what the compositor repaints.
The raw list is snapshot-repair-only. This keeps hidden pixels updateable
without a whole-buffer copy and without re-introducing the alternating-buffer
rollback: a later commit never overwrites pixels it did not declare.

A client that changes hidden pixels is responsible for declaring them with
`wl_surface.damage_buffer`, exactly as it is responsible for declaring any
other change. A surface-local `wl_surface.damage` rectangle cannot address
hidden pixels and is not guessed into them.

### 7. Overflow covers the whole input

`normalize_damage_rects` decomposes damage into pairwise-disjoint rectangles.
If the decomposition exceeds its work budget, it answers the bounding box of
the **entire input**, never of the processed prefix. Dropping unvisited input
rectangles is an under-cover, which is the one direction damage may not fail.

## Invariants

- `[INV-ARCH-113]` **Damage space is surface-local.** `SurfacePixels::damage`
  and `SurfaceDmabuf::damage` contain post-viewport surface-local rectangles.
  No consumer applies viewport scaling to them again.
- `[INV-ARCH-114]` **Geometry conversions are inverses.** For every buffer
  transform, buffer scale, and viewport state, a raw buffer rect mapped to
  surface space and back covers the original rect exactly.
- `[INV-ARCH-115]` **Snapshot repair is per-commit and monotonic.** A commit's
  snapshot copy consumes only that commit's damage; an empty reported set
  copies nothing; only an explicit `Full` copies the whole buffer.
- `[INV-ARCH-116]` **Damage overflow covers all input.** A normalized damage
  set that cannot remain a bounded disjoint list answers a bounding box that
  covers every input rectangle.
- `[INV-ARCH-117]` **A coordinate-space change invalidates accumulated damage.**
  A commit that changes viewport, transform, buffer scale, or the surface
  extent latches the frame damage to full and, when it attaches a buffer,
  repairs the SHM snapshot from that whole buffer; earlier rects are never
  reinterpreted through the new geometry.
- `[INV-ARCH-120]` **Raw buffer damage survives the crop.**
  `wl_surface.damage_buffer` repairs the raw SHM snapshot even when it lands
  outside the visible `wp_viewport.set_source`; frame damage remains the
  visible surface intersection. Snapshot repair never widens to a whole-buffer
  copy merely because a crop is active.

Enforced by:

- `tessera-primitives`: fixed cases
  `surface_rect_to_buffer_*` / `buffer_and_surface_damage_conversions_*` /
  `surface_rect_to_buffer_inverts_the_wayland_transforms`, plus property tests
  `every_surface_damage_rect_round_trips_through_the_buffer` and
  `every_visible_buffer_damage_rect_round_trips_through_the_surface` over the
  valid protocol domain (buffer extent = surface extent × buffer_scale, source
  crop within the buffer, all 8 transforms);
- `tessera-composite`: `test_inv_arch_92_*`, `test_inv_arch_93_*`,
  `viewport_destination_damage_is_not_scaled_twice`,
  `damage_normalization_overflow_covers_every_input_rect`, plus property tests
  `placement_maps_full_surface_damage_to_exactly_the_drawn_rect` and
  `placement_lands_every_surface_damage_inside_the_drawn_rect`;
- `tessera-wayland`: `snapshot_copy_never_reapplies_another_commits_damage`,
  `shm_snapshot_repairs_only_the_committing_buffer`,
  `shm_snapshot_maps_viewport_crop_origin_into_the_buffer`,
  `shm_snapshot_copies_hidden_raw_buffer_damage`,
  `shm_snapshot_reported_empty_damage_copies_nothing`, and the property test
  `shm_snapshot_copies_exactly_the_mapped_damage`.

## Consequences

### Positive

- Fractional-scale clients repaint the exact region they damaged; the texture
  upload and the compositor repair move in lockstep.
- Viewport source crops no longer copy stale rows into the SHM snapshot or
  leave the visible far edge unrefreshed.
- The snapshot and the upload share one geometry authority instead of two
  scale-factor approximations.
- A damage-overflow can no longer silently strand the unvisited tail.

### Negative / Trade-offs

- `SurfacePlacement` no longer carries the viewport origin as a damage-map
  input; callers that want the sampled source read `source`/`surface_source`.
- The geometry conversion is rect-aware and uses `f64`; it is slightly more
  code than a scale pair, but the scale pair was wrong for every crop.
- `SurfaceRec` grows one boolean to distinguish "no damage reported" from
  "reported but clipped away".

## Rejected Alternatives & Negative Knowledge

### Keep the pre-viewport damage space and convert at the server

- **Why considered**: preserves ADR-0203's original formula.
- **Why rejected**: the protocol defines `wl_surface.damage` in surface-local
  coordinates; converting it back to a pre-viewport space would have to undo
  the viewport for every commit and would still leave `damage_buffer` and
  `damage` with different origins.

### Keep a scale-only inverse and document over-cover as safe

- **Why considered**: simpler, and over-cover is safe for texture uploads from
  the current snapshot.
- **Why rejected**: it is not safe for per-commit snapshot repair. Pixels
  outside the commit's damage live in an older frame on the alternating
  buffer; copying them rolls the surface backwards. A crop origin also makes
  the scale-only map under-cover the far edge.

### Treat an empty reported damage set as `Full`

- **Why considered**: avoids a second state.
- **Why rejected**: a commit that reported damage entirely outside the visible
  surface changed no visible pixel. Copying the whole buffer would overwrite
  pixels contributed by an earlier commit in the same frame interval.
