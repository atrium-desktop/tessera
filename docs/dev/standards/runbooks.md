# Subsystem Runbook & Playbook Standards

This document establishes the standards for authoring, structuring, and
maintaining subsystem runbooks and diagnostic playbooks under `docs/dev/runbooks/`.

---

## 1. Scope & Intent

A **Runbook** is an operational and hands-on guide for complex subsystems,
hardware integrations, or external protocols (such as Miracast WFD, DRM/KMS
bring-up, Bluetooth peripherals, or PipeWire audio routing).

It captures hard-won operational know-how that cannot be fully expressed in
compiler types or unit tests alone—such as real-world hardware quirks, network
topology constraints, A/B diagnostic procedures, and packet inspection recipes.

### Boundary Matrix

| Document Class | Purpose | What It Contains | Location |
| :--- | :--- | :--- | :--- |
| **Explanation** | Conceptual understanding | High-level architecture, protocol state machines, mathematical/timing models. | `docs/explanation/` |
| **ADR** | Decision contracts | Why an architecture or dependency was chosen, rejected alternatives, trade-offs. | `docs/adr/` |
| **Runbook** | Operational execution | Real-world hardware setup, diagnostic commands, quirks registries, A/B recipes. | `docs/dev/runbooks/` |
| **Triage** | Symptom attribution | Rapid classification of user-visible bug reports (client vs. compositor vs. driver). | `docs/dev/triage/` |
| **Postmortem** | Failure retrospective | Blameless timeline and defense-in-depth analysis of major production/session failures. | `docs/dev/postmortems/` |

---

## 2. Anti-Patterns & Prohibitions

1. **No Ephemeral Task Handoffs**:
   A runbook is a durable operational manual, not a scratchpad for incomplete
   tasks. Ephemeral sprint notes (e.g., "file X is untracked", "local editor bug
   workaround") must be resolved before merging, never recorded in a runbook.
2. **No Pure Rationale**:
   Design rationale belongs in ADRs. If a runbook explains *why* a design was
   chosen over another, summarize it in one sentence and link to the governing ADR.
3. **No Unactionable Checklists**:
   Every item in a "Do-Not-Regress Checklist" must state a concrete verification
   method (e.g., a specific command, packet trace, or automated test).

---

## 3. Standard Runbook Structure

Every subsystem runbook under `docs/dev/runbooks/` must follow this structure:

### 1. Target Subsystem & Reference Environment
- Hardware, firmware, OS kernel, and driver baselines tested.
- External dependencies and tools required (e.g., `wireshark`, `iw`, `ffprobe`).
- Topology requirements (e.g., P2P roles, AP concurrence, network isolation).

### 2. Core Protocol Rules & Invariants
- Summary of non-negotiable operational invariants (e.g., socket roles, port
  bindings, packet framing requirements).
- Pointer to the corresponding ADR and architecture explanation.

### 3. Hardware / Vendor Quirks Registry
- Known behaviors, anomalies, and edge cases of specific sinks, chips, or
  client implementations.
- Concrete workarounds or defensive behaviors implemented in code.

### 4. Diagnostic Playbook & Verification Recipes
- Step-by-step commands to reproduce, inspect, and verify behavior.
- Local mock/emulation recipes (no hardware required).
- Live hardware recipes and network packet capture filters.
- Fast failure classification (symptom -> root cause -> verified fix).

### 5. Do-Not-Regress Checklist
- Explicit, verifiable checks that must pass before delivering changes to the
  subsystem.

---

## 4. Lifecycle & Maintenance

- **Active Maintenance (HOT)**: Runbooks must be updated in the same change that
  modifies the subsystem's operational behavior or discovers a new hardware quirk.
- **Mechanisms First**: Whenever a manual check in a runbook can be automated
  via an integration test, mock, or CI assert, automate it and reduce the manual
  runbook footprint.
