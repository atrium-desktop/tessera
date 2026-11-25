# Engineering Documentation & Collaboration Standards

This directory defines the authoritative meta-standards for documentation,
decision lifecycle, testing and verification, and operational playbooks in
this repository.

These standards are designed to be high-cohesion, practical, and adaptable
across systems projects. They govern how knowledge is structured, how
architectural consensus is ratified, and how operational runbooks are maintained.

---

## Standards Suite

| Standard | Purpose & Scope |
| :--- | :--- |
| **[`taxonomy.md`](taxonomy.md)** | **Documentation Taxonomy & Pipeline**: The Diátaxis layout, audience boundaries, repository root whitelist, and the 4-stage knowledge crystallization pipeline. |
| **[`adr.md`](adr.md)** | **Architectural Decision Records**: The 3-question qualification test, frontmatter schema, in-place lifecycle (`draft`, `accepted`, `superseded`, `rejected`), and template. |
| **[`rfc.md`](rfc.md)** | **Request for Comments (RFC)**: Consensus and deliberation protocol for high-reversal-cost subsystem redesigns prior to ADR ratification. |
| **[`testing.md`](testing.md)** | **Automated Testing**: The `cargo-nextest` runner, test layout by crate boundary, unit-test rules, `live_*` tests, and the automation boundary CI does not cross. |
| **[`verification.md`](verification.md)** | **Manual Verification**: The four-layer verification ladder, document classes under `docs/dev/verification/`, procedure and acceptance-gate structure, writing rules, and the support-matrix status vocabulary. |
| **[`runbooks.md`](runbooks.md)** | **Subsystem Runbooks & Playbooks**: Operational standards for complex protocol bring-up, hardware quirks registries, and hands-on diagnostic playbooks. |
| **[`postmortems.md`](postmortems.md)** | **Incident Postmortems**: Blameless timeline reconstruction, detection gap analysis, and defense-in-depth principles for major production/runtime failures. |
| **[`style.md`](style.md)** | **Markdown & Hyperlink Style**: Canonical relative links, anchor consistency, formatting rules, and prose guidelines. |

---

## Core Principles

1. **Mechanisms Over Exhortations**:
   Rules must be enforced by concrete mechanisms (compiler types, automated tests,
   and deterministic tooling) rather than natural-language warnings in comments.
2. **Knowledge Crystallization**:
   Ad-hoc development trial-and-error must never remain as loose notes. Knowledge
   must flow through the 4-stage pipeline: automated tests, ADRs, explanation
   docs, or structured runbooks.
3. **Flat & Pragmatic**:
   Zero arbitrary directory nesting or governance overhead. Every standard
   directly serves day-to-day engineering velocity and long-term maintainability.
