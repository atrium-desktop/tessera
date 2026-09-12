# System Invariants Constitution

This document codifies the non-negotiable architectural and documentation invariants across the repository.

Every invariant carries a unique canonical identifier (`INV-<DOMAIN>-<NUMBER>`) citeable in code reviews, CI lint errors, and AI agent instructions.

---

## 1. Core Structural Invariants (`INV-CORE`)

- **`[INV-CORE-01]` Contributor Firewall Rule**:
  User-facing documentation (`docs/tutorials/`, `docs/how-to/`, `docs/reference/`, `docs/explanation/`) must never contain relative links into `docs/dev/`. Contributor documentation may link outward into user-facing explanation pages.
- **`[INV-CORE-02]` Location Exception Constraint**:
  Do not create arbitrary Markdown files at the repository root. Only recognized location exceptions (`README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `AGENTS.md`, `SECURITY.md`, `LICENSE`) may reside at the root level; all other documentation must route through `docs/`.
- **`[INV-CORE-03]` Single Source of Truth**:
  Do not duplicate text between files. Specifically, do not duplicate the `README.md` quickstart inside `docs/`; link directly to `README.md`.
- **`[INV-CORE-04]` Conceptual Purity in Explanation**:
  Documents under `docs/explanation/` must not contain implementation coordinates (source file paths, line numbers, struct field definitions, API signatures, or fenced code blocks). Technical coordinates belong exclusively in `docs/reference/` or inline code comments.
- **`[INV-CORE-05]` Directory Registry Completeness**:
  No documentation directory may exist without an `index.md` (or `README.md`) defining its charter, scope, and index table.

---

## 2. Knowledge Temperature & Archival Invariants (`INV-TEMP`)

- **`[INV-TEMP-01]` Hot Data Synchronization Mandate**:
  Living state documentation (`docs/architecture/`, Diátaxis quadrants, `README.md`) must be updated in the same Pull Request that alters code behavior, CLI syntax, public APIs, or configuration.
- **`[INV-TEMP-02]` The Archival Firewall Rule (AI Context Isolation)**:
  Automated toolchains, coding assistants, and prompt loaders must default to excluding all `**/archive/**` directories (`docs/adr/archive/`, `docs/rfc/archive/`) from general context loading and recursive code searches.
- **`[INV-TEMP-03]` No Direct Deletion (Negative Knowledge Preservation)**:
  Never physically delete (`rm` / `git rm`) accepted ADRs, concluded RFCs, or completed postmortems. Retired or obsolete records must be formally marked (`Superseded`, `Compacted`, `Withdrawn`) and relocated into cold storage (`archive/`).
- **`[INV-TEMP-04]` Cold Tier Immutability**:
  Once relocated to an `archive/` directory, files are permanently read-only historical records. They are never modified in place, except for pointer corrections in header metadata.

---

## 3. Architecture Domain Invariants (`INV-ARCH`)

- **`[INV-ARCH-01]` ADR Immutability Contract**:
  Once an Architectural Decision Record in `docs/adr/` is marked `Accepted`, its decision outcome and invariants are permanently immutable. To modify a prior decision, author a new ADR that explicitly supersedes or amends the prior record.
- **`[INV-ARCH-02]` Negative Knowledge Requirement**:
  Every ADR must contain a dedicated "Rejected Alternatives & Negative Knowledge" section detailing which alternatives were considered and why they failed.
- **`[INV-ARCH-03]` RFC Misuse Isolation**:
  Unresolved, in-flight RFC proposals (`docs/rfc/`) must never be loaded into AI coding assistants as binding architectural policies. Speculative designs must not be mistaken for ratified invariants.
- **`[INV-ARCH-04]` RFC Terminal Closure Mandate**:
  RFCs are ephemeral deliberation vehicles. Every RFC must reach a terminal state within a bounded review window: either distilled into an immutable ADR upon consensus, or marked `Withdrawn` and moved to `docs/rfc/archive/`.
- **`[INV-ARCH-05]` Snapshot Compaction Integrity**:
  When multiple related ADRs are compacted into a living architecture document (`docs/architecture/<subsystem>.md`), the snapshot must preserve all active invariants and maintain a historical lineage table citing the original ADR numbers.
- **`[INV-ARCH-06]` Architectural Significance Threshold**:
  An ADR must only be authored for decisions satisfying the 3-Question Significance Test (high reversal cost, cross-boundary blast radius, or generating binding invariants). Trivial implementation details, routine dependency updates, or ephemeral workarounds must never be admitted to `docs/adr/`.

---

## 4. Product Validation Invariants (`INV-VAL`)

- **`[INV-VAL-01]` Real User Journey Parity**:
  Acceptance journeys in `docs/dev/acceptance.md` must exercise the system strictly through real user interfaces, public CLI commands, or documented public APIs from a cold start, without invoking private test harness backdoors.
- **`[INV-VAL-02]` Deterministic Test Reproducibility**:
  Every automated test suite and test command listed in `docs/dev/testing.md` must execute deterministically without unstated environment variables, hidden local fixtures, or unversioned dependencies.

---

## 5. Operations Domain Invariants (`INV-OPS`)

- **`[INV-OPS-01]` Blameless Postmortem Structure**:
  Post-incident reviews in `docs/dev/postmortems/` must strictly focus on timeline reconstruction, detection gaps, systemic defense failures, and preventative engineering action items. Attribution of human error is prohibited.
- **`[INV-OPS-02]` Escalation Layering**:
  Recurring incident mitigation steps must be codified into operational runbooks. If a systemic root cause requires architectural modification, an ADR must be authored to ratify the structural fix.

---

## 6. Toolchain Invariants (`INV-TOOL`)

- **`[INV-TOOL-01]` Zero External Dependencies**:
  All utilities in `tools/` (`sync.sh`, `verify.sh`, `update-hashes.sh`) must execute on standard POSIX shell (`/bin/bash` or `/bin/sh`) and the Python 3 standard library (`hashlib`, `json`, `os`, `shutil`, `sys`). No external package managers or pip wheels are permitted.
- **`[INV-TOOL-02]` Zero-Drift Cryptographic CI Enforcement**:
  Any unauthorized modification or drift of non-editable governance surfaces detected by `./tools/verify.sh` must exit with non-zero status (`exit 1`) and fail CI pipeline validation.
