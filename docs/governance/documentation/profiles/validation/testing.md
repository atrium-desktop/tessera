# Entity: Programmatic Implementation Testing

Intent: Document automated test runner commands, suite categories, environment prerequisites, and fixture topologies to guarantee reproducible mechanical verification.

---

## 1. Intent & Scope

Implementation testing provides fast inner-loop feedback for developers:
- **Inner-Loop Focus**: Verifies that components, algorithms, and interfaces behave correctly in isolation and integration.
- **Deterministic Execution**: Tests must run deterministically on clean developer workstations and CI runners without undeclared external state.

---

## 2. Normative Invariants

- **Deterministic Reproducibility**: Automated test commands documented in `docs/dev/testing.md` must execute predictably without unversioned network dependencies, hidden local caches, or undeclared environment variables.
- **`[INV-LINT-04]` Code-Doc Synchronization**: When test runner commands, environment flags, or CI test topologies change, `docs/dev/testing.md` must be updated in the same Pull Request.
- **`[INV-LINT-02]` Contributor Firewall**: `docs/dev/testing.md` resides in `docs/dev/` behind the contributor firewall; user-facing documentation must not link directly into it.

---

## 3. Document Structure Requirements

Every `docs/dev/testing.md` document must provide:
1. **Quick Test Commands**: Direct commands to execute unit, integration, and full suites.
2. **Test Suite Matrix**: Tabular breakdown of scopes, runtimes, commands, and CI gates.
3. **Environment & Fixture Isolation**: Setup requirements, mock strategies, and teardown behavior.

---

## 4. Authoritative Testing Template

```markdown
# Testing Guide

This document defines automated test commands, suite categories, and verification procedures for this repository.

---

## 1. Quick Test Commands

Run test suites from the repository root:

```bash
# Run standard workspace test suite
cargo test --workspace

# Run targeted unit tests
cargo test -p <package> --lib

# Run integration tests
cargo test --test <integration_test_name>

# Run linting with warnings treated as errors
cargo clippy --workspace --all-targets -- -D warnings
```

---

## 2. Test Suite Matrix

| Suite | Scope | Execution Command | Target Runtime | CI Enforcement |
| :--- | :--- | :--- | :--- | :--- |
| **Unit** | Isolated functions and modules | `cargo test --lib` | < 5s | Mandatory (blocks PR) |
| **Integration** | Multi-component subsystem interactions | `cargo test --tests` | < 30s | Mandatory (blocks PR) |
| **E2E / CLI** | Full binary execution and filesystem outputs | `cargo test --test cli_tests` | < 2m | Mandatory (blocks PR) |
| **Lint & Typecheck** | AST invariants, style, compiler warnings | `cargo clippy -- -D warnings` | < 15s | Mandatory (blocks PR) |

---

## 3. Environment & Fixture Management

- **Isolated Temporary Directories**: Tests that perform filesystem I/O must allocate isolated temporary directories (e.g., via `tempfile`) and clean them up upon completion.
- **Hermetic Mocking**: Network requests and external system dependencies must be hermetically mocked or stubbed during unit and integration test runs.
- **Zero Undeclared State**: Tests must not rely on pre-existing host environment variables unless explicitly documented under Prerequisites.
```
