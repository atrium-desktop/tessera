---
id: ADR-0182
title: "Fullscreen Frame Pacing and Direct-Scanout Fidelity for Uncapped Clients"
status: accepted
date: 2026-10-17
scope: runtime/presentation, runtime/damage, runtime/input, runtime/event_loop, runtime/rendering, composite/scanout, wayland/compositor, platform/drm
superseded_by: null
negative_knowledge: true
---

# 0182. Fullscreen Frame Pacing and Direct-Scanout Fidelity for Uncapped Clients

- Status: Accepted
- Date: 2026-10-17
- Deciders: Tessera Maintainers & Optics Graphics Team
- Amends: [ADR-0077](0077-presentation-domain-redraw-state-machine.md), [ADR-0101](0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md), [ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md), [ADR-0134](0134-compositor-driven-fullscreen-for-any-toplevel.md), [ADR-0179](0179-zero-redundancy-presentation-pipeline-substepped-motion-and-render-target-bucketing.md), [ADR-0181](0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md)
- Amended by: [ADR-0183](0183-geometric-output-ownership.md) — the chrome stand-down and wallpaper pause are keyed on resolved physical ownership, not the fullscreen state bit; [ADR-0184](0184-integral-automatic-output-scale.md) — the automatic scale no longer produces buffers that miss the mode extent.
- Extends: Optics [ADR-0110](https://github.com/ming2k/optics/blob/main/docs/adr/0110-frame-slot-abandonment-without-ring-advance.md)

---

## Context and Problem Statement

End-to-end profiling of a fullscreen 3D client (Minecraft) against the
reference compositors exposed a class of defects where Tessera is *materially*
slower than Sway/Hyprland/KWin for the exact workload the direct-scanout path
was built for. Every defect was independently reachable in the shipped build
and each one either starved the client's swapchain, manufactured redundant
GPU work under an opaque fullscreen surface, or rejected a scanout that should
have been eligible.

1. **Unpresented `wl_buffer` retention self-deadlocks uncapped clients.**
   `surface_commit` pushed every superseded `current_buffer` into
   `retired_buffer_releases`, and `release_retired_buffers` ran only *after the
   next KMS page flip*. A client that commits faster than vblank (mailbox
   presentation, `EGL_SWAP_INTERVAL=0`) therefore exhausted Mesa's fixed
   `color_buffers` pool inside one refresh interval and blocked in
   `get_back_bo()` waiting for a `wl_buffer.release` the compositor would not
   send until the *next* vblank. Frame rate collapsed to the refresh rate or
   half of it regardless of in-game settings.

2. **`ARGB8888` fullscreen clients were rejected as `not-fully-opaque`.**
   GLFW/Mesa allocate `DRM_FORMAT_ARGB8888`. `surface_is_fully_opaque` derived
   the logical extent from `width / buffer_scale`, ignoring `wp_viewport`. A
   client using `wp_fractional_scale_v1` + `wp_viewporter` declares its
   `opaque_region` in *destination* coordinates, so `fully_covered_by` never
   matched and every such frame fell back to compositing.

3. **Identity `wp_viewport` was an unconditional scanout veto.** The
   canonical fractional-scale shape — buffer == physical mode, `viewport_src`
   covering the whole buffer at `(0, 0)`, `viewport_dst` == complete logical
   output — is a 1:1 physical-buffer-to-plane mapping, yet `evaluate_surface`
   rejected any active viewport with `ScanoutRejectReason::Viewport`.

4. **A `wp_viewport` ARGB client was composited with the wrong UVs.** Both
   ARGB draw paths (`draw_dmabuf_toplevels_impl`, `draw_surfaces_ordered_impl`)
   routed `viewport_src` through a plain `draw_image_sub`, discarding the
   client's `opaque_region` and paying a destination readback for pixels the
   client declared opaque.

5. **Per-frame client framebuffer churn under direct scanout.**
   `import_scanout_client` called `PRIME_FD_TO_HANDLE` + `ADDFB2` on every
   frame and `RMFB` + `GEM_CLOSE` on every retirement. The compositor's own
   output path already cached framebuffers (`ScanoutOwnership::Cached`); the
   client path had no cache, so a steady-state fullscreen client paid four
   kernel ioctls per frame for identical backing objects.

6. **Occluded windows still recorded offscreen shadow masks and Gaussian blur
   passes.** `WindowShadowRenderer::prepare` consumed
   `Server::render_windows()`, which has no occlusion filter. Windows covered
   by a fullscreen surface each ran an offscreen rounded-rect pass plus a
   3-dispatch Gaussian blur *every frame*, and the results were never drawn
   (the covered windows are in `sets.occluded`). Combined with the per-frame
   wallpaper redraw under an opaque fullscreen surface (`begin_desktop_output`
   had no occlusion elision, unlike the `assess_frame_damage` wallpaper term),
   this is a continuous GPU tax on a frame that should cost the compositor
   nothing.

7. **Shell scene snapshots were pushed after the plane decision.** The
   `shell.set_windows` / `set_scale` / IPC fan-out block ran *after*
   `plan_primary_plane`, and never ran at all on a direct-scanout frame (early
   return). Entering fullscreen therefore took one composited frame before HUD
   and Dock could stand down, which is exactly the one frame where scanout
   should have been taken.

8. **A non-input wakeup escalated pointer-only motion to full damage.**
   `FrameState::merge` folded a later iteration with `had_input == false` (a
   page-flip completion, a client commit, a capture-worker event) into
   `input_pointer_only &= newer.input_pointer_only`, clearing the flag. The
   damage assessment then read `had_input && !input_pointer_only` and produced
   `FrameDamage::Full` for pointer motion that changed no compositor pixel. Its
   twin, `cursor_motion_plain`, also required `pointer_before != cursor`, which
   is false once a `zwp_pointer_constraints_v1` locked pointer clamps at an
   output edge — so locked-camera rotation re-enabled full composites.

9. **Client-bound keys escalated to full damage before the client committed.**
   Any non-pointer input took the conservative `FrameDamage::Full` branch even
   when no shell chrome was visible; the compositor re-composited the old
   client buffer into the vblank that the client's response frame needed.

## Decision Drivers

- **Extend the "zero-GPU-cost path" to actually be zero-cost**: a fullscreen
  opaque client must not pay wallpaper, shadow, blur, or per-frame ioctl work.
- **Never let compositor pacing starve a client swapchain**: buffer release
  timing is a protocol obligation, not an optimization.
- **Make direct-scanout eligibility follow *physical identity*, not the
  presence of protocol surface**: an identity viewport is not a scaling
  transform.
- **One canonical scene snapshot per frame, resolved before the plane
  decision** (extends `[INV-ARCH-65]`'s "the consumer list is load-bearing").
- **No second copy of the math**: buffer-retirement and merge-classification
  decisions get named, testable seams rather than inline boolean chains.

## Decision Outcome

### 1. Client buffer release is vblank-independent until first presentation

`SurfaceRec` gains `current_buffer_presented`. It is set `false` on every new
dma-buf commit, and `true` by `Server::acknowledge_presented_surface_damage`
(called by both the direct-scanout and composited success paths) and by
`State::note_close_transition` when a ghost retains the buffer.

The acknowledgement is **per-surface, not blanket**: the damage pass returns the
resource ids of every surface it sampled, and only those are marked. A surface
the frame never sampled (hidden workspace, fully occluded, minimized) keeps its
buffer releasable immediately. Two cases fall back to marking every surface
because the caller cannot enumerate the frame's buffers: the client damage pass
was skipped for a full-repaint frame, and a frame that drew additional preview
sources (overview thumbnails, window-switcher live cards, workspace-rail
miniatures, or a workspace-slide source page).

`retire_surface_buffer` defers to `retired_buffer_releases` only when
`current_buffer_presented` is set; otherwise it releases the `wl_buffer` (and
an explicit-sync release object) immediately. The decision is a named pure
function, `buffer_retirement(presented) -> BufferRetirement`, so the policy is
mechanically testable without a live Wayland server.

### 2. Direct-scanout eligibility recognises physical identity

`evaluate_surface` takes the `logical_size` and accepts an active viewport iff
its source covers the complete physical buffer at `(0, 0)` and its destination
equals the complete logical output; sub-buffer crops and non-output
destinations keep returning `ScanoutRejectReason::Viewport`.
`surface_is_fully_opaque` derives the logical extent from
`viewport_dst`/`viewport_src` before falling back to `width / buffer_scale`, so
a fractional-scale ARGB client's destination-space `opaque_region` is compared
in the space it was declared.

### 3. ARGB viewport blits honour `opaque_region`

The ARGB `Some(src)` branch in both `draw_dmabuf_toplevels_impl` and
`draw_surfaces_ordered_impl` routes through `split_opaque_blit`, offsetting the
per-rect source UVs by the viewport origin so opaque fragments take the
SRC-replace path and only the translucent remainder pays source-over.

### 4. Client direct-scanout framebuffers are cached

`ScanoutOwnership` gains `CachedClient(ClientFbKey)`; a bounded
(`MAX_CLIENT_SCANOUT_FBS` = 16) LRU keyed by `fstat` identity plus the full
layout descriptor reuses `framebuffer`/`gem` across swapchain rotations.
`reap_client_fb_cache` mirrors the compositor cache's `reusable`/`users`
protocol. `GEM_CLOSE` is issued only when no other cached entry (client or
compositor) still references the same handle.

### 5. Occluded windows and the occluded wallpaper are elided

`Server::unoccluded_render_windows` returns the presentation-visible set minus
`occluded_window_ids_with`; `WindowShadowRenderer::prepare` consumes it. Shadow
filter outputs are additionally reused per frame slot keyed by
`(window_w, window_h, scale, activated)`, because a shadow's pixels are
window-local and independent of translation. `begin_desktop_output` takes a
`wallpaper_occluded` flag that skips both the 2D wallpaper and the 3D model
pass when an unoccluded fullscreen surface owns the output.

### 6. Scene snapshots resolve before the plane decision

`CompositorRuntime::sync_shell_and_ipc_snapshots` (windows, global strip,
workspaces, outputs, interaction domains, DND, scale) runs at the top of
`render_and_present`, before `Shell::prepare_frame`, so a fullscreen configure
and its HUD/Dock stand-down land on the same frame. `assess_frame_damage`
remains the single place that *advances* the `last_*` revision baselines,
because it decides whether the delta forces full scene/backdrop damage, and it
now does so on the direct-scanout path too.

### 7. Pointer motion and client-bound keys do not manufacture damage

`FrameState::merge` uses a `merge_classification` helper that treats a
`had_input == false` iteration as neutral for both `input_pointer_only` and
`cursor_fast_path` (two input-bearing iterations still intersect).
`cursor_motion_plain` drops its `pointer_before != cursor` requirement (the
sprite baseline already decides whether a commit is needed). Non-pointer input
with no visible shell pixels and no keyboard capture classifies as empty
damage; a focus/stacking change still forces full damage through the window
signature check ahead of it.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-70] Vblank-Independent Buffer Release`: A superseded `wl_buffer`
  whose backing is not owned by a presented frame or a retained close-ghost
  must be released (`wl_buffer.release` / `immediate_release`) in the same
  commit that supersedes it, never deferred to the next presentation epoch. A
  buffer the presented frame *did* sample (drawn, previewed, or scanned out)
  must instead be deferred until that frame retires. Enforced by
  `buffer_retirement` + `test_inv_arch_70_unpresented_buffers_release_immediately`,
  and by the sampled-id contract of
  `Server::acknowledge_presented_surface_damage`.
- `[INV-ARCH-71] Neutral Non-Input Wakeup Merge`: Merging a presentation-wakeup
  `FrameState` (`had_input == false`) into an accumulated frame must not clear
  `input_pointer_only` or `cursor_fast_path`; only two input-bearing iterations
  may intersect. Enforced by `FrameState::merge` +
  `test_inv_arch_71_merge_preserves_pointer_only_across_a_non_input_wakeup`.
- The occlusion-elision consumer list of `[INV-ARCH-65]` is extended to cover
  the wallpaper and the compositor shadow pass: any future surface that samples
  the desktop scene under a fullscreen occluder must be added to
  `wallpaper_occluded`/`unoccluded_render_windows` or it will be elided with the
  base pass.
- `Server::acknowledge_presented_surface_damage` is the sole site that marks a
  latched dma-buf as presented outside of the close-ghost path; a new
  presentation success path that skips it would re-introduce `[INV-ARCH-70]`'s
  starvation in reverse (releasing a buffer KMS still reads).

## Rejected Alternatives & Negative Knowledge

### 1. Releasing every superseded buffer immediately
- **Why considered**: One code path; no `current_buffer_presented` state.
- **Why rejected**: A buffer that a present transaction is *currently* sampling
  (composited import) or that KMS is *currently* scanning out (direct scanout)
  must not be recycled into the client's render loop; that is a tearing/flicker
  bug, not a performance bug. The deferral is load-bearing exactly for buffers
  already owned by a frame, which is why the flag distinguishes the two cases
  rather than removing deferral entirely.

### 2. Keeping `pointer_before != cursor` in `cursor_motion_plain`
- **Why considered**: Skipping the cursor commit when the clamped position did
  not change avoids a redundant KMS call.
- **Why rejected**: Under a locked pointer the absolute position saturates at
  an output edge while the client's camera keeps rotating on the relative
  deltas; the *visible* cursor is unchanged, so the correct outcome is a
  zero-cost no-op, not a full composite. The sprite/position baseline in the
  presentation path already suppresses the redundant commit, so the guard only
  served to escalate the common locked-camera case into `FrameDamage::Full`.

### 3. Filtering occluded windows inside `WindowShadowRenderer`
- **What considered**: Have the shadow renderer call `occluded_window_ids`.
- **Why rejected**: The occlusion walk is O(windows × surfaces) and is already
  computed once per frame by `desktop_frame_sets`/`assess_frame_damage`;
  recomputing it inside the renderer would add a second copy of the walk (the
  defect ADR-0181 §Single-Pass Seam exists to prevent). The server exposes the
  resolved set instead.

### 4. Marking `current_buffer_presented` from `release_retired_buffers`
- **Why considered**: One fewer call site.
- **Why rejected**: By the time `release_retired_buffers` runs, the buffer has
  already been moved out of `current_buffer`; the flag must be latched by the
  acknowledgement of the *present transaction* that consumed it, which is
  `acknowledge_presented_surface_damage`, not by the later release sweep.

### 5. A blanket "mark every surface presented" acknowledgement
- **Why considered**: `acknowledge_presented_surface_damage` needs no
  arguments, no sampled-id plumbing, and no preview-source special case.
- **Why rejected**: It re-introduces the very starvation this ADR fixes in
  reverse. A client on a hidden workspace or behind a fullscreen occluder
  commits a dma-buf that the frame never samples; a blanket mark defers that
  buffer to the next vblank, so the client's commits are throttled by a frame
  they never appear in. That was the shipped behaviour before this ADR and the
  first implementation attempt; the sampled-id set is what actually makes the
  release timing correct, and it is cheap because the damage pass already
  enumerates exactly those surfaces.

### 6. Restricting the acknowledgement to the damage pass's sampled set alone
- **Why considered**: It is the tightest possible set for the common case.
- **Why rejected**: The overview, the window switcher, the workspace rail, and a
  workspace slide draw *additional* live client dma-bufs that the damage pass
  never enumerates (they are, by construction, windows the physical-desktop
  pass culled). Marking only the damage set would release a buffer the frame is
  actively reading — visible tearing. The `drew_preview_sources` flag routes
  those frames to the conservative fallback instead.

### 7. Generalising the shadow-output cache to a compositor-wide effect cache
- **Why considered**: The backdrop graph already has a per-slot effect cache.
- **Why rejected**: The shadow output is the *only* effect whose pixels are
  window-local and translation-invariant; folding it into the general backdrop
  cache (which keys on absolute physical regions) would make the key thrash on
  every window move, the exact failure ADR-0181 §5 documents. Scope stays
  local.

### 8. Rejecting `wp_viewport` scanout and fixing the client instead
- **Why considered**: The old veto was conservative and simple.
- **Why rejected**: The identity viewport *is* the modern fractional-scale
  contract; every HiDPI GTK4/Electron/GLFW-3.4 client uses it. Rejecting it
  penalises exactly the clients the path exists for and cannot be fixed from
  the compositor side.

### 9. Offloading a client `wl_pointer.set_cursor` surface to the KMS cursor plane
- **Why considered**: A game with a custom cursor surface currently trips
  `ScanoutRejectReason::ClientOverlay`, because `overlay_frames` composites the
  cursor surface through Vulkan. The compositor already has the pieces to place
  it on the hardware cursor plane (`DrmBackend::set_hardware_cursor` accepts any
  tightly-packed BGRA8 sprite within `cursor_extent`, and a client SHM cursor
  surface already holds exactly that in `SurfaceRec.pixels`).
- **Why deferred, not rejected**: It is a genuine follow-up but crosses the
  hardware boundary — per-CRTC placement, plane format/size capability, and
  hotspot semantics must be verified against real KMS per `docs/dev/vt-drm-testing.md`,
  which this change cannot do. The dominant starvation/fidelity defects are
  fixed without it, and a client that *hides* the cursor (GLFW
  `GLFW_CURSOR_DISABLED` sends a null surface) never trips `ClientOverlay` at
  all. A follow-up ADR should own it.

### 10. Giving the cursor-only atomic commit its own page-flip completion event
- **Why considered**: ADR-0181 made the cursor plane single-writer but the
  out-of-band/render cursor commit is a `NONBLOCK` atomic commit with no
  `PAGE_FLIP_EVENT`, so `pending_flips` stays empty; on a driver that latches a
  cursor-plane commit at vblank, the following composite flip can be rejected
  `EBUSY` and retry at 1 ms.
- **Why deferred, not rejected**: The retry is already paced and bounded
  (`[INV-ARCH-60]`), and the out-of-band path only runs while the gate is closed
  (a primary flip is already in flight, so the scheduler is not misled). Making
  the cursor commit carry a completion event reintroduces the second
  flip-tracking channel ADR-0181 §3 exists to avoid; the correct fix is
  driver-aware and needs vblank-level hardware verification.

## Follow-Up Work

- Client `wl_pointer.set_cursor` surface offload to the KMS cursor plane
  (Rejected Alternative 9), with real-hardware verification per
  `docs/dev/vt-drm-testing.md`.
- Cursor-only commit completion tracking (Rejected Alternative 10) if profiling
  shows the paced `EBUSY` retry dominating under a high-report-rate pointer.

## Consequences

### Positive
- An uncapped fullscreen client no longer blocks on compositor buffer release;
  its frame rate is bounded by its own rendering, not by vblank.
- Fractional-scale ARGB clients (Chrome, GTK4, Electron, GLFW 3.4) reach direct
  scanout and, when composited, no longer over-blend declared-opaque pixels.
- A fullscreen surface elides wallpaper, occluded-window shadows and blur, and
  per-frame client framebuffer ioctls; entering fullscreen stands chrome down
  on the same frame.
- Pointer motion and client-bound input stop producing spurious full composites
  while a client owns the whole output.

### Negative / Trade-offs
- `current_buffer_presented` is one more piece of per-surface state to keep
  coherent across the commit, acknowledge, ghost, and unmap paths; a missed
  `acknowledge` silently reverts to the starvation bug. Mitigated by the flag
  being set in exactly two server-side sites and asserted by the release
  invariant's test.
- The acknowledgement now depends on the damage pass's sampled-id set, threaded
  through `AssessedFrameDamage`. Frames that draw preview sources (overview,
  window switcher, workspace rail, workspace slide) or that skip the client
  damage pass fall back to marking every surface presented; a new draw path that
  pulls client dma-bufs outside those cases must extend
  `drew_preview_sources` or it will release a buffer the frame still reads.
- The client scanout cache holds up to 16 KMS framebuffers/GEM handles per
  session. Bounded and LRU-evicted; the alternative (uncached) is four ioctls
  per frame for the workload under test.
- Shadow-output reuse depends on `(size, scale, activated)` fully describing
  the shadow; a future per-window shadow parameter (e.g. a live blur radius
  animation) must extend `ShadowSlotKey` or the cache will serve a stale mask.
- The `wallpaper_occluded` predicate is hand-maintained like
  `[INV-ARCH-65]`'s consumer list; a new scene-sampling surface under a
  fullscreen occluder must be added to it.
