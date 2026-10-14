# Entity: Architectural Decision Record (ADR)

Intent: Record durable architectural decisions, trade-offs, and negative knowledge for the lifetime of the repository.

---

## 1. Intent & Qualification (The 3-Question Test)

To prevent ADR inflation and maintain a high-signal log, author an ADR only if a proposal meets **at least two** of these criteria:

1. **High Reversal Cost**: Would reversing this decision require multi-week refactoring or breaking migration paths?
2. **Cross-Boundary Blast Radius**: Does it alter subsystem boundaries, public API protocols, data layouts, or non-functional SLAs (latency, security)?
3. **Generates Binding Invariants**: Does it establish hard behavioral red-lines that developers and AI assistants must unconditionally follow?

If a decision does not meet this threshold, **do not author an ADR**. Route it to a Pull Request description, commit message, or code comment.

### Anti-Patterns to Reject
- **The Triviality Smear**: Authoring ADRs for routine implementation choices (e.g., choosing a JSON serializer or renaming a function).
- **The Ephemeral Workaround**: Recording temporary bug fixes, transient feature flags, or environment hacks.
- **The Toothless Declaration**: Vague prose lacking testable invariants or clear behavioral boundaries.
- **The Post-Hoc Justification**: Authoring an ADR after code is merged with zero genuine consideration of discarded alternatives.

---

## 2. Normative Invariants

- **`[INV-LINT-03]` Frontmatter & Lifecycle Integrity**: Every ADR must declare standardized YAML frontmatter (`id`, `title`, `status`, `date`). Retired or superseded records transition in-place without physical file relocation.
- **`[INV-AGENT-01]` Negative Knowledge Mandate**: Every ADR must include a dedicated "Rejected Alternatives & Negative Knowledge" section documenting discarded options and their specific failure modes.
- **`[INV-AGENT-02]` Context Routing & Historical State**: Superseded and rejected records serve as negative knowledge constraints during refactoring and forensics, but are excluded during routine generative tasks.
- **`[INV-AGENT-04]` Significance Threshold**: Localized refactorings, ephemeral fixes, and cosmetic changes must never be admitted to `docs/adr/`.

---

## 3. In-Place Lifecycle & Frontmatter Schema

ADRs evolve strictly in-place through YAML frontmatter transitions. Files are never physically moved to an `archive/` directory:

```text
       [ draft ] ───► [ accepted ] ───► [ superseded ]
           │
           └───► [ rejected ]
```

- **draft**: Proposal under active deliberation.
- **accepted**: Ratified invariant binding on the repository.
- **superseded**: Formally replaced by a subsequent ADR (`superseded_by: ADR-NNNN`).
- **rejected**: Discarded alternative preserved in-place for negative knowledge audit.

### Frontmatter Schema
```yaml
---
id: ADR-NNNN
title: "[Title of Decision]"
status: draft # [draft | accepted | superseded | rejected]
date: YYYY-MM-DD
superseded_by: null # e.g., ADR-0002 (mandatory when status is superseded)
---
```

### Registry Index (`docs/adr/index.md`)
Every repository maintaining ADRs must keep a root registry table at `docs/adr/index.md`:

```markdown
# Architectural Decision Records

| ID | Title | Status | Decision Summary & Primary Invariant | Date |
| :--- | :--- | :--- | :--- | :--- |
| [0001](0001-setup.md) | Modular Architecture | accepted | Adopt hexagonal architecture with decoupled adapter layers | 2026-01-10 |
| [0002](0002-wal.md) | In-Memory WAL | superseded | Shared-memory append log; superseded by ClickHouse engine in ADR-0004 | 2026-02-15 |
```

---

## 4. Authoritative ADR Template

```markdown
---
id: ADR-NNNN
title: "[Title of Decision]"
status: draft
date: YYYY-MM-DD
superseded_by: null
---

# NNNN. [Title of Decision]

- Status: Draft | Accepted | Superseded by [ADR-NNNN](NNNN-slug.md) | Rejected
- Date: YYYY-MM-DD
- Deciders: [Names / GitHub handles]

---

## Context and Problem Statement

Describe the context and motivation in a few concise sentences. What problem is being solved, and why must it be addressed now?

## Decision Drivers

- Driver 1: e.g., Low-latency guarantees (< 10ms)
- Driver 2: e.g., Backward-compatible schema evolution

## Considered Options

- Option 1: Title of option 1
- Option 2: Title of option 2
- Option 3: Title of option 3

## Decision Outcome

Chosen option: "[Option 1]", because [detailed technical justification].

### Invariants & Behavioral Boundaries

List non-negotiable architectural rules resulting from this decision:
- Invariant 1: Mandatory behavioral constraint for developers and AI assistants.
- Invariant 2: Permitted or prohibited cross-boundary dependencies.

### Consequences

- **Positive**: Expected benefits and performance gains.
- **Negative & Trade-offs**: Incurred operational complexity and mitigation strategy.

## Rejected Alternatives & Negative Knowledge

Detail why alternative options were discarded:

### Option 2 (Rejected)
- **Why considered**: [e.g., Lower initial implementation complexity]
- **Why rejected**: [e.g., Introduced unbounded memory growth under backpressure]

### Option 3 (Rejected)
- **Why considered**: [e.g., Simpler concurrency model]
- **Why rejected**: [e.g., Incompatible with required latency SLA]

## Links

- Related PRs / Issues: [Link]
- Related ADRs: [Link]
```
