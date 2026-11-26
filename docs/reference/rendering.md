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
| Composited | Compositor-owned dma-buf | gfx renders the desktop, clients, effects, shell, overlays, and capture work | Selected whenever any visible scene element or buffer constraint requires composition |
| Direct scanout | Client-owned dma-buf | The compositor output render is skipped | Selected only when one eligible client buffer is the complete physical presentation domain |

Direct-scanout eligibility keys on physical identity, not on the presence of a
protocol object. Fullscreen and maximized window states do not select either
plan. The plan follows physical coverage and the complete visible scene. A
borderless client can scan out without an XDG fullscreen request, while a
fullscreen client is composited when another visible element is present.

Normal DRM presentation is per output. Each physical output owns a
mode-sized compositor target with its own gfx ring, KMS primary slot, damage
scope, and page flip. An extended target renders only that output's logical
region; a mirror target reuses cached Vulkan imports of its source target's
DMA-BUF ring and blits only projected source/cursor damage with the fitted
letterbox, so the mirror CRTC never asks KMS to scale and steady-state mirror
frames avoid per-frame Vulkan image creation or full-screen blits (ADR-0246,
ADR-0250). A new target renders full for its first ring cycle, then uses
target-local damage. Absolute input devices can bind to a connector and use the
fitted content rectangle (ADR-0245). Retired client DMA-BUFs are released
immediately at the presentation boundary (and on composited `NoDamage` paths)
using the merged GPU render sync_file and/or KMS `OUT_FENCE_PTR`, with
`DMA_BUF_SYNC_READ` fence injection for implicit-sync clients (ADR-0250).

