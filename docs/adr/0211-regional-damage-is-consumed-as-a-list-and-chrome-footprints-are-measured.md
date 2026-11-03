---
id: ADR-0211
title: "Regional Damage Is Consumed as a List, and Chrome Footprints Are Measured, Not Guessed"
status: accepted
date: 2026-11-03
scope: rendering, composite, shell, optics-interop, architecture
superseded_by: null
negative_knowledge: true
---

# 0211. Regional Damage Is Consumed as a List, and Chrome Footprints Are Measured, Not Guessed

- Status: Accepted
- Date: 2026-11-03
- Deciders: Tessera Maintainers
- Amends: [ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md), [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md), [ADR-0210](0210-performance-core-affinity-allocation-zero-copy-strided-uploads-and-quiescent-lens-lifecycle.md)
- Depends on: Optics ADR-0143 (regional frame damage as a rect list and paint-bounds localization)

---

## Context and Problem Statement

ADR-0210 introduced regional composition: `FrameDamage::Area(Vec<Rect>)` carries disjoint rectangles, `frame_damage_render_area` derives the Vulkan `renderArea`, and the KMS `FB_DAMAGE_CLIPS` hint receives the list. That machinery is inert if the *producer* of chrome damage cannot describe a set, and after ADR-0210 the shell had four independent places where it invented a footprint instead of measuring one.

1. **Lens answered with one rectangle.** `lens_damage_rect` unioned the root, every placed band, the caret, the tooltip, and every ghost into a single box, and additionally accumulated damage over each changed node's `final_rect` — the *arranged* box. A label in a stretch column is arranged across the column, and the root column spans the display, so a one-cell text edit reported `{0, 0, 1920, 1080}`.

2. **The shell then second-guessed that rectangle.** `anim_damage_regions` tested the footprint for `covers_full_display` and, when true and no immersive surface was active, *replaced* it with the top and bottom decoration bands (`INV-ARCH-98`). Because the query reported the full display for any base-tree change, this substitution fired on legitimate full-output changes: the middle of the screen was excluded from the render area and never repainted. A reporting defect had been converted into a correctness defect.

3. **The status strip's band arithmetic was wrong by 8 px in three places.** HUD chips are placed at `CHIP_TOP` (8) with height `HUD_HEIGHT` (32), so they occupy `[8, 40)`. The chip's own `damage_region` returned `[0, 32)`, the clock-rollover tick used `reserved.top.max(32)` — and since the HUD never overrode `Chrome::reserved`, `reserved.top` is `0` — and the overlay-elision test used `.max(36)`. All three left the chips' bottom 8 px stale on every tick.

4. **The dock's context menu contributed no damage and no rebuild signal.** `Dock::damage_region` unioned the capture footprint, hover rect, live-preview panel, and tooltip band, but never `app_menu`. `Dock::anim_pending` consulted the menu only to gate idle timers. A menu anchored *above* the strip landed outside `output_render_area`, so the overlay pass clipped it away; it appeared only on frames where an unrelated backdrop refresh widened the damage through `effect_damage`. That is the reported "sometimes renders, sometimes flickers".

5. **The overlay-elision predicate was y-only and incomplete.** `chrome_intersects_render_area` compared `area.y` against a top threshold and a bottom threshold derived from `reserved`. A dock on the left or right edge reserves `left`/`right`, so `reserved.bottom` was `0`, both thresholds collapsed, and the predicate answered "no chrome here" for any area touching a side-docked bar — clipping the dock. It also carried dead code (`let area_y0 = area.x; let _ = area_y0;`).

6. **Two snapshot setters dirtied unconditionally.** `set_settings`, `set_resource_stats`, and `set_interaction_domains` set `dirty = true` on every call, and the host republishes all three every frame from the status poller. The quiescent-elision that ADR-0210 introduced (`INV-ARCH-103`) was therefore defeated for the components consuming them.

## Decision Drivers

