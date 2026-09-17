# ADR-0155: Workspace tiering, physical Panels/Design lexicon, and State purity

- Status: Proposed
- Date: 2026-09-15
- Scope: Workspace topology, `crates/**`, `tooling/xtask`, `docs/explanation/architecture.md`;
  amends [ADR-0021](0021-chrome-component-trait.md), [ADR-0044](0044-dock-and-control-center-crates.md),
  [ADR-0046](0046-design-system-crate.md), [ADR-0110](0110-shared-model-crate-naming-and-placement.md)

## Context

The workspace grew organically across 154 architectural decisions into 36 crates distributed across six ad-hoc directories (`api/`, `core/`, `providers/`, `chrome/`, `services/`, `apps/`). While early isolation rules prevented circular dependencies, this layout accumulated severe conceptual debt, terminology jargon, and cognitive friction:

1. **The "API" Bucket Anti-Pattern**: `crates/api/` was chartered for "pure types, zero side effects". Over time it became a dumping ground for non-`core` libraries, housing complex composite UI widgets (`tessera-ui`), material shaders and palette generators (`tessera-design`), and file-system/INI parsers (`tessera-config`, `tessera-desktop-entries`).
2. **"Chrome" and "Shell" Semantic Failures**:
   - **Chrome**: Historically denotes peripheral frames, window borders, and edge decorations. Using "chrome" to encompass floating centered search surfaces (Pivot), full settings panels, and reusable widget toolkits is conceptually distorted.
   - **Shell**: In general computing, "shell" predominantly connotes CLI command-line interpreters (`/bin/sh`, bash) or binary packers/obfuscators. Using "shell" for graphical desktop overlays causes immediate misunderstanding.
3. **The "UI" vs "System-UI" Tautological Collision**:
   Attempting to partition interface code into `ui/` and `system-ui/` creates acute tautological friction (is system-ui not UI? are UI components not for the system?). It fails to capture the orthogonal boundary between **reusable design parts** and **concrete assembled panels**.
4. **"Model" Cognitive Ambiguity**: `tessera-model` carries window topology, workspace states, and keybindings. However, "model" has become heavily overloaded with 3D mesh geometry, database ORM entities, and Large Language Models (LLMs)—directly conflicting with the platform's AI agent broker (`tessera-agent-broker`) and MCP bridges.
5. **Arbitrary Directory Fragmentation**: The `providers/` tier housed only two crates (`tessera-tray`, `tessera-avatar`), whose responsibilities (D-Bus integration and user identity) are indistinguishable from `services/` (such as `tessera-idle` or `tessera-capture`).

The platform requires an uncompromised, long-term workspace architecture founded on **Headless-First design, strict physical-role naming, and semantic precision**.

---

## Decision

### 1. Establish the Canonical Physical-Role Workspace Topology

We replace historical jargon with six strictly orthogonal tiers named after their concrete physical role:

```text
       ┌────────────────────────────────────────────────────────┐
 6     │                        apps/                           │
 Roots │  tessera (compositor)   tessera-lock   tessera-ctl     │
       └──────────────┬──────────────────────────┬──────────────┘
                      ▼                          ▼
       ┌───────────────────────────┐ ┌──────────────────────────┐
 5     │          panels/          │ │      tessera-engine      │
 Surfs │  Desktop Interactive      │ │  Unified Display Server  │
       │  Panels (Dock, HUD, Pivot)│ │  (KMS/Input, Compositor) │
       └──────────────┬────────────┘ └────────────┬─────────────┘
                      ▼                           │
       ┌───────────────────────────┐              │
 4     │          design/          │              │
 Parts │  Design System Tokens,    │              │
       │  Materials, UI Widgets    │              │
       └──────────────┬────────────┘              │
                      ▼                           ▼
       ┌────────────────────────────────────────────────────────┐
 3     │                       services/                        │
 Engine│  100% Headless Platform Capabilities & Daemons          │
       │  (Intent, Launcher, Desktop-Entries, Tray, AI Broker)  │
       └────────────────────────────┬───────────────────────────┘
                                    ▼
       ┌────────────────────────────────────────────────────────┐
 2     │                         api/                           │
 Types │  Pure Immutable Domain State, IPC Schemas & Contracts  │
       │  (Zero rendering dependencies, zero side-effects)      │
       └────────────────────────────────────────────────────────┘
```

