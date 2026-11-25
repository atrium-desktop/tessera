---
id: ADR-0253
title: "A Backdrop Capture Beneath Chrome Draws the Glass Materials"
status: accepted
date: 2026-11-25
scope: runtime, rendering, design, shell
superseded_by: null
negative_knowledge: true
---

# 0253. A Backdrop Capture Beneath Chrome Draws the Glass Materials

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0252](0252-chrome-materials-are-one-swappable-shell-wide-choice.md) (`RENDERER_MATERIAL_CEILING` rises to `LiquidGlass`; a sampled material keeps its own body tint instead of being painted as translucent)
- Amends: [ADR-0226](0226-single-pass-chrome-composition-preview-layering-and-modal-dock-inhibition.md) (while a glass material is in effect, the frame takes offscreen capture passes before the single output pass; the output pass itself stays single)

---

## Context and Problem Statement

[ADR-0252](0252-chrome-materials-are-one-swappable-shell-wide-choice.md) made
the chrome material a user choice and accepted `frosted` and `liquid-glass`
into the vocabulary, but drew both as `translucent`. Drawing them needs three
things [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) deleted
from Tessera: where each body is, a capture of the desktop beneath chrome,
and the filter dispatch. ADR-0213 asked that the capture and the shader
"argue on their own merits". This record does that.

The merit is the user's: glass is the look they asked for, and ADR-0252 makes
it a choice paid for only by the people who pick it. What this record must
show is that the price stays inside that choice: an opaque or translucent
shell runs exactly ADR-0213's frame, and the glass frame is bounded.

## Decision Drivers

- **No idle cost** ([INV-MAT-05]): no filter, capture target or pass exists
  unless the effective material samples the backdrop.
- **No second declaration of geometry.** Every floating body already passes
  through one helper; making each component report its rects again is a
  second source of truth that drifts.
- **Bounded per frame.** The capture covers the bodies, not the output.
- **A failure degrades, it does not break.** A capture that cannot be taken
  must not leave glass-tuned bodies over an unblurred desktop.

## Decision Outcome

### The shell reports its bodies

`tessera_design::materials::place_floating`, through which the Dock, the
Control Center, notifications, the battery alert, the OSD, modal prompts and
Pivot place their bodies, reports each body's bounds, corner radius and fade
to a ledger, but only while the component's `Design` is in a backdrop-sampling
material. `Shell::build_scene` brackets the optics-ui frame with
`record_backdrop_bodies`, keeps the list of the last successful build next to
`last_good_scene`, and exposes it as `Shell::backdrop_bodies` (empty under a
painted material). A body that casts no halo, such as a menu, the tray popover
or the screenshot loupe, is placed through `materials::place_body`, which
reports the same way. A bare `frame.place` of a body would get the tint
without the frost.

The Control Center's cards paint from `Design::control_center_colors`, which
carries the floating panel's material alpha onto its solid-surface palette.
Before this they used the bare palette and stayed opaque under every
material. The ledger is a scoped thread-local: a body placed outside
a recording, such as in a test or the settings app, is drawn and not reported.

### The runtime captures beneath them

`runtime::backdrop` turns the bodies into a `BackdropPlan` in output pixels.
Each body's *reach* is the body padded by the blur footprint (3σ, plus the
lens's refraction for liquid glass) and clamped to the output. The capture
image covers only the union of the reaches, rounded up to 128 px buckets: a
lone Dock costs a strip, not a full-output target.

At the pass boundary before the output pass opens, on the output that
composites chrome:

1. The 2D wallpaper and the same desktop scene the output pass draws are
   rendered into the capture in **one** canvas pass scoped to the reaches.
2. `optics_gfx::BlurFilter` blurs the reaches (σ = 20 logical px for frost,
   8 for glass).
3. `optics_material::FrostedFilter` or `LiquidGlassFilter` writes one slab:
   each body's rounded shape, at its own fade, with the filter's tint at zero
   and its drop shadow off. Chrome glass runs at `frost_strength` 4.0, so
   about two thirds of a lens interior is frosted. Optics' default (8%) is
   clear glass, which is too busy behind text. The refracting rim and its
   highlights are unchanged.
4. The output pass draws the desktop, then the slab, then the chrome. The
   shell's body fill is the tint over the slab, and its painted halo is the
   elevation.

The output pass is unchanged and still single (ADR-0226); the capture passes
precede it like the window-shadow masks of ADR-0139.

Only the compositor package (`tessera`) may depend on `optics-material` and
`optics-material-sys` (the ADR-0159 dependency policy). The design and shell
crates stay data-only: they report bodies and never see a capture.

### Bodies are the tint

`Design::with_material` no longer paints a sampled material as translucent.
Frosted and liquid glass have their own body alphas, more open than
translucent because a blurred backdrop carries no detail that competes with
text (dark frost: menus 214, panels 194, Dock 176; dark glass: 196, 168,
140). The legibility order (menus > panels > Dock) holds in every material.

