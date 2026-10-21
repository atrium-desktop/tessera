# Fullscreen And Uncapped Client Frame-Pacing Triage

Engineering know-how and diagnostic guide for attributing low frame rate in a
fullscreen or uncapped client to `tessera`, `tessera-wayland`,
`tessera-composite`, `tessera-platform`, or `optics` (see
[ADR-0077](../../adr/0077-presentation-domain-redraw-state-machine.md),
[ADR-0101](../../adr/0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md),
[ADR-0110](https://github.com/ming2k/optics/blob/main/docs/adr/0110-frame-slot-abandonment-without-ring-advance.md),
[ADR-0130](../../adr/0130-stream-paced-presentation-and-scanout-exclusion.md),
and [ADR-0182](../../adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)).

This page exists because "the compositor is slow" is the default attribution for
a game that drops to the refresh rate, and it is usually wrong in one of a small
number of ways. Each rule below states the invariant, the failure signature, and
where the fix belongs.

---

## 1. Client Buffer Lifetime (`[INV-ARCH-70]`)

A `wl_buffer` a presented frame has **not** sampled or scanned out is released on
the commit that supersedes it, independent of vblank. Only a buffer already owned
by a presentation transaction or a retained close-ghost is deferred to the next
presentation epoch.

- **Owner**: `crates/tessera-wayland/src/protocol/compositor.rs`
  (`retire_surface_buffer` → `buffer_retirement`),
  `crates/tessera-wayland/src/server/scene.rs`
  (`acknowledge_presented_surface_damage`).
- **Failure Mode**: A client that commits faster than vblank (mailbox
  presentation, `EGL_SWAP_INTERVAL=0`, `WLR_DRM_NO_MODIFIERS`) exhausts Mesa's
  fixed `color_buffers` pool inside one refresh interval and blocks in
  `get_back_bo()` waiting for a release the compositor will not send until the
  next flip. Frame rate collapses to the refresh rate, or to half of it from
  beat interference, regardless of in-game settings.
- **Correct behaviour is timing-sensitive**: releasing *every* superseded buffer
  immediately is wrong (a buffer the frame is reading or KMS is scanning out must
  not be recycled); deferring *every* buffer is the starvation bug. The decision
  is per-surface.

### Why the acknowledgement is per-surface, not blanket

`acknowledge_presented_surface_damage` marks exactly the surfaces the damage pass
sampled (`AssessedFrameDamage::sampled_surface_ids`). It falls back to marking
every surface in two cases, both of which mean the caller cannot enumerate the
frame's buffers:

- the client damage pass was skipped for a full-repaint frame, or
- the frame drew additional preview sources: overview thumbnails, the window
  switcher's live cards, the workspace rail's miniatures, or a workspace-slide
  source page (`drew_preview_sources`).

- **Failure Mode (blanket mark style)**: A client on a hidden workspace or behind
  a fullscreen occluder commits a dma-buf the frame never samples. A blanket
  acknowledgement defers that buffer to vblank, throttling a client whose commits
  never reach the output — the same starvation, mirrored.
- **Failure Mode (sampled-set-only style)**: Marking only the damage set releases
  a buffer the overview / switcher / rail is actively reading → visible tearing.

---

## 2. Direct-Scanout Eligibility Is Physical Identity, Not Protocol Presence

The direct-scanout plan follows actual output coverage. Three rejections were
historically over-conservative and had to be narrowed
([ADR-0182](../../adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)):

- **Identity `wp_viewport`** (the canonical `wp_fractional_scale_v1` +
  `wp_viewporter` shape) is a 1:1 physical-buffer-to-plane mapping and must scan
  out. Only a sub-buffer crop or a non-output destination is a `viewport`
  rejection.
- **Destination-space `opaque_region`**: an ARGB client under `wp_viewport`
  declares its opaque region in *destination* coordinates, so the opacity check
  must compare against the viewport destination extent, not `width / buffer_scale`.
- **`ARGB8888` is not opaque by format**: GLFW/Mesa allocate `ARGB8888`. A
  fullscreen client that does not set `opaque_region` is rejected
  `not-fully-opaque` and falls back to compositing.

- **Owner**: `crates/tessera-composite/src/presentation/scanout.rs`
  (`evaluate_surface`, `surface_is_fully_opaque`), which delegates extent and
  opacity to `tessera_desktop::presentation::SurfacePresentation`.
- **Failure Mode**: The path built for fullscreen games never engages for the
  clients that use it, so the compositor pays a full GPU composite every frame.

---

## 3. Zero-Cost Means Zero Work — Occlusion Elision Under A Fullscreen Surface

An opaque fullscreen surface owns the output. Work that cannot reach a pixel must
not run.

- **Wallpaper**: `begin_desktop_output` elides both the 2D background and the 3D
  model pass when `wallpaper_occluded`. The wallpaper clock keeps advancing, so
  unfullscreening resumes in step. The `assess_frame_damage` wallpaper terms
  must stay consistent with this predicate
  (`!self.space_use.client_owns_output()` — the frame's single resolved
  ownership verdict, ADR-0183).
- **Window shadows**: `Server::unoccluded_render_windows` feeds the shadow pass so
  covered windows never record an offscreen mask or a Gaussian blur dispatch.
  Shadow outputs are reused per frame slot keyed by window-local
  `(size, scale, activated)`.
- **Owners**: `crates/tessera/src/runtime/presentation.rs`
  (`begin_desktop_output`, `wallpaper_occluded`, shadow `prepare` call site),
  `crates/tessera/src/runtime/rendering.rs` (`WindowShadowRenderer::prepare`),
  `crates/tessera-wayland/src/server/input/routing.rs`
  (`unoccluded_render_windows`).
- **Failure Mode**: A steady GPU tax (a full-screen clear + wallpaper blit, or
  N windows × (mask pass + 3 blur dispatches) per frame) on a frame whose
  result is entirely hidden.

### Scope: the occlusion filter is for the plain client scene

`draw_overview_scene` replaces the client scene with a thumbnail grid and does
**not** consume the shadow layer; filtering occluded windows there would only
suppress work nothing uses. The overview therefore keeps the full
`render_windows()` set. Window-switcher cards route through `draw_client_scene`
and keep the unoccluded set. This mirrors `[INV-ARCH-65]`'s rule that the elided
base pass requires a *dead* scene.

---

## 4. Client Direct-Scanout Framebuffers Are Cached

`DrmBackend` caches KMS framebuffer imports for directly scanned-out client
dma-bufs in a bounded LRU keyed by `fstat` identity plus the full layout
descriptor (`ClientFbKey`: width, height, stride, offset, fourcc, modifier).

- **Owner**: `crates/tessera-platform/src/drm/device.rs`
  (`import_scanout_client`, `reap_client_fb_cache`),
  `crates/tessera-platform/src/drm.rs` (`ScanoutOwnership::CachedClient`).
- **Failure Mode**: Uncached, a steady-state fullscreen client pays four kernel
  ioctls per frame (`PRIME_FD_TO_HANDLE` + `ADDFB2` on import, `RMFB` +
  `GEM_CLOSE` on retirement) for identical backing objects.
- **Eviction invariant**: an entry is reapable only when it is both
  `!reusable` and `users == 0` (`composite_fb_reapable`), so an evicted entry
  still in flight is never destroyed under the atomic commit.

---

## 5. The Fullscreen Stand-Down Lands On The Same Frame

`sync_shell_and_ipc_snapshots` (windows, global strip, workspaces, outputs,
Interaction Domains) plus the idempotent `Shell::set_scale` run at the top of
`render_and_present`, **before** `Shell::prepare_frame`, `assess_frame_damage`,
and `plan_primary_plane`.

- **Owner**: `crates/tessera/src/runtime/presentation.rs`.
- **Failure Mode**: Pushing scene snapshots after the plane decision (the
  pre-ADR-0182 order) takes one composited frame before HUD and Dock can stand
  down — exactly the frame where direct scanout should have been taken, i.e. a
  one-frame flicker on every fullscreen toggle.
- **Do not move** the minimize-flight targets or the do-not-disturb publish into
  the pre-pass: they do not feed the scene snapshot, and `assess_frame_damage`
  must observe the DND transition as full damage *before* the publish consumes it.

---

## 6. Damage Classification Must Not Inflate

Two shape bugs turned pointer motion and client-bound input into full-output
composites, re-compositing the old client buffer into the vblank the client's
response frame needed.

- **Neutral non-input wakeup** (`[INV-ARCH-71]`):
  `FrameState::merge` treats an iteration with `had_input == false` (a page-flip
  completion, a client commit, a capture-worker event) as neutral for
  `input_pointer_only` and `cursor_fast_path`. Only two input-bearing iterations
  intersect. `self.input_pointer_only &= newer.input_pointer_only` cleared the
  flag and escalated to `FrameDamage::Full`.
- **Locked-pointer clamping**: `cursor_motion_plain` must not require
  `pointer_before != cursor`. Under `zwp_pointer_constraints_v1` the absolute
  position saturates at an output edge while the client keeps rotating on
  relative deltas; the guard re-enabled full composites on the common
  locked-camera case.
- **Client-bound keys**: non-pointer input routed to a client while no shell
  chrome is visible and nothing captures keyboard classifies as empty damage. A
  focus/stacking change still forces full damage through the window signature.
- **Owners**: `crates/tessera/src/runtime/input.rs` (`FrameState::merge`,
  `cursor_motion_plain`), `crates/tessera/src/runtime/damage.rs`.

---

## 7. Optics: Deferred dma-buf Transition Recycles Its Command Pool

The `FOREIGN → graphics-family` acquire transition is submitted deferred. Its
command pool is tagged with `d->graphics_family` and the command buffer is freed
on recycle, so the pool returns to the transient cache instead of being destroyed
and recreated per import.

- **Owner**: `../optics/libs/flux/src/core/dmabuf.c`
  (`dmabuf_transition_image_layout`),
  `../optics/libs/flux/src/core/oneshot.c`
  (`flux_vk_upload_pending_park_families`, `transient_pool_release`).
- **Failure Mode**: Tagging the pool `UINT32_MAX` (the removed plain
  `flux_vk_upload_pending_park` entry point) made every recycle call
  `vkDestroyCommandPool` + `vkCreateCommandPool`; leaving `pool_cmd` unset leaked
  the command buffer into a reset pool on drivers that retain allocations.

---

## 8. Client Buffers Must Stay On The Physical Grid (ADR-0184)

An output scale is an integer in the protocol. A client that does not bind
`wp_fractional_scale_v1` — GLFW, Xwayland, most games — renders every surface at
`ceil(scale)` times its logical extent. Any fractional scale therefore makes
such a client allocate a buffer larger than the output mode and guarantees
`geometry-mismatch` at the scanout gate.

- **Owner**: `crates/tessera-desktop/src/output.rs` (`automatic_scale`),
  `crates/tessera-wayland/src/protocol/output.rs` (`integer_output_scale`),
  `crates/tessera-wayland/src/server/window_manager.rs` (fractional override
  warning).
- **Failure Mode**: The client renders 1.78× (150%) or 2.56× (125%) the pixels
  of the mode, downsamples to the output, and is composited forever. The one
  symptom that identifies it immediately is the client's own reported render
  resolution exceeding the mode.
- **Do not** "fix" this by scaling a plane: primary-plane scaling is not
  universally supported, its filter quality is driver-specific, and the client
  still pays the fill-rate tax.

## 9. Presentation Ownership Is Geometric (ADR-0183)

`xdg_toplevel.set_fullscreen` is a request. GLFW's Wayland backend — what
Minecraft and most games ship — emulates fullscreen by resizing to the monitor
and never sends it. Chrome stand-down, the wallpaper pause, and therefore
direct-scanout eligibility are resolved from *physical* ownership: a visible,
opaque surface covering an output.

- **Owner**: `crates/tessera-desktop/src/presentation.rs`
  (`SurfacePresentation::owns`), `crates/tessera-desktop/src/window.rs`
  (`SpaceUse::resolve`, which restricts geometric ownership to self-placed,
  non-maximized windows so auto-hidden work areas cannot stand chrome down for
  every maximized window), `crates/tessera-wayland/src/server/input/routing.rs`
  (`Server::space_use`, the single per-frame producer),
  `crates/tessera-shell/src/lib.rs` (`Shell::set_space_use`).
- **Failure Mode**: If ownership regresses to the state bit, a borderless game
  keeps the HUD's glass and blur, the wallpaper, and the backdrop capture every
  frame, and reports `direct scanout blocked (live-backdrop-effect)` with a
  perfectly eligible buffer.
- **Diagnostic**: `tessera window` shows the window's `fullscreen` flag; if it
  is false *and* scanout is blocked by a shell/backdrop reason, ownership
  resolution is the suspect, not the client.

## 10. Multi-Output: One Union Presentation Domain (ADR-0185, draft)

The DRM backend renders one framebuffer spanning the union of all outputs and
commits every CRTC in one atomic batch.

- **Owner**: `crates/tessera-platform/src/drm/output.rs` (`DisplaySet::size`,
  horizontal layout), `crates/tessera-platform/src/drm/device.rs`
  (`commit_scanout`, `pending_flips`),
  `crates/tessera/src/runtime/presentation.rs` (`physical_size =
  surface.size()`).
- **Failure Mode (eligibility)**: With a second output attached, a covering
  client's buffer is the size of *its* mode, never the union, so
  `evaluate_surface` rejects it `geometry-mismatch` (or `non-zero-origin` on the
  second output) on every frame. Direct scanout is unreachable, not unlikely.
- **Failure Mode (pacing)**: The batch retires only when the slowest CRTC flips,
  so a 165 Hz primary beside a 60 Hz secondary presents at 60 Hz.
- **Do not** apply a partial fix (per-output plane assignment without per-plane
  framebuffer reference counting): the `current`/`retiring` pair is sound only
  because all CRTCs flip together, and a partial commit destroys that argument.

---

## Quick Symptom-to-Root-Cause Matrix

| Observed Symptom | Primary Suspect | Where to Inspect |
|------------------|-----------------|------------------|
| Fullscreen game pinned to the refresh rate (or half) with V-Sync off | A superseded `wl_buffer` deferred to vblank (`[INV-ARCH-70]`), or a blanket acknowledgement throttling a hidden/occluded client | `crates/tessera-wayland/src/protocol/compositor.rs` (`retire_surface_buffer`), `crates/tessera-wayland/src/server/scene.rs` (`acknowledge_presented_surface_damage`) |
| `direct scanout blocked` with `viewport`, `not-fully-opaque`, or `geometry-mismatch` for a fullscreen game | Identity `wp_viewport` or destination-space `opaque_region` mishandled | `crates/tessera-composite/src/presentation/scanout.rs` (`evaluate_surface`, `surface_is_fully_opaque`) |
| Fullscreen client's F3 render resolution exceeds the physical mode | `wl_output.scale` rounding (`ceil`) or a fractional automatic scale driving the logical configure (ADR-0184) | `crates/tessera-desktop/src/output.rs` (`automatic_scale`), `crates/tessera-wayland/src/protocol/output.rs` (`integer_output_scale`), `crates/tessera-wayland/src/protocol/xdg_shell.rs` (`apply_state_geometry`) |
| `direct scanout blocked (live-backdrop-effect)` while the client fills the output and reports `fullscreen = false` | Chrome stand-down keyed on the state bit instead of physical ownership (ADR-0183) | `crates/tessera-wayland/src/server/input/routing.rs` (`Server::space_use`), `crates/tessera-desktop/src/presentation.rs` (`SurfacePresentation::owns`) |
| `direct scanout blocked (geometry-mismatch)` / `(non-zero-origin)` only when a second display is connected | Union presentation domain: eligibility is measured against the union, not the candidate's output (ADR-0185, not yet implemented) | `crates/tessera-platform/src/drm/output.rs` (`DisplaySet::size`), `crates/tessera/src/runtime/presentation.rs` (`physical_size`) |
| The client renders steadily but presentation beats against a fixed vblank (visible judder at 55–90 fps) | No VRR support: the backend never programs `VRR_ENABLED` and never uses async flips, so every frame snaps to the fixed refresh interval | `crates/tessera-platform/src/drm/` (no VRR property is discovered or set; a separate decision) |
| Frame rate pinned to the slowest connected display's refresh rate | One atomic batch spans every CRTC and the domain retires only when the last flips (ADR-0185, not yet implemented) | `crates/tessera-platform/src/drm/device.rs` (`commit_scanout`, `pending_flips`) |
| Compositor GPU busy on a frame that is entirely a fullscreen client | Wallpaper or occluded-window shadow/blur not elided | `crates/tessera/src/runtime/presentation.rs` (`wallpaper_occluded`), `crates/tessera/src/runtime/rendering.rs` (`WindowShadowRenderer::prepare`) |
| Steady per-frame KMS ioctl traffic under direct scanout | Client framebuffer cache miss (identity unknown, or unbounded eviction) | `crates/tessera-platform/src/drm/device.rs` (`import_scanout_client`, `reap_client_fb_cache`) |
| One-frame flicker on entering fullscreen | Scene snapshots pushed after the plane decision | `crates/tessera/src/runtime/presentation.rs` (`sync_shell_and_ipc_snapshots` call order) |
| Random full-output composites while a game owns the screen | `[INV-ARCH-71]` merge clearing pointer-only, or `cursor_motion_plain` rejecting clamped motion | `crates/tessera/src/runtime/input.rs` (`FrameState::merge`, `cursor_motion_plain`) |
| Per-import command-pool churn in a frame capture | dma-buf transition pool tagged `UINT32_MAX` | `../optics/libs/flux/src/core/dmabuf.c` (`dmabuf_transition_image_layout`) |

## Third-Party Behaviours Tessera Must Not Work Around

- A client may commit faster than vblank by design (mailbox presentation). The
  compositor's obligations are `wl_buffer.release` timing and frame callbacks, not
  throttling the client. Do not "fix" a fast client by delaying its release.
- GLFW/Mesa allocating `ARGB8888` for a window with `GLFW_ALPHA_BITS=8` is
  correct. The compositor must treat a full-output opaque region as opaque; it
  must not require the client to switch to `XRGB8888`.

## Verification On Real Hardware

A physical DRM/KMS session is required to confirm any of the above; a nested run
cannot exercise direct scanout, page flips, or plane allocation. Follow
[How to Run tessera on Bare Metal](../../how-to/bare-metal-drm.md) and
[VT and DRM Testing](../vt-drm-testing.md), and confirm the plan transitions
through the [Rendering Reference](../../reference/rendering.md#diagnostic-messages)
diagnostic messages.
