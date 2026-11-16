---
id: ADR-0234
title: "Output Layers Are Plane-Allocation Contracts, Not Scene Layers"
status: accepted
date: 2026-11-13
scope: platform/drm, composite/scanout, runtime/presentation, docs/reference
superseded_by: null
negative_knowledge: true
---

# 0234. Output Layers Are Plane-Allocation Contracts, Not Scene Layers

- Status: Accepted
- Date: 2026-11-13
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-platform` (DRM plane discovery/allocation/commit), `crates/tessera-composite` (scanout policy), `crates/tessera` (presentation pass order)
- Amends: [ADR-0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md) (plane allocation vocabulary), [ADR-0061](0061-window-tree-atomic-client-surface-compositing.md) (scene stacking remains authoritative), [ADR-0180](0180-single-pass-chrome-fades-backdrop-allocation-decoupling-and-dock-prepass-coherence.md), [ADR-0226](0226-single-pass-chrome-composition-preview-layering-and-modal-dock-inhibition.md) (single-pass composition remains authoritative)
- Amended by: [ADR-0235](0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md) — the first overlay candidate policy: one topmost, single-surface, opaque, identity-mapped window per output; [ADR-0237](0237-a-direct-scanned-primary-excludes-overlay-offload.md) — a direct-scanned Primary excludes overlay offload, so at most one bypass layer exists per output per frame.

---

## Context and Problem Statement

The codebase used “layer” for four different things:

1. a **scene layer** — a z-order/damage group painted into one framebuffer
   (wallpaper, client surface tree, chrome, protocol overlays, software cursor);
2. an **output layer** — an independently scanned-out source with its own
   buffer and update path (primary, cursor, overlay);
3. a **KMS plane** — the kernel object that binds an output layer to a CRTC;
4. an **offscreen image** — a render target used for effects, captures, or
   streams (backdrop, screenshot freeze, shadow mask, preview).

Conflating them produced recurring wrong questions: whether the wallpaper
should get a “background plane”, whether always-on-top should select an
“overlay plane”, and whether the cursor should be a generic overlay. Each
answer would have turned a scene/stacking concept into a hardware allocation
policy.

The current implementation is intentionally conservative: exactly one primary
plane per active CRTC, at most one cursor plane per CRTC, and overlay planes
inventoried but never allocated. That policy is correct, but it was spread
across `DisplaySet`, `Output`, `OverlayPlaneInventory`, and ad-hoc cursor
matching, so the contract was implicit and easy to erode.

## Decision Drivers

- KMS plane roles are hardware capabilities, not z-order bands.
- Cursor motion must stay off the compositor’s repaint path.
- Direct scanout correctness depends on an explicit primary-plane source.
- Overlay allocation without complete validation produces wrong pixels.
- Damage-driven composition is the first constraint; per-node offscreen
  layers and speculative plane allocation are both rejected.
- Terminology must make the wrong design hard to express.

## Decision Outcome

### 1. Four concepts, four names

| Concept | Owns | Never means |
|---|---|---|
| **Scene layer** | z-order, damage, effects, hit-testing | a KMS plane |
| **Output layer** | an independent scanout source and its update contract | a z-order group |
| **KMS plane** | the kernel object bound to a CRTC | compositor stacking policy |
| **Offscreen image** | a reusable render target | an output layer |

The compositor continues to use one authoritative scene order and
immediate-mode painting. It does not grow a retained scene graph or a
“canvas layer” tree. `WorkspaceSurfaceLayer` and `SoftShadowLayer` remain draw
batches inside one Canvas pass, not independent canvases.

### 2. Output layer kinds are closed

The only output-layer kinds are:

- **Primary** — exactly one per active CRTC. Carries either the compositor
  framebuffer or one eligible client dma-buf; the sources are mutually
  exclusive within an atomic commit.
- **Cursor** — at most one per active CRTC. Backed by a cursor plane when the
  hardware exposes a compatible ARGB8888 linear plane; otherwise the cursor is
  software-composited and direct scanout is blocked while it is visible.
- **Overlay** — zero or more per active CRTC, but only through a validated
  planner. No overlay is allocated by default.

There is deliberately no `Background` and no `AlwaysOnTop` output-layer kind.
The wallpaper is the first scene layer painted into Primary. Always-on-top is
`Window::always_on_top` and the `restack_always_on_top_band()` scene ordering;
it never selects a plane.

### 3. Cursor is a cursor output layer, not a generic overlay

The cursor needs asynchronous position updates without repainting the primary
framebuffer, hotspot semantics, and a fixed-on-top presentation contract. A
generic overlay plane does not provide that update contract and may be
rejected or require a full plane-set commit. A cursor therefore uses a
`Cursor` output layer and, when available, a `Cursor` KMS plane. It must never
be reclassified as an `Overlay` candidate merely because both are “above
Primary” in z-order.

### 4. Overlay is a proof obligation, not a best-effort allocation

Overlay offload remains disabled until a planner exists that can submit the
complete plane set for one atomic `TEST_ONLY` request and validate:

- z-order (`zpos`) and plane type;
- format, modifier, scaling, rotation, and source/destination rectangles;
- alpha/pixel-blend mode and opaque-region coverage;
- explicit synchronization (`IN_FENCE_FD`), damage hints, and color pipeline;
- every CRTC in the presentation domain.

A rejected candidate must fall back to composition. There is no partial
scanout and no “allocate the plane and hope” path. The first eligible
candidates are expected to be fullscreen video/zero-copy client buffers and
single opaque windows; chrome, notifications, menus, shadows, wallpaper, and
always-on-top are not candidates.

### 5. Backend ownership

`crates/tessera-platform/src/drm/planes.rs` owns plane discovery,
classification, cursor matching, and the overlay allocation record. `Output`
owns one `OutputPlanes` value; no module outside the DRM backend constructs
plane assignments.
`DisplaySet` no longer carries a display-wide overlay counter: overlay
availability is per-CRTC and travels with the output whose CRTC it can drive.

## Invariants & Behavioral Boundaries

- `[INV-ARCH-122] One Primary Output Layer per Active CRTC.` Every selected
  output has exactly one primary plane. A commit selects exactly one source
  for it: the compositor framebuffer or one client dma-buf. **Enforcement:**
  `OutputPlanes.primary` is non-optional, `PrimaryPlaneSource` is the only
  source switch, and `test_inv_arch_122_primary_source_and_lifetime_are_one_contract`
  rejects a mismatched source/lifetime pair.
- `[INV-ARCH-123] Cursor Is a Cursor Output Layer.` At most one cursor output
  layer exists per CRTC, it commits through the cursor plane path, and its
  absence degrades to software composition rather than to a generic overlay.
  **Enforcement:** `OutputPlanes.cursor` is a single `Option<CursorPlane>`;
  `assign_optional_planes` is a one-to-one maximum matching, covered by
  `test_inv_arch_123_cursor_plane_matching_is_one_to_one`.
- `[INV-ARCH-124] Scene Semantics Never Allocate Planes.` Background/wallpaper,
  always-on-top, chrome, notifications, menus, shadows, and protocol overlays
  are scene concepts. They may be composited into Primary but must not appear
  in `OutputPlanes` or `PlaneInventory`. **Enforcement:** `PlaneRole` is a
  closed enum with only `Primary`, `Cursor`, and `Overlay`; `OutputPlanes` is
  private to the DRM backend module tree and no public API accepts a plane
  role, so no scene concept can name one.
- `[INV-ARCH-125] Overlay Requires Complete-Set TEST_ONLY.` No overlay plane
  may be allocated without validating the complete plane set in one atomic
  `TEST_ONLY` request. Rejection means composition, never partial scanout.
  **Enforcement:** `import_overlay_scanout` requires an enabled
  `OverlayPlanePolicy`; `commit_scanout` runs the `TEST_ONLY` preflight for
  every overlay transition; the KMS system test exercises the kernel proof.

## Rejected Alternatives & Negative Knowledge

### A fixed three-layer stack: primary, cursor, overlay

- **Why considered**: It is a convenient mental model and matches the three
  KMS plane types.
- **Why rejected**: Plane counts are not fixed. A CRTC may have no cursor
  plane, several overlay planes, or overlays usable as underlays. Modelling
  them as three ordered layers would encode a stack the hardware does not
  guarantee and would invite z-order semantics to leak into plane selection.

### A background/wallpaper plane

- **Why considered**: The wallpaper is static and often full-output, so a
  dedicated plane looks like free offload.
- **Why rejected**: Primary is still required for windows and chrome, so the
  wallpaper plane does not remove composition work. Wallpaper effects,
  scaling, and color handling do not fit plane constraints, and underlay
  `zpos` support is not portable. The real optimization is reusing the
  composed Primary framebuffer while the scene is idle, not adding a plane.

### Always-on-top selects an overlay plane

- **Why considered**: Both are “above” normal content.
- **Why rejected**: Always-on-top is a stacking policy, not a scanout source
  property. A topmost window may be fully composited into Primary; an overlay
  plane may carry content below Primary (underlay). Plane assignment must
  preserve scene z-order, not define it.

### Cursor as a generic overlay

- **Why considered**: Both are planes above Primary.
- **Why rejected**: Cursor planes have asynchronous position updates, hotspot
  properties, size hints, and a fixed-on-top contract. Generic overlays do not
  provide that update path and may be rejected as part of a full plane-set
  commit. Treating the cursor as an overlay would either serialize pointer
  motion behind composition or silently lose the cursor.

### Allocating every compatible surface to an overlay plane

- **Why considered**: Format support alone appears sufficient.
- **Why rejected**: Format support does not prove stacking, blending, color,
  scaling, damage, or fence compatibility. False-positive allocation produces
  incorrect pixels; composition is the correct fallback.

### A general canvas-layer tree or per-node offscreen passes

- **Why considered**: Layers make effects and z-order easy to reason about.
- **Why rejected**: ADR-0180 recorded per-node `save_layer` exploding into
  hundreds of full-screen offscreen passes; ADR-0226 and ADR-0180 established
  single-pass desktop/chrome composition. Damage-driven composition remains
  the first constraint, so offscreen images exist only where a consumer needs
  to sample or reuse them (backdrop, capture, shadows, streams).

## Consequences

### Positive

- Plane allocation is a named, testable subsystem instead of an implicit
  property of `DisplaySet`/`Output`.
- Background and always-on-top can no longer be mistaken for plane roles
  without violating a documented invariant.
- Cursor fast-path and software fallback keep one explicit contract.
- Overlay policy states exactly what must be proven before offload is enabled.
- The terminology matches the hardware: “plane” in KMS/backend code, “output
  layer” in compositor architecture, “scene layer” for z-order/damage.

### Negative / Trade-offs

- Overlay hardware remains unused until the complete-set planner is
  implemented. This trades potential bandwidth savings for deterministic
  output and is deliberate.
- `OutputPlanes` adds one indirection to every plane access; the alternative
  is scattering plane fields across `Output` again.
- The ADR does not implement overlay offload. It defines the contract that
  implementation must satisfy; a future ADR must record the planner’s
  candidate policy and measured constraints.

## Implementation Notes

- `crates/tessera-platform/src/drm/planes.rs` owns `PlaneRole`,
  `PlaneCapabilities`, `PlaneInventory`, `OutputPlanes`,
  `OverlayPlaneAllocation`, cursor matching, and plane-role discovery.
- `crates/tessera-platform/src/drm.rs` exposes `Output.planes` and the
  primary/cursor commit paths; it no longer stores plane roles directly.
  `PrimaryScanout`, `PrimaryPlaneSource`, and `PrimaryScanoutOwnership` name
  the primary-plane buffer, its semantic source, and its lifetime contract,
  rather than leaving them as an unqualified `Scanout`.
- `crates/tessera-platform/src/drm/output.rs` builds outputs, resolves
  per-CRTC overlay inventory, and computes the display signature.
- `crates/tessera-platform/src/drm/device.rs` commits primary/cursor state and
  reports plane allocation telemetry.
- Unit tests cover constrained cursor matching and the disabled overlay
  allocation. Hardware `TEST_ONLY` coverage belongs to the future planner.
