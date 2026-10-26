---
id: ADR-0186
title: "Session-Scoped Virtual Extend Outputs, Offscreen Output Streaming, and In-Place Stream Renegotiation"
status: accepted
date: 2026-10-21
scope: desktop/output, wayland/server, composite/stream, protocol/schema, ipc/server, cli, runtime
superseded_by: null
negative_knowledge: true
---

# 0186. Session-Scoped Virtual Extend Outputs, Offscreen Output Streaming, and In-Place Stream Renegotiation

- Status: Accepted
- Date: 2026-10-21
- Deciders: Tessera Maintainers
- Amends: [ADR-0028](0028-output-and-monitor-model.md), [ADR-0052](0052-scoped-output-frame-streaming.md), [ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md), [ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md), [ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md), [ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md)

---

## Context and Problem Statement

Wireless display casting and remote desktop extension (Miracast / Wi-Fi Display,
AirPlay, Google Cast, Sunshine/Moonlight, WebRTC, and PipeWire `ScreenCast`
sessions with `SourceType::VIRTUAL`) operate in two distinct modes:

1. **Mirroring (Clone Mode)**: The remote sink displays an existing physical
   output. `StreamTarget::Output { output: Some(connector) }` ([ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md))
   already serves this by cropping or blitting from the primary presentation
   surface.
2. **Extend Mode (Secondary Wireless Display)**: The remote sink acts as an
   independent monitor in the human desktop session (`HUMAN_INTERACTION_DOMAIN`)
   with its own `wl_output` / `zxdg_output_v1` global, its own per-output
   `WorkspaceModel` strip ([ADR-0025](0025-workspace-model.md)), continuous
   pointer traversal across output borders, and independent resolution, refresh
   rate, and scale.

Attempting to support Extend Mode by naively adding a `VIRTUAL-*` connector into
the existing output and stream pipeline exposes six structural flaws:

1. **Single-Framebuffer Union Coupling Breaks Direct Scanout and Wastes VRAM**:
   The DRM and nested backends render the physical desktop into a single union
   `flux::Surface` (`desktop_width = sum(width)`), and physical output streams
   crop `self.host.presented_dmabuf()`. Expanding that union surface to include
   a `VIRTUAL-*` connector inflates all `FRAMES_IN_FLIGHT` KMS buffers, risks
   `DrmError::DesktopTooLarge` on hardware plane width limits, forces the
   virtual output to inherit the primary display's render scale and HDR color
   pipeline (`ColorPipeline`), and permanently disqualifies physical
   primary-plane direct scanout (`ScanoutRejectReason::GeometryMismatch` /
   `MultipleDmabufSurfaces`).
2. **Agent `VirtualOutput` Conflation Violates Domain Isolation**:
   `tessera_authority::interaction_domain::VirtualOutput` and
   `PresentationTarget::Virtual` ([ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md))
   isolate an `InteractionDomainKind::Agent` with a separate logical seat,
   separate window visibility, and on-demand snapshot readback. A human wireless
   extend display belongs to `HUMAN_INTERACTION_DOMAIN`: the human seat's
   pointer and keyboard must cross into it, windows must drag between `eDP-1`
   and `VIRTUAL-1`, and its authority must never be conflated with an Agent
   sandbox.
3. **Physical VBlank & Clamshell Coupling**:
   Physical output streams only produce frames inside `render_and_present` and
   force full physical composites at `max_fps` ([ADR-0130](0130-stream-paced-presentation-and-scanout-exclusion.md)).
   When the laptop lid is closed or physical panels enter DPMS power-save
   (`PresentationAvailability::OutputsOff`), `render_and_present` stops running
   and freezes any stream tied to it. Conversely, while the physical panel is on
   and static (or scanning out a fullscreen game directly), a virtual output
   needs its own independent offscreen render drive and per-output
   `wl_surface.frame` callback dispatch.
4. **Orphan "Black-Hole" Connectors vs. Session-Scoped Lifecycle**:
   In the XDG Desktop Portal `ScreenCast` workflow, a virtual display does not
   exist until a portal or streaming session requests one, and must vanish the
   moment the session or IPC connection terminates. Static configuration-only
   virtual outputs leave invisible "black-hole" monitors that trap the cursor
   and newly mapped windows when no consumer is connected.
