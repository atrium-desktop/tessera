# Invariant & Constraint Governance

This document codifies the definition, categorization, and lifecycle of
invariants within Tessera. It establishes an uncompromising policy against
tag proliferation, so that invariant identifiers remain high-value
architectural anchors rather than ceremonial comments.

---

## 1. Core Principle: Mechanisms Over Exhortations

An invariant is a condition that must unconditionally hold across all valid
states of a system. Projects suffer from "invariant inflation" when mundane UI
styles, layout heuristics, or ephemeral flags are labeled as invariants without
a verification mechanism.

**The Golden Rule**:

> **If a rule has neither a compiler-level proof nor an automated test
> assertion, it is NOT an invariant — it is merely an aspiration.**

---

## 2. Constraint Admissibility Matrix

Every architectural and operational rule in Tessera maps deterministically into
one of four categories:

| Category | Definition & Scope | Identifier Policy | Mandatory Enforcement Mechanism |
| :--- | :--- | :--- | :--- |
| **System Invariant** | Non-negotiable structural red-lines with cross-boundary blast radius (e.g., protocol purity, composition-root ownership, pre-pass seam uniqueness). | Declared as `[INV-<DOMAIN>-NN]` in the ADR that establishes it. Domain prefixes in use include `ARCH`, `IPC`, `INPUT`, `DND`, `A11Y`, `BT`, `NET`. | 1. Cargo dependency firewalls (`check-boundaries`)<br>2. Dedicated regression test (`test_inv_*`)<br>3. Rust compiler typestates / private visibility |
| **Domain Contract** | State-machine transitions and behavioral rules within a single crate or subsystem (e.g., Dock autohide reveal, window-switcher phases). | **No global `INV-` tag.**<br>Expressed as named types. | 1. Rust `enum` variants & private field encapsulation<br>2. RAII guard patterns<br>3. Unit tests |
| **Design Specification** | Visual styling, layout margins, truncation heuristics, motion curves, keyboard shortcuts, theme and material tokens. | **No `INV-` tag.**<br>Expressed as design constants. | 1. `tessera-design` tokens<br>2. `check-motion` zero-math gate (ADR-0174)<br>3. Surface preview snapshots |
| **Implementation Note** | Transient workarounds, driver bug workarounds, localized edge-case guards. | **No `INV-` tag.**<br>Standard code comments. | Direct regression unit test |

---

## 3. The 3-Condition Gate for System Invariants

A rule may be elevated to a **System Invariant** if and only if it satisfies all
three criteria:

1. **High Blast Radius**: Violation results in process corruption,
   unrecoverable data loss, cross-process deadlocks, or architectural inversion.
2. **Durable Architecture**: It represents a foundational pillar that will not
   become obsolete due to regular feature iterations or UI redesigns.
3. **Mechanically Enforced**: It specifies an automated enforcement mechanism
   (compiler type check, Cargo dependency rule, or deterministic automated
   test).

An invariant that fails condition 3 must be demoted to a domain contract, a
design token, or an implementation note.

---

## 4. Tag Discipline & Retirement

1. **No Pseudo-Invariants (UI, Styling, Local State)**:
   - Do not mint tags for presentation details (e.g. `INV-UI-*`,
     `INV-FOOTER-*`, `INV-AFFORDANCE-*`). Model states as typed enums and styles
     as design constants instead.
   - **No ceremonial tombstones**: when a pseudo-tag is retired it is simply
     deleted from annotations, not preserved as a comment.
2. **One Declaration Site**:
   - A tag is declared once, in the `### Invariants & Behavioral Boundaries`
     section of the ADR that establishes it.
   - Later ADRs may reference, amend, or supersede the tag, but must not
     redeclare it with different meaning. Number collisions within one domain
     prefix are a documentation defect.
3. **True Invariants (Architectural Red-Lines) Lifecycle**:
   - `Active`: Bound and verified in current releases.
   - `Superseded`: Formally superseded through a successor ADR
     (`superseded_by: ADR-NNNN`), retaining the documented failure mode in the
     successor's Negative Knowledge section.
   - `Deprecated`: Retired due to subsystem removal, with the historical context
     preserved in the ADR registry.
4. **Verification Sweep**:
   - Invariants whose enforcement mechanism is a test use the `test_inv_*`
     naming convention so the suite can be audited as a set. See the test layer
     model in [Surface Testing](../surface-testing.md).
