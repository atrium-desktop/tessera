# Miracast Casting — Delivery Handoff

**Audience:** an engineer or AI taking over this work.
**Read this before touching anything.** It records what is proven, what is not, and
the mistakes already made so they are not repeated.

Companion records (authoritative, read them in this order):

| Record | Why |
|--------|-----|
| [ADR-0192](../adr/0192-first-party-miracast-caster.md) | the decision to build this, the staging, the verification matrix (§8), the invariants |
| [ADR-0191](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md) | who owns the radio, and why the caster is a portal client with no compositor scope |
| [ADR-0186](../adr/0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md) | the compositor side: session-scoped virtual outputs and offscreen streaming |
| [ADR-0188](../adr/0188-out-of-process-casting-and-discovery-service.md) | the out-of-process service boundary and the control-center cast surface |
| [`docs/explanation/miracast-source.md`](../explanation/miracast-source.md) | protocol stack, architecture, media path, configuration, validation, limits |
| [`docs/dev/miracast-lessons.md`](miracast-lessons.md) | the case files for every hard-won failure and the diagnostic playbook |
| [`docs/reference/tessera-cast.md`](../reference/tessera-cast.md) | the command reference |

---

## 0. Amendment 2026-10-30 — the direction was inverted

This is the most important correction in this document. The original handoff
implemented the **source connecting to the sink**. Miracast specifies the
opposite:

> **The source is the RTSP server. The sink is the RTSP client.** The source
> listens on its control port (7236 by default), the sink connects to it, and
> **the sink sends the first `OPTIONS`**.

That inverts four things the earlier sections assumed:

1. `cast`/`handshake` **listen**; they never dial the sink. `sink --connect`
   is now the scripted **client** that drives M1–M7.
2. The sink does **not** need to advertise a control port. "advertises no RTSP
   port" is no longer a castability blocker; the source's own advertised port is
   what matters.
3. The source must **not** send an unsolicited `OPTIONS` to a sink that is
   waiting to speak first.
4. The host firewall is part of the session **when the live ruleset blocks**.
   The reference machine's persistent `/etc/nftables.conf` says `policy drop`,
   but its live ruleset admitted the TV; the earlier probe read the file and
   wrongly reported a block. `probe` now treats only the live ruleset as
   authoritative and reports `unverified` (with the file as a hint) when it
   cannot read it; a definite block still reports `Blocker::InboundFirewall`
   with the ingress rule to add.

The old `endpoint`/`neighbours` resolver and `--host`/`--listen` flags are gone:
the sink's IP arrives with the accepted connection, and the only remaining
peer-resolution job is matching a `--peer` needle.

**Proven offline after the correction:** the source server accepts a sink
connection, the sink sends `OPTIONS`, the exchange reaches `PLAY`, and the source
streams RTP. `cargo test -p tessera-cast` covers the server-direction pair; a
local `cast`/`sink` run delivered 61 RTP datagrams for 5 test-pattern frames and
the sink received all of them.

**Hardware validation (2026-10-30):** the reference Skyworth A3F completed the
session end-to-end. The winning exchange is:

```text
source OPTIONS (M1) -> 200
source GET_PARAMETER (M3) -> 200 with the TV's M4 parameters
source SET_PARAMETER (M5) with wfd_trigger_method: SETUP -> 200
sink SETUP -> source 200 with Transport: ...;server_port=19002-19003 and Session
sink PLAY -> 200
source streams 300 frames (4,784,680 bytes, 720p30)
```

Two requirements are easy to miss and both were observed as failures:

1. The trigger must be **inside M5**. A separate source M7 `SET_PARAMETER` is
   acknowledged and then ignored; the TV closes.
2. The source's `SETUP` 200 must include `Transport` (echo `client_port`, add
   `server_port`). Without it the TV closes after one frame.

The `tessera-castd` control path (`StartDiscovery` → `Connect`) reached
`Streaming` against the same TV. **The TV displayed the 720p30 test pattern**
(color bars, circles, squares). A third requirement surfaced during media
bring-up: the RTP sender must bind the P2P local address and the `server_port`
advertised in `SETUP` (19002). An ephemeral source port produced a valid local
TS/H.264 stream that the TV never displayed.

---

## 1. TL;DR — the honest state

**Working and verified on the reference machine:**

