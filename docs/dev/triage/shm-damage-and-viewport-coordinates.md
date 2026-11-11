# SHM Damage and Viewport Coordinate Triage

Diagnostic guide for the symptom class where an SHM-backed client updates
incorrectly: text or a caret rolls back, only part of a fractional-scale window
refreshes, cropped surfaces leave stale bands, or the whole scene flickers. The
contract is [ADR-0230](../../adr/0230-damage-is-surface-local-and-viewport-is-a-buffer-transfer-inverse.md),
built on [ADR-0228](../../adr/0228-a-commit-repairs-its-own-buffer-not-the-frame-s-damage-union.md)
and [ADR-0229](../../adr/0229-the-compositor-supplies-current-damage-flux-owns-the-image-ledger.md).

The short version: **damage has a coordinate space and a role.** Confusing
either produces a visual artifact that looks like a client bug but is a
compositor invariant violation.

---

## 1. Symptom Classifier

| Symptom | First suspect | Invariant |
|---|---|---|
| Typed text/caret returns to an earlier position, then the next commit advances it; dragging the window "fixes" it | Frame-aggregate damage applied to a later commit's SHM buffer | `[INV-ARCH-115]` |
| A 1.5×/2× fractional-scale window repaints only the top-left fraction of the changed region | Viewport destination applied as a second output scale | `[INV-ARCH-113]` |
| A `wp_viewport.set_source` client leaves the right/bottom strip stale, or a rotated buffer shows shifted rows | Scale-only surface→buffer conversion; crop origin/transform dropped | `[INV-ARCH-114]` |
| Changing viewport/transform/buffer scale leaves stale bands for one frame | Accumulated damage reinterpreted through new geometry | `[INV-ARCH-117]` |
| Whole-output strip tearing/displaced blocks, independent of SHM | Overlapping `VkImageCopy` or repeated output passes in flux | Optics ADR-0154/0155, not this page |
| A damage set that should name a few cells repaints the output | Damage overflow dropped unvisited input | `[INV-ARCH-116]` |

---

## 2. The Two Independent Axes

### Axis A: what the damage describes

- **Per-commit damage** (`pending_damage` / `SnapshotRepair::Rects`): what to
  copy from **this commit's buffer** into the retained SHM snapshot.
- **Frame damage** (`committed_damage`): what the frame must assess, repaint,
  upload, and (via Flux) hint to KMS.

A synthetic frame can contain several commits with different `wl_buffer`s. The
frame list is a union across buffers; the snapshot repair may only take the
current commit's rectangles. Using the frame list for repair rolls the snapshot
backwards.

An empty per-commit list is **not** "copy everything." It means the commit
reported damage that clipped away: copy nothing. Only an explicit `Full` (a new
buffer with no damage request, or an unlocalizable buffer) copies the whole
buffer.

The exposed surface boundary uses the same rule: `SurfacePixels::damage_full`
and `SurfaceDmabuf::damage_full` are the whole-surface verdict. Empty rectangles
with `damage_full == false` are a visible no-op; the tracker contributes no
output damage and the SHM uploader consumes the generation without touching the
texture (ADR-0231).

### Axis B: what coordinate space the rectangle lives in

The only damage space exposed by `SurfacePixels`/`SurfaceDmabuf` is the stable
**surface-local logical extent after `wp_viewport`**:

```text
surface extent = viewport destination
              or viewport source size
              or post-transform buffer extent / buffer_scale
```

Output placement is then:

```text
output = surface origin + damage rect * (transition size / surface extent)
```

There is no viewport term in that expression. The viewport source origin and
extent belong to the surface→buffer conversion:

```text
raw buffer px
  -> buffer transform
  -> buffer_scale
  -> viewport source crop + destination stretch
  -> surface-local logical px
```

`SurfaceGeometry::buffer_rect_to_logical` and `surface_rect_to_buffer` are the
only sanctioned implementations of those two directions. Do not re-derive a
scale factor at a call site.

---

## 3. Fast Diagnostic Recipe

### 3.1 Turn on the damage traces

```bash
RUST_LOG="info,tessera.damage=trace" TESSERA_LOG_FORMAT=json \
  cargo run -p tessera -- <usual flags>
```

