# Documentation Governance Core

Universal meta-governance protocol and verification standard for software engineering repositories.

This specification implements the **Clean-Break Architecture** (Protocol v5.0.0): a universal, project-neutral semantic core coupled with pluggable, self-contained vertical domain profiles.

---

## The Four Core Primitives

Documentation governance is organized into four orthogonal core primitives:

| Core Document | Primitive | Description |
| :--- | :--- | :--- |
| [Taxonomy](taxonomy.md) | **Spatial Tensor** | 4-Dimensional coordinate system (Temperature, Lifecycle, Audience, Cognitive Mode) |
| [Invariants](invariants.md) | **Rule Constitution** | Codified, numbered non-negotiable negative and positive constraints (`INV-*`) |
| [Workflow](workflow.md) | **Operational Lifecycle** | Code-to-doc trigger matrix, PR review gates, standard intake SOP, and adoption |
| [Style Guide](style.md) | **Presentation Syntax** | Voice, capitalization, heading hierarchy, relative links, and formatting rules |

---

## Domain Capability Profiles

Adopting repositories activate domain profiles in `contracts.md`. Each profile contains fully self-contained vertical entities:

| Domain Profile | Focus | Vertical Entities | Directory |
| :--- | :--- | :--- | :--- |
| **Architecture** | Architectural decision records, living blueprints, and pre-decision proposals | `adr.md`, `living-snapshot.md`, `rfc.md` | `profiles/architecture/` |
| **Validation** | Real user journey acceptance and automated test verification | `acceptance.md`, `testing.md` | `profiles/validation/` |
| **Operations** | Operational knowledge layering and incident analysis | `postmortem.md` | `profiles/operations/` |

---

## Quick Navigation

1. **Adopting this governance**: Work through [Workflow: Adoption](workflow.md#part-4-repository-adoption-workflow) and initialize [Repository Contracts](../contracts.md).
2. **Deciding where content lives**: Consult the 4D coordinate tensor in [Taxonomy](taxonomy.md).
3. **Checking hard rules**: Verify against the codified [Invariants](invariants.md).
4. **Reviewing a Pull Request**: Follow [Workflow: PR Review Gates](workflow.md#part-2-pull-request-review-gates).
5. **Evaluating documentation impact of code changes**: Check [Workflow: Trigger Matrix](workflow.md#part-1-code-to-documentation-trigger-matrix).

---

## Maintainer Invariant

This specification is normative engineering policy, not arbitrary project documentation. AI coding assistants may read, evaluate, and suggest improvements to this specification, but must never modify it without explicit maintainer instruction.
