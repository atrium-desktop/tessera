# Contributor Documentation

Contributor-only documentation for tessera. User-facing material lives under
the other `docs/` sections; see the [documentation index](../index.md).

## Pages

| Page | Purpose |
|------|---------|
| [Setup](setup.md) | Toolchain, dependencies, and build/run |
| [Manual Testing Playbook](testing/manual-testing.md) | Scenario-based manual testing recipes, nested desktop execution, client injection, and smoke checklists |
| [Testing Directory](testing/index.md) | Complete directory map of testing playbooks, scenario runbooks, and test architecture standards |
| [Tessera and Optics Cross-Repository Development](cross-repository-development.md) | Worktree-isolated local patches, commit protection, and release promotion |
| [Miracast Operational Runbook](runbooks/miracast.md) | Operational guide, real-world hardware verification, WFD protocol quirks, and diagnostic playbook |
| [Nested Backend Development](nested-backend.md) | Successful-build process replacement, scene restoration, and backend validation inside a Wayland session |
| [Surface Testing](testing/surface-testing.md) | Test model and code conventions for shell surfaces: command-panel and lock-screen previews, nested integration, agent pixel capture, and physical validation |
| [First-Party Application Development](first-party-applications.md) | Installation staging, application discovery, and focused test behavior for standalone system applications |
| [Distribution Packaging](packaging.md) | Reproducible build inputs, complete install manifest, package integration, and validation |
| [VT/DRM Manual Testing](testing/vt-drm-testing.md) | Run and try the compositor on real display and input hardware |
| [Project Layout](project-layout.md) | Source tree map and ownership boundaries |
| [Architecture Topology](architecture.md) | Concentric workspace layers, capability flow, and architectural red-lines |
| [System Invariants](invariants.md) | Non-negotiable structural red-lines, admissibility matrix, and tag discipline |
| [Error Handling & Panic Discipline](error-handling.md) | Distinguishing recoverable errors from panics, lock poisoning recovery, and ratchet checks |
| [PR Workflow & Review Gates](workflow.md) | Code-to-documentation trigger matrix, xtask validation gates, and commit hygiene |
| [Design Language](design/index.md) | System-wide foundations, components, patterns, guidelines, tooling, and product surface specifications |
| [Observability](observability.md) | The tracing-based logging seam, log levels, and `RUST_LOG` workflow |
| [Development Environment Variables](environment-variables.md) | Contributor environment overrides for backend selection, nested UI debugging, asset dumps, and XDG isolation |
| [Issue Triage](triage/index.md) | Case-based bug-attribution know-how: invariants, diagnostic recipes, and fix ownership |
| [Engineering Standards](standards/README.md) | Universal meta-standards for taxonomy, ADRs, RFCs, runbooks, testing, and postmortems |
