# ADR-0156: Engine consolidation and zero-glue display server architecture

- Status: Superseded by [ADR-0158](0158-clean-break-workspace-boundaries.md) and [ADR-0159](0159-package-boundaries-follow-capabilities.md)
- Date: 2026-09-15
- Scope: `crates/core/**` -> `crates/tessera-engine`, `crates/apps/tessera`;
  amends [ADR-0002](0002-hand-rolled-wayland-server.md), [ADR-0003](0003-nested-first-bring-up.md),
  [ADR-0004](0004-client-buffers-via-flux-dmabuf-import.md), [ADR-0011](0011-subsurface-tree-and-z-split-rendering.md),
  [ADR-0015](0015-damage-tracking.md), [ADR-0061](0061-window-tree-atomic-client-surface-compositing.md)

## Context

Historically, the compositor's low-level display server foundation was partitioned across four distinct crates under `crates/core/`:
- `tessera-backend`: Hardware DRM/KMS outputs, libinput device management, libseat session switching, and nested Wayland host windows.
- `tessera-render`: Linux dma-buf zero-copy texture import, CPU shm pixel uploading, damage-graph tracking (`composition_graph.rs`), and z-stack surface compositing via `flux`.
- `tessera-compositor`: Wayland protocol server, client lifecycle, toplevel window state, focus arbitration, and Universal Interaction Protocol (UIP) dispatch.
- `tessera-wayland-protocols`: XML-generated C ABI protocol bindings.

### The Failure Mode of Artificial Core Slicing

Comprehensive dependency inspection reveals that **no external crate outside `crates/apps/tessera` (the composition root) references `tessera-backend`, `tessera-render`, or `tessera-compositor`**. Strict architecture rules forbade `shell`, `services`, or `api` from directly touching core crates.

Consequently, partitioning this subsystem across four Cargo boundaries yielded no reuse benefits, while creating severe engineering friction:
1. **Proliferation of Fragile FFI & Descriptor Glue**: Raw Vulkan device pointers, DRM device nodes, and borrowed file descriptors (`BorrowedFd`) had to be wrapped in public types, explicitly annotated across crate APIs, and marshaled through multi-stage setup functions in `apps/tessera`.
2. **Pathological Code Duplication**: To maintain crate boundary compliance without circular dependencies, identical platform code was duplicated across crates. For example, `tessera-compositor/src/protocol/viewport.rs` contains verbatim copies of transform matrix logic:
   > `// duplicated here so tessera-compositor stays independent of tessera-backend.`
3. **Impedance Mismatch with Upstream Optics**: Optics (`flux`) provides rendering primitives, but does not own Linux kernel KMS atomic page flips or Wayland protocol state. Treating backend, render, and server as independent crates obscured the fact that they are simply internal organs of a single **Display Server Engine**.

---

## Decision

### 1. Consolidate into a Single High-Cohesion `tessera-engine` Crate

We dissolve `crates/core/` and merge the four crates into a single, unified, headless engine crate located at **`crates/tessera-engine`**:

```text
crates/tessera-engine/
├── Cargo.toml
├── build.rs               # Consolidates wayland-scanner XML code-generation
└── src/
    ├── lib.rs             # Minimal, safe public engine entry point
    ├── backend/           # (Formerly tessera-backend)
    │   ├── drm/           # KMS atomic modesetting, CRTC, pageflip, connectors
    │   ├── input/         # libinput scanning, pointer, keyboard, tablet, gestures
    │   ├── seat/          # libseat VT switching and seat authority
    │   └── nested/        # Development window runtime via ash/VkSurfaceKHR
    ├── render/            # (Formerly tessera-render)
    │   ├── dmabuf.rs      # Kernel dma-buf zero-copy import into flux::Texture
    │   ├── shm.rs         # wl_shm memory staging and texture cache
    │   ├── damage.rs      # Damage-graph tracking & partial repaint ROI planning
    │   └── composite.rs   # Surface tree z-order traversal and flux draw dispatch
    ├── server/            # (Formerly tessera-compositor)
    │   ├── protocol/      # Wayland protocol implementations (xdg_shell, wl_seat, etc.)
    │   ├── window/        # Window tree, workspace mapping, and focus management
    │   └── uip/           # Universal Interaction Protocol frame dispatch
    └── protocols/         # Generated Wayland C ABI bindings
```

### 2. Private In-Process Boundary & Zero Glue

All interactions between backend DRM framebuffers, flux device instances, client buffer imports, and compositor commit hooks are internalized as standard Rust module-level visibility (`pub(crate)` / `pub(super)`):
- All duplicated code (e.g. viewport transforms, DRM format conversion tables) is immediately consolidated.
- Inter-crate conversion boilerplate, redundant error types, and intermediate public structs are permanently excised.
- The public API of `tessera-engine` exposes only high-level session, frame-pump, and scene abstractions to the binary composition root (`apps/tessera`).

### 3. Absolute Headless Invariant

`tessera-engine` is strictly **Headless**:
- It owns the plumbing to present buffers to hardware displays, but **it contains zero UI, zero widget drawing, zero fonts, and zero product policy**.
- It does not depend on `tessera-design`, `tessera-widgets`, or any `shell/*` crate.
- It can be initialized in headless/virtual mode for automated continuous-integration testing without a GPU or physical monitor attached.

---

## Invariants & Behavioral Boundaries

- **`[INV-ENG-01]` Zero Shell Awareness**:
  `tessera-engine` must never import or reference `shell/*` or `ui/*` crates. It manages Wayland client surfaces, subsurfaces, and presentation timing, remaining entirely agnostic to system shell surfaces.
- **`[INV-ENG-02]` Internal Safe Seams**:
  Unsafe code necessitated by Linux kernel ioctls (DRM/KMS), libinput FFI, and Vulkan extensions must be contained inside `src/backend/` and `src/render/dmabuf.rs`. `src/server/` remains safe code.
- **`[INV-ENG-03]` Single Source of Truth for Protocols**:
  Wayland protocol XML definitions are scanned and compiled in `tessera-engine/build.rs`. No other crate in the workspace may invoke `wayland-scanner`.

---

## Alternatives (Negative Knowledge)

- **Keep `backend` and `render` separate, merge only `compositor`**: Rejected. `render` requires the DRM device node and Vulkan memory exports created by `backend`, while `backend` requires the presented frame tokens produced by `render`. Splitting them requires circular type passing without external reuse value.
- **Re-export modules through a facade crate while keeping 4 crates**: Rejected. Facade crates preserve compile-time overhead, require maintaining 4 separate `Cargo.toml` files, and fail to resolve internal code duplication.

---

## Consequences

### Positive
- Eliminates 4 `Cargo.toml` manifests, several thousand lines of cross-crate glue, and duplicate transform tables.
- Substantially improves incremental compile and link times for core modifications.
- Simplifies the conceptual model: the platform has exactly **one** display server engine.

### Negative / Follow-up Work
- Requires updating `apps/tessera/src/main.rs` and `apps/tessera/src/runtime/` to import from `tessera_engine::*`.
