---
id: ADR-0235
title: "Overlay Offload Is a Topmost Opaque Single-Surface Candidate, Never a General Layer"
status: accepted
date: 2026-11-13
scope: composite/presentation, platform/drm, runtime/presentation, docs/reference
superseded_by: null
negative_knowledge: true
---

# 0235. Overlay Offload Is a Topmost Opaque Single-Surface Candidate, Never a General Layer

- Status: Accepted
- Date: 2026-11-13
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-composite` (candidate policy), `crates/tessera-platform` (plane capabilities/commit), `crates/tessera` (scene facts/damage)
- Amends: [ADR-0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md), [ADR-0234](0234-output-layers-are-plane-allocation-contracts-not-scene-layers.md)
- Amended by: [ADR-0236](0236-overlay-placement-is-a-pure-state-machine.md) — placement transitions are a pure state machine with an explicit Primary-repaint contract; [ADR-0237](0237-a-direct-scanned-primary-excludes-overlay-offload.md) — a direct-scanned Primary excludes overlay offload.

---

## Context and Problem Statement

ADR-0234 established that overlay planes are output layers, not scene layers,
and that no overlay may be allocated until a planner proves the complete plane
set with one atomic `TEST_ONLY` request. That leaves a policy gap: which scene
surface, if any, is a legitimate overlay candidate?

Without a narrow candidate contract, “overlay offload” naturally degenerates
into “put anything with a matching format on a spare plane”. That fails for
translucency, rounded corners, shadows, scaling, color management, stacking
against chrome, and multiple overlapping overlays. It also turns a hardware
accelerator into a second compositor with different semantics.

The planner, audit probe, and commit path now exist. The decision therefore has
to cover not only candidate selection but the proof boundary between planning a
plane and presenting through it. A driver-specific overlay failure must degrade
to composition, never to a partial or duplicated scanout.

## Decision Drivers

- Overlay planes sit above or below the entire primary framebuffer; they cannot
  interleave with chrome inside Primary.
- Overlay content bypasses the compositor’s damage and capture path.
- Client buffers use arbitrary formats, modifiers, transforms, and color.
- Hardware overlay availability and constraints vary per CRTC and driver.
- One wrong allocation is a visible correctness failure, not a performance
  regression.
- Direct primary scanout already owns the output with no compositor
  framebuffer underneath; overlay must not be layered onto that state.
- Default behavior must remain fail-closed until real KMS drivers prove the
  candidate class.

## Decision Outcome

### 1. Candidate class

At most one overlay candidate is selected per output per frame. It must be:

- a **toplevel** window;
- **topmost** in the scene among client windows;
- a **single-surface** tree: exactly one dma-buf surface, no subsurfaces,
  popups, or SHM surfaces;
- **fully opaque** by format or declared opaque region;
- **identity-mapped**: normal transform, no viewport crop, buffer extent
  matching the target rectangle 1:1;
- **inside one output**;
- **not overlapped** by visible chrome or a software cursor;
- **not color-managed** in a way the plane cannot express;
- free of open/close/move/resize transitions, overview, window switcher,
  screenshot freeze, capture, stream, and session-lock state;
- evaluated only when Primary is **composited**. A direct-scanned Primary
  excludes overlay offload for that frame (ADR-0237).

Video and zero-copy client buffers are the expected first users. The policy is
not “fullscreen video only”: a large opaque topmost window is eligible when it
satisfies every condition.

### 2. Not candidates

The following are never overlay candidates, regardless of format support:

- wallpaper or any background scene layer;
- always-on-top as a policy (a pinned window may be a candidate only because
  it is topmost, opaque, and otherwise eligible);
- chrome, docks, panels, menus, notifications, tooltips, IME popups, drag
  icons, or client cursor surfaces;
- shadows, blur, rounded-corner masks, opacity groups, or any effect output;
- translucent or partially occluded windows;
- a second overlay stacked above or below another overlay candidate;
- the client already scanning out directly on Primary (ADR-0237).

### 3. Plan, prove, and present are three separate steps

- **Plan** — `plan_overlay` is a pure evaluation. It returns either an
  `OverlayPlan` (candidate surface id, window, target rectangle, optional
  `zpos`) or a stable `OverlayRejection` reason. It does not import or own a
  buffer, allocate a plane, or mutate scanout state.
- **Prove** — `Host::probe_overlay` imports the candidate buffer, programs
  Primary + Cursor + Overlay for the candidate CRTC, submits one atomic
  `TEST_ONLY` request, and releases the import. It never commits and never arms
  a page flip. The kernel validates the resulting complete atomic state. While
  overlay presentation is disabled, the composition root runs this probe only
  for a policy-accepted candidate under debug audit logging; when presentation
  is enabled, the real commit path performs the same proof before committing.
- **Present** — `Host::present_with_overlay` is the only allocation entry.
  It imports the client buffer through the client scanout cache, programs the
  complete Primary + Cursor + Overlay set, runs `TEST_ONLY` before the real
  commit, and retires the overlay buffer with the page flip.

`OverlayPlanePolicy::Disabled` is the default. `TESSERA_OVERLAY_OFFLOAD=1`
selects the explicit hardware-validation path. Enabling the policy allocates
nothing by itself: the next frame still has to pass policy, proof, and commit.

### 4. Damage and ownership

- Overlay buffer damage belongs to the overlay plane, not to Primary.
- Overlay geometry, stacking, or visibility changes invalidate the plan and
  damage the old and new Primary rectangles, because Primary must repaint
  whatever the overlay used to cover.
- While the plan is stable and only overlay content changes, Primary placement
  state must not be invalidated or force a full repaint merely because the
  overlay client committed. The first presentation path may still present a
  Primary commit whose pixels are unchanged; an overlay-only fast path is a
  follow-up optimization, not part of this contract.
- A capture, screenshot, stream, or session-lock transition disables the
  overlay plan before the frame is assembled.
- If the candidate disappears between planning and presentation, the runtime
  deactivates the overlay, forces a full Primary repaint, and retries a
  composited frame.

### 5. z-order

When the overlay plane exposes `zpos`, the backend programs the planner’s
requested value only inside the driver-declared range. The current planner
requests no explicit `zpos` and relies on the driver’s default order (Primary
below overlay, Cursor above overlay); the complete set is still validated.
Overlay never participates in scene z-order: it must be above every Primary
pixel it covers, which is why chrome/software-cursor overlap is a hard reject.

### 6. Failure is fallback, never partial scanout

- A policy or probe rejection means composition.
- A rejected or failed real commit deactivates overlay state, forces a full
  Primary repaint, and retries a composited frame.
- No frame may commit a subset of the intended plane set, leave a stale overlay
  plane enabled, or present a candidate without a `TEST_ONLY` proof.
- A failed overlay path must not poison the primary direct-scanout path or the
  ordinary composited path.

### 7. Default-on is a hardware-gated rollout, not an ADR status

This ADR is accepted as the binding contract for the opt-in path. Removing
`TESSERA_OVERLAY_OFFLOAD` and enabling overlay by default is a separate,
evidence-based rollout step: it requires the [KMS Overlay System
Test](../dev/testing/kms-overlay-system-test.md) exit criteria on at least one
real KMS driver. “Draft” is not used as a feature-flag lifecycle; the ADR
records the architecture, and the runbook records the hardware gate.

## Invariants & Behavioral Boundaries

- `[INV-ARCH-126] Overlay Is One Topmost Single-Surface Candidate.` No
  multi-surface tree, popup, subsurface, or non-topmost window may be planned
  as an overlay. **Enforcement:** `plan_overlay` rejects every other shape
  before plane evaluation; `test_inv_arch_126_overlay_requires_one_topmost_single_surface`
  and the accepted-plan `proptest` cover the policy.
- `[INV-ARCH-127] Overlay Never Overlaps Chrome or a Software Cursor.` If the
  candidate rectangle intersects a visible chrome footprint or software cursor
  sprite, the plan is rejected rather than drawing the overlay above it.
  **Enforcement:** `chrome_overlaps_candidate` and
  `software_cursor_overlaps_candidate` rejection ordering;
  `test_inv_arch_127_overlay_rejects_chrome_and_software_cursor_overlap`.
- `[INV-ARCH-128] Overlay Requires an Enabled Policy and Complete-Set
  TEST_ONLY.` Primary, cursor, and overlay must be validated together before
  any real commit; `Disabled` allocates nothing; rejection means composition.
  **Enforcement:** `import_overlay_scanout` refuses a disabled policy,
  `commit_scanout` runs the `TEST_ONLY` preflight for every overlay
  transition, `test_inv_arch_128_disabled_policy_never_allocates_overlay`
  covers the policy gate, and the KMS system test covers the kernel proof.
- `[INV-ARCH-129] Overlay Content Is Opaque, Identity-Mapped, and
  Color-Expressible.` Translucent, color-managed, scaled, cropped, or
  transformed candidates are rejected. **Enforcement:**
  `test_inv_arch_129_overlay_rejects_translucent_scaled_or_color_managed`
  and the accepted-plan `proptest`.

## Rejected Alternatives & Negative Knowledge

### Multiple overlay planes as a general layer stack

- **Why considered**: Hardware may offer several overlay planes.
- **Why rejected**: Each additional plane multiplies z-order, damage, fence,
  and fallback combinations. The first implementation proves one candidate per
  output before considering a stack.

### Overlay for chrome, notifications, or effects

- **Why considered**: A spare plane could offload a panel or menu.
- **Why rejected**: These surfaces are translucent, rounded, shadowed, or
  sampled from the composited scene. They do not have an independent scanout
  contract and would produce wrong pixels above Primary.

### Overlay for always-on-top by policy

- **Why considered**: Pinned windows are conceptually “above” normal content.
- **Why rejected**: Always-on-top is a scene-stacking flag. Plane eligibility
  is determined by actual topmost position, opacity, geometry, and chrome
  overlap, not by the flag.

### Cursor on a generic overlay

- **Why considered**: Overlay planes are also above Primary.
- **Why rejected**: Cursor needs asynchronous movement and hotspot semantics
  (ADR-0234). A generic overlay cannot provide that update path.

### Best-effort allocation

- **Why considered**: It might make more frames use hardware.
- **Why rejected**: A false positive produces incorrect pixels or a frozen
  region. Composition is always the safe fallback.

### Layer overlay onto a direct-scanned Primary

- **Why considered**: Both paths bypass composition, so combining them appears
  to offload more pixels.
- **Why rejected**: Direct scanout owns the output with no compositor
  framebuffer behind the overlay, and an overlay candidate above a direct
  client contradicts direct scanout’s single-client, chrome-free
  precondition. ADR-0237 makes the exclusion explicit.

### Treat “draft” as the feature-flag lifecycle

- **Why considered**: The candidate policy is only enabled on real hardware
  after a manual test.
- **Why rejected**: ADR status records architectural ratification, not
  deployment state. A draft ADR that is already encoded and enforced in the
  tree is a toothless record; the hardware gate belongs in the runbook and the
  default policy value.

## Consequences

### Positive

- The candidate policy is explicit, testable, and independent of KMS.
- Rejection reasons become stable audit telemetry before default-on.
- The first overlay implementation has one candidate, one output, and one
  complete-set proof instead of a general layer graph.
- Plan, prove, and present have separate APIs, so audit cannot silently
  become allocation.
- The ADR is binding while default-on remains a measured rollout decision.

### Negative / Trade-offs

- Overlay offload remains opt-in until the KMS system test passes on real
  drivers.
- Large opaque windows are the only early beneficiaries; small, translucent,
  or chrome-overlapped windows stay composited.
- The first presentation path still submits Primary alongside an overlay
  content update; an overlay-only commit is a follow-up optimization, not a
  correctness requirement.
- The pure policy deliberately stops at format, modifier, opacity, geometry,
  and scene eligibility. Plane-level `zpos`, fence, and geometry constraints
  are proven by the backend `TEST_ONLY` request and surface as `kms-rejected`
  if the kernel refuses them.

## Verification

- `cargo test -p tessera-composite -p tessera-platform` covers policy,
  placement, the disabled-policy gate, and the backend request values.
- `tests/integration/composite_overlay.rs` repeats accepted-plan →
  placement → buffer update → movement → deactivation through the public
  presentation boundary.
- `cargo run -p xtask -- check-adrs` validates this record and its lineage.
- [KMS Overlay System Test](../dev/testing/kms-overlay-system-test.md) is the
  hardware acceptance gate for default-on and documents the full scenario
  matrix, log expectations, and rejection-label triage.

## Implementation Notes

- `crates/tessera-composite/src/presentation/overlay.rs` owns the pure
  candidate policy: `OverlaySceneFacts`, `OverlaySurfaceFacts`,
  `OverlayPlaneFacts`, `OverlayRejectReason`, `OverlayTelemetry`, and
  `plan_overlay`.
- `crates/tessera/src/runtime/scanout.rs` gathers candidate facts from the
  live scene and evaluates the planner. When overlay presentation is disabled
  and debug logging is enabled, it runs the audit probe and records the result
  through `OverlayTelemetry`; the result never feeds presentation.
- `crates/tessera-platform` exposes `OverlayPlaneInfo` value snapshots
  (formats, modifiers, zpos range, alpha, fence, damage support) so the
  composition root never sees DRM property handles.
- `Host::probe_overlay` / `DrmBackend::probe_overlay` import the candidate
  buffer, program Primary + Cursor + Overlay, and run `TEST_ONLY`. They never
  commit, never arm a page flip, and release the import immediately.
- `Renderer::draw_surfaces_ordered_with_window_shadows_excluding` can remove a
  complete window tree (including shadow and feedback) from the Primary pass.
- `DrmBackend::present_with_overlay` imports the client buffer through the
  client scanout cache, restates Primary + Cursor + Overlay, runs `TEST_ONLY`,
  then commits the complete set. `OverlayScanout` retires with the page flip
  and shares the client framebuffer lifetime rules.
- Overlay presentation is gated by `TESSERA_OVERLAY_OFFLOAD=1`
  (`OverlayPlanePolicy::Enabled`). Unset keeps allocation and presentation
  disabled; the audit probe still works.
- Unit tests cover topmost/single-surface/opaque acceptance, every rejection
  class, plane capability matching, window exclusion, and transition damage.
- KMS system test procedure, scenario matrix, log expectations, and
  rejection-label triage are documented in
  [KMS Overlay System Test](../dev/testing/kms-overlay-system-test.md).
