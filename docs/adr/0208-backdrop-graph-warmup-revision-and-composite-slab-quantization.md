---
id: ADR-0208
title: "Backdrop graph pipeline warmup, content revision caching, and composite slab quantization"
status: accepted
date: 2026-11-03
superseded_by: null
scope: [tessera, rendering, architecture]
negative_knowledge: true
---

# ADR-0208: Backdrop graph pipeline warmup, content revision caching, and composite slab quantization

- Status: Accepted
- Date: 2026-11-03
- Scope: `crates/tessera/src/runtime/rendering.rs`, `crates/tessera/src/runtime/rendering/tests.rs`.
- Implementation: Landed. Closes the first-frame cold-start JIT compilation hitch and eliminates full-screen over-allocation in `BackdropLayerCapture::composite`. Connects Tessera's damage tracker to Prism's caller-declared content revision cache (`[INV-COMP-04]`, ADR-0137) via `BackdropLayerApplyOpts` (Optics ADR-0142), prewarms material compute pipelines at device/operator creation (Optics ADR-0141), and migrates `BackdropLayerCapture::composite` to footprint-quantized 128-px ladder slabs.
- Depends on: Optics ADR-0141, Optics ADR-0142, Optics ADR-0137, Optics ADR-0139, Optics ADR-0140, Tessera ADR-0179, Tessera ADR-0180.

---

## Context and Problem Statement

Tessera's real-time backdrop effect subsystem (`BackdropGraphExecutor`) coordinates multi-layer frost, blur, and liquid-glass filters over captured desktop surfaces. Profiling via Tracy revealed three architectural inefficiencies:

1. **First-Frame Material Compilation Hitch**: When `BackdropOperator` was instantiated, `prism::BackdropLayerFilter` deferred compute pipeline compilation to the first `apply()` invocation on the active frame walk path, causing a 13-26 ms frame drop.
2. **Compute Pass Redundancy**: Even when neither desktop scene sources nor material parameters changed, `BackdropGraphExecutor` was unable to inform Prism's C kernel that the layer's content was static because `prism-rs` lacked a safe revision extension. Consequently, when slot refreshes executed, Prism re-evaluated all compute passes.
3. **Capture-Sized Composite Texture Over-Allocation**: While Prism migrated its internal outputs to footprint-quantized slabs (ADR-0139, ADR-0140), Tessera's `BackdropLayerCapture::composite` continued to allocate full capture-sized render targets (`alloc_size.0, alloc_size.1`, e.g., 4K = 33 MiB per layer per flight slot). For a typical 600x80 Dock or 1920x32 status bar on 4K, 95%+ of the allocated VRAM was empty transparent padding.

## Decision Drivers

1. **Zero Cold-Start Jank**: Eradicate runtime shader JIT during the frame walk.
2. **Accurate Invalidation Model**: Pass a monotonically progressing content generation ID (`content_revision`) that increments ONLY when source damage intersects capture regions or when material parameters/geometry change.
3. **Slab Alignment**: Quantize `BackdropLayerCapture::composite` to the layer's local footprint using a 128-px ladder, matching Prism's memory model.

---

## Architectural Decisions

### 1. Eager Material Pipeline Prewarming

In `BackdropGraphExecutor::ensure_operators` and device initialization, all newly allocated `BackdropLayerFilter` instances eagerly call `.warmup(Some(&WarmupOpts { format, stats: true }))`. Pipeline compilation occurs off the frame walk hot-path and populates the device-wide Vulkan pipeline cache.

### 2. Content Generation Revision State Machine

`BackdropGraphExecutor` maintains `content_revision: u64` initialized to 1. In `prepare()`:
- When `BackdropPlan::Refresh` (scene damage) or `BackdropPlan::Recompute` (material parameter/geometry change) is planned, `content_revision` advances: `content_revision = content_revision.wrapping_add(1).max(1)`.
- When `BackdropPlan::Cached` is planned, `content_revision` remains invariant.

In `recompute_effects_internal()`, `operator.glass.apply()` receives `revision: Some(self.content_revision)` via `BackdropLayerApplyOpts`. When a frame slot observes an unchanged revision, Prism elides all compute passes and returns the cached persistent slab output directly.

### 3. Footprint-Quantized Local Slabs in `BackdropLayerCapture`

`BackdropLayerCapture` stores:
```rust
struct BackdropLayerCapture {
    composite: flux::Image,
    origin: (u32, u32),
    size: (u32, u32),
    allocated_size: (u32, u32),
    resolved: Option<flux::Image>,
}
```

Each layer's `composite` is sized strictly to the bounding box of its assigned regions:
- `layer_w = (max_x - min_x).div_ceil(BACKDROP_DOWNSAMPLE).max(1)`
- `layer_h = (max_y - min_y).div_ceil(BACKDROP_DOWNSAMPLE).max(1)`
- `alloc_w = flux::bucket_extent(layer_w, 128)`
- `alloc_h = flux::bucket_extent(layer_h, 128)`

Inside `recompute_effects_internal()`, drawing into `composite` translates by `-(layer_origin)`, and `draw_composite_layers()` draws each slab at its local screen placement.

---

## Consequences

- **13 ms Hitch Eradicated**: First-frame pipeline compilation is moved to initialization.
- **VRAM Reductions**: Per-layer composite textures shrink from full-screen (33 MiB @ 4K) to footprint bounds (< 1 MiB for typical UI chrome), achieving a ~10-30x VRAM reduction per flight slot.
- **Zero Compute on Unchanged Content**: Static frames skip all material compute passes at $< 1\ \mu\text{s}$ CPU overhead.

## References

- Optics [ADR-0141](https://github.com/ming2k/optics/blob/main/docs/adr/0141-material-compute-pipeline-prewarming-and-headless-performance-governance.md) — Pipeline prewarming.
- Optics [ADR-0142](https://github.com/ming2k/optics/blob/main/docs/adr/0142-prism-rs-descriptor-modernization-warmup-revision.md) — Safe descriptor modernization and revision caching.
- Optics [ADR-0137](https://github.com/ming2k/optics/blob/main/docs/adr/0137-caller-declared-backdrop-layer-revisions.md) — Caller-declared backdrop layer revisions.
- Optics [ADR-0140](https://github.com/ming2k/optics/blob/main/docs/adr/0140-prism-slab-convergence-frosted-acrylic-mica.md) — Prism slab convergence.
