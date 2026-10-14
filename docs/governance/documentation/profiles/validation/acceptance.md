# Entity: Acceptance Criteria & User Journeys

Intent: Verify that software delivers intended end-to-end user outcomes through real-world user journeys, scenario matrices, and formal release readiness gates.

---

## 1. Intent & Scope

Acceptance defines the contractual agreement on what constitutes deliverable software:
- **Contract vs. Execution**: Acceptance specifies *what* must work and *why*. How it is executed (automated CI suite vs. human exploratory testing) is an implementation detail.
- **Outer-Loop Focus**: Exercises the complete system end-to-end from a cold start, distinct from internal unit or algorithm testing.
- **Automation Target**: Aim for >= 90% of acceptance scenarios to be automated in CI pipelines. Manual verification is reserved for exploratory review and usability sign-offs.

---

## 2. Normative Invariants

- **Real User Interface Parity**: Acceptance journeys must exercise the system strictly through documented public interfaces, CLI commands, or APIs from a cold start, without invoking synthetic test backdoors or private mock overrides.
- **`[INV-LINT-04]` Code-Doc Synchronization**: When a user-visible feature or capability is introduced, changed, or retired, `docs/dev/acceptance.md` must be updated in the same Pull Request.
- **`[INV-LINT-02]` Contributor Firewall**: `docs/dev/acceptance.md` lives behind the contributor firewall; public user-facing documentation must never link directly into it.

---

## 3. Document Structure Requirements

Every `docs/dev/acceptance.md` document must provide four core sections:
1. **Scope & Execution Strategy**: Prerequisites and mapping of automated vs. human-verified scenarios.
2. **Core User Journeys**: Chronological steps from a cold start (`Initial State` -> `Action` -> `Verification` -> `Outcome`).
3. **Four-Quadrant Acceptance Scenario Matrix**: Tabular coverage across Happy Path, Edge Cases, Error Recovery, and Security.
4. **Release Readiness Checklist**: Explicit criteria required for production release sign-off.

---

## 4. Authoritative Acceptance Template

```markdown
# Acceptance Criteria & User Journeys

This document defines end-to-end user journeys, acceptance scenario matrices, and release readiness gates for this repository.

---

## 1. Scope & Execution Strategy

- **Target System**: [Artifact Name / Version / Scope]
- **Prerequisites**: [Pristine environment requirements: OS, runtime, network]
- **Automation Status**:
  - Automated Acceptance: [e.g., CI workflow `.github/workflows/e2e.yml` runs Scenarios `SCEN-01` through `SCEN-04`]
  - Human Walkthrough: [e.g., Pre-release exploratory checks and UX audit for `SCEN-05`]

---

## 2. Core User Journeys

### Journey 1: Clean Onboarding & Primary Workflow
1. **Initial State**: Clean host environment with prerequisites installed; zero pre-existing configuration or caches.
2. **Action**: Run the canonical setup / installation command:
   ```bash
   ./install.sh
   ./bin/app init
   ```
3. **Verification**: Exits with code 0; produces valid initialized directory layout and default config.
4. **Outcome**: System is verified ready for end-user operation.

### Journey 2: Standard Production Operation
1. **Initial State**: Initialized production environment.
2. **Action**: Execute core capability:
   ```bash
   ./bin/app run --input sample.dat
   ```
3. **Verification**: Outputs match specification schema without diagnostic warnings; runtime meets latency SLAs.
4. **Outcome**: User outcome successfully delivered end-to-end.

---

## 3. Four-Quadrant Acceptance Scenario Matrix

| Scenario ID | Category | Initial Condition | Action / Trigger | Expected Observable Outcome | Verification Medium |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `SCEN-01` | Happy Path | Default config | Execute primary workflow | Status 0; valid artifact generated | Automated (CI) |
| `SCEN-02` | Edge Case | Oversized payload | Ingest data exceeding limit | Clean exit 1; clear diagnostic error message; no panic | Automated (CI) |
| `SCEN-03` | Error Recovery | Network disconnected | Trigger remote sync | Retries with exponential backoff; reports network failure; zero state corruption | Automated (CI) |
| `SCEN-04` | Security | Unauthorized token | Invoke authenticated API | HTTP 401; security audit log emitted | Automated (CI) |
| `SCEN-05` | Usability / UX | Cold-start terminal | Run CLI with `--help` | Help text aligns with actual arguments; clean formatting | Human Walkthrough |

---

## 4. Release Readiness Checklist

Before signing off on a production release:
- [ ] Every journey in Section 2 succeeds from a cold start on a clean environment.
- [ ] All automated acceptance scenarios in Section 3 pass in CI.
- [ ] Exploratory and usability walkthroughs conducted with zero high-severity ergonomic defects.
- [ ] Real user interface parity verified: no synthetic test backdoors or private mocks used.
```
