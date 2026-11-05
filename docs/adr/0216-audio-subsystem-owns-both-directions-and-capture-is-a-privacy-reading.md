---
id: ADR-0216
title: "The Audio Subsystem Owns Both Directions, and Capture Is a Privacy Reading"
status: accepted
date: 2026-11-04
scope: core/audio, live system, host system, shell/control-center, protocol, cli
superseded_by: null
negative_knowledge: true
---

# 0216. The Audio Subsystem Owns Both Directions, and Capture Is a Privacy Reading

- Status: Accepted
- Date: 2026-11-04
- Deciders: Tessera Maintainers
- Extends: [ADR-0215](0215-audio-output-selection-is-live-service-state.md) (playback routing and the handle discipline this mirrors), [ADR-0175](0175-bluetooth-peripheral-subsystem-and-bluez-bridge.md) (the subsystem shape), [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md) (the same shape, one direction earlier)
- Related: [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md) (the screen-capture indicator this is deliberately *not*), [ADR-0159](0159-package-boundaries-follow-capabilities.md) (why this is a module and not a crate)

---

## Context and Problem Statement

ADR-0215 gave the desktop a playback picker and explicitly deferred the input
side:

> "Add an input (source) selector at the same time. Deferred: microphones carry a
> different failure model (an app may hold the source exclusively, and privacy
> indicators are involved)."

Two problems accumulated behind that deferral.

1. **Audio had no home.** The PipeWire work lived in `host_system.rs`, a
   1,200-line file that also owns `brightnessctl`, battery sysfs, `rfkill`, and
   `systemctl`. Wireless and Bluetooth each have a directory with a backend
   trait, a live adapter, a mock, and captured-document tests; audio had
   functions. The one seam every test of the feature needs was the least
   reviewable part of the crate, and its mock doubled as the whole host's mock,
   so a test that wanted a capturing microphone had to reach into a struct shared
   with brightness and battery.

2. **The microphone was invisible in both directions.** A machine could not
   choose which microphone applications would use, could not mute one, and could
   not say whether anything was listening. Every other desktop on the platform
   answers at least the third: it is the only way a user learns that a
   background tab is holding the microphone open.

The tempting shortcut for the third is to count what the desktop itself
requested. That is wrong in the way that matters: the desktop's own bookkeeping
knows about sessions it started, and a browser tab, a Rust program, or a
sandboxed agent process are all invisible to it. The sound server, by contrast,
knows exactly what is linked.

## Decision Drivers

- **One authority per fact.** The sound server owns the graph, the device
  inventory, the default nodes, the mute flags, and the links. The desktop
  observes and names; it never models a second opinion.
- **A stable handle.** Anything a user or a script can keep must be the server's
  `node.name`, never a session-assigned number (`[INV-AUDIO-1]`, carried over).
- **Capture is a privacy reading, so it is observed, not inferred.** An
  indicator that reports what the desktop asked for is a promise, not a
  measurement.
- **Modules by default.** ADR-0159 retains a package only for a concrete
  dependency, build, shared capability, or delivery boundary. Audio has none: it
  links no client library, ships no binary, and talks to the server through two
  short-lived CLI probes.
- **Deterministic tests and preview.** A mock that accepts what the server
  rejects lets a test pass on behaviour the shipping path does not have.
- **One observation, one moment.** The inventory, both default markers, the
  levels, and the capture flag must describe the same instant of a graph that is
  changing under the desktop (`[INV-AUDIO-6]`, extended).

## Considered Options

- **Option A — Keep the audio code in `host_system.rs` and add the microphone
  there.** Smallest diff; leaves the one seam every audio test needs inside a
  1,200-line mixed-concern file, and keeps the microphone mock coupled to the
  brightness/battery mock.
- **Option B — A `crates/tessera-audio` package.** Maximum separation; adds a
  manifest, a public API surface, and a dependency-policy row for a crate that
  links nothing and ships nothing.
- **Option C — A `crates/tessera/src/audio/` subsystem module** that owns the
  backend trait, the `pw-dump` parse seam, the capture observation, and the mock,
  composed by `host_system` as one `AudioSnapshot` per probe.

## Decision Outcome

