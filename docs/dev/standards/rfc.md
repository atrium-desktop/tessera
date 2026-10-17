# Request for Comments (RFC) Standards

This document defines the process for authoring, reviewing, and ratifying
Request for Comments (RFC) proposals in Tessera.

---

## 1. When to Author an RFC

While an **ADR** records an accepted architectural decision and its negative
knowledge, an **RFC** is an upfront exploratory proposal used during early
deliberation when:

- The problem space has multiple competing solutions with divergent trade-offs.
- The proposed change spans multiple major subsystems (e.g., rewriting the
  presentation pipeline or altering the composition root's resource ownership).
- Alignment is required before investing in proof-of-concept code.

### Use or Skip RFC

| Situation | Decision | Rationale |
| :--- | :--- | :--- |
| **Localized implementation detail** | **Skip RFC** | Operates inside existing boundaries; a PR discussion suffices. |
| **Incremental architecture adjustment** | **Skip RFC** | Propose directly via a draft ADR. |
| **Cross-subsystem paradigm shift** | **Use RFC** | Multiple stakeholders, competing trade-offs, broad blast radius. |
| **Breaking public API, wire protocol, or dependency-policy redesign** | **Use RFC** | Broad consumer feedback and migration planning required. |

---

## 2. RFC Lifecycle

```text
  [ draft ] ──► [ under_review ] ──► [ accepted ] ──► Distill into ADR
                     │
                     └──► [ withdrawn ]
```

1. **draft**: Initial proposal drafted by the author.
2. **under_review**: Open for formal review and debate.
3. **accepted**: Consensus reached; must be distilled into a binding ADR under
   `docs/adr/`.
4. **withdrawn**: Proposal abandoned; retained in-place for historical forensic
   review.

RFCs evolve in-place through frontmatter transitions without moving into an
`archive/` subfolder.

### Frontmatter Schema

```yaml
---
id: RFC-NNNN
title: "[Title of Proposal]"
status: draft # [draft | under_review | accepted | withdrawn]
date: YYYY-MM-DD
author: "[Name / GitHub handle]"
target_adr: null # e.g., ADR-0142 (set when accepted)
withdrawn_reason: null # documented when withdrawn
---
```

---

## 3. RFC Structure

An effective RFC must address:

1. **Motivation & User Stories**: What real-world user or system problem
   requires this change?
2. **Detailed Design**: Precise APIs, data models, wire messages, and
   concurrency or teardown semantics.
3. **Security, Privacy & Failure Modes**: Behavior under malformed client
   input, GPU or driver failure, process crash, and lock-screen boundaries.
4. **Migration & Compatibility**: How existing configuration, persisted audit
   data, and downstream consumers transition.
5. **Unresolved Questions**: Known unknowns explicitly listed for reviewers.

---

## 4. Normative Rules

- **Speculative Isolation**: In-flight RFCs are proposals, not ratified system
  truth. Implementations and AI assistants must never treat an unratified RFC
  design as an active pattern.
- **Terminal Closure Mandate**: RFCs must reach a decisive terminal outcome
  (`accepted` or `withdrawn`). Abandoned, indefinite "zombie RFCs" are
  prohibited.
- **Distillation into ADR**: When consensus is reached, the ratified decisions,
  invariants, and negative knowledge must be distilled into a binding ADR. The
  RFC is then linked from that ADR as its historical proposal.
