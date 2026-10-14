# Product Validation Profile

Domain capability profile governing the dual-tier product validation model, decoupling acceptance contracts from execution mechanisms.

Activate this profile in `.docgov.yml` under `validation`:
```yaml
validation:
  acceptance_path: "docs/dev/acceptance.md"
  testing_path: "docs/dev/testing.md"
```

---

## 1. The Dual-Tier Validation Model

Software correctness is verified across two complementary tiers:

```text
┌─────────────────────────────────────────────────────────────┐
│ Tier 1: Acceptance (`docs/dev/acceptance.md`)               │
│ User Journey Verification & Release Readiness Matrices      │
│ Question: "Does the system deliver what the user needs?"    │
│ Target: Outer-Loop Outcomes & Release Verification          │
└──────────────────────────────┬──────────────────────────────┘
                               │ Backed by
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Tier 2: Implementation Testing (`docs/dev/testing.md`)       │
│ Test Runner Commands, Unit / Integration Suites, Coverage   │
│ Question: "Is the implementation mechanically correct?"     │
│ Target: Inner-Loop Developer Regression Prevention          │
└─────────────────────────────────────────────────────────────┘
```

| Entity File | Surface | Scope | Focus |
| :--- | :--- | :--- | :--- |
| [Acceptance Entity](acceptance.md) | `docs/dev/acceptance.md` | Outer loop | End-to-end user journeys, acceptance scenario matrices, release sign-off |
| [Testing Entity](testing.md) | `docs/dev/testing.md` | Inner loop | Automated test runner commands, unit/integration suites, environment prerequisites |

---

## 2. Decoupling Intent from Execution Medium

A core engineering principle is decoupling **Acceptance Criteria** (what/why) from the **Execution Medium** (how):

| Validation Tier | Automated Execution (Machine in CI) | Manual Execution (Human Engineer) |
| :--- | :--- | :--- |
| **Tier 1: Acceptance**<br>*"Does it deliver value?"* | Automated E2E test suites, CLI smoke tests, contract tests (Target: >=90% automated) | Exploratory edge-case testing, UX auditing, final release cold-start walkthrough |
| **Tier 2: Testing**<br>*"Is code mechanically ok?"* | Unit test suites, integration tests, benchmark harnesses | Ad-hoc local debugging, interactive breakpoint inspection (Ephemeral) |

*Anti-Pattern*: Repetitive manual verification ("human clicking buttons" on routine releases) is an anti-pattern. Predictable user journeys must be automated in CI; human testing is reserved for exploratory review and high-cognition sign-off.

---

## 3. Invariant Governance

- **`[INV-LINT-02]` Contributor Firewall**: Both `acceptance.md` and `testing.md` live within `docs/dev/`. User-facing documentation must never link directly into internal validation docs.
- **`[INV-LINT-04]` Code-Doc Synchronization**: Introducing new user-visible capabilities requires updating `docs/dev/acceptance.md`; altering test runners or suite structures requires updating `docs/dev/testing.md`.
