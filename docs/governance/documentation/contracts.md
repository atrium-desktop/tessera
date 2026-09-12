# Repository Contracts

Reference data, profile declarations, and path bindings for documentation surfaces adopted by this repository.

For adoption procedures, see [Workflow: Adoption](core/workflow.md#part-4-repository-adoption-workflow).

---

## 1. Activated Profiles

Declare the domain capability profiles active in this repository. `tools/sync.sh` and `tools/verify.sh` use this declaration to assemble and verify documentation surfaces.

- [x] `core` (Mandatory: 4D spatial taxonomy, system invariants, operational workflow, style)
- [x] `architecture` (Architecture records, living blueprints, pre-decision RFCs)
- [x] `validation` (Product validation: user journeys, acceptance matrices, testing guides)
- [ ] `operations` (Operational knowledge: postmortems, triage runbooks)

---

## 2. Directory Layout Bindings

| Surface | Path | Required | Temperature | Purpose |
| :--- | :--- | :--- | :--- | :--- |
| **Core Governance** | `docs/governance/documentation/` | Yes | **HOT** | Mirrored governance standard (`core/` + active profiles) |
| **Project Governance** | `docs/governance/` | Yes | **HOT** | Repository charters, API standards, and guidelines |
| **Contributor Firewall**| `docs/dev/` | Yes | **HOT** | Developer bootstrap, testing, and procedures |
| **Active ADRs** | `docs/adr/` | If `architecture` | **WARM** | Immutable Architectural Decision Records |
| **Living Blueprints** | `docs/architecture/` | If `architecture` | **HOT** | Living subsystem blueprints and compacted invariants |
| **Archived ADRs** | `docs/adr/archive/` | If `architecture` | **COLD** | Compacted, superseded, and deprecated records |
| **In-Flight RFCs** | `docs/rfc/` | Optional (`architecture`) | **WARM** | Active pre-decision proposals under deliberation |
| **Archived RFCs** | `docs/rfc/archive/` | Optional (`architecture`) | **COLD** | Concluded and withdrawn RFC deliberations |
| **Incident Reviews** | `docs/dev/postmortems/` | If `operations` | **COLD** | Archived blameless post-incident reviews |
| **Root Entry** | `README.md` | Yes | **HOT** | Project value pitch and shortest setup |
| **Docs Portal** | `docs/index.md` | Yes | **HOT** | Primary documentation navigation portal |

---

## 3. Optional Document Contracts

| Contract Surface | Active Profile | If Present | If Absent |
| :--- | :--- | :--- | :--- |
| `CHANGELOG.md` | Universal | User-visible changes must update it in the same PR | Omit changelog checks from PR review |
| `CONTRIBUTING.md` | Universal | Contributor entry point; links into `docs/dev/` | Add before accepting outside contributions |
| `docs/dev/acceptance.md` | Profile `validation` | User journey or acceptance changes update it | Rely on internal testing guides |
| `docs/dev/testing.md` | Profile `validation` | Test runner, command, or suite changes update it | Document testing in dev setup guide |
| `docs/adr/index.md` | Profile `architecture` | Active and archived ADRs registered in table | Create index before authoring ADRs |
| `docs/adr/template.md` | Profile `architecture` | New ADRs start from the template | Create a template before requiring ADRs |
| `docs/rfc/` | Profile `architecture` | In-flight debates in `docs/rfc/`; closed RFCs to `archive/` | Omit RFCs; record decisions directly in ADRs |
| `docs/reference/glossary.md` | Universal | Canonical project terms defined and cross-linked | Keep term definitions local to documents |
| `docs/tutorials/01-getting-started.md` | Universal | Setup changes update it with the README | Update the closest getting-started tutorial instead |
