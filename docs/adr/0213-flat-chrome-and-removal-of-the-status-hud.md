---
id: ADR-0213
title: "Flat Chrome: the Compositor Draws No Material, and the Status HUD Is Removed"
status: accepted
date: 2026-11-04
scope: design, shell, rendering, composite, config, architecture
superseded_by: null
negative_knowledge: true
---

# 0213. Flat Chrome: the Compositor Draws No Material, and the Status HUD Is Removed

- Status: Accepted
- Date: 2026-11-04
- Deciders: Tessera Maintainers
- Supersedes: the analytic-glass material model established by [ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md), [ADR-0105](0105-single-body-liquid-glass-interaction-focus.md), [ADR-0120](0120-glass-material-roles-and-region-level-backdrop-adaptation.md), and the explicit offscreen composition DAG of [ADR-0143](0143-explicit-offscreen-composition-dag.md) as it applies to chrome materials
- Amends: [ADR-0021](0021-chrome-component-trait.md) (the `Chrome` contract loses its material declarations), [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0081](0081-hud-and-command-panel-naming.md), [ADR-0083](0083-frameless-transient-toasts-and-hud-consolidation.md) (the display-only status HUD and its `[hud]` table are removed)
- Related: [ADR-0212](0212-zen-vram-minimalism-scratch-consolidation-and-tight-effect-budgets.md) (the VRAM work that measured the cost)

---

## Context and Problem Statement

Chrome bodies were analytic lenses: each surface declared a region, the
compositor rendered the desktop into a full-resolution offscreen capture, and a
per-pixel shader refracted it through a convex slab with dispersion, rim lighting
and an adaptive tint ([ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)).
The look was deliberate. Three facts made it the wrong trade.

**It was the dominant steady-state GPU cost.** ADR-0212 measured it directly: the
backdrop pipeline, not the wallpaper or idle composition, dominated normal-use
GPU time. Every frame in which a glass body's contents moved — a scroll, a window
drag, a dock dwell, a cursor crossing a chip — paid the capture and the shader
again. On a laptop panel that is the difference between a cool machine and a
warm one, and it is paid continuously for a decorative surface.

**It was most of the resident VRAM.** The same investigation found 1.2–1.6 GiB
resident with a dock, the HUD and a few windows open, against roughly 100 MiB for
a compositor that only does direct scanout. The capture targets, composite slabs
and resolved images were the largest single contributor.

**It competed with the content it floated over.** This is the argument that
matters beyond cost. A lens bends the pixels beneath it, adds a rim and a
directional key light, and shifts its body tint against the backdrop. All of that
is exactly what draws the eye to the material rather than to the window. A
surface that is *supposed* to be quiet was spending real GPU time asking to be
looked at.

The status HUD failed the same test from the other direction. It was a
display-only strip across the top of the screen: no reserved space, no pointer
capture, a fade when the cursor approached. It could not be interacted with, and
what it showed — workspace position, charge, link state — is information a user
reads at a glance while reaching for a control, not information that earns a
permanent band above every window.

## Decision Drivers

- **Chrome must not cost more than its area.** A surface's per-frame price should
  be proportional to the pixels it covers, not to a sampling radius or a
  per-pixel shader.
- **A quiet surface does not compete with content.** The product's chrome is
  infrastructure; anything that pulls attention to itself has to earn that.
- **Readings belong where the question is asked.** A user asks "which desktop am
  I on" while reaching for the dock, not while reading a window.
- **No mechanism without a consumer.** A contract that lets components declare a
  material must be deleted with the material, or the next component will declare
  one and the cost returns.

## Considered Options

- **Keep the material, add a low-power mode.** Rejected: two looks to maintain,
  two sets of contrast budgets, and the expensive one is still the default a user
  actually runs. It also keeps the whole capture and shader path compiled in.
- **Reduce the material's cost (smaller captures, lower resolution, cheaper
  shader).** Rejected by [ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)'s
  own findings: quarter-resolution capture was the source of the smear the full
  resolution change existed to remove, and the interior-work elision of
  [ADR-0202](0202-single-pass-full-output-backdrop-capture-and-glass-interior-compute-elision.md)
  already takes the provably-null pixels. What remains is the effect.
- **Make the HUD configurable rather than removed.** Rejected: a setting whose
  default is wrong for most users is a setting most users never find, and the
  surface has no interaction to justify its band.
- **Flat painted bodies with a painted elevation halo.** Chosen.

## Decision Outcome

Chosen option: "Flat painted bodies with a painted elevation halo", because a
surface whose cost is one rounded-rect fill can be on screen all day, and because
a still surface lets the window be the brightest thing in the frame.

### The design vocabulary names surfaces, not materials

`GlassRole`, `GlassStyle`, `GlassStyles` and `GlassFocus` become `SurfaceRole`,
`SurfaceStyle`, `SurfaceStyles` and `SelectionStyles`. A role resolves to an
opaque fill, a hairline boundary, a corner radius and a shadow configured in
logical pixels. `glass_surface`/`glass_border` become `panel_surface`/
`panel_border`, and `hud_foreground` becomes `floating_foreground` — the contour
policy the dock capsule and its ticks still need, named for what it does rather
than for the surface that used to need it.

