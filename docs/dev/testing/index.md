# Testing in Tessera

This directory provides testing documentation, operational execution playbooks,
and verification procedures for Tessera.

For the build, run, and automated verification workflow, see the
[Build & Run Guide](../build-and-run.md#5-automated-verification--testing).

---

## 1. Testing Documentation Map

The testing directory is organized by progressive disclosure across operational
scenarios:

| Document | Scope & Purpose |
| :--- | :--- |
| **[`manual-testing.md`](manual-testing.md)** | **Manual Testing Playbook**: Unified scenario-based decision matrix, copy-paste ready isolated execution recipes, client injection, native-free fast tests, and smoke checklists. |
| **[`surface-testing.md`](surface-testing.md)** | **Surface Testing**: Detailed specification of the 4-layer testing model, preview harness contracts, non-default feature gating, and agent pixel capture. |
| **[`vt-drm-testing.md`](vt-drm-testing.md)** | **VT/DRM Manual Testing**: In-depth hardware verification runbook for KMS atomic modesetting, multi-output pacing, GPU device selection, and TTY switching. |
| **[`network-hardware.md`](network-hardware.md)** | **Network and Casting Hardware Acceptance**: Live NetworkManager/iwd checks, isolated-AP station operations, Miracast group formation, and the release gate for ADR-0242. |
| **[`../nested-backend.md`](../nested-backend.md)** | **Nested Backend Development**: Subsystem architectural details on nested Vulkan swapchains, FIFO timing, and dma-buf tranche negotiation. |

---

## 2. The Four-Layer Testing Hierarchy

Testing execution adheres to a strict cost-ordered verification ladder:

```text
Low Cost / High Speed                            High Cost / Broad Scope
[ Layer 1: Unit & Logic ] ──▶ [ Layer 2: Previews ] ──▶ [ Layer 3: Nested Integration ] ──▶ [ Layer 4: VT/DRM Hardware ]
(Sub-second, hermetic)        (Instant UI window)       (Full compositor in window)         (Bare-metal Linux TTY)
```

| Layer | Environment | Key Coverage | Reference Guide |
| :--- | :--- | :--- | :--- |
| **Layer 1: Unit & Logic** | In-crate tests, zero GPU / display dependencies | State machines, layout math, protocol encoding, IPC schemas, config parsing | [`manual-testing.md`](manual-testing.md#2-scenario-0-native-free-rapid-logic-verification-zero-c-dependencies) |
| **Layer 2: Surface Previews** | Standalone inspection harness (`dev-preview`), mock sources | Isolated surface layout, animations, VRM avatar rendering, credential input | [`surface-testing.md`](surface-testing.md#inspection-harnesses-separate-development-only-binaries) |
| **Layer 3: Nested Integration** | Nested Wayland compositor window (`TESSERA_BACKEND=nested`) | Full compositor runtime, multi-window stacking, client injection, input routing | [`manual-testing.md`](manual-testing.md#4-scenario-2-testing-inside-an-existing-desktop-nested-mode) |
| **Layer 4: Physical Hardware** | Direct KMS/DRM session from bare Linux TTY | Atomic modeset, page flips, multi-GPU affinity, physical outputs, PAM login, output roles and mirror letterbox, overlay offload | [`vt-drm-testing.md`](vt-drm-testing.md), [`mirror-and-multi-output-system-test.md`](mirror-and-multi-output-system-test.md), [Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md) |

---

### Current Automation Boundary

CI runs unit, property, Cargo integration, protocol-probe, CLI/IPC, and nested
logic tests. It does **not** run a real KMS/DRM session: physical atomic
commits, VT ownership, and overlay planes require a hardware lab or a dedicated
TTY run. Use [How to Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md)
runbook for that layer; do not treat a unit or nested result as proof of KMS
scanout.

## 3. Test Runner Standards & Tooling

The workspace standardizes on [`cargo-nextest`](https://nexte.st/) for fast,
parallelized test execution in local development and CI:

```bash
# Install nextest once
cargo install cargo-nextest --locked

# Run every unit, integration, and end-to-end test
cargo nextest run --locked --workspace
```

The workspace has no `cargo test` fallback and no xtask test command. xtask
owns architecture and packaging checks only; test execution is always
`cargo nextest`.

The root suites spawn the production binaries (`tessera`, `tessera-mcp`,
`tessera-cast`, ...) as separate processes. `.config/nextest.toml` declares a
setup script that builds `--workspace --bins` once before the tests that spawn
them start, because a test that runs `cargo build` itself would block on the
target-directory lock the runner already holds. The script is filtered to those
suites, so the native-free lane (where they are compiled out) never builds a
binary. Outside nextest the siblings must be built first.

For zero-native-dependency development (without compiling Optics C libraries),
use the [Native-Free Test Pathway](manual-testing.md#2-scenario-0-native-free-rapid-logic-verification-zero-c-dependencies)
to validate the native-free crates and the root suites in sub-second time.

### Test layout rules

The workspace deliberately keeps its multi-crate architecture; tests follow
the crate boundary rather than forcing everything into one test package.

| Scope | Location | Rule |
| :--- | :--- | :--- |
| Small and medium modules | `src/<module>.rs` | Inline `#[cfg(test)] mod tests` next to the code. |
| Large modules whose test block is bloated | `src/<module>/tests.rs` | Extract the block to a sibling `tests` submodule; the parent keeps `#[cfg(test)] mod tests;`. |
| Shared test infrastructure | `crates/tessera-test-support` | Dev-dependency only: environment guards, scratch dirs, socket names, polling, workspace/binary lookup. |
| External integration tests | `tests/integration/main.rs` + `tests/integration/` | One entry compiles all integration modules once. |
| External end-to-end tests | `tests/e2e/main.rs` + `tests/e2e/` | One entry compiles all e2e modules once; native modules are feature-gated. |
| Cross-suite helpers | `tests/common/mod.rs` | Re-exports `tessera-test-support`; no tests of its own. |
| Test fixtures | `crates/<crate>/tests/fixtures/` | Data and probe sources live in the package whose tests consume them, resolved through `env!("CARGO_MANIFEST_DIR")`. A missing fixture fails the test; it never skips. |

The root `tests/` package is the single external-test entry point. It depends on
the workspace libraries, but no production crate depends on it, and it never
links the compositor application crate: the production binaries are driven as
separate processes through `tessera-test-support`'s artifact lookup. Large modules are
moved to a sibling `tests` file only when the test block is genuinely bloated;
business complexity stays in focused crate-local modules. End-to-end tests that
must drive private protocol state (for example
`crates/tessera-wayland/src/tests/`) stay crate-local under `src/tests/`; the
root suite owns external process, binary, and public-API e2e coverage.

---

## 4. Directory Organization Rule

In accordance with the verification ladder documented in the
[Build & Run Guide](../build-and-run.md#5-automated-verification--testing):

- Daily-use manual testing recipes and execution commands belong in
  [`manual-testing.md`](manual-testing.md).
- When an individual scenario accumulates deep operational edge cases,
  multi-GPU procedures, or specialized harness documentation, it is maintained as
  a focused scenario document within `docs/dev/testing/` (e.g. `surface-testing.md`,
  `vt-drm-testing.md`) to keep the primary playbook compact and intuitive.
- Every specialized scenario document maintains a breadcrumb link back to the
  testing index and the primary playbook.
