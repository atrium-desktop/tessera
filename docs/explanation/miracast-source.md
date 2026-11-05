# Miracast Source: Protocols, Architecture, and Media Path

This document is the technical map of Tessera's first-party Miracast (Wi-Fi
Display, WFD) source. It explains what protocols are used, how the pieces fit
together, which parts are hardware-validated, and where the remaining limits
are. The operational runbook, diagnostic playbook, and hardware quirks
are in [`docs/dev/runbooks/miracast.md`](../dev/runbooks/miracast.md). Decisions are
recorded in [ADR-0192](../adr/0192-first-party-miracast-caster.md) and
[ADR-0191](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md).

## Status at a glance

- **Pattern video is hardware-validated** against the reference Skyworth A3F_99E
  (`Android 9`, `stagefright/1.2`): the TV displays the 720p30 test pattern. The
  desktop path shares the media pipeline but its live display/audio check is
  still the remaining validation.
- **Audio is implemented as an opt-in feature**: PipeWire system-audio capture →
  AAC-LC → the same MPEG2-TS/RTP flow. The TV plays the tone; the ffmpeg
  reference `ref.ts` is continuous and `ref_small.ts` is near-continuous with
  occasional clicks. The all-IDR test pattern is worse; that points away from
  audio capture, but the video encoding and the muxer/SDT remain separate
  variables. The desktop (VA-API P-frames) is the final validation path.
- **The Control Center backend is validated** (`StartDiscovery` → `Connect` →
  `Streaming`); the compositor UI click and the live desktop visual/audio check
  are the remaining product checks.
- **Non-goals:** HDCP, UIBC, AirPlay/FairPlay mirroring, DLNA desktop mirroring.
- **Known infrastructure limit:** Wi-Fi Direct P2P uses the same physical Wi-Fi
  radio as the station link. A driver/firmware that cannot do concurrent
  STA + P2P (or cannot put both on one channel) will lose normal networking
  while casting. See §5.3.

## 1. Vocabulary and roles

| Term | Meaning here |
|------|--------------|
| **WFD** | Wi-Fi Display, the Miracast protocol family |
| **Source** | The device that sends video/audio. **Tessera is the source.** |
| **Sink** | The TV/projector that renders. The reference TV is a sink. |
| **P2P / Wi-Fi Direct** | The direct Wi-Fi group between source and sink |
| **GO** | P2P Group Owner. In the reference captures the **TV is the GO** (`192.168.49.1`); the source is a P2P client (`192.168.49.x`). |
| **RTSP** | The WFD control protocol (RFC 2326). **The source is the RTSP server.** |
| **WFD IE** | The Wi-Fi Display Information Element in P2P probe/discovery frames |
| **MPEG2-TS** | The transport stream that carries H.264 and AAC |
| **RTP/RTCP** | Real-time media transport and clock/sender reports |

The direction is not optional: the sink connects to the source and the source
serves RTSP. Code that makes the source dial the sink's port 7236 implements a
different protocol and must not be reintroduced.

## 2. Protocol stack

