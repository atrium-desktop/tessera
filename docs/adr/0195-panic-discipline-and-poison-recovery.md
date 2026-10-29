---
id: ADR-0195
title: "Panic discipline: recover from lock poisoning, ratchet the rest"
status: accepted
date: 2026-10-26
scope: runtime/availability, ipc, platform, shell, worker-threads
superseded_by: null
negative_knowledge: true
---

# 0195. Panic discipline: recover from lock poisoning, ratchet the rest

- Status: Accepted
- Date: 2026-10-26
- Deciders: Tessera Maintainers

---

## Context and Problem Statement

Tessera is a long-lived, multi-threaded service: a compositor render thread, an
IPC accept/dispatch loop, and several out-of-process-adjacent workers (tray
watcher, Bluetooth/BlueZ, Wi-Fi/IWD, wallpaper decode, audit writer). A panic on
any of those threads is a visible session failure, not a contained error.

The codebase carried two related hazards with no policy and no mechanism:

1. **Poison cascade.** A `Mutex`/`RwLock` acquisition written as
   `.lock().unwrap()` turns one panicking writer into a panic generator for
   *every* later reader. An inventory found **234** such sites in production
   code across 19 files — including the tray watcher and the runtime IPC
   dispatcher. The recovery idiom (`unwrap_or_else(|e| e.into_inner())`) existed
   but was used only inside one test.
2. **Unbounded panic surface.** Roughly 154 further `unwrap`/`expect`/`panic!`
   sites sat in production code with no distinction between a deliberate
   fail-fast at startup, an invariant tripwire, and an unexamined edge case.
   Nothing prevented new ones from being added.

`unwrap` is a fine *failure signal in tests*; the problem is only production
reachability. Any policy must therefore see test code and production code
differently, and must not rely on reviewers to enforce it.

## Decision Drivers

- A lock acquisition must not be the thing that spreads one thread's panic.
- The rule must be **mechanical**: enforced by a gate, not by review vigilance.
- The distinction between test and production code must be **structural**, not
  "whatever the last editor did".
- Existing debt must be **visible and monotonically shrinking**, not hidden
  behind an allow attribute or a blanket lint.

## Decision Outcome

Panic discipline is stated in
[`docs/dev/standards/error-handling.md`](../dev/standards/error-handling.md)
and enforced by `cargo run -p xtask -- check-panics` over production code.

**Rule 1 — no poisonable lock may panic (hard zero).** Every production
`.lock()`/`.read()`/`.write()` followed by `.unwrap()`/`.expect(` is replaced
with `unwrap_or_else(|e| e.into_inner())`; the gate fails on any occurrence. All
234 sites were migrated. The change only alters the already-poisoned path
(today: panic; after: recover), so it cannot change non-poison behaviour.

**Rule 2 — no new panic sites (ratchet).** The remaining production panic sites
are recorded per crate in
[`tooling/xtask/panic-baseline.toml`](../../tooling/xtask/panic-baseline.toml).
The gate fails when a count rises or a new crate appears with sites and no
baseline; counts may fall, and `--update` rewrites the ledger so a reduction is
recorded in the same change.

The gate's production/production line is drawn by a brace-aware scan of
`#[cfg(test)]` items (not "everything after the first test attribute", which
would hide production code below a mid-file test helper), excluding `tests/`
directories and `tests.rs` modules.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-51] Panic Discipline`: production code contains no poisonable lock
  acquisition and no panic site beyond its recorded baseline. Enforced by
  `cargo run -p xtask -- check-panics` (hard-fail on both rules) in CI.

## Consequences

### Positive

- One worker's panic can no longer cascade into a whole-session failure through
  a mutex.
- The panic surface is a visible, per-crate number that can only go down; a
  reviewer sees the debt and its direction.
- The test/production split is structural, so a mid-file test helper cannot
  silently exempt the code beneath it.

### Negative / Trade-offs

- Recovering a poisoned mutex proceeds with possibly inconsistent data rather
  than refusing to run. This is the deliberate trade — a compositor that keeps
  presenting is preferred over one that panics — and the recovery is logged
  where the value matters.
- The ratchet accepts the existing 154 sites as debt rather than fixing them in
  one pass. They are the ledger's purpose: each removal is a small, reviewable
  change that lowers a committed number.
- The brace-aware scan is a source heuristic; braces inside string literals or
  raw strings could confuse it. It errs toward over-counting production (which
  only ever makes the gate stricter), and the frozen baseline makes any drift
  visible.

## Rejected Alternatives & Negative Knowledge

### Enable `clippy::unwrap_used`/`expect_used`/`panic_used` workspace-wide
- **Why considered**: It is the idiomatic Rust mechanism, needs no bespoke gate,
  and the workspace already carries clippy lints.
- **Why rejected**: Clippy has no test-code exemption, so the restriction would
  flag all ~1,780 test sites too; suppressing it requires a `cfg_attr(test,
  allow(...))` on every crate root and every integration-test file, and the lint
  still cannot express Rule 1 (it flags `.lock().unwrap()` as an ordinary
  `unwrap_used`, losing the poisoning distinction). A ratchet it cannot express
  would force fixing all 154 sites at once or allowing the lint wholesale.

### `disallowed_methods` for `Mutex::lock`/`RwLock::read|write`
- **Why considered**: It reuses the existing `clippy.toml` mechanism and would
  route every acquisition through a helper.
- **Why rejected**: It bans the *acquisition*, not the *panic on poison* — a
  correctly recovering `.lock().unwrap_or_else(...)` would be rejected too, and a
  `try_lock` fallback is legitimate. It also demands migrating every `.lock()`
  call site rather than the poison sites, a far larger blast radius for a
  weaker rule.

### Blanket `.expect("lock poisoned")` messages
- **Why considered**: Preserves a message at each site.
- **Why rejected**: It still panics, so it does not address the cascade; the
  message describes a symptom the recovery removes entirely.

### Assert only under `debug_assertions`
- **Why considered**: Cheaper, and tests would catch new sites.
- **Why rejected**: The cascade is a release-only availability hazard; a check
  that disappears in release protects the wrong build.

## Verification

- `cargo run -p xtask -- check-panics` — Rule 1 (hard zero) and Rule 2
  (ratchet), wired into CI Gate A.
- `cargo nextest run -p xtask` — the gate's own tests cover produce detection
  (mid-file `#[cfg(test)]`, brace-less test items, chain collapsing, and that
  the recovery idiom is not itself counted as a poison site).
- `cargo nextest run --workspace` — the full suite, which exercised the 234
  migrated lock sites.
