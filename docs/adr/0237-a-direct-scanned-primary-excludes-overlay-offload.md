---
id: ADR-0237
title: "A Direct-Scanned Primary Excludes Overlay Offload: One Bypass Layer per Frame"
status: accepted
date: 2026-11-14
scope: runtime/presentation, composite/presentation, platform/drm
superseded_by: null
negative_knowledge: true
---

# 0237. A Direct-Scanned Primary Excludes Overlay Offload: One Bypass Layer per Frame

- Status: Accepted
- Date: 2026-11-14
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera` (frame orchestration), `crates/tessera-composite` (primary/overlay planners), `crates/tessera-platform` (KMS commit)
- Amends: [ADR-0234](0234-output-layers-are-plane-allocation-contracts-not-scene-layers.md), [ADR-0235](0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md)

---

## Context and Problem Statement

Two independent output-layer paths can remove client pixels from the compositor
pass:

1. **Direct primary scanout** puts one full-output client dma-buf on the primary
   plane. The compositor framebuffer is not on screen, so there is no scene
   underneath the client.
2. **Overlay offload** puts one topmost opaque client on an overlay plane above
   a composited Primary.

Both are bypasses, and both are useful, but they are not additive. If the
runtime planned an overlay while a direct-scanned client owned Primary, it could
program the same surface twice (once as Primary, once as overlay), or place a
second client above an output that has no compositor framebuffer behind it.
Direct scanout also requires a chrome-free, single-client scene, which already
contradicts having another visible topmost window to offload.

The runtime encoded this as `primary.rejection().is_some()` before evaluating
overlay, but the rule was implicit, unstated in the ADR set, and unprotected
against a future orchestration change. The backend independently rejected
overlay commits whose primary source was `DirectClient`, but that guard was
also undocumented.

## Decision Drivers

- A direct-scanned Primary has no compositor framebuffer behind an overlay.
- A direct-scanout scene already contains exactly one visible client surface.
- Programming the same client buffer into two planes is a correctness and
  lifetime hazard.
- The fallback chain must be deterministic: direct primary, then one overlay,
  then full composition.
- The backend must reject an invalid combination even if the runtime regresses.

## Decision Outcome

### 1. One bypass layer per output per frame

For each output and frame, at most one client output layer bypasses
composition:

- **Direct primary** — one client dma-buf owns Primary, or
- **Overlay** — one client dma-buf owns an overlay plane above a composited
  Primary.

The two modes never coexist. The third mode, full composition, has no bypass
layer. The cursor plane is compositor-owned and is not a client bypass layer;
it may coexist with either mode under its own output-layer contract.

### 2. Direct primary wins

`plan_output_layers` evaluates overlay **only after** the primary plan is
rejected. A primary plan with a direct candidate is final for that output: the
overlay planner does not run, no overlay placement is produced, and no overlay
plane is allocated.

This is not merely an optimization. Running the overlay planner on a
direct-scanout frame would make the same topmost client the candidate, because
it is the only remaining client window, and would duplicate it.

### 3. Backend enforcement

`DrmBackend::commit_scanout` accepts an overlay only with
`PrimaryPlaneSource::Composited`. A `DirectClient` source plus an overlay
request is rejected as `DrmError::ScanoutUnsupported`; the caller must fall back
to a composited frame.

`Host::present_with_overlay` inherits that rule. There is no backend entry point
that presents a direct client and an overlay in one commit.

### 4. Deterministic fallback chain

The presentation decision is ordered:

1. Try direct primary scanout.
2. If Primary must be composited, evaluate at most one overlay candidate.
3. If the overlay is rejected or its proof fails, composite the complete scene.

A failure at one step never promotes the next step into a mixed mode.

### 5. No duplicate surface ownership

A surface already selected for direct primary scanout cannot be an overlay
candidate in the same frame. The gate in `plan_output_layers` guarantees this
because the overlay planner never sees an accepted direct primary plan.

## Invariants & Behavioral Boundaries

- `[INV-ARCH-134] One Bypass Layer per Output per Frame.` A frame may contain a
  direct-scanned Primary or one overlay layer, never both. **Enforcement:**
  `overlay_plan_permitted` in `tessera-composite` gates
  `plan_output_layers`; `test_inv_arch_134_direct_primary_excludes_overlay`
  constructs both an accepted direct plan and a rejected primary plan.
- `[INV-ARCH-135] Overlay Requires Composited Primary.` The backend rejects an
  overlay request when the primary source is `DirectClient`. **Enforcement:**
  `overlay_source_supported` at the `commit_scanout` boundary;
  `test_inv_arch_135_overlay_requires_composited_primary`.

## Rejected Alternatives & Negative Knowledge

### Evaluate overlay anyway and let the planner reject the duplicate

- **Why considered**: One less branch in the runtime.
- **Why rejected**: It makes the same topmost client eligible twice, produces
  misleading audit telemetry, and relies on policy coincidence instead of an
  explicit ownership rule. It also risks programming one buffer into two
  planes if a future planner accepted it.

### Allow direct Primary plus an overlay for another window

- **Why considered**: Both paths reduce composition work.
- **Why rejected**: Direct scanout requires the client to own the complete
  output with no compositor framebuffer underneath. An overlay above it has no
  backing for the rest of the scene and contradicts the single-client
  precondition. The correct result is composition plus at most one overlay.

### Prefer overlay over direct scanout

- **Why considered**: Overlay keeps the compositor framebuffer available for
  chrome and other windows.
- **Why rejected**: When a full-output, chrome-free, single-client scene exists,
  direct primary scanout is the stronger zero-copy path and avoids a
  compositor pass entirely. Overlay is the fallback for composited desktops,
  not a replacement for direct scanout.

### Let the backend silently drop the overlay

- **Why considered**: The backend already validates plane sets.
- **Why rejected**: Silent dropping would leave the runtime believing an
  overlay is active, with stale placement state and wrong Primary damage
  accounting. The invalid combination must be rejected at the API boundary.

## Consequences

### Positive

- The presentation precedence is explicit and testable.
- The same client can never be programmed into two output layers.
- A direct-scanout frame has one owner and one buffer-release contract.
- The backend has a defense-in-depth guard independent of runtime policy.
- Overlay audit telemetry no longer describes impossible direct-scanout
  candidates.

### Negative / Trade-offs

- A fullscreen direct-scanned client cannot benefit from overlay offload. It
  already bypasses composition, so there is no compositor work left to remove.
- The runtime must deactivate any previously active overlay before entering
  direct scanout. The direct commit disables the overlay plane, and the next
  composited frame is marked for a full Primary repaint.
- The KMS system test must exercise the boundary explicitly, because unit tests
  cannot prove the kernel accepted the intended plane set.

## Verification

- `test_inv_arch_134_direct_primary_excludes_overlay` in
  `crates/tessera-composite/src/presentation/scanout.rs` proves the planner
  gate for accepted and rejected primary plans.
- `test_inv_arch_135_overlay_requires_composited_primary` in
  `crates/tessera-platform/src/drm.rs` proves the backend source guard.
- `crates/tessera/src/runtime/scanout.rs` uses `overlay_plan_permitted` as the
  only overlay gate in the frame orchestration.
- [How to Validate KMS Overlay Offload](../how-to/kms-overlay-validation.md) includes
  the direct-scanout precedence scenario: a fullscreen opaque client must log
  `direct scanout active` and must not produce an overlay placement.

## Implementation Notes

- `crates/tessera-composite/src/presentation/scanout.rs` owns
  `overlay_plan_permitted`, the single pure predicate for the runtime gate.
- `crates/tessera/src/runtime/scanout.rs` evaluates the overlay planner only
  when `overlay_plan_permitted(&primary)` is true; the audit probe follows the
  same gate.
- `crates/tessera-platform/src/drm/device.rs` owns `overlay_source_supported`
  and rejects any overlay request whose primary source is not `Composited`.
- The existing `PrimaryPlaneSource` enum is the single source-of-truth for
  which buffer owns Primary; no boolean parameter reintroduces mixed sources.