| Layer | Protocol / mechanism | Implemented in | Notes |
|-------|----------------------|----------------|-------|
| Radio / group | Wi-Fi Direct (P2P) through **iwd** | `radio/iwd.rs`, `discover.rs` | `RegisterDisplayService`, `RequestDiscovery`, `SimpleConfiguration.PushButton`, `Peer.Disconnect` |
| Discovery | P2P probe + **WFD IE** | iwd / WFD | **Not mDNS**; see §2.1 |
| Control | **RTSP 1.0** (RFC 2326), WFD M1–M6, `SETUP`/`PLAY` | `wfd/rtsp.rs`, `wfd/session.rs`, `cast.rs` | source is the RTSP server on 7236 |
| Parameters | WFD `text/parameters` (`wfd_video_formats`, `wfd_audio_codecs`, `wfd_client_rtp_ports`, `wfd_presentation_URL`, `wfd_trigger_method`, …) | `wfd/parameters.rs`, `wfd/session.rs` | never invent packed bitfields |
| Video | H.264 Annex-B (ITU-T H.264) | `media/pattern.rs`, `media/desktop.rs`, `media/h264.rs` | pattern is pre-encoded; desktop uses VA-API |
| Container | **MPEG2-TS** (ITU-T H.222.0 / ISO/IEC 13818-1) | `wfd/ts.rs` | PAT/PMT, SDT, PES, PCR |
| Transport | **RTP** over UDP (RFC 3550; MPEG2-TS payload, RFC 2250/3551, **PT 33**) | `wfd/rtp.rs`, `media/udp.rs` | 7 TS packets per datagram; 90 kHz timestamps |
| Clock / sync | **RTCP** SR + SDES (RFC 3550) | `media/rtcp.rs` | source sends SR to the sink's RTCP port |
| Audio | PipeWire sink monitor or source → **AAC-LC ADTS** (ffmpeg) | `media/audio.rs` | opt-in; same TS/RTP flow |
| Screen capture | xdg-desktop-portal **ScreenCast** + PipeWire | `media/portal.rs`, `media/desktop.rs` | portal consent |
| Video encode | VA-API H.264 slice encoder | `media/desktop.rs`, `media/h264.rs` | `VAEntrypointEncSlice` |
| Service / UI | `tessera-castd` Unix socket + `CastStatus` | `daemon.rs`, `tessera/src/casting.rs`, Control Center | compositor never owns iwd |

### 2.1 mDNS is not the Miracast discovery path

Miracast/WFD discovery is **Wi-Fi Direct P2P discovery plus the WFD Information
Element**, followed by RTSP. It does **not** use mDNS/DNS-SD.

mDNS (RFC 6762) and DNS-SD (RFC 6763) are used by AirPlay, Chromecast, DIAL and
similar application-level discovery. The broader casting service in ADR-0188
maps those protocols, but this WFD source has **no AirPlay backend**; AirPlay
mirroring is ruled out by FairPlay. Do not add mDNS to the WFD path to "fix
discovery"; if no WFD peer is visible, inspect iwd P2P, the WFD IE, the peer's
Screen Mirroring mode, and the host's P2P state.

## 3. End-to-end architecture

```text
┌──────────────────────────── Tessera compositor ───────────────────────────┐
│  Control Center / Cast surface                                            │
│       │  CastStatus (service state, discovery, sink capability, session)   │
│       ▼                                                                   │
│  tessera::casting::CastHandle ── Unix socket ──► tessera-castd            │
└───────────────────────────────────────────────────────────────────────────┘
                                                        │
                                                        │ iwd D-Bus (P2P)
                                                        ▼
                                              ┌──────────────────┐
                                              │  Wi-Fi radio      │
                                              │  P2P group (GO)   │
                                              └────────┬─────────┘
                                                       │ 192.168.49.0/24
                                                       ▼
                                        ┌────────────────────────┐
                                        │  Miracast sink (TV)    │
                                        │  RTSP client on 7236   │
                                        │  RTP receiver 19000    │
                                        └────────────────────────┘

tessera-castd
  ├─ radio/iwd: enable P2P, register WFD source, discovery, group formation
  ├─ session: accept the sink on 7236, WFD handshake, media loop
  ├─ media: portal/PipeWire/VA-API or pattern; optional PipeWire audio
  └─ wfd: RTSP, parameters, TS, RTP, RTCP
```

### 3.1 Component map

