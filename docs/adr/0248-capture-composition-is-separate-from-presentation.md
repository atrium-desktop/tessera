---
id: ADR-0248
title: "Capture Composition Is Separate from Per-Output Presentation"
status: accepted
date: 2026-11-18
scope: runtime/presentation, runtime/capture, platform/drm, docs
superseded_by: null
negative_knowledge: true
---

# 0248. Capture Composition Is Separate from Per-Output Presentation

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0185](0185-per-output-presentation-domains.md), [ADR-0247](0247-per-output-render-targets.md)

## Context

Screenshots, screenshot freeze, output streams, and window capture need a
desktop-wide image. The old union swapchain made that image the presentation
source, so starting a capture changed the presentation path and could change
pacing, damage, and color behavior.

## Decision

Capture composition is an offscreen consumer domain:

1. `Host::compose_only` submits the capture composition and retains its
   descriptor, acquire fence, and Flux lease.
2. The image is **not** scanned by any CRTC.
3. `present_union_to_targets` uploads the image into every physical output
   target and commits those targets 1:1.
4. Streams and screenshot readers consume the retained composition descriptor;
   they do not select a different presentation path.
5. Session-lock frames and an overlay-active fallback may use the same
   offscreen composition source, but presentation still goes through output
   targets whenever they are available.

Normal desktop, overview, window switcher, live previews, soft shadows, 3D
wallpaper, and workspace slides render per output target and do not enter the
capture composition.

## Invariants

- `[INV-ARCH-152]` A capture request cannot change the KMS presentation target
  set.
- `[INV-ARCH-153]` A capture composition image is never scanned as a union
  swapchain.
- `[INV-ARCH-154]` Capture consumption holds the composition lease for exactly
  as long as the capture path needs the image.

## Consequences

- Capture cadence and presentation cadence stay separate.
- A screenshot or stream cannot force a union KMS commit.
- The capture composition still has a union-sized intermediate; that is a
  capture cost, not a presentation architecture.
- Real hardware must validate simultaneous capture and independent target
  flips.