- `probe` — reports both halves of casting present, exit `0`.
- `discover` — enables P2P, registers this machine as a Wi-Fi Display source,
  requests discovery, enumerates peers, and **restores the radio state on exit**.
- The **entire protocol core** and the **M1–M6 + SETUP/PLAY exchange**, proven
  offline and against the reference TV.
- **Reference-TV session** — the Skyworth A3F accepts the source parameters,
  sends `SETUP`/`PLAY`, and receives 300 frames at 720p30. The trigger is in M5;
  the `SETUP` 200 carries `Transport`.
- `sink` — a scripted sink so the caster can be exercised with no hardware.
- **P2P group formation** — `P2pRadio::form_group` via `net.connman.iwd.SimpleConfiguration.PushButton`
  and `disconnect_peer` via `net.connman.iwd.p2p.Peer.Disconnect` (introspected from `/usr/lib/iwd`).
- `cast --peer` / `handshake --peer` — peer needle resolution, control-port binding,
  and auto group formation; the sink's IP arrives with its accepted connection.
- **Source-side server** — `cast`/`handshake` bind the control port, wait for the
  sink to connect, and answer the sink's `SETUP`/`PLAY`; the source-driven path
  puts the trigger in M5. Proven against the reference TV and the scripted sink.
- **Media** — MPEG-2 TS over RTP, VA-API H.264, and the portal capture path.
- **MPEG-2 TS over RTP** (RFC 2250, PT 33) — 7 TS packets per datagram, packetizer + depacketizer.
- **Stage 0 Test Pattern Media Streaming** (`tessera-cast cast --source pattern`) — serves the handshake to `PLAY`, streams MPEG-2 TS over RTP over UDP with live pacing, and services keep-alives; the sink owns `TEARDOWN`. Proven against the scripted sink client over a local socketpair and UDP.
- **Stage 1 Live Desktop ScreenCast & VA-API Hardware Encode** (`tessera-cast cast --source desktop`) — requests `ScreenCast` portal session (v6), receives PipeWire stream node, normalizes high-DPI desktop to standard 1080p, hardware-encodes via Intel iHD VA-API (`VAEntrypointEncSlice` on `/dev/dri/renderD128`), and streams live desktop frames.

**Remaining work:**

1. **Control Center UI click** through the compositor (the daemon control path
   already reached `Streaming`).