Capture, screenshot freeze, active streams, session lock, and an overlay-active
fallback use one dedicated union composition as their source. That image is
uploaded into every output target and presented through the target's own KMS
slot; it is not scanned as a union swapchain. Overview, window switcher, live
previews, soft shadows, 3D wallpaper, and workspace slides render per output.
The union path is also the fallback when a target cannot be built. Every composite image carries a lease
([Optics ADR-0168](https://github.com/ming2k/optics/blob/v0.0.87/docs/adr/0168-exported-dmabuf-image-leases.md): exported dma-buf image leases);
gfx refuses to reuse a ring slot while a target still samples or scans it.
Outputs that must share a scene generation form a presentation group (mirrors,
touching same-refresh outputs, and outputs touched by one spanning window).
A group is one atomic KMS commit; groups are independent and the loop paces at
the fastest group. Callbacks, presentation feedback, and damage acknowledgement
are scoped to the outputs that actually flipped, and feedback refresh follows
the target output's rate. Ring exhaustion is a retryable
`OPTICS_GFX_ERROR_BUSY`. A direct client flip remains independent of the composite
ring. When every physical output has its own covering opaque client and no
chrome/overlay needs composition, the runtime enters simultaneous direct
scanout on all of them, one target-scoped atomic commit per CRTC.

Virtual extend outputs (`OutputKind::Virtual`, `VIRTUAL-*`,
[ADR-0186](../adr/0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md))
are **not** part of the physical presentation domain. Their windows are
excluded from the desktop framebuffer and scanout eligibility
(`[INV-ARCH-76]`): a window living only on a virtual output never disqualifies
direct scanout for a covering client on a physical output, and a stream that
targets a virtual output renders into its own offscreen target rather than
compositing the physical desktop.

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
| Shell | No visible shell pixels, overview, window switcher, or transition requires composition. Chrome stands down when a client owns the output (ADR-0183), so a window that fills the output does not block itself. |
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
| `visible-shell-pixels` | Shell chrome contributes pixels to the output |
| `client-overlay` | A scene overlay (IME popup, drag icon, or client cursor surface) is visible, not a KMS overlay plane |
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

A `visible-shell-pixels` rejection also names the first chrome component that
claims visible pixels, for example `visible-shell-pixels (Pivot)`.

Telemetry is driven by state changes, so a steady game or video writes nothing:

| Level | When | Example |
|-------|------|---------|
| `info` | A plausible full-output candidate becomes blocked, or its blocker changes | `direct scanout blocked: visible-shell-pixels (Pivot)` |
| `info` | The candidate acquires the primary plane | `direct scanout active for 0x34325258 (mod 0x0); composite bypassed` |
| `warn` | One blocker holds a plausible candidate for 30 s (once per episode) | `direct scanout blocked for over 30s by visible-shell-pixels (Pivot) …` |
| `warn` | At startup, a transient chrome component already claims visible pixels | `chrome claims visible pixels while idle and will block direct scanout: Pivot` |
| `debug` | Every 5 s: current block, recent rejection counts, and session totals | `scanout summary: current block: none; …; totals: direct=… blocked-candidate=… desktop=…` |

Ordinary desktop states with no candidate never log above debug. To watch the
decision live, run `journalctl --user -f -g 'direct scanout'`.

Every `Chrome` component must implement `requires_composition` explicitly;
the trait has no default. The `tessera-shell` tests
`idle_default_chrome_never_blocks_direct_scanout` and
`fullscreen_client_leaves_no_shell_composition_requirement` register the
runtime's chrome set and assert that it stands down. Add a new component to
that fixture when you register it in the runtime.

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

## Output Layers, Scene Layers, and KMS Planes

These are three different concepts (ADR-0234):

- A **scene layer** is z-order, damage, and effects inside one framebuffer:
  wallpaper, client surface trees, chrome, protocol overlays, and the
  software cursor. Scene layers do not allocate KMS resources.
- An **output layer** is an independently scanned-out source with its own
  buffer and update contract: primary, cursor, or overlay.
- A **KMS plane** is the kernel object that binds an output layer to a CRTC.

Background and always-on-top are scene semantics. The wallpaper is painted as
the first scene layer into Primary; it is not a “background plane”.
Always-on-top is the window-stacking band, not an overlay plane.

| Output layer | Allocation | Content contract | Direct-scanout effect |
|--------------|------------|------------------|-----------------------|
| Primary | Exactly one per active CRTC | Either the compositor framebuffer or the direct client framebuffer in one atomic commit | Owns the selected presentation plan |
| Cursor | At most one compatible ARGB8888 linear plane per active CRTC | Compositor-owned cursor sprite; allocation maximizes coverage across all CRTCs | Allows a visible cursor without reclaiming the primary plane |
| Overlay | Discovered per CRTC; one validated candidate only when `TESSERA_OVERLAY_OFFLOAD=1` | One topmost, opaque, single-surface client window; no chrome, cursor, effects, or scene composition | Offloads that client from the composite; rejection falls back to composition |

A missing cursor plane is not a startup failure. The cursor falls back to
software composition, which blocks direct scanout while the cursor is
visible. The cursor is a cursor output layer, never a generic overlay: it
needs asynchronous position updates and hotspot semantics that overlay planes
do not provide.

Overlay availability is not proof that content can be offloaded. Overlay
allocation is disabled by default; the explicit validation path still
allocates nothing until the complete plane set — not one candidate at a time —
passes one atomic `TEST_ONLY` request. Validation covers z-order, plane type,
alpha, color, scaling, format, modifier, damage, and explicit synchronization.
A rejected candidate is composited; partial scanout is forbidden.

### Overlay Candidate Policy

The first candidate contract is deliberately narrow (ADR-0235): at most one
overlay per output, and it must be a topmost, single-surface, fully opaque
dma-buf toplevel with identity geometry, inside one output, not overlapped by
chrome or a software cursor, and free of color management, transitions,
captures, streams, overview, window-switcher, and session-lock state.

Wallpaper, chrome, menus, notifications, shadows, effects, translucent
windows, and always-on-top as a policy are never candidates. Overlay content
damage belongs to the overlay plane; geometry, stacking, or visibility changes
must repaint the old and new Primary rectangles.

Direct primary scanout takes precedence (ADR-0237): overlay is evaluated only
when Primary is composited. A direct-scanned client owns the whole output and
has no compositor framebuffer underneath an overlay, so direct primary and
overlay never coexist in one frame. The backend rejects that combination even
if the runtime regresses.

The planner always runs under debug logging as an audit: it gathers the live
candidate facts, records rejection reasons through `OverlayTelemetry`, and for
a policy-accepted candidate runs a complete-set Primary + Cursor + Overlay
`TEST_ONLY` probe. The probe imports the client buffer, validates the atomic
request, and releases the import without committing or arming a page flip.

Setting `TESSERA_OVERLAY_OFFLOAD=1` enables the explicit hardware-validation
path: the candidate window is excluded from the Primary scene, the overlay
plane is committed with Primary and Cursor in one atomic set, and the client
buffer retires with the page flip. Rejection falls back to composition. The
first path still submits Primary alongside overlay content updates; an
overlay-only commit is a follow-up optimization, not a correctness requirement. Hardware
bring-up and the full scenario matrix live in
[How to Validate KMS Overlay Offload](../how-to/kms-overlay-validation.md).

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
| `plane allocation` | Reports assigned primary and cursor planes, per-CRTC overlay candidates, and the current overlay policy |
| `direct scanout active` | A client framebuffer successfully acquired the primary plane |
| `compositor reclaimed the primary plane` | A compositor framebuffer successfully replaced a direct client framebuffer |
| `direct scanout blocked` | A plausible full-output candidate failed an eligibility condition. Logged once per change, with the blocking chrome component named for `visible-shell-pixels` |
| `direct scanout blocked for over 30s` | One blocker held a plausible candidate for 30 s. If nothing visible explains it, that component is not standing down |
| `chrome claims visible pixels while idle` | Startup self-check: the named transient chrome would block direct scanout before any interaction |
| `scanout summary` | Debug-level periodic summary: the current block, recent rejection counts, and direct / blocked-candidate / desktop frame totals |
| `direct scanout failed; compositing instead` | A non-policy backend error rejected the direct attempt and composition was selected |
| `cursor-plane ... rejected` | KMS rejected the cursor portion of the request; the next frame uses a software cursor |

Follow [How to Run tessera on Bare Metal](../how-to/bare-metal-drm.md) to verify
these transitions on real hardware.
