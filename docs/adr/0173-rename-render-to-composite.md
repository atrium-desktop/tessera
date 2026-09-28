---
id: ADR-0173
title: "Rename tessera-render to tessera-composite"
status: accepted
date: 2026-09-28
scope: core/compositing, architecture/workspace
superseded_by: null
negative_knowledge: true
---

# 0173. Rename tessera-render to tessera-composite

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Consulted: Optics Graphics Team & Compositor Runtime Group
- Informed: Desktop Working Group
- Amends: [ADR-0158](0158-clean-break-workspace-boundaries.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md), [ADR-0161](0161-rename-backend-to-platform.md)

---

## Context and Problem Statement

The crate previously named `tessera-render` is responsible for:
- Linux dma-buf zero-copy texture import and CPU shm pixel uploading;
- Z-order layer stacking, surface hierarchy traversal, and occlusion culling;
- Parametric color management (`wp_color_management_v1`) to target display colorimetry;
- Per-surface damage tracking and Vulkan scissor rectangle computation;
- Direct KMS scanout eligibility evaluation (zero-copy hardware plane bypass vs GPU composite pass);
- Composition DAG planning (`composition_graph.rs`) and backdrop blur caching.

The package description historically read: *"Compositing: client buffers to flux textures, scene to the output via flux"*.

However, naming this crate `tessera-render` introduced a persistent **semantic conflict and architectural ambiguity**:
1. **Conflation with the Underlying Graphics Engine**: In modern systems architecture, "rendering" refers to rasterization, vector path calculation, shader compilation, and drawing primitives. That responsibility belongs strictly to the **Optics** monorepo (`flux` for Vulkan RHI and 2D canvas, `prism` for dielectric material compute shaders, and `lens` for declarative UI widgets). `tessera-render` contains zero drawing routines, zero rasterization logic, and zero custom shader programs.
2. **Industry Terminology Divergence**: Across contemporary operating systems and window managers, the layer that ingests client buffers, resolves occlusion, and schedules scanout is universally designated as the **Compositor**:
   - Chromium explicitly separates `Blink / Skia` (Renderers) from `cc` (Chrome Compositor).
   - Android separates `HWUI / Skia` (Renderers) from `SurfaceFlinger` (Compositor).
   - macOS separates `CoreGraphics / Metal` (Renderers) from `Quartz Compositor` (WindowServer).
   - Wayland is by specification a *display server compositor protocol*.

Continuing to call this module `tessera-render` created cognitive friction, misleading developers into expecting drawing mechanisms inside the compositor rather than delegation to Optics.

## Decision Drivers

- **Semantic Precision**: Package names must unambiguously reflect their true systems role: layer composition, damage tracking, and scanout scheduling, not drawing.
- **Architectural Symmetry**: Reinforcing the foundational triad established in ADR-0161:
  1. **`tessera-platform`**: Physical hardware adaptation (DRM/KMS modesetting, input devices, seat sessions).
  2. **`tessera-wayland`**: Wayland client protocol handlers and buffer state machines.
  3. **`tessera-composite`**: Surface layer composition, damage scissoring, and scanout bypass orchestrating Optics.
- **Clear Demarcation with Optics**: Optics owns 100% of GPU rendering and material physics (ADR-0001, ADR-0139, ADR-0172); `tessera-composite` owns 100% of compositor-domain surface assembly.

## Considered Options

- **Option 1: Rename `tessera-render` to `tessera-composite` (Chosen)**: Rename the crate directory, package name, and workspace references to `tessera-composite`, aligning with the industry-standard "Compositor" domain.
- **Option 2: Rename `tessera-render` to `tessera-presentation`**: Emphasizes the output presentation pipeline (vblank timing, KMS plane scheduling), but obscures its primary role of multi-layer surface and texture composition.
- **Option 3: Retain `tessera-render` (Status Quo)**: Retain the historical name and attempt to clarify the distinction in documentation.

## Decision Outcome

Chosen option: **Option 1: Rename `tessera-render` to `tessera-composite`**.

The crate directory `crates/tessera-render/` is renamed to `crates/tessera-composite/`, its package manifest declares `name = "tessera-composite"`, and references in `crates/tessera` and tooling policies are migrated completely.

### Invariants & Boundaries

- `[INV-ARCH-47] Zero-Drawing Invariant in Composite`: `tessera-composite` MUST NOT implement custom rasterization pipelines or drawing primitives. All GPU execution is delegated through the `flux` / `prism` / `lens` interfaces of Optics.
- `[INV-ARCH-48] Composition Seam Isolation`: Only the application composition root (`crates/tessera`) MAY depend on `tessera-composite`. Domain packages (`tessera-desktop`, `tessera-authority`, `tessera-launch-services`) and shell chrome (`tessera-shell`) remain decoupled from composite engine internals.

## Rejected Alternatives & Negative Knowledge

### Rename to `tessera-presentation` (Option 2)
- **Why considered**: `tessera-render` already contains an extensive `presentation/` submodule covering scanout planning and damage.
- **Why rejected**: Presentation is only the tail end of the pipeline (submitting frames to display hardware). The crate's broader responsibility is importing heterogeneous client memory (DMA-BUF, SHM), resolving occlusion, managing parametric HDR colorimetry, and building the offscreen composition graph. "Composite" captures the full lifecycle.

### Retain `tessera-render` (Option 3)
- **Why considered**: Avoided cross-file refactoring in `crates/tessera`.
- **Why rejected**: Preserving a misnomer to save renaming edits is the definition of accumulating technical and cognitive debt. Aligning terminology with Optics and industry reality eliminates recurring architectural confusion.

## Consequences

### Positive
- Crystal-clear role boundary: Optics is the *Renderer*, `tessera-composite` is the *Compositor*.
- Complete harmony across the display triad: `tessera-platform`, `tessera-wayland`, `tessera-composite`.
- Zero legacy facade packages left behind; manifest and directory match 1:1.

### Negative / Trade-offs
- Internal imports in `crates/tessera/src/runtime/**` update from `tessera_render` to `tessera_composite`.
- Tooling policies in `tooling/xtask/dependency-policy.toml` must be synchronized.
