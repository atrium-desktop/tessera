# Documentation Taxonomy & Layout

This document defines the organization, scope boundaries, and classification of
all documentation assets in the Tessera repository.

---

## 1. The Four-Quadrant System (Diátaxis)

Public user-facing documentation in `docs/` follows the Diátaxis architecture:

```text
               PRACTICAL                   THEORETICAL
          ┌─────────────────────────┬─────────────────────────┐
          │  Tutorials              │  How-To Guides          │
LEARNING  │  (Learning-Oriented)    │  (Task-Oriented)        │
          │  `docs/tutorials/`      │  `docs/how-to/`         │
          ├─────────────────────────┼─────────────────────────┤
          │  Reference              │  Explanation            │
WORKING   │  (Information-Oriented) │  (Understanding-Orient) │
          │  `docs/reference/`      │  `docs/explanation/`    │
          └─────────────────────────┴─────────────────────────┘
```

- **`docs/tutorials/`**: Guided walk-throughs taking a new user through their
  first successful run (e.g., getting started).
- **`docs/how-to/`**: Step-by-step problem-solving recipes for specific user
  goals (e.g., configuring a wallpaper, installing the lock client).
- **`docs/reference/`**: Technical descriptions of interfaces without tutorial
  prose — configuration keys, CLI commands, IPC messages, settings routes.
- **`docs/explanation/`**: Architectural deep dives, design rationale, and
  background discussion (e.g., the compositor architecture, the agent phase).

---

## 2. Scope Boundaries (Public vs. Internal)

Tessera enforces a strict audience boundary between public consumers and
internal maintainers:

```text
┌─────────────────────────────────────────────────────────────┐
│ Public Surface (Users, Integrators, End Developers)         │
│ `docs/tutorials/`, `how-to/`, `reference/`, `explanation/`  │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               │ Scope Boundary: Public docs must
                               │ NEVER link to internal docs.
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Internal Surface (Maintainers, Core Contributors, Agents)   │
│ `docs/dev/` (setup, internals, testing, triage, standards)  │
│ `docs/adr/` (binding architectural decision records)        │
└─────────────────────────────────────────────────────────────┘
```

1. **Firewall Rule**: Public documentation surfaces
   (`docs/{tutorials,how-to,reference,explanation}/`) must NEVER contain
   relative links pointing into internal spaces (`docs/dev/**`).
2. **Reverse Linking Permitted**: Internal documents may freely link outward to
   public reference and how-to guides.
3. **Decision Records Are Internal**: ADRs under `docs/adr/` are contributor
   material; public docs cite them only through the
   [ADR index](../../adr/index.md) when a user-visible rationale matters.

---

## 3. Repository Root Whitelist

To keep the repository root clean and clutter-free, only canonical top-level
documentation files are permitted at the root:

- `README.md`
- `CHANGELOG.md`
- `CONTRIBUTING.md`
- `AGENTS.md` (with `CLAUDE.md` and `GEMINI.md` as symlinks to it)
- `LICENSE`
- `SECURITY.md` (when vulnerability reporting policy exists)

All other documentation must reside inside `docs/`. Scratch exports, session
dumps, and editor artifacts never belong at the root; `/target`, `/.cargo/`,
and the local `.muta/` workspace metadata directory are ignored by Git.

---

## 4. Knowledge Lifecycle

| Temperature | Content | Cadence |
| :--- | :--- | :--- |
| **HOT** | Living ground truth: `README.md`, `docs/{tutorials,how-to,reference,explanation}/`, `docs/dev/`. | Updated in the same change as the code it describes. |
| **WARM** | Active decision contracts: `docs/adr/` (draft, accepted). | Created or amended upon architectural review. |
| **COLD** | Historical rationale: superseded and rejected ADRs, triage post-mortems. | Retained in place; retrieved during forensics and refactoring. |

Superseded and rejected records are never deleted and never moved; their status
is recorded in frontmatter. See [ADR Standards](adr.md) for the in-place
lifecycle.