5. **Destructive Stream Freeze on Dynamic Mode Changes**:
   Wireless sinks renegotiate resolution mid-session (RTSP M3/M4 portrait/
   landscape rotation, tablet window resize, adaptive bitrate scaling). Under
   [ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md),
   a geometry change freezes the stream with `Event::StreamGeometryChanged` and
   requires `StreamOutputStop` + `StreamOutputStart`, destroying `stream_id` and
   resetting sequence/drop telemetry.
6. **Security Modal Hijacking and Unbounded Remote Input**:
   If a virtual output is allowed to become the primary output (`index 0`),
   compositor-owned security prompts (`SecretPrompt`, `CapabilityPrompt`,
   `ConfirmPrompt`, `PickTarget`, `AppPicker`) would render on the remote sink,
   hiding consent UI from the local user and leaking sensitive prompts into the
   encoded network stream. Furthermore, remote input back-channels (Miracast
   UIBC, Moonlight, `RemoteDesktop` portal) require coordinate translation
   bounded strictly to the target output's logical rectangle.

## Decision Drivers

- Keep all protocol signaling, Wi-Fi P2P / RTSP / WebRTC transport, and
  hardware video encoding strictly out-of-process ([ADR-0001](0001-scope-and-responsibility-boundary.md)).
- Preserve zero-cost physical primary-plane direct scanout and idle power on
  physical displays even while a `VIRTUAL-*` output is actively streaming.
- Maintain strict separation between `HUMAN_INTERACTION_DOMAIN` extend outputs
  and `InteractionDomainKind::Agent` virtual outputs ([ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)).
- Guarantee automatic reclamation of virtual outputs and window relocation when
  a streaming daemon or portal connection disconnects.
- Support in-place stream geometry renegotiation without losing `stream_id` or
  frame-counter continuity.
- Enforce fail-closed security boundaries: physical primary anchoring for
  compositor modals and output-bounded remote input injection.

## Considered Options

1. **Session-scoped `OutputKind::Virtual` in the human domain, backed by
   per-stream offscreen rendering (`VirtualOutputStream`), in-place
   `StreamOutputRenegotiate`, and output-bounded remote input.**
2. **Expand the shared union desktop framebuffer to include `VIRTUAL-*`
   connectors and reuse post-present `presented_dmabuf` cropping.**
3. **Generalize `tessera_authority::interaction_domain::VirtualOutput`
   (`PresentationTarget::Virtual`) to serve both AI Agents and human extend
   displays.**

## Decision Outcome

Chosen option: **Option 1**, making a clean break from single-framebuffer output
cropping for virtual outputs while keeping the compositor boundary pure.

### 1. `OutputKind::Virtual` and `VirtualOutputSpec` (`tessera-desktop::output`)

`OutputKind` becomes a first-class, serialized property on
`tessera_desktop::output::OutputInfo` and `tessera_protocol::OutputInfo`:

```rust
pub enum OutputKind {
    Internal,
    #[default]
    External,
    Virtual,
}
```

- `OutputKind::Virtual` identifies a headless extend connector in
  `HUMAN_INTERACTION_DOMAIN`. It participates in `WorkspaceModel`, `wl_output`,
  and `zxdg_output_v1` alongside physical connectors, and never shares types or
  authority with `tessera_authority::interaction_domain::VirtualOutput`.
- `VirtualOutputSpec` validates creation and resize requests at the pure model
  boundary:
  - Connector names must match `VIRTUAL-[A-Za-z0-9_-]{1,56}` (when omitted on
    creation, the server allocates the lowest unused `VIRTUAL-1`, `VIRTUAL-2`,
    …).
  - Physical mode dimensions must lie in `64..=16_384`, `refresh_mhz` in
    `1_000..=240_000`, `scale` in `0.25..=8.0`, and `width * height <= 67_108_864`
    (bounding one RGBA frame to 256 MiB).

### 2. Physical/Virtual Partitioning and Physical Primary Anchoring (`tessera-wayland`)

`Server` stores `physical_outputs` (updated by backend hotplug via
`Server::set_outputs`) separately from `virtual_outputs` (managed via IPC). Both
feed `Server::reconcile_outputs`:

