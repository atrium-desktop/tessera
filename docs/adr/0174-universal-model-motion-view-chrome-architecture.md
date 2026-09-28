---
id: ADR-0174
title: "Universal Model-Motion-View Architecture and Full Transit Adoption Across Shell Chrome"
status: accepted
date: 2026-09-28
scope: shell, motion, headless-architecture, optics-integration, dual-citizen
superseded_by: null
negative_knowledge: true
---

# 0174. Universal Model-Motion-View Architecture and Full Transit Adoption Across Shell Chrome

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Consulted: Optics Graphics Team & Accessibility Working Group
- Informed: Compositor Runtime & AI Agent Platform Team
- Amends: [ADR-0021](0021-chrome-component-trait.md), [ADR-0139](0139-animation-effect-placement.md), [ADR-0151](0151-pivot-unified-intent-and-action-surface.md), [ADR-0157](0157-headless-intent-subsystem-and-pivot-surface-decoupling.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md), [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md)

---

## Context and Problem Statement

Following the adoption of Optics `transit` in [ADR-0172](0172-adopt-optics-transit-motion-vocabulary.md) and the renaming of the composition pipeline in [ADR-0173](0173-rename-render-to-composite.md), two architectural anomalies remain in the human-facing desktop shell (`tessera-shell`):

1. **Scattered Math & Incomplete Transit Adoption**: While the primary tile and mode-switch springs now re-export `transit`, multiple chrome components still embed bespoke exponential follow formulas (`1.0 - (-RATE * dt).exp()`) for visibility fades and cursor tracking:
   - `Pivot`: Hand-rolled exponential blend in `advance_visibility` (`src/components/pivot/mod.rs`).
   - `HUD`: Manual exponential approaches in `chip_fade` and `workspace_position` (`src/components/hud/hud.rs`).
   - `Control Center`: Scattered ad-hoc scrollbar and tooltip decay formulas (`src/components/control_center/mod.rs`).
   - This violates the foundational contract of [ADR-0139](0139-animation-effect-placement.md) and Optics ADR-0077: *Optics owns motion mechanism, callers own policy*.
