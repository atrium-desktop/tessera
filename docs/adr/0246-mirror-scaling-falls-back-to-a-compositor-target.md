---
id: ADR-0246
title: "Mirrored Outputs Present Through a Compositor-Owned Mode-Sized Target"
status: accepted
date: 2026-11-18
scope: platform/drm, runtime/mirror_target, desktop/output, docs
superseded_by: null
negative_knowledge: true
---

# 0246. Mirrored Outputs Present Through a Compositor-Owned Mode-Sized Target

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0185](0185-per-output-presentation-domains.md), [ADR-0243](0243-output-roles-and-mirroring.md)
- Amended by: [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md) — *Fence-Accurate Client Buffer Release and Cached Partial-Damage Mirror Composition*

## Context

ADR-0243 mirrors by scanning the source rectangle directly on the target
CRTC. That is cheap when the primary plane can scale the source to the
mirror's native mode. Not every KMS primary plane can scale, and an
aspect-preserving fitted destination is a scaling request even when the two
panels have the same resolution but different aspect ratios. The atomic
commit then fails instead of producing a mirror frame.

Duplicating the scene into an extra union slice would grow the framebuffer,
render chrome twice, and still leave the union image as the scaling source.
It also would not solve input letterboxing or target-local lifecycle.

## Decision

A DRM mirror presents through one compositor-owned target at its native mode:

1. `sync_mirror_targets` creates one exportable Flux surface and canvas per
   mirror when the topology/mode changes.
2. On the normal path the source output's target is the source of truth. A
   capture/fallback frame may instead use the dedicated union composition. The
   target imports that finished image with its producer fence and lease.
3. A target pass clears to black and samples the source image into the fitted
   content rectangle. Chrome, overlays, workspace transitions, and captures are
   already in that image; no second scene render and no second shell layout are
   needed.
4. The target is exported with its acquire fence and committed to the mirror
   CRTC with `source_size = mode` and `destination = (0, 0, mode)`. The KMS
   plane performs no scaling.
5. The source and mirror KMS slots are independent. The source ring lease is
   retained by the mirror target until its next successful target frame.
6. The union image is still presented to extended outputs and is still the
   stream/screenshot source. Mirror targets are not capture surfaces.

The union path remains a fallback if target creation or presentation fails; a
failure removes the target and records the mode so allocation is not retried
every frame. Frame callbacks and presentation feedback remain scoped to the
source output because a mirror owns no workspace or window.

## Invariants

- `[INV-ARCH-148]` A mirror target always commits a mode-sized framebuffer
  with a full-CRTC destination; no KMS plane scaling is requested for it.
- `[INV-ARCH-149]` The union image's lease outlives every mirror target blit
  that samples it.
- `[INV-ARCH-150]` A target failure returns the mirror to the union path
  rather than freezing the last target image.
- `[INV-ARCH-151]` Mirror targets use a separate framebuffer-cache epoch from
  the union surface; overlapping Flux slot numbers cannot invalidate the
  union cache.

## Consequences

- Mirroring no longer depends on KMS plane scaling, including letterboxed
  aspect mismatches.
- The source and mirror can retire page flips independently.
- The cost is one GPU blit and one exported target ring per mirror. That is
  bounded by the number of mirrors, not by output resolution duplication of
  the scene.
- This is **not** full per-output rendering (ADR-0247): extended outputs still
  share the union image, chrome is still composed once for the primary, and a
  slow extended output can still bound the union ring.
- Hardware validation still must prove the target commit, source lease
  lifetime, mixed refresh, cursor visibility, and hotplug on two real outputs.
