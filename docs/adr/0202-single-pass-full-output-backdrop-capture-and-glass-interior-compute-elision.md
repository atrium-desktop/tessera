---
id: ADR-0202
title: "Single-pass full-output backdrop capture and glass interior compute elision"
status: accepted
date: 2026-11-02
scope: core/rendering, material/glass
superseded_by: null
negative_knowledge: true
---

# 0202. Single-pass full-output backdrop capture and glass interior compute elision

- Status: Accepted
- Date: 2026-11-02
- Deciders: ming
- Consulted: optics maintainers (opts lineage ADR-0063)
- Informed: compositor runtime, prism material library, flux RHI

---

## Context and Problem Statement

Tessera's normal-use GPU cost is dominated by the Liquid Glass backdrop
pipeline, not by the wallpaper or by idle composition. Two independent
sources of redundant GPU work were identified, one in each repository:

1. **Compositor scene double-draw.** On a full-output Liquid Glass frame the
   desktop scene (wallpaper + client windows + soft shadows) was rendered
   twice per frame: once *into the backdrop capture* (the offscreen target
   that the blur and prism operators sample) and once *into the output commit
   target*. A full-screen refresh therefore paid for two identical scene
   renders plus the capture/effect passes, and any animation under glass
   (scroll, window drag, dock dwell) repeated that double-draw at the full
   output refresh rate.

2. **Glass shader interior work.** Every liquid-glass pixel executed the same
   workload regardless of how far it sat from the silhouette: four SDF
   gradient taps, three chromatic-dispersion backdrop samples, a meniscus
   specular rim (two pow chains plus two gaussian envelopes), and a contact
   AO crease. For a typical body more than half the pixels are deep in the
   flat interior where the lens bend is exactly zero, making all of that work
   numerically null.

A 3072×1920@120 output at scale 2.0 multiplies both costs: the capture pixel
scale doubles every refraction/edge/rim operand, and every frame at 120 Hz
re-runs the work. The goal is to eliminate provably-null work without
changing a single visible pixel.

## Decision Drivers

- The analytic SDF already knows the signed distance `d` and the rim
  parameter `u`; eliminating work must be a *mathematical cut of zero terms*,
  never an approximation.
- The backdrop capture must remain a strict representation of the desktop
  scene: reusing it as the output base is only valid when the capture covers
  the whole output, was refreshed this frame, and contains every layer the
  direct scene pass would have drawn.
- Physical-device selection for a direct-display host is a strict DRM identity
  contract (optics ADR-0049); a process-global preference must never override
  it.

## Considered Options

- Keep double-drawing and skip only the *wallpaper* on capture-reuse frames.
  Rejected: wallpaper is a fraction of the scene draw; clients and shadows
  dominate and would still be drawn twice.
- Gate the glass rim on a constant `d > -5.0` and the SDF taps on
  `u > 0.0 || d > -5.0`. Considered and fixed during review: two inconsistent
  gates create a band (`u == 0 && d > -5.0`) on small bodies
  (`edge < 5`, `body_min < ~14.3px`) where a fallback normal would drive a
  false bottom sheen.
- Honor `FLUX_GPU_PREFERENCE` on every selection pass. Rejected: on a
  DRM-pinned compositor path the env var is inert anyway (the node filter
  pre-selects one candidate), and ADR-0049 explicitly rejects process-global
  GPU policy for direct-display hosts.

## Decision Outcome

Chosen option: **single-pass full-output capture + capture-as-output-base** in
the compositor, **uniform `u > 0` gates** in the glass shader, and a
**DRM-scoped GPU preference** in flux.

### Compositor: capture covers output, so reuse it as the base

`CompositorRuntime::render_and_present` computes
`capture_covers_output_tentative` before the capture pass:

```text
capture_origin == (0,0) && capture_extent == physical_size && capture_size == physical_size
```

When true, the capture pass renders the full desktop scene **once** into the
backdrop target (a single full-frame render area instead of one scissored pass
per damaged region), including wallpaper, client scene, and — where the
shadow policy is Soft — the soft-shadow layer. After the effect stack runs,
`BackdropGraphExecutor::draw_capture_opaque` blits that capture as the output
base, and `begin_desktop_output` receives
`wallpaper_occluded || (capture_covers_output && refreshed)` so the wallpaper
is not drawn a second time into the commit target.

The reuse is strictly gated:

- `capture_covers_output && refreshed` — the capture was actually produced on
  this frame and covers the complete output;
- `draw_capture_opaque` fails (missing capture, mismatched allocation) → fall
  through to the normal base scene;