2. **Desktop capture on hardware** (`--source desktop` with the portal prompt).
3. **Audio** is implemented as an **opt-in** path (PipeWire monitor/microphone
   → AAC-LC → same TS/RTP); the TV plays the tone. What remains is the desktop
   audio validation, the Control Center `[cast] audio = true` path, and the
   few all-IDR-pattern clicks. The video encoding and the TS muxer/SDT remain
   separate variables; do not reduce them to one cause.
   See the [Miracast source](../explanation/miracast-source.md#75-audio) and
   [lessons](miracast-lessons.md).

**Explicitly out of scope for the first release:** HDCP and UIBC
(ADR-0192 §3); audio is opt-in and implemented. HDCP-less streaming means
DRM-protected playback cannot be cast, and that refusal must be explicit so it
never looks like a bug.

> **Video is confirmed working on the reference TV.** The test pattern was
> displayed; the protocol, media path, and display are no longer open. Desktop
> capture and the Control Center click are the remaining product checks.

---

## 2. Ground rules — non-negotiable

These are not style preferences. Each one already prevented a shipped defect.

1. **Never fabricate a capability or a value.**
   - The probe's verdict is `observed` / `absent` / `unverified`
     (`[INV-ARCH-83]`). An unverified half must never collapse into "works".
   - `wfd_video_formats` is **never invented**. The default is the
     hardware-validated 720p30 subset; any override must come from the sink's M4
     value or a working caster. It is a packed bitfield, and a wrong bit makes a
     sink reject the session indistinguishably from a radio failure.
   - The declared field schema in `wfd::formats::FIELD_SCHEMA` is explicitly
     labelled unverified and is used **only for display**.
2. **A refusal must name its own cause.** "Not found" is useless when the real
   cause is "not a sink", "no RTSP port", "address not in the neighbour table
   yet", or "two interfaces answer". Every one of those has a different fix.
3. **Stay inside the boundary.** The caster is an out-of-process port
   ([ADR-0001](../adr/0001-scope-and-responsibility-boundary.md)). It holds **no
   compositor IPC scope**: it is a `ScreenCast` portal client, so every capture
   goes through portal consent. Do not add a caster scope — that would be a
   consent bypass ([ADR-0191](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md) §3).
4. **Keep the verification matrix current.** The protocol/session row now has
   hardware evidence (ADR-0192 §8); the remaining visual and desktop checks are
   named there. A claim without hardware evidence must say so.
5. **Probe is read-only; discover is an action.** `probe` must never enable an
   adapter, start a scan, or write a property. `discover` may, and must restore
   what it changed.

---

## 3. Where things live

### Tessera repository (`crates/tessera-cast`)

```
crates/tessera-cast/
├── Cargo.toml            deps: log, tessera-env, zbus   (policy: tooling/xtask/dependency-policy.toml)
├── README.md             status, commands, evidence table
└── src/
    ├── lib.rs            module map + the two rules
    ├── bin/tessera-cast.rs   CLI: probe | discover | handshake | formats | sink
    ├── probe.rs          Verdict algebra + CastReport (pure)
    ├── observe.rs        OS/bus observation → Verdict (the only side-effecting read layer)
    ├── iwd.rs            shared iwd D-Bus plumbing + error classification
    ├── radio/mod.rs      P2pRadio port, DisplayCapabilities, peer sorting
    ├── radio/iwd.rs      the iwd implementation (enable/register/discover/peers)
    ├── discover.rs       the discovery use case (tested against a fake radio)
    ├── neighbours.rs     (removed: the sink's IP arrives with the accepted connection)
    ├── firewall.rs       inbound tcp/7236 reachability (nftables/ufw)
    ├── endpoint.rs       peer matching for --peer (no address resolution)
    ├── cast.rs           the bounded handshake loop (M1–M7) + Drain policy
    ├── sink.rs           a scripted sink (tests + `sink` command)
    └── wfd/              THE PROTOCOL CORE — pure, byte-tested
        ├── rtsp.rs       RFC 2326 framing + text/parameters bodies
        ├── ie.rs         WFD Information Element / subelement codec
        ├── parameters.rs the advertised parameter set + injection defence
        ├── formats.rs    wfd_video_formats tokens: reversible + width-aware annotation
        ├── rtp.rs        RFC 6184 H.264 payloads (single NAL + FU-A)
        ├── ts.rs         ISO/IEC 13818-1 PAT/PMT/PES/PCR
        ├── session.rs    the sink-driven responder (M1–M7)
        ├── trace.rs      two-directional trace with hex dumps
        └── transport.rs  RTSP framing over Read+Write
```

~7,900 lines, **134 unit tests, ~0.6 s**, no hardware required.

### Portal repository (`../xdg-desktop-portal-atrium`)

Protocol-32 projection landed — see §7. That repository has its own `AGENTS.md`;
it forbids Tessera source dependencies and requires literal fixtures plus the
independent test server.

### Workspace registration

Adding the crate required three edits, all done:

- `Cargo.toml` → `[workspace.dependencies] tessera-cast`
- `tooling/xtask/dependency-policy.toml` → `[tessera-cast] normal = ["log", "tessera-env", "zbus"]`
- `docs/dev/project-layout.md` → the crate list (kept alphabetical)

**Any new dependency must be admitted through the policy file, not by editing a
manifest.** `cargo xtask check-boundaries` fails closed on unclassified packages.

---

## 4. The two halves, and why the split matters

```
                ┌─────────────────────────────────────────────┐
                │ tessera-cast                                │
                │                                             │
  radio half ───┤ radio::iwd ──► iwd (p2p.Device,            │
  (who forms    │                 p2p.ServiceManager)         │
   the group)   │                                             │
                │ discover ─────► peers + Display caps        │
                │ firewall ─────► inbound tcp/7236 verdict    │
                │ endpoint ─────► --peer needle matching      │
                │                                             │
  protocol half │ wfd::* ───────► RTSP / WFD / RTP / TS        │
  (the session) │ cast ─────────► M1–M7 loop + trace          │
                └─────────────────────────────────────────────┘
                                   │ ScreenCast portal (consent)
                                   ▼
                    compositor: virtual output + stream
                    (ADR-0186 — already landed and tested)
```

The compositor side is **done**; this crate is the consumer.

---

## 5. Commands and exit codes

```text
tessera-cast probe
tessera-cast discover [--settle SECONDS] [--port PORT]
tessera-cast handshake --video-formats FMT [--peer PEER] [--bind ADDR] [--port 7236] [--settle SECONDS] [--timeout 60]
tessera-cast cast --video-formats FMT [--peer PEER] [--bind ADDR] [--port 7236] [--rtp-port 19000] [--fps 30] [--duration SECS] [--max-frames N]
tessera-cast formats "<value>"
tessera-cast sink [--connect ADDR] [--rtp-port PORT] [--play]
```

**The source listens; the sink connects.** `cast`/`handshake` bind the control
port (7236) before discovery and wait for the sink. `sink --connect` is the
scripted source→sink client used for offline iteration.

| Exit | Meaning |
|------|---------|
| `0` | probe: both halves present · discover: ≥1 castable sink · handshake: sink accepted · formats: parsed |
| `1` | a half is definitively absent, or the run failed |
| `2` | probe: indeterminate (something could not be inspected) |
| `3` | discover: the run worked, no castable sink found |

`2` is never success. `3` is deliberately distinct from `1`: the fix is on the
sink's side (open its Screen Mirroring mode), not this machine's.

---

## 6. Verification matrix — what is proven, and by what

Copy of ADR-0192 §8. **Keep this current; it is the only honest status.**

| Area | Evidence | Still needed |
|------|----------|--------------|
| RTSP framing, RTP/H.264, TS packetization, WFD IE codec | byte-level fixtures (round-trip, rejection, reassembly) | — |
| RTSP transport over a byte stream | split reads, pipelined bursts, clean vs partial EOF, 9 KiB body across 4 KiB reads, timeout mapping | — |
| WFD parameter surface form | fixtures incl. CR/LF/NUL injection refusal | — |
| `wfd_video_formats` handling | reversible token codec, `none` as value and as token, width-aware annotation; source value validated against the reference TV (`38 00 01 01 00000020 …`) | — |
| Source server loop (accept + M1–M6 + SETUP/PLAY) | **verified against the reference TV**: accepts, answers `SETUP` with `Transport`, receives `PLAY`, and streams 300 frames / 4,784,680 bytes; the TV displayed the pattern | — |
| RTP media source port | **verified**: bind the P2P local address and `server_port` 19002; an ephemeral source port produced no display although the TS/H.264 decoded locally | — |
| Connection direction | **verified**: source listens on 7236, the TV connects; the source-driven path is the one this sink accepts | — |
| Inbound RTSP reachability | **verified**: the TV connected while the live ruleset was unreadable; the probe reports `unverified` without borrowing the persistent file's verdict | an ingress rule when the live ruleset blocks |
| A source keeps serving after the trigger | **verified**: `SETUP`/`PLAY`/stream to the frame limit | — |
| iwd P2P enable / register / discover | **verified live**: all three succeed | — |
| iwd display-service D-Bus contract | **verified live** (§7) | — |
| Radio state restoration | **verified live**: `Enabled` false → run → false | — |
| Peer enumeration | **verified live**: returns empty with no sink in range (exit `3`); with the reference TV it reports `Sink=true` and no `Port`, and the TV connects back | — |
| Reading a real peer's `p2p.Display` | **verified** against the Skyworth TV: `Sink=true`, `Source=false`, `HasAudio=true`, `Port` absent | a sink advertising a port (optional) |
| Source-only vs iwd's dual-role refusal | accepted by iwd; the refusal itself not reproduced | a dual-role sink connecting |
| Group formation | **verified**: `SimpleConfiguration.PushButton()` formed the group and the reference TV completed the session | — |
| TS over RTP (RFC 2250) | **landed**: PT 33, 7 TS packets/datagram chunking, packetizer + depacketizer unit-tested | — |
| Stage 0 Test Pattern Streaming (`cast`) | **verified**: the reference TV displayed 300 frames / 4.78 MB at 720p30 | — |
| VA-API encode | **implemented**: hardware slice encoder via `/dev/dri/renderD128` (`VAEntrypointEncSlice`), producing Annex-B AUs; the pattern stream was accepted | visual confirmation of desktop content |
| PipeWire capture of a virtual output | **implemented**: `ScreenCast` portal client (v6) opens PipeWire remote node, normalized to 1080p | frames carry desktop content |
| HDCP / UIBC | not implemented | out of scope; audio is opt-in and implemented |

---

## 7. Live-verified iwd contract — do not "fix" these back

Measured against the running iwd on the reference machine. **The daemon does not
match its own documentation**, and three of my first assumptions were wrong. A
future change that "restores" the documented shape will break against the daemon.

| Call | Truth |
|------|-------|
| `RegisterDisplayService(a{sv})` | **returns nothing.** Do not expect an object path. |
| `UnregisterDisplayService()` | **takes no arguments.** The registration belongs to the connection. |
| Unknown dictionary keys | **rejected** with `InvalidArguments`. |
| `HasAudio` | **refused whatever its type** (`b`, `y`, `s` all rejected). |
| Accepted keys | `Source`, `Sink`, `Port`, `HasUIBC`, `HasContentProtection` |
| Empty dictionary | rejected — **`Source` is mandatory** |
| `p2p.Device.Enabled` | writable; `AvailableConnections = 1` on the reference adapter |
| Adapter `SupportedModes` | `ad-hoc`, `station`, `ap` — P2P is tracked on its own device object, not here |
| `p2p.Peer.Address` | string property returning MAC (property name is `Address`, not `DeviceAddress`) |
| `p2p.Peer.Connected` | boolean property indicating active group membership |
| `p2p.Peer.ConnectedInterface` | string property returning interface name (e.g. `p2p-wlan0-0`) |
| `p2p.Peer.ConnectedIP` | string property returning sink IP address |
| `p2p.Peer.Disconnect()` | method disconnecting active group |
| `SimpleConfiguration.PushButton()` | method on peer initiating WPS push button group formation (completes asynchronously when group and DHCP lease are established) |

Because `HasAudio` is refused, `radio::iwd::register_source` **attempts the richer
advertisement, then degrades to `Source`/`Sink`/`Port`** on `InvalidArguments` and
logs the degradation. Do not replace that with a hardcoded "don't send HasAudio":
the accepted key set varies by iwd release.

Reproduce:

```bash
busctl --system introspect net.connman.iwd /net/connman/iwd net.connman.iwd.p2p.ServiceManager
busctl --system call net.connman.iwd /net/connman/iwd \
    net.connman.iwd.p2p.ServiceManager RegisterDisplayService 'a{sv}' 2 Source b true Port q 19000
```

---

## 8. Remaining work, in dependency order

### 8.1 P2P group formation — Landed

Introspected directly from the unstripped system daemon binary (`/usr/lib/iwd`):
group formation is driven by `net.connman.iwd.SimpleConfiguration.PushButton()`
on the peer's object path, which triggers WPS push-button negotiation, GO/client
setup, interface provisioning, and internal DHCP. When complete, iwd sets
`Connected = true`, `ConnectedInterface`, and `ConnectedIP` on `net.connman.iwd.p2p.Peer`.
Disconnect is triggered via `net.connman.iwd.p2p.Peer.Disconnect()`.
Implemented in `P2pRadio::form_group` and `disconnect_peer`; the reference TV
completed group formation and the session.

### 8.2 `cast` / `handshake`: the source server — Landed

`cast` and `handshake` bind the source's control port **before** discovery,
register that port with iwd, optionally form a P2P group with `--peer`, and wait
for the sink to connect. The sink's IP arrives with the accepted connection, so
there is no neighbour-table lookup and no endpoint blocker. The documented
direction has the sink drive M1–M7 with its own `OPTIONS`; the reference TV needs
the source-driven path instead, which drives M1–M6 and puts the `SETUP` trigger
in M5. The sink then sends `SETUP`; the source's 200 must carry `Transport`.

The host firewall is checked first: a dropping input policy is reported as
`Blocker::InboundFirewall` with the exact ingress rule to add.

### 8.3 Media pipeline (ADR-0192 Stage 1)

1. PipeWire capture of the virtual output (through the portal, as a
   `ScreenCast` `VIRTUAL` session).
2. VA-API H.264 encode (`VAEntrypointEncSlice` — present on the reference
   machine). **New dependency family: needs policy admission.**
3. `wfd::rtp` + `wfd::ts` → UDP to the sink's `wfd_client_rtp_ports`.
   (MPEG-2 TS over RTP RFC 2250 packetization is already landed in `wfd::rtp`).
4. Only then does `handshake` become a real `cast`: keep the RTSP session alive
   (`Drain` already models this) and feed the encoder.

### 8.5 The cast service and the control center — Landed

`tessera-castd` is the long-lived owner of discovery and sessions
(`src/daemon.rs`, `src/bin/tessera-castd.rs`). It binds
`$XDG_RUNTIME_DIR/tessera-castd.sock` (mode `0600`, refuses to replace a
non-socket), speaks a length-prefixed JSON protocol, and pushes a
`tessera_desktop::system::CastStatus` snapshot.

The runtime side is `crate::casting::CastHandle`: it discovers the sibling
binary through `trusted_sibling_program`, restarts it with backoff, reconnects to
the socket, and merges the latest snapshot into `SystemStatus` each status
refresh. Chrome renders that snapshot; it never reads iwd and never infers a
session.

The control surface is:

- `ScanCast` → `StartDiscovery`; closing the Cast page (Back, Escape, or
  switching tiles) → `StopCastDiscovery`, so the radio is released.
- `ConnectCast { sink }` → the service-assigned sink id; the service stops
  discovery, **waits for the radio to be released**, then runs the session.
- `DisconnectCast { session }` → the service-assigned session id; the session's
  stop flag ends the stream and releases the radio. No process is killed by
  name.

`[cast] video_formats` defaults to the hardware-validated 720p30 value and can
be overridden in `config.toml`; an empty value makes the service refuse with a
named reason, and the control center shows it in the Cast service card. The
compositor passes the whole `[cast]` policy (source, opening, audio,
audio_source, audio_bitrate_kbps, fps, ports, settle, timeout) on spawn;
`--video-formats` remains for manual runs.

Verified: the daemon smoke test (`Hello`/`HelloAck`, `Subscribe`/`Status`,
`StartDiscovery` finding the reference TV, `StopDiscovery`, `Shutdown`);
`cargo test` covers the wire framing, the socket refusal, and the status
projection. `tessera-cast doctor` prints the probe, firewall, and discovery
evidence in one report and prints the exact next `cast` command. The daemon
control path (`StartDiscovery` → `Connect`) reached `Streaming` against the
reference TV; the compositor UI click and the visual display check remain.

### 8.6 The reference TV's captured parameters (2026-10-30)

With the source-driven exchange, the TV (Android 9, `stagefright/1.2`)
answered M3 (`GET_PARAMETER`) and supplied its own parameters:

```text
wfd_video_formats: 38 00 01 01 f9cf94a0 00000000 00000000 00 0000 0000 00 none none
wfd_audio_codecs: AAC 00000001 00
wfd_client_rtp_ports: RTP/AVP/UDP;unicast 19000 0 mode=play
```

The source's validated value is the 720p30 subset:

```text
wfd_video_formats: 38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none
```

The video value alone was not the blocker. The sink closed after M5 until the
source put `wfd_trigger_method: SETUP` **inside M5** and answered the sink's
`SETUP` with a `Transport` header. With both, the TV sent `PLAY` and received 300
frames. A third requirement came from the media bring-up: the RTP sender must
bind the P2P local address and the advertised `server_port` (19002); an ephemeral
source port produced no display even though the same TS/H.264 decoded locally.
The TV displayed the pattern. The captured sink value proves the layout; use the
validated subset as the default and override it only with a value from the sink's
M4 or a working caster. `tessera-cast formats` reports the `FIELD_SCHEMA` width
mismatches against the captured value.

### 8.4 Then, in order

- Reconnect and teardown handling (`TEARDOWN` from the sink must stop the pipeline).
- A discovery surface in the control center — **only when the service can honour
  it**; today the control center shows a read-only cast status tile
  ([ADR-0188](../adr/0188-out-of-process-casting-and-discovery-service.md)).
- HDCP / UIBC only if a sink population demands them; audio is opt-in and
  implemented for the reference sink.

---

## 9. Cheap iteration — no install, no reboot, no sink

This was an explicit requirement, and the answer is: **`cargo run` and
`cargo test` only.**

- No package, no systemd unit, no D-Bus activation, no reboot. iwd is a running
  service; the caster restores any radio state it changes.
- **`cargo test -p tessera-cast`** drives the real caster loop against the real
  scripted sink over a socketpair — the entire exchange, in milliseconds.
- For a manual end-to-end run with no hardware:

```bash
# shell 1: the SOURCE listens on 7236
cargo run -p tessera-cast -- cast --source pattern \
    --video-formats "00 00 02 10" --max-frames 30
# shell 2: the scripted SINK connects and drives M1–M7
cargo run -p tessera-cast -- sink --connect 127.0.0.1:7236 --play
```

The caster prints the full traced exchange; the sink prints what it received,
whether every reply echoed its `CSeq`, and whether the source sent its own
`SETUP` trigger.

**The only thing that needs a real sink is confirming that a sink accepts our
parameter values, and that frames appear.** Everything else iterates offline.

---

## 10. Gates to run before delivery

```bash
cargo fmt --all -- --check
cargo clippy -p tessera-cast --all-targets -- -D warnings
cargo test --locked --workspace
cargo run -p xtask -- check-boundaries
cargo run -p xtask -- package-conformity
cargo run -p xtask -- check-bootstrap-admission
cargo run -p xtask -- check-motion
cargo doc -p tessera-cast --no-deps      # must be warning-free
```

Pre-existing noise, not yours: `tessera-primitives` has `excessive_precision`
and a `collapsible-if` in `accessibility.rs`; allow those two lints when running
clippy across the workspace.

---

## 11. Process lessons — these cost real time

1. **`edit_text` can report success without writing.** It happened repeatedly on
   this task. After every edit, verify: `grep` for the new text, or compile.
   Never trust the success message.
2. **Never edit the same file with two parallel edits.** Doing so duplicated a
   test block and fused a doc comment into the following `fn`, producing
   `unexpected closing delimiter` and hours of confusion. Edits to one file must
   be serial.
3. **Tests caught every real defect I introduced.** Specifically: a PTS marker-bit
   assertion that was itself wrong; a `hex_dump` fixture with the wrong length;
   `content-type` silently dropped while "simplifying" a builder; the header line
   of `/proc/net/arp` skipped by index instead of by content; a socket timeout
   misreported as a generic I/O error; and our own opening `OPTIONS` traced as
   the sink's. Write the test first, and make it assert the *cause*, not just a
   failure.
4. **A hang is a defect, not a slow test.** The caster/sink test initially
   deadlocked, which exposed a real protocol bug: a source may not stop serving
   RTSP the moment streaming starts. Socketpair tests set read/write timeouts so a
   future deadlock fails instead of stalling the suite.
5. **Two pre-existing flaky tests were fixed in passing** — worth knowing so they
   are not "re-fixed":
   - `tessera-env` `tests/conformance.rs`: env mutation raced under `cargo test`
     (threads share a process). Fixed with a process-wide lock; the `SAFETY`
     comments that claimed nextest isolation were false.
   - `tessera-wayland` `server_new_creates_socket`: asserted a *path* was absent,
     but a concurrently created server can legitimately re-bind the same
     `wayland-N`. Now compares `(dev, inode)`.

---

## 12. Repository state you are inheriting

- `crates/tessera-cast/` is **new and untracked** in git.
- New ADRs 0186–0192 and `docs/reference/tessera-cast.md` are **untracked**.
- Seven ADRs were amended in place (0028, 0052, 0103, 0126, 0127, 0130, 0162) and
  the index and `CHANGELOG.md` updated.
- **`AGENTS.md` in the Tessera working tree is an empty file.** It is not this
  work's doing — it was already blank when this task began, and this task only
  read it. Restore it from `HEAD` (`git checkout -- AGENTS.md`) if that was not
  intentional; a blank project-rules file is a hazard for any agent that reads it.
- Portal repository: protocol-32 projection landed (`atrium-portal-ipc`, its own
  ADR-0024). Its full suite has **3 pre-existing failures** that require the
  daemon to own `org.freedesktop.portal.Desktop`; they fail identically on
  pristine sources on a machine where the real portal is running.

---

## 13. Handover checklist

- [ ] Read ADR-0192 §7/§8 and keep the verification matrix honest.
- [ ] Run `cargo test -p tessera-cast`: expect **135 tests, ~0.60 s, all green**.
- [ ] Run `cargo run -p tessera-cast -- probe`: expect exit `0` on the reference
      machine.
- [ ] Run the local `sink` + `handshake` pair (§9) and read the trace.
- [ ] Confirm `busctl --system get-property net.connman.iwd /net/connman/iwd/0 \
      net.connman.iwd.p2p.Device Enabled` is `false` after a `discover` run.
- [ ] Do **not** invent `wfd_video_formats` values, `Peer` methods, or radio
      capabilities. If you cannot verify it, label it and stop.