Chosen option: **Option C**, because it satisfies every driver at the cost of
none: the shape matches the two subsystems that already exist, the parse is
testable against captured documents, the mock is scriptable in isolation, and no
new package edge appears in `dependency-policy.toml`.

### 1. The subsystem and its one observation

`crates/tessera/src/audio/` holds `AudioBackend` (seven verbs: three per
direction plus the two inventory switches), `PipeWireBackend` (the shipping
adapter), `MockAudioBackend` (scripted, contract-faithful), and `AudioSnapshot`,
the single record a probe produces:

| Field | Meaning |
|-------|---------|
| `outputs`, `inputs` | The inventories, ordered by transport rank, then default, then name |
| `output_volume`, `input_volume` | The live default node's fader position in percent, or `None` |
| `output_muted`, `input_muted` | The live default node's mute as the server reports it |
| `capturing` | Whether a live link feeds any capture stream from an `Audio/Source` |

`ForkedStatus` carries the whole `AudioSnapshot` rather than five loose fields,
so a new reading cannot be silently dropped by the cheap `/sys` pass. Both
`host_system` probes copy it through one `apply_audio` function — the single
place the fields are wired, so the full and cheap passes cannot drift.

### 2. Capture without a mixer

`pw-dump` is read once per full probe. It already carries the node list, the
per-node `Props` volume and mute, the `default` metadata object **and** the live
link graph, so:

- **Volume** is recovered from `channelVolumes` by cube root. PipeWire stores
  cubed amplitude; a fader at 65% reports `0.2746`, and reading that back
  verbatim would make every fader disagree with `wpctl`.
- **Capture** is a link whose output end is an `Audio/Source` node. A sink's
  *monitor* feeding a capture stream is screen recording, not microphone use:
  conflating them would accuse a user of recording audio whenever they recorded a
  window with sound. A stream node with no live link is not recording either —
  the link is the observation.
- **Routing** is written into **both** keys of the `default` metadata object:
  `default.configured.audio.<kind>` (the stored preference `wpctl set-default`
  writes and WirePlumber restores from) and `default.audio.<kind>` (the live
  default new and followed streams attach to), preference first. ADR-0215 wrote
  only the `configured` key on the belief that WirePlumber re-elects from it.
  Measured against WirePlumber 0.5.14, it does **not**: the `configured` write
  left the live default and an already-running playback link exactly where they
  were, so a switch made through the shipping code moved nothing while the
  panel's marker — which reads the live key — never moved either. Both keys are
  written here, in that order, so the persisted choice already agrees with the
  live one when WirePlumber re-elects. The spelling is name-addressed end to
  end; a numeric id would need a resolve step that can go stale between calls.

### 3. The domain and wire vocabulary (additive)

`tessera-desktop` gains `AudioInputKind` (`BuiltIn`, `Headset`, `Bluetooth`,
`Usb`, `Other`) and `AudioInput` (`name`, `description`, `kind`, `is_default`),
plus four `SystemStatus` fields — `audio_inputs`, `microphone_volume`,
`microphone_muted`, `microphone_active` — and three `SystemAction` variants:
`SetAudioInput { name }`, `SetMicrophoneVolume { level }`,
`ToggleMicrophoneMute`. Every field carries `serde(default)`, so a peer that
predates the feature deserializes the same snapshot and needs no protocol bump.

`AudioNode` is the one trait that expresses the handle discipline, implemented by
both `AudioOutput` and `AudioInput`, so sinks and sources are validated by the
same function instead of by two that will eventually differ.

### 4. Chrome

The sound fader's disclosure opens a **Sound** page: the capture banner (when
anything is listening), the microphone fader, then the output list and — when
there is more than one — the input list. A dedicated microphone fader occupies
its own row in Quick Controls, labelled with the device in use or with
"Microphone in use" when capture outranks the device name. Both the banner and
the fader glyph are non-interactive readings.

### Invariants & Behavioral Boundaries

- **`[INV-AUDIO-1]` The handle is the server's node name, in both directions.**
  No node number, index, or ordinal appears in a `SystemAction`, a status field,
  or a CLI argument. Enforced by `audio_node_observed` (one generic validator)
  and by the transport bounds on both name-carrying variants.
