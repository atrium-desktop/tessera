# Contributor Documentation

Contributor-only documentation for tessera. User-facing material lives under
the other `docs/` sections; see the [documentation index](../index.md).

## Pages

| Page | Purpose |
|------|---------|
| [Setup](setup.md) | Toolchain, dependencies, and environment preparation |
| [Build & Run Guide](build-and-run.md) | Compiling Tessera and starting it nested, on VT/DRM, or as a surface preview |
| [Verification](verification/index.md) | Running Tessera during development and checking changes by hand: the manual verification playbook, surface and VT/DRM procedures, and hardware acceptance gates |
| [Support Matrix](verification/support-matrix.md) | What is verified right now, on which environment, and when |
| [Local Optics Development](optics-local-development.md) | Developing against a local Optics checkout (via linked worktree), commit protection, and release promotion |
| [Miracast Operational Runbook](runbooks/miracast.md) | Operational guide, real-world hardware verification, WFD protocol quirks, and diagnostic playbook |
| [Nested Backend Development](nested-backend.md) | Successful-build process replacement, scene restoration, and backend validation inside a Wayland session |
| [First-Party Application Development](first-party-applications.md) | Installation staging, application discovery, and focused test behavior for standalone system applications |
| [Distribution Packaging](packaging.md) | Reproducible build inputs, complete install manifest, package integration, and validation |
| [How to Validate KMS Overlay Offload](../how-to/kms-overlay-validation.md) | Hardware validation for the complete-set overlay candidate, `TEST_ONLY`, deactivation paths, and failure triage |
| [Project Layout](project-layout.md) | Source tree map and ownership boundaries |
| [Machine Gates](workflow.md#gate-a-mechanical-gates) | The xtask checks and the CI lanes that run them |
| [Architecture Topology](architecture.md) | Concentric workspace layers, capability flow, and architectural red-lines |
| [System Invariants](invariants.md) | Non-negotiable structural red-lines, admissibility matrix, and tag discipline |
| [Error Handling & Panic Discipline](error-handling.md) | Distinguishing recoverable errors from panics, lock poisoning recovery, and ratchet checks |
| [PR Workflow & Review Gates](workflow.md) | Code-to-documentation trigger matrix, xtask validation gates, and commit hygiene |
| [Design Language](design/index.md) | System-wide foundations, components, patterns, guidelines, tooling, and product surface specifications |
| [Observability](observability.md) | The tracing-based logging seam, log levels, and `RUST_LOG` workflow |
| [Development Environment Variables](environment-variables.md) | Contributor environment overrides for backend selection, nested UI debugging, asset dumps, and XDG isolation |
| [Issue Triage](triage/index.md) | Case-based bug-attribution know-how: invariants, diagnostic recipes, and fix ownership |
| [Engineering Standards](standards/README.md) | Universal meta-standards for taxonomy, ADRs, RFCs, testing, verification, runbooks, and postmortems |
