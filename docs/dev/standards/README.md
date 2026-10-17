# Tessera Engineering Standards

This directory defines the authoritative engineering, architectural, and quality
standards for the Tessera project.

Rather than maintaining an external governance meta-framework with deeply nested
directories or a disconnected top-level governance silo, Tessera keeps a
**flat, high-cohesion, practical, and code-aligned standards suite directly
inside `docs/dev/standards/`**.

---

## Standards Directory Structure

All engineering standards reside flatly in `docs/dev/standards/`:

| Document | Purpose & Authority |
| :--- | :--- |
| **[`architecture.md`](architecture.md)** | **Workspace Topology & System Boundaries**: Composition root, capability layering, platform/render firewalls, and crate admission thresholds. |
| **[`invariants.md`](invariants.md)** | **Invariant Standards & Admissibility**: Codifies true system invariants (mechanisms over exhortations), the classification matrix, and the tag-tightening policy. |
| **[`adr.md`](adr.md)** | **Architectural Decision Records**: The 3-Question Test for ADR qualification, in-place lifecycle (`draft`, `accepted`, `superseded`, `rejected`), frontmatter schema, and template. |
| **[`rfc.md`](rfc.md)** | **Request for Comments (RFC)**: Deliberation and consensus protocol for cross-cutting, high-reversal-cost subsystem redesigns prior to ADR ratification. |
| **[`taxonomy.md`](taxonomy.md)** | **Documentation Taxonomy**: Four-quadrant Diátaxis layout, the public/contributor audience boundary, and the repository root whitelist. |
| **[`style.md`](style.md)** | **Documentation & Markdown Style**: Canonical hyperlink conventions, relative routing, formatting consistency, and prose guidelines. |
| **[`workflow.md`](workflow.md)** | **Engineering Workflow**: PR review gates, code-to-doc synchronization, release hygiene, and zero-legacy-drag standards. |
| **[`postmortems.md`](postmortems.md)** | **Blameless Incident Postmortems**: Root-cause analysis, timeline reconstruction, detection gap mitigation, and defense-in-depth principles. |

For hands-on testing execution and QA runbooks, see
[`docs/dev/surface-testing.md`](../surface-testing.md) and
[`docs/dev/vt-drm-testing.md`](../vt-drm-testing.md). For build and toolchain
setup, see [`docs/dev/setup.md`](../setup.md).

---

## Core Governance Philosophy

1. **Mechanisms Over Exhortations**:
   Rules and system boundaries must be enforced by concrete mechanisms: Rust
   compiler type mechanics (typestates, ownership, private visibility), crate
   dependency firewalls checked from Cargo metadata, or deterministic test
   assertions. Natural-language "swearing" in comments is forbidden.
2. **Anti-Inflation**:
   A sharp line separates **System Invariants** (non-negotiable structural
   red-lines), **Domain Contracts** (internal state machines), **Design
   Specifications** (UI styling, motion, keyboard shortcuts), and
   **Implementation Notes**.
3. **Flat & Pragmatic**:
   Zero arbitrary nesting or nested "core" junk drawers. Every governance
   document directly addresses an active engineering concern in Tessera.
4. **Self-Contained Epistemic Lineage**:
   All decisions and references must resolve directly within `docs/adr/`
   (ADR-0001 through current). Ghost references to non-existent external
   `ADR-01xx`/`ADR-02xx` legacy numbers are prohibited.
