# Repository Governance

This directory defines repository governance charters, review standards, and
the mirrored documentation governance standard for Tessera.

---

## Architecture Decision Records

Immutable architectural decisions are registered in [ADRs](../adr/index.md), governed by the
[Architecture Profile](documentation/profiles/architecture/adr.md).

---

## Documentation Governance Standard (Protocol v5.1.0)

The standard is mirrored in [documentation/](documentation/core/index.md):

### Core Meta-Governance
- [Core Navigation & Charter](documentation/core/index.md): Standard overview and core primitives.
- [Taxonomy](documentation/core/taxonomy.md): 4D spatial coordinate tensor (`Temperature`, `Lifecycle`, `Audience`, `Cognitive Mode`).
- [Invariants](documentation/core/invariants.md): Codified constitution of numbered system invariants (`INV-*`).
- [Workflow](documentation/core/workflow.md): Code-to-doc trigger matrix, PR review gates, intake SOP, and adoption.
- [Style Guide](documentation/core/style.md): Technical voice, structural syntax, link contracts.
- [Repository Contracts](documentation/contracts.md): Active profiles and directory layout bindings.

### Active Domain Profiles
- **Architecture**: [Architecture Profile](documentation/profiles/architecture/index.md) (`adr.md`, `living-snapshot.md`, `rfc.md`).
- **Validation**: [Validation Profile](documentation/profiles/validation/index.md) (`acceptance.md`, `testing.md`).
