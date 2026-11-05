# tessera-cast Reference

`tessera-cast` is the Miracast (Wi-Fi Display) caster
([ADR-0192](../adr/0192-first-party-miracast-caster.md)). It is an ordinary
process, not part of the compositor: it obtains frames as a `ScreenCast` portal
client, so every capture passes through the portal's consent and session
mediation, and it holds **no compositor IPC scope**.

## No install, no reboot

Every command runs straight from the source tree:

```bash
cargo run -p tessera-cast -- <command>
```

There is no package to install, no systemd unit, no D-Bus activation, and no
reboot. The Wi-Fi authority it talks to (iwd) is a service that is already
running, and the P2P state it changes is restored on exit. A fix therefore costs
a compile, not a machine cycle.

The whole session can be exercised **without any sink device** with the scripted
sink client in the same binary:

```bash
cargo run -p tessera-cast -- cast --source pattern \
    --video-formats "00 00 02 10" --max-frames 30            # in one shell
cargo run -p tessera-cast -- sink --connect 127.0.0.1:7236 --play  # in another
```

The source listens; the sink connects and sends `OPTIONS` first. That prints the
source's full trace and the sink's report of what it received.
`cargo test -p tessera-cast` drives the same two sides against each other over a
socketpair in milliseconds.

Miracast has two halves that come from different places, so a machine can have
one without the other:

| Half | Provided by |
|------|-------------|
| Radio — a Wi-Fi Display P2P authority | the system's Wi-Fi authority; on the reference configuration **iwd**, with no NetworkManager and no `wpa_supplicant` |
| Protocol — the WFD session (RTSP M1–M6 + SETUP/PLAY, MPEG2-TS over RTP, H.264) | this crate |

## Commands

```text
tessera-cast probe
tessera-cast discover [--settle <seconds>] [--port <port>]
tessera-cast doctor [--settle <seconds>] [--port <port>]
tessera-cast cast --video-formats <value> [options]
tessera-cast handshake --video-formats <value> [options]
tessera-cast sink [--connect <addr>] [--rtp-port <port>] [--play]
tessera-cast formats "<value>"
tessera-cast help
```

With no subcommand, `probe` runs. **The source is the RTSP server**: `cast` and
`handshake` listen on the control port (7236 by default) and the sink connects to
them. Neither command ever dials the sink.

### `probe`

Reports each half separately and exits with the stage-gate status. It is strictly
read-only: it never enables a P2P adapter, never starts a scan, and never writes
a property. Enabling P2P is a later stage's action, not a diagnostic side
effect.

Every line is one of three verdicts, and the distinction is load-bearing
(`[INV-ARCH-83]`):

| Verdict | Meaning |
|---------|---------|
| observed | The capability was seen. |
| absent | The capability is conclusively missing. |
| unverified | The probe could not look — a permission wall, a missing tool, an unsupported interface. **Never** reported as available. |

What it inspects:

- **Radio:** `net.connman.iwd.p2p.ServiceManager` (the WFD registration point)
  and `net.connman.iwd.p2p.Device` facts (`Enabled`,
  `AvailableConnections`) on the first adapter.
- **Pipeline:** a VA-API H.264 `VAEntrypointEncSlice` entry point (via
  `vainfo`), a PipeWire daemon socket in `XDG_RUNTIME_DIR`, and the session
  bus's `org.freedesktop.portal.Desktop`.
- **Inbound RTSP:** whether the host firewall accepts tcp/7236 from the P2P
  interface. A dropping input policy is a definite blocker, because the sink's
  connection is discarded before any protocol runs.

A definite absence in either half outranks an unknown in the other, because an
absence is actionable and an unknown is not.

| Exit status | Meaning |
|-------------|---------|
| `0` | Both halves present. |
| `1` | A half is definitively absent. |
| `2` | Indeterminate — something could not be inspected. |

`2` is never a success. A gate that treats it as one is using the tool wrong.

## `discover`

