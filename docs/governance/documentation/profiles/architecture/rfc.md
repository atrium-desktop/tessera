# Entity: Request for Comments (RFC)

Intent: Provide an optional, structured space for pre-decision debate, stakeholder feedback, and exploration of trade-offs *before* committing to a durable architectural contract (ADR).

---

## 1. Intent & Qualification

An RFC is an exploratory deliberation tool, not an immutable decision record:

| Scenario | Action | Rationale |
| :--- | :--- | :--- |
| **Small-team or single-maintainer repo** | **Skip RFC** | Author ADRs directly; pre-decision debate overhead exceeds collaborative benefit. |
| **Localized refactoring or minor feature** | **Skip RFC** | Operates inside existing boundaries; PR discussion suffices. |
| **Incremental architecture adjustment** | **Skip RFC** | Propose directly via an ADR with `status: draft`. |
| **Cross-subsystem paradigm shift** | **Use RFC** | Multiple stakeholders, competing trade-offs, and broad blast radius. |
| **Breaking public API or protocol redesign** | **Use RFC** | Broad consumer feedback and migration planning required. |

---

## 2. Normative Invariants

- **Speculative Isolation (`[INV-AGENT-02]`)**: In-flight RFC debates represent speculative proposals, not ratified system truth. AI coding assistants must never use unratified RFC designs as authoritative implementation patterns.
- **Terminal Closure Mandate**: RFCs must reach a decisive terminal outcome (`accepted` or `withdrawn`). Abandoned, indefinite "zombie RFCs" are prohibited.
- **Distillation into ADR**: When consensus is reached (`accepted`), the ratified decisions, invariants, and negative knowledge must be distilled into a binding ADR under `docs/adr/`.

---

## 3. In-Place Deliberation Lifecycle & Frontmatter

RFCs evolve in-place through frontmatter status transitions without moving into an `archive/` subfolder:

```text
[ draft ] ──► [ under_review ] ──► [ accepted ] ──► Distill into ADR (docs/adr/)
                     │
                     └──► [ withdrawn ]
```

- **draft**: Initial proposal drafted by author.
- **under_review**: Open for formal review and debate.
- **accepted**: Consensus reached; ratified into a binding ADR.
- **withdrawn**: Proposal abandoned; retained in-place for historical forensic review.

### Frontmatter Schema
```yaml
---
id: RFC-NNNN
title: "[Title of Proposal]"
status: draft # [draft | under_review | accepted | withdrawn]
date: YYYY-MM-DD
author: "[Name / GitHub handle]"
target_adr: null # e.g., ADR-0003 (set when accepted)
withdrawn_reason: null # documented when withdrawn
---
```

---

## 4. Authoritative RFC Template

```markdown
---
id: RFC-NNNN
title: "[Title of Proposal]"
status: draft
date: YYYY-MM-DD
author: "[Name / GitHub handle]"
target_adr: null
withdrawn_reason: null
---

# RFC-NNNN: [Title of Proposal]

- Status: Draft | Under Review | Accepted | Withdrawn
- Date: YYYY-MM-DD
- Target ADR: [None | ADR-NNNN](docs/adr/NNNN-slug.md)

---

## 1. Summary

A 2–3 sentence executive summary of the proposed change and its primary motivation.

## 2. Motivation & Problem Statement

What real-world problem or performance bottleneck does this proposal address? Why is the current architecture insufficient?

## 3. Detailed Design

Technical description of the proposed architecture, components, data flows, and APIs.

### Key Workflows
Step-by-step description of critical execution paths.

### Cross-Subsystem Impacts
How does this change affect security, performance, storage, or operational complexity?

## 4. Alternatives Considered

Compare the primary proposal with competing approaches:

| Alternative | Pros | Cons | Reason Discarded |
| :--- | :--- | :--- | :--- |
| Option A | ... | ... | ... |
| Option B | ... | ... | ... |

## 5. Unresolved Questions & Risks

- Open Question 1: [Unresolved edge case or benchmark needed]
- Open Question 2: [Dependency on external ecosystem timeline]

## 6. Adoption & Migration Strategy

How will existing code, data, and downstream consumers migrate to this new pattern?
```
