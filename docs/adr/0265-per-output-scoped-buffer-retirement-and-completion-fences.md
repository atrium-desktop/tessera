---
id: ADR-0265
title: "Per-output scoped buffer retirement and per-output retained completion fences"
status: accepted
date: 2026-11-27
scope: wayland/server, runtime/presentation, runtime/output_target, platform/sync
superseded_by: null
negative_knowledge: true
---

# 0265. Per-output scoped buffer retirement and per-output retained completion fences

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0249](0249-presentation-groups-and-per-output-pacing.md)
  (aligns retired-buffer release and retained idle fences with per-output
  independent pacing),
  [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md)
  (replaces the global retirement queue and global `last_composited_fence` with
  per-output tracking),
  [ADR-0264](0264-fail-safe-sync-fence-merging-and-subset-out-fences.md)
  (adds fail-safe wait fallback on `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` failure and
  synchronizes both KMS and Vulkan on unmerged-fence timeout)
- Amended by: [ADR-0266](0266-linux-drm-syncobj-v1-explicit-synchronization.md) — extends per-output retired buffer release to signal `wp_linux_drm_syncobj_v1` release timeline points.

---

## Context and Problem Statement

[ADR-0249](0249-presentation-groups-and-per-output-pacing.md) enabled
independent presentation pacing per output group (for example, a 120 Hz
internal panel and a 60 Hz external monitor), and
[ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md)
replaced frame-count deferral with immediate fence-backed retired-buffer
release. However, two single-output assumptions remained in the release path:

1. **Global retired-buffer queue (`State::retired_buffer_releases`).** Every
   retired DMA-BUF sat in one flat list with no record of which output(s) had
   sampled or scanned it out. When output A flipped while output B had an
   in-flight flip, releasing the global list released buffers that still
   belonged to output B's live read or scanout, using only output A's fence.
2. **Global retained idle fence (`CompositorRuntime::last_composited_fence`).**
   One slot tracked the entire desktop. Entering direct scanout on output A
   cleared `last_composited_fence`, discarding output B's retained fence before
   a subsequent `NoDamage` release.
3. **Silent fail-open on `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` and KMS-blind timeout
   escalation.** `release_retired_buffers` ignored the return code of
   `DMA_BUF_IOCTL_IMPORT_SYNC_FILE`, and unmerged fence timeouts escalated only
   to `device.wait_idle()` even when the unmerged fences came from KMS
   `OUT_FENCE_PTR`.

## Decision Drivers

- **Per-output isolation.** An output flip may only satisfy retired-buffer
  dependencies belonging to the output(s) that actually flipped.
- **Correctness for cross-output surfaces.** A buffer sampled by multiple
  outputs must accumulate each completing output's fence and release only once
  every consuming output has completed.
- **No silent fail-open on implicit-sync fence import or KMS wait timeouts.**

## Considered Options

1. **Per-output scoped retirement + `RetainedOutputFences`:** record exact
   presented output connectors per latched surface buffer, store
   `PendingReleaseOutputs` and `collected_fences` on each
   `RetiredBufferRelease`, replace `last_composited_fence` with
   `RetainedOutputFences`, fail safe on `DMA_BUF_IOCTL_IMPORT_SYNC_FILE`
   errors, and escalate unmerged timeouts through `host.wait_presented` +
   `device.wait_idle`.
2. **Keep a global retired queue and defer release until every output is idle.**

## Decision Outcome

Chosen option: **Option 1**, because waiting for all outputs to go idle
re-couples independent refresh rates and recreates the swapchain starvation
that [ADR-0249](0249-presentation-groups-and-per-output-pacing.md) and
[ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md)
were designed to eliminate.

### Concrete Design

1. **Per-surface presented-output tracking (`crates/tessera-wayland/src/lib.rs`,
   `server/scene.rs`, `protocol/compositor.rs`).**
   - `SurfaceRec` tracks `current_buffer_presented_outputs: BTreeSet<String>`
     plus `current_buffer_presented_all: bool`.
   - `acknowledge_presented_surface_damage_on_outputs` records the exact
     connector(s) that presented each acknowledged surface (populating every
     active non-mirror physical connector on unscoped or `drew_preview_sources`
     frames).
   - `retire_surface_buffer` releases immediately when neither flag nor output
     set was populated (`[INV-ARCH-70]`), or enqueues a `RetiredBufferRelease`
     carrying `pending_outputs` and `collected_fences: Vec<i32>` (with a `Drop`
     impl closing any owned duplicated fds).
2. **Scoped release and cross-output fence accumulation (`server/scene.rs`).**
   - `release_retired_buffers_on_outputs(outputs, completion_fence)` leaves
     entries whose `pending_outputs` do not intersect `outputs` untouched.
   - Intersecting entries remove the satisfied connectors; if other outputs
     still remain, they duplicate the live `completion_fence` into
     `collected_fences` and stay pending until every remaining output finishes.
   - When an entry's pending set drains to empty, all live accumulated fds and
     the final fence are merged via `SYNC_IOC_MERGE` (or waited on fail-safely
     if merging fails).
   - If `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` fails for an implicit-sync DMA-BUF
     while a live fence exists, the server logs a warning and waits on the live
     fence before posting `wl_buffer.release`.
3. **Per-output retained fences (`crates/tessera/src/runtime/state.rs`,
   `output_target_render.rs`, `presentation.rs`).**
   - `RetainedOutputFences` replaces `last_composited_fence`, storing one
     `CompletionFence` per connector (zero `dup()` on single-output commits)
     plus an unscoped fallback.
   - `release_idle_retired_buffers` queries `retired_buffer_pending_outputs()`
     and merges only the retained fences covering outputs with pending retired
     buffers.
   - `wait_unmerged_or_sync_all` escalates unmerged-fence timeouts to both
     `host.wait_presented(device)` (KMS page-flip barrier) and
     `device.wait_idle()`.

### Invariants & Behavioral Boundaries

Refines `[INV-ARCH-159]` from
[ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md):

- **`[INV-ARCH-159]` (Amended):** A presentation commit on output set `S` may
  satisfy retired-buffer ownership only for outputs in `S`; buffers still owned
  by outputs outside `S` remain queued and accumulate fences until every
  consuming output completes. Failure of `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` on a
  live fence must wait before posting `wl_buffer.release`.
  **Enforcement:** `test_inv_arch_159_per_output_buffer_retirement_isolates_sibling_outputs`
  and `test_inv_arch_159_retained_output_fences_isolate_independent_outputs`.

## Rejected Alternatives & Negative Knowledge

### Option 2: Global barrier across all outputs before releasing retired buffers
- **Why considered**: Avoids per-output sets and cross-output fence merging.
- **Why rejected**: A 60 Hz monitor would stall buffer releases for a 120 Hz
  monitor whenever both have active clients, reintroducing the exact swapchain
  exhaustion ADR-0250 fixed.

## Consequences

### Positive
- Multi-output sessions with mixed refresh rates or mixed direct-scanout /
  composited modes cannot prematurely release another output's live client
  DMA-BUFs or clobber sibling retained fences.
- Implicit-sync clients (including fallback drivers on older kernels) cannot
  race live compositor reads when `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` fails.
- KMS unmerged-fence fallback waits on KMS flips rather than only Vulkan idle.

### Negative / Trade-offs
- A surface actively spanning two outputs with different refresh rates releases
  replaced buffers at the slower output's cadence, which is the physical
  minimum required for tear-free cross-output scanout.