An **action**, unlike `probe`: it enables the adapter's P2P role, registers this
machine as a Wi-Fi Display **source**, requests discovery, and lists the peers it
found with the reason each one is or is not castable. It restores the P2P switch
and unregisters on exit, so a diagnostic leaves the radio as it found it.

`--settle` is how long discovery is given to accumulate peers before they are
read; discovery is asynchronous, so reading immediately reports the previous
run's list. `--port` is the **source's RTSP control port** advertised to sinks
(default 7236), not the RTP port.

Registration is **source-only** on purpose: iwd refuses to connect a dual-role
local device to a dual-role peer, and most sinks advertise both roles.

| Exit status | Meaning |
|-------------|---------|
| `0` | At least one castable sink. |
| `1` | The run failed; the message names why (for example `not authorized`). |
| `3` | The run worked but found no castable sink — open the sink's Screen Mirroring mode. |

### Verified D-Bus behaviour

Measured against live iwd on the reference machine, because the documented shape
and the daemon's shape differ:

- `RegisterDisplayService(a{sv})` returns **nothing**.
- `UnregisterDisplayService` takes **no arguments** — the registration belongs to
  the connection.
- Unknown dictionary keys are rejected with `InvalidArguments`: `Source`, `Sink`,
  `Port`, `HasUIBC`, and `HasContentProtection` are accepted; `HasAudio` is
  refused whatever its type. The caster therefore attempts the richer
  advertisement and degrades to `Source`/`Sink`/`Port`, logging the degradation
  instead of failing.

## `doctor`

```bash
cargo run -p tessera-cast -- doctor [--settle 8] [--port 7236]
```

One evidence report for a hardware session: the probe's half-by-half verdict,
the **inbound firewall** verdict with the exact ingress rule when blocked, and a
live discovery pass with every peer's castability and reason. When a castable
sink is found it prints the exact `cast` command to run next.

It exits `0` only when the probe reports no blocker, `2` when something could not
be inspected, and `1` otherwise — so a support script can branch without parsing
text.

## `handshake`

```bash
cargo run -p tessera-cast -- handshake --video-formats <value> \
    [--peer <needle>] [--bind 0.0.0.0] [--port 7236] [--settle 4] [--timeout 60]
```

A **diagnostic**, not a caster: it binds the source's control port, registers the
source with iwd, optionally forms a P2P group with `--peer`, waits for the sink
to connect, completes the WFD exchange (M1–M6, with the `SETUP` trigger in M5 on
the source-driven path), and prints the full traced exchange. **No media is
sent**, so a sink will not show frames — this proves the negotiation only. The
sink is the RTSP client and sends the first `OPTIONS`; the source must not
pre-empt it.

`--video-formats` is required on the CLI and never invented. The value is a packed bitfield
(resolutions, micro-steps, profile/level), and a wrong one makes a sink reject the
session indistinguishably from a radio failure. Take it from a working caster's
exchange.

`--peer` targets a discovered peer by substring matching its name, MAC address,
or D-Bus path. If the peer is not yet connected, `handshake` triggers P2P group
formation (`form_group` via iwd's
`net.connman.iwd.SimpleConfiguration.PushButton`). The sink's IP is not needed:
it arrives with the accepted connection.

The run is bounded three ways, and each bound reports a different `outcome` line:

| Outcome | Meaning |
|---------|---------|
| `sink accepted the handshake` | The exchange completed; the sink will `SETUP` the given URL |
| `sink stopped answering` | The peer went silent before the socket timeout |
| `sink closed the connection` | The peer closed, with the message count |
| `sink tore the session down` | The peer sent `TEARDOWN` |
| `message budget exhausted` | The peer answered without ever concluding |

### The firewall is part of the session

Because the sink dials the source, the host's input policy must accept tcp/7236
from the P2P interface. `probe` reports this as a blocker and both commands warn
with the exact rule, for example:

```bash
sudo nft add rule inet filter input iifname "wlan0-p2p*" accept
```

A dropping input policy silently discards the sink's SYN; the failure looks
exactly like a sink that never answered, which is the misdiagnosis this check
exists to prevent.

