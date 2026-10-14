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

<!-- BEGIN DOCGOV DIRECTIVES -->
<!-- CRITICAL: MANAGED BY DOCGOV. DO NOT MODIFY, MOVE, OR REMOVE DELIMITER TAGS. -->
## Documentation Governance Directives <!-- @docgov-anchor -->

All modifications must adhere to repository invariants and pass `docgov check`.

### 1. Hard Constraints (Zero-Tolerance Invariants)
- `[INV-LINT-01] Root Whitelist`: Never create arbitrary Markdown files at the repository root. Only whitelisted files (`README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `AGENTS.md`, `LICENSE.md`, `SECURITY.md`) are allowed.
- `[INV-LINT-02] Scope Boundary`: Public documentation (`docs/{tutorials,how-to,reference,explanation}/`) MUST NEVER link to internal contributor docs (`docs/dev/**`).
- `[INV-LINT-03] Frontmatter Schema`: Every ADR in `docs/adr/` must declare valid YAML frontmatter (`id`, `title`, `status`, `date`).
- `[INV-LINT-04] Code-Doc Sync`: Modifying monitored paths in `src/` requires updating corresponding `docs/` in the same change.
- `[INV-LINT-05] Directives Integrity`: Never remove, reformat, or strip boundary delimiters (`<!-- BEGIN/END DOCGOV DIRECTIVES -->`) when modifying agent configuration files.

### 2. Architectural Decision Standards (Reasoning Principles)
- `[INV-AGENT-01] Rejected Alternatives`: Every new ADR MUST document discarded options and their specific failure modes.
- `[INV-AGENT-02] Historical State Routing`:
  - New implementations: DO NOT use `superseded` or `rejected` ADRs as active patterns.
  - Refactoring & investigations: MUST inspect `superseded` ADRs to learn from historical failure modes.
- `[INV-AGENT-03] Blameless Postmortems`: Incident postmortems must focus strictly on timeline, detection gaps, and systemic defense failures, prohibiting personal human blame.
- `[INV-AGENT-04] Architectural Significance Threshold`: Only author ADRs for decisions with high reversal cost or cross-boundary blast radius (skip localized or trivial refactorings).

### 3. Reference Specifications (Authoritative Standards)
Before drafting or restructuring documentation, inspect the local governance specifications:
- Taxonomy & Layout (Diátaxis): `docs/governance/documentation/core/taxonomy.md`
- System Invariants: `docs/governance/documentation/core/invariants.md`
- Style & Hyperlink Rules: `docs/governance/documentation/core/style.md`
- ADR & RFC Standards: `docs/governance/documentation/profiles/architecture/adr.md`
- Quality & Verification: `docs/governance/documentation/profiles/validation/testing.md`

### 4. Verification & Advisory Gate (On-Demand / Lazy)
`docgov` is an on-demand verification tool, not an unconditional mandatory step for every turn. Apply a lazy strategy:
- **Skip (No-op)**: Pure discussions, architecture Q&A, intermediate edits, or internal code changes that do not touch documentation or governance boundaries.
- **Run (Targeted Verification)**: Execute `docgov check` before final hand-off only when modifying `docs/`, `spec/`, ADRs (`docs/adr/`), or files affecting code-doc sync (`[INV-LINT-04]`). Prioritize static self-review over blind tool invocation.
<!-- END DOCGOV DIRECTIVES -->
