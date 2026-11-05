# Contributing to Tessera

Thank you for your interest in contributing to **tessera** (*autonomous surface shell*)!

## Quick Orientation

- **Developer Setup**: Read [Development Setup](docs/dev/setup.md) for build prerequisites, system dependencies, and compiling sibling projects.
- **Distribution Packaging**: Read [Distribution Packaging](docs/dev/packaging.md) for reproducible source preparation, artifact ownership, and system integration.
- **Architecture**: Review [Architecture Explanation](docs/explanation/architecture.md), [Architecture Topology](docs/dev/architecture.md), and [Architecture Decision Records (ADRs)](docs/adr/index.md) before proposing structural changes.
- **Engineering Standards**: Read [Engineering Documentation & Collaboration Standards](docs/dev/standards/README.md) for taxonomy, ADR authoring, RFCs, runbooks, testing, and postmortems.
- **Workflow & Invariants**: Consult [PR Workflow](docs/dev/workflow.md), [System Invariants](docs/dev/invariants.md), and [Error Handling](docs/dev/error-handling.md) for PR review gates and implementation discipline.

## Workflow & Guidelines

1. **Branching & Commits**: Write clear, descriptive commit messages following the project's commit history style (e.g. `feat: ...`, `fix: ...`, `refactor: ...`).
2. **Code Style & Formatting**:
   - Run `cargo fmt --all` before committing.
   - Ensure `cargo clippy --locked` passes without warnings on workspace crates.
3. **Tests**: Add unit or integration tests for new functionality where
   applicable (`cargo test --locked`).
4. **Documentation**:
   - Updates to public interfaces or schemas must update corresponding files in `docs/reference/` and `CHANGELOG.md`.
   - Structural design decisions must be recorded in a new ADR under `docs/adr/` starting from `docs/adr/template.md`.

## Documentation & Governance Policy Note

Documentation and governance meta-standards live in `docs/dev/standards/` as a
flat, portable suite — taxonomy, ADRs, RFCs, runbooks, testing, style, and
postmortems. System-specific architecture, invariants, error handling, and PR
workflows live directly under `docs/dev/`. AI assistants and contributors may read
them, but updates to the standards themselves are reserved for project
maintainers.

The machine-checked gates these standards describe are implemented by
`tooling/xtask` and the CI workflow: package conformity, crate dependency
boundaries, bootstrap admission, chrome motion purity, formatting, and typos.