## `cast`

```bash
cargo run -p tessera-cast -- cast --video-formats <value> \
    [--peer <needle>] [--bind 0.0.0.0] [--port 7236] [--rtp-port 19000] \
    [--source desktop|pattern] [--opening sink|source] [--fps 30] \
    [--audio] [--audio-source system|microphone] [--audio-bitrate 128] \
    [--duration SECONDS] [--max-frames COUNT] [--timeout 60]
```

Serves a WFD session and streams live H.264. It binds the control port **before**
discovery, registers itself with that port, forms the P2P group with `--peer`,
waits for the sink to connect, completes the exchange (M1–M6; the source-driven
path puts the `SETUP` trigger in M5), answers the sink's `SETUP` with a
`Transport` header, and on `PLAY` delivers MPEG-2 Transport Stream packets over
RTP (RFC 2250, payload type 33) to the sink's negotiated port.

`--source` selects the video source:
- `desktop` (default): Requests a monitor capture session from `org.freedesktop.portal.ScreenCast`,
  normalizes high-DPI desktop resolution to standard 1080p, and hardware-encodes frames via
  VA-API (`/dev/dri/renderD128`, Intel iHD `VAEntrypointEncSlice`). If portal consent is denied or
  fails, falls back cleanly to the test pattern.
- `pattern`: Directly streams the self-contained 1280x720 30fps SMPTE color bars H.264 test pattern
  with zero desktop or portal dependency.

During streaming:
- Emits PAT and PMT tables every 100 ms (and on the first frame) to maintain
  transport stream sync; the source's media sender is bound to the P2P local
  address and the advertised `server_port` (19002).
- Services incoming RTSP control messages (such as keep-alive `GET_PARAMETER` or
  sink-initiated `TEARDOWN`) without disrupting frame pacing.
- Delivers video access units paced at the configured frame rate, with RTP
  packets paced across the frame interval.
- Sends an RTCP SR/SDES every second from 19003 to the sink's 19001.
- Stops when the sink tears the session down; the sink owns `TEARDOWN`.

### Audio (opt-in)

Audio is off unless explicitly enabled. `--audio` captures the selected PipeWire
node, encodes AAC-LC 48 kHz stereo with ffmpeg, and interleaves it into the same
MPEG2-TS/RTP flow. The source echoes the sink's AAC value in M5; if the sink has
no AAC it stays video-only and emits `AudioUnavailable`.

- `--audio-source system` (default): the default sink's monitor, i.e. desktop
  output. The capture stream declares `stream.capture.sink=true`; without it
  PipeWire links the stream to the default *microphone*, because a sink is an
  output-direction node.
- `--audio-source microphone`: the default source.
- `--audio-bitrate 128`: AAC kbit/s, 32..=512.

