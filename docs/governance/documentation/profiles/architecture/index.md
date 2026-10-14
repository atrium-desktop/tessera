# Architecture Governance Profile

Domain capability profile governing architectural decision lifecycles, living system blueprints, and pre-decision proposals.

Activate this profile in `.docgov.yml` under `architecture`:
```yaml
architecture:
  adr_path: "docs/adr"
  require_frontmatter:
    status_enum: ["draft", "accepted", "superseded", "rejected"]
    mandatory_fields: ["id", "title", "status", "date"]
```

---

## The Three Architecture Entities

This profile establishes three vertical entities across the lifecycle:

```text
┌──────────────────────────────┐
│ In-Flight RFC (`rfc.md`)     │  Pre-Decision Deliberation (WARM)
│ Exploration, feedback, debate│  Location: `docs/rfc/RFC-NNNN-*.md`
└──────────────┬───────────────┘
               │ Reaches consensus
               ▼
┌──────────────────────────────┐
│ Ratified ADR (`adr.md`)      │  Immutable Decision Covenant (WARM / COLD)
│ Decision, trade-offs, rules  │  Location: `docs/adr/NNNN-*.md`
└──────────────┬───────────────┘
               │ Distillation & synthesis
               ▼
┌──────────────────────────────┐
│ Living Snapshot              │  Current System Truth (HOT)
│ (`living-snapshot.md`)       │  Location: `docs/architecture/*.md`
└──────────────────────────────┘
```

| Entity File | Purpose | Document Surface | Temperature |
| :--- | :--- | :--- | :--- |
| [ADR Entity](adr.md) | Immutable decision records, rationale, and negative knowledge | `docs/adr/` | **WARM** (`accepted`) / **COLD** (`superseded \| rejected`) |
| [RFC Entity](rfc.md) | Optional pre-decision debate and alternative evaluation | `docs/rfc/` | **WARM** (`draft \| under_review`) / **COLD** (`accepted \| withdrawn`) |
| [Living Snapshot](living-snapshot.md) | Living subsystem state blueprints, invariants, and ADR synthesis | `docs/architecture/` | **HOT** |

---

## Directory Layout Bindings

When the `architecture` profile is active, the repository establishes:

| Surface | Path | Required | Temperature | Lifecycle Model |
| :--- | :--- | :--- | :--- | :--- |
| **ADRs** | `docs/adr/` | Yes | **WARM** / **COLD** | In-place frontmatter progression (`[INV-LINT-03]`) |
| **Living Architecture** | `docs/architecture/` | Recommended | **HOT** | Living ground truth synchronized with code (`[INV-LINT-04]`) |
| **RFC Proposals** | `docs/rfc/` | Optional | **WARM** / **COLD** | In-place frontmatter progression (`status: draft \| accepted \| withdrawn`) |

*Zero Physical Archival*: Retired records are never moved into an `archive/` subfolder. They remain in-place to preserve stable file paths, Git blame history, and cross-references.
