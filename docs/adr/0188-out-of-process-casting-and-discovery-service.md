---
id: ADR-0188
title: "Out-of-Process Casting and Discovery Service, and the Control-Center Cast Surface"
status: accepted
date: 2026-10-26
scope: shell/control_center, desktop/system, runtime, protocol
superseded_by: null
negative_knowledge: true
---

# 0188. Out-of-Process Casting and Discovery Service, and the Control-Center Cast Surface

- Status: Accepted
- Date: 2026-10-26
- Deciders: Tessera Maintainers
- Amends: [ADR-0001](0001-scope-and-responsibility-boundary.md), [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md), [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)

---

## Context and Problem Statement

[ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)
gives the compositor a session-scoped virtual extend output (`OutputKind::Virtual`)
and offscreen frame streams. That is the compositor half of wireless display
casting (Miracast / Wi-Fi Display, AirPlay, Google Cast, Sunshine/Moonlight,
WebRTC, and PipeWire `ScreenCast` sessions with `SourceType::VIRTUAL`). The
consumer half — discovery, negotiation, transport, and hardware video encoding
— does not exist anywhere in this repository:

```
$ grep -rniE "miracast|p2p|rtsp|mdns|avahi|airplay|chromecast" crates/ --include=*.rs
(no matches)
```

This is deliberate ([ADR-0001](0001-scope-and-responsibility-boundary.md)): the
compositor must not embed Wi-Fi P2P negotiation, RTSP signalling, MPEG-TS/RTP
muxing, or VA-API/NVENC encoding. But it leaves three questions unanswered:

1. Where does discovery and encoding live, and what is its boundary?
2. What exactly may the control center show and do, so it is not a dead button?
3. Miracast discovery is Wi-Fi Direct **P2P**, while Tessera already runs a
   native **iwd** `Station` client (`crates/tessera/src/wireless/iwd.rs`); the
   mainstream Linux Miracast stack (`gnome-network-displays`) instead drives
   NetworkManager's `NMDeviceWifiP2P`, whose only complete backend is
   **wpa_supplicant**. Two clients competing for one radio is a real conflict.

## Decision Drivers

- Keep discovery, signalling, muxing, and encoding out of the compositor
  ([ADR-0001](0001-scope-and-responsibility-boundary.md)).
- Let a casting daemon drive the compositor only through mechanisms the
  compositor already owns and authorises: scoped IPC / the desktop portal, and
  the virtual-output + stream primitives of [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md).
- The control center must be a surface over real state, never a placeholder that
  promises a capability the platform cannot yet honour.
- Resolve the radio-ownership conflict before any P2P work starts.

## Considered Options

1. **A separate out-of-process casting/discovery service; the compositor
   exposes cast state and the control center renders a status surface over it.**
2. **Fold discovery + capture into the compositor.**
3. **Ship a control-center "Cast" toggle now, backed by nothing.**

## Decision Outcome

Chosen option: **Option 1.**

### 1. The casting/discovery service is a separate process

Discovery, negotiation, transport, and encoding are an independent process that
reaches the compositor only through:

- **Scoped IPC / desktop portal** for virtual-output lifecycle
  (`CreateVirtualOutput` / `ResizeVirtualOutput` / `DestroyVirtualOutput`,
  [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md))
  and for the frame stream (`StreamOutputStart` targeting the virtual
  connector, dmabuf zero-copy or SHM).
- **`RemoteDesktop` / `InjectOutputInput`** for any back-channel input.

The daemon is never linked into the compositor, exactly as the portal backend
and the MCP bridge are not. It need not live in a separate repository (the MCP
bridge is an in-tree crate run as its own process); the boundary is the
process and the scope, not the checkout.

**The compositor side of the contract already exists, on identity-bound
scopes.** A caster reaches it through one of the built-in scopes, each bound to
a kernel-verified peer executable ([ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)):

- `atrium-portal` (`LOCAL_PORTAL_SCOPE`) already carries `ManageVirtualOutput`
  and `StreamOutput` — the exact pair `org.freedesktop.impl.portal.ScreenCast`
  needs to implement a virtual source. A `RemoteDesktop` back-channel is **not**
  part of that scope because the portal backend does not implement
  `RemoteDesktop` (see [portal reference](../reference/portal.md)); adding input
  to the scope before the interface exists would be a capability with no
  consumer.
- `tessera-owner-admin` carries `ManageVirtualOutput` for the `tessera` binary,
  which is what `tessera display virtual …` uses.

A bespoke caster binary gets its own identity-bound scope **when it exists**,
not before: minting a scope now would bind a security-relevant name to a
non-existent executable, which fails closed but grants nothing and cannot be
tested.

### 2. Discovery mechanism is a per-consumer choice, not a compositor concern