- **`[INV-AUDIO-2]` Exactly one authority marks each default.** `is_default` is
  read from the server's `default` metadata object for that direction. A name
  that resolves to nothing marks nothing; chrome never fabricates a marker and
  never claims a switch optimistically.
- **`[INV-AUDIO-3]` The compositor writes no routing policy and no mute state it
  cannot read back.** It forwards the intent and the next probe reports the
  result.
- **`[INV-AUDIO-4]` Classification happens once, in the subsystem.** `tessera-shell`
  never sees `device.api`, `device.bus`, or node-name prefixes; an unrecognized
  transport is `Other` in both directions, never a guess.
- **`[INV-AUDIO-5]` An empty inventory means "no sound server answered".** It is
  never rendered as "no hardware", and `microphone_volume: None` is unknown, not
  silent. A fader pinned at zero would claim the machine has no microphone.
- **`[INV-AUDIO-6]` One fork, one snapshot.** Both inventories, both default
  markers, both levels, and the capture flag come from a single `pw-dump` per
  full probe, and the cheap `/sys` pass reuses them through one `apply_audio`
  call so the two passes cannot disagree.
- **`[INV-AUDIO-7]` Capture is observed, never inferred.** `microphone_active`
  is true exactly when the server's link graph feeds a capture stream from an
  `Audio/Source` node. It is never derived from a `SystemAction` the desktop
  dispatched, from a session the desktop started, or from chrome's own state.
  Enforced by the parse tests that distinguish a source link from a sink-monitor
  link and from an unlinked stream.

## Rejected Alternatives & Negative Knowledge

### A `tessera-audio` package
- **Why considered**: audio is a named domain with a real external dependency
  (PipeWire), and a package would make the boundary explicit.
- **Why rejected**: the dependency is a *process*, not a library. The crate would
  link nothing, ship nothing, and add a manifest, a public API surface, and a
  policy row — exactly the "package per responsibility" ADR-0159 rejects. The
  subsystem shape already provides the reviewability a package would have bought.

### A `tessera-peripheral` package for audio, Bluetooth, and input
- **Why considered**: the three are all "devices the user plugs in", and one
  package sounds like one boundary.
- **Why rejected**: their abstraction levels are incompatible. HID input is on
  the frame's critical path through `libinput`; Bluetooth is an asynchronous
  D-Bus session; audio is a graph owned by another daemon. A combined package
  would bundle `libinput`, BlueZ, and PipeWire behind one manifest and force
  `host_system`, `platform`, and the compositor to take all three or none.

### Deriving the capture indicator from the compositor's own capture streams
- **Why considered**: the desktop already counts `capture_streams` (ADR-0128),
  so the microphone indicator could reuse that plumbing and need no probe.
- **Why rejected**: it answers a different question. `capture_streams` counts
  *compositor-owned* offscreen streams, which is precisely the set that excludes
  a browser tab, an encoder, and a sandboxed agent. An indicator built on it
  would be silent during exactly the captures a user most needs to see, and the
  failure is invisible — a green indicator over a live microphone.

### Counting capture *stream nodes* instead of live links
- **Why considered**: a `Stream/Input/Audio` node in the dump looks like a
  recorder, and counting nodes is one filter.
- **Why rejected**: a stream node can exist unlinked (waiting for a target, or
  after its source vanished), and a *sink-monitor* capture is a stream node that
  is not using a microphone at all. The link is the observation; the node is a
  hypothesis.

### Reporting the raw `channelVolumes` value as the level
- **Why considered**: it is the number on the node, so it is "what the server
  says".
- **Why rejected**: it is cubed amplitude. A 65% fader would read back as 27%
  and the panel would disagree with `wpctl` on every device — the silent, subtle
  kind of wrong that makes users distrust the whole page.

### Writing only the persisted `configured` default key
- **Why considered**: it is the key `wpctl set-default` writes, so delegating to
  it looks like the most faithful way to honour the server's authority, and
  ADR-0215 shipped on that reasoning.