- Physical hotplugs and `[[output]]` config reloads preserve all active virtual
  outputs.
- **Physical Primary Anchoring**: Partitioning places physical outputs ahead of
  virtual outputs. Index `0` of `output_infos()` — which drives
  `output_geometry`, the primary shell HUD/Dock/Launcher, and all security
  modals — is unconditionally a physical output whenever at least one physical
  output exists.
- **Sequential Logical Origin Assignment**: Outputs without an explicit
  configured or requested `logical_origin` are laid out left-to-right using
  accumulated **logical** widths (`origin.x + logical_size.w`), eliminating
  coordinate gaps or overlaps across mixed-scale outputs (amending
  [ADR-0185](0185-per-output-presentation-domains.md) §4).
- **Per-Output Occlusion & Cross-Output Relocation**:
  - `Server::occluded_window_ids_with` evaluates fullscreen and opaque coverage
    against each window's own output rectangle rather than the primary output
    (resolving [ADR-0185](0185-per-output-presentation-domains.md) §5).
  - Moving a window across outputs via `move_to_workspace`, interactive pointer
    drag (`finish_interactive`), or output removal translates the window's
    logical coordinates into the destination output's work area and updates
    per-output `wl_surface.enter` / `wl_surface.leave` membership.

### 3. Connection-Scoped Lifecycle over Scoped IPC (`tessera-protocol`, `tessera-ipc`)

Protocol version `32` adds `ActorCapability::ManageVirtualOutput` (pregranted to
`LOCAL_PORTAL_SCOPE` and `LOCAL_OWNER_ADMIN_SCOPE`, never agent-requestable) and
three lifecycle requests:

- `Request::CreateVirtualOutput { spec: VirtualOutputSpec } -> Response::VirtualOutputCreated { output: OutputInfo }`
- `Request::ResizeVirtualOutput { connector, mode, scale, logical_origin } -> Response::VirtualOutputResized { output: OutputInfo }`
- `Request::DestroyVirtualOutput { connector } -> Response::VirtualOutputDestroyed { connector }`

By default (`spec.persistent == false`, and mandatory for `LOCAL_PORTAL_SCOPE`),
a virtual output is bound to the creating IPC connection's `conn_id`. When that
connection closes (`connection_disconnected`), the compositor destroys all
virtual outputs owned by `conn_id`, ends any streams targeting them, and
relocates their workspaces and windows back to the primary physical output.

### 4. Offscreen Virtual-Output Streaming Decoupled from Physical Presentation (`tessera-composite`, `tessera`)

When `StreamOutputStart` targets a connector with `OutputKind::Virtual`, the
stream attaches a `VirtualOutputStream` state (analogous to `WindowStream` in
[ADR-0127](0127-occlusion-safe-window-streams-and-cursor-compositing.md)):