| Consumer | Discovery | Transport |
|----------|-----------|-----------|
| Miracast / Wi-Fi Display | Wi-Fi Direct **P2P** (WFD IE) | RTSP (M1–M7) + MPEG-TS over RTP/UDP, H.264/H.265 |
| AirPlay | mDNS/DNS-SD | RTSP/HTTP + HLS/RTP |
| Google Cast | mDNS/DNS-SD | TLS + VP8/H.264 |
| Sunshine / Moonlight | mDNS/DNS-SD + pairing | low-latency UDP/RTP |
| WebRTC / Deskreen | signalling server | RTP/WebRTC |

"Scan the LAN" is **not** one mechanism: it is mDNS/DNS-SD for the AirPlay/Cast/
Moonlight family and Wi-Fi Direct P2P for Miracast. A single "scan" button that
hides which discovery is running would be dishonest. The control-center surface
therefore enumerates **casters** (services), not "networks".

### 3. Radio ownership is a prerequisite, recorded as a blocker

Before any P2P work, one authority must own the radio's P2P role. Tessera
already runs a native **iwd** `Station` client. The options are (a) drive P2P
through iwd if and when its P2P API is complete, or (b) delegate the whole Wi-Fi
subsystem to NetworkManager / `wpa_supplicant` and drop the direct iwd
integration. Running both simultaneously is rejected: channel conflicts,
disconnects, and status races. This is a session-configuration decision, not a
compositor change, and it is recorded here so it is not rediscovered as a bug.

> Resolved by [ADR-0191](0191-casting-p2p-belongs-to-the-system-wifi-authority.md)
> and [ADR-0192](0192-first-party-miracast-caster.md): Tessera never owns the
> radio; the authority that already owns it keeps it (iwd on the reference
> configuration, with no NetworkManager); the caster is a portal client and
> needs no compositor scope; and the WFD session is a first-party
> out-of-process crate rather than a mandate to switch radio authority.

### 4. The control center is a status surface over compositor-owned cast state

The compositor owns the *facts* about casting, because only it has them:

- `SystemStatus::virtual_displays` — the live `OutputKind::Virtual` connector
  count.
- `SystemStatus::cast_streams` — live streams actively rendering a virtual
  output (the extended-display streams, as distinct from mirror/window capture).

`CompositorRuntime::publish_cast_status` publishes both whenever they change,
alongside the existing `capture_streams` indicator
([ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)).
The control center renders a **non-interactive** Cast tile from that state. It
never claims a capability the platform cannot honour: discovery, pairing, and
"start casting" stay disabled until the casting service exists and its own
surface is designed. Ending a cast or removing a virtual display becomes an
interactive affordance only in the follow-up that ships the service, so the
affordance and the action arrive together.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-80] Cast State Is Compositor-Owned`: The control center MUST render
  cast state exclusively from the compositor-published `SystemStatus` fields and
  MUST NOT infer casting from window, workspace, or network snapshots.
  Enforced by the shell reading `SystemStatus` only and verified by
  `test_inv_arch_80_cast_tile_reflects_status`.

## Rejected Alternatives & Negative Knowledge

### 2. Fold discovery and capture into the compositor
- **Why considered**: One process, no IPC contract to design.
- **Why rejected**: It embeds Wi-Fi P2P negotiation, RTSP, and video encoding in
  a GPU/input event loop, inverting [ADR-0001](0001-scope-and-responsibility-boundary.md)
  and making the compositor's failure domain include third-party display
  firmware and radio drivers.

### 3. Ship a control-center "Cast" toggle now
- **Why considered**: Visible progress; a place to land the feature.
- **Why rejected**: With no casting service, the toggle either does nothing (a
  dead button, worse than no button) or fabricates a virtual output that becomes
  the "black-hole monitor" [ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)
  rejects. A read-only status tile is honest; a control is not, until a service
  backs it.

### 4. A single generic "scan for devices" button
- **Why considered**: One entry point for every caster.
- **Why rejected**: Miracast (P2P) and the mDNS family require different radios,
  permissions, and pairing flows; collapsing them hides which mechanism is
  running and which failures are possible.

## Consequences

### Positive
- The compositor stays free of discovery, signalling, muxing, and encoding.
- Every consumer (Miracast, AirPlay, Cast, Moonlight, WebRTC) attaches to the
  same virtual-output + stream primitives and the same portal contract.
- The control center shows only truth: a read-only cast status, no promises.

### Negative / Trade-offs
- End-to-end casting is not usable until the out-of-process service exists and
  the iwd/wpa_supplicant P2P decision is made; this ADR unblocks the work but
  does not deliver a working caster.
- `SystemStatus` grows two compositor-owned counters; like `capture_streams`
  they must be published by the runtime and never cleared by the host status
  poller.
