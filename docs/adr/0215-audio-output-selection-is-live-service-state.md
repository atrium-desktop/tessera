---
id: ADR-0215
title: "Audio Output Selection Is Live Service State, Named by the Server"
status: accepted
date: 2026-11-04
scope: live system, host system, shell/control-center, protocol, cli
superseded_by: null
negative_knowledge: true
---

# 0215. Audio Output Selection Is Live Service State, Named by the Server

- Status: Accepted
- Date: 2026-11-04
- Deciders: Tessera Maintainers
- Extends: [ADR-0060](0060-statusbar-system-controls-and-live-system-ipc.md) (live-system control surface), [ADR-0168](0168-standardized-keyboard-backlight-subsystem.md) (the host-probe + typed-action shape this reuses)
- Related: [ADR-0140](0140-session-power-modes.md) (the additive `SystemStatus` evolution precedent)

---

## Context and Problem Statement

A machine with a Bluetooth headset connected, wired headphones plugged in, and
built-in speakers exposed has three audio outputs, and only one of them receives
playback at a time. PipeWire/WirePlumber picks that one with a policy:
`priority.session`, a user's stored `default.configured.*`, and a
"move to the new device" behavior that treats a freshly connected headset as an
upgrade.

That policy is right most of the time and wrong exactly when the user is
demonstrating something to the room: with a headset connected, the sound goes to
the headset and the people in front of the screen hear nothing. The user's
intent — "play this out loud, here" — is not expressible through any surface the
desktop had. Volume was a single fader against `@DEFAULT_AUDIO_SINK@`; nothing
named the sink.

The obvious shortcut is the misleading one. `wpctl status` prints a human tree
with node numbers, and those numbers look like identities. They are not: they are
assigned at server start and change on every restart, so a remembered number is a
promise the system cannot keep.

## Decision Drivers

- **The sound server owns routing; the compositor must not model it.** Where
  playback goes is PipeWire's decision, refined by WirePlumber's policy. A second
  opinion in the desktop would fight it, and the loser would be whichever updated
  last.
- **An identity that survives restart.** Any handle a user or a script can keep
  must be the server's stable node name, never a session-assigned number.
- **One snapshot, one shape.** The panel, IPC, and CLI all need the same
  inventory and the same default marker. Three queries would eventually disagree.
- **Additive on the wire.** Peers that predate this feature must keep working
  without a protocol bump; `SystemStatus` has evolved that way before.

## Considered Options

- **Store a "preferred output" in the desktop configuration.** Rejected: it would
  be a second authority that the sound server does not consult. A stored
  preference that does not move playback is worse than no preference, because the
  UI would claim a state the machine is not in.
- **Enumerate outputs from `wpctl status` text.** Rejected: parsing a
  human-formatted tree is positional and locale-sensitive, and it yields the
  unstable node numbers as the only handle.
- **Expose `wpctl set-default <id>` directly.** Rejected: the id must be resolved
  by a second call, and between resolving and applying it the server may have
  renumbered. The metadata write is name-addressed end to end.
- **Optimistically move the default marker on click.** Rejected: a stream may pin
  its own target, and a Bluetooth output may refuse to become default. Chrome
  reports what the server says it did, on the next snapshot.
- **Add an input (source) selector at the same time.** Deferred: microphones carry
  a different failure model (an app may hold the source exclusively, and privacy
  indicators are involved). Shipping both in one step would blur which of the two
  the invariants below are about.

## Decision Outcome

Chosen option: **observe the server, and name the output the server names.**

The live-system vocabulary gains two additive types in `tessera-desktop`:

| Type | Meaning |
|------|---------|
| `AudioOutputKind` | Daemon-neutral transport: `Speaker`, `Wired`, `Hdmi`, `Bluetooth`, `Usb`, `Other` |
| `AudioOutput` | `name` (server node name), `description`, `kind`, `is_default` |

`SystemStatus::audio_outputs` carries the inventory, sorted by transport rank and
then by default and name, so the list is stable across probes.
`SystemAction::SetAudioOutput { name }` is the intent. The host backend resolves
and applies it; the runtime never guesses the outcome.

### Invariants & Behavioral Boundaries

- **`[INV-AUDIO-1]` The output handle is the server's node name.** No `u64` node
  id, no index, and no ordinal appears in a `SystemAction`, a status field, or a
  CLI argument. Node numbers are session-local and are never persisted or
  accepted from a peer.
- **`[INV-AUDIO-2]` Exactly one authority marks the default.** `is_default` is
  read from the server's `default` metadata object. Chrome draws the marker from
  the snapshot and never from a cached click; a refused switch leaves the marker
  where the server left it. The switch reading is raised when the probe *observes*
  the marker move, never at dispatch: the write is a spawn the compositor does
  not wait on.
