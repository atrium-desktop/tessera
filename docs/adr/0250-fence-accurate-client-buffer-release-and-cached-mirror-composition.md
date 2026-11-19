---
id: ADR-0250
title: "Fence-Accurate Client Buffer Release and Cached Partial-Damage Mirror Composition"
status: accepted
date: 2026-11-20
scope: runtime/presentation, runtime/output_target, runtime/damage, platform/drm, platform/sync, wayland/server, docs
superseded_by: null
negative_knowledge: true
---

# 0250. Fence-Accurate Client Buffer Release and Cached Partial-Damage Mirror Composition

- Status: Accepted
- Date: 2026-11-20
- Amends: [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md), [ADR-0246](0246-mirror-scaling-falls-back-to-a-compositor-target.md), [ADR-0247](0247-per-output-render-targets.md), [ADR-0249](0249-presentation-groups-and-per-output-pacing.md)

## Context

Hardware-accelerated Wayland clients (`zwp_linux_dmabuf_v1` — e.g. Chromium/Chrome
and Firefox/Gecko) operate small swapchains of 2–4 DMA-BUF slots, whereas
`wl_shm` clients (such as `foot`) copy pixels on commit and receive
`wl_buffer.release` immediately.

Under the per-output target pipeline introduced in
[ADR-0247](0247-per-output-render-targets.md) and
[ADR-0249](0249-presentation-groups-and-per-output-pacing.md), GPU-backed
clients exhibited severe frame drops and multi-frame stalls while SHM clients
remained smooth. Root-cause diagnosis identified two compounding architectural
defects:

1. **Frame-count deferred retired-buffer release and `NoDamage` starvation.**
   `render_output_targets` deferred releasing retired client buffers via a
   4-frame `retired_defer` counter (`frame_count - since >= 4`, five frames in
   practice) instead of propagating the render targets' completion sync_files.
   That alone exhausted 2–4 buffer browser swapchains. Worse, once a starved
   client stopped committing new frames, `render_and_present` saw
   `FrameDamage::None` with `retired_buffers_pending() == true`, refused the
   `NoDamage` fast-exit, entered `render_output_targets`, produced an empty
   group render plan, and returned `PresentationOutcome::NoDamage` without
   incrementing `frame_count` or releasing retired buffers — deadlocking the
   client until unrelated external damage arrived. The union fallback also
   routed implicit-sync DRM sessions with hardware cursors into the nested
   `retired_defer` branch via an accidental
   `completion_fence.is_none() && !uses_software_cursor()` predicate.
   Furthermore, transitioning from direct client scanout back to composited
   targets did not request KMS `OUT_FENCE_PTR`, and implicit-sync retired
   DMA-BUFs did not import the compositor's read sync_file via
   `DMA_BUF_IOCTL_IMPORT_SYNC_FILE`.

2. **Uncached full-frame mirror composition and cursor-shape full redraws.**
   Mirror targets re-imported their source target's DMA-BUF into a brand-new
   `flux::Image` (`vkCreateImage`, `vkAllocateMemory`, layout transition) on
   every frame, submitted `Damage::Full` across the entire mirror resolution
   (e.g. 4K), and dropped the import immediately. In addition, software-cursor
   shape changes (`software_cursor_changed`) were classified as `base_full` in
   `assess_frame_damage`, and hardware-cursor updates under active mirror
   targets escalated to `FrameDamage::Full`, so hovering links or text in a
   browser triggered full 3K+4K repaints on every pointer shape transition.

## Decision

