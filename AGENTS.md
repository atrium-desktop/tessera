# Agent System Directives

## Documentation Governance (Protocol v6.0.0)

Before writing, modifying, or archiving any documentation, follow `docs/governance/documentation/core/index.md` and repository configuration in `.docgov.yml`.

### Machine Invariants (Pre-Submit Checklist)
- `[INV-LINT-01] Location Sanitization`: Never create arbitrary Markdown files at the repository root; only whitelisted root files are permitted.
- `[INV-LINT-02] Contributor Firewall`: Public docs (`docs/{tutorials,how-to,reference,explanation}/`) must NEVER link into internal docs (`docs/dev/**`).
- `[INV-LINT-03] Frontmatter & Lifecycle Integrity`: New ADRs in `docs/adr/` must declare standardized YAML frontmatter with valid status.
- `[INV-LINT-04] Code-Doc Sync`: Modifying monitored paths in `crates/` requires updating matching documentation in the same change.

### Cognitive & Architecture Protocols
- `[INV-AGENT-01] Negative Knowledge`: Every new ADR MUST contain a dedicated 'Rejected Alternatives & Negative Knowledge' section detailing why discarded options were not chosen.
- `[INV-AGENT-02] Context Routing & Chesterton's Fence`:
  - When generating code or proposing new solutions, NEVER recommend solutions from records with `status: superseded` or `status: rejected` (prevents resurrecting dead patterns).
  - When investigating root causes or refactoring, MUST retrieve `superseded` / `rejected` records as negative constraints (learn from historical failure modes).
- `[INV-AGENT-03] Blameless Postmortem`: Postmortems in `docs/dev/postmortems/` must analyze system defense failures and detection gaps without personal blame.
- `[INV-AGENT-04] Significance Threshold`: ADRs must satisfy the 3-Question Significance Test; localized refactoring or trivial fixes do not belong in `docs/adr/`.

---

Do not bypass Git hooks. When local Optics mode is active, the pre-commit hook
keeps the worktree-local `Cargo.lock` and `.cargo/config.toml` out of commits.

Local Optics mode is active when `.cargo/config.toml` contains
`[patch."https://github.com/ming2k/optics"]`. In that mode:

- Treat `.cargo/config.toml` and the path-resolved `Cargo.lock` as local
  worktree state. They must not be staged or committed.
- Leave `Cargo.lock` in the state produced by the local `../optics` patch.
  Do not restore the Git `source` fields merely to make `git status` clean;
  Cargo will remove them again, and doing so breaks the intended joint
  development workflow.
- Do not use `--no-verify`, force-add either file, or otherwise defeat the
  pre-commit hook. Check the staged diff and let the hook unstage either file
  if necessary.
- Update the canonical committed lockfile only after intentionally disabling
  local Optics mode to promote a released Optics revision.

Release commits are always canonical. A commit that bumps the
`workspace.package` version in `Cargo.toml` must be created with local
Optics mode disabled: regenerate the canonical `Cargo.lock`, stage it with
the version bump, and verify `cargo check --locked --workspace` passes. The
pre-commit hook refuses release-shaped commits in local Optics mode. Tag and
push a release only when CI on that commit is green; CI validates the
canonical lockfile on pushes to `main` and `dev`.

## Testing Rules & AI Behavioral Boundaries
- **Runner Tool**: Always use `cargo nextest run` instead of `cargo test` for unit and integration tests. Use `cargo test --doc` only when verifying documentation tests.
- **Tiered Verification (Do Not Over-test)**:
  - During intermediate edits, use `cargo check -p <crate>` for fast type checking, or `cargo nextest run -p <crate> --lib` / `cargo nextest run -E 'test(name)'` for targeted test validation.
  - Run full workspace tests (`cargo nextest run --workspace`) ONLY at the final delivery stage of a task.
- **Async Test Safety**:
  - Never write unbounded `rx.recv().await` on open channels. Always wrap with `tokio::time::timeout` or use non-blocking `try_recv()` to prevent infinite hangs/deadlocks.
  - Always use `#[tokio::test(start_paused = true)]` for tests involving timers, timeouts, or sleep to advance virtual time instantly.
- **Environment & State Isolation**:
  - Never hardcode ports (always bind to `:0` for ephemeral ports).
  - Never touch user home or global state paths; always isolate using `tempfile::tempdir()` and local sandbox roots.
