---
id: ADR-0268
title: "Per-output KMS overlay offload and zero-GPU overlay-only fast-path commits"
status: accepted
date: 2026-11-27
scope: composite/presentation, platform/drm, runtime/presentation, runtime/output_target
superseded_by: null
negative_knowledge: true
---

# 0268. Per-output KMS overlay offload and zero-GPU overlay-only fast-path commits

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0235](0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md)
  (implements the follow-up overlay-only commit fast path and extends overlay
  commits to per-output subset presentation groups),
  [ADR-0247](0247-per-output-render-targets.md)
  (removes the mutual exclusion between per-output render targets and KMS
  overlay offload)

---

## Context and Problem Statement

[ADR-0235](0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md)
and [ADR-0236](0236-overlay-placement-is-a-pure-state-machine.md) established a
sound KMS overlay-offload planner and placement state machine, but two
structural limitations kept windowed hardware overlay offload unreachable on
ordinary DRM desktops:

1. **Per-output render targets excluded overlay offload.**
   [ADR-0247](0247-per-output-render-targets.md) replaced the union swapchain
   with per-output mode-sized render targets for normal DRM frames, yet
   `presentation.rs` gated `wanted_overlay` on
   `!self.output_targets_cover_all_physical()`, `can_render_output_targets`
   rejected active overlays, and `DrmBackend::commit_scanout` required
   `targets.len() == self.displays.outputs.len()` whenever an overlay was
   present. On every ordinary DRM session, `TESSERA_OVERLAY_OFFLOAD=1` was
   therefore silently bypassed. Furthermore, `DrmBackend::probe_overlay`
   programmed Primary `SRC_X`/`SRC_Y` from `(output.x, output.y)` instead of
   the slot's active `PrimaryScanoutGeometry`, causing `TEST_ONLY` failures on
   secondary extended outputs using mode-sized targets.
2. **No overlay-only commit fast path.** Even when an offloaded windowed video
   or high-frame-rate client updated only its own buffer in place while the
   Primary desktop behind it remained completely static, damage assessment
   folded the overlay surface's damage into Primary, forcing a Vulkan frame
   acquire and Primary presentation every frame — defeating the bandwidth,
   latency, and power-efficiency benefits of hardware overlay offload.

## Decision Drivers

- **Windowed zero-GPU presentation.** Steady-state in-place buffer updates of
  an offloaded overlay window must bypass Vulkan frame acquisition, GPU
  composition, and Primary plane re-scanout completely.
- **Native per-output integration.** Overlay offload must work directly on the
  per-output target pipeline without falling back to the legacy union
  swapchain.
- **Preservation of every ADR-0234–0237 safety invariant.** Complete-set
  `TEST_ONLY` preflight (`[INV-ARCH-125]`), strict `TESSERA_OVERLAY_OFFLOAD`
  policy gating (`[INV-ARCH-128]`), mutual exclusion with direct primary
  scanout (`[INV-ARCH-134]`), and Primary repaint on activation, move, or
  deactivation (`[INV-ARCH-132]`) remain inviolate.

## Considered Options

1. **Integrate KMS overlay offload into per-output targets and add a
   zero-GPU `present_overlay_only` fast path** backed by split Primary vs.
   overlay client damage assessment and per-slot `PrimaryScanoutGeometry`.
2. **Keep overlay offload restricted to the legacy union swapchain** and keep
   re-submitting Primary alongside every overlay buffer swap.

## Decision Outcome

Chosen option: **Option 1**, because the per-output target pipeline is the
production DRM presentation path, and eliminating Primary GPU work on
steady-state overlay buffer swaps is the primary reason hardware overlay planes
exist.

### Concrete Design

1. **Primary vs. overlay damage split (`crates/tessera-composite/src/presentation/damage.rs`,
   `crates/tessera/src/runtime/damage.rs`).**
   - `ClientDamageTracker::assess_with_overlay_split` advances every visible
     surface's baseline generation in lockstep while separating in-place buffer
     commits (`previous.placement == sample.placement && previous.generation !=
     sample.generation`) of the active overlay surface from Primary damage.
   - `AssessedFrameDamage` exposes `primary_output` and
     `overlay_buffer_changed`. Placement changes (move/resize/appear/disappear)
     still dirty both Primary and total damage (`[INV-ARCH-132]`).