1. **A consumer must not re-derive a producer's answer.** Damage geometry is Lens's to compute; the shell's job is to forward it. Every heuristic that "fixes up" a reported region is a second, divergent source of truth.
2. **`[INV-ARCH-98]` was expressing a real need through the wrong mechanism.** The intent — a chrome tick must not poison the client workspace — is satisfied by accurate producer-side localization, which makes the substitution unnecessary rather than merely harmful.
3. **Painted extent, not arranged extent.** A region that is not painted must not be repainted; a region that is painted may not be omitted. Only the paint geometry satisfies both.
4. **Structural chrome changes need their own signal.** `anim_pending` drives the frame *clock*; it is not a change notification. A popup that opens without animating needs a one-shot rebuild-and-repaint report.
5. **Damage bounds are constants, and constants belong in one place.** Three call sites independently hardcoded the status band; one named constant cannot drift.

## Architectural Decisions

### 1. Consume the Lens damage list verbatim

`Shell::chrome_damage_regions` replaces `anim_damage_regions`. It forwards Lens's `damage_rects` list into the frame's damage set unchanged, and the `covers_full_display` band substitution is **deleted**.

The substitution cannot be reintroduced safely: with the producer reporting paint bounds (Optics ADR-0143), a full-display report is now a genuine full-display change, and narrowing it omits real pixels. `[INV-ARCH-98]` is retired in favour of producer-side localization.

### 2. Chrome damage has two triggers, and both are answered in one query

```rust
pub fn chrome_damage_regions(
    &self,
    display: (f32, f32),
    scene_changed: bool,
) -> Option<Vec<tessera_primitives::Rect>>;
```

* `scene_changed` — the scene was rebuilt for a structural reason, so every participating component repainted its pixels.
* otherwise — only components with an animation in flight repaint, plus Lens's own eased values and caret blink.

`None` still means "a component cannot localize; repaint everything", and that escape hatch is preserved unchanged.

### 3. `ChromeEvents::scene_changed` is the structural change signal

A component that mutates its own presentation structure — the dock's tile context menu opening or closing — sets `ChromeEvents::scene_changed`. The shell folds it into the build and exposes it once through `Shell::take_scene_changed`, which clears it.

Reading clears it deliberately: the rebuild it asks for has already happened, so a persistent flag would keep the frame loop awake indefinitely. `Dock::reanchor_app_menu` and `Dock::dismiss_expanded_autohide` now return whether they closed a menu so the caller can report it.

### 4. Chrome footprints are measured through `Chrome::damage_region`

`chrome_intersects_render_area` no longer derives edge thresholds from `reserved`. It answers from the union of every *visible persistent decoration's* `damage_region` — the same rectangle the component repaints — tested against the render area on **both** axes:

```rust
let (x0, y0, x1, y1) = footprint;
area.x < x1 && area.x + area.width  as i32 > x0
    && area.y < y1 && area.y + area.height as i32 > y0
```

A visible non-decoration cannot be bounded by any band, so it disqualifies the elision outright. A decoration that is not visible contributes nothing.

### 5. One named status-band constant

`tessera_shell::HUD_BAND_HEIGHT` (40) is the single definition of the status strip's extent, and a `const _: () = assert!(...)` pins it to the chips' real geometry (`CHIP_TOP + HUD_HEIGHT`). The HUD's `damage_region` and the runtime's clock-rollover tick both consume it; the dead `reserved` local in the runtime is gone.

### 6. Snapshot setters are equality-guarded

`set_settings`, `set_resource_stats`, and `set_interaction_domains` return early when the published value is unchanged, matching `set_system_status`, `set_workspaces`, and `set_space_use`. A `dirty` flag for an identical snapshot rebuilds every component's scene for no pixel difference.

### 7. Text damage follows the glyph run, not the command rectangle

A text command's painted extent is not its `rel` rectangle: negative `w`/`h` centre the run inside the box, so a centred label paints at the box's middle while a rectangle built from `rel` claims its corner — reporting pixels the glyphs never touch and omitting the ones they do.

Resolution: the command carries `text_advance`, the shaped width its widget already measured. Both replay and the damage walk position the run from it through one helper (`lensi_text_bounds`), so they cannot disagree and **neither re-shapes**. Re-shaping was tried first and is wrong for a non-obvious reason: `glyph_measure` outside a frame mutates cache state the next submit depends on, and the damage query runs after `lens_end`. Measurement now happens once, in the widget, where the frame is open.

