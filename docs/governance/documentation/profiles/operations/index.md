# Operational Knowledge Profile

Domain capability profile governing incident postmortems, root-cause forensics, and operational escalation layering.

Activate this profile in `.docgov.yml` under `operations`:
```yaml
operations:
  postmortems_path: "docs/dev/postmortems"
```

---

## The Operational Escalation Ladder

Operational knowledge flows upward through three distinct layers:

```text
┌─────────────────────────────────────────────────────────────┐
│ Layer 1: Triage Runbooks (Immediate Mitigation)             │
│ Symptoms, operational health checks, restart SOPs           │
└──────────────────────────────┬──────────────────────────────┘
                               │ Incident resolves
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 2: Postmortems (`postmortem.md` — Forensic Analysis)   │
│ Blameless timeline, detection gaps, systemic defense fixes  │
└──────────────────────────────┬──────────────────────────────┘
                               │ Structural redesign required
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Layer 3: Architectural Decision Record (`docs/adr/`)        │
│ Permanent invariant change or protocol redesign             │
└─────────────────────────────────────────────────────────────┘
```

| Entity File | Surface | Temperature | Focus |
| :--- | :--- | :--- | :--- |
| [Postmortem Entity](postmortem.md) | `docs/dev/postmortems/` | **COLD** | Root-cause analysis, timeline reconstruction, systemic action items |

---

## Directory Layout Bindings

When the `operations` profile is active, the repository establishes:

| Surface | Path | Required | Firewall Boundary | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| **Incident Reviews** | `docs/dev/postmortems/` | Yes | Internal (`[INV-LINT-02]`) | Archived blameless post-incident reviews |

---

## Invariant Governance

- **`[INV-AGENT-03]` Blameless Postmortem Structure**: Post-incident reviews must focus strictly on timeline reconstruction, detection gaps, and systemic defense failures. Personal human blame is strictly prohibited.
- **`[INV-LINT-02]` Contributor Firewall**: Postmortems reside in `docs/dev/` behind the contributor firewall; public user-facing guides must never link directly into postmortems.
