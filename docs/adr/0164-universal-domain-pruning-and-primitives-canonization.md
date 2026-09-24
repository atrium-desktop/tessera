---
id: ADR-0164
title: "Universal Domain Pruning and Primitives Canonization"
status: accepted
date: 2026-09-19
scope: architecture/workspace, core/primitives, services/launch
superseded_by: null
negative_knowledge: true
---

# 0164. Universal Domain Pruning and Primitives Canonization

- Status: Accepted
- Date: 2026-09-19
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0158](0158-clean-break-workspace-boundaries.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md), [ADR-0160](0160-consolidate-application-subsystem.md)

---

## Context and Problem Statement

The platform has undergone rapid capability additions, leading to **Micro-crate Mania (微包通胀)** and **Concept Inflation (概念抽象过度)** across the workspace:

1. **Semantic Void of `tessera-types`**: Calling a leaf library `types` hides its domain value; in a statically typed language, everything is a type.
2. **Naming Mismatch of `tessera-scene`**: It defines zero scene-graph rendering nodes, but instead owns physical display geometry (`SurfaceGeometry`), DRM FourCC buffer layout, and HDR/EDID colorimetry. The split between `types` and `scene` created artificial dependency barriers between `types::Rect` and `scene::Transform`.
3. **Plural Ambiguity of `tessera-apps`**: The name suggests a bundle of userland application programs (e.g. calculator, text editor), while its true role is the system-level **Launch Services & Application Catalog** (XDG `.desktop` scanning, icon resolution, sandboxed process execution, and intent search).
4. **Micro-crates Under 1,000 Lines**: Six micro-packages (`tessera-bootstrap` [274 lines], `tessera-wayland-protocols` [180 lines], `tessera-semantic` [608 lines], `tessera-i18n` [906 lines], `tessera-tray` [1,575 lines], and `tessera-idle` [1,587 lines]) introduce ritual overhead, Cargo metadata bloat, and compile-time latency without genuine capability or process isolation.

[ADR-0159](0159-package-boundaries-follow-capabilities.md) firmly established that **"Package per responsibility is REJECTED; use modules by default"**, and [ADR-0160](0160-consolidate-application-subsystem.md) consolidated four micro-crates into `tessera-apps`. This record takes the next uncompromised step in that continuum.

## Decision Drivers

- **MECE Domain Orthogonality**: Consolidate the entire compositor and desktop environment into 8 coherent, mutually exclusive domains.
- **Cognitive Ergonomics**: Package names must immediately communicate physical domain responsibility without jargon or euphemism.
- **Zero-Logic Core Foundation**: Establish an indivisible, zero-dependency, `#![forbid(unsafe_code)]` primitives layer (`tessera-primitives`).
- **Reduced Compilation Ritual**: Eliminate ceremonial packaging overhead and restore Rust's private module encapsulation (`pub(crate)`).

## Decision Outcome

1. **Canonize `tessera-primitives`**:
   - Merge `tessera-types` and `tessera-scene` into a unified, zero-dependency foundational crate: **`crates/tessera-primitives`**.
   - Modularized into: `geometry` (Point, Rect, Size, Transform, SurfaceGeometry), `color` (ColorSpace, Primaries, EOTF, HDR, EDID), `buffer` (FourCC, DmaBufFormat, Modifiers), `identity` (WindowId, WorkspaceId, OutputId, SeatId), and `input` (KeyChar, KeyModifiers, PointerButton).
   - `tessera-types` and `tessera-scene` act as compatibility facades during migration and are subsequently deprecated.

2. **Re-canonize `tessera-launch-services`**:
   - Rename and elevate `tessera-apps` to **`tessera-launch-services`**.
   - Retains pure headless modularity: `entries` (XDG parsing), `icons` (freedesktop lookup), `launcher` (cgroup/bwrap spawning), and `search` (intent matching).
   - Retained as an independent crate to allow CLI (`tessera-cli`), portals, and diagnostic tools to consume application catalog services without linking the Wayland compositor or `tessera-desktop` window state.

3. **In-tree Chrome Cohesion**:
   - The intent and application search surface **`Pivot` remains strictly inside `tessera-shell`** alongside `Dock`, `HUD`, and `ControlCenter`. No separate UI micro-crate is permitted.

4. **Absorb Ritual Micro-crates**:
   - `tessera-wayland-protocols` $\rightarrow$ internal build logic in `tessera-wayland`.
   - `tessera-bootstrap` $\rightarrow$ internal tracing bootstrap in `tessera` / `tessera-platform`.
   - `tessera-semantic` $\rightarrow$ absorbed into `tessera-authority::ui_tree` as an internal zero-trust A11y validation module.
   - `tessera-i18n` $\rightarrow$ absorbed into `tessera-design` / `tessera-shell`.
   - `tessera-tray` & `tessera-idle` $\rightarrow$ absorbed into `tessera-desktop::services`.

### Invariants & Behavioral Boundaries

- `[INV-PRIM-01] Zero-Logic Primitives`: `tessera-primitives` MUST remain `#![forbid(unsafe_code)]`, zero-I/O, zero-GPU-handle, and free of runtime event loops.
- `[INV-LAUNCH-01] Headless Launch Services`: `tessera-launch-services` MUST NOT depend on `tessera-shell`, Lens, Flux, or any GUI rendering stack.
- `[INV-BOUND-02] Monitored Capability Edges`: Capability edges remain explicitly verified by `xtask check-boundaries` via `dependency-policy.toml`.

## Rejected Alternatives & Negative Knowledge

### Monolithic Single Crate
- **Why considered**: Simplest possible build tree.
- **Why rejected**: Rejected by [ADR-0158](0158-clean-break-workspace-boundaries.md). Blurs unsafe boundaries (DRM vs pure geometry), forces headless tools (`tessera-cli`) to link Vulkan and Wayland, and ruins parallel compilation.

### Merging Launch Services into `tessera-desktop`
- **Why considered**: Desktop manages windows and sessions; launching applications is closely related.
- **Why rejected**: `tessera-desktop` owns the compositor's window tree, workspace layouts, and active seats. Headless tools like `tessera-cli` need to inspect desktop entries and icons without compiling or linking the window manager state machine.

### Keeping `types` and `scene` Split
- **Why considered**: `types` had zero dependencies; `scene` had color definitions.
- **Why rejected**: Creates a split personality where `SurfaceGeometry` in `scene` had to import `Rect` from `types`. Both are descriptive mathematical and physical facts; separating them across package boundaries served no ownership purpose.

## Consequences

- Workspace crate count contracts from 27 to 19 coarse-grained, highly cohesive domain packages.
- Compile and link times decrease by avoiding redundant rlib packaging for micro-libraries.
- API surface shrinks by restoring module-private encapsulation (`pub(crate)`).