- **`[INV-AUDIO-2b]` A name is validated before the write, not after.** The sound
  server does not reject an unresolvable name: `pw-metadata` exits 0 and
  WirePlumber persists it as the stored default, evicting the user's real
  preference. The write therefore cannot be validated after the fact and must be
  refused before the spawn, against the last observed inventory. The cost is a
  retryable refusal for a device that appeared after that probe.
- **`[INV-AUDIO-3]` The compositor writes no routing policy.** The host backend
  forwards the intent to the sound server's own selection mechanism. It does not
  reimplement priority, does not remember a preference, and does not mute or
  unlink anything itself.
- **`[INV-AUDIO-4]` Presentation classifies transports in one place.** The
  daemon-neutral `AudioOutputKind` is derived in the host probe from the server's
  own transport vocabulary. `tessera-shell` never sees `device.api` strings or
  node-name prefixes; an unrecognized transport is `Other`, never a guess.
- **`[INV-AUDIO-5]` An empty inventory means "no sound server".** It is never
  rendered as "no hardware". The output disclosure is shown only when there are
  at least two outputs: with none there is nothing to pick, and with one the page
  would list the device already playing.
- **`[INV-AUDIO-6]` One fork, one snapshot.** The inventory and its default marker
  come from a single `pw-dump` parse per full probe, and the cheap `/sys` pass
  reuses them through `ForkedStatus` so the picker cannot flicker between probes.

## Rejected Alternatives & Negative Knowledge

### A desktop-owned "preferred output" setting
- **Why considered**: it would let the desktop re-assert the user's choice after
  a headset connects, which is the exact scenario in the problem statement.
- **Why rejected**: the sound server would still be the thing that routes, and it
  has no idea a preference exists. Re-asserting on every probe is a fight, not a
  configuration: it would override a later deliberate switch made by another
  client (`pavucontrol`, a `pactl` script, the server's own autoswitch).

### Parsing `wpctl status`
- **Why considered**: it is the command every tutorial uses, and it is already
  present on every machine that has PipeWire.
- **Why rejected**: the output is a formatted tree whose columns are padding, its
  only identity is the volatile node number, and its labels are localized
  descriptions. A parse of it is a parse of the presentation layer.

### `wpctl set-default <id>` behind a name lookup
- **Why considered**: it is the documented CLI for exactly this intent, and it
  delegates the policy entirely to WirePlumber.
- **Why rejected**: it is id-addressed, so it needs a resolve step whose result
  can go stale, and the resolve step itself (`wpctl inspect <name>`) is another
  fork on the latency path. Writing the name into the server's `default`
  metadata is the same operation the tool performs, addressed by the identity the
  user actually chose.

### An optimistic default marker
- **Why considered**: the click would feel instant, and the probe is only a few
  hundred milliseconds behind.
- **Why rejected**: it is a claim about a machine state that may not have been
  reached. A pinned stream keeps playing where it is; a Bluetooth profile switch
  can refuse the move. The panel's whole job here is to show the server's answer.

### A per-application volume/routing mixer in the same change
- **Why considered**: the same metadata and the same nodes carry per-stream
  routing.
- **Why rejected**: per-stream targets are a different authority (the client's own
  choice, not the session's), and mixing the two would make it ambiguous what
  `is_default` means. Out of scope by construction.

## Consequences

### Positive
- A user demonstrating to a room switches to the speakers in two clicks from the
  control center, and the machine keeps the headset connected for later.
- Scripts get the same capability through `tessera system audio-output <name>`,
  with names they can discover from `tessera system status`.
- The panel, IPC, and CLI read one inventory, so they cannot disagree about which
  device is playing.

### Negative / Trade-offs
- The probe now forks `pw-dump` (~13 ms measured on the reference machine, once
  per full probe every 30 s plus on demand), instead of one more `wpctl` call per
  output. The cheap pass stays fork-free by reusing `ForkedStatus`, so the
  cadence cost is unchanged.
- Transport classification is heuristic where the server does not say: an analog
  node is `Speaker`/`Wired`/`Hdmi` by its own name and description, because that
  is where "which jack" lives. The categories are ordering and iconography only —
  no behavior depends on them, and an unrecognized node is `Other`.
- The inventory is a snapshot. A device that disappears between probes is still
  listed until the next one, and a device that appeared after the last probe is
  not yet selectable. Both are the same honest limit: the name is validated
  before the write, so the answer is a retryable refusal rather than a write the
  server would persist but never honour.
- **Inactive HDMI outputs are listed and selectable.** On a host whose analog
  codec exposes several HDMI pins, every pin has a sink node regardless of
  whether a monitor is attached, so the picker offers outputs that may be
  digitally silent. The sound server routes to them happily; the failure is the
  display's, not the graph's. Filtering on `node.state == "running"` would hide
  the idle built-in speaker too, so a filter needs a real availability signal
  (a connected-port query) that this change does not carry.