- Each virtual-output stream renders the virtual output's visible surface tree
  (and embedded cursor when `StreamCursorMode::Embedded` is negotiated and the
  human pointer lies within the virtual output's `logical_rect`) directly into
  the stream's own `DmabufStream` slot ring or `WindowShmTarget`.
- **Zero Physical Scanout or Pacing Coupling**:
  - `OutputStreams::any_output_live`, `forcing_due_shm`, `forcing_due_dmabuf`,
    `due_shm_ids`, and `due_dmabuf_ids` filter for physical-framebuffer streams
    (`window.is_none() && virtual_output.is_none()`).
  - `Server::desktop_frame_sets` scopes the physical scene to physical outputs,
    so windows on `VIRTUAL-*` do not inflate `shm_surface_count` or
    `dmabuf_surface_count` in `plan_primary_plane`.
  - `drive_virtual_output_streams` runs per event-loop iteration independently
    of physical KMS flip state and continues running when physical displays are
    in `OutputsOff` (clamshell mode), gated only on `!session_locked() && host.is_active()`.
  - Rendering a virtual-output frame dispatches `wl_surface.frame` callbacks to
    surfaces on that virtual output via `Server::send_output_frame_callbacks`.

### 5. In-Place Stream Geometry Renegotiation (`Request::StreamOutputRenegotiate`)

When a streamed output or window changes physical pixel extent, the stream still
freezes with `Event::StreamGeometryChanged { stream_id, width, height }` so
consumers never receive unnegotiated buffer dimensions. Protocol version `32`
adds:

- `Request::StreamOutputRenegotiate { stream_id, max_fps, dmabuf } -> Response::StreamOutputStarted { ... }`

`StreamOutputRenegotiate` verifies that the caller owns `stream_id`, allocates a
fresh dmabuf slot table (or SHM readback surface) matching the target's new
physical dimensions, swaps the transport on `stream_id` in place while
preserving `sequence` and `dropped` counters, unfreezes the stream, and
transfers the new slot fds on the blob channel.

### 6. Output-Scoped Remote Input & Multi-Output Pointer Span

- Physical relative pointer accumulation spans the union bounding rectangle of
  all active human outputs (physical and virtual), allowing the cursor to move
  freely onto `VIRTUAL-*` monitors.
- `Command::InjectOutputInput { connector, actions }` validates that every
  pointer/touch coordinate lies within `0..logical_size.w × 0..logical_size.h`
  of `connector`, translates it by `connector`'s `logical_origin`, and routes it
  through `HUMAN_SEAT` while failing closed when `session_locked()`.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-74] Physical Primary Anchoring`: Whenever at least one physical
  output (`OutputKind::Internal` or `OutputKind::External`) is present,
  `Server::output_infos()[0]` MUST be a physical output, anchoring primary shell
  chrome and all compositor-owned security modals to physical hardware; a
  `Virtual` output can never preempt `primary`.
  Enforced by `Server::reconcile_outputs` partitioning and verified by
  `test_inv_arch_74_physical_output_always_outranks_virtual_primary`.
- `[INV-ARCH-75] Connection-Scoped Virtual Output Reclamation`: Every
  non-persistent virtual output created over IPC is bound to its creating
  `conn_id`; on connection teardown the compositor MUST atomically destroy those
  virtual outputs, end their streams, and relocate their workspaces and windows
  into the surviving primary output's bounds.
  Enforced by `Server::disconnect_virtual_outputs` on IPC disconnect and
  verified by
  `test_inv_arch_75_connection_disconnect_reclaims_virtual_outputs_and_relocates_windows`.
- `[INV-ARCH-76] Virtual-Output Stream & Scanout Decoupling`: A stream targeting
  an `OutputKind::Virtual` connector renders into its own offscreen target and
  MUST NOT disqualify physical primary-plane direct scanout, force physical
  composite presentations, or pollute `Server::desktop_frame_sets` with
  virtual-output-only windows.
  Enforced by `OutputStreams` physical-vs-virtual predicate separation and
  `Server::desktop_frame_sets` physical-output scoping; verified by
  `test_inv_arch_76_virtual_output_stream_preserves_physical_direct_scanout`
  (stream-side gating) and
  `test_inv_arch_76_virtual_output_does_not_evict_the_physical_scene`
  (scene-side scoping, which pins that the exclusion is targeted rather than a
  blanket suppression of every window while a virtual output exists).
- `[INV-ARCH-77] In-Place Stream Geometry Renegotiation`: A stream frozen by a
  target geometry change produces no frames until renegotiated;
  `StreamOutputRenegotiate` rebuilds the stream's backing target at the new
  physical dimensions while preserving `stream_id`, `sequence`, and `dropped`
  continuity, and refuses unowned stream ids.
  Enforced by `OutputStreams::renegotiate` and verified by
  `test_inv_arch_77_stream_renegotiate_rebuilds_target_in_place`.
- `[INV-ARCH-78] Output-Bounded Remote Input Isolation`: `InjectOutputInput`
  translates coordinates strictly within the target output's `logical_rect()`
  and MUST reject coordinates outside `0..logical_size.w × 0..logical_size.h`,
  unknown connectors, and locked sessions.
  Enforced by `Server::prepare_output_synthetic_input` and verified by
  `test_inv_arch_78_output_scoped_input_rejects_out_of_bounds_and_locked_sessions`.

## Rejected Alternatives & Negative Knowledge

### 1. Expand the primary union `flux::Surface` to cover `VIRTUAL-*` outputs
- **Why considered**: Allows `StreamTarget::Output { output: Some("VIRTUAL-1") }`
  to reuse `blit_dmabuf_stream_frames` and `crop_stream_frame` without a new
  offscreen stream driver.
- **Why rejected**: Inflates all 3 KMS swapchain slots, risks
  `DrmError::DesktopTooLarge`, forces `VIRTUAL-1` to share the primary panel's
  scale and HDR color pipeline, stalls when physical outputs are powered off
  (clamshell mode), and permanently breaks physical direct scanout (`plan_scanout`
  requires a single surface matching the union framebuffer).

### 2. Reuse `tessera_authority::interaction_domain::VirtualOutput` (`PresentationTarget::Virtual`)
- **Why considered**: `tessera-authority` already defines a `VirtualOutput`
  struct and offscreen capture for AI Agents.
- **Why rejected**: Agent virtual outputs are security-isolated sandboxes with
  separate seats and disjoint window trees (`InteractionDomainKind::Agent`).
  Human extend displays belong to `HUMAN_INTERACTION_DOMAIN` and share the human
  seat, cursor, and workspace model. Conflating them breaks the three-way
  `Physical / Virtual / Secure` authority invariant of [ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md).

### 3. Static config-only virtual outputs with pre-created PipeWire nodes
- **Why considered**: Avoids dynamic IPC lifecycle calls from the portal daemon.
- **Why rejected**: Standard `org.freedesktop.portal.ScreenCast` sessions create
  virtual displays on demand when a client requests `SourceType::VIRTUAL` and
  destroy them when the session closes. Pre-allocated static virtual outputs
  create invisible "black-hole" monitors whenever no casting client is active.

### 4. Require `StreamOutputStop` + `StreamOutputStart` on every virtual-output resize
- **Why considered**: Already implemented in [ADR-0126](0126-damage-driven-stream-pacing-and-geometry-renegotiation.md).
- **Why rejected**: Tearing down and recreating the stream mints a new
  `stream_id`, resets `sequence` and `dropped` counters, and complicates portal
  PipeWire node re-binding during routine wireless display mode negotiation or
  tablet rotation.

## Consequences

### Positive
- Wireless extend displays (Miracast, AirPlay, Sunshine/Moonlight, WebRTC,
  PipeWire `ScreenCast`) work out of the box with zero-copy dmabuf or SHM
  streams, independent scale/mode, and automatic window relocation on
  disconnect.
- Physical displays retain full direct-scanout eligibility and zero forced
  composites while virtual outputs stream concurrently.
- Virtual output streams remain live in clamshell / `OutputsOff` mode.
- Portal daemons and CLI users can dynamically create, resize, renegotiate, and
  destroy virtual outputs with strict fail-closed security guarantees.

### Negative / Trade-offs
- The stream registry now drives two offscreen target classes (`WindowStream`
  and `VirtualOutputStream`), sharing the slot-ring, readback-worker, and
  fence-polling machinery.
- Offscreen stream damage was originally sampled from the physical desktop
  accumulator. That mismatch — inverted for a virtual output, whose windows are
  excluded from the physical scene — is resolved by
  [ADR-0187](0187-offscreen-stream-damage-from-the-stream-s-own-tree.md), which
  derives window and virtual-output frame damage from the stream's own surface
  tree (`[INV-ARCH-79]`).
- **The physical KMS cursor sprite is hidden while the pointer is over a
  virtual output** (`Server::point_over_virtual_output`), because that sprite
  has no CRTC over a headless connector. The virtual output's stream draws the
  embedded cursor in its place when `StreamCursorMode::Embedded` is negotiated;
  with `Hidden` the pointer is simply invisible on that output, matching how a
  mirroring consumer already behaves.
- **`InjectOutputInput` is bounded to the target output's logical rectangle.**
  A remote back-channel (UIBC, Moonlight, WebRTC, `RemoteDesktop` portal) that
  needs to drive the *whole* desktop rather than one output must scope its
  capability to a connector that covers that rectangle, or use a whole-desktop
  streaming session. Coordinate translation is a translation, never a scale, so
  a remote client cannot reach pixels it was not granted.
