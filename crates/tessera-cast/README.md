# tessera-cast

Miracast (Wi-Fi Display) caster for the Tessera desktop
([ADR-0192](../../docs/adr/0192-first-party-miracast-caster.md)).

This crate is an ordinary process, not part of the compositor. It holds **no
compositor IPC scope**: it obtains frames as a `ScreenCast` portal client, so
every capture it performs passes through the portal's consent and session
mediation.

## Status: stage 0 probe + protocol core

The two halves of Miracast come from different places, so a machine can have one
without the other:

| Half | Provided by |
|------|-------------|
| Radio: a Wi-Fi Display P2P authority | the system's Wi-Fi authority — on the reference configuration, **iwd**, with no NetworkManager and no `wpa_supplicant` |
| Protocol: the WFD session (RTSP M1–M7, MPEG2-TS over RTP, H.264) | this crate |

### The probe

```bash
cargo run -p tessera-cast -- probe
```

It reports, for each half, whether the capability was **observed**, **absent**,
or **unverified**, and nothing else. It is read-only: it never enables a P2P
adapter, starts a scan, or writes a property.

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

`cargo test -p tessera-cast` runs the real caster loop against the real sink loop
over a socketpair, so the entire M1–M7 exchange is verified with no hardware. For
a manual run, `tessera-cast sink --listen 127.0.0.1:7236` in one shell and
`tessera-cast handshake --host 127.0.0.1 --video-formats …` in another.

Four deliberate choices worth knowing:

- **Unprovable spec details are handled conservatively, not guessed.** `wfd::ie`
  names only the two bitfields this source sets and preserves every other bit and
  unknown subelement verbatim, so a wrong assumption about an unused bit cannot
  corrupt a peer's advertisement.
- **The session reacts instead of scripting M1–M7.** Sinks vary the order; a
  machine that insists on a sequence turns a working sink into a failure. The
  responder answers what it is asked and reports what it understood as effects.
- **`wfd_video_formats` is caller-supplied, never invented.** Its bitfield packs
  resolutions, micro-steps and profile/level codes; a wrong bit makes a sink
  reject the session with no distinguishable diagnostic, so the value is
  configuration plus a trace rather than a guess.
- **A parameter value containing CR, LF, or NUL is refused.** The body is
  line-based, so such a value would forge a further parameter — a source that
  refuses HDCP could be made to look as if it negotiated it.


### The handshake (`src/cast.rs`, `src/wfd/transport.rs`)

`handshake` drives M1–M7 over a real byte stream and traces it. No media is sent,
so nothing here claims to cast — but this is the first part of the pipeline that
can be proven end to end, because a scripted sink is enough:

| Module | Provenance | Evidence |
|--------|-----------|----------|
| `wfd::transport` | RTSP framing over `Read + Write` | split reads, pipelined bursts, clean vs partial EOF, a 9 KiB body across 4 KiB reads, socket timeout mapped to its own error |
| `cast` | the bounded handshake loop | completes, times out, closes, overruns its budget, traces both directions, and labels the opening `OPTIONS` as ours |

## Not yet implemented

Group formation and the WFD session against a sink, VA-API encoding, PipeWire
capture, and the caster's own `cast` command — plus **HDCP**, audio, and UIBC,
which are recorded non-goals. The outstanding verification matrix is ADR-0192 §8.

Nothing here may be described as working before a real sink shows frames.

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