### 8. `lens_damage_rect` is deleted; only the set exists

The single-rect accessor had no consumer that could not build the union itself, and its existence was an invitation to collapse early. `lens_damage_rects` is the only damage query. Manufacturers of a single rectangle — `iris_frame_packet_damage_rect` for a mailbox frame and `iris_frame_lens_damage_rect` for a direct-present host — live in iris and are documented as the one sanctioned collapse, for backends (Win32, some Vulkan paths) that accept a single `renderArea`.

The Rust binding drops `Ui::damage_rect` with it. `Chrome::damage_region` in Tessera likewise became `Chrome::damage_regions`, returning a list.

### 9. Bounded accumulation degrades toward cover

The damage walk accumulates into a fixed 64-rect buffer with no allocation. When it overflows, the omitted regions have no coordinates to contribute, so the answer is the **full viewport** rather than the bounding box of the survivors — under-covering is the unsafe direction. Callers that need the true regions raise the compile-time bound.

### 10. Rounding is outward on both edges, everywhere

Flooring an origin while ceiling only an extent leaves the far edge short by the fractional part: a band ending at 1042.2 reports 1042 and strands a row. Every logical→integer damage conversion rounds the origin down *and* the far edge up. Fixed in the control center's band merge, the dock's popup accumulator, and the shell's region conversion.

## Consequences

- **Regional composition works end to end.** A HUD tick and a dock spring are separate strips from Lens through the frame damage set to the KMS hint.
- **A one-cell terminal edit repaints one cell.** Measured against the shipped library: `{0,0,1920,1080}` → `{0,0,26,20}`.
- **Centred text is no longer under-reported.** A centred run paints at the middle of its box; damage taken from the box corner omits exactly the pixels that changed. Lens resolves text geometry through one helper shared with replay, so the two cannot disagree.
- **Genuine full-output changes are no longer clipped.** The band substitution that discarded the middle of the screen is gone.
- **Popups appear on the frame they open, and vanish on the frame they close.** The menu is inside the reported footprint; opening, closing, and self-dismissal on click-outside all request a rebuild; and a popup travelling with its sprung anchor repaints the band it vacated.
- **Side-docked bars keep their pixels.** The elision predicate covers all four edges and falls back to "chrome is here" for anything it cannot measure.
- **The status strip repaints completely.** Its band is one pinned constant.
- **Quiescence holds under a chatty poller.** Unchanged snapshots no longer rebuild the scene.
- **Breaking, with no compatibility shim.** `lens_damage_rect` is deleted (only the set exists); `Chrome::damage_region` becomes `Chrome::damage_regions` returning a list; `Shell::anim_damage_regions` becomes `Shell::chrome_damage_regions`; `ChromeEvents` gains a field. Every consumer in both repositories was migrated in the same change.
- **`iris` carries the only sanctioned collapse.** `iris_frame_packet_damage_rect` and `iris_frame_lens_damage_rect` reduce the set to one device-pixel rectangle for backends that accept one. Harmless on a platform with no registered backend: the path never runs.

## Audit of the implementation

The changes above were reviewed after landing and five defects in them were found and fixed. They are recorded because each is a trap the next regional-damage change will meet again:

1. **Uninitialized read on overflow.** The accumulator incremented its count for rectangles it could not store, and the normalizer was handed that larger count — so it read past the fixed buffer into uninitialized stack memory. Confirmed by ASan/UBSan. The accumulator now tracks an explicit `overflowed` flag and only written entries are ever read.
2. **Overflow under-covered.** Answering an overflowed walk with the bounding box of the *recorded* rectangles omits the unknown ones — the unsafe direction. An overflowed accumulation now answers with the full viewport.
3. **Text sentinels ignored.** `rel.w < 0` / `rel.h < 0` mean "centre the run in the box". Measured: a centred label in a 380px box reported `{0,0,10,20}` while the glyphs painted at x ≈ 191.
4. **Re-shaping inside the damage query.** The first fix for (3) called `glyph_measure` from the damage walk. Because that runs after `lens_end`, it corrupted the glyph cache and the *next* frame's `lens_snapshot_submit` failed with `FLUX_ERROR_INVALID_STATE`. Measurement moved into the widget, which records `text_advance` on the command.
5. **Rounding short on the far edge.** The control-center band merge floored the origin and ceiled only the extent, leaving the far edge up to 1px short — an under-covering region.

