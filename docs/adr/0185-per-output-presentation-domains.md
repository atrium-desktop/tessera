---
id: ADR-0185
title: "Per-Output Presentation Domains"
status: draft
date: 2026-10-20
scope: platform/drm, composite/presentation, runtime/presentation, wayland/server
superseded_by: null
negative_knowledge: true
---

# 0185. Per-Output Presentation Domains

- Status: Draft (proposal under deliberation; not implemented)
- Date: 2026-10-20
- Deciders: Tessera Maintainers
- Amends (on acceptance): [ADR-0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md), [ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md), [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)

---

## Context and Problem Statement

The DRM backend presents the desktop as **one presentation domain**: a single
framebuffer spanning the union of every active output, laid out left to right,
committed to every CRTC in one atomic batch.

```rust
// crates/tessera-platform/src/drm/output.rs
desktop_width += size.0;
desktop_height = desktop_height.max(size.1);
// ...
x += size.0;
// ...
DisplaySet { outputs, size: (desktop_width, desktop_height), /* … */ }
```

```rust
// crates/tessera-platform/src/drm/device.rs
self.pending_flips = self.displays.outputs.iter().map(|o| o.crtc).collect();
```

`presentation_pending()` is "any CRTC still pending", and the redraw scheduler
may not start the next frame until the whole batch retires. Three consequences
follow that the existing direct-scanout work cannot route around:

1. **Direct scanout is unreachable with more than one output.**
   `plan_primary_plane` passes `physical_size = surface.size()` — the union — to
   `evaluate_surface`, which requires `buffer == physical_size` and
   `position == (0, 0)`. A game covering one output has a buffer the size of
   *that* mode at *that* origin. With a second output attached, every covering
   client is rejected `geometry-mismatch` (or `non-zero-origin`), so it is
   composited forever. On a laptop with an external display this is the normal
   configuration, not an edge case.
2. **The slowest output paces every output.** One atomic batch flips all CRTCs,
   and the domain is released only when the last one retires. A 165 Hz primary
   beside a 60 Hz secondary is capped at 60 Hz, for the game and for the
   desktop. `atomic_domain_interval` already takes the minimum refresh, which
   documents the same coupling.
3. **The composite cost scales with the union.** A frame caused by one output
   clears and renders the entire desktop, and `FB_DAMAGE_CLIPS` saves KMS
   bandwidth but not GPU writes.
4. **Logical layout is not modelled per output.** The backend reports each
   output's `logical_origin` as its accumulated *physical* x offset
   (`crates/tessera-platform/src/drm.rs`, `output_infos`), and the composition
   root derives the desktop's logical extent from
   `output_infos().first()` (`crates/tessera/src/runtime/presentation.rs`,
   `crates/tessera/src/runtime/config.rs`, `crates/tessera/src/runtime/iteration.rs`).
   A 2x-scaled output therefore starts at twice the logical coordinate of its
   neighbour, and chrome lays itself out over the first output's extent only.
5. **Occlusion still uses the fullscreen bit as a coverage proxy.**
   `Server::occluded_window_ids_with`
   (`crates/tessera-wayland/src/server/scene.rs`) pushes the *primary output's*
   logical rect as opaque coverage for any window with
   `state.fullscreen`, so a fullscreen window on a secondary output claims
   coverage over the primary one. The geometric pass immediately below it
   already derives the same answer from formats and declared opaque regions.

ADR-0182 made the *single*-output zero-cost path genuinely zero-cost.
Multi-output reaches none of it. Items 4 and 5 are recorded here rather than
fixed: both change what the compositor draws on a multi-output desktop, and
neither can be validated without a physical two-output KMS session.

## Decision Drivers

- **A client owns one output's plane, not the desktop's framebuffer.** Plane
  assignment and flip retirement are KMS concepts per CRTC; the current model
  collapses them for implementation convenience.
- **A frame should present only what changed.** An output with no new content
  must not be flipped, and an output with a client-owned plane must not be
  re-composited because a different output animated.
- **Framebuffer lifetime must be reference-counted per plane.** The existing
  `current`/`retiring` pair is sound only because all CRTCs flip together; a
  partial commit destroys that argument.
- **No visual regression for the single-output case**, which is the majority.

## Decision Outcome (proposed)

### 1. Per-output plane slots with reference-counted framebuffers

Replace `current: Option<Scanout>` / `retiring: Option<Scanout>` with one slot
per output:

```rust
struct PlaneSlot {
    /// Framebuffer currently programmed on this CRTC's primary plane.
    displayed: Option<Rc<Scanout>>,
    /// A flip was committed and its page-flip event has not arrived.
    pending: bool,
}
```

A plane assignment holds a reference. This is the load-bearing change: the
composite framebuffer can outlive the frame that replaced it on one CRTC
because another CRTC still references it, and the cache's `users` count becomes
`Rc::strong_count`-driven instead of hand-maintained.

### 2. Presentation targets are explicit

Both presentation entry points take the outputs they present to:

- `present(surface, frame, damage, targets)` — the composite framebuffer is
  programmed on `targets`.
- `present_scanout(candidate, target, damage)` — one CRTC receives the client
  buffer; the others keep whatever `displayed` already holds.

