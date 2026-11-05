# tessera-cast

Miracast (Wi-Fi Display) caster for the Tessera desktop
([ADR-0192](../../docs/adr/0192-first-party-miracast-caster.md)).

This crate is an ordinary process, not part of the compositor. It holds **no
compositor IPC scope**: it obtains frames as a `ScreenCast` portal client, so
every capture it performs passes through the portal's consent and session
mediation.

## Status: a source-side Miracast endpoint

The WFD session, the media pipeline, and the source-side RTSP server are
implemented and proven offline. A physical sink has not yet been observed
displaying frames; that is the outstanding gate in ADR-0192 §8.

### The direction is load-bearing

Miracast makes the **source** the RTSP server and the **sink** the client. The
source listens on its control port (7236 by default), the sink connects, and the
sink sends the first `OPTIONS`. This crate therefore **listens**; it never dials
the sink. A source that dials the sink is implementing a different protocol.

That inbound connection is a real firewall dependency: if the host's input policy
drops tcp/7236, the sink's connection is silently discarded. `probe` reports this
as a blocker, and `discover`/`cast` warn with the exact rule to add.

The two halves of Miracast come from different places, so a machine can have one
without the other:

| Half | Provided by |
|------|-------------|
| Radio: a Wi-Fi Display P2P authority | the system's Wi-Fi authority — on the reference configuration, **iwd**, with no NetworkManager and no `wpa_supplicant` |
| Protocol: the WFD session (RTSP M1–M6 + SETUP/PLAY, MPEG2-TS over RTP, H.264) | this crate |

### The probe

```bash
cargo run -p tessera-cast -- probe
```

It reports, for each half, whether the capability was **observed**, **absent**,
or **unverified**, and nothing else. It is read-only: it never enables a P2P
adapter, starts a scan, or writes a property. It also reports whether the host
firewall would accept the sink's inbound RTSP connection; a dropping input policy
is a definite blocker, not a footnote.

Exit codes are the contract, so a gate can branch on them:

| Code | Meaning |
|------|---------|
| `0` | both halves present |
| `1` | a half is definitively absent (actionable) |
| `2` | indeterminate — something could not be inspected |

`2` is never reported as success. An unreadable verdict is not a capability
(`[INV-ARCH-83]`).

### The protocol core (`src/wfd/`)

Two kinds of module, both provable without hardware. First, **pure byte
transformations**:

| Module | Provenance | Evidence |
|--------|-----------|----------|
| `wfd::rtsp` | RFC 2326 framing, `text/parameters` bodies | round-trip, pipelining, truncation, malformed-line rejections |
| `wfd::ie` | WFD Information Element and subelement codec | round-trip, unknown subelements/bits preserved, non-WFD OUI refused |
| `wfd::parameters` | the advertised parameter set | injection defence, round-trip |
| `wfd::formats` | `wfd_video_formats` tokens | round-trip, `none` as value and as token, width-aware annotation that reports layout mismatches instead of hiding them |
| `wfd::rtp` | RFC 6184 H.264 payloads over RFC 3550 headers | single-NAL, FU-A fragmentation, depacketizer reassembly, wrap |
| `wfd::ts` | ISO/IEC 13818-1 PAT/PMT/PES/PCR | CRC verification, PES reassembly, PCR on the first packet, continuity wrap |

Then a **stateful responder** that needs no hardware either, because a scripted
sink drives it:

| Module | Provenance | Evidence |
|--------|-----------|----------|
| `wfd::session` | the sink-driven source session | CSeq echoed for every method, out-of-order tolerated, unknown trigger acknowledged but not acted on, unserializable parameters fail closed |
| `wfd::trace` | two-directional message trace | hex-dump shape, unverified traces never read as proof |
| `sink` | a scripted sink (the other side) | drives the nominal order, answers interleaved requests, checks `CSeq`, and reports what the caster advertised |

`cargo test -p tessera-cast` runs the real source server against the real scripted
sink client over a socketpair, so the entire M1–M6 + SETUP/PLAY exchange is
verified with no
hardware. For a manual run, `tessera-cast cast --source pattern --video-formats
"00 00 02 10" --max-frames 30` in one shell and `tessera-cast sink --connect
127.0.0.1:7236 --play` in another.

Four deliberate choices worth knowing:

- **Unprovable spec details are handled conservatively, not guessed.** `wfd::ie`
  names only the two bitfields this source sets and preserves every other bit and
  unknown subelement verbatim, so a wrong assumption about an unused bit cannot
  corrupt a peer's advertisement.
- **The session reacts instead of insisting on one order.** Sinks vary the
  sequence; a machine that insists on one turns a working sink into a failure.
  The responder answers what it is asked and reports what it understood as
  effects. The source-driven compatibility path drives M1–M6 and puts the
  `SETUP` trigger in M5, because the reference Android 9 sink ignores a separate
  source M7.
- **`wfd_video_formats` is validated, never invented.** Its bitfield packs
  resolutions, micro-steps and profile/level codes; a wrong bit makes a sink
  reject the session with no distinguishable diagnostic. The default is the
  hardware-validated 720p30 subset; overrides come from the sink's M4 value or a
  working caster.
- **A parameter value containing CR, LF, or NUL is refused.** The body is
  line-based, so such a value would forge a further parameter — a source that
  refuses HDCP could be made to look as if it negotiated it.


### The session and media (`src/cast.rs`, `src/wfd/transport.rs`)

