---
id: ADR-0201
title: "Decouple Documentation Governance from Agent Configuration and Canonize Sandbox Isolation"
status: accepted
date: 2026-11-02
scope: docs/standards, docs/governance, docs/adr
superseded_by: null
negative_knowledge: true
---

# 0201. Decouple Documentation Governance from Agent Configuration and Canonize Sandbox Isolation

- Status: Accepted
- Date: 2026-11-02
- Deciders: Tessera Maintainers
- Amends: [ADR-0170](0170-canonical-shell-preview-routing-and-workspace-debug-asset-topology.md)

---

## Context and Problem Statement

During the transition from the legacy external `docgov` meta-framework to flat in-tree standards (`docs/dev/standards/`), several architectural inversions and coupling anomalies were left unresolved:

1. **Inversion of Epistemic Authority (ADR-0170)**:
   In [ADR-0170](0170-canonical-shell-preview-routing-and-workspace-debug-asset-topology.md), the rejected alternatives section cited `AGENTS.md` as the authoritative source for test and workspace isolation rules (*“Never touch user home or global state paths”*). Citing an AI assistant directives file as the normative basis for architectural decisions reverses the flow of authority: architectural standards belong in canonical system documentation, never in a developer-agent prompt configuration.

2. **Documentation Taxonomy Conflation**:
   In `docs/dev/standards/taxonomy.md` §3, `AGENTS.md (with CLAUDE.md and GEMINI.md as symlinks to it)` was codified into the repository root documentation whitelist. Documentation governance exists to structure technical human and contributor documentation (under the Diátaxis model). Prescribing agent prompt files and vendor-specific symlinks inside the documentation taxonomy conflates documentation governance with developer-local tooling.

3. **Residual Governance Directives in Agent Prompts**:
   `AGENTS.md` previously retained extensive routing sections (§4 and §5) detailing documentation governance, ADR schema enforcement, and review habits—historical residue from automated `docgov` injection. `AGENTS.md` is meant to be a user-maintained file for AI assistant directives, completely decoupled from repository documentation.

## Decision Drivers

- **Single Source of Truth (SSOT)**: Repository architectural invariants and test isolation rules must be defined in `docs/dev/standards/` and enforced by code or tests, not cited from prompt files.
- **Strict Separation of Concerns**: Project documentation covers technical specifications and developer guides; developer agent directives belong exclusively to user-maintained tooling surfaces.
- **Clean Break**: Eliminate all remaining coupling, prescriptive symlink mandates, and docgov artifacts without backward compatibility compromises.

## Decision Outcome

1. **Complete Decoupling of `docs` and `AGENTS.md`**:
   - `docs/` shall never cite or depend on `AGENTS.md` or any AI assistant prompt file.
   - `AGENTS.md` is decoupled from repository documentation governance. It does not carry documentation standards, review matrices, or docgov residue; it is dedicated to user maintenance.

2. **Canonization of `[INV-ARCH-87]` (Environment & State Isolation)**:
   - The test and state isolation rule previously referenced in ADR-0170 is promoted to a formal System Invariant in `docs/dev/standards/architecture.md` §2.6 and `docs/dev/standards/invariants.md`.

3. **Purification of Documentation Taxonomy**:
   - `docs/dev/standards/taxonomy.md` §3 is purified to whitelist only canonical top-level documentation files (`README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `LICENSE`, `SECURITY.md`). Developer workspace configs and agent files are formally classified as user-maintained tooling decoupled from the documentation hierarchy.

4. **Amending ADR-0170**:
   - The citation in [ADR-0170](0170-canonical-shell-preview-routing-and-workspace-debug-asset-topology.md) is updated to cite `[INV-ARCH-87]` and `docs/dev/standards/architecture.md`.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-87] Environment & State Isolation`: Unit tests, integration tests, preview environments, and workspace tool runs must NEVER mutate, read, or depend on the host user's home directory (`$HOME`, `~/.config`, `~/.local`) or global host state. Stateful filesystem operations must isolate inside `tempfile::tempdir()`, and network services must bind ephemeral ports (`:0`).

## Rejected Alternatives & Negative Knowledge

### Retaining `AGENTS.md` as an Invariant Anchor
- **Why considered**: `AGENTS.md` already contained natural-language summaries of isolation rules.
- **Why rejected**: Violates the core governance philosophy (*Mechanisms Over Exhortations*, `docs/dev/standards/invariants.md`). Prompt files are mutable consumer inputs, not authoritative architectural foundations. Citing prompt files in ADRs creates circular dependencies and epistemic rot.

### Prescribing Vendor Symlinks (`CLAUDE.md`, `GEMINI.md`) in Taxonomy
- **Why considered**: Convenient for developers utilizing multiple AI editor clients.
- **Why rejected**: Prescribing specific third-party AI assistant symlinks inside the repository's documentation taxonomy bleeds external tool assumptions into canonical engineering standards. Tooling configurations belong to the developer.

### Maintaining Docgov Routing Sections in `AGENTS.md`
- **Why considered**: Reminded AI agents of the location of standards in `docs/dev/standards/`.
- **Why rejected**: Perpetuated docgov-era baggage. Authoritative developer standards are self-evident in `docs/dev/standards/README.md`.

## Consequences

### Positive
- The single source of truth for architectural constraints is fully restored inside `docs/dev/standards/`.
- Documentation taxonomy strictly governs documentation without overreaching into developer agent configurations.
- `AGENTS.md` is liberated from documentation governance duties and becomes purely user-maintained.

### Negative / Trade-offs
- AI agents reading solely `AGENTS.md` must discover repository standards via the standard developer onboarding paths (`CONTRIBUTING.md` / `docs/dev/standards/`) rather than inline docgov summaries.
