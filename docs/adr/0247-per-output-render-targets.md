---
id: ADR-0247
title: "Per-Output Render Targets Replace the Union Swapchain"
status: accepted
date: 2026-11-18
scope: runtime/presentation, composite, platform/drm, docs
superseded_by: null
negative_knowledge: true
---

# 0247. Per-Output Render Targets Replace the Union Swapchain

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0185](0185-per-output-presentation-domains.md)
- Amended by: [ADR-0248](0248-capture-composition-is-separate-from-presentation.md) — *Capture Composition Is Separate from Per-Output Presentation*
- Amended by: [ADR-0249](0249-presentation-groups-and-per-output-pacing.md) — *Presentation Groups Synchronize Shared Content Without Reintroducing a Union Swapchain*
- Amended by: [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md) — *Fence-Accurate Client Buffer Release and Cached Partial-Damage Mirror Composition*
- Amended by: [ADR-0258](0258-client-cursor-hardware-plane-offload-and-per-output-overlay-parity.md) — per-output DRM rendering draws non-offloaded client cursor overlays and offloads eligible client cursors onto the KMS hardware cursor plane.
- Amended by: [ADR-0268](0268-per-output-overlay-offload-and-overlay-only-commits.md) — per-output render targets natively support KMS overlay offload and zero-GPU overlay-only commits.

## Context

The desktop is currently rendered into one union framebuffer and each CRTC
scans a sub-rectangle. Per-output damage acknowledgement and page-flip state
are already independent (ADR-0185), but image lifetime is not: a slow CRTC can
pin a union ring slot and bound the compositor to the slowest output. A
mirror that needs compositor scaling (ADR-0246) also wants a native-mode target
rather than a scaled union slice.

## Decision (proposed)

Each physical output owns an independent render target:

- target extent equals its mode (one Flux exportable surface per output);
- the scene pass uses an output-local transform (`translate` by the output's
  logical origin, `scale` by its scale) and clips to the target;
- each target has its own damage ledger, page-flip/slot state, completion
  fence, and callback/feedback scope;
- direct client scanout still bypasses the target for that output;
- the union framebuffer remains only as a compatibility fallback for nested
  hosts and for capture paths that explicitly request the composed desktop.

A mirror target renders its source output's complete scene (including chrome
and overlays) at the mirror's fitted destination; an extended target renders
only its own workspace region. Targets are created/destroyed with topology,
mode, scale, transform, and color-pipeline changes. Composition ownership is
per target, so there is no shared ring slot across outputs.

## Current implementation

The normal DRM path now creates one `OutputTarget` per physical output. An
extended target renders the local scene into its mode-sized surface; a mirror
target samples its source target and fits it into the content rectangle. Each
target has its own Flux ring, target-local damage (full for the first complete
ring cycle), KMS slot, page flip, and cursor handling. Direct scanout still
bypasses targets.

Capture, screenshot freeze, active streams, session lock, and an overlay-active
fallback use one dedicated union composition as their source. That image is
uploaded into every output target and presented through the target's own KMS
slot; it is not scanned as a union swapchain. Overview, window switcher, live
previews, soft shadows, 3D wallpaper, workspace slides, and ordinary desktop
frames render each extended target locally and blit mirror targets from their
source target. The union image is therefore a shared capture/security
composition, not the presentation swapchain.

## Migration gates

1. A target planner pure-tests output-local transforms and damage mapping.
2. A DRM target commit/test proves no plane scaling and independent flips.
3. ScreenCast/screenshot paths select an explicit target instead of assuming
   one presented union image.
4. Nested mode keeps the single host surface until the nested WSI backend can
   own multiple surfaces.
5. Real hardware validates mixed refresh (165 + 60 Hz), mixed scale, HDR/SDR,
   hotplug, suspend/resume, and a single slow output without blocking the
   others.

ADR-0246 now creates a mode-sized compositor target for each DRM mirror and
blits the union image into it. That removes mirror plane scaling but still
uses one union composition and one union ring; it is not complete per-output
presentation.

This ADR is accepted as the presentation architecture. Real two-output KMS
hardware still has to validate independent flips, mixed refresh, target-local
damage, capture handoff, cursor behavior, and hotplug; that is a hardware
evidence gate, not a reason for the normal path to keep a union scanout.