1. **Remove `retired_defer` completely.** Retired client buffers are released
   immediately at every presentation boundary using the exact synchronization
   primitive available for the active path:
   - **Per-output DRM targets:** collect every rendered source target's
     explicit render `acquire_fence` plus any KMS `OUT_FENCE_PTR` returned when
     reclaiming a direct-scanned client plane (`leaving_direct_client` in
     `commit_scanout`), prune already-signaled fences, merge live fences via
     `SYNC_IOC_MERGE` (`tessera_platform::sync::merge_live_sync_fences`), and
     release retired buffers immediately with the merged fence (or `None` when
     implicit sync already waited / all fences signaled).
   - **Union DRM path:** merge the union render fence and any KMS
     `OUT_FENCE_PTR` (from `present` or `present_union_to_targets`) and release
     immediately.
   - **`NoDamage` fast-exit:** retain a duplicate of the latest composited DRM
     completion fence (`last_composited_fence`). When `FrameDamage::None`
     arrives while composited (neither direct scanout nor overlay active),
     release any retired buffers immediately using the retained fence if still
     unsignaled, or `None` once signaled.
   - **Implicit-sync DMA-BUF safety:** before posting `wl_buffer.release` on a
     retired DMA-BUF without `zwp_linux_drm_syncobj_v1` explicit release,
     import the live completion sync_file as a `DMA_BUF_SYNC_READ` fence via
     `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` (best-effort; kernel >= 5.20).
   - **Nested host swapchain:** gate strictly on
     `host.presents_through_host_swapchain()` (never on `uses_software_cursor`)
     and synchronize with `device.wait_idle()` before immediate release.

2. **Cache mirror source DMA-BUF imports per target.** Each `OutputTarget` owns
   a bounded LRU `TargetImportCache` (capacity 8, 4-epoch retirement ring)
   keyed by kernel `(st_dev, st_ino)` plus image geometry, modifier, format,
   and color pipeline. Cache misses pass the producer acquire fence to
   `flux::Image::import_dmabuf_with_color_space`; cache hits attach the
   producer's live acquire fence inside the active canvas session via
   `session.wait_dmabuf_acquire`, eliminating per-frame Vulkan image creation
   and layout transitions.

3. **Localize software-cursor shape damage and mirror damage.**
   - Remove `software_cursor_changed` from `base_full`; localize cursor shape
     and visibility transitions to the swept cursor bounding rect using the
     maximum of previous and current cursor extents.
   - Project each source target's `FrameDamage` through the mirror's fitted
     content rectangle with a 2-pixel bilinear filter margin, union old/new
     mirror cursor bounds when the mirror draws the cursor, and pass the
     resulting mirror-local damage to Flux's native damage ledger
     (`begin_frame_timeout_with_damage` + `frame.repaint_scope()`).
   - Retain the most recently presented source frame descriptor
     (`RetainedSourceExport`, sharing the active KMS scanout slot's
     `Rc<flux::DmabufLease>` via `RenderTargetCommit`) only while a mirror
     consumer exists, so hardware-cursor-only mirror updates re-blit only the
     cursor rects without re-rendering the source output or holding an extra
     Flux ring slot.

## Invariants

- `[INV-ARCH-159]` Retired client buffers are released immediately at the
  presentation boundary with a fence-accurate completion signal (merged GPU
  render sync_file and/or KMS `OUT_FENCE_PTR` on DRM, `DMA_BUF_SYNC_READ`
  injection for implicit-sync DMA-BUFs, or `device.wait_idle()` on nested host
  swapchains), including on composited `NoDamage` paths; frame-count deferral
  (`retired_defer`) is prohibited.
- `[INV-ARCH-160]` Mirror target blits reuse cached source DMA-BUF Vulkan
  imports keyed by `(st_dev, st_ino)` and image descriptor, scope mirror canvas
  passes to projected source/cursor damage through Flux's native damage ledger,
  and never escalate software-cursor shape changes or mirror-only cursor motion
  to `FrameDamage::Full`.

## Consequences

- Browser and GPU-backed Wayland clients receive immediate, GPU-ordered buffer
  releases on every frame, eliminating swapchain exhaustion and `NoDamage`
  stalls across single-output, multi-output, and mirrored DRM configurations.
- Mirror targets stop allocating and destroying Vulkan images every frame and
  only rasterize projected dirty regions once their Flux ring is primed.
- Cursor hover shape changes in browsers no longer trigger full-desktop
  repaints under software cursor or mirror mode.
- Nested development mode trades an occasional `device.wait_idle()` only when
  a retired DMA-BUF is pending in exchange for deterministic, stall-free
  client swapchain pacing without frame-count heuristics.
- Non-mirrored outputs hold zero retained source descriptors or extra fds;
  mirrored source targets share the existing scanout `Rc<flux::DmabufLease>` so
  Flux's 3-slot offscreen ring depth is unchanged.
