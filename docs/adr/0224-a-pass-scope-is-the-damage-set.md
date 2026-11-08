---
id: ADR-0224
title: "A Canvas pass scope is the damage set, not its bounding box — and a fence's reference is not its scope"
status: accepted
date: 2026-11-06
scope: composite/presentation, runtime, shell/runtime
superseded_by: null
negative_knowledge: true
---

# ADR-0224: A Canvas pass scope is the damage set, not its bounding box — and a fence's reference is not its scope

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-composite` (presentation damage, stream transport),
  `crates/tessera` (rendering, presentation), `crates/tessera-shell`,
  `libs/flux` (canvas pass descriptor)
- Amends: [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md),
  [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md)
- Extends: Optics ADR-0149 (damage-scoped frames on a windowed surface)

---

## Context and Problem Statement

ADR-0203 and ADR-0211 established that a compositor frame's damage is a *set* of
disjoint rectangles, and that every consumer must keep the set: the bounding box of
"a top HUD band" and "a bottom dock" is the entire output, so collapsing early
throws away the localisation the whole pipeline exists to produce.

Optics ADR-0149 then replaced flux's single-rectangle `render_area` with a
`flux_damage` list in `flux_canvas_pass_desc`. Tessera was half-migrated: the
descriptor field had been renamed in the bindings, but Tessera's own scope
producer still computed one box.

```rust
pub fn frame_damage_render_area(
    repaint: &FrameDamage,
    physical_size: (u32, u32),
) -> Option<flux::CanvasRenderArea> {
    clamp_render_area(repaint.area_union()?, physical_size.0, physical_size.1)
}
```

`area_union()` is exactly the collapse ADR-0203 forbids. Two consequences, both
real and both measured:

1. **The work was lost at the last step.** Damage was carried as a list through
   the tracker, the ring-slot history and the chrome footprint measurement, and
   then reduced to one rectangle immediately before the GPU pass — so every
   region between two damaged bands was scissored, cleared and encoded as part of
   the box.
2. **The workspace did not build.** The bindings no longer had `render_area`, and
   `tessera-composite`, `tessera` and the `spikes` transport still referenced it.
   A half-finished contract migration had left `cargo build --workspace` failing,
   which blocks every other verification in the repository.

One further detail made the migration non-mechanical. `FrameDamage::None` and
`FrameDamage::Full` both mean "the whole destination", and flux expresses that as
an *empty* scope (`count == 0`), not as a full-extent rectangle. So the producer's
return type had to change meaning, not just shape: `None` used to mean "no area,
draw everything" and now an empty `Vec` does. Any call site reading the old
`Option` as "was there damage" would silently invert.

---

## Decision Drivers

- **A set must survive to the consumer or it was never a set.** Collapsing at the
  boundary is indistinguishable, from the GPU's side, from never having localized.
- **One representation per meaning.** `Full`/`None` → empty scope is flux's
  contract; Tessera must express it that way rather than invent a parallel
  convention.
- **A rejected scope is not a no-op.** Both flux backends reject a scope naming no
  pixel of the target, and a rejected pass costs the whole frame's chrome — so
  clamping has to live on this side of the fence, per rectangle.
- **A workspace that does not build cannot be verified.** Contract migrations are
  atomic or they are not done.

---

## Decision Outcome

### 1. The producer returns the clamped list

`frame_damage_render_area` is deleted; there is no wrapper and no deprecated
alias. Its replacement is `frame_damage_scope`:

```rust
pub fn frame_damage_scope(
    repaint: &FrameDamage,
    physical_size: (u32, u32),
) -> Vec<flux::CanvasRenderArea> {
    let Some(rects) = repaint.area_rects() else { return Vec::new(); };
    let mut scope: Vec<_> = rects
        .iter()
        .filter_map(|rect| clamp_render_area(*rect, physical_size.0, physical_size.1))
        .map(/* Rect -> CanvasRenderArea */)
        .collect();
    if scope.is_empty() && !rects.is_empty() {
        scope.push(full_extent(physical_size));
    }
    scope
}
```

Three properties, each load-bearing:

- **`Full`/`None` → empty `Vec`.** Matches flux's own reading, so there is no
  second convention to keep in sync.
- **Each rectangle clamped independently.** An off-screen sibling is dropped
  without discarding a legitimate one.
- **All rectangles dropped → one full-extent rectangle, not an empty scope.**
  Otherwise "every region was off-screen" would be indistinguishable from `Full`
  arriving by accident, and the pass would silently be given an empty scope where
  it needed to cover the destination.

### 2. Every pass inherits the list

`begin_stencil_frame_overlay`, `begin_opaque_frame_repaint` and the desktop
output pass all take `Vec<flux::CanvasRenderArea>` and forward it verbatim. The
chrome overlay pass inherits the *base pass's* scope, because it draws into the
same damaged regions.

### 3. The chrome-intersection test asks the list

`chrome_intersects_render_area(area, …)` was asked about one rectangle. With a
set, the pass is worth beginning when **any** damaged rectangle can contain
chrome; an empty scope means the whole destination, so the answer is then
unconditionally yes.

### 4. `sub_rects` is not a scope

`tessera-composite`'s stream transport described its readback target with
`render_area: None`. That `None` never meant "full destination" — it meant "this
pass is not damage-scoped at all". The stream's own field is `sub_rects`:
the region it reads back. Mapping it to `RenderArea` conflated two unrelated
concepts, and the migration would have carried the conflation forward. It is now
`damage: Vec::new()` with the intent stated at the field.

### 5. The shell consumes the diagnostic, not the code

Separately, the log line that motivated the previous record was

```
WARN shell: chrome scene build failed: shell render: Render(FLUX_ERROR_INVALID_ARGUMENT)
```

`FLUX_ERROR_INVALID_ARGUMENT` cannot distinguish a non-finite rect from an
out-of-bounds scope from a double-begun pass, so no reader could act on it. Flux
records which entry point failed, at which source line, and why, in a
thread-local slot — and discards it unless the caller reads it *at the failure
site*. The lens binding now captures that at construction into an `ErrorInfo` that
travels with the error (Optics ADR-0153), so this log line carries the entry point
and message without any change at the call site.

---

## Consequences

### Positive

- **Localization survives to the GPU.** A frame with two damaged bands is a pass
  with two scissor areas, and flux encodes each one separately (its
  `canvas_pass_areas` path exists precisely so the between-band pixels are never
  written from an unseeded intermediate).
- **The workspace builds and tests again**: `cargo build --workspace` and
  `cargo test --workspace` are green, which restores the repository's own gates.
- **Off-screen damage can no longer reject a pass.** Clamping is per rectangle and
  the empty case has a defined answer, so the
  `Render(FLUX_ERROR_INVALID_ARGUMENT)` → chrome-less-frame path loses one of its
  triggers.
- **Concepts stopped sharing a field.** "Narrower scope" and "not scoped" are now
  distinguishable in the type.

### Negative / Trade-offs

- **`Vec` allocation per pass.** One small allocation per pass per frame. The
  alternative — a fixed-capacity array threaded through four signatures — would
  put a capacity constant in the public shape of three crates to save a
  sub-microsecond allocation on a path that already builds several vectors per
  frame. Revisit if profiling says otherwise; the type is the right one either way.
- **`clone()` where a pass is built twice.** The desktop output pass and the chrome
  overlay both need the same scope. Cloning a handful of `Copy` rects is cheaper
  than restructuring the borrowing, and the alternative (passing `&[...]` into
  `CanvasPassOptions`, which owns its scope) would push a lifetime into the
  bindings for no benefit.
- **`frame_damage_scope` has no `Option` in its return.** A caller that genuinely
  wanted "was anything damaged" must now ask `FrameDamage` directly. That is
  correct — the question was never the pass scope's to answer — but it is a call
  site that had to be found and moved.

---

## Rejected Alternatives & Negative Knowledge

### 1. Keep the single-rect producer and let flux handle the box

- **Why considered**: zero call-site churn; flux already accepts a scope and would
  clamp it.
- **Why rejected**: it is the collapse ADR-0203 exists to prevent, moved one layer
  down. Flux cannot recover regions the caller already merged, and "the compositor
  draws the union" is precisely the whole-output repaint ADR-0211 deleted. It would
  also have left the descriptor half-migrated, which is what broke the build.

### 2. Return `Option<Vec<CanvasRenderArea>>` to keep the `None` = "full" reading

- **Why considered**: preserves the old call sites' semantics with a mechanical
  type change, so review could be about the shape rather than the meaning.
- **Why rejected**: it invents a second convention for a meaning flux already has,
  and the two would drift — a `Some(vec![])` and a `None` would both have to be
  interpreted, in every caller. An empty list *is* the whole destination in flux;
  encoding that once, in the type, removes the ambiguity rather than relocating it.

### 3. Clamp the scope inside flux instead of in Tessera

- **Why considered**: one place, and every caller would benefit.
- **Why rejected**: flux *does* validate (it rejects a scope naming no pixel, which
  is how the failure is detected at all), but clamping on flux's side would mean
  guessing what the caller meant by an off-screen rect — cover the destination, or
  skip the pass? Tessera knows which it wants (cover, because a pass must draw
  something). A library silently widening a caller's scope would also hide the
  bug that produced an out-of-range rect.

### 4. Keep `render_area` in the stream transport and translate it

- **Why considered**: least disruption to the transport struct, which is shared
  with the capture path.
- **Why rejected**: translating it labelled the readback region as a damage scope,
  so a later reader would reasonably conclude the stream is damage-scoped — and
  then wire the two together. The fields are unrelated; the migration is the
  moment to say so.

### 5. Leave the workspace half-migrated and fix only the call sites that blocked

- **Why considered**: the runbook for this session was the two behavioural
  defects, not a binding migration.
- **Why rejected**: `cargo build --workspace` failing means no test, no clippy, no
  ADR check and no behavioural measurement in the repository can be trusted. A
  contract migration that stops halfway is worse than one not started, because the
  tree looks like it has the new contract while half of it still assumes the old.

---

## Resolution of Degenerate Geometry Sources

The reachable sources of `FLUX_ERROR_INVALID_ARGUMENT` during chrome scene builds
have been structurally eliminated across Optics and Tessera Shell:

1. **Lens Layout and Opacity Sanitization (Optics)**:
   - `lens_set_opacity`: Added `!isfinite(opacity)` checks. In IEEE-754 floating point,
     `NaN < 0.0f` and `NaN > 1.0f` both evaluate to false, allowing `NaN` to poison
     `ui->opacity` and cascade into invalid brush opacities. Non-finite opacity is now
     clamped to `0.0f`.
   - `lensi_apply_box`: Negative or non-finite `box.width`, `box.height`, and `box.flex`
     are clamped to `0.0f` before reaching solved node bounds.
   - `lensi_drawlist_push`: Dropping non-finite command geometries (`x`, `y`, `w`, `h`,
     `radius`, `width`, `text_size`) and clamping negative dimensions (`w < 0.0`, `h < 0.0`,
     `radius < 0.0`, `width < 0.0`) on non-text commands before command recording.
   - `LayoutOpts::to_raw` (`lens-rs`): Enforced finiteness and non-negativity across all
     dimensions, padding, gaps, and corner radii.
2. **Dock Morphing and Geometry Sanitization (Tessera Shell)**:
   - `collapsing_radius`: Clamped `surface_progress` to `[0.0, 1.0]` and clamped the final
     radius to `(height * 0.5).max(0.0).max(0.0)` so negative panel heights during spring
     oscillations cannot produce negative corner radii.
   - Running indicator dots and divider hairlines: Clamped `dot_long`, `dot_thick`, `strip_span`,
     and `divider_len` to `.max(0.0)` so collapsed autohide states cannot emit negative-extent
     rectangles.
   - Bento reading rows: Clamped `radio_glyph_rect` and `battery_parts` available bounds and
     glyph sizes to `.max(0.0)` with a smooth scale-down floor below 10 px.
3. **Flux Encoder Fail-Fast Diagnostics**:
   - `flux_encoder_draw_geometry`, `flux_encoder_clip_rect`, `flux_encoder_save_layer`,
     and `encoder_vec2` now record detailed thread-local diagnostics (`FLUX_FAILF`) detailing
     the exact failing geometry kind, coordinate values, and invalid parameters upon failure.

---

## Links

- Amends [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md): its "keep the
  set" rule now holds all the way to the pass descriptor, not just to the tracker.
- Amends [ADR-0211](0211-regional-damage-is-consumed-as-a-list-and-chrome-footprints-are-measured.md):
  the chrome overlay consumes the same list the base pass does.
- Extends Optics ADR-0149: this is the consuming side of that contract, completed.
- Related Optics ADR-0153: the lens binding carries flux's diagnostic, which is
  what makes the shell's failure log actionable.
