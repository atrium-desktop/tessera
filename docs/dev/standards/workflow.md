# Operational Workflows & Review Gates

This document defines the code-to-documentation trigger matrix, the pull request
review gates, and release hygiene for Tessera.

---

## 1. Code-to-Documentation Trigger Matrix

Whenever a source change is proposed, use this matrix to determine the
documentation that must move in the same change:

| Code / System Change | Required Documentation Surface |
| :--- | :--- |
| **New or changed configuration key** | `docs/reference/config.md` (+ `docs/reference/settings.md` for settings routes) |
| **New or changed CLI command or flag** | `docs/reference/cli.md` and `docs/reference/keyboard-shortcuts.md` where a global binding is added |
| **Wire protocol, IPC message, or audit vocabulary change** | `docs/reference/ipc.md`, `CHANGELOG.md`, and an ADR when the change alters a boundary |
| **Render, scanout, damage, or plane-role behavior** | `docs/reference/rendering.md` |
| **Subsystem architectural refactoring or crate boundary change** | New ADR under `docs/adr/`, plus `docs/dev/standards/architecture.md` and `tooling/xtask/dependency-policy.toml` when edges move |
| **Chrome, control surface, or user-visible workflow change** | The matching `docs/how-to/` guide and `docs/dev/design/` where visual language is affected |
| **Build system, test runner, or CI pipeline change** | `docs/dev/setup.md`, `docs/dev/surface-testing.md`, and this document |
| **Contributor bootstrap or worktree workflow change** | `docs/dev/setup.md` or `docs/dev/cross-repository-development.md` |
| **New end-to-end user journey or capability** | `CHANGELOG.md`, a `docs/how-to/` guide, and `docs/tutorials/` when it changes first-run flow |
| **Production outage, security incident, or data loss** | A postmortem following [Postmortem Standards](postmortems.md), and a triage page when the symptom class is recurring |
| **New invariant tag** | The declaring ADR's `### Invariants & Behavioral Boundaries` section, following [Invariant Standards](invariants.md) |

Every entry in the matrix also produces a `CHANGELOG.md` line when it is
user-visible.

---

## 2. Pull Request Review Gates

### Gate A: Mechanical Gates

| Check | Command | Covers | Runs in CI |
| :--- | :--- | :--- | :--- |
| Format | `cargo fmt --all -- --check` | Rust formatting | Yes |
| Spelling | `crate-ci/typos` | Typos across the tree, with `typos.toml` allowances | Yes |
| Package conformity | `cargo run -p xtask -- package-conformity` | Workspace lint and dependency standards | Yes |
| Dependency boundaries | `cargo run -p xtask -- check-boundaries` | Capability edges from `dependency-policy.toml` | Yes |
| Bootstrap admission | `cargo run -p xtask -- check-bootstrap-admission` | ADR-0147 process-entry-point rule | Yes |
| Lockfile validity | `cargo metadata --locked` | Canonical `Cargo.lock` matches the manifests | Yes |
| Tests | `cargo nextest run --locked --workspace` | Unit, integration, and doc tests | Yes |
| Motion purity | `cargo run -p xtask -- check-motion` | ADR-0174 zero-hand-rolled-motion-math rule in `tessera-shell` | Local gate — run when chrome motion code moves |

The CI column reflects `.github/workflows/ci.yml`; the local-only gate is
enforced by reviewers on the corresponding changes.

### Gate B: Reasoning Gates (Reviewer-Verified)

- [ ] A proposed ADR satisfies the 3-Question Significance Test
  ([ADR Standards](adr.md)).
- [ ] A proposed ADR explicitly details rejected alternatives and failure modes.
- [ ] Declared invariant tags have a real enforcement mechanism
  ([Invariant Standards](invariants.md)).
- [ ] New implementation code introduces no superseded or rejected patterns.
- [ ] Code changes have synchronized documentation per the matrix above.
- [ ] Public documentation contains no links into `docs/dev/**`
  ([Taxonomy](taxonomy.md)).

---

## 3. Commit and Release Hygiene

1. **Commit Shape**: Use descriptive, scoped messages in the repository style:
   `feat(shell): …`, `fix(render): …`, `docs(dev): …`, `chore(release): …`.
   Name the governing ADR in the body when one applies.
2. **Release Commits**: A commit that bumps `workspace.package.version` in
   `Cargo.toml` must be created with local Optics mode disabled, staging the
   regenerated canonical `Cargo.lock` together with the version bump, and
   verified with `cargo check --locked --workspace`. See
   [Cross-Repository Development](../cross-repository-development.md) for the
   worktree mechanics.
3. **Changelog Integrity**: Add user-visible features, breaking changes, and
   migrations to `CHANGELOG.md` under `## [Unreleased]`, following
   [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
4. **Clean Git State**: Verify no untracked experimental files, session exports,
   or temporary debug assets remain in the tree before hand-off.