2. **Subset-aware overlay commits and `present_overlay_only`
   (`crates/tessera-platform/src/drm/device.rs`, `drm.rs`, `host.rs`).**
   - `PlaneSlot` records `geometry: PrimaryScanoutGeometry` so `probe_overlay`
     validates `TEST_ONLY` against the exact active primary framebuffer
     geometry (including `ModeSizedTarget` on secondary extended outputs).
   - `overlay_commit_allowed` permits attaching an overlay in a subset
     `commit_scanout` whenever `source == Composited`, the overlay's output is
     in `targets`, and no unrelated output outside `targets` holds an active
     overlay. Rotating `current_overlay`/`retiring_overlay` happens only when
     `overlay_transition` touches the committed CRTC(s).
   - `DrmBackend::present_render_targets_with_overlay` /
     `Host::present_render_targets_with_overlay` commit per-output render
     targets and an output's overlay plane atomically in one `TEST_ONLY`-proven
     KMS request.
   - `DrmBackend::present_overlay_only` / `Host::present_overlay_only` perform
     an atomic overlay-plane (+ hardware cursor) swap without touching Primary
     or Vulkan, verifying steady-state placement equality (`output`, `plane`,
     `target`, `zpos`), running `TEST_ONLY`, requesting `OUT_FENCE_PTR` (with
     `wait_for_flip` fallback), and returning `CompletionFence`.
3. **Runtime orchestration (`crates/tessera/src/runtime/presentation.rs`,
   `output_target_render.rs`).**
   - Remove the `!output_targets_cover_all_physical()` and
     `overlay_state.active().is_some()` exclusions so overlays run natively on
     per-output targets (mirrors still disqualify overlay offload because a
     mirror blits its source target's DMA-BUF).
   - When `overlay_change == OverlayChange::Updated`, `primary_output == None`,
     and `overlay_buffer_changed == true` (and no capture/stream/freeze/lock is
     active), take `present_overlay_only` directly — zero Vulkan frame acquire,
     zero draw calls, scoped per-output buffer retirement via ADR-0265.
   - Inside `render_output_targets`, pass `excluded_overlay_window` to
     `draw_direct_desktop_scene`, commit `present_render_targets_with_overlay`
     when Primary renders, and run `present_overlay_only` for an undamaged
     overlay group when a sibling group rendered. Any `ScanoutUnsupported`
     rejection deactivates the overlay and falls back to full composition
     without evicting output targets.

### Invariants & Behavioral Boundaries

Extends `[INV-ARCH-131]` from
[ADR-0236](0236-overlay-placement-is-a-pure-state-machine.md):

- **`[INV-ARCH-131]` (Enforcement extended):** An in-place buffer commit on an
  active KMS overlay surface produces `ClientDamage::None` on Primary and
  routes through `present_overlay_only` without acquiring a Vulkan frame or
  reprogramming the Primary plane.
  **Enforcement:** `test_inv_arch_131_overlay_in_place_buffer_commit_leaves_primary_damage_none`
  and `overlay_commit_allows_subset_target_only_for_owned_output`.

## Rejected Alternatives & Negative Knowledge

### Option 2: Keep overlay offload on the union swapchain only
- **Why considered**: Avoids extending `present_render_targets` and adding
  `present_overlay_only`.
- **Why rejected**: Leaves windowed hardware overlay offload dead on every
  normal per-output DRM session and wastes 3D GPU cycles re-compositing an
  unchanged Primary desktop behind every video or game frame.

## Consequences

### Positive
- `TESSERA_OVERLAY_OFFLOAD=1` now works natively on per-output DRM sessions
  (single- and multi-output extended desktops).
- Steady-state windowed overlay buffer updates incur zero Vulkan rendering and
  zero Primary framebuffer writes.
- Fixes `probe_overlay` geometry on secondary extended outputs with
  mode-sized render targets.

### Negative / Trade-offs
- Mirrors continue to disable overlay offload because mirror targets blit the
  source target's composited DMA-BUF.
- Multi-plane subsurface video offload and underlay hole-punching remain
  separate future milestones building on top of this per-output overlay seam.
