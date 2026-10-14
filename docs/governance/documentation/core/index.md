# Documentation Governance Core

Universal documentation governance standard and verification protocol for software engineering repositories.

---

## The Four Core Primitives

Documentation governance is organized around four orthogonal primitives:

| Core Document | Primitive | Description |
| :--- | :--- | :--- |
| [Taxonomy](taxonomy.md) | **Governance Coordinates** | 4-Dimensional system: Temperature, Lifecycle, Audience Isolation, Diátaxis |
| [Invariants](invariants.md) | **Rule Constitution** | Deterministic AST rules (`INV-LINT`) and Agent cognitive protocols (`INV-AGENT`) |
| [Workflow](workflow.md) | **Operational Lifecycle** | Code-to-doc trigger matrix, PR review gates, and zero-vendoring adoption |
| [Style Guide](style.md) | **Syntax & Formatting** | Voice, capitalization, heading hierarchy, relative links, and document patterns |

---

## Domain Capability Profiles

Repositories activate domain capabilities declared in `.docgov.yml`. Each profile provides vertical entities:

| Domain Profile | Focus | Vertical Entities | Location |
| :--- | :--- | :--- | :--- |
| **Architecture** | Architectural decision records, living blueprints, and proposals | `adr.md`, `living-snapshot.md`, `rfc.md` | `profiles/architecture/` |
| **Validation** | Real user journey acceptance and programmatic test verification | `acceptance.md`, `testing.md` | `profiles/validation/` |
| **Operations** | Operational knowledge layering and incident analysis | `postmortem.md` | `profiles/operations/` |

---

## Quick Navigation

1. **Adopting governance**: Consult [Workflow: Zero-Vendoring Adoption](workflow.md#3-zero-vendoring-repository-adoption).
2. **Deciding where content lives**: Consult [Taxonomy](taxonomy.md).
3. **Checking hard rules**: Review the codified [Invariants](invariants.md).
4. **Reviewing a Pull Request**: Follow [Workflow: PR Review Gates](workflow.md#2-pull-request-review-gates).
5. **Evaluating code change impact**: Check [Workflow: Trigger Matrix](workflow.md#1-code-to-documentation-trigger-matrix).
