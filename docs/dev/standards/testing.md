# Automated Testing Standards

This document defines how automated tests are written, organized, and run in
Tessera. Automated tests are Stage 1 of the
[crystallization pipeline](taxonomy.md#5-experience-crystallization-pipeline):
any behavior a test can hold belongs in a test, not in a manual procedure.

Manual checks against a running system follow
[Verification Standards](verification.md). The two standards share one
cost-ordered ladder; this document owns its first layer.

---

## 1. Scope & Intent

A **test** is a deterministic check that the runner executes without a human:
unit, property, integration, protocol-probe, CLI/IPC, and end-to-end tests.
A test either passes or fails on its own; it never asks a person to look at a
screen, press a key, or judge a result.

| Question | Answer | Standard |
| :--- | :--- | :--- |
| Can a machine decide pass or fail? | Yes | This document |
| Does it need a person, real hardware, or a live host service? | Yes | [Verification Standards](verification.md) |

---

## 2. Test Runner

The workspace standardizes on [`cargo-nextest`](https://nexte.st/) in local
development and CI:

```bash
cargo install cargo-nextest --locked
cargo nextest run --locked --workspace
```

1. **One runner.** There is no `cargo test` fallback and no xtask test command.
   xtask owns architecture and packaging gates only; test execution is always
   `cargo nextest`.
2. **Binaries are built once.** The root suites spawn the production binaries
   (`tessera`, `tessera-mcp`, `tessera-cast`, ...) as separate processes.
   `.config/nextest.toml` declares a setup script that builds
   `--workspace --bins` once before those suites start; a test that ran
   `cargo build` itself would block on the target-directory lock the runner
   already holds. The script is filtered to those suites, so the native-free
   lane never builds a binary. Outside nextest, build the siblings first.
   The suites look the binaries up in the profile directory that holds the
   running test executable, so a custom `CARGO_TARGET_DIR`, `--profile` or
   `--target` needs no extra setup. Set `TESSERA_TARGET_DIR` only to point at
   binaries built somewhere else.
3. **Native-free lane.** Crates decoupled from the Optics C libraries run with
   only a Rust toolchain. The command is in
   [Manual Verification, Scenario 0](../verification/manual-verification.md#2-scenario-0-native-free-rapid-logic-verification-zero-c-dependencies).

---

## 3. Test Layout

Tests follow the crate boundary. The workspace keeps its multi-crate
architecture rather than collecting tests into one package.

| Scope | Location | Rule |
| :--- | :--- | :--- |
| Small and medium modules | `src/<module>.rs` | Inline `#[cfg(test)] mod tests` next to the code. |
| Large modules whose test block is bloated | `src/<module>/tests.rs` | Extract the block to a sibling `tests` submodule; the parent keeps `#[cfg(test)] mod tests;`. |
| Shared test infrastructure | `crates/tessera-test-support` | Dev-dependency only: environment guards, scratch dirs, socket names, polling, workspace/binary lookup. |
| External integration tests | `tests/integration/main.rs` + `tests/integration/` | One entry compiles all integration modules once. |
| External end-to-end tests | `tests/e2e/main.rs` + `tests/e2e/` | One entry compiles all e2e modules once; native modules are feature-gated. |
| Cross-suite helpers | `tests/common/mod.rs` | Re-exports `tessera-test-support`; no tests of its own. |
| Test fixtures | `crates/<crate>/tests/fixtures/` | Data and probe sources live in the package whose tests consume them, resolved through `env!("CARGO_MANIFEST_DIR")`. A missing fixture fails the test; it never skips. |

1. **The root `tests/` package is the single external-test entry point.** It
   depends on the workspace libraries, no production crate depends on it, and
   it never links the compositor application crate: production binaries are
   driven as separate processes through `tessera-test-support`'s artifact
   lookup.
2. **Private protocol state stays crate-local.** End-to-end tests that must
   drive private protocol state (for example
   `crates/tessera-wayland/src/tests/`) live under the crate's `src/tests/`.
   The root suite owns external process, binary, and public-API coverage.
3. **Extract only when bloated.** A test block moves to a sibling `tests` file
   only when it is genuinely bloated; business complexity stays in focused
   crate-local modules.

---

## 4. Writing Unit Tests

1. **Pure logic.** Presentation and policy logic is written as pure data
   transformations and tested in in-crate `#[cfg(test)]` modules with
   `cargo nextest run --locked -p <crate> --lib`.
2. **Source-free constructors.** A type that reads live sources provides a
   constructor that removes them, for example
   `ControlCenter::without_sources()`, so its tests need no GPU, bus, or tray.
3. **No windows, no devices.** Unit tests never open windows, devices, sockets
   outside a scratch directory, or the host session's runtime directory.
4. **Fail, never skip.** A missing fixture, binary, or precondition the test
   owns is a failure. Only the live tests in [Section 5](#5-live-tests) may be
   excluded from the default run.

---

## 5. Live Tests

Some checks are automated but need a live host service (NetworkManager, iwd,
a Wi-Fi P2P device). They follow one convention:

1. **Ignored by default.** Mark the test `#[ignore]` so CI and the default
   local run never depend on host state.
2. **Named `live_*`.** The prefix makes the whole class selectable:

   ```bash
   cargo nextest run -p <crate> --run-ignored only -E 'test(live_)' --no-capture
   ```

3. **Read-only unless stated.** A live test never connects, disconnects,
   forms a group, or mutates host configuration unless its name and doc
   comment say so, and it cleans up anything it creates.
4. **Owned by an acceptance gate.** Every live test is listed in the
   [verification](verification.md) document that requires it, so a release
   that depends on it runs it.

---

## 6. Automation Boundary

CI runs unit, property, Cargo integration, protocol-probe, CLI/IPC, and nested
logic tests. It does **not** run:

- a real KMS/DRM session: physical atomic commits, VT ownership, overlay
  planes, hotplug;
- installed PAM policy or physical lock, suspend, and resume;
- live host services: the `live_*` tests above;
- visual judgement of rendered surfaces.

These layers are covered by [verification procedures](verification.md). A
passing unit or nested result is never evidence of KMS scanout, PAM behavior,
or radio behavior.

When a manual check becomes automatable, automate it and delete the manual
step in the same change ([Verification Standards](verification.md#6-lifecycle--maintenance)).