The default sink must not be muted. The capture is opt-in because it is a
privacy decision. The Control Center uses the same settings from `[cast]`.
See [Miracast Source §7.5](../explanation/miracast-source.md#75-audio).

### Development diagnostics

`--ts-file <PATH>` and `--ts-seconds <SECONDS>` send a pre-muxed MPEG-TS file
through the real RTSP/P2P/RTP/RTCP path. They exist to A/B a known-good muxer
against the built-in one, not as a product media source. `TESSERA_CAST_DUMP_TS`
and `TESSERA_CAST_DUMP_AUDIO` write the live TS/AAC for one session; they are
bring-up diagnostics, not stable configuration.

## `sink`

```bash
cargo run -p tessera-cast -- sink [--connect <addr>] [--rtp-port <port>] [--play]
```

A scripted **sink client**, for exercising a source with no television: it
connects to `--connect` (default `127.0.0.1:7236`), sends `OPTIONS` first, asks
for the source's parameters, requests a session, and with `--play` continues
through `SETUP`/`PLAY` and optionally receives RTP.

## `formats`

```bash
cargo run -p tessera-cast -- formats "<wfd_video_formats value>"
```

Decodes a value into its tokens with offset, hex and bit listings, and prints the
field name this build **assumes** at each position — marked `declared
(unverified)`.

It is a check, not a constructor:

- the value round-trips byte-exactly (lower-cased, single-spaced); the tool never
  rewrites what it was given;
- a token whose width disagrees with the declared field is reported as *"does not
  match the assumed layout"* rather than given that field's name;
- a token past the end of the schema is reported as an *unrecognised position*
  rather than borrowing a name;
- a `none` token is a literal, and is reported as such — not as a width mismatch;
- declared fields with no token in this value are counted at the end.

This is how a real sink's advertisement gets checked in one place instead of
guessed in several: paste it, and any place where the assumed layout is wrong
shows up immediately. `FIELD_SCHEMA` is the single table to correct.

## `tessera-castd`

The long-lived out-of-process service (ADR-0188, ADR-0192). The compositor
starts and supervises it; the control center renders the `CastStatus` snapshots
it pushes. It owns discovery and sessions, so chrome never opens iwd, never
spawns a caster, and never kills a process by name.

```bash
tessera-castd [--socket PATH] [--video-formats FMT] [--port 7236] \
    [--rtp-port 19000] [--settle 4] [--timeout 60] [--fps 30] \
    [--source desktop|pattern] [--opening sink|source] \
    [--audio] [--audio-source system|microphone] [--audio-bitrate 128]
```

- Socket: `$XDG_RUNTIME_DIR/tessera-castd.sock`, mode `0600` inside the user's
  runtime directory. It refuses to replace a non-socket path.
- `--video-formats` defaults to the hardware-validated 720p30 value; an empty
  value makes the service refuse `Connect` with a named reason rather than
  inventing a bitfield.
- Discovery runs only while a surface asked for it. Starting a session stops
  discovery first and waits for the radio to be released, so two iwd clients do
  not fight over P2P.
- Wire format: a 4-byte big-endian length followed by JSON, versioned in the
  protocol. Requests: `Hello`, `Subscribe`, `StartDiscovery`, `StopDiscovery`,
  `Connect { sink }`, `Disconnect { session }`, `Shutdown`. Events:
  `HelloAck`, `Status { status }`, `Error { message }`.

The compositor passes the `[cast]` configuration (`video_formats`, `source`,
`opening`, `audio`, `audio_source`, `audio_bitrate_kbps`, `fps`, ports, settle,
timeout) when it launches the service; `--video-formats` remains for manual runs.
See [`config.md`](config.md#cast).

## Hardware validation

The reference Skyworth A3F completed the exchange end-to-end: the source drove
M1–M6, the sink answered with `SETUP`, the source replied with `Transport` and
`Session`, the sink sent `PLAY`, and the source delivered **300 frames /
4,784,680 bytes** at 720p30. **The TV displayed the test pattern** (color bars,
circles, squares). The three requirements this sink exposed are:

1. `wfd_trigger_method: SETUP` must be in the **M5** parameter body; a separate
   source `SETUP` request (M7) is ignored and the sink closes.
2. The `SETUP` 200 response must include `Transport` (echoing `client_port` and
   adding `server_port`); without it the sink closes after one frame.
3. RTP must be sent from the advertised `server_port` (19002) and the P2P local
   address. An ephemeral source port produced no display even though the same
   TS/H.264 decoded locally.

The `tessera-castd` control path (`StartDiscovery` → `Connect`) reached the
`Streaming` state against the same TV. The source sends RTCP sender reports; the
reference TV does not send receiver reports. Audio is implemented as an opt-in
path (PipeWire → AAC-LC → same TS/RTP); the TV plays it, with a few remaining
all-IDR-pattern clicks. The video encoding and the TS muxer/SDT are still
separate suspects; desktop capture and the compositor UI click remain the
product checks.

HDCP and UIBC are recorded non-goals. Audio is opt-in and implemented; see
[Miracast Source](../explanation/miracast-source.md#75-audio). HDCP-less
streaming means DRM-protected playback cannot be cast, and the refusal must be
explicit so it never looks like a bug.

