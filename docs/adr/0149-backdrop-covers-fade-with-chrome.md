# ADR-0149: Backdrop covers fade with the chrome they back

- Status: Accepted
- Date: 2026-09-07
- Scope: `tessera-chrome`, `tessera-command-panel`, `tessera-shell`,
  `tessera-prism`, and the compositor backdrop executor; extends
  [ADR-0123](0123-chrome-fades-via-lens-opacity.md) and rides the material
  half of [ADR-0143](0143-explicit-offscreen-composition-dag.md)

## Context

ADR-0123 gave chrome enter/exit fades one mechanism: the frame-scoped lens
opacity switch, which bakes a build-time opacity stamp into every draw
command of the subtree built under it. That covers everything lens draws —
rects, borders, text, icons, host images, scrollbars — and closed the
per-widget fade-patching bug class for chrome pixels.

It does not cover the pixels chrome does not draw. An immersive surface
that sits on a full-screen backdrop cover (the command panel, the launcher,
the prism spotlight) is composited as *compositor-side material* under its
lens content: a declared `BackdropRegion` becomes a frost rect in a layered
backdrop material, and the chrome only paints on top of it. The cover
declaration had no visibility channel at all — `BackdropCover::region`
produced a fixed wash and the executor mapped every frost rect with
`opacity: 1.0` hardcoded — so the cover could only be present or absent.

The result was a visible defect at the tail of every close animation. The
lens content faded out over ~200 ms while the frosted sheet beneath it held
full strength, leaving a flat gray-white plate over the desktop; when the
component finally reported inactive, the plate vanished in a single frame.
The animation read as "fade the panel, then blink the backdrop away". The
same defect ran in reverse on open, where the cover popped in at full
strength before the content arrived.

The missing capability was not in the graphics stack: Prism's layered
backdrop material has always taken a per-frost `opacity` that blends the
frosted body against the sharp backdrop inside the rect's SDF coverage
(`mix(sharp, frosted, coverage * opacity)`). Tessera declared the field in
its descriptor but never populated it.

## Decision

A backdrop cover's visibility is part of its declaration, and it fades with
the surface it backs. `BackdropRegion` gains an `opacity` in `[0, 1]`;
`BackdropCover::region` takes the surface's enter/exit progress and scales
both the declared scrim wash strength and the frost body by it, while
`modal_scrim_backdrop` (instant modals with no exit animation) declares
`1.0`. Every animated surface that declares a full-screen cover passes its
own reveal clock: the command panel and the launcher use their content
fade's curve, the prism spotlight its visibility.

The compositor maps that opacity and the declared wash straight into
Prism's `BackdropFrost`, so one material value governs the whole cover.

Opacity and wash are **material**, not capture: they join the backdrop
material key, not the capture key. A fade therefore re-runs the effect
composite over the still-valid scene capture — it does not re-render
clients, re-capture the desktop, or change the blur radius. This is the
same profile as any other animating glass or frost material (the dock's
autohide morph, a HUD chip fade), and it is the reason the two keys are
split. The radius stays constant for the entire fade precisely so the
capture cache cannot teardown mid-animation (the launcher's documented
bright-flash failure mode); only the frost body eases.

## Alternatives

- **Keep the cover at full strength and delay teardown.** Extending the
  component's `active()` window by a fixed number of frames hides the pop
  behind extra frames of a fully opaque plate — the plate is still there,
  just longer. It trades a visible pop for a visible stall.
- **Fade the cover by alpha instead of frost body.** Compositing the cover
  as a translucent sheet over the sharp desktop would leave the layered
  material's output non-opaque, which the glass pass above it depends on:
  a lens may sample outside its own body's footprint and would read
  premultiplied half-brightness fragments or transparent holes. The
  material's own `opacity` blends frost-vs-sharp and keeps the resolve
  opaque, which is why it exists.
- **Animate the blur radius to zero.** Radius is capture-side: changing it
  per frame invalidates the capture key on every frame of the fade and any
  failed rebuild falls through to the sharp desktop — the flash ADR-0123's
  launcher work already had to fix once.
- **Per-component painted scrims on top.** A chrome-painted veil above the
  glass hides the glass's refraction and splits the stack into "effects
  below, paint above" — the defect ADR-0142 removed.

## Consequences

- New chrome surfaces with a full-screen backdrop cover must pass their
  reveal progress to `BackdropCover::region`; a cover declared at constant
  strength reintroduces the teardown pop.
- The fade channel is a material value, so mid-fade frames recompute the
  effect composite but reuse the capture. Components must still keep their
  declared capture geometry (region rects) constant across the fade, or
  every frame re-captures the scene.
- Blur sigma stays a step function of `active()`: it is set once when the
  surface becomes active and cleared once when it settles. Easing it is a
  regression, not a refinement.
- Surfaces whose pixels come from lens only (overview thumbnails, live
  previews) are unaffected: they keep their own progress plumbing.