### Damage follows what a body samples

A frosted Dock changes when a window beside it redraws, without that window
touching it. Before the frame begins, damage that intersects a body's reach
is widened to the whole body (`BackdropPlan::widen`), on the single-surface
path and on the primary output target. A pass whose scope touches no body
takes no capture (`BackdropPlan::restrict_to`).

### Failure demotes the material

Any allocation, capture or dispatch failure marks the backdrop unavailable
for the session. Before the shell builds its next frame,
`enforce_backdrop_capability` sets the shell to `translucent`, the most
expensive material that needs no capture, and logs it. A configuration
reload that asks for glass again is demoted again on its first frame. While
the effective material is painted, the same check releases every filter and
capture target.

### Invariants & Behavioral Boundaries

- `[INV-MAT-07] One source of body geometry`: the backdrop is drawn exactly
  where `place_floating` placed a body in the scene being submitted. No
  component declares backdrop geometry separately.
- `[INV-MAT-08] The capture is bounded by the bodies`: capture passes and
  blur regions are scoped to the bodies' reaches. No glass frame captures the
  full output unless the bodies cover it.
- `[INV-MAT-09] A body repaints with what it samples`: damage inside a body's
  reach repaints that body in the same frame.
- `[INV-MAT-10] Failure is translucent`: a backdrop that cannot be drawn
  demotes the effective material to `translucent` before the next build.
- [INV-MAT-05] holds by construction: `ChromeBackdrop` allocates lazily and
  is released on every frame whose material does not sample the backdrop.

## Rejected Alternatives & Negative Knowledge

### A `Chrome::backdrop_regions` method on every component
- **Why considered**: ADR-0252 named "a per-component body-region declaration
  on the `Chrome` contract" as the first missing piece.
- **Why rejected**: every component already hands its body to
  `place_floating`. A second method is a second copy of the same geometry,
  and the copies drift: the Dock's body morphs every frame of its reveal.

### Re-introduce ADR-0142's layered backdrop graph
- **Why rejected**: per-layer captures (frost under glass, glass sampling
  frost) are what made the old pipeline dominate GPU time and VRAM. One
  capture beneath all chrome is enough. A popover over the Dock samples the
  desktop, not the Dock, which reads the same at these blur widths.

### Blur a downscaled copy of the finished frame
- **Why rejected**: reading back the output means ending and reopening the
  single output pass (ADR-0226's LOAD round-trip) and samples the chrome
  itself. Re-drawing the scene under the reaches costs less than that.

### Capture the wallpaper and the scene in separate passes
- **Why considered**: it mirrors `begin_desktop_output`, which draws the 3D
  wallpaper model between two passes.
- **Why rejected for now**: Optics v0.0.86 binds a deferred effect to the
  *first* pass that wrote its input, while its render-plan validation demands
  a dependency path to *every* writer. A two-writer capture is therefore
  rejected with a bare `OPTICS_GFX_ERROR_INVALID_STATE` when the next output
  pass opens. This was fixed in Optics after v0.0.86, together with the
  write-after-read rejection that also cost a window its shadow when two
  shadows were rebuilt in one frame (Optics ADR-0147, amended 2026-11-25).
  The fixes shipped in Optics v0.0.87, which Tessera now pins. The capture
  stays one pass anyway: it is the cheaper capture, and the 3D model still
  cannot join it, because the model renders a full-output target while the
  capture covers only the bodies' extent.

### Keep sampled materials painted as translucent and add the slab on top
- **Why rejected**: translucent alphas are tuned to hold text over sharp
  content on their own. Over a blur they are nearly opaque, and the glass
  barely shows.

## Consequences

### Positive
- All four materials are drawn as named. Settings no longer describes glass
  as "drawn as Translucent".
- Opaque and translucent shells hold no filter, no target and no extra pass.
- Any surface placed through `place_floating` gets the glass without
  per-component work.

### Negative / Trade-offs
- A glass frame draws the desktop scene twice inside the reaches and runs a
  blur plus a material dispatch. With the Dock visible, that is every frame
  that repaints near it.
- Partial damage grows: a redraw beside a glass body repaints the whole body.
- One capture target per frame slot per output stays resident while glass is
  in effect.
- A screenshot freeze's capture frame takes its own backdrop with a second
  `ChromeBackdrop`. Sharing the live one's capture or filter slots in one
  frame would give their images a second writer, which Optics v0.0.86
  rejects. That is one more set of resident targets while glass is in
  effect.
- A 3D wallpaper model is not captured, because it renders a full-output
  target and the capture covers only the bodies' extent: glass shows the 2D
  wallpaper behind it.

### Verification
- `runtime::backdrop` tests render sharp 4 px stripes under a body on the
  GPU and read the frame back. Frosted turns the body into a uniform grey
  (spread ≤ 6 of 255) while the stripes stay sharp up to its edge, and
  liquid glass holds a frosted interior with a refracting rim. These tests
  skip on a machine with no Vulkan device.
