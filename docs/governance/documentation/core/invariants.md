# System Invariants

This document codifies the non-negotiable architectural and documentation invariants across repositories adopting `docgov`.

Every invariant carries a unique, canonical identifier partitioned into two operational tiers:
- **`[INV-LINT-*]`**: Deterministic Machine Layer, verified by automated AST linters and CI pipelines.
- **`[INV-AGENT-*]`**: Cognitive & Agent Protocol Layer, governing LLM context routing, negative knowledge, and architectural reasoning.

---

## 1. Deterministic Machine Layer (`INV-LINT`)

Enforced deterministically by compiler/AST engines (`docgov check`). Violations block CI and pull requests.

- **`[INV-LINT-01]` Location Sanitization (Root Whitelist)**:
  Arbitrary Markdown files are prohibited at the repository root. Only explicitly whitelisted entries (`README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `AGENTS.md`, `LICENSE.md`, `SECURITY.md`) may reside at the root level. All other documentation must route through `docs/`.
- **`[INV-LINT-02]` Contributor Firewall**:
  Public user-facing documentation (`docs/tutorials/`, `docs/how-to/`, `docs/reference/`, `docs/explanation/`) must never contain relative links into internal contributor spaces (`docs/dev/**`). Contributor documentation may freely link outward into public documentation.
- **`[INV-LINT-03]` Frontmatter Schema & Lifecycle Integrity**:
  Every record in `docs/adr/` must declare valid YAML frontmatter containing mandatory fields (`id`, `title`, `status`, `date`). The `status` field must be an enum member of `[draft, accepted, superseded, rejected]`. Records marked `superseded` must supply a valid `superseded_by` pointer. Retired records evolve in-place without physical file relocation.
- **`[INV-LINT-04]` Code-Doc Synchronization Trigger**:
  Any Pull Request modifying monitored source code surfaces (e.g., `src/api/**`, `cli/src/**`) must synchronize corresponding documentation surfaces in the same change, as verified by Git diff analysis.
- **`[INV-LINT-05]` Agent Directives Boundary Integrity**:
  Target AI assistant instruction configurations (e.g., `AGENTS.md`) must contain the docgov directives block enclosed within exact boundary comments (`<!-- BEGIN DOCGOV DIRECTIVES -->` and `<!-- END DOCGOV DIRECTIVES -->`). Delimiter comments must never be removed, reformatted, or stripped.

---

## 2. Cognitive & Agent Protocol Layer (`INV-AGENT`)

Enforced by AI assistant directives (`AGENTS.md`), reasoning chains, and human architectural peer reviews.

- **`[INV-AGENT-01]` Negative Knowledge Mandate**:
  Every Architectural Decision Record (ADR) must contain a dedicated "Rejected Alternatives & Negative Knowledge" section detailing which alternatives were considered and why they failed. Decisions without explicit failure modes must be rejected.
- **`[INV-AGENT-02]` Context Routing & Historical State**:
  - *Generative Isolation*: When generating code or proposing new implementations, AI agents must filter out documents with `status: superseded` or `status: rejected` to prevent resurrecting dead patterns.
  - *Forensic Retrieval*: When prompted for refactoring, historical rationale, or root-cause investigation, AI agents must actively retrieve superseded/rejected records and cite their documented failure modes as negative constraints.
- **`[INV-AGENT-03]` Blameless Postmortem Structure**:
  Post-incident reviews in `docs/dev/postmortems/` must strictly focus on timeline reconstruction, detection gaps, and systemic defense-in-depth failures. Attribution of personal human error or developer blame is strictly prohibited.
- **`[INV-AGENT-04]` Architectural Significance Threshold**:
  An ADR must only be authored for decisions satisfying at least two criteria of the 3-Question Significance Test (high reversal cost, cross-boundary blast radius, or generating binding invariants). Trivial implementation details, localized function refactorings, or ephemeral workarounds must never be admitted to `docs/adr/`.
