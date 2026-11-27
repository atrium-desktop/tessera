---
id: ADR-0267
title: "In-Bounds Bezel-Flush Hot Zone Geometry, Per-Corner Analytic SDF, and Exact Damage Parity"
status: accepted
date: 2026-11-27
scope: shell/hot-zone, design, optics-integration
superseded_by: null
negative_knowledge: true
---

# 0267. In-Bounds Bezel-Flush Hot Zone Geometry, Per-Corner Analytic SDF, and Exact Damage Parity

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0254](0254-dual-channel-reachability-and-component-mounted-hot-zones.md)
  (replaces out-of-viewport negative-coordinate clipping with strictly in-bounds
  per-corner analytic squircle geometry, adds bounded `pointer_damage_regions`,
  consolidates hover dwell advancement into `prepare_frame`, and retires
  suppressed slots immediately)
- Related: Optics ADR-0175 (`Per-Corner Analytic Squircle SDF, Unified RRect Geometry, and Single-Representation UI Corner Radii`)

---

## Context and Problem Statement

The initial ADR-0254 `HotZones` implementation attempted to create crisp 90°
bezel edges and quarter-circle corner arcs by placing oversized uniform-radius
rectangles and full `2r × 2r` circles past the screen boundary (e.g.
`x: -22, y: -22, w: 44, h: 44` for `TopLeft`, `y: -8, h: 14` for `Top`) and
relying on viewport clipping to cut away the off-screen portion.

This broke in three compounding ways:

1. **Viewport clamping turned corner arcs into full circles and doubled edge tab thickness**:
   `optics-ui`'s `OPTICS_UI_PLACE_EXACT` layout solver clamps every placed box
   into `[0, display.w] × [0, display.h]`. Consequently `TopLeft` was pushed to
   `(0, 0, 44, 44)` (a full 44×44 circle inside the corner) and edge tabs were
   pushed to 14 px-thick all-rounded pills.
2. **Partial-damage scissors sliced the clamped shapes in half and stranded ghost pixels**:
   `HotZones::damage_regions()` reported `visual_bounds` (`22×22` corners,
   `6 px` edge tabs, plus 2 px padding), which was only half the clamped paint
   extent. Because `HotZones` animated in Rust without setting `scene_changed`,
   `optics-ui`'s own `damage_rects` were not merged during animations; the GPU
   partial-damage scissor literally cut the clamped circle/pill in half on live
   frames, while retiring only cleared half the footprint and left ghost pixels
   across swapchain slots.
3. **Screenshots hid the defect**:
   Both Print-key `screenshot_freeze` (`CanvasPassOptions { damage: Vec::new() }`)
   and IPC capture frames re-render cleanly with `FrameDamage::Full` into a
   fresh target rather than reading back the live partial-damage swapchain, so
   captured screenshots never reproduced the half-clipped live frames.

Additional state-machine defects were found alongside the geometry bug:
- `HotZones` did not implement `Chrome::pointer_damage_regions`, causing every
  pointer move inside a `Click` trigger hitbox to fall back to `FrameDamage::Full`.
- Transitions without ongoing easing (`reduced_motion` instant reveal, final
  settle frame `0.995 -> 1.0`, or leaving a trigger hitbox back into proximity)
  emitted no damage.
- Suppressing a visible slot (`DockExpanded` / `LauncherActive`) stopped
  painting immediately in `render()` while still running ~20 empty decay frames
  in `advance_slots()`.
- Hover dwell timers advanced inside `render()` and zeroed `dwell_secs` before
  computing `dwell_boost`, flickering the accent colour back to neutral on the
  summon frame.

## Decision

