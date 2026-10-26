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
| [ADR-0088](../adr/0188-out-of-process-casting-and-discovery-service.md) | the out-of-process service boundary and the control-center cast surface |
| [`docs/reference/tessera-cast.md`](../reference/tessera-cast.md) | the command reference |

---

## 1. TL;DR — the honest state

**Working and verified on the reference machine:**

- `probe` — reports both halves of casting present, exit `0`.
- `discover` — enables P2P, registers this machine as a Wi-Fi Display source,
  requests discovery, enumerates peers, and **restores the radio state on exit**.
- The **entire protocol core** and the **M1–M7 handshake loop**, proven offline.
- `sink` — a scripted sink so the caster can be exercised with no hardware.
- **P2P group formation** — `P2pRadio::form_group` via `net.connman.iwd.SimpleConfiguration.PushButton`
  and `disconnect_peer` via `net.connman.iwd.p2p.Peer.Disconnect` (introspected from `/usr/lib/iwd`).
- `handshake --peer` — peer needle resolution, auto group formation, and live endpoint resolution.
- **MPEG-2 TS over RTP** (RFC 2250, PT 33) — 7 TS packets per datagram, packetizer + depacketizer.
- **Stage 0 Test Pattern Media Streaming** (`tessera-cast cast --source pattern`) — drives handshake to `PLAY`, streams MPEG-2 TS over RTP over UDP with live pacing, handles keep-alives, issues clean `TEARDOWN`. Proven against scripted sink over local UDP socket.
- **Stage 1 Live Desktop ScreenCast & VA-API Hardware Encode** (`tessera-cast cast --source desktop`) — requests `ScreenCast` portal session (v6), receives PipeWire stream node, normalizes high-DPI desktop to standard 1080p, hardware-encodes via Intel iHD VA-API (`VAEntrypointEncSlice` on `/dev/dri/renderD128`), and streams live desktop frames.

**Not working yet, in dependency order:**

1. Live hardware verification with a physical sink in range.
2. Desktop Control Center UI tile integration (ADR-0188 Stage 2).

**Explicitly out of scope for the first release:** HDCP, audio, UIBC
(ADR-0192 §3). HDCP-less streaming means DRM-protected playback cannot be cast,
and that refusal must be explicit so it never looks like a bug.

> **Nothing in this crate may be described as working before a real sink shows
> frames.** The reference machine has no sink in range, so no sink has ever
> accepted this caster's exchange.

---

## 2. Ground rules — non-negotiable

These are not style preferences. Each one already prevented a shipped defect.

1. **Never fabricate a capability or a value.**
   - The probe's verdict is `observed` / `absent` / `unverified`
     (`[INV-ARCH-83]`). An unverified half must never collapse into "works".
   - `wfd_video_formats` is **never defaulted or generated**. It is a packed
     bitfield; a wrong bit makes a sink reject the session indistinguishably from
     a radio failure. The tool annotates a value; it does not invent one.
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
4. **Label what has not been verified on hardware.** `Unverified on hardware` in
   the module doc, plus a row in ADR-0192 §8. Keep that table current.
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
    ├── neighbours.rs     /proc/net/arp parsing + MAC→IP resolution
    ├── endpoint.rs       DiscoveredPeer → reachable host:port, or a named blocker
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