The structured schema is documented in
[Observability](../observability.md#damage-diagnostics). Read the events in
this order:

1. **`tessera.damage.commit` / `commit.decision`** — `frame_damage` is the
   visible per-commit union, `surface_damage` is the `wl_surface.damage` list,
   and `buffer_damage` is the raw `wl_surface.damage_buffer` list. If
   `frame_damage` is smaller than the client's visible report, the producer lost
   it before the snapshot. `damage_space_changed` must be false for ordinary
   content commits.
2. **`unknown_full`** — must be true only for a new buffer with no damage
   request, an unlocalizable buffer, or a geometry revision that attached a new
   buffer. `true` on every commit means the `pending_damage_reported` path is
   broken.
3. **`tessera.damage.shm` / `shm.snapshot`** — `reason` says why the repair was
   full (`unknown_full`, `snapshot_mismatch`) or precise (`incremental`,
   `reported_empty`). `surface_commit_rects` are mapped through the viewport;
   `buffer_commit_rects` are raw and may be outside the visible crop.
   `mapped` is the raw buffer box actually written for the surface list.
   Compare `surface_extent`, `viewport_src`, `viewport_dst`, `buffer_scale`,
   and `transform` before judging the mapped box.
4. **`tessera.damage.upload` / `shm.upload`** — `mode=none` means
   `damage_full=false` and no visible pixel changed; the generation must still
   be consumed. `mode=partial` with a box outside the expected buffer region is
   the scale/origin bug. For a 2× viewport destination, a full 1200×800 surface
   must map to a 2400×1600 buffer box, not 600×400.
5. **`tessera.damage.assess`** — aggregate `rects` and `area` should match the
   per-surface placements; `damage_full=true` on a surface with an empty
   `damage_rects` is the explicit-full path, not a sentinel.

### 3.2 Reproduce with the smallest shapes

- Run the fractional-scale probe:
  `cargo test -p tessera-wayland --lib fractional_scale_popup_commits_are_copied_in_full -- --ignored --nocapture`
- For a crop, reduce the client to one surface with
  `viewport_src=(100,50,400,300)`, no destination, and a full-surface
  `wl_surface.damage`. The snapshot must contain raw rows 50..350 and columns
  100..500, with no invented pixels outside.
- For rollback, commit buffer A with damage A, then buffer B with damage B
  before one present. The snapshot must be `A` content at A's rect and `B`
  content at B's rect, never B at A's rect.

### 3.3 Rule out flux/optics

If the snapshot pixels are correct and the texture upload box is correct, but
the presented image still tears or shows displaced blocks, stop looking at SHM.
Check the flux output-copy path: overlapping `VkImageCopy` regions and
per-rectangle render passes are Optics ADR-0154 territory.

---

## 4. Attribution

A conforming client on a compositor that implements ADR-0230 must not exhibit
these artifacts. Do not file the bug against the client because it:

- alternates two same-size SHM buffers;
- uses `wp_viewport.set_destination` for fractional scale;
- uses `wp_viewport.set_source` to crop a larger buffer;
- uses `wl_surface.damage` in surface coordinates or
  `wl_surface.damage_buffer` in buffer coordinates.

All four are protocol-shaped inputs the compositor must normalize. The bug
belongs in `tessera-wayland` if the snapshot bytes are wrong, and in
`tessera-composite` if the snapshot is right but the texture upload or repaint
box is wrong.

---

## 5. Regression Set

Keep these green before touching any damage code:

- `crates/tessera-primitives`: `surface_rect_to_buffer_*`,
  `buffer_and_surface_damage_conversions_round_trip_under_viewport`,
  `surface_rect_to_buffer_inverts_the_wayland_transforms`, plus the property
  tests `every_surface_damage_rect_round_trips_through_the_buffer` and
  `every_visible_buffer_damage_rect_round_trips_through_the_surface`.
- `crates/tessera-composite`: `test_inv_arch_92_*`, `test_inv_arch_93_*`,
  `viewport_destination_damage_is_not_scaled_twice`,
  `damage_normalization_overflow_covers_every_input_rect`,
  `explicit_full_verdict_is_not_inferred_from_an_empty_list`,
  `shm_upload_plan_distinguishes_full_empty_and_region`, plus the placement
  property tests.
- `crates/tessera-wayland`:
  `snapshot_copy_never_reapplies_another_commits_damage`,
  `shm_snapshot_repairs_only_the_committing_buffer`,
  `shm_snapshot_maps_viewport_crop_origin_into_the_buffer`,
  `shm_snapshot_reported_empty_damage_copies_nothing`,
  `shm_snapshot_copies_exactly_the_mapped_damage`,
  `commit_sequence_keeps_the_frame_ledger_monotonic`, plus the ignored
  fractional-scale probe.

---

## 6. Negative Knowledge

- **Never pass the frame damage union to snapshot repair.** A later buffer's
  undamaged pixels are not guaranteed to equal the surface the earlier commit
  produced.
- **Never treat an empty reported damage set as `Full`.** Track whether the
  client reported damage separately from whether any rectangle survived
  clipping, and carry the explicit `damage_full` verdict at the surface
  boundary.
- **Never retry a no-op generation.** When `damage_full=false` and no rect
  maps to a visible buffer pixel, advance the texture cache generation without
  uploading; a fallback to a full upload makes a no-op permanently expensive.
- **Never discard raw buffer damage behind a crop.** A partial `viewport_src`
  hides raw pixels that `wl_surface.damage_buffer` can still address. Map that
  list directly into the snapshot; do not widen the whole buffer and do not let
  the visible-surface clip erase it.
- **Never use a scale-only surface→buffer map.** A viewport source rectangle has
  an origin; a crop also changes the source extent. Both must participate.
- **Never apply viewport scaling to output damage.** Damage is already in the
  post-viewport surface space; only an in-flight transition scales it.
- **Never reinterpret accumulated damage after a viewport/transform/scale
  revision.** Latch the interval to full and, for a new SHM buffer, repair the
  snapshot from that whole buffer.
- **Never let an overflow fallback drop unvisited input.** The fallback must be
  the bounding box of the entire input set.
- **Do not put viewport math in two places.** If a new caller needs
  surface↔buffer conversion, add a method to `SurfaceGeometry`, not a local
  scale factor.

## 7. New-Consumer Checklist

Before adding another damage consumer:

1. Which space is the rect in — raw buffer, surface-local, or output?
2. Which role is it — per-commit snapshot repair, frame delta, repaint scope, or
   KMS scanout hint?
3. Is the transition between spaces an exact inverse pair, or a scale
   approximation?
4. What happens when damage is empty, clipped away, over capacity, or arrives
   after a geometry revision?
5. Which test asserts the exact landing box, not just the extent?
