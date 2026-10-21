# Rendering and KMS Plane Reference

This page defines the observable rendering contract for direct DRM/KMS
sessions. Nested sessions always present a composited frame to the outer
compositor and do not own KMS planes.

For the design model, read
[Architecture](../explanation/architecture.md#platform-and-presentation).
For the decision history, read
[ADR-0101](../adr/0101-dual-presentation-paths-and-conservative-kms-plane-allocation.md)
and [ADR-0182](../adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md).

## Presentation Plans

| Plan | Primary-plane source | Output composition | Selection rule |
|------|----------------------|--------------------|----------------|
| Composited | Compositor-owned dma-buf | Flux renders the desktop, clients, effects, shell, overlays, and capture work | Selected whenever any visible scene element or buffer constraint requires composition |
| Direct scanout | Client-owned dma-buf | The compositor output render is skipped | Selected only when one eligible client buffer is the complete physical presentation domain |

Direct-scanout eligibility keys on physical identity, not on the presence of a
protocol object. Fullscreen and maximized window states do not select either
plan. The plan follows physical coverage and the complete visible scene. A
borderless client can scan out without an XDG fullscreen request, while a
fullscreen client is composited when another visible element is present.

All active outputs currently form one presentation domain and share one
desktop framebuffer. A direct-scanout candidate must therefore cover the
complete physical domain and be accepted by every selected primary plane.
Replacing that union domain with per-output presentation domains — which is
what lets a client own one output while another keeps compositing — is
[ADR-0185](../adr/0185-per-output-presentation-domains.md), still a draft.

## Output Ownership

Direct scanout, chrome stand-down, and wallpaper suspension share one resolved
verdict (`ADR-0183`): a visible, non-minimized toplevel *that placed its own
geometry* owns an output when its committed surface covers that output and
cannot let anything behind it show through. Maximized and tiled windows are
excluded — the compositor sized them, and chrome stays. `xdg_toplevel.set_fullscreen`
is an additional *declaration* input, so a client that asked for fullscreen owns
the presentation during the configure round-trip.

`Server::space_use()` produces the verdict once per frame; chrome receives it as
`ChromeUpdate::SpaceUse` and never infers it from the window snapshot. This is
why a game that emulates fullscreen by resizing — rather than sending
`set_fullscreen` — still reaches the direct plan.

## Direct-Scanout Eligibility

Every condition in this table must hold for the direct plan.

| Area | Required condition |
|------|--------------------|
| Scene | Exactly one client dma-buf surface is visible and it is the single toplevel surface |
| Other buffers | No SHM client surface, popup, subsurface, protocol overlay, or second dma-buf surface is visible |
| Coverage | Buffer dimensions exactly match the physical presentation domain and its origin is `(0, 0)` |
| Geometry | Buffer transform is normal. An active `wp_viewport` is allowed only as an identity mapping: its source covers the complete physical buffer at `(0, 0)` and its destination equals the complete logical output |
| Opacity | The format has no alpha channel, or the declared opaque region covers the complete logical surface. The logical surface is the viewport destination where one is set, otherwise the viewport source size, otherwise the buffer dimensions divided by the buffer scale |
| Shell | No visible shell pixels, overview, window switcher, transition, or live backdrop effect requires composition. Chrome stands down when a client owns the output (ADR-0183), so a window that fills the output does not block itself. |
| Capture | No frame capture, screenshot freeze, or Interaction Domain capture is pending |
| Session | The session is unlocked and the presentation target is active |
| Cursor | The cursor is hidden or every active output has a compatible hardware cursor plane |
| KMS format | Every active primary plane accepts the client format and modifier |
| Synchronization | An explicit acquire fence is present only when every active primary plane supports `IN_FENCE_FD` |
| Atomic commit | The kernel accepts the complete atomic plane request |

Client damage does not by itself block direct scanout. Damage regions are
forwarded to KMS through `FB_DAMAGE_CLIPS` when the selected primary planes
support the property.

### Redundant Work Under An Opaque Fullscreen Surface

When one opaque surface covers the whole presentation domain, work that cannot
reach an output pixel is elided:
- the desktop wallpaper (both the 2D background and a 3D model pass); its clock
  keeps advancing so unfullscreening resumes in step, and
- compositor window shadows, including their offscreen mask pass and Gaussian
  blur dispatches, for windows the surface fully covers.

Window shadows are only elided for the plain client scene. The overview replaces
the client scene with a thumbnail grid and draws no shadow layer, so it keeps the
complete window set.

This is the same principle as the occlusion-elision rule in
[ADR-0181](../adr/0181-canonical-chrome-prepass-seam-occlusion-elision-and-deferred-scanout-ownership.md):
an elided base pass requires a scene in which the elided content is provably
invisible.

## Composition Rejection Reasons

Direct-scanout rejection telemetry uses the following stable labels.

| Label | Meaning |
|-------|---------|
| `session-locked` | The lock scene owns the output |
| `capture-pending` | Output, screenshot, or Interaction Domain capture requires a composited frame |
| `screenshot-freeze` | A screenshot freeze session owns the visible frame |
| `surface-transition` | A client-surface transition is active |
| `overview` | Overview mode is visible |
| `window-switcher` | The live window switcher is opening, visible, or closing |
| `live-backdrop-effect` | Visible chrome samples or filters the composited backdrop |
| `visible-shell-pixels` | Shell chrome contributes pixels to the output |
| `client-overlay` | A protocol overlay or client cursor surface is visible |
| `software-cursor` | A visible cursor must be drawn into the compositor frame |
| `shm-client-surface` | A visible client surface uses SHM rather than dma-buf |
| `no-dmabuf-candidate` | No client dma-buf can supply the primary plane |
| `multiple-dmabuf-surfaces` | More than one client dma-buf surface is visible |
| `not-single-toplevel` | The only dma-buf is not the complete toplevel scene |
| `non-trivial-transform` | The candidate uses a non-normal buffer transform |
| `non-zero-origin` | The candidate does not begin at physical origin `(0, 0)` |
| `viewport` | The candidate uses a `wp_viewport` that is not an identity mapping: its source does not cover the complete physical buffer at `(0, 0)`, or its destination does not equal the complete logical output |
| `geometry-mismatch` | The candidate dimensions do not match the physical domain |
| `not-fully-opaque` | Alpha may expose content below the candidate |
| `plane-unsupported` | The primary-plane format or modifier intersection rejects the buffer |
| `kms-rejected` | KMS rejected an otherwise plausible direct atomic commit |

Plausible full-output candidates are reported at info level. Ordinary desktop
states with no candidate are reported at debug level. Reports are rate-limited
and include recent rejection counts.

## Client Buffer Release

A client `wl_buffer` is the client's memory, and its release timing is a
protocol obligation rather than a pacing knob.

- A buffer that a presented frame has **not** sampled or scanned out is released
  (`wl_buffer.release`, or `immediate_release` for an explicit-sync release) on
  the commit that supersedes it, independent of vblank. An uncapped client
  therefore renders at its own rate instead of blocking in its swapchain.
- A buffer a presented frame **did** sample is deferred until that frame retires.
  This covers both the composited path (the renderer borrowed the buffer's
  texture) and the direct-scanout path (KMS is scanning it out).
- A retained close-ghost (`ADR-0029`) owns its buffer until the ghost expires.

See `[INV-ARCH-70]` and
[ADR-0182](../adr/0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md).

## Client Framebuffer Caching

Direct scanout imports the client's dma-buf as a KMS framebuffer. Those imports
are cached in a bounded LRU keyed by the buffer's file identity plus its complete
layout (dimensions, stride, plane offset, format, and modifier), so a steady
swapchain rotation reuses one framebuffer per buffer instead of issuing a fresh
import and a matching teardown every frame. An evicted entry is destroyed only
once no in-flight commit references it.

## KMS Plane Roles

| Plane role | Allocation | Content contract | Direct-scanout effect |
|------------|------------|------------------|-----------------------|
| Primary | Exactly one per active CRTC | Either the compositor framebuffer or the direct client framebuffer in one atomic commit | Owns the selected presentation plan |
| Cursor | At most one compatible ARGB8888 linear plane per active CRTC | Compositor-owned cursor sprite; allocation maximizes coverage across all CRTCs | Allows a visible cursor without reclaiming the primary plane |
| Overlay | Discovered and counted, but not allocated | No desktop, shell, client, or effect content is offloaded | None; overlay offload remains disabled |

A missing cursor plane is not a startup failure. The cursor falls back to
software composition, which blocks direct scanout while the cursor is
visible.

Overlay availability is not proof that a layer can be offloaded. The current
policy requires a future planner to validate z-order, alpha, color, scaling,
format, modifier, damage, and synchronization in one atomic `TEST_ONLY`
request before overlay allocation becomes active.

## Primary-Plane State

Software state describes the content from the last successful presentation;
it does not describe a planned or attempted commit.

| Event | Resulting state |
|-------|-----------------|
| Startup or presentation-target invalidation | Unassigned |
| Successful compositor commit | Composited |
| Successful client direct commit | Direct scanout, owned by that surface |
| Failed direct or compositor commit | Unchanged |
| Output power loss, topology loss, or backend device-epoch loss | Unassigned |

Leaving direct scanout forces full compositor damage and invalidates cached
backdrop inputs. The software state changes to composited only after the
fallback framebuffer has been accepted.

## Backdrop Effect Recomputation and Material Fade

Backdrop effects decouple scene capture from material composition (ADR-0163):
- **Scene Refresh**: Triggered when client windows, desktop geometry, or blur sigma change. Re-captures the desktop scene and executes the Dual-Kawase pyramid blur passes.
- **Material Recompute**: Triggered on material property updates (e.g. frost opacity, wash alpha, glass tint) while the underlying scene and sigma remain valid. Reuses the cached blurred scene output without re-recording blur compute passes, sustaining high-refresh frame pacing during chrome enter/exit transitions.

## Window-Switcher Transition

The window-switcher action uses this sequence:

1. Mark the switcher as a composition requirement before planning the frame.
2. Reject direct scanout with `window-switcher`.
3. Render a full compositor frame containing the current client scene and the
   switcher.
4. Reclaim the primary plane after the compositor atomic commit succeeds.
5. Keep the existing client's fullscreen state and Wayland keyboard focus
   unchanged while previewing candidates.
6. Raise and focus the selected window when the switcher closes.
7. Re-enter direct scanout on a later frame only if the resulting scene passes
   every eligibility condition again.

Canceling the switcher retains the original focus. It does not send a
fullscreen configure round trip merely to display compositor chrome.

## Diagnostic Messages

| Log fragment | Interpretation |
|--------------|----------------|
| `plane allocation` | Reports assigned primary and cursor planes, discovered overlays, and the disabled overlay policy |
| `direct scanout active` | A client framebuffer successfully acquired the primary plane |
| `compositor reclaimed the primary plane` | A compositor framebuffer successfully replaced a direct client framebuffer |
| `direct scanout blocked` | A plausible full-output candidate failed an eligibility condition |
| `direct scanout unavailable` | The ordinary scene did not contain a plausible direct candidate |
| `direct scanout failed; compositing instead` | A non-policy backend error rejected the direct attempt and composition was selected |
| `cursor-plane ... rejected` | KMS rejected the cursor portion of the request; the next frame uses a software cursor |

Follow [How to Run tessera on Bare Metal](../how-to/bare-metal-drm.md) to verify
these transitions on real hardware.
