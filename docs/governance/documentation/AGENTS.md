# AGENTS.md

Instructions for AI coding assistants working within `docs/governance/documentation/`.

---

## 1. Directory Mission & Invariant

This directory is an exact mirror of the canonical `docs-governance` specification (Protocol v5.0.0).

- **Policy Status**: This directory contains normative governance policy, not application source code.
- **AI Modification Invariant**: AI assistants may read this directory to inspect system invariants, taxonomy coordinates, and templates, but must never edit files within this directory unless explicitly instructed by a repository maintainer.

---

## 2. Invariant Checklist for AI Assistants

When authoring or modifying documentation in this repository, always verify:

1. **Spatial Tensor Compliance (`[INV-CORE-01]`, `[INV-CORE-02]`)**:
   - Check destination against `core/taxonomy.md`.
   - Never link from public documentation into `docs/dev/`.
   - Never create arbitrary root Markdown files.
2. **Archival Firewall (`[INV-TEMP-02]`)**:
   - Default to excluding `**/archive/**` (`docs/adr/archive/`, `docs/rfc/archive/`) from routine searches and prompt contexts.
3. **Immutability & Negative Knowledge (`[INV-ARCH-01]`, `[INV-ARCH-02]`)**:
   - Never modify an `Accepted` ADR in place; create a superseding record.
   - Always retain rejected options and negative rationale.
4. **RFC Isolation (`[INV-ARCH-03]`)**:
   - Never treat in-flight RFC proposals (`docs/rfc/`) as established architectural constraints.
5. **Living State Synchronization (`[INV-TEMP-01]`)**:
   - Update `docs/architecture/` and Diátaxis documentation in the same PR as related code changes.

---

## 3. Toolchain Verification

Before completing tasks that modify governance or documentation, run:
```bash
python3 -W error -m unittest discover -s tests -v
./tools/verify.sh .
```
