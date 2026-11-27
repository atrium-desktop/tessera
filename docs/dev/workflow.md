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
| **Subsystem architectural refactoring or crate boundary change** | New ADR under `docs/adr/`, plus `docs/dev/architecture.md` and `xtask/dependency-policy.toml` when edges move |
| **Chrome, control surface, or user-visible workflow change** | The matching `docs/how-to/` guide and `docs/dev/design/` where visual language is affected |
| **Build system, test runner, or CI pipeline change** | `docs/dev/setup.md`, `docs/dev/standards/testing.md`, and this document |
| **Contributor bootstrap or local Optics workflow change** | `docs/dev/setup.md` or `docs/dev/optics-local-development.md` |
| **New end-to-end user journey or capability** | `CHANGELOG.md`, a `docs/how-to/` guide, and `docs/tutorials/` when it changes first-run flow |
| **Production outage, security incident, or data loss** | A postmortem following [Postmortem Standards](standards/postmortems.md), and a triage page when the symptom class is recurring |
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
| Package conformity | `cargo run -p xtask -- package-conformity` | Workspace lint and dependency standards; ADR-0263 package naming and placement | Yes |
| Dependency boundaries | `cargo run -p xtask -- check-boundaries` | Capability edges from `dependency-policy.toml` | Yes |
| Bootstrap admission | `cargo run -p xtask -- check-bootstrap-admission` | ADR-0147 process-entry-point rule | Yes |
| Vtable admission | `cargo run -p xtask -- check-vtables` | ADR-0193 metadata-derived vtable sizing in `tessera-wayland` | Yes |
| ADR registry | `cargo run -p xtask -- check-adrs` | Unique ids, frontmatter, index sync, and lineage-link resolution across `docs/adr/` | Yes |
| Panic discipline | `cargo run -p xtask -- check-panics` | ADR-0195: no poisonable-lock panics, no new panic sites | Yes |
| System files | `cargo run -p xtask -- check-system-files` | ADR-0241: `system/` mirrors the install tree; manifest, file modes, and documented `system/` paths agree | Yes |
| Install manifest | `cargo run -p xtask -- check-install-manifest` | Every binary the workspace builds is installed by `scripts/install.sh` and listed in the packaging manifest, and vice versa | Yes |
| Documentation | `cargo run -p xtask -- check-docs` | Relative links and anchors resolve, no public page links into `docs/dev/`, no retired repository owner, no unresolved version placeholder | Yes |
| Desktop entries | `desktop-file-validate system/share/applications/*.desktop` | freedesktop Desktop Entry specification | Yes |
| Shell scripts | `shellcheck -x system/bin/* scripts/*.sh` | The session entry point, installer, and developer run scripts | Yes |
| Supply chain | `cargo deny check` | Advisories, license allowlist, wildcard requirements, and dependency sources (`deny.toml`) | Yes |
| Lockfile validity | `cargo metadata --locked` | Canonical `Cargo.lock` matches the manifests | Yes |
| Tests | `cargo nextest run --locked --workspace` | Unit, integration, and end-to-end tests | Yes |
| Motion purity | `cargo clippy -p tessera-shell --all-targets` | ADR-0174: `f32::exp`/`f64::exp` are `disallowed-methods` in `crates/tessera-shell/clippy.toml`; only `widgets::motion` may opt out | Yes (full-workspace clippy) |

The CI column reflects `.github/workflows/ci.yml`. Every mechanical gate runs in CI.

### Gate B: Reasoning Gates (Reviewer-Verified)

- [ ] A proposed ADR satisfies the 3-Question Significance Test
  ([ADR Standards](standards/adr.md)).
- [ ] A proposed ADR explicitly details rejected alternatives and failure modes.
- [ ] Declared invariant tags have a real enforcement mechanism
  ([Invariant Standards](invariants.md)).
- [ ] New implementation code introduces no superseded or rejected patterns.
- [ ] Code changes have synchronized documentation per the matrix above.
- [ ] Public documentation contains no links into `docs/dev/**`
  ([Taxonomy](standards/taxonomy.md)).

---

## 3. Commit and Release Hygiene

1. **Commit Shape**: Use descriptive, scoped messages in the repository style:
   `feat(shell): …`, `fix(render): …`, `docs(dev): …`, `chore(release): …`.
   Name the governing ADR in the body when one applies.
2. **Release Commits**: A commit that bumps `workspace.package.version` in
   `Cargo.toml` must be created with local Optics mode disabled, staging the
   regenerated canonical `Cargo.lock` together with the version bump, and
   verified with `cargo check --locked --workspace`. See
   [Local Optics Development](optics-local-development.md) for the
   worktree mechanics.
3. **Changelog Integrity**: Add user-visible features, breaking changes, and
   migrations to `CHANGELOG.md` under `## [Unreleased]`, following
   [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
4. **Clean Git State**: Verify no untracked experimental files, session exports,
   or temporary debug assets remain in the tree before hand-off.
