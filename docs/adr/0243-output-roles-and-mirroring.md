---
id: ADR-0243
title: "Output Roles: Extend or Mirror, with an Automatic Topology Policy"
status: accepted
date: 2026-11-17
scope: desktop/output, config, protocol/schema, platform/drm, wayland/server, settings, cli
superseded_by: null
negative_knowledge: true
---

# 0243. Output Roles: Extend or Mirror, with an Automatic Topology Policy

- Status: Accepted
- Date: 2026-11-17
- Deciders: Tessera Maintainers
- Amends: [ADR-0028](0028-output-and-monitor-model.md), [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)
- Amended by: [ADR-0244](0244-tentative-display-edits-revert.md) — *Tentative Display Edits Revert on a Compositor-Owned Deadline*
- Amended by: [ADR-0245](0245-absolute-input-devices-bind-to-outputs.md) — *Absolute Input Devices Bind to Outputs and Use the Letterbox Content Rectangle*
- Amended by: [ADR-0246](0246-mirror-scaling-falls-back-to-a-compositor-target.md) — *Mirrored Outputs Present Through a Compositor-Owned Mode-Sized Target*

---

## Context and Problem Statement

ADR-0028 modelled every physical output as an independent logical region and
explicitly rejected mirror/clone as the *default* multi-output mode, leaving
clone as a mode a user opts into. That mode was never implemented: the DRM
backend laid every connector out as a disjoint horizontal slice of one union
framebuffer (`desktop_width = Σ width`, `output.x = cumulative width`), the
server gave every connector its own workspace strip, and neither Settings nor
the configuration schema had a way to express "show the same desktop on both
screens".

The missing mode matters for the common living-room and presentation cases:
a laptop panel plus a smaller TV/projector should show the same desktop, while
a larger external monitor should extend it. Requiring a hand-edited TOML file
also fails the expected desktop contract that display topology is a visual
Settings operation.

## Decision Drivers

- One logical desktop: a mirror must share the source's workspace, windows,
  logical coordinates, input mapping, and capture geometry.
- Prefer scanning the shared union framebuffer's source region instead of
  duplicating the scene. When KMS cannot scale that region, a compositor-owned
  mode-sized target blit is the bounded exception (ADR-0246).
- Deterministic, shared topology resolution: the server and the DRM backend
  must never disagree about which connector is a mirror.
- Explicit user intent wins and survives restarts; automatic choice only fills
  the gap for connectors that have never been configured.
- A mirror is never a security boundary: compositor-owned consent UI and the
  primary output stay on a physical extended output.

## Considered Options

1. **A first-class output role (`Extend` or `Mirror { source }`) resolved by a
   pure shared function, with a session topology policy (`auto`, `extend`,
   `mirror`).**
2. Expand the union framebuffer with a duplicate region for the mirror and
   render the source scene twice.
3. Keep mirroring solely as an out-of-process stream (`StreamTarget::Output`)
   and require a capture/encode/decode loop even for a local HDMI cable.

## Decision Outcome

Chosen option: **Option 1**.

### Model

`OutputRole` is part of the output model, not a compositor-only flag:

```rust
pub enum OutputRole {
    Extend,
    Mirror { source: String },
}

pub enum DisplayTopology {
    Auto,    // default
    Extend,
    Mirror,
}
```

`[[output]]` gains `role = "extend" | "mirror"` and `mirror_of = "<connector>"`.
`[display] topology` selects the policy for connectors without an explicit
role; `OutputInfo.role_source` reports whether the effective role is automatic
or explicit so Settings can offer and restore **Automatic**.
`OutputInfo.role` reports the effective role on the IPC wire;
`OutputGeometry.logical_size_override` carries the source's exact logical
extent so a mirror with a different native mode still reports the same
`wl_output`/`xdg-output` geometry.

`resolve_output_roles` is a pure function in `tessera-desktop::output`. The
server and the DRM backend call the same function, so the logical model and the
scanout layout cannot drift.

### Automatic topology

`Auto` mirrors the **first additional external output that is strictly
smaller than the primary panel**, or equal-sized when the primary is an
internal panel; larger external outputs and equal-sized external pairs extend.
Virtual outputs always extend. An internal panel is preferred as the source;
an explicit `primary = true` policy wins. This models the "laptop + smaller
TV/projector" case without guessing that a large monitor or a second identical
desktop monitor should duplicate the desktop.

The heuristic is only a default: any explicit role wins, and the Settings
display page persists the resolved choice as an explicit role when the user
applies it. A connector that has been configured once therefore never changes
role because a different monitor was plugged in.

### Invariants

- `[INV-ARCH-138]` A mirror's logical rect is exactly its source's logical
  rect; it owns no workspace strip and is never the primary output.
