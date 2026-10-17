# Architectural Decision Records (ADR) Standards

This document establishes the qualification criteria, lifecycle progression,
frontmatter schema, and canonical template for Architectural Decision Records
(ADRs) in Tessera.

---

## 1. Intent & Qualification (The 3-Question Test)

To prevent ADR inflation and maintain a high-signal log, author an ADR only if a
proposal meets **at least two** of these criteria:

1. **High Reversal Cost**: Would reversing this decision require multi-week
   refactoring or breaking migration paths?
2. **Cross-Boundary Blast Radius**: Does it alter subsystem boundaries, crate
   dependency policy, wire protocol, data layouts, or non-functional SLAs
   (latency, security)?
3. **Generates Binding Red-Lines**: Does it establish hard structural
   boundaries that developers and AI assistants must follow?

### Anti-Patterns to Reject
- **The Triviality Smear**: Authoring ADRs for routine implementation choices
  (e.g., choosing a serializer or renaming a function).
- **The Ephemeral Workaround**: Recording temporary bug fixes, transient feature
  flags, or environment hacks.
- **The Toothless Declaration**: Vague prose lacking enforceable constraints or
  clear behavioral boundaries.
- **The Post-Hoc Justification**: Authoring an ADR after code is merged with
  zero genuine consideration of discarded alternatives.

---

## 2. In-Place Lifecycle & Frontmatter Schema

ADRs evolve strictly in-place through YAML frontmatter transitions. Files are
never physically moved to an `archive/` directory:

```text
       [ draft ] ───► [ accepted ] ───► [ superseded ]
           │
           └───► [ rejected ]
```

- **draft**: Proposal under active deliberation.
- **accepted**: Ratified contract binding on the repository.
- **superseded**: Formally replaced by a subsequent ADR (`superseded_by: ADR-NNNN`).
- **rejected**: Discarded alternative preserved in-place for negative knowledge.
- **deprecated**: Retired because the subsystem it governed was removed.

### Frontmatter Schema

```yaml
---
id: ADR-NNNN
title: "[Title of Decision]"
status: accepted # [draft | accepted | superseded | rejected | deprecated]
date: YYYY-MM-DD
scope: shell/dock, runtime/presentation # optional comma-separated domain scopes
superseded_by: null # e.g., ADR-0142 (mandatory when status is superseded)
negative_knowledge: true
---
```

`id`, `title`, `status`, and `date` are mandatory on every record. `scope` is
recommended for records that touch more than one crate. `superseded_by` is
required whenever `status: superseded`.

---

## 3. Status Line Convention

The frontmatter is the machine-readable source of truth; the body repeats the
status for readers:

```markdown
- Status: Accepted
- Date: 2026-10-15
- Deciders: Tessera Maintainers
- Amends: [ADR-0021](0021-chrome-component-trait.md)
```

`Amends`, `Amended by`, and `Extends` lines are how intra-repository lineage is
expressed. When a later ADR changes part of an earlier one, add the reciprocal
`Amended by:` line to the earlier record rather than editing its decision text.

---

## 4. Authoritative ADR Template

```markdown
---
id: ADR-NNNN
title: "[Title of Decision]"
status: draft
date: YYYY-MM-DD
scope: [comma-separated domain/component scopes]
superseded_by: null
negative_knowledge: true
---

# NNNN. [Title of Decision]

- Status: Draft | Accepted | Superseded by [ADR-NNNN](NNNN-slug.md) | Rejected | Deprecated
- Date: YYYY-MM-DD
- Deciders: [Tessera Maintainers]
- Amends: [Optional links to amended ADRs]

---

## Context and Problem Statement

Describe the context and motivation in concise, precise terms. What
architectural bottleneck, boundary violation, or shipped defect requires
resolution?

## Decision Drivers

- Driver 1: e.g., Guarantee one canonical pre-pass seam per chrome frame
- Driver 2: e.g., Eliminate a per-frame redundant full-output GPU pass

## Decision Outcome

State the chosen approach, module responsibilities, and structural changes.
Cite the concrete symbols (traits, functions, crates) that carry the decision.

### Invariants & Behavioral Boundaries

If the decision establishes a non-negotiable red-line, declare it once here with
its enforcement mechanism:

- `[INV-<DOMAIN>-NN] Name`: Mandatory constraint backed by `test_inv_*`, a
  `check-boundaries` edge, or a compiler typestate.

Follow the admissibility rules in [Invariants](invariants.md). A tag without a
mechanism is not an invariant.

## Consequences

### Positive
- [Architectural and operational benefit]

### Negative / Trade-offs
- [Incurred complexity or risk, and its mitigation]

## Rejected Alternatives & Negative Knowledge

Detail why alternative options were discarded:

### Alternative A (Rejected)
- **Why considered**: [Rationale]
- **Why rejected**: [Failure mode or architectural trade-off that disqualified it]

## Verification

Name how the accepted decision is checked in this repository: the boundary
rule, the regression test, the CI job, or the manual hardware validation step.
```

---

## 5. Numbering, Indexing, and Verification

1. **Append-Only Numbering**: IDs are monotonic and never reused. A withdrawn
   proposal keeps its number with `status: rejected`.
2. **Index Registration**: Every record is listed in
   [`docs/adr/index.md`](../../adr/index.md) with its title, status, and
   supersession link. The index is generated alongside the record, not later.
3. **Verification**: Before hand-off, confirm the record's links resolve, its
   frontmatter parses, and its declared enforcement mechanism exists in the
   tree (`cargo run -p xtask -- check-boundaries`, `check-motion`, or the named
   test).
