# System Taxonomy & Governance Coordinates

Every documentation asset in an adopting repository maps deterministically to a 4-dimensional governance coordinate:
**(Temperature, Lifecycle, Audience, Content Type)**.

---

## 1. Temperature (Retrieval & Synchronization Cadence)

Governs maintenance frequency, update urgency, and AI agent context-loading priority:

| Temperature | State & Role | Maintenance Cadence | AI Context Routing |
| :--- | :--- | :--- | :--- |
| **HOT** | Living ground truth (`README.md`, `docs/{tutorials,how-to,reference,explanation}/`, `docs/architecture/`). | Synchronous: updated in the same change as code (`[INV-LINT-04]`). | Primary context window for daily coding, generation, and review. |
| **WARM** | Active decision contracts and proposals (`docs/adr/`, `docs/rfc/`). | Transactional: created or updated upon architectural review. | Consulted just-in-time during architectural reasoning. |
| **COLD** | Historical rationale, retired decisions, and postmortems (`docs/dev/postmortems/`, superseded ADRs). | Permanent in-place retention (`status: superseded \| rejected`). | Excluded from generative mode (`[INV-AGENT-02]`); retrieved during forensics and refactoring. |

---

## 2. Lifecycle (In-Place Frontmatter Progression)

Knowledge evolves in-place through YAML frontmatter transitions without physical file relocation:

```text
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│  Draft / RFC    │  ──►  │    Accepted     │  ──►  │   Superseded    │
│ (In-Flight)     │       │ (Active Ground) │       │ (Historical)    │
└─────────────────┘       └─────────────────┘       └─────────────────┘
         │                                                   ▲
         └───────────────────────────────────────────────────┘
                           (Direct Rejection)
```

- **Draft**: In-flight proposal under active deliberation.
- **Accepted**: Ratified invariant or architectural contract binding on the repository.
- **Superseded**: Retired decision replaced by a newer standard. Retained in-place with `superseded_by: ADR-NNNN`.
- **Rejected**: Discarded alternative preserved in-place for negative knowledge forensics (`[INV-AGENT-01]`).

*Zero File Relocation*: Moving files to `archive/` directories breaks Git history, hyperlinks, and external citations. All status progression is recorded via frontmatter (`[INV-LINT-03]`).

---

## 3. Audience Isolation (Contributor Firewall)

Enforces strict separation between public consumers and internal maintainers:

```text
┌─────────────────────────────────────────────────────────────┐
│ Public Surface (External Users & Integrators)               │
│ `docs/tutorials/`, `how-to/`, `reference/`, `explanation/`  │
└──────────────────────────────┬──────────────────────────────┘
                               │ [INV-LINT-02] Contributor Firewall:
                               │ Public NEVER links into Internal
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Internal Surface (Maintainers & Core Contributors)          │
│ `docs/dev/` (setup, testing, acceptance, postmortems)       │
└─────────────────────────────────────────────────────────────┘
```

- **Public Surface (`docs/{tutorials,how-to,reference,explanation}/`)**: Consumed by external developers and end-users.
- **Internal Surface (`docs/dev/`)**: Protected environment setup, test topologies, acceptance sign-offs, and incident postmortems.
- **Firewall Rule (`[INV-LINT-02]`)**: Public documentation must never contain relative links into `docs/dev/`. Internal documentation may freely link outward to public documentation.

---

## 4. Content Type (Diátaxis Framework)

Public living documentation is organized into four distinct cognitive modes:

| Category | Directory | Objective | Tone & Voice |
| :--- | :--- | :--- | :--- |
| **Tutorial** | `docs/tutorials/` | Learning from scratch | Step-by-step guidance; guarantees beginner success. |
| **How-To Guide** | `docs/how-to/` | Solving a real problem | Imperative ("Run", "Configure"); assumes competence. |
| **Reference** | `docs/reference/` | Factual lookup | Neutral, austere. Tables, signatures, and configurations. |
| **Explanation** | `docs/explanation/` | Architectural understanding | Discursive; explains design rationale and boundaries. |

---

## 5. Repository Layout & Root Whitelist

To eliminate root clutter, arbitrary Markdown files at the repository root are strictly forbidden (`[INV-LINT-01]`). Only whitelisted files are permitted:

| Root File | Governance Role | Constraints |
| :--- | :--- | :--- |
| `.docgov.yml` | Declarative SSOT | Configuration for rules, firewalls, and path triggers. |
| `AGENTS.md` | AI Guardrail | Machine directives and invariant bindings. |
| `README.md` | Public Pitch & Gateway | High-level overview; routes deep dives into `docs/`. |
| `CHANGELOG.md` | Release Ledger | User-facing changelog per version. |
| `CONTRIBUTING.md` | Onboarding Entry | Links contributors to `docs/dev/setup.md`. |
| `SECURITY.md` | Disclosure Policy | Vulnerability reporting procedures. |
| `LICENSE` / `LICENSE.md` | Legal | Unaltered license text. |

All other project documentation must reside within the structured `docs/` hierarchy.