`SurfaceRole::Tile` exists for a cell that sits *on* a panel canvas: it carries a
fill and no shadow, and `SurfaceRole::floats` is what a caller asks instead of
testing for a role by name.

### Separation is painted, not sampled

A floating body still has to read as floating. `materials::elevation_halo`
returns two inflated rounded fills at low alpha, outermost first, drawn beneath
the body: a soft edge at the cost of two fills, with no sampling radius anywhere.
`materials::place_floating` places the halo and the body together so no caller
can emit one without the other.

### The `Chrome` contract loses its material declarations

`backdrop_blur_sigma`, `backdrop_regions`, `liquid_glass_regions` and
`backdrop_layers` are deleted. A component no longer declares a material the
compositor draws behind it; it paints its own body.

Two explicit declarations replace what the compositor used to *infer*:

- `paints_opaque_cover` / `cover_opacity` let a full-output surface state whether
  everything beneath it is elidable this frame. Inferring that from a capture's
  coverage meant a cover mid-fade either held its body hostage at full strength
  or vanished in a single frame.
- `samples_scene_effect` covers the one remaining scene consumer, the blurred
  window-shadow layer.

`CompositionRequirements::live_backdrop_effect` becomes `live_scene_effect` to
name what it now gates.

### The compositor draws no material at all

The backdrop graph planner and its capture-region algebra, the capture/material
cache keys, the per-slot capture targets and composite slabs, the prism operator
plumbing, the region-level adaptation pass and the `prism` dependency are all
deleted. The frame is: build damage, record the window shadow layer, draw the
desktop base, draw chrome on top.

### The status HUD is removed

The component, its crate directory, its `[hud]` configuration table and the
`chrome-hud` feature chain are deleted. A stale `[hud]` table is rejected as an
unknown field rather than silently ignored, because silently ignoring it would
leave a user believing they had configured a surface that no longer exists.

The top of the screen belongs to windows. Transient corner chrome anchors to
`SCREEN_CORNER_INSET` instead of to the band the chips used to occupy.

### What replaced the HUD's readings

- **Workspace position** is the dock's leading Bento cell. It answers the
  question where the question is asked, and the dock's own reveal already puts it
  on screen at that moment instead of colouring the top of every window all day.
- **Charge, link state and device state** are transient system readings
  ([ADR-0214](0214-tiered-system-readings-and-a-barrier-only-at-the-end.md)),
  shown for a couple of seconds when they change and then gone.
- **Notification history** stays in the command panel, which has room for it.

### Invariants & Behavioral Boundaries

- Every `SurfaceRole` resolves to a body whose alpha is at least 236. A chrome
  body that leaned on a backdrop effect would be a translucent smear over
  application content, and there is no effect left to lean on.
- A role that `floats` has a non-zero shadow; a role that does not casts nothing.
- No crate may re-introduce a backdrop capture, a blur operator, or a per-pixel
  material shader behind chrome. `tessera` no longer depends on `prism` at all.
- The dock holds one edge offset in both states, and its reveal corridor is
  composed from the edge distance, the capsule body, and the approach band rather
  than picked as one number.

## Rejected Alternatives & Negative Knowledge

- **Two captures, quarter-resolution for frost and full-resolution for glass.**
  Rejected in [ADR-0094](0094-liquid-glass-lens-model-and-full-resolution-capture.md)
  for doubling the scene render; moot now that neither exists.
- **A painted fill over the material instead of removing it.** This is what the
  product effectively had (a minimal painted layer plus a material body under
  it), and it was the worst of both: the cost of the material and the flatness of
  a fill.
- **Keeping `[hud] enabled = false` as the default.** Rejected: it leaves a
  component, a feature, and a configuration table alive for a surface nobody
  should run, and every future reader has to learn why it exists.
- **A block-level modal for every battery threshold.** Rejected by
  [ADR-0214](0214-tiered-system-readings-and-a-barrier-only-at-the-end.md): the
  previous design raised a pointer- and keyboard-capturing barrier to report a
  level a glance would have covered.

## Consequences

- Chrome's per-frame cost is a function of its area. A frame with a dock, an open
  command panel and a switcher no longer pays a full-resolution scene render plus
  a lens shader on top of the scene render it already draws.
- `tessera` drops its `prism` and `prism-sys` dependencies. The workspace patch
  entries for them report as unused, which is the observable proof that no code
  path requests a prism operator.
- 3049 lines leave `tessera` in the pipeline deletion alone; `presentation.rs`
  goes from 2955 to 2387.
- The 24 rendering tests that exercised the deleted mechanism are deleted with
  it. A test for a removed mechanism is a second copy of the removed mechanism.
- Contrast is now a property of the palette rather than of an adaptive shader.
  Text-bearing roles take the palettes' highest-contrast body (menus at alpha
  248/250) instead of boosting frost and pinning plate polarity.
- A future surface that wants optical treatment has to argue for the capture and
  the shader on their own merits, with no existing declaration surface to
  inherit.
