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

The whole handshake can be exercised **without any sink device** by pointing the
caster at the scripted sink in the same binary:

```bash
cargo run -p tessera-cast -- sink --listen 127.0.0.1:7236      # in one shell
cargo run -p tessera-cast -- handshake --host 127.0.0.1 \
    --port 7236 --video-formats "00 00 02 10"                  # in another
```

That prints the caster's full trace and the sink's report of what it received.
`cargo test -p tessera-cast` drives the same two sides against each other over a
socketpair in milliseconds.

Miracast has two halves that come from different places, so a machine can have
one without the other:

| Half | Provided by |
|------|-------------|
| Radio — a Wi-Fi Display P2P authority | the system's Wi-Fi authority; on the reference configuration **iwd**, with no NetworkManager and no `wpa_supplicant` |
| Protocol — the WFD session (RTSP M1–M7, MPEG2-TS over RTP, H.264) | this crate |

## Commands

```text
tessera-cast probe
tessera-cast discover [--settle <seconds>] [--port <port>]
tessera-cast help
```

With no subcommand, `probe` runs.

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
run's list. `--port` is the RTP port advertised to sinks.

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

## `handshake`

```bash
cargo run -p tessera-cast -- handshake (--host <sink-ip> | --peer <needle>) \
    --video-formats <value> [--port 7236] [--settle 4] [--timeout 10]
```

A **diagnostic**, not a caster: it opens an RTSP connection to the sink, drives the
WFD M1–M7 exchange through the protocol core, and prints the full traced exchange.
**No media is sent**, so a sink will not show frames — this proves the negotiation
only.

`--video-formats` is required and never defaulted. The value is a packed bitfield
(resolutions, micro-steps, profile/level), and a wrong one makes a sink reject the
session indistinguishably from a radio failure. Take it from a working caster's
exchange.

`--host` is the sink's RTSP address. Alternatively, `--peer` targets a discovered peer
by substring matching its name, MAC address, or D-Bus path. If the peer is not yet
connected, `handshake` triggers P2P group formation (`form_group` via iwd's
`net.connman.iwd.SimpleConfiguration.PushButton`), resolves its IP endpoint, and
proceeds with the handshake.

The run is bounded three ways, and each bound reports a different `outcome` line:

| Outcome | Meaning |
|---------|---------|
| `sink accepted the handshake` | M1–M7 completed; the sink will `SETUP` the given URL |
| `sink stopped answering` | The peer went silent before the socket timeout |
| `sink closed the connection` | The peer closed, with the message count |
| `sink tore the session down` | The peer sent `TEARDOWN` |
| `message budget exhausted` | The peer answered without ever concluding |

## `cast`

```bash
cargo run -p tessera-cast -- cast (--host <sink-ip> | --peer <needle>) \
    --video-formats <value> [--port 7236] [--rtp-port 19000] [--settle 4] \
    [--source desktop|pattern] [--fps 30] [--duration SECONDS] [--max-frames COUNT] [--timeout 10]
```

Streams live H.264 video to a sink. Unlike `handshake`, `cast` completes the M1–M7
negotiation, establishes the streaming session on `PLAY`, opens an ephemeral UDP
socket, and delivers MPEG-2 Transport Stream packets over RTP (RFC 2250, payload type
33) to the sink's RTP port.

`--source` selects the video source:
- `desktop` (default): Requests a monitor capture session from `org.freedesktop.portal.ScreenCast`,
  normalizes high-DPI desktop resolution to standard 1080p, and hardware-encodes frames via
  VA-API (`/dev/dri/renderD128`, Intel iHD `VAEntrypointEncSlice`). If portal consent is denied or
  fails, falls back cleanly to the test pattern.
- `pattern`: Directly streams the self-contained 1280x720 30fps SMPTE color bars H.264 test pattern
  with zero desktop or portal dependency.

During streaming:
- Emits PAT and PMT tables once per second to maintain transport stream sync.
- Services incoming RTSP control messages (such as keep-alive `GET_PARAMETER` or
  sink-initiated `TEARDOWN`) without disrupting frame pacing.
- Delivers video access units paced at the configured frame rate.
- On completion or operator interruption, issues a clean RTSP `TEARDOWN` to ensure
  the sink resets cleanly.

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

## Not yet implemented

Live hardware confirmation with a physical sink displaying frames, VA-API hardware
encoder integration from portal dmabufs (ADR-0192 Stage 1), and portal ScreenCast
virtual capture — plus **HDCP**, audio, and UIBC, which are recorded non-goals
([ADR-0192](../adr/0192-first-party-miracast-caster.md)).

HDCP-less streaming means DRM-protected playback cannot be cast, and the refusal
must be explicit so it never looks like a bug.