1. **Strictly In-Bounds Bezel-Flush Geometry via Optics ADR-0175 Per-Corner Analytic SDF**:
   - Delete `bezel_flush_paint_rect()` and all negative/overflowing placement
     coordinates.
   - Replace it with `HotZones::bezel_flush_geometry(slot, display, reveal) -> (Rect, CornerRadii)`
     whose rectangle is strictly contained inside both `[0, display.0] × [0, display.1]`
     and `HotZones::visual_bounds(slot, display)` for every `reveal ∈ (0, 1]`.
   - Edge slots (`Top`, `Bottom`, `Left`, `Right`) paint at exact `w × t` / `t × h`
     with `0.0` radius on both bezel-facing vertices and `r = t` on both
     desktop-facing vertices plus continuous G2 curvature (`0.6`).
   - Corner slots (`TopLeft`, `TopRight`, `BottomLeft`, `BottomRight`) paint at
     exact `r × r` anchored to the screen vertex with `r` exclusively on the
     single desktop-facing corner and `0.0` on the other three vertices plus
     continuous G2 curvature (`0.6`), evaluated directly by `optics-gfx`'s
     analytic per-corner squircle SDF on CPU and Vulkan GPU.
2. **Exact Damage and State-Machine Parity**:
   - Track per-slot `dirty` across all visual state changes (`reveal`,
     `in_trigger`, `dwell_secs`, `hover_latched`, suppression, appearance, and
     policy reloads) and iterate `HotZoneSlot::ALL` in `damage_regions()` so
     freshly unbound slots retire cleanly.
   - Implement `Chrome::pointer_damage_regions()` returning each touched slot's
     bounded padded `visual_bounds` instead of `None`.
   - Immediately retire suppressed slots (`reveal = 0.0`, `retire_frames = RETIRE_DRAIN_FRAMES`)
     instead of decaying invisibly.
   - Advance hover dwell timers in `prepare_frame()` (`advance_slots()`), queue
     triggered summons in `pending_summon`, and preserve `dwell_boost == 1.0`
     while `in_trigger && hover_latched`.
3. **Clean-Break Adoption of Unified `optics_ui::CornerRadii`**:
   - Migrate all `LayoutOpts` call sites across `tessera-design`, `tessera-shell`,
     and `tessera-settings` from the retired `radius: f32` field to
     `corner_radii: CornerRadii`.

### Invariants

- **`[INV-HOTZONE-06] In-bounds paint containment`**: For every `slot ∈ HotZoneSlot::ALL`
  and every `reveal ∈ (0, 1]`, `HotZones::bezel_flush_geometry(slot, display, reveal).0`
  is strictly contained within both the output viewport `[0, display.0] × [0, display.1]`
  and `HotZones::visual_bounds(slot, display)`. Live partial-damage frames and
  full-damage capture frames therefore paint identical pixels.
- **`[INV-HOTZONE-07] Bounded pointer hover damage`**: `HotZones::pointer_damage_regions`
  always returns `Some(regions)` bounded to the touched slot footprints and
  never escalates pointer motion inside a hot zone to `FrameDamage::Full`.

## Rejected Alternatives & Negative Knowledge

### Sub-Pixel Horizontal Band Approximation (like `screenshot.rs`'s `rounded_corner_bands`)
- **Why considered**: Requires no changes to `optics`.
- **Why rejected**: Emits dozens of flex rows/columns per corner every animation
  frame, approximates smooth curves with stair-stepped rectangles, and leaves
  the underlying `optics` gap unresolved.

### Adding an "Unclamped Placement" Escape Hatch to `optics-ui`
- **Why considered**: Preserves the negative-coordinate `bezel_flush_paint_rect()`
  trick.
- **Why rejected**: Leaks off-screen geometry across adjacent outputs on
  multi-monitor canvases, breaks damage/hit/a11y bounding boxes, and fails to
  produce 90° tangents when the off-screen pill's midline lies outside the bezel.

## Consequences

### Positive
- Edge tabs and corner arcs match ADR-0254's bezel-flush 90° + continuous-curvature
  specification with single-draw analytic SDF antialiasing and pixel-exact
  partial-damage bounds.
- Zero idle cost (`[INV-HOTZONE-01]`) holds across normal motion, reduced
  motion, suppression transitions, and live config reloads.
- Live frames and screenshots agree pixel-for-pixel.

### Negative / Trade-offs
- Requires Optics ADR-0175 (`optics_ui::CornerRadii`, unified `LayoutOpts.corner_radii`,
  and `optics_gfx_sdf_squircle_radii`).