### 2. Rename `tessera-model` to `tessera-state`

`tessera-model` is canonically designated as **`tessera-state`**. It represents the pure, deterministic state machines and snapshots of the desktop (windows, workspaces, outputs, keybinds, and authority domains). It remains `#![forbid(unsafe_code)]` and carries zero graphics, OS-handle, or rendering dependencies.

### 3. Replace Chrome/Shell with `panels/` and Establish `design/`

- **`crates/panels/`**: Dedicated exclusively to concrete desktop interactive surfaces assembled from design primitives (`tessera-dock`, `tessera-hud`, `tessera-pivot`, `tessera-control-center`, `tessera-settings`, `tessera-wallpaper`, and `tessera-shell` host).
- **`crates/design/`**: Dedicated exclusively to reusable visual building blocks (`tessera-design` for Liquid Glass materials and tokens, `tessera-ui` for base widgets: buttons, cards, inputs).

### 4. Consolidate Headless Services and Eliminate `providers/`

- `crates/providers/` is permanently abolished. `tessera-tray` and `tessera-avatar` reside in `crates/services/`.
- Configuration parsing (`tessera-config`) and desktop metadata enumeration (`tessera-desktop-entries`, `tessera-icons`, `tessera-commands`) reside in `crates/services/`.

### 5. Enforce Anti-Singleton Invariant

No directory tier under `crates/` may contain exactly one child crate. Singular infrastructure crates sit directly at the `crates/` root as top-level first-class members.

---

## Invariants & Behavioral Boundaries

- **`[INV-ARCH-01]` Headless-First Verification**:
  Every crate under `services/` and `api/` must compile and execute its complete test suite without linking GPU libraries (`flux`, `lens`, `prism`, `wgpu`), without a display server connection, and without graphical context initialization.
- **`[INV-ARCH-02]` Unidirectional Dependency Law**:
  ```text
  api       -> external dependencies only
  services  -> api + core (optional)
  design    -> api
  panels    -> api + design + services (never core/engine)
  apps      -> everything
  ```
- **`[INV-ARCH-03]` Panel Independence**:
  No panel crate in `panels/` may depend on another panel crate in `panels/`. Shared state flows through `api/tessera-state`; shared visuals flow through `design/`; lifecycle is coordinated exclusively by the host.
- **`[INV-ARCH-04]` Anti-Jargon Lexicon**:
  New code and documentation must not introduce "chrome" or "shell" as synonyms for desktop panels or UI components. Concrete surfaces are **Panels**, visual foundations are **Design**.

---

## Alternatives (Negative Knowledge)

- **Use `shell/` instead of `panels/`**: Rejected. In Unix and general systems programming, "shell" strongly implies command-line interpreters or binary wrappers. Using it for GUI overlays invites persistent confusion.
- **Use `ui/` and `system-ui/` simultaneously**: Rejected. Splitting UI into two peer tiers whose names share the identical root causes immediate semantic collision and tautological hesitation during code organization.
- **Keep `tessera-icons` in `design/`**: Rejected. `tessera-icons` performs pure XDG INI file parsing and directory path resolution without touching pixels or rendering. It is a headless platform resource service and belongs in `services/`.

---

## Consequences

### Positive
- Completely eliminates cognitive ambiguity and jargon collisions (no "Shell vs CLI", no "UI vs System-UI").
- Unambiguous physical role separation: `design/` owns the parts, `panels/` owns the assembled desktop surfaces.
- Enforces testability: services, state, and design primitives are verified independently.

### Negative / Follow-up Work
- Mechanical import rewrites where necessary during ongoing module canonicalization.
