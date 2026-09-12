# Operational Workflows & Verification Gates

This document defines the operational lifecycle: the code-to-doc change trigger matrix, pull request review gates, governance intake SOP, and downstream adoption procedures.

---

## Part 1: Code-to-Documentation Trigger Matrix

Whenever a code change is proposed, consult this matrix to determine mandatory documentation updates:

| Code / System Change | Required Documentation Surface | Invariant Reference |
| :--- | :--- | :--- |
| **New public CLI flag, subcommand, or API** | `docs/reference/` + `README.md` (if primary start path) | `[INV-TEMP-01]` |
| **Breaking API, protocol, or ABI change** | `CHANGELOG.md` + `docs/reference/` + `docs/adr/` | `[INV-ARCH-01]`, `[INV-TEMP-01]` |
| **Subsystem paradigm shift / Architectural refactor** | `docs/adr/NNNN-*.md` + `docs/architecture/*.md` | `[INV-ARCH-01]`, `[INV-ARCH-05]` |
| **Build system, test runner, or CI pipeline change** | `docs/dev/testing.md` | `[INV-VAL-02]` |
| **Contributor workflow or dev bootstrap change** | `docs/dev/setup.md` | `[INV-CORE-01]` |
| **New end-to-end user capability or journey** | `docs/dev/acceptance.md` + `docs/tutorials/` | `[INV-VAL-01]` |
| **Production outage, security incident, data loss** | `docs/dev/postmortems/YYYY-MM-DD-*.md` | `[INV-OPS-01]` |
| **Governance policy or profile activation change** | `docs/governance/documentation/contracts.md` | `[INV-TOOL-02]` |

---

## Part 2: Pull Request Review Gates

Maintainers and AI code review agents must verify pull requests against these gates before merging:

### Gate A: Spatial & Architectural Integrity
- [ ] Every modified or added document conforms to the 4D coordinate tensor in `spec/core/taxonomy.md`.
- [ ] No arbitrary Markdown files are added at the repository root (`[INV-CORE-02]`).
- [ ] No user-facing document links into `docs/dev/` (`[INV-CORE-01]`).
- [ ] Conceptual explanation documents do not contain source file line numbers or code blocks (`[INV-CORE-04]`).

### Gate B: Invariant & Lifecycle Compliance
- [ ] Living state documentation (`docs/architecture/`, Diátaxis) is updated alongside code changes (`[INV-TEMP-01]`).
- [ ] Proposed ADRs satisfy the 3-Question Significance Test and exhibit zero Bad ADR Anti-Patterns (`[INV-ARCH-06]`).
- [ ] Accepted ADRs are never modified in place; decisions are superseded or compacted (`[INV-ARCH-01]`).
- [ ] ADRs contain a complete "Rejected Alternatives & Negative Knowledge" section (`[INV-ARCH-02]`).
- [ ] Concluded or withdrawn RFCs are relocated into `docs/rfc/archive/` (`[INV-ARCH-04]`).
- [ ] Acceptance criteria verify journeys without test backdoors (`[INV-VAL-01]`).

### Gate C: Cryptographic & Standard Verification
- [ ] Automated verification passes cleanly:
  ```bash
  ./tools/verify.sh .
  ```
- [ ] If files in `spec/` were modified, SHA-256 signatures are updated via `./tools/update-hashes.sh`, and protocol version bumped per semantic versioning rules.

---

## Part 3: Standard Evolution Intake SOP

Proposed modifications to the `docs-governance` specification itself must pass the **Four-Tier Admission Filter**:

```text
┌─────────────────────────────────────────────────────────────┐
│ Tier 0: Core Protocol Meta-Rules (`spec/core/`)             │
│ Universal rules affecting >=95% of software repositories    │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Tier 1: Core Common Patterns                                │
│ Baseline recurring patterns universal to >=90% of repos     │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Tier 2: Domain Capability Profiles (`spec/profiles/`)       │
│ Cohesive engineering methodologies for opt-in adoption      │
└──────────────────────────────┬──────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Tier 3: Repository-Specific Governance                      │
│ Domain-specific rules belonging in adopting repo's private  │
│ governance (`docs/governance/<domain>.md`). Reject from spec│
└─────────────────────────────────────────────────────────────┘
```

### Evaluation Criteria
1. **Tier 0 (Core)**: Requires universal applicability across all programming languages, platforms, and architectures. Triggers Major or Minor protocol version bump.
2. **Tier 1 (Patterns)**: Single recurring document patterns common to nearly all repositories (README, Contributing).
3. **Tier 2 (Profiles)**: Must be a cohesive, standalone engineering methodology with high cohesion and zero coupling to unrelated profiles.
4. **Tier 3 (Private)**: Language-specific conventions (e.g., Rust clippy rules, TypeScript linter guides) must never enter `spec/`; they belong in downstream repository-level governance.

---

## Part 4: Repository Adoption Workflow

To onboard a repository to `docs-governance`:

1. **Step 1: Synchronize Standard**:
   Execute `sync.sh` pointing to the target repository:
   ```bash
   ./tools/sync.sh <target-repo-path>
   ```
   This copies `spec/core/` and the default profiles into `<target-repo-path>/docs/governance/documentation/`.

2. **Step 2: Declare Activated Profiles**:
   Edit `<target-repo-path>/docs/governance/documentation/contracts.md` to check the profiles active in your project (`core`, `architecture`, `validation`, `operations`).

3. **Step 3: Re-synchronize Profile Surfaces**:
   Re-run `sync.sh` to assemble activated profiles:
   ```bash
   ./tools/sync.sh <target-repo-path>
   ```

4. **Step 4: Verify in CI Pipeline**:
   Add `./tools/verify.sh .` to your repository's automated Pull Request validation matrix:
   ```yaml
   - name: Verify Documentation Governance
     run: ./tools/verify.sh .
   ```