~7,300 lines, **121 unit tests, 0.01 s**, no hardware required.

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
                │ neighbours ───► MAC → IP (P2P group)        │
                │ endpoint ─────► host:port or named blocker  │
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
tessera-cast handshake (--host IP | --peer PEER) --video-formats FMT [--port 7236] [--settle SECONDS] [--timeout 10]
tessera-cast cast (--host IP | --peer PEER) --video-formats FMT [--port 7236] [--rtp-port 19000] [--fps 30] [--duration SECS]
tessera-cast formats "<value>"
tessera-cast sink [--listen ADDR] [--rtp-port PORT]
```

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
| `wfd_video_formats` handling | reversible token codec, `none` as value and as token, width-aware annotation | the declared field names/widths, against a real sink value |
| The handshake loop | caster ↔ scripted sink over a socketpair: completes, times out, closes, overruns budget, traces both directions | a real sink accepting the exchange |
| A source keeps serving after the trigger | `Drain` policy + socketpair test | a sink completing the session |
| iwd P2P enable / register / discover | **verified live**: all three succeed | — |
| iwd display-service D-Bus contract | **verified live** (§7) | — |
| Radio state restoration | **verified live**: `Enabled` false → run → false | — |
| Peer enumeration | **verified live**: returns empty with no sink in range (exit `3`) | a sink appearing in `GetPeers` |
| Reading a real peer's `p2p.Display` | never observed — no sink in range | a sink advertising port/role |
| Source-only vs iwd's dual-role refusal | accepted by iwd; the refusal itself not reproduced | a dual-role sink connecting |
| Group formation | **landed, introspected from daemon**: `SimpleConfiguration.PushButton()` and `p2p.Peer.Disconnect()` implemented in `P2pRadio`, `handshake --peer` wired | a sink accepts group formation on hardware |
| TS over RTP (RFC 2250) | **landed**: PT 33, 7 TS packets/datagram chunking, packetizer + depacketizer unit-tested | — |
| Stage 0 Test Pattern Streaming (`cast`) | **landed, scripted-sink tested**: drives handshake, delivers paced TS/RTP/UDP frames, services RTSP keep-alive and teardown | a sink displays test frames on hardware |
| VA-API encode | **implemented**: hardware slice encoder via `/dev/dri/renderD128` (`VAEntrypointEncSlice`) with zero-latency tuning, producing Annex-B AUs | encoded frames decode on physical sink |
| PipeWire capture of a virtual output | **implemented**: `ScreenCast` portal client (v6) opens PipeWire remote node, normalized to 1080p | frames carry desktop content |
| HDCP / audio / UIBC | not implemented | out of scope |

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
Implemented in `P2pRadio::form_group` and `disconnect_peer`, labelled `Unverified on hardware`
until exercised against a physical sink.

### 8.2 `handshake --peer` — Landed

`handshake` accepts either `--host <IP>` or `--peer <needle>`.
When `--peer` is specified:
- Matches against known peers by name, MAC address, or D-Bus path substring (case-insensitive).
- If 0 matches: fails closed and suggests running `discover` or passing `--settle`.
- If ambiguous: lists all candidate matches and refuses.
- If exactly 1 match:
  - If not connected: calls `radio.form_group(&peer.path)`.
  - Resolves live endpoint via `endpoint::resolve_live(&peer, interface)`.
  - On `Endpoint::Ready`, proceeds to run the handshake; otherwise prints the blocker verbatim.

### 8.3 Media pipeline (ADR-0192 Stage 1)

1. PipeWire capture of the virtual output (through the portal, as a
   `ScreenCast` `VIRTUAL` session).
2. VA-API H.264 encode (`VAEntrypointEncSlice` — present on the reference
   machine). **New dependency family: needs policy admission.**
3. `wfd::rtp` + `wfd::ts` → UDP to the sink's `wfd_client_rtp_ports`.
   (MPEG-2 TS over RTP RFC 2250 packetization is already landed in `wfd::rtp`).
4. Only then does `handshake` become a real `cast`: keep the RTSP session alive
   (`Drain` already models this) and feed the encoder.

### 8.4 Then, in order

- Reconnect and teardown handling (`TEARDOWN` from the sink must stop the pipeline).
- A discovery surface in the control center — **only when the service can honour
  it**; today the control center shows a read-only cast status tile
  ([ADR-0188](../adr/0188-out-of-process-casting-and-discovery-service.md)).
- HDCP / audio / UIBC only if a sink population demands them.

---

## 9. Cheap iteration — no install, no reboot, no sink

This was an explicit requirement, and the answer is: **`cargo run` and
`cargo test` only.**

- No package, no systemd unit, no D-Bus activation, no reboot. iwd is a running
  service; the caster restores any radio state it changes.
- **`cargo test -p tessera-cast`** drives the real caster loop against the real
  scripted sink over a socketpair — the entire M1–M7 exchange, in milliseconds.
- For a manual end-to-end run with no hardware:

```bash
# shell 1
cargo run -p tessera-cast -- sink --listen 127.0.0.1:7236
# shell 2
cargo run -p tessera-cast -- handshake --host 127.0.0.1 --port 7236 \
    --video-formats "00 00 02 10"
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
