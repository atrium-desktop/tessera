# Session Service Commands

`tessera` supervises `tessera-idle`, `tessera-networkd`, and `tessera-castd`,
and `tessera-idle` starts `tessera-lock`. Installed session binaries must be
siblings of `tessera`: the compositor resolves each one next to its own
executable, verifies ownership and mode, and refuses a replacement found on
`PATH`. A missing sibling is reported as an unavailable service rather than
silently substituted.

## Lock Screen

| Command or option | Result |
|-------------------|--------|
| `tessera-lock` | Lock the Wayland session connected by `WAYLAND_DISPLAY`. |
| `tessera-lock --ready-fd FD` | Write one byte after the compositor confirms that the secure frame was presented on every output. `FD` must be an inherited, writable descriptor. |
| `tessera-lock --help` | Print local usage without connecting to Wayland. |
| `tessera-lock --version` | Print the package version. |

The command exits with status `0` after authenticated unlock. A setup,
protocol, or rendering failure exits nonzero. An unavailable authentication
service leaves the secure lock active and displays an error. If a confirmed
lock client disappears, the compositor keeps the session fail-closed;
restarting a different locker cannot take over that lock.

## Idle Coordinator

| Command or option | Default | Result |
|-------------------|---------|--------|
| `tessera-idle` | — | Run one idle coordinator for the current session. |
| `--lock-now` | off | Ask the running coordinator to lock; start the sibling lock screen directly if the coordinator is unavailable. |
| `--dim-after SECONDS\|off` | `300` | Set or disable the backlight-dim stage. |
| `--lock-after SECONDS\|off` | `600` | Set or disable the automatic lock stage. |
| `--display-off-after SECONDS\|off` | `660` | Set or disable physical display power-off. |
| `--suspend-after SECONDS\|off` | `1800` | Set or disable the logind suspend request. |
| `--dim-percent 1..100` | `30` | Set the dimmed hardware-backlight percentage. |
| `--mode balanced\|awake\|secure` | `balanced` | Select the session power mode (ADR-0140): which stages stay armed. The running coordinator also accepts a live `MODE <name>` control datagram, which re-arms without a restart. |
| `--socket PATH` | `$XDG_RUNTIME_DIR/tessera.sock` | Select the compositor IPC socket used for output power. |
| `--control-socket PATH` | `$XDG_RUNTIME_DIR/tessera-idle.sock` | Select the owner-only datagram control socket. |
| `--no-logind` | off | Disable lock-before-sleep monitoring, the sleep delay inhibitor, and automatic suspend. |
| `--help` | — | Print local usage. |
| `--version` | — | Print the package version. |

Timeouts are seconds, must be strictly increasing when enabled, and cannot
exceed `604800`. Display-off and suspend require an enabled lock stage.
Invalid policy or a second live coordinator exits nonzero.

The compositor supplies validated options from `[idle]`; manual daemon
invocation is primarily for diagnostics. Nested sessions automatically
disable backlight dimming, physical output power, logind integration, and
suspend while retaining the lock stage.

See the [Configuration Reference](config.md#idle-and-locking) for persistent
policy, [How to Install and Verify the Lock Screen](../how-to/lock-screen.md)
for installation and PAM validation, and
[How to Configure Locking and Idle](../how-to/lock-and-idle.md) for the user
workflow.

## Network Service

| Command or option | Default | Result |
|-------------------|---------|--------|
| `tessera-networkd` | — | Run the out-of-process station-network service (ADR-0242). |
| `--socket PATH` | `$XDG_RUNTIME_DIR/tessera-networkd.sock` | Owner-only control socket. The service refuses a parent it does not own or one that is group/world-writable. |
| `--backend auto\|kestrel\|networkmanager\|iwd\|mock` | from `[network] backend` | Absolute backend selection; `mock` exists for previews and tests only. |
| `--help` | — | Print local usage. |

The compositor never opens NetworkManager, iwd, or Kestrel itself. It speaks
the versioned service protocol over the socket and renders the snapshot the
service publishes; without the service the Control Center reports the network
as unavailable. See the [Configuration Reference](config.md#network).

## Cast Service

| Command or option | Default | Result |
|-------------------|---------|--------|
| `tessera-castd` | — | Run the out-of-process Miracast service (ADR-0188, ADR-0192). |
| `--socket PATH` | `$XDG_RUNTIME_DIR/tessera-castd.sock` | Owner-only control socket; a non-socket path is never replaced. |
| `--video-formats FMT` | the hardware-validated 720p30 value | Advertised `wfd_video_formats`; an empty value refuses `Connect` with a named reason. |
| `--port`, `--rtp-port` | `7236`, `19000` | RTSP control port and RTP base port. |
| `--source desktop\|pattern` | from `[cast] source` | Capture source for a session. |
| `--opening sink\|source` | from `[cast] opening` | Which side opens the WFD exchange. |
| `--audio`, `--audio-source system\|microphone`, `--audio-bitrate KBPS` | from `[cast]` | Opt-in AAC-LC audio path. |
| `--fps`, `--settle`, `--timeout` | from `[cast]` | Session cadence and timeouts. |
| `--help` | — | Print local usage. |

The compositor starts and supervises the service; chrome never opens a radio,
spawns a caster, or kills a process by name. See the
[tessera-cast reference](tessera-cast.md#tessera-castd) for the wire protocol
and hardware validation.
