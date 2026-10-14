# Operational Workflows & Review Gates

This document defines operational lifecycles for repositories adopting `docgov`: the code-to-doc trigger matrix, pull request review gates, and zero-vendoring adoption.

---

## 1. Code-to-Documentation Trigger Matrix

Whenever a source code change is proposed, consult this matrix to determine mandatory documentation synchronization (`[INV-LINT-04]`):

| Code / System Change | Required Documentation Surface | Invariant Reference |
| :--- | :--- | :--- |
| **New public CLI flag, subcommand, or API** | `docs/reference/` + `README.md` (if primary entry path) | `[INV-LINT-04]` |
| **Breaking API, protocol, or schema change** | `CHANGELOG.md` + `docs/reference/` + `docs/adr/` | `[INV-LINT-03]`, `[INV-LINT-04]` |
| **Subsystem architectural refactoring** | `docs/adr/NNNN-*.md` (evaluated against 3-Question Test) | `[INV-AGENT-01]`, `[INV-AGENT-04]` |
| **Build system, test runner, or CI pipeline change** | `docs/dev/testing.md` | `[INV-LINT-04]` |
| **Contributor workflow or dev bootstrap change** | `docs/dev/setup.md` (or `CONTRIBUTING.md`) | `[INV-LINT-02]` |
| **New end-to-end user journey or capability** | `docs/dev/acceptance.md` + `docs/tutorials/` | `[INV-LINT-04]` |
| **Production outage, security incident, or data loss**| `docs/dev/postmortems/YYYY-MM-DD-*.md` | `[INV-AGENT-03]` |

---

## 2. Pull Request Review Gates

Maintainers and AI review agents must evaluate changes against two formal gates before merging:

### Gate A: Machine AST Gate (`INV-LINT`)
Deterministically enforced by automated CI via `docgov check`:
- [ ] No arbitrary Markdown files at the repository root (`[INV-LINT-01]`).
- [ ] No user-facing document contains relative links into `docs/dev/` (`[INV-LINT-02]`).
- [ ] All records in `docs/adr/` contain valid frontmatter with valid status enums (`[INV-LINT-03]`).
- [ ] Monitored code modifications have synchronized documentation in the same PR (`[INV-LINT-04]`).
- [ ] Delimiter boundary comments in AI agent configurations remain intact (`[INV-LINT-05]`).

### Gate B: Cognitive & Reasoning Gate (`INV-AGENT`)
Verified by human peer reviewers and AI reasoning self-checks:
- [ ] Proposed ADRs satisfy the 3-Question Significance Test (`[INV-AGENT-04]`).
- [ ] Proposed ADRs explicitly detail rejected alternatives and failure modes (`[INV-AGENT-01]`).
- [ ] Generative coding introduces no superseded or rejected architectural patterns (`[INV-AGENT-02]`).
- [ ] Post-incident reviews maintain blameless systemic focus without personal attribution (`[INV-AGENT-03]`).

---

## 3. Zero-Vendoring Repository Adoption

Repositories adopt `docgov` governance without vendoring code or duplicating upstream specifications:

1. **Initialize Configuration**:
   Create `.docgov.yml` at the repository root defining project settings, monitored paths, and active profiles.
2. **Anchor AI Directives**:
   Embed the canonical docgov directives block within `AGENTS.md` (or run `docgov init` / `docgov fix`).
3. **Verify Locally (On-Demand)**:
   Run verification via the zero-install CLI before major doc deliveries:
   ```bash
   docgov check
   ```
4. **Enforce in CI**:
   Add a lightweight CI step running `docgov check` on pull requests to guarantee deterministic repository compliance.
