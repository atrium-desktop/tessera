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
| **[`manual-testing.md`](manual-testing.md)** | **Manual Testing Playbook**: Unified scenario-based decision matrix, copy-paste ready isolated execution recipes, client injection, flux-free fast tests, and smoke checklists. |
| **[`surface-testing.md`](surface-testing.md)** | **Surface Testing**: Detailed specification of the 4-layer testing model, preview harness contracts, non-default feature gating, and agent pixel capture. |
| **[`vt-drm-testing.md`](vt-drm-testing.md)** | **VT/DRM Manual Testing**: In-depth hardware verification runbook for KMS atomic modesetting, multi-output pacing, GPU device selection, and TTY switching. |
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
| **Layer 1: Unit & Logic** | In-crate tests, zero GPU / display dependencies | State machines, layout math, protocol encoding, IPC schemas, config parsing | [`manual-testing.md`](manual-testing.md#scenario-0-flux-free-rapid-logic-verification) |
| **Layer 2: Surface Previews** | Standalone inspection harness (`dev-preview`), mock sources | Isolated surface layout, animations, VRM avatar rendering, credential input | [`surface-testing.md`](surface-testing.md#inspection-harnesses-separate-development-only-binaries) |
| **Layer 3: Nested Integration** | Nested Wayland compositor window (`TESSERA_BACKEND=nested`) | Full compositor runtime, multi-window stacking, client injection, input routing | [`manual-testing.md`](manual-testing.md#scenario-2-testing-inside-an-existing-desktop-nested-mode) |
| **Layer 4: Physical Hardware** | Direct KMS/DRM session from bare Linux TTY | Atomic modeset, page flips, multi-GPU affinity, physical outputs, PAM login | [`vt-drm-testing.md`](vt-drm-testing.md) |

---

## 3. Test Runner Standards & Tooling

The workspace standardizes on [`cargo-nextest`](https://nexte.st/) for fast,
parallelized test execution in local development and CI:

```bash
# Recommended: Install nextest once
cargo install cargo-nextest --locked

# Run tests using nextest
cargo nextest run --locked --workspace

# Fallback: Standard Cargo test runner (always supported)
cargo test --locked --workspace
```

For zero-native-dependency development (without compiling Optics C libraries),
use the [Flux-Free Test Pathway](manual-testing.md#scenario-0-flux-free-rapid-logic-verification)
to validate 13 workspace crates in sub-second time.

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