- the `Recompute` plan (source damage outside the capture) and the
  `Refresh(partial)` plan keep `capture_covers_output` conservative: only a
  true full-output refresh is eligible;
- overview, screenshot freeze, session lock, window switcher, and
  live previews each bypass or invalidate the path through the existing
  `backdrop_plan` selection, so the capture is never reused for a scene it did
  not reconstruct.

### Prism: the interior is exactly zero, so skip it

The liquid-glass shader now computes the SDF gradient and runs the whole
meniscus rim / contact-AO block only where `u > 0.0` — the true rim band. In
the flat interior (`u == 0`) the lens bend is exactly zero, so the
chromatic-dispersion samples collapse to a single direct backdrop read, and
the fallback `rim_normal` is provably never read (it feeds only lens-scaled
offsets and the `u > 0` rim block). One gate, not a distance constant: the cut
tracks the geometry, so small bodies never see a false-normal rim.

### Flux: GPU preference is a generic-app default

`FLUX_GPU_PREFERENCE=integrated|low-power` biases `score_device` only when no
DRM node is pinned (`drm_node == nullptr`). A pinned node is a strict identity
contract (optics ADR-0049); the env var cannot override it.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-88] [Full-Output Capture Reuse]`: The backdrop capture may be
  used as the output base only when `capture_origin == (0,0)`,
  `capture_extent == physical_size`, `capture_size == physical_size`, and the
  capture was refreshed on the current frame. Any other plan must fall back to
  the direct scene draw. *Enforced by* the golden hard invariants
  (tests/prism/integration/test_prism_golden.c) and the compositor fallback
  branch that leaves `draw_capture_opaque` unreached on non-eligible plans.
- `[INV-ARCH-89] [Glass Rim Gate]`: The meniscus rim / contact-AO block and
  the SDF-gradient taps must share the same gate `u > 0.0`; a fallback
  `rim_normal` must never be consumed by the rim block. *Enforced by* the
  `u > 0.0` literals in `liquid_glass.comp` (both branches) and the golden
  test's zero-outlier pixel gate.

## Rejected Alternatives & Negative Knowledge

### Inconsistent gates (`u > 0 || d > -5` taps vs `d > -5` rim)
- **Why considered**: they produced the same output for every body in the
  golden test (32×32). 
- **Why rejected**: on small bodies (`edge < 5`, `body_min < ~14.3px`) the band
  `d ∈ (-5, -edge]` has `u == 0` yet `d > -5`, so the fallback `(0,1)` normal
  would drive a flat bottom sheen of up to ~1.7% of `rim_light` — a visible
  regression the 32×32 golden cannot catch because it never enters that
  regime. Single-gate (`u > 0`) is strictly equivalent for large bodies and
  removes the degenerate band entirely.

### `FLUX_GPU_PREFERENCE` honored on DRM-pinned paths
- **Why considered**: uniform behavior across nested and direct-display hosts.
- **Why rejected**: on a pinned path the env var is a silent no-op or,
  worse, a contradiction of the typed `flux_device_drm_node_desc` contract
  (ADR-0049). Scoping it to `drm_node == nullptr` keeps the useful generic/
  nested knob and makes the strict-affinity contract undeniable.

## Consequences

### Positive
- Full-output Liquid Glass frames render the desktop scene once instead of
  twice; the compositor saves a full scene draw per affected frame.
- Glass interiors skip 4 SDF taps, 2 of 3 backdrop samples, and the entire
  specular rim / contact-AO block — the dominant per-pixel ALU/texture cost
  for the majority of glass pixels.
- Multi-GPU development hosts get a documented preference knob that cannot
  leak into direct-display correctness.

### Negative / Trade-offs
- `capture_covers_output` is a heuristic over several size/plan conditions;
  every new backdrop plan must re-check the eligibility predicate before
  reaching the reuse path (bounded by `[INV-ARCH-88]`).
- The glass interior now skips the rim entirely; any future feature that wants
  interior-edge effects (e.g., interior specular dust) must re-open the `u > 0`
  gate deliberately and re-run the golden.

## References

- [ADR-0179 — Zero-redundancy presentation pipeline](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md)
- [ADR-0180 — Single-pass chrome fades](0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md)
- [ADR-0181 — Canonical chrome prepass seam](0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md)
- Optics [ADR-0049 — Strict DRM identity](https://github.com/ming2k/optics/blob/main/docs/adr/0049-strict-drm-vulkan-device-selection.md)
- Optics [ADR-0063 — Liquid glass material library](https://github.com/ming2k/optics/blob/main/docs/adr/0063-liquid-glass-material-library.md)