| Path | Responsibility |
|------|----------------|
| `crates/tessera-cast/src/session.rs` | one session: P2P, accept, handshake, media, cleanup |
| `crates/tessera-cast/src/cast.rs` | RTSP accept/handshake drivers and the permissive responder |
| `crates/tessera-cast/src/wfd/session.rs` | WFD state machine, M1–M6, trigger, SETUP/PLAY, trace |
| `crates/tessera-cast/src/wfd/parameters.rs` | WFD parameter validation/serialization |
| `crates/tessera-cast/src/wfd/ts.rs` | MPEG2-TS: PAT/PMT/SDT, PES, PCR, AAC audio |
| `crates/tessera-cast/src/wfd/rtp.rs` | RTP payloading (PT 33), sequence/timestamp/marker |
| `crates/tessera-cast/src/media/rtcp.rs` | RTCP SR + SDES; receiver-report diagnostics |
| `crates/tessera-cast/src/media/stream.rs` | media pacing, TS/RTP send, audio interleave |
| `crates/tessera-cast/src/media/audio.rs` | `pw-record` → ffmpeg AAC capture, ADTS parsing |
| `crates/tessera-cast/src/media/desktop.rs` | portal + PipeWire + VA-API desktop source |
| `crates/tessera-cast/src/daemon.rs` | long-lived service, control protocol, `CastStatus` |
| `crates/tessera/src/casting.rs` | client/supervisor; compositor projection |

### 3.2 Control plane and media plane are separate

The control plane is the `tessera-castd` Unix socket plus RTSP. The media plane
is one UDP/RTP flow (MPEG2-TS with H.264 and, when enabled, AAC). The compositor
only sees `CastStatus`; it never opens iwd, starts a caster, or kills a process.

## 4. Discovery and connection

1. `discover` enables the P2P role and registers this machine as a WFD source
   with iwd. Discovery is an **action**; `probe` is read-only.
2. The sink (TV) appears as a peer with a `p2p.Display` interface. A sink does
   **not** need to advertise a WFD RTSP port: the source is the server and its
   own advertised control port is what matters.
3. The source binds its control port (default **7236**) **before** discovery,
   registers it with iwd, and optionally forms the P2P group with `--peer`.
4. The source waits for the sink to connect. The sink's IP arrives with the
   accepted TCP connection; no neighbour-table lookup is needed.
5. The sink sends its first `OPTIONS`. For the reference Android 9 sink the
   source must then drive the exchange; see §5.

P2P is exclusive: discovery and a session cannot own the radio at the same time.
Starting a session stops discovery and waits for the radio to be released. The
radio state is restored on exit.

### 4.1 The reference sink

| Property | Value |
|----------|-------|
| Model | Skyworth A3F_99E |
| User agent | `stagefright/1.2 (Linux;Android 9)` |
| M4 `wfd_video_formats` | `38 00 01 01 f9cf94a0 00000000 00000000 00 0000 0000 00 none none` |
| M4 `wfd_audio_codecs` | `AAC 00000001 00` |
| M4 `wfd_client_rtp_ports` | `RTP/AVP/UDP;unicast 19000 0 mode=play` |
| WFD port | absent; still castable in the passive direction |
| Behaviour | exits Screen Mirroring after a failed/ended session; must be re-enabled before each test |
| Phone-cast audio | works, so the TV's WFD audio path is functional |

### 4.2 STA + P2P concurrency and normal networking

Miracast is Wi-Fi Direct: the source and sink form a P2P group on the same
physical radio. Whether the normal station (STA) link survives while the P2P
group is active is a **driver/firmware/hardware capability**:

- If the driver supports concurrent `managed` + `P2P` interfaces, the station
  can remain (on one shared channel or, as on the reference host, on two
  channels).
- If the driver lacks the combination, or the P2P/AP channels exceed its
  `#channels`, the station drops or disconnects.

This is not a caster policy decision. ADR-0191 assigns P2P to the system Wi-Fi
authority; the compositor/caster do not own the station.

On the reference host (`phy0`, station on 2.4 GHz channel 6) `iw list` reports:

```text
#{ managed } <= 2, #{ P2P-client, P2P-GO } <= 1, #{ P2P-device } <= 1,
total <= 4, #channels <= 2
```