`handshake` serves M1–M6 and the sink's `SETUP`/`PLAY`, and traces it; `cast`
additionally streams H.264 in
MPEG-2 TS over RTP. The source is the RTSP **server**, so the loop is: accept the
sink's connection, wait for the sink's `OPTIONS`, answer in order, then stream to
the port the sink names in `SETUP`/`SET_PARAMETER`.

| Module | Provenance | Evidence |
|--------|-----------|----------|
| `wfd::transport` | RTSP framing over `Read + Write` | split reads, pipelined bursts, clean vs partial EOF, a 9 KiB body across 4 KiB reads, socket timeout mapped to its own error |
| `cast` | the source-side server: accept, answer, stream | completes, times out, closes, overruns its budget, traces both directions, and never sends an unsolicited `OPTIONS` to a sink that speaks first |

## `cast`

```bash
cargo run -p tessera-cast -- cast --peer <NEEDLE> --video-formats FMT \
    [--port 7236] [--bind 0.0.0.0] [--rtp-port 19000] \
    [--source desktop|pattern] [--opening sink|source] [--fps 30] \
    [--audio] [--audio-source system|microphone] [--audio-bitrate 128] \
    [--max-frames N]
```

The source binds the control port **before** discovery, registers itself with
that port, forms the P2P group with `--peer`, and waits for the sink to connect.
It then serves M1–M6, answers the sink's `SETUP` with a `Transport` header, and
on `PLAY` streams H.264 in MPEG-2 TS over RTP (RFC 2250, PT 33) to
the sink's negotiated port. `--source pattern` needs no portal or GPU;
`--source desktop` requests a `ScreenCast` session and encodes with VA-API.

Audio is opt-in. `--audio` captures the default sink monitor (or the default
source with `--audio-source microphone`), encodes AAC-LC 48 kHz stereo with
ffmpeg, and interleaves it into the same TS/RTP flow. The sink's monitor is
requested with `stream.capture.sink=true`, so "system" means the desktop's own
playback rather than a microphone. The source echoes the sink's AAC codec in M5;
the default sink must not be muted. See
[Miracast Source §7.5](../../docs/explanation/miracast-source.md#75-audio).

## `sink`

```bash
cargo run -p tessera-cast -- sink --connect 127.0.0.1:7236 [--rtp-port 19000] [--play]
```

A scripted **sink client**: it connects to a running source, sends `OPTIONS`
first, asks for the source's parameters, requests a session, and optionally
receives RTP. This is the direction a real sink uses.

## Hardware validation

The reference Skyworth A3F completed the exchange end-to-end: source-driven
M1–M6, sink `SETUP`, source 200 with `Transport`/`Session`, sink `PLAY`, and
**300 frames / 4,784,680 bytes** at 720p30. **The TV displayed the test pattern**
(color bars, circles, squares). The three requirements this sink exposed:

1. `wfd_trigger_method: SETUP` must be in the **M5** body; a separate source M7
   is ignored and the sink closes.
2. The `SETUP` 200 must include `Transport` (echo `client_port`, add
   `server_port`); without it the sink closes after one frame.
3. RTP must be sent from the advertised `server_port` (19002) on the P2P local
   address; an ephemeral source port produced no display.

The `tessera-castd` control path reached `Streaming` against the same TV. Desktop
capture and the compositor UI click remain the product checks.

HDCP and UIBC remain non-goals. Audio is opt-in and implemented; see the
[architecture](../../docs/explanation/miracast-source.md#75-audio) and the
[operational runbook](../../docs/dev/runbooks/miracast.md).

## `tessera-castd`

The long-lived service the control center talks to (ADR-0188, ADR-0192). It
listens on `$XDG_RUNTIME_DIR/tessera-castd.sock` (mode `0600`), owns discovery
and sessions, and pushes a `CastStatus` snapshot. The compositor starts and
supervises it; chrome never opens iwd or spawns a caster.

```bash
cargo run -p tessera-cast --bin tessera-castd -- --source pattern
```

The compositor passes the `[cast]` configuration (`video_formats`, `source`,
`opening`, `audio`, `audio_source`, `audio_bitrate_kbps`, `fps`, ports, settle,
timeout) on spawn; `--video-formats` remains for manual runs.

## `doctor`

```bash
cargo run -p tessera-cast -- doctor
```

One evidence report: the probe's half-by-half verdict, the inbound-firewall
verdict with the exact ingress rule when blocked, and a live discovery pass with
each peer's castability and reason. When a castable sink is found it prints the
exact `cast` command to run next.

## `discover`

```bash
cargo run -p tessera-cast -- discover [--settle SECONDS] [--port PORT]
```

An **action**, unlike `probe`: it enables the adapter's P2P role, registers this
machine as a Wi-Fi Display **source**, requests discovery, and lists the peers it
found with the reason each one is or is not castable. It restores the P2P switch
and unregisters on exit, so a diagnostic run leaves the radio as it found it.

| Code | Meaning |
|------|---------|
| `0` | at least one castable sink |
| `1` | the run failed (the message names why, e.g. `not authorized`) |
| `3` | the run worked but found no castable sink — open the sink's Screen Mirroring mode |

Verified against live iwd on the reference machine, including the daemon's real
D-Bus contract: `RegisterDisplayService(a{sv})` returns nothing,
`UnregisterDisplayService()` takes no arguments, and iwd rejects unknown keys with
`InvalidArguments` — `HasAudio` is refused whatever its type, so a richer
advertisement degrades to `Source`/`Sink`/`Port` and says it did.
