---
id: ADR-0240
title: "Repository automation lives in a root xtask; test fixtures live with their consumer"
status: accepted
date: 2026-11-16
scope: xtask, tests, crates/tessera-wayland, crates/tessera-shell, ci
superseded_by: null
negative_knowledge: true
---

# ADR-0240: Repository automation lives in a root xtask; test fixtures live with their consumer

- Status: Accepted
- Date: 2026-11-16
- Deciders: Tessera Maintainers
- Consulted: `xtask` (architecture gates), `crates/tessera-wayland` (probe-driven
  e2e tests), `crates/tessera-test-support`, `crates/tessera-shell` (motion seam), CI
- Amends: [ADR-0155](0155-workspace-tiering-shell-lexicon-and-state-canonization.md)
  (workspace topology), [ADR-0174](0174-universal-model-motion-view-chrome-architecture.md) (enforcement
  mechanism of INV-ARCH-49), [ADR-0195](0195-panic-discipline-and-poison-recovery.md)
  (baseline location)

---

## Context and Problem Statement

The workspace root carried `tooling/xtask` and `tests/fixtures/` with neither
placement earning its depth:

- `tooling/` held one package. The extra level matched no Cargo convention and
  forced every gate to locate the root as `CARGO_MANIFEST_DIR/../..`, while other
  gates read paths relative to the current directory, so the same tool behaved
  differently depending on where it was invoked.
- `tests/fixtures/` belonged to the root `tessera-tests` package, but its only
  consumers were `tessera-wayland` unit-level e2e tests. They reached it through
  `tessera_test_support::workspace::fixture()`, coupling a crate's tests to a
  different package's files and the shared support crate to repository layout.
  A missing probe printed "skipping" and passed.
- Several gates discovered packages by globbing directories and hand-parsing
  manifests, approximating what Cargo already resolves (renames, `[[bin]]`
  targets, platform-specific edges).
- `optics --verify-api` kept 38 hand-maintained substring markers to predict
  whether the pinned Optics tag provides the symbols the workspace uses. It ran
  nowhere, and the compiler answers the same question authoritatively when the
  workspace builds against the tag without the local `[patch]`.
- `xtask clippy` duplicated the CI clippy invocation with different flags, and
  `check-motion` (ADR-0174) ran only when someone remembered; it matched `.exp(`
  textually and missed `f32::exp(x)`.

## Decision Outcome

1. **xtask lives at `xtask/`** in the workspace root, the conventional cargo-xtask
   layout. It is a workspace member but not a default member, so plain builds and
   test runs do not compile it. `dependency-policy.toml` and `panic-baseline.toml`
   stay inside it.
2. **One source of workspace facts.** Every gate resolves the root from the xtask
   manifest and discovers packages, dependency edges, and binary targets through
   `cargo metadata`. Gates behave identically from any working directory.
3. **Fixtures live in the consuming package** at `crates/<crate>/tests/fixtures/`,
   resolved with `env!("CARGO_MANIFEST_DIR")`. A missing fixture fails the test.
   `tessera-test-support` no longer exposes a fixture lookup.
4. **Compiler-backed checks replace approximations.** `optics --verify-api` is
   removed; `optics` keeps `--tag-only` (CI) and `--set` (edits through
   `toml_edit`). `xtask clippy` is removed; CI owns clippy invocation.
5. **INV-ARCH-49 is enforced by clippy.** `crates/tessera-shell/clippy.toml`
   lists `f32::exp` and `f64::exp` under `disallowed-methods`; the
   `widgets::motion` seam opts out with a reasoned `#[expect]`. CI runs clippy
   over the full workspace so the rule is enforced, not advisory.
6. **Package conformity is enforced, not reported.** Every member, including
   `tests` and `xtask`, inherits workspace lints and declares every dependency
   with `workspace = true`.

### Invariants & Behavioral Boundaries

- The workspace root has no `tooling/` directory and no shared `tests/fixtures/`.
- No gate reads a path relative to the process working directory.
- A crate-local `clippy.toml` restates the root settings, because clippy uses the
  nearest file and does not merge.

## Rejected Alternatives & Negative Knowledge

- **Move xtask into `crates/`.** `crates/*` is the product tier that the boundary
  and panic gates reason about; mixing developer tooling into it needs exemptions.
- **Move the probe tests to the root `tests/e2e/`.** They drive crate-private
  protocol state (`use crate::*`); moving them would force private API public.
- **Express the raw `wl_resource_create` ban as a clippy `disallowed-methods`
  entry.** Clippy cannot resolve a path to a foreign function inside the private
  `ffi` module (`does not refer to a reachable function`), so `check-vtables`
  keeps that rule.
- **Ban `f32::exp` in the root `clippy.toml`.** INV-ARCH-49 is a chrome rule;
  other crates may legitimately need exponentials.

## Consequences

### Positive

- About a quarter of xtask's code is gone, and the remaining gates share one
  root resolver, one package model, and one file walker.
- The motion rule now catches every call form and runs in CI.
- A fixture moves or disappears loudly.

### Negative / Trade-offs

- `crates/tessera-shell/clippy.toml` duplicates the root clippy settings and must
  be kept in step with them.
- Full-workspace clippy needs the Optics native libraries, so it runs in the
  slower full-workspace CI job; the fast job keeps its flux-free subset.