A live test with the TV confirmed the advertised steady-state concurrency: the
P2P group ran on channel **11**, the station stayed on channel **6**, and
AP/internet pings succeeded during the P2P session. During the **~6 s group
formation window**, however, the station was disconnected and AP/internet pings
failed: this is a real outage, not merely control-plane setup latency. Once the
station reassociated on channel 6 while the P2P group was on channel 11, both
worked concurrently. A short session streamed 60 frames while the network
remained usable. The **~6 s outage is a limitation of the tested Linux/iwd/driver
group-formation path**, not evidence that the hardware lacks steady-state
concurrency: vendor stacks often hide the transition or use a dedicated P2P
radio, while iwd can take the station down to form the group. The caster cannot
change that. If uninterrupted networking is required, use Ethernet or a second
Wi-Fi adapter; alternatively test `wpa_supplicant`/GNOME Network Displays as a
different P2P backend. The teardown transition was not separately measured.

Check the actual adapter with `iw list` (look for `managed` + `P2P` in the same
combination, `#channels`, and the P2P/AP channels); do not generalize this result
to another adapter. A future `[cast] p2p_adapter` selection can make the
second-adapter path explicit.

## 5. WFD RTSP exchange

### 5.1 The sequence that works on the reference sink

```text
source → sink   OPTIONS *                         (M1)
sink   → source 200 OK                            (M2)
source → sink   GET_PARAMETER *                   (M3)
sink   → source 200 OK + M4 parameters            (M4)
source → sink   SET_PARAMETER *                   (M5)
                wfd_audio_codecs: AAC 00000001 00
                wfd_presentation_URL: rtsp://<source>/wfd1.0/streamid=0 none
                wfd_video_formats: <validated 720p30 value>
                wfd_trigger_method: SETUP
sink   → source 200 OK                            (M6)
sink   → source SETUP <presentation URL>          ; Transport: RTP/AVP/UDP;...
source → sink   200 OK                            ; Session: 1
                                                  ; Transport: ...;server_port=19002-19003
sink   → source PLAY <presentation URL>           ; Session: 1
source → sink   200 OK
source → sink   RTP/TS media                      ; from 19002 to 19000
source → sink   RTCP SR                           ; from 19003 to 19001
```

The CLI default is sink-first; `[cast] opening = "source"` is the default for the
reference TV. Use `--opening source` for a manual run. The documented Miracast
direction is sink-first (`--opening sink`); the permissive responder supports
both.

### 5.2 Message responsibilities

| Message | Direction | Required facts |
|---------|-----------|----------------|
| M1 `OPTIONS` | source → sink (source-driven) | `Require: org.wfa.wfd1.0` |
| M2 `200` | sink → source | `Public` |
| M3 `GET_PARAMETER` | source → sink | ask for the sink parameters |
| M4 `200` + parameters | sink → source | source absorbs the sink's video/audio/RTP ports |
| M5 `SET_PARAMETER` | source → sink | source parameters **plus the trigger in the body** |
| M6 `200` | sink → source | trigger accepted |
| `SETUP` | **sink → source** | presentation URL + `Transport: ...client_port=...` |
| `SETUP 200` | source → sink | `Session` **and** `Transport` with `server_port` |
| `PLAY` | **sink → source** | `Session` |
| `PLAY 200` | source → sink | `Session` |
| media | source → sink | RTP PT 33, source port 19002 → sink 19000 |
| RTCP SR | source → sink | from 19003 → 19001 |

### 5.3 The non-obvious constraints

These are protocol requirements for this sink, not optional tuning. Each one was
observed as a hard failure; the detailed case files are in
[`miracast runbook`](../dev/runbooks/miracast.md).

1. **The trigger belongs in the M5 body.** `wfd_trigger_method: SETUP` inside
   M5 is accepted. A separate source `SET_PARAMETER` after M6 (M7) gets no
   response; the TV closes.
2. **The `SETUP` 200 must carry `Transport`.** Echo the sink's
   `client_port=19000-19001` and add `server_port=19002-19003`; the TV otherwise
   closes after one frame.
3. **RTP must leave from the advertised `server_port` on the P2P local
   address.** A media sender bound to an ephemeral port produces a valid local
   TS/H.264 stream that the TV never displays. The send socket is bound to
   `<rtsp-local-ip>:19002`.
