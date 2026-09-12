# Spatial Taxonomy: The 4D Coordinate Tensor

Every documentation surface in an adopting repository maps deterministically to a unique coordinate in a **4-Dimensional Space Tensor**:

$$\text{Coordinate} = (\text{Temperature}, \text{Lifecycle}, \text{Audience}, \text{Cognitive Mode})$$

---

## Dimension 1: Temperature Tier (Retrieval & Maintenance Cadence)

Dictates update frequency, synchronization urgency, and AI agent context loading priorities:

| Tier | Characteristics | Maintenance Cadence | AI Context Rule |
| :--- | :--- | :--- | :--- |
| **HOT** | Current ground truth; living state. | Continuous; updated in same PR as code changes. | Primary context window for daily coding & generation. |
| **WARM** | Active architectural contracts and in-flight deliberations. | Append-only; transactional upon decision. | Consulted just-in-time during architectural review. |
| **COLD** | Historical provenance, superseded decisions, closed debates. | Immutable archive; never edited in place. | **Firewalled**: Default-excluded from daily AI prompts & tool searches. |

---

## Dimension 2: Lifecycle State (Temporal Phase)

Tracks the formal maturation stage of engineering knowledge:

```text
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│ Proposal Phase  │  ──►  │ Ratified Record │  ──►  │ Living State    │  ──►  │ Cold Archive    │
│ (In-Flight RFC) │       │ (Accepted ADR)  │       │ (Blueprint)     │       │ (Archived / Graveyard)
└─────────────────┘       └─────────────────┘       └─────────────────┘       └─────────────────┘
```

- **Proposal**: Exploration, stakeholder feedback, and alternatives debate (`docs/rfc/`).
- **Ratified Record**: Immutable historical covenant formalizing binding invariants (`docs/adr/`).
- **Living State**: Continuously updated snapshot describing how the system works today (`docs/architecture/`, Diátaxis).
- **Cold Archive**: Retired records preserved for historical forensics and negative knowledge (`docs/*/archive/`).

---

## Dimension 3: Audience Firewall (Access & Intent Boundary)

Enforces strict isolation between external consumers, internal contributors, and repository governance:

```text
┌─────────────────────────────────────────────────────────────┐
│ Governance Plane: `docs/governance/`                        │
│ Project-wide charters, review filters, and standards        │
└──────────────────────────────┬──────────────────────────────┘
                               │ Governs both planes
        ┌──────────────────────┴──────────────────────┐
        ▼                                             ▼
┌──────────────────────────────┐       ┌──────────────────────────────┐
│ User Plane (External)        │       │ Contributor Plane (Internal) │
│ `docs/tutorials/`, `how-to/` │       │ `docs/dev/`                  │
│ `reference/`, `explanation/` │       │ Setup, testing, acceptance   │
└──────────────────────────────┘       └──────────────────────────────┘
```

- **Governance Plane (`docs/governance/`)**: Constitutional rules, API guidelines, and documentation policies.
- **User Plane (Public)**: Learning, task execution, and conceptual explanation for system consumers.
- **Contributor Plane (`docs/dev/`)**: Firewall protecting internal developer setup, release steps, and acceptance matrices.
- *Boundary Rule*: User documentation must never link into `docs/dev/`. Contributor documentation may link outward to explanation.

---

## Dimension 4: Cognitive Mode (User-Facing Diátaxis)

Splits user-facing living documentation by cognitive objective:

| Quadrant | Directory | Objective | Voice & Tone |
| :--- | :--- | :--- | :--- |
| **Tutorial** | `docs/tutorials/` | Learning from zero | Second person ("you will build"). Step-by-step guarantee of success. |
| **How-To Guide** | `docs/how-to/` | Solving a real problem | Imperative ("Run", "Configure"). Assumes basic competence. |
| **Reference** | `docs/reference/` | Accurate factual lookup | Neutral, austere. Tables, lists, signatures. Minimal prose. |
| **Explanation** | `docs/explanation/` | Understanding architecture | Discursive, illuminating. Focuses on why; links to ADRs for decisions. |

---

## Complete 4D Document Mapping Tensor

Every document in a compliant repository occupies exactly one cell in the tensor:

| Document Surface | Temperature | Lifecycle | Audience | Cognitive Mode | Primary Purpose |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `README.md` | **HOT** | Living | Public/User | Mixed (Pitch) | Project value pitch and shortest setup |
| `docs/architecture/*.md` | **HOT** | Living | Dual | Explanation | Living subsystem blueprints & invariants |
| `docs/tutorials/*.md` | **HOT** | Living | User | Learning | Guided learning journey for beginners |
| `docs/how-to/*.md` | **HOT** | Living | User | Doing | Practical recipe solving a specific goal |
| `docs/reference/*.md` | **HOT** | Living | User | Lookup | Authoritative API, config, and CLI specs |
| `docs/explanation/*.md` | **HOT** | Living | Dual | Understanding | Deep background, domain rationale |
| `docs/dev/setup.md` | **HOT** | Living | Contributor | Doing | Local environment bootstrap & build |
| `docs/dev/acceptance.md` | **HOT** | Living | Contributor | Verification | Real user journey acceptance matrices |
| `docs/dev/testing.md` | **HOT** | Living | Contributor | Verification | Automated test suites, commands, fixtures |
| `docs/governance/index.md`| **HOT** | Living | Dual | Governance | Charters, review gates, intake rules |
| `docs/adr/NNNN-*.md` | **WARM** | Ratified | Dual | Governance | Immutable architectural decision record |
| `docs/rfc/RFC-NNNN-*.md` | **WARM** | Proposal | Contributor | Deliberation | Active, in-flight pre-decision proposal |
| `docs/adr/archive/*.md` | **COLD** | Archived | Auditor/Forensic | Archive | Compacted or superseded decision record |
| `docs/rfc/archive/*.md` | **COLD** | Archived | Auditor/Forensic | Archive | Concluded or withdrawn RFC debate |
| `docs/dev/postmortems/*.md`| **COLD** | Archived | Contributor | Postmortem | Blameless analysis of past incident |

---

## Location Exception: Root Files

Files required at the repository root by Git hosting conventions or automated toolchains are classified as **location exceptions**:

| File | Conceptual Home | Invariant |
| :--- | :--- | :--- |
| `README.md` | Public pitch / Entry | Must link to `docs/` rather than sprawling into a monolithic manual. |
| `CHANGELOG.md` | Release history | Chronological ledger of user-facing changes per release. |
| `CONTRIBUTING.md` | Contributor entry | Onboarding firewall entry point; links to `docs/dev/setup.md`. |
| `AGENTS.md` | Policy guardrail | Machine instructions defining coding invariants for AI assistants. |
| `SECURITY.md` | Public disclosure | Vulnerability reporting procedures. |
| `LICENSE` | Legal | Unaltered license text. |

Do not create arbitrary root Markdown files. All other documentation must route through the 4D tensor into `docs/`.
