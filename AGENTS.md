# Agent Directives

## 1. Operational Guardrails (Hard Constraints)
- **Prohibited Git Commands**: NEVER execute `git stash`, `git branch`, or `git reset`.
- **Command Termination**: All commands MUST be finite and self-terminating. Never start background servers or unbounded loops without a strict timeout.
- **Hook Integrity**: Do not bypass Git hooks (`--no-verify`), and do not force-add files the hooks exclude.

## 2. Core Mental Model & Subsystem Boundaries
- **Composition Root**: `tessera` is the sole public binary and owns the display engine lifecycle (Wayland host, GPU device, presentation surface, canvas, renderer) with strict drop order.
- **Protocol Purity**: `tessera-protocol` carries versioned wire DTOs and audit vocabulary only — no sockets, filesystem persistence, or GPU types. Unix framing and connections live in `tessera-ipc`.
- **Shell Firewall**: `tessera-shell` components emit typed events; they never mutate the server, open sockets, or write configuration. Chrome code cannot depend on Wayland or renderer internals.
- **Domain Isolation**: `tessera-desktop` and `tessera-authority` cannot depend on platform, storage, transport, or graphics implementations. `tessera-primitives` stays foundational and must never become a miscellaneous helper crate.
- **Native Build Ownership**: Native-library generation has exactly one build owner; never use an intermediate crate as a re-export conduit.

## 3. Engineering Discipline
- **Mechanisms Over Exhortations**: Rely on Rust compiler typestates, private visibility, Cargo dependency firewalls (`cargo run -p xtask -- check-boundaries`), and automated tests to guarantee invariants. Never rely on natural-language comments alone.
- **Anti-Inflation**: Do not mint pseudo-invariant tags (`INV-UI`, `INV-FOOTER`, …). Model states via typed enums and styles via `tessera-design` constants. Declare a genuine invariant tag once, in the ADR that establishes it.
- **Single Source of Truth**: Fix the root cause, never a second copy of the math. When a subsystem resolves geometry, motion, or configuration, every caller consumes that resolution rather than re-inlining it.
- **Clean Epistemic Lineage**: References must resolve to canonical in-repo ADRs (`docs/adr/0001`+). Never invent external `ADR-01xx`/`ADR-02xx` numbers; cite Optics ADRs only by full URL.
- **Atomic Verification**: Verify changes with `cargo check -p <crate>` while iterating, and relevant crate test suites before declaring a task complete.

## 4. On-Demand Standards Routing
Do not guess repository rules or rely on stale assumptions. Consult authoritative specifications in `docs/dev/standards/` as needed:
- Workspace Topology & Red-Lines: `docs/dev/standards/architecture.md`
- Invariant & Constraint Policy: `docs/dev/standards/invariants.md`
- ADR Authoring & Lifecycle: `docs/dev/standards/adr.md`
- RFC Standards: `docs/dev/standards/rfc.md`
- Documentation Taxonomy & Firewalls: `docs/dev/standards/taxonomy.md`
- Markdown & Hyperlink Style: `docs/dev/standards/style.md`
- PR Gates & Code-Doc Sync: `docs/dev/standards/workflow.md`
- Blameless Postmortems: `docs/dev/standards/postmortems.md`
- Testing Model & Execution: `docs/dev/surface-testing.md`, `docs/dev/vt-drm-testing.md`

## 5. Documentation Governance

Before writing, modifying, or archiving any documentation, follow
`docs/dev/standards/README.md` and the standards routed in section 4.

### Machine Invariants (Pre-Submit Checklist)
- `[INV-LINT-01] Location Sanitization`: Never create arbitrary Markdown files at the repository root; only the whitelisted root files are permitted.
- `[INV-LINT-02] Contributor Firewall`: Public docs (`docs/{tutorials,how-to,reference,explanation}/`) must NEVER link into internal docs (`docs/dev/**`).
- `[INV-LINT-03] Frontmatter & Lifecycle Integrity`: New ADRs in `docs/adr/` must declare standardized YAML frontmatter with valid status, and are registered in `docs/adr/index.md`.
- `[INV-LINT-04] Code-Doc Sync`: Modify documentation in the same change as the code it describes, per the trigger matrix in `docs/dev/standards/workflow.md`.

### Cognitive & Architecture Protocols
- `[INV-AGENT-01] Negative Knowledge`: Every new ADR MUST contain a dedicated 'Rejected Alternatives & Negative Knowledge' section detailing why discarded options were not chosen.
- `[INV-AGENT-02] Context Routing & Chesterton's Fence`:
  - When generating code or proposing new solutions, NEVER recommend solutions from records with `status: superseded` or `status: rejected` (prevents resurrecting dead patterns).
  - When investigating root causes or refactoring, MUST retrieve `superseded` / `rejected` records as negative constraints (learn from historical failure modes).
- `[INV-AGENT-03] Blameless Postmortem`: Postmortems must analyze system defense failures and detection gaps without personal blame.
- `[INV-AGENT-04] Significance Threshold`: ADRs must satisfy the 3-Question Significance Test; localized refactoring or trivial fixes do not belong in `docs/adr/`.

### Verification & Advisory Gate (On-Demand / Lazy)
The gates above are enforced by CI and by static self-review, not by a mandatory per-turn invocation:
- **Skip (No-op)**: Pure discussions, architecture Q&A, intermediate edits, or internal code changes that do not touch documentation.
- **Run (Targeted Verification)**: Before final hand-off, re-check links and frontmatter in any changed `docs/`, ADR, or `spec/` file, and confirm each code-doc trigger row is satisfied. Run `cargo run -p xtask -- check-boundaries` and `check-motion` when crate edges or chrome motion code move.

---

## Optics Cross-Repository Development

When local Optics mode is active, the pre-commit hook keeps the worktree-local
`Cargo.lock` and `.cargo/config.toml` out of commits.

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

See `docs/dev/cross-repository-development.md` for the dual-worktree workflow.

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
- **Hardware Boundaries**: Do not claim a physical DRM/KMS, PAM, or VT behavior from a nested or preview run. Finish on real hardware per `docs/dev/vt-drm-testing.md` when the change crosses a hardware or authentication boundary.