- `[INV-ARCH-139]` A mirror source is a live physical connector with role
  `Extend`. Self-mirrors and mirror chains are invalid configuration and
  degrade to `Extend`.
- `[INV-ARCH-140]` The DRM backend excludes mirrors from the union width. A
  mirror scans its source's framebuffer rectangle and the primary plane scales
  that rectangle into an aspect-preserving destination inside the mirror's
  native CRTC (centered bars on a mismatch); the mirror's source rect is used
  for `FB_DAMAGE_CLIPS` and its hardware cursor plane is disabled.
- `[INV-ARCH-141]` A mirror follows the source's logical geometry, workspaces,
  input mapping, and capture content. Its own native mode selects the
  connector's scanout timing and the plane's destination extent; scale and
  position are derived from the source and are not user-editable.

### Backend behavior

`Output` gains `source_size` and `role`. For an extended output `source_size`
is its own mode; for a mirror it is the source output's mode size. The atomic
commit writes `SRC_W/SRC_H` from `source_size` and `CRTC_W/CRTC_H` from the
mirror's own mode, which lets KMS scale between different resolutions. Mirrors
are excluded from cursor-plane allocation (the shared framebuffer already
contains the software cursor) and from overlay offload. They also disqualify
direct primary-plane scanout and overlay bypass for the session: a bypassed
client buffer exists on only one CRTC, so the mirror would keep showing the
stale composite.

### Settings and CLI

The Settings Display page gains a **Default topology** selector and an
**Output role** selector (**Automatic**, **Extend desktop**, **Mirror another
display**) with a mirror-source selector. A mirrored output keeps its resolution selector (the connector's
native mode) but hides scale, position, primary, and arrangement controls
because those follow the source. `tessera display` marks a mirror as
`[mirror of <connector>]`.

## Rejected Alternatives

### Duplicate union-framebuffer region
- **Why considered**: no KMS scaling and no new source-rect plumbing.
- **Why rejected**: it renders and stores the scene twice, inflates every
  in-flight KMS buffer, and makes a mixed-DPI mirror inherit the primary
  render scale. It also guarantees two different pictures whenever a frame is
  half-rendered.

### Mirror as a per-output workspace
- **Why considered**: no server model change.
- **Why rejected**: a mirror with its own workspace strip is not a mirror. It
  would let a window live on the "duplicate" screen and disappear from the
  source, break pointer traversal, and make workspace switching diverge between
  the two panels.

### Stream-based local mirror
- **Why considered**: reuse the existing `StreamTarget::Output` capture path.
- **Why rejected**: a local HDMI cable must not require an encode/decode
  round-trip, a portal session, or a consumer process. Streaming remains the
  right path for *remote* sinks, not for a connector attached to the same GPU.

## Consequences

### Positive

- One explicit topology model serves config, IPC, Settings, CLI, and DRM.
- A mirror normally costs one additional KMS plane scan of an existing
  framebuffer region. A mirror target fallback costs one mode-sized target
  ring and one blit; it does not duplicate the scene.
- Different native resolutions work through plane scaling where available;
  the target blit removes that dependency when it is not.
- The automatic policy is testable in isolation and cannot disagree between
  the server and backend.

### Negative / Trade-offs

- Mirroring disables the hardware cursor on the session because the mirror has
  no cursor plane; the compositor paints the software cursor into the shared
  framebuffer so both displays show it.
- A mirror target has its own KMS slot and page flip, but it samples the union
  image; an extended output's slow union flip can still delay the source image
  that the target blits (ADR-0246/0247).
- Mixed aspect ratios are fitted with centered black bars. A bound absolute
  input device uses the fitted content rectangle (ADR-0245); an unbound device
  still addresses the union unless configured.
- `PROTOCOL_VERSION` moves to 34 and `OutputInfo`/`DisplaySettings` gain role
  state; a role omitted from `DisplaySettings` means Automatic rather than the
  old implicit extend behavior.

### Known limitations

These are explicit boundaries, not hidden fallbacks:

- A mirror of a **secondary** output still inherits the single-union
  framebuffer and primary render scale until ADR-0185 lands; chrome is rendered
  for the primary output only.
- Letterboxed mirror content insets absolute input only when the device is
  bound with `[[input.mapping]]`; unbound absolute devices keep the historical
  union-wide mapping. Relative pointer motion is unaffected (ADR-0245).
- DRM mirrors present through a compositor-owned mode-sized target blitted
  from the union image, so KMS plane scaling is not required (ADR-0246). The
  target is not a full per-output renderer; see ADR-0247.
- Display-mode changes are tentatively applied and revert after 15 seconds
  unless confirmed (ADR-0244).