Each has a regression test: `test_overflow_collapses_to_one_bounding_box` and `test_centered_text_damage_follows_the_glyphs` in `tests/lens/test_repaint.c`, and the band-localization tests in `control_center/tests.rs`.

## Invariants & Behavioral Boundaries

- `[INV-ARCH-98] (Retired)` *Persistent decoration animations never poison client workspaces.* The band substitution it mandated is deleted: with producer-side paint-bounds localization an unlocalized full-display footprint is a genuine full-output change, and narrowing it omits real pixels. Superseded by `[INV-ARCH-104]`.
- `[INV-ARCH-102] (Amended)` *Chrome overlay pass is skipped only when the measured chrome footprint misses the render area.* The test consumes the union of every visible persistent decoration's `damage_region` on both axes. No extent may be derived from `reserved`, which is a window-placement inset. A visible non-decoration disqualifies the elision.
- `[INV-ARCH-103] (Amended)` *Quiescent Shell Scene Elision.* Holds only if every host-driven setter is equality-guarded; a setter that dirties unconditionally defeats it for its consumers.
- `[INV-ARCH-104] Chrome damage is forwarded, never re-derived.` `Shell::chrome_damage_regions` publishes the producer's rectangle list unchanged. The only sanctioned consumer-side response to an unusable report is `None` (repaint everything).
- `[INV-ARCH-105] Every chrome paint extent has one named definition.* The status strip's extent is `HUD_BAND_HEIGHT`, pinned at compile time to the chips' geometry; no damage path may hardcode its own band height.
- `[INV-ARCH-106] A structural chrome change without an animation must still repaint.* A component that mutates its own presentation structure sets `ChromeEvents::scene_changed`; `anim_pending` drives the frame clock and is not a change notification.

---

## Negative Knowledge

- **Never "correct" a reported damage rectangle downstream.** A consumer that clamps, substitutes, or unions a producer's region has created a second source of truth, and the producer's next improvement will be silently discarded. The one legitimate consumer-side response to an unusable report is `None` / `FrameDamage::Full`, which is honest.
- **Never encode a chrome extent as a threshold computation on `reserved`.** `reserved` is a *window-placement* inset, not a chrome-paint footprint; a side-docked bar reserves `left` while `bottom` is zero, so any band derived from it vanishes. Ask the component where it paints.
- **Never treat `anim_pending` as a change notification.** It answers "keep the clock running". A structural change that does not animate has no clock and needs its own one-shot signal, or it renders into the model and never into a frame.
- **Never set a dirty flag unconditionally in a setter the host calls every frame.** The quiescent-elision is only as good as its quietest caller.
- **Never derive a damage rect from a draw command's `rel` without checking its sentinels.** Negative `w`/`h` mean "centre within the box", not "no extent"; a rect built from such a command describes the box corner and omits the painted run.
- **Never let a counting accumulator outrun its buffer.** Counting a rectangle you could not store is how an in-bounds loop becomes an uninitialized read; track overflow explicitly and never read an unwritten slot.
- **Never degrade an unknown damage region to the bounding box of the known ones.** Omitted regions have no coordinates to contribute, so the box under-covers them.

## References

- [ADR-0203](0203-damage-is-a-placement-mapped-rectangle.md) — Damage is a placement-mapped rectangle.
- [ADR-0210](0210-performance-core-affinity-allocation-zero-copy-strided-uploads-and-quiescent-lens-lifecycle.md) — Regional damage containment and quiescent Lens lifecycle (amended).
- [ADR-0198](0198-collapsed-dock-capsule-hugs-its-edge-behind-an-independent-reveal-corridor.md) — Dock reveal corridor (amended: the popup footprint is part of the dock's damage).
- Optics ADR-0143 — Regional frame damage as a rect list, and paint-bounds localization. The producer-side contract this record consumes.
