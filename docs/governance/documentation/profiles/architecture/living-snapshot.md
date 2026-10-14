# Entity: Living Architecture Blueprint & Compaction

Intent: Condense incremental, historical ADR logs into living subsystem architecture blueprints (`docs/architecture/`) to prevent context bloat, stale-rule poisoning, and high discovery latency.

---

## 1. Intent & The Dual-Plane Architecture

Engineering documentation operates on two complementary planes:

```text
┌─────────────────────────────────────────────────────────────────┐
│ Event Log Plane (WARM / COLD — Append-Only): `docs/adr/`        │
│ Immutable chronological sequence of decisions and trade-offs.   │
└────────────────────────────────┬────────────────────────────────┘
                                 │ Periodic synthesis & compaction
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│ Living State Plane (HOT — Ground Truth): `docs/architecture/`   │
│ Authoritative current blueprints, subsystem boundaries, rules.  │
└─────────────────────────────────────────────────────────────────┘
```

- **The ADR Log (`docs/adr/`)**: Captures *why* the system evolved over time. Referenced just-in-time when investigating historical rationale.
- **The Living Snapshot (`docs/architecture/`)**: Captures *what* the system is today. Primary context for daily engineering and AI code generation.

### Compaction Triggers
Execute a compaction cycle when any trigger fires:
1. **Subsystem Saturation**: A subsystem (e.g., auth, networking, storage) accumulates >= 5 amending or overlapping ADRs.
2. **High Invalidation Ratio**: Over 30% of records in `docs/adr/` are marked `superseded` or `rejected`.
3. **Major Architectural Milestone**: A subsystem undergoes a major protocol or boundary redesign.

---

## 2. Normative Invariants

- **Living State Synchronization (`[INV-LINT-04]`)**: Living architecture blueprints are **HOT** tier documents. Any structural modification to code must update the corresponding snapshot in the same Pull Request.
- **Lineage Preservation**: When synthesizing ADRs into a living blueprint, the blueprint must retain a Lineage & Decision History table citing the original ADR IDs.
- **Zero Physical Deletion & In-Place Deprecation (`[INV-LINT-03]`)**: Never delete compacted ADRs. Mark their frontmatter as `status: superseded` in-place, pointing `superseded_by: docs/architecture/<subsystem>.md`.

---

## 3. Compaction Lifecycle SOP

```text
Step 1: Synthesize Blueprint   Step 2: Update In-Place State   Step 3: Update Registry Index
┌───────────────────────────┐  ┌────────────────────────────┐  ┌───────────────────────────┐
│ Write / update            │  │ Mark compacted ADRs as     │  │ Update docs/adr/index.md  │
│ docs/architecture/        │─►│ status: superseded in-place│─►│ with pointers to living   │
│ <subsystem>.md            │  │ (no file relocation)       │  │ subsystem snapshot        │
└───────────────────────────┘  └────────────────────────────┘  └───────────────────────────┘
```

1. **Synthesize**: Create or update the living blueprint in `docs/architecture/<subsystem>.md`, distilling active components, invariants, and data flows.
2. **Transition in-place**: In each compacted ADR, update frontmatter:
   ```yaml
   status: superseded
   superseded_by: docs/architecture/<subsystem>.md
   ```
3. **Re-index**: Update `docs/adr/index.md` reflecting the new status and linking to the living blueprint.

---

## 4. Authoritative Living Blueprint Template

```markdown
# [Subsystem Name] Architecture Blueprint

Authoritative living architectural specification for the [Subsystem Name] subsystem.

---

## 1. System Topology & Responsibilities

- **Boundary**: Explicit definition of what this subsystem owns and what it delegates.
- **Dependencies**: Allowed upstream and downstream subsystem couplings.

```text
[ Upstream Ingress ] ──► [ This Subsystem ] ──► [ Downstream Persistence ]
```

---

## 2. Active Invariants & Behavioral Boundaries

The current binding behavioral red-lines enforced by this subsystem:
- `[INV-SUB-01]`: Invariant description and enforcement mechanism.
- `[INV-SUB-02]`: Non-functional SLA (e.g., max latency, memory footprint).

---

## 3. Data Flow & Component Model

Detailed component decomposition, state machines, and concurrency guarantees.

---

## 4. Lineage & Historical Decision Index

Compact table tracing active invariants back to their ratifying historical ADRs:

| Invariant / Feature | Original Ratifying ADR | Ratification Date | Status |
| :--- | :--- | :--- | :--- |
| Core Protocol Framing | [ADR-0001](../adr/0001-modular.md) | 2026-01-10 | Active |
| Storage Engine | [ADR-0004](../adr/0004-engine.md) | 2026-03-15 | Active |
```