4. **The source-side RTCP port must be the advertised 19003**, and the SR is
   sent to the sink's 19001. The reference TV does not send RTCP receiver
   reports, so loss cannot be read back from this sink.

## 6. WFD parameters

The WFD parameters are line-based `text/parameters`. The source **never invents**
a packed `wfd_video_formats` bitfield; the validated 720p30 value is the default
in `[cast]`, and a different sink should supply its own value from M4.

| Parameter | Source | Validated value / policy |
|-----------|--------|--------------------------|
| `wfd_video_formats` | source / config | `38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none` (CEA 720p30 subset of the TV's `f9cf94a0` bitmap) |
| `wfd_audio_codecs` | sink M4, echoed in M5 | `AAC 00000001 00` when audio is enabled; `none` otherwise |
| `wfd_client_rtp_ports` | sink M4 | `RTP/AVP/UDP;unicast 19000 0 mode=play` |
| `wfd_presentation_URL` | source M5 | `rtsp://<p2p-local-ip>/wfd1.0/streamid=0 none` |
| `wfd_trigger_method` | source M5 | `SETUP` |
| `wfd_content_protection` | source M5 | `none` (no HDCP) |
| `wfd_uibc_capability` | source M5 | `none` (no UIBC) |

The source accepts AAC only for audio. If the sink advertises no AAC (or a
codec the source cannot encode), the session continues video-only with a named
`AudioUnavailable` event; it does not claim audio.

## 7. Media path

### 7.1 Video

- **Pattern**: `assets/pattern_720p.h264`, a 1280×720 Constrained Baseline
  Level 3.1 IDR frame repeated at the requested frame rate. This is a Stage-0
  diagnostic source, not the final desktop source.
- **Desktop**: `org.freedesktop.portal.ScreenCast` (v6) → PipeWire → normalized
  720p/1080p path → VA-API H.264 (`VAEntrypointEncSlice` on
  `/dev/dri/renderD128`).
- The source advertises 720p30 in `wfd_video_formats`; the desktop normalizes
  the capture to a standard size and the encoder produces Annex-B access units.
- `--fps` controls the frame pacing. Diagnostic tests often use `--fps 10` to
  reduce the bitrate; the default and the Control Center use **30 fps**.
- The pattern's single-IDR repetition has a much higher bitrate than a normal
  P-frame sequence and can expose sink decoder/audio behaviour that the desktop
  path does not. Use the desktop path for the final judgement.

### 7.2 MPEG2-TS layout

| PID | Content | Cadence |
|-----|---------|---------|
| `0x0000` | PAT | every 100 ms and on the first frame |
| `0x1000` | PMT | every 100 ms and on the first frame |
| `0x0100` | H.264 video PES + PCR | every video access unit; PCR every 100 ms |
| `0x0101` | AAC ADTS audio PES | groups of 8 AAC frames |
| `0x0011` | SDT | present in the ffmpeg reference; **not emitted in production** (the reference TV reacted badly to our first SDT implementation; revisit only with a capture) |

The metadata choices are deliberate:

- PAT/PMT every **100 ms** (matching the ffmpeg reference), not every second.
- Video/audio PIDs `0x0100`/`0x0101` (matching the reference harness).
- PCR on the video PID, every **100 ms** (every 3 frames at 30 fps, every frame
  at 10 fps).
- PES header flags, PTS, and audio PES length match the ffmpeg reference
  (`0x80`, PTS-only, bounded audio PES).
- Audio is grouped 8 ADTS frames per PES; one tiny PES per 21 ms frame can
  starve some Android audio decoders.
- A tail TS packet reserves the adaptation-field bytes before choosing its
  payload; otherwise the stuffing calculation underflows.
- The source starts the media timeline at **1.5 s** (`MEDIA_START_PTS`) to give
  the sink decoder a lead; ffmpeg's reference starts around 1.4 s.

### 7.3 RTP

- One RTP flow carries the whole TS: payload type **33** (MPEG2-TS), 7 TS
  packets (1316 bytes) per datagram, SSRC `0x1900_0000`, 90 kHz timestamps,
  marker on the last packet of the access-unit group.
- The source binds the media socket to the **RTSP local P2P address** and
  **port 19002** (the advertised `server_port`). The sink receives on 19000.
- RTP packets are paced evenly across the frame interval. Bursting all packets
  of a frame and then sleeping causes occasional audio gaps on the reference
  TV; after pacing, the steady-state audio is continuous.
- The UA/RTP source port is part of the transport contract: a sink may filter
  RTP by the source port named in `SETUP`.

### 7.4 RTCP

- The source sends a compound **SR + SDES(CNAME)** packet from `19003` to
  `19001` once per second, with the current RTP timestamp and sender counts.
- The source parses RTCP RR packets if a sink sends them. The reference TV does
  **not** send RR, so fraction-lost/cumulative-lost is not available from this
  sink; do not interpret "no RR" as "no loss".
- RTCP does not carry the media; it provides the NTP/RTP clock relationship for
  A/V sync.

### 7.5 Audio

- **Opt-in** (`[cast] audio = true` or `--audio`); the default is off because
  capturing the desktop's audio is a privacy decision.
- Capture: `pw-record --target <default sink id> -P stream.capture.sink=true
  --raw` (system output) or the default source (microphone). The
  `stream.capture.sink` property is what makes the target a monitor: a sink is
  an output-direction node, so without it PipeWire links the capture stream to
  the default *source* and the "system" mode records the room. The local monitor
  test shows the tone at about `-30 dB`; the default sink must not be muted.
- Encode: `ffmpeg -f s16le -ar 48000 -ac 2 -i pipe:0 -c:a aac -b:a <kbps>k -f
  adts pipe:1`; AAC-LC, 48 kHz, stereo, 128 kbit/s default.
- The reader splits ADTS frames, computes the 90 kHz PTS (1024 samples per
  frame), groups 8 frames per PES, and interleaves them into the same
  TS/RTP flow. `AudioFrameSource` keeps the stream layer independent of the
  subprocesses.
- Startup: capture starts before the RTSP handshake to build a pre-roll; the
  first audio group is flushed on the first video frame; the media timeline has
  a 1.5 s lead.
- Ownership: `AacCapture` owns the `pw-record` and `ffmpeg` children and kills
  and reaps them on drop. No `pkill`.
- Negotiation: the source echoes the sink's AAC value in M5. The TV advertises
  `AAC 00000001 00`; the source sends the matching AAC-LC stream.
- Current limits: the test pattern's all-IDR video and some bitrates still
  produce a few short clicks on the reference TV even though the AAC/TS are
  clean locally; the desktop P-frame path is the final validation. The source
  never claims audio when it is unavailable; `AudioActive`/`AudioUnavailable`
  events carry the truth.

### 7.6 Frame rate and bitrate

- The default is 30 fps. The diagnostic runs used `--fps 10`; do not judge the
  final frame rate from those. Measure the source's actual FPS as
  `frames_sent / duration`, and restart `tessera-castd` after changing
  `[cast] fps`.
- Isolate the layer: run the **pattern** at 30 fps and the **desktop** at 30 fps
  separately. If the pattern is smooth and the desktop is not, the bottleneck is
  desktop capture/encode (`ScreenCast` portal, PipeWire, VA-API), not the radio.
- The station and P2P share airtime even when they use two channels. Moving the
  station to **Ethernet** or a second adapter frees the radio for P2P and also
  removes the ~6 s formation outage; this is the largest single win on the
  reference host.
- Check the P2P link and channel (`iw dev`, `station dump`): the reference TV's
  GO is on 2.4 GHz channel 11; a 5 GHz P2P or a dedicated adapter can improve
  the link. On the source side, lower resolution/bitrate only after confirming
  the capture path.
- Use the phone→TV cast as the sink baseline. If the phone is smooth and the
  source is not, compare the source's P2P link, bitrate, and profile before
  blaming the TV.

## 8. Service, daemon, and Control Center

- `tessera-castd` is a long-lived per-user service on
  `$XDG_RUNTIME_DIR/tessera-castd.sock` (mode `0600`). It owns discovery and
  sessions; it refuses to overwrite a non-socket path.
- Wire format: 4-byte big-endian length + JSON, versioned. Requests:
  `Hello`, `Subscribe`, `StartDiscovery`, `StopDiscovery`, `Connect { sink }`,
  `Disconnect { session }`, `Shutdown`. Events: `HelloAck`, `Status`, `Error`.
- A session stops discovery and waits for the radio before starting. The
  compositor `CastHandle` starts/supervises the daemon, reconnects, and publishes
  `CastStatus` snapshots.
- `CastSink` carries the sink's `Wfd` capability and blocker. `CastSession`
  carries state, frames, optional audio codec, and failure. The Control Center
  only offers Connect when the service is available and the blocker is `None`.
- No fabricated `connected` state, no `pkill`, and no compositor ownership of
  iwd/P2P. Audio is shown as active only when `AudioActive` was emitted.

## 9. Configuration and commands

```toml
[cast]
# Validated against the reference Skyworth A3F: CEA 720p30.
video_formats = "38 00 01 01 00000020 00000000 00000000 00 0000 0000 00 none none"
source = "desktop"       # "desktop" | "pattern"
opening = "source"       # "source" | "sink"
audio = false            # explicit opt-in
audio_source = "system"  # "system" | "microphone"
audio_bitrate_kbps = 128
fps = 30
control_port = 7236
rtp_port = 19000
settle_seconds = 4
session_timeout_seconds = 60
```

| Setting | Default | Meaning |
|---------|---------|---------|
| `video_formats` | validated 720p30 | packed WFD value; override from a sink's M4 for another sink |
| `source` | `desktop` | ScreenCast/VA-API or the test pattern |
| `opening` | `source` | source-driven exchange (reference TV) or sink-first |
| `audio` | `false` | opt-in system/microphone audio |
| `audio_source` | `system` | default sink monitor or default source |
| `audio_bitrate_kbps` | `128` | AAC bitrate, 32..=512 |
| `fps` | `30` | 1..=120 |
| `control_port` | `7236` | RTSP listen/advertised port |
| `rtp_port` | `19000` | fallback sink RTP port |

The CLI reference is [`docs/reference/tessera-cast.md`](../reference/tessera-cast.md).
`tessera-cast doctor` reports the radio/pipeline, inbound-firewall verdict,
live discovery peers, and the next command.

The compositor launches `tessera-castd` with these values. Edit the config before
launching, or restart the compositor/cast service after a change: a daemon that
was already spawned does not pick up new spawn arguments.

## 10. Validation matrix

| Area | Evidence | Remaining |
|------|----------|-----------|
| RTSP framing, TS/RTP/H.264 payloads, WFD parameter parsing | byte-level fixtures and round-trips | none |
| P2P enable/register/discover/group | verified live with iwd against the TV | none |
| Source-first handshake | M1–M6, SETUP, PLAY; 300/900 frames | none |
| SETUP transport | 200 with `Session`/`Transport`; one-frame failure without it | none |
| RTP source port | 19002 bound to the P2P local IP | none |
| Video display | TV displays the 720p30 pattern | desktop visual check |
| Audio | TV plays the AAC tone; `ref.ts` continuous, `ref_small.ts` near-continuous; capture/TS clean | a few all-IDR-pattern clicks; desktop audio |
| RTCP | SR/SDES sent; RR parser available; the TV sends no RR | a sink that reports RR |
| Control Center backend | `StartDiscovery` → `Connect` → `Streaming` | UI click and desktop visual/audio |
| Normal network during cast | **verified on the reference `phy0`**: managed + P2P-device, up to 2 channels; P2P ch11 + station ch6; AP/internet OK after a ~6 s station outage at formation | adapter/AP/TV-channel dependent; Ethernet or a second adapter for uninterrupted networking |

## 11. Limitations, non-goals, and safety

- **Audio is opt-in**; the default is off. Enabling it captures the selected
  PipeWire node; do not enable it silently.
- **No HDCP and no content protection.** DRM-protected playback cannot be
  mirrored; the refusal must remain explicit.
- **No UIBC** (input back-channel).
- **No AirPlay/FairPlay mirroring** (legal risk); mDNS-based protocols are
  outside this WFD source. **DLNA is media push, not desktop mirroring.**
- **P2P is single-radio** on the reference machine; normal networking may drop
  while casting. Use Ethernet, a second adapter, or a second machine. This is a
  driver/hardware boundary, not a caster feature.
- **Portal consent** is required for desktop capture. The caster has no
  compositor IPC scope; it is a portal client.
- The daemon socket is mode `0600` inside the user's runtime directory; the
  protocol grants nothing the user could not do by running the CLI.
- **No `pkill`** and no process-name lifecycle. The daemon uses cooperative
  stop flags and service-assigned session ids.
- Firewall diagnostics read only the **live** ruleset; a persistent
  `/etc/nftables.conf` is not evidence of the live state. Report `unverified`
  rather than a false `BLOCKED`.

## 12. References

### Official specifications

- Wi-Fi Alliance, **Miracast Specification v2.3**:
  <https://www.wi-fi.org/system/files/Miracast_Specification_v2.3.pdf>
- Wi-Fi Display Technical Specification v2.1 (community mirror):
  <https://raw.githubusercontent.com/wiki/albfan/miraclecast/files/Wi-Fi_Display_Technical_Specification_v2.1_0.pdf>
- Wi-Fi Alliance, **Wi-Fi Direct**:
  <https://www.wi-fi.org/discover-wi-fi/wi-fi-direct>
- **RTSP 1.0**, RFC 2326: <https://www.rfc-editor.org/rfc/rfc2326>
- **RTP**, RFC 3550: <https://www.rfc-editor.org/rfc/rfc3550>
- **RTP payload for MPEG2-TS**, RFC 2250: <https://www.rfc-editor.org/rfc/rfc2250>
- **RTP payload types / PT 33**, RFC 3551: <https://www.rfc-editor.org/rfc/rfc3551>
- **MPEG2-TS**, ITU-T H.222.0 / ISO/IEC 13818-1:
  <https://www.itu.int/rec/T-REC-H.222.0>
- **H.264**, ITU-T H.264 / ISO/IEC 14496-10:
  <https://www.itu.int/rec/T-REC-H.264>
- **AAC / ADTS**, ISO/IEC 14496-3 and ISO/IEC 13818-7 (the MPEG-2 AAC
  transport used in TS)
- **mDNS**, RFC 6762: <https://www.rfc-editor.org/rfc/rfc6762>; **DNS-SD**,
  RFC 6763: <https://www.rfc-editor.org/rfc/rfc6763> (not WFD discovery)
- **iwd** documentation: <https://iwd.wiki.kernel.org/>
- **PipeWire** documentation: <https://docs.pipewire.org/>
- **VA-API**: <https://github.com/intel/libva>
- **xdg-desktop-portal ScreenCast**:
  <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html>

### Reference implementations and cross-checks

- GNOME Network Displays: <https://gitlab.gnome.org/GNOME/gnome-network-displays>
- albfan/gst-rtsp-server-wfd:
  <https://github.com/albfan/gst-rtsp-server-wfd>
- albfan/miraclecast: <https://github.com/albfan/miraclecast>
- GStreamer `mpegtsmux`, `rtpmp2tpay`, `rtpbin`:
  <https://gstreamer.freedesktop.org/documentation/>

### Project records

- [ADR-0192: first-party Miracast caster](../adr/0192-first-party-miracast-caster.md)
- [ADR-0191: P2P belongs to the system Wi-Fi authority](../adr/0191-casting-p2p-belongs-to-the-system-wifi-authority.md)
- [Miracast operational runbook](../dev/runbooks/miracast.md)
- [tessera-cast CLI reference](../reference/tessera-cast.md)
- [Configuration reference](../reference/config.md)
