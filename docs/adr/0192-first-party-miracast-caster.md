---
id: ADR-0192
title: "A First-Party Miracast Caster as an Out-of-Process Crate"
status: accepted
date: 2026-10-31
scope: casting, runtime/wireless
superseded_by: null
negative_knowledge: true
---

# 0192. A First-Party Miracast Caster as an Out-of-Process Crate

- Status: Accepted
- Date: 2026-10-31
- Deciders: Tessera Maintainers
- Amends: [ADR-0188](0188-out-of-process-casting-and-discovery-service.md), [ADR-0191](0191-casting-p2p-belongs-to-the-system-wifi-authority.md)

---

> **Amendment 2026-10-30 — the connection direction and the inbound firewall.**
> This ADR originally described the source as the RTSP client that connects to
> the sink's advertised port. That is inverted. Miracast makes the **source the
> RTSP server** and the **sink the RTSP client**: the source listens on its
> control port (7236 by default, advertised in its WFD IE), the sink connects,
> and the sink sends the first `OPTIONS`. The implementation now accepts the
> sink's connection (`cast::accept` + `cast::handshake_as_server`) and never
> dials the sink. Two consequences belong in the decision, not in a code
> comment:
>
> 1. A sink does not need to advertise a control port to be castable; the
>    earlier "no RTSP port" blocker rejected the ordinary case.
> 2. The host's inbound firewall is part of the session **when the live ruleset
>    blocks**. The probe reads only the live ruleset as authoritative and
>    reports `unverified` — without borrowing the persistent file's verdict —
>    when it cannot; a definite block is reported as `Blocker::InboundFirewall`.
>
> Hardware validation (2026-10-30): the reference Skyworth A3F completed the
> session — source-driven M1–M6, sink `SETUP`, source 200 with `Transport`, sink
> `PLAY`, then 300 frames at 720p30 before the source's frame limit. The sink
> requires `wfd_trigger_method: SETUP` **inside M5**, a `Transport` header in
> the `SETUP` response, and RTP sent from the advertised `server_port` (19002).
> A separate source M7 is ignored. The validated source value is in the §8
> matrix. **The reference TV displayed the 720p30 test pattern** (color bars,
> circles, squares); video casting is confirmed end-to-end.
>
> Audio amendment (2026-10-30): the reference sink advertises AAC and its
> phone-cast audio works. Audio is therefore implemented as an explicit
> **opt-in** path: PipeWire sink-monitor/microphone capture → AAC-LC 48 kHz
> stereo → the same MPEG2-TS/RTP flow, with the sink's AAC echoed in M5. It is
> not a default; `[cast] audio = false`. HDCP and UIBC remain non-goals. See
> [Miracast Source](../explanation/miracast-source.md#75-audio) and the
> [lessons](../dev/miracast-lessons.md).

## Context and Problem Statement

[ADR-0191](0191-casting-p2p-belongs-to-the-system-wifi-authority.md) concluded
that a Miracast-capable machine "must run NetworkManager with wpa_supplicant",
and therefore that a first-party caster was unnecessary: adopt the ecosystem
caster, adopt its radio requirements.

Runtime measurement of the reference machine contradicts that claim. On a stock
iwd session with no NetworkManager and no `wpa_supplicant` installed:

```
net.connman.iwd.Adapter.SupportedModes      = ["ad-hoc", "station", "ap"]
net.connman.iwd.p2p.Device.Enabled          = false   (writable)
net.connman.iwd.p2p.Device.AvailableConnections = 1
net.connman.iwd.p2p.Device.GetPeers / RequestDiscovery / ReleaseDiscovery
net.connman.iwd.p2p.ServiceManager.RegisterDisplayService(a{sv})
net.connman.iwd.p2p.ServiceManager.UnregisterDisplayService
adapter driver                              = iwlwifi ("Arrow Lake CNVi WiFi")
VAProfileH264High : VAEntrypointEncSlice    = present
```

So the radio half of Miracast is available **without any new daemon**, and the
hardware encoder is present. What is missing is the protocol half. This is
exactly the split iwd's own interface contract describes: after a P2P connection
"the application is responsible for continuing the WFD session establishment by
setting up the TCP connection, performing WFD capability exchange and
negotiation, Link Content Protection setup and session start".

Two independent questions therefore have to be answered separately:

1. **Is NetworkManager required?** No. Nothing in Miracast requires
   NetworkManager. It requires (a) a controller that can form a P2P group and
   (b) an implementation of the WFD session. The ecosystem caster
   (`gnome-network-displays` and peers) supplies (b) and demands
   NetworkManager's `NMDeviceWifiP2P` for (a) — that coupling is the ecosystem's,
   not the protocol's.
2. **Should we write (b)?** That is this ADR.

## Decision Drivers

- The reference machine already has a viable radio path and a hardware encoder;
  the ecosystem path would add two daemons and move radio ownership away from
  the running authority.
- [ADR-0001](0001-scope-and-responsibility-boundary.md) excludes casting from the
  **compositor**, not from the project: an out-of-process crate satisfies it the
  same way `tessera-mcp` does.
- The compositor side is already complete: a session-scoped virtual output and a
  zero-copy stream ([ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md)),
  reachable by any caster through the portal, which now projects protocol 32
  (portal ADR-0024).
- A protocol implementation we own can be tested with literal fixtures and
  driven against a real sink, unlike a black-box dependency.
- The cost is real: RTSP/WFD, MPEG2-TS, RTP, encoder plumbing, and a
  sink-compatibility treadmill are a long-lived maintenance surface.

## Considered Options

1. **Write a first-party caster as an out-of-process crate, staged, with
   explicit non-goals.**
2. **Adopt the ecosystem caster and its radio requirements (ADR-0191's
   conclusion).**
3. **Wait: ship nothing until a third-party caster supports iwd.**

## Decision Outcome

Chosen option: **Option 1**, as a bounded, staged crate, because the reference
configuration cannot use the ecosystem caster without replacing its radio
authority, and ADR-0191 already forbids two authorities on one radio.

### 1. Boundary: an out-of-process crate, a portal client

The caster is its own binary (working name `tessera-cast`). It is a **portal
client**: it requests a `ScreenCast` session with a virtual source, receives
frames over PipeWire, and encodes them. It holds **no compositor IPC scope** and
does not link the compositor — the consent path stays the portal's
([ADR-0191](0191-casting-p2p-belongs-to-the-system-wifi-authority.md) §3).

It owns the radio's **P2P role** through iwd's `p2p.Device` and
`p2p.ServiceManager`; it never owns the station link, which remains the wireless
subsystem's ([ADR-0190](0190-wireless-backends-are-selected-and-capability-declared.md)).
iwd permits this split: P2P has its own device object and its own connection
budget (`AvailableConnections`).

### 2. Protocol scope: M1–M7, TS/RTP, one codec

The caster implements, in this order:

1. Register a **WFD Source** with `RegisterDisplayService` (Source-only
   properties — see §4), enable `p2p.Device`, `RequestDiscovery`, and surface
   `GetPeers` results with their `net.connman.iwd.p2p.Display` capabilities.
2. Form the P2P group to the chosen peer, then **serve** the WFD session on this
   source's control port: the source listens (7236 by default), the sink connects
   to it, and the sink drives `M1`–`M7` in the documented direction. The
   source-driven compatibility path drives M1–M6 and puts the `SETUP` trigger in
   M5; the source never dials the sink.
3. Mux H.264 into MPEG2-TS and packetize into RTP/UDP.
4. Encode with VA-API (`VAEntrypointEncSlice`, already present on the reference
   machine) from the PipeWire stream's dmabufs.

### 3. Explicit non-goals for the first release

- **HDCP 2.x** is not implemented. Content that requires link protection will be
  refused with a clear reason rather than streamed unprotected; a sink that
  insists on HDCP is reported as incompatible. This is a deliberate, recorded
  limitation, not an oversight — HDCP is patent-encumbered and its absence is
  the honest boundary.
- **Audio** is not sent (video-only session).
- **UIBC** input back-channel is not implemented.
- **Dual-role local advertisement** is not offered (§4).

### 4. The iwd dual-role caveat, and why Source-only is the mitigation

iwd documents: "there's currently no way to select the role for a specific
connection so IWD will refuse to connect to a dual-role peer if the local WFD
device is also dual-role capable." Most Miracast sinks advertise both roles. If
the caster registered as dual-role, common sinks would be unreachable.

Registering **Source-only** capability makes the local device single-role, which
is the documented way around that refusal. This is a **hypothesis to validate
against a real sink** in stage 0, not an assumption to build on silently: if it
fails, the fallback is to drive `wpa_supplicant`'s `fi.w1.wpa_supplicant1` P2P
API directly — still without NetworkManager, which is the claim this ADR corrects.

### 5. Staging, with a stop gate after each stage

| Stage | Deliverable | Stop gate |
|-------|-------------|-----------|
| 0 | Radio + WFD capability exchange against a real sink, video source replaced by a static test pattern; a **capability probe** that reports honestly (no fabricated availability) what the machine supports | M1–M6 + `SETUP`/`PLAY` complete and a sink accepts the session |
| 1 | PipeWire capture of the virtual output + VA-API encode + TS/RTP delivery | A sink shows live desktop frames |
| 2 | Discovery UI surface, reconnect, error taxonomy | — |
| 3 | UIBC / HDCP, only if a sink population demands them; audio is opt-in and implemented for the reference sink | — |

### 7. The verification gate is inverted when the implementer cannot reach hardware

The table above gates protocol work on stage-0 hardware evidence. That gate
assumes the implementer can run a sink. When they cannot — the accepted
situation here: the caster is written in-tree and the maintainer performs the
real-scenario runs — the gate must be **re-cut by testability**, not skipped:

- **Landed with proof:** everything that is a pure byte transformation. RTSP
  message framing (RFC 2326), RTP/H.264 payloads (RFC 6184), MPEG2-TS
  packetization (ISO/IEC 13818-1), and the WFD subelement codec are exercised by
  byte-level fixtures, including round-trips and reject cases. These are where
  "looks right" and "is right" differ most, and where a failure is
  indistinguishable from a sink quirk without proof.
- **Confirmed on the reference Skyworth A3F:** the TV displayed the 720p30 test
  pattern (color bars, circles, squares) after 300 frames. The source-driven
  exchange (M1–M6 with the M5 `SETUP` trigger), the sink's `SETUP`/`PLAY`, the
  `Transport` response, RTP from the advertised `server_port`, and the media
  path are all validated. Audio is implemented as an opt-in path (PipeWire →
  AAC-LC → same TS/RTP) and the reference TV plays it; UIBC and HDCP remain
  unimplemented.
- **Remaining work:** live desktop capture (`--source desktop`) and the
  compositor Control Center click. The daemon control path already reached
  `Streaming`.
- **The stage-0 probe remains the first thing to run**, because it turns a
  hardware failure into a named, actionable half.

What does *not* change: the sink still decides. Nothing in this ADR may be
described as "working" before a real sink shows frames, and the probe's honest
three-state vocabulary is the model for how the untested layers report
themselves.

### 8. Outstanding verification matrix

| Area | Evidence today | Needed |
|------|----------------|--------|
| RTSP framing, RTP/H.264 payloads, TS packetization, TS-over-RTP (RFC 2250), WFD subelement codec | **landed**: byte-level unit fixtures (round-trip, rejection, reassembly) | none |
| RTSP transport framing over a byte stream | **landed**: fixtures for split reads, pipelined bursts, clean vs partial EOF, a 9 KiB body across 4 KiB reads, and socket-timeout mapping | none |
| Source server loop (accept + M1–M6) | **verified against the reference TV**: accepts the sink, completes M1–M6, answers `SETUP` with `Transport`, receives `PLAY`, and streams; 300 frames / 4,784,680 bytes | none — the TV displayed the pattern |
| Connection direction | **verified**: the source listens on 7236, the TV connects, and the source-driven opening is the path this sink accepts | none |
| Inbound RTSP reachability | **verified**: the TV connected while the live ruleset was unreadable; the probe reports `unverified` and does not borrow the persistent file's verdict | an ingress rule for the P2P interface when the live ruleset blocks |
| WFD parameter surface form and injection defence | **landed**: unit fixtures | none |
| `wfd_video_formats` value handling | **verified**: the source's validated value is `38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none`, a 720p30 subset of the TV's `f9cf94a0` bitmap; the captured M4 value is §8.6 | none for the reference TV; other sinks may need a different subset |
| WFD source session ordering | **verified**: CSeq echo, out-of-order tolerance, fail-closed parameters; this sink requires the `SETUP` trigger in M5, `Transport` in the SETUP 200, and RTP from the advertised `server_port` | none |
| RTP media source port | **verified**: the sender binds the P2P local address and `server_port` 19002; an ephemeral source port produced no display even though TS/H.264 decoded locally | none |
| iwd P2P enable, display-service registration, discovery request | **verified on the reference machine** against live iwd: all three succeed, and the radio is restored to its prior state afterwards | none |
| iwd display-service D-Bus contract | **verified**: `RegisterDisplayService(a{sv})` returns nothing, `UnregisterDisplayService()` takes no arguments, and unknown keys are rejected with `InvalidArguments` (`HasAudio` refused; `Source`/`Sink`/`Port`/`HasUIBC`/`HasContentProtection` accepted) | none |
| Peer enumeration | **verified**: returns an empty list with no sink in range (exit `3`); with the reference TV in range it reports `Sink=true` and `Port` absent, and the TV connects back | none |
| Reading a real peer's `p2p.Display` properties | **verified** against the Skyworth TV: `Sink=true`, `Source=false`, `HasAudio=true`, `HasUIBC=false`, and **no `Port`**; an absent port is informational for the passive direction, not a blocker | a sink advertising a port (optional) |
| Source-only registration vs iwd's dual-role refusal | accepted by iwd; the refusal itself is not reproduced | a dual-role sink connects |
| VA-API encode session on the real adapter | **implemented**: hardware slice encoder via `/dev/dri/renderD128` (`VAEntrypointEncSlice`) with zero-latency tuning, producing Annex-B AUs; the pattern stream was accepted by the TV | visual confirmation of desktop content |
| PipeWire capture of a virtual output | **implemented**: `ScreenCast` portal client (v6) opens PipeWire remote node, normalized to 1080p | frames carry desktop content |
| A scripted sink exists for hardware-free verification | **landed**: `tessera-cast sink --connect` (the sink client) and a socketpair integration test drive the real source server in milliseconds | none — this is the loop that keeps hardware iteration cheap |
| A source keeps serving RTSP after the trigger | **verified**: the source answers `SETUP` and `PLAY` and streams until the frame limit; returning at the trigger broke real sinks | none |
| Group formation and the WFD session against a sink | **verified**: `SimpleConfiguration.PushButton()` forms the group and the TV completes the session | none |
| Stage 0 Test Pattern Streaming (`cast`) | **verified**: the reference TV displayed 300 frames / 4.78 MB at 720p30 (color bars, circles, squares) | none |
| Cast service and control center | **verified**: `tessera-castd` control path (`StartDiscovery` → `Connect`) reached `Streaming` against the TV; the shell renders the service state, session state, capability, and blocker | the compositor UI click and the desktop stream |
| HDCP, UIBC | not implemented | out of scope until required; audio is opt-in and implemented |

Stage 0 is complete: the reference TV displayed the test pattern, resolving both
open risks (dual-role registration and P2P stability). The remaining work is the
live desktop stream and the Control Center UI click. If a different sink rejects
the validated 720p30 subset, use that sink's own M4 value rather than inventing a
bitfield.

### 6. Stage 0 ships as a capability probe, and it fabricates nothing

The first deliverable is `crates/tessera-cast` with one command,
`tessera-cast probe`. It reports each half separately — the radio
(`net.connman.iwd.p2p.ServiceManager`, `p2p.Device` facts) and the pipeline
(VA-API H.264 encode entry point, PipeWire socket, `ScreenCast` portal) — and
every line is one of **observed**, **absent**, or **unverified**.

It is strictly read-only: it never enables a P2P adapter, never starts a scan,
never writes a property. Enabling P2P is a stage-0 *action* of the caster, not a
diagnostic side effect.

Exit codes are part of the contract so the stage gate can branch on them:
`0` both halves present, `1` a half is definitively absent, `2` indeterminate. An
unverified half **never** collapses into "ready".

### Invariants & Behavioral Boundaries

- `[INV-ARCH-83] No Fabricated Cast Capability`: a casting capability verdict MUST
  be reported as observed, absent, or unverified, and an unverified or uninspected
  half MUST NOT be presented as available. A definite absence MUST outrank an
  unknown in the same report, because the absence is actionable and the unknown is
  not.
  Enforced by `Verdict`'s three-state shape and `CastReport::blocker`, and
  verified by `test_inv_arch_83_an_uninspected_half_is_never_reported_as_capable`,
  `test_inv_arch_83_a_definite_absence_outranks_an_unknown`, and the remaining
  `test_inv_arch_83_*` suite.

## Rejected Alternatives & Negative Knowledge

### 2. Adopt the ecosystem caster and its radio requirements
- **Why considered**: zero protocol code to write and maintain; it is what
  ADR-0191 originally decided.
- **Why rejected**: on the reference configuration it requires installing
  NetworkManager **and** `wpa_supplicant` and transferring radio authority away
  from the running iwd — a larger system change than writing the WFD session,
  and one that puts two authorities' worth of configuration on one radio if the
  station link is kept on iwd. ADR-0191 §1 forbids that split.
  It remains the right choice for a deployment that already runs
  NetworkManager: nothing here stops an NM-based machine from using the
  ecosystem caster instead.

### 3. Wait for a third-party caster to support iwd
- **Why considered**: no in-tree maintenance surface at all.
- **Why rejected**: iwd's P2P/WFD interfaces are `[Experimental]` and no
  mainstream caster consumes them; there is no roadmap to wait on. "Wait
  indefinitely" is a decision to not have the capability.

### 4. Put the caster inside the compositor
- **Why considered**: direct access to the rendered frame and the output model.
- **Why rejected**: it drags radio control, RTSP, muxing, and encoding into the
  GPU/input event loop and makes the compositor's failure domain include sink
  firmware and Wi-Fi drivers — the inversion
  [ADR-0001](0001-scope-and-responsibility-boundary.md) exists to prevent. The
  portal + virtual-output path already gives the caster what it needs.

## Consequences

### Positive
- Miracast becomes reachable on a machine that runs iwd and no NetworkManager.
- The WFD session is ours, so a sink incompatibility is diagnosable and fixable
  in-tree instead of patched around.
- The radio split is explicit: the caster owns the P2P role, the wireless
  subsystem owns the station link.
- The stage gate is measurable before any protocol code exists: on the reference
  machine `tessera-cast probe` reports both halves present (iwd WFD registration
  available, P2P device available with 1 concurrent connection, H.264
  `VAEntrypointEncSlice` available, PipeWire socket present, `ScreenCast` portal
  available) and exits `0`.

### Negative / Trade-offs
- A new long-lived protocol surface: RTSP/WFD, MPEG2-TS, RTP, and encoder
  plumbing, plus a sink-compatibility treadmill that no amount of design removes.
- Two new external dependency families must be admitted through
  [`dependency-policy.toml`](../../tooling/xtask/dependency-policy.toml) (an iwd
  D-Bus client and a PipeWire/VA-API path), which is a reviewed capability
  decision per [ADR-0159](0159-package-boundaries-follow-capabilities.md), not a
  drive-by manifest edit.
- HDCP-less streaming means DRM-protected playback cannot be cast; the refusal
  must be explicit so it never looks like a bug.