`Rc::ptr_eq` on the composite `Scanout` lets a frame reuse the already-imported
compositor framebuffer for outputs whose content did not change, so a
client-owned output costs the compositor no GPU work even while other outputs
are re-presented.

### 3. Flip gating is per output

`presentation_pending()` becomes `any(slot.pending)`, and the redraw scheduler
gains the notion of a presentation target: a frame whose only pending work is a
direct scanout may proceed while the composite domain is idle, and a composite
frame may start while a client flip is in flight on another CRTC. The scheduler
keeps its single-frame-at-a-time rule *per domain*, not per host.

### 4. Scanout eligibility is evaluated against the candidate's output

`plan_scanout` takes the target output's `(logical_origin, mode_extent,
logical_extent)` and requires the surface to cover exactly that output. The
union desktop remains the composite canvas; it stops being the scanout
yardstick.

### 5. The composited canvas stays a single union target (explicitly)

Chrome is rendered by one lens context into one canvas
(`Shell`/`Ui`), and the color pipeline is negotiated per session. Per-output
render targets — the design that would also give per-output scale and transform
— require lens multi-target rendering and per-output color policy, and are the
subject of a separate decision. This ADR must not be implemented as a partial
version of that one.

### Invariants & Behavioral Boundaries (to be established on acceptance)

These are the red-lines the implementation must prove; none is declared as an
`INV-` tag yet, because the enforcement mechanism does not exist until the code
lands ([Invariant Governance](../dev/standards/invariants.md) §3).

- A framebuffer referenced by a plane slot must not be destroyed, regardless of
  cache reuse flags, until that slot is reprogrammed or the CRTC is disabled.
  To be enforced by `Rc` ownership plus a regression test that invalidates the
  cache while a partially-committed batch is in flight.
- A commit names every CRTC whose plane state it changes and no others; a
  `pending` slot must be cleared only by its own page-flip event.
- The single-output path must be observationally identical: one slot, one
  target, one flip per frame.

## Verification (required before acceptance)

- Unit: plane-slot reference counting across interleaved composite and scanout
  commits; a cache invalidation while one CRTC still references the old
  framebuffer must not destroy it.
- Unit: `plan_scanout` against per-output targets, including a candidate on the
  second output and a candidate spanning two outputs (rejected).
- Hardware ([VT and DRM Testing](../dev/vt-drm-testing.md)), 165 Hz + 60 Hz:
  a full-output game on the fast output logs `direct scanout active`, its
  presented frame rate follows the fast output's vblank, and the slow output
  keeps a static frame without flipping.

## Consequences

### Positive
- Direct scanout becomes reachable in the multi-output configuration that most
  laptops and desktops actually run.
- Each output retires at its own vblank, so a slow secondary output stops
  capping a fast primary.
- Framebuffer lifetime becomes structurally safe under partial commits instead
  of relying on "all CRTCs flip together".

### Negative / Trade-offs
- The KMS presentation path grows a per-output state machine; `present`,
  `present_scanout`, `commit_scanout`, `disable_outputs`, `Drop`, and the
  page-flip handler all change together, and this is code that cannot be
  exercised without a physical DRM session.
- Two commits may be needed in one iteration when a client owns one output and
  another output has damage; the loop must not double-render the canvas to do
  it.
- The composite canvas is still the union: memory and fill cost for composite
  frames do not improve until per-output render targets land.

## Rejected Alternatives & Negative Knowledge

### 1. Mixed single-batch commit (client plane + composite plane in one batch)
- **Why considered**: Keeps one atomic batch and the `current`/`retiring`
  lifetime argument; fixes only the eligibility half.
- **Why rejected as the endpoint**: It leaves the batch paced by the slowest
  CRTC, which is the second defect. It is a reasonable *first step* only if it
  also introduces the per-plane reference counting from §1, because a mixed
  batch already references two different framebuffers.

### 2. Keep the union framebuffer and scale a covering client onto its output
- **Why considered**: No plane bookkeeping at all: compose everything.
- **Why rejected**: That is the status quo this ADR exists to replace; it pays
  a full composite for a frame the primary plane could have carried for free.

### 3. One render target per output now
- **Why considered**: The fully correct end state: per-output scale, transform,
  color policy, and scanout geometry fall out of it.
- **Why rejected for this decision**: Chrome renders through one lens `Ui`
  bound to one canvas, and `DisplayColorMode` is negotiated once per session
  across all outputs. Introducing per-output canvases is a lens capability
  decision plus a color-policy decision; smuggling half of it in here would
  create two half-models of the same concept.

### 4. Fix the logical-layout and occlusion defects ahead of this decision
- **Why considered**: Both are small, local edits, and both are wrong today.
- **Why rejected as a precursor**: Changing logical origins repositions every
  client on a multi-output desktop and changing the occlusion proxy changes
  what is drawn under a fullscreen window; each is exactly the kind of change
  that must be validated on hardware, and both land naturally once plane
  assignment and per-output targets exist. Recording them here keeps them from
  being rediscovered as new bugs.

### 5. Mirror instead of extend when outputs disagree
- **Why considered**: A 60 Hz output beside a 165 Hz output could mirror the
  fast one and drop frames on the slow one.
- **Why rejected**: Mirroring changes what the user sees on the second screen
  to solve a pacing problem. Each output must present its own content at its
  own rate.
