---
id: ADR-0236
title: "Overlay Placement Is a Pure State Machine, Not Runtime Ad-Hoc Branching"
status: accepted
date: 2026-11-13
scope: composite/presentation, runtime/presentation
superseded_by: null
negative_knowledge: true
---

# 0236. Overlay Placement Is a Pure State Machine, Not Runtime Ad-Hoc Branching

- Status: Accepted
- Date: 2026-11-13
- Deciders: Tessera Architecture Working Group
- Consulted: `crates/tessera-composite` (overlay/presentation values), `crates/tessera` (frame orchestration)
- Amends: [ADR-0235](0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md)

---

## Context and Problem Statement

The first overlay presentation path grew inside `render_and_present` as ad-hoc
comparisons:

- `active_overlay: Option<ActiveOverlay>`;
- `same_placement()` implemented on the runtime type;
- inline `match` arms deciding activate/update/replace/deactivate;
- inline `damage = Full` and logging in the same branch.

That made the most failure-prone part of the feature — the transition from a
stable plan to a new buffer, a different window, or no overlay — impossible to
test without a full compositor, GPU, and DRM session. It also let presentation
orchestration own policy that belongs to the overlay layer.

## Decision Drivers

- Buffer-only updates and placement changes have different Primary-damage
  contracts and must be distinguishable by tests.
- Rejected plans must never become active state.
- The transition policy must be testable without KMS or a renderer.
- The runtime should react to a transition, not compute it.

## Decision Outcome

### 1. Pure placement value

`OverlayPlacement` owns:

- `window`;
- `surface_id` (current buffer generation);
- `output` connector;
- physical target rectangle;
- optional `zpos`.

`surface_id` is **not** part of placement identity. A new client buffer is an
update, not a Primary layout transition.

### 2. Pure state machine

`OverlayState` owns `Option<OverlayPlacement>` and exposes:

```rust
reconcile(wanted: Option<OverlayPlacement>) -> OverlayChange
deactivate() -> OverlayChange
```

`OverlayChange` is one of:

- `None`;
- `Activated`;
- `Updated` (same placement, new buffer);
- `Replaced` (different window/output/target/zpos);
- `Deactivated`.

`OverlayChange::requires_primary_repaint()` is true only for activation,
replacement, and deactivation.

### 3. Runtime reacts, never decides

`render_and_present` only:

1. builds `Option<OverlayPlacement>` from the accepted plan;
2. calls `reconcile`;
3. applies full Primary damage when the transition requires it;
4. logs the resulting state.

It does not compare fields, mutate an `active_overlay` option, or duplicate the
transition policy.

## Invariants & Behavioral Boundaries

- `[INV-ARCH-130] Placement Identity Excludes Buffer Generation.` A different
  `surface_id` with the same window/output/target/zpos is `Updated`, never
  `Replaced`. **Enforcement:**
  `test_inv_arch_130_placement_identity_excludes_buffer_generation`.
- `[INV-ARCH-131] Buffer-Only Overlay Updates Do Not Damage Primary.` Only the
  overlay plane's pixels change; `requires_primary_repaint()` must be false.
  **Enforcement:**
  `test_inv_arch_131_buffer_only_updates_do_not_damage_primary`.
- `[INV-ARCH-132] Placement Transitions Damage Primary.` Activated, Replaced,
  and Deactivated transitions require a full Primary repaint in the first
  presentation path. **Enforcement:**
  `test_inv_arch_132_placement_transitions_damage_primary`.
- `[INV-ARCH-133] A Rejected Plan Never Enters Overlay State.`
  `OverlayPlacement::from_plan` returns `None` for any rejected plan.
  **Enforcement:** `test_inv_arch_133_rejected_plan_never_enters_overlay_state`.

## Rejected Alternatives & Negative Knowledge

### Keep `active_overlay` and inline comparison in the runtime

- **Why considered**: It was already written and worked for the happy path.
- **Why rejected**: It is untestable without a full compositor and duplicates
  the placement identity in two places. The first bug (treating a new buffer as
  a full transition) would force a Primary repaint on every video frame.

### Put the state machine in the backend

- **Why considered**: The backend owns plane state.
- **Why rejected**: The backend must not own candidate policy or scene damage.
  It receives a placement only after policy acceptance.

### Make `surface_id` part of placement identity

- **Why considered**: It makes every change a replacement and avoids a special
  case.
- **Why rejected**: It forces a full Primary repaint for every overlay client
  commit, defeating the purpose of overlay offload and violating
  `[INV-ARCH-131]`.

## Consequences

### Positive

- Overlay transitions are unit-testable and property-testable in
  `tessera-composite`.
- The runtime has one transition call instead of four inline branches.
- Buffer-only updates and layout changes have one explicit contract.
- A rejected plan cannot contaminate active state.

### Negative / Trade-offs

- The first presentation path still repaints Primary on placement transitions
  even when a narrower old/new rectangle would suffice; `overlay_transition_damage`
  remains the follow-up optimization.
- The runtime still owns the decision to enable overlay and the full-redraw
  response; the state machine owns only placement transitions.

## Implementation Notes

- `crates/tessera-composite/src/presentation/overlay.rs` owns
  `OverlayPlacement`, `OverlayState`, and `OverlayChange`.
- `crates/tessera/src/runtime/presentation.rs` reconciles the accepted plan and
  reacts to `requires_primary_repaint()`.
- Unit tests cover activation, buffer-only update, replacement, deactivation,
  rejected-plan filtering, and the repaint contract.
- A `proptest` property asserts that any accepted `OverlayPlan` satisfies every
  candidate invariant.
- `tests/integration/composite_overlay.rs` repeats the critical
  transitions through the public presentation boundary, so a breaking API
  change fails as an integration test rather than only a module-private unit
  test.