2. **Coupled State and Rendering (The "Fat Component" Problem)**:
   Several shell components (notably `Pivot` at 1,200 lines, and parts of `Dock` and `ControlCenter`) interleave input event parsing, state machine mutations, animation stepping, and Lens UI layout in single monolithic structures.
   - This prevents sub-millisecond headless unit testing of component state.
   - It violates the **Dual-Citizen Principle** ([Vision & Scope](../../docs/explanation/vision.md#the-two-phases)): an AI Agent or CLI tool inspecting or operating system actions through `tessera-mcp` should be able to query and drive headless component state without executing GPU layout passes or linking immediate-mode rendering dependencies.

We need an uncompromised, future-proof architectural model that standardizes chrome implementation across the entire compositor.

## Decision Drivers

- **Absolute Zero-Math Duplication**: Completely eliminate all manual `.exp()` loops, ad-hoc decay formulas, and custom integrators across `tessera-shell`. All scalar travel must delegate to Optics `transit` (`transit::approach`, `transit::decay`, `transit::Spring`, `transit::Smoother`, `transit::Hysteresis`).
- **Universal Model-Motion-View (MMV) Separation**: Enforce a strict three-tier module structure within every chrome component (`dock`, `control_center`, `pivot`, `hud`):
  - `state.rs`: 100% Headless, deterministic state machine ($S_{t+1} = f(S_t, \text{Event})$). Zero GPU/Vulkan, zero `lens::Frame`, zero pixel dependencies. Sub-millisecond unit testable.
  - `motion.rs`: Physical dynamics parameters (`SpringParams`, rates, thresholds) stepping `transit` primitives.
  - `rendering.rs` / `presentation.rs`: Pure stateless projection from $(S, M) \rightarrow \text{Lens DrawLists}$.
- **Dual-Citizen Parity (Human & Agent Equality)**: State machines expose deterministic query and action APIs (`query()`, `execute()`) consumable with identical semantics by human UI events and out-of-process AI Agents via `tessera-mcp`.
- **Zero Package Bloat**: Adhere strictly to [ADR-0159](0159-package-boundaries-follow-capabilities.md) and [ADR-0164](0164-universal-domain-pruning-and-primitives-canonization.md): MMV separation is enforced at the **module level within `tessera-shell`**, not by spawning dozens of micro-crates.

## Considered Options

- **Option 1: Universal Model-Motion-View (MMV) with Complete Transit Delegation (Chosen)**:
  Standardize all components in `tessera-shell` onto `(state.rs, motion.rs, rendering.rs)`. Replace all ad-hoc exponential math with `transit::approach` and `transit::decay`. Expose headless state APIs for agent inspection.
- **Option 2: Micro-crate Decomposition (Separate `*-core` and `*-ui` packages)**:
  Extract every chrome component into standalone headless crates (e.g., `tessera-dock-core`, `tessera-dock-ui`).
- **Option 3: Incremental Patching (Status Quo with Local Helpers)**:
  Retain existing monolithic component structs and add helper methods on demand.

## Decision Outcome

Chosen option: **Option 1: Universal Model-Motion-View (MMV) with Complete Transit Delegation**.

### Architecture & Module Blueprint

Every first-party shell component under `crates/tessera-shell/src/components/<name>/` conforms to the canonical three-tier layout:

```text
crates/tessera-shell/src/components/<component>/
├── mod.rs          # Component lifecycle, Chrome trait implementation, and public facade
├── state.rs        # ★ Headless State Machine (Pure Rust, deterministic, zero UI types)
│                   #   - Holds data, selection indices, dwell timers, and flags
│                   #   - Exports tick(event) -> TransitionOutcome
│                   #   - Exposes Dual-Citizen inspection & action dispatch
├── motion.rs       # ★ Motion Dynamics & Optics Transit Binding
│                   #   - Defines const SpringParams tuning presets
│                   #   - Owns transit::Spring, transit::Smoother, or approach rates
│                   #   - Enforces reduced_motion in <= 1 frame
└── rendering.rs    # ★ Pure Presentation Projection
                    #   - Inputs: (&state, &motion, &tokens)
                    #   - Outputs: Lens Frame nodes, Glass materials, DrawLists
```

### Invariants & Behavioral Boundaries

- `[INV-ARCH-49] Zero-Math Invariant in Chrome`: No component in `tessera-shell` MAY invoke `f32::exp` or custom numerical integration directly for visual motion. All continuous scalar updates MUST delegate to `transit` via `crate::widgets::motion` (`approach`, `decay`, `Spring`, `Smoother`, `Hysteresis`).
- `[INV-ARCH-50] Pure Headless State Boundary`: The `state.rs` module of any chrome component MUST NOT import `lens::Frame`, `lens::LayoutOpts`, `tessera_design::materials`, or Vulkan/GPU types. State mutations MUST be purely functional and testable without mock display contexts.
- `[INV-ARCH-51] Dual-Citizen State Equivalence`: Any user action triggerable through UI clicks or keypresses MUST map to a publicly accessible method on the component's headless state, ensuring 100% parity with agent actions dispatched over `tessera-mcp` or IPC.

## Rejected Alternatives & Negative Knowledge

### Micro-crate Decomposition (Option 2)
- **Why considered**: Provides the strictest physical compiler firewall between headless logic and rendering dependencies.
- **Why rejected**: Rejected by [ADR-0159](0159-package-boundaries-follow-capabilities.md) and [ADR-0164](0164-universal-domain-pruning-and-primitives-canonization.md). Splitting internal chrome components into 8+ separate Cargo packages creates severe "Micro-crate Mania": duplicate Cargo.toml manifests, redundant CI build graphs, ceremonial DTO wrappers, and high impedance mismatch, with zero gain since chrome components are never consumed independently outside `tessera-shell`. Module-level boundaries (`pub(crate)`) achieve identical isolation without packaging overhead.

### Retaining Ad-Hoc Exponential Math (Option 3)
- **Why considered**: Avoided touching working animation code in `Pivot` and `HUD`.
- **Why rejected**: Leads to fragmentation and tuning drift. Optics ADR-0077 and ADR-0106 specifically created `transit` to provide a proven, non-divergent, NaN-resilient motion vocabulary. Leaving local `.exp()` formulas re-invites the exact numerical bugs that prompted the creation of `transit`.

## Consequences

### Positive
- 100% uniform motion behavior across Dock, Control Center, Pivot, and HUD.
- Headless testing of chrome component logic runs in sub-millisecond execution times.
- Frictionless exposure of desktop state to out-of-process AI Agents via `tessera-mcp`.
- Clean, predictable code navigation across all chrome subsystems.

### Negative / Trade-offs
- Requires refactoring existing ad-hoc exponential loops in `pivot` and `hud` onto `transit::approach` and `transit::Spring`.
