---
id: ADR-0232
title: "Damage diagnostics link tracing, never a subscriber"
status: accepted
date: 2026-11-10
scope: observability, dependency-boundaries, composite/wayland
superseded_by: null
negative_knowledge: true
---

# ADR-0232: Damage diagnostics link tracing, never a subscriber

- Status: Accepted
- Date: 2026-11-10
- Deciders: Tessera Architecture Working Group
- Consulted: `tessera-wayland` (commit/snapshot events), `tessera-composite`
  (assessment/upload events), `tessera-env` (process-level subscriber),
  `tooling/xtask` (capability policy)
- Amends: [ADR-0079](0079-tracing-based-observability-seam.md),
  [ADR-0164](0164-universal-domain-pruning-and-primitives-canonization.md),
  [ADR-0230](0230-damage-is-surface-local-and-viewport-is-a-buffer-transfer-inverse.md),
  [ADR-0231](0231-surface-damage-carries-an-explicit-full-verdict.md)

---

## Context and Problem Statement

ADR-0079 deliberately kept the `log` facade in libraries and the
`tracing`-based subscriber in the binary root. That split is still correct:
a subscriber is process-global and must be installed exactly once.

The damage diagnostics added by ADR-0230/0231 need more than a formatted log
string: surface identity, geometry, damage role, mapped boxes, repair reason,
and upload mode must be machine-readable and correlatable. The `log` facade
cannot carry structured fields. The producer crates — `tessera-wayland` and
`tessera-composite` — own the events, so moving the events into the runtime
would either duplicate state or lose the commit-time facts.

The capability policy in `tooling/xtask/dependency-policy.toml` explicitly
allowed `tracing` in the composition root but not in those two producers.
Adding the dependency without changing the policy fails
`cargo xtask check-boundaries`; changing the policy without a decision record
would make the edge an unreviewed exception.

## Decision Outcome

### 1. Producer crates may link `tracing`

`tessera-wayland` and `tessera-composite` may declare `tracing` as a normal
dependency **only to emit structured diagnostics**. Other events continue to
use the `log` facade. The dependency policy lists `tracing` for exactly those
two producers, with a comment naming this record.

### 2. No library installs a subscriber

`tracing-subscriber` and `tracing-log` remain confined to the process root
(`tessera-env` / the binaries). A library that depends on `tracing` must not
initialize a subscriber, install a layer, or assume one exists; events are
no-ops without one.

### 3. Targets are a stable diagnostic namespace

Damage events use `tessera.damage.commit`, `tessera.damage.shm`,
`tessera.damage.assess`, and `tessera.damage.upload`. They carry `metric` and
`count = 1`, so a tracing/metrics layer can aggregate them without a bespoke
in-process counter registry. Field expressions are gated by
`tracing::enabled!` so a disabled trace has no mapping or allocation cost.

## Invariants

- `[INV-BOUND-03]` **Tracing is a diagnostics edge, not a second logging
  stack.** Only the damage-producing crates listed in the policy may link
  `tracing`; no library may link or install a subscriber.
- `[INV-ARCH-121]` **Damage events are counter-shaped and gated.** Every
  `tessera.damage.*` event carries `metric`, `count`, and its diagnostic fields;
  expensive field construction is behind `tracing::enabled!`.

Enforced by:

- `cargo xtask check-boundaries` (`dependency-policy.toml`);
- `docs/dev/observability.md` target/field schema;
- the property/regression tests listed in ADR-0230/0231 (the events describe
  the same values those tests assert).

## Consequences

### Positive

- Damage failures are diagnosable from JSON traces without a debugger or a
  second telemetry crate.
- The library/binary observability split remains intact: events are cheap
  no-ops until the process root installs a subscriber.
- The capability policy now states the exact exception instead of silently
  forbidding the implementation.

### Negative / Trade-offs

- Two library crates grow a dependency on `tracing`; the policy and this record
  must move together.
- The diagnostic event schema is now a compatibility surface for tooling. Field
  renames require updating `docs/dev/observability.md` and the triage page.

## Rejected Alternatives & Negative Knowledge

### Keep damage diagnostics as formatted `log` strings

- **Why considered**: no dependency-policy change.
- **Why rejected**: structured fields are the point. A string is not a counter,
  cannot be filtered by viewport/transform, and cannot be correlated by a
  metrics layer.

### Move the events into `tessera`

- **Why considered**: `tessera` already links `tracing` and owns the frame loop.
- **Why rejected**: commit-time facts (per-commit damage, snapshot repair,
  mapped raw box) belong to the producer. Forwarding them would duplicate state
  and still need the same fields.

### Add a diagnostics crate

- **Why considered**: one shared event schema type.
- **Why rejected**: an event schema is not a runtime subsystem. The stable
  target/field names plus the observability document are sufficient; a crate
  would add a dependency edge for every producer without a payload type that
  crosses a boundary.