- **Why rejected**: measured against WirePlumber 0.5.14 it does not move
  anything. A `default.configured.audio.sink` write left the live `default`
  key and a running `pw-play` link untouched for ten seconds, and the same
  command via `wpctl set-default <id>` behaved identically. The desktop would
  report success, the panel would not move, and the user's sound would keep
  coming out of the old device — the worst of the three possible outcomes,
  because nothing fails loudly. The live key is now written too, after the
  preference.

### Assuming a level write clears the mute
- **Why considered**: setting a volume on a muted device obviously means "play
  at this level", and every desktop behaves that way, so it is easy to assume the
  server implements it.
- **Why rejected**: it does not. Measured against WirePlumber 0.5.14,
  `wpctl set-volume` on a muted sink leaves it `[MUTED]`, for sinks and sources
  alike. The runtime was already optimistically clearing the flag in its own
  snapshot, so the panel would have shown an unmuted fader and the machine would
  have produced no sound — a defect with no error and no visual clue. The level
  write now issues its own `set-mute 0`, and the mock models the whole contract
  rather than the server's raw behaviour.
- **What deliberately does not follow it**: a *volume step* (`StepVolume`, the
  media keys). Ambient input can be pressed by accident on a machine someone
  muted on purpose, so the step moves the level and leaves the mute, and the
  glyph explains the silence.

### Making the capture banner dismissible
- **Why considered**: a persistent band costs space on a small panel, and a user
  who knows what is recording might want it gone.
- **Why rejected**: an indicator a click can dismiss is a promise the desktop
  cannot keep. It reports the server's link graph; if the user wants it gone,
  the honest action is to stop the capture, which the banner's presence makes
  discoverable.

### Adding a per-application capture mixer in the same change
- **Why considered**: the same `pw-dump` carries per-stream nodes, so a
  per-application view is nearby.
- **Why rejected**: per-stream targets are the client's own choice, not the
  session's, and mixing the two authorities would make `microphone_active`
  ambiguous. Out of scope by construction, as ADR-0215 did for playback.

## Consequences

### Positive
- A user can choose the microphone, mute it, and see that something is listening
  — from the same page that chooses the output, with the same vocabulary.
- The capture reading is true for a browser tab, a recorder, and an agent session
  alike, because it reports the server's links and not the desktop's intentions.
- The audio seam is a subsystem with captured-document tests: the version-
  sensitive `pw-dump` shapes, the cubed-volume recovery, and the source-vs-sink-
  monitor distinction are all pinned without host hardware.
- `host_system.rs` loses the audio parse, the reaper, and the command plumbing;
  the mock host shrinks to the `/sys` half and delegates audio to a scriptable
  mock.

### Negative / Trade-offs
- The full probe still forks `pw-dump` once (~8 ms measured on the reference
  machine, every 30 s plus on demand). It now yields the capture flag and both
  inventories for the same fork, so the marginal cost of the input half is zero,
  but the capture indicator's latency is the full-probe cadence: a microphone
  opened between probes appears on the next one. The cheap `/sys` pass cannot
  shrink that without a second fork, which would be strictly worse.
- Classification is heuristic where the server does not say: an analog node is
  `BuiltIn`/`Headset` by its own name and description, and a USB device outranks
  the label. The categories are ordering and iconography only; no behaviour
  depends on them, and an unrecognized node is `Other`.
- Both inventories are snapshots. A device that disappears between probes is
  still listed until the next one; clicking it is refused by the server, which
  the panel then shows.
- The Quick Controls grid gains a row, so `MAIN_H` grows from 540 to 620 and the
  panel is taller on every machine, including one with no microphone. The row is
  conditional but the panel constant is not: sizing the panel to the content
  would move the panel's edge when a sound server starts. `MAIN_H` is also
  clamped to the display, so on a short output the grid is taller than its body
  regardless — which is why the grid scrolls, as every other page in this panel
  already did. Without that, the fourth fader row was painted outside the body
  and the control was simply unreachable.
- The three capture verbs required a `PROTOCOL_VERSION` bump to 33. The
  `SystemStatus` additions are additive in both directions, but a new variant of
  a tagged `SystemAction` enum is not: a server built before it cannot decode the
  request at all, so the failure would have surfaced as a transport
  deserialization error against a client that believed it could use the verbs.
  (ADR-0215 landed inside version 32 without that bump and carries the same
  latent problem for its own verb.)
