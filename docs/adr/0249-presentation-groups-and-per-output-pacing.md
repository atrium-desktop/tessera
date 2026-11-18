---
id: ADR-0249
title: "Presentation Groups Synchronize Shared Content Without Reintroducing a Union Swapchain"
status: accepted
date: 2026-11-18
scope: desktop/output, runtime/presentation, platform/drm, wayland/server, docs
superseded_by: null
negative_knowledge: true
---

# 0249. Presentation Groups Synchronize Shared Content Without Reintroducing a Union Swapchain

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0185](0185-per-output-presentation-domains.md), [ADR-0247](0247-per-output-render-targets.md)

## Context

Per-output targets and page flips remove the union ring and let fast outputs
advance. They also introduce a coherence question: a window spanning two
adjacent outputs can tear at the seam if one CRTC flips a frame before the
other, and two outputs with different refresh rates cannot share one clock.

## Decision

Targets remain independent, but physical outputs are grouped into a
**presentation group** when they must present the same scene generation:

1. Mirrors join their source group.
2. Touching outputs with the same refresh join one group.
3. A visible window spanning multiple groups merges those groups for that
   scene generation, so mixed-refresh neighbors temporarily present at the
   slower group's cadence instead of tearing.
4. A group is committed as one ATOMIC KMS request with one target per CRTC.
   Groups remain independent of one another.
5. A group whose member slot is still pending is skipped as a whole; other
   groups can continue. No union framebuffer or union ring is involved.
6. The frame loop paces at the fastest group. Target flips are independent, so
   the scheduler uses the fastest group's estimated vblank rather than waiting
   for every CRTC batch to retire.
7. `wp_presentation_feedback` uses each surface's target output refresh rate,
   not the session-wide slowest rate.

Grouping is recomputed when the output topology, window set, or window geometry
changes. A moved spanning window therefore re-merges and later splits groups
without a mode set.

## Invariants

- `[INV-ARCH-155]` A group commit carries every member CRTC in one atomic
  request; a group is never presented as separate uncoordinated flips.
- `[INV-ARCH-156]` Independent groups may advance at different rates; the
  compositor never waits for a blocked group before rendering a ready group.
- `[INV-ARCH-157]` A spanning window merges the groups it touches; a mixed
  refresh pair therefore accepts the slower cadence while coherent.
- `[INV-ARCH-158]` Presentation feedback refresh is per target output.

## Consequences

- A 165 Hz output can advance while a disjoint 60 Hz group is still retiring.
- A window spanning mixed-refresh outputs does not tear; it temporarily runs
  at the slower group's cadence.
- Groups are scheduling policy, not shared images: targets, rings, damage,
  leases, and cursor planes remain per output.
- Direct client scanout still owns its CRTC independently; a client buffer
  cannot span two outputs because direct eligibility is per physical output.
- Real two-output KMS hardware must still validate atomic group commits,
  re-grouping while a window moves across the seam, mixed-refresh pacing, and
  capture feedback timestamps.
