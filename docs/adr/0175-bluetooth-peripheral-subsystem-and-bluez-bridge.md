---
id: ADR-0175
title: "Bluetooth Peripheral Subsystem, BlueZ Bridge, and HUD/Control-Center Parity"
status: accepted
date: 2026-09-29
scope: core/system, core/bluetooth, shell/hud, shell/control-center, desktop/domain
superseded_by: null
negative_knowledge: true
---

# 0175. Bluetooth Peripheral Subsystem, BlueZ Bridge, and HUD/Control-Center Parity

- Status: Accepted
- Date: 2026-09-29
- Deciders: Tessera Maintainers & Shell Architects
- Consulted: Compositor Runtime & Optics Graphics Team
- Amends: [ADR-0060](0060-statusbar-system-controls-and-live-system-ipc.md), [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md), [ADR-0115](0115-command-panel-desktop-behavior-scope.md)
- Extends: [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md), [ADR-0169](0169-control-center-bento-grid-and-mpris-absorption.md)

---

## Context and Problem Statement

Wi-Fi and Bluetooth were handled asymmetrically, and Bluetooth was the poor
relation on every surface:

1. **No device model.** `SystemStatus` carried only
   `bluetooth_enabled: Option<bool>`; there was no peripheral list, no link
   state, and no way to see, connect, or forget a device.
2. **Wrong control layer.** `SystemAction::SetBluetooth` shelled out to
   `rfkill block|unblock bluetooth`, and `detect_bluetooth_radio` read
   `/sys/class/rfkill/*/type`.
   *This was not the compositor-blocking defect ADR-0162 cited for Wi-Fi:*
   `spawn_host_command` is fire-and-forget (`.spawn()` plus a reaper thread,
   never `.output()`), so the fork never blocked the main loop — an earlier
   draft of this ADR claimed otherwise and was wrong. The real defects were
   that rfkill manipulates a **soft-block below the daemon**, so it cannot
   power a BlueZ adapter, pair, connect, or list a device; and the shell-out
   offered **no mockable seam**, so no deterministic CI path existed. Wi-Fi
   had already moved to its daemon (iwd `Powered`); Bluetooth had not.
3. **Uninformative HUD.** The Bluetooth chip printed the localized *On/Off*
   word next to the glyph. The word restated what the glyph already said and
   displaced the only genuinely useful string: the name of the connected
   device (which the Wi-Fi chip, by contrast, already showed).
4. **Bare quick-control tile.** The Bluetooth tile was a toggle with an
   On/Off subtitle and no expansion, while the Wi-Fi tile expanded into a
   scan/connect/disconnect/forget view.
5. **Crowded status row.** Every HUD cell was separated by a 2px gap equal to
   the icon↔label gap *inside* a cell, so distinct status groups (network,
   Bluetooth, speaker, battery) ran together with no grouping rhythm.

The host exposes Bluetooth through **BlueZ** (`org.bluez`) over the system
D-Bus, the exact structural counterpart of iwd. The subsystem gap — not the
lack of a daemon — was the defect.

## Decision Drivers

- **Parity, not novelty.** Bluetooth should read and control like Wi-Fi: a
  daemon-neutral subsystem, a per-tick snapshot, additive protocol fields, and
  an expandable control-center view.
- **Non-blocking.** No D-Bus call, fork, or blocking wait on the compositor
  main or render thread (`[INV-NET-NONBLOCK]`, reused).
- **Daemon-neutral presentation.** No object path, interface name, or BlueZ
  artifact may escape the subsystem (`[INV-NET-DAEMON-NEUTRAL]`, reused).
- **Honest absence.** A host without BlueZ or without an adapter must report
  *unavailable*, never a fabricated radio or device list.
- **Deterministic testing and preview.** A mock backend drives headless tests
  and `TESSERA_PREVIEW` without touching host hardware.

## Considered Options

- **Option A — Fork `bluetoothctl` / keep `rfkill`:** extend the existing
  process-spawn path to list and connect devices by parsing CLI output.
- **Option B — A new BlueZ subsystem (`crates/tessera/src/bluetooth`)** that
  mirrors `wireless`: a `BluetoothBackend` trait, a zbus-blocking `BluezBackend`,
  a `MockBluetoothBackend`, and one worker thread with a snapshot.
- **Option C — Presentation-only:** keep `bluetooth_enabled`, add the device
  name to the HUD from nowhere, and leave the subsystem for later.

## Decision Outcome

Chosen option: **Option B**, because it is the only one that satisfies every
driver at once — it reuses the wireless subsystem's proven shape (mpsc command
lane, `Arc<Mutex<Snapshot>>`, plain `std::thread` worker, 500 ms
`recv_timeout`), needs no new dependency (`zbus` is already a workspace
dependency), and keeps the unsafe/blocking surface confined to one worker.

### 1. Domain Models and Protocol Contract (`tessera-desktop`)

Additive, daemon-neutral types in `tessera_desktop::system` (each new
`SystemStatus` field carries `#[cfg_attr(feature = "serde", serde(default))]`):

- `BluetoothLinkState`: `Disabled`, `Unavailable`, `Idle`, `Scanning`,
  `Connected`. Deliberately **no `Connecting`**: `WifiLinkState` can reach it
  because iwd publishes a `Connecting` state string, and BlueZ's `Device1`
  publishes only a boolean `Connected` (verified against the live bus) — an
  `Connecting` variant here would be a state nothing could ever report.
- `BluetoothKind`: `Other`, `Audio`, `Input`, `Phone`, `Computer`, `Imaging`,
  `Wearable` — mapped from BlueZ's `Icon`/`Class` inside the backend.
- `BluetoothDevice`: `address` (identity), `name`, `kind`, `connected`,
  `paired`, `trusted`.
- `SystemStatus.bluetooth_state`, `SystemStatus.bluetooth_devices`.
- `SystemAction`: `ScanBluetooth`, `PairBluetooth { address }`,
  `ConnectBluetooth { address }`, `DisconnectBluetooth { address }`,
  `ForgetBluetooth { address }`, alongside the existing `SetBluetooth`.

### 2. Bluetooth Subsystem (`tessera`)

`crates/tessera/src/bluetooth/{mod,bluez,mock,tests}.rs`, structurally mirroring
`crate::wireless`:

- `BluetoothBackend` trait: `set_enabled`, `scan`, `pair`, `connect`,
  `disconnect`, `forget`, `poll`, `enabled`, `state`, `devices`.
- `BluezBackend`: a blocking zbus `Connection::system()`, one
  `GetManagedObjects` refresh that projects `org.bluez.Adapter1` and
  `org.bluez.Device1` into the cache. Scans are **bounded** (`DISCOVERY_WINDOW`
  = 20 s): `StartDiscovery` is paired with a `StopDiscovery` on expiry, because
  BlueZ latches `Discovering` until asked to stop and an unbounded scan is a
  battery and privacy hazard. `poll` refreshes on a 2 s gate (and while
  scanning), so it is safe to call on every worker wakeup.
- `MockBluetoothBackend`: deterministic fixtures (a paired headset, a paired
  keyboard, an in-range earbud), scriptable state, and a scan that settles on
  the next `poll`.
- `UnavailableBackend`: the `spawn_auto` fallback when BlueZ is absent —
  `enabled()` returns `None` and every mutation is refused.
- `BluetoothHandle`: `spawn_with_backend`, `spawn_auto`, and typed dispatch
  methods; the snapshot is read per tick without blocking.

The runtime (`iteration.rs`) reconciles `bluetooth_enabled`/`bluetooth_state`/
`bluetooth_devices` from the snapshot every tick, exactly as it does the Wi-Fi
fields, and publishes through the shared `publish_system_status_parts` path.
`apply_system_action` routes the new actions to the handle;
`SetBluetooth` no longer shells out to `rfkill`: powering the adapter is a
BlueZ `Powered` write, exactly as Wi-Fi writes iwd `Powered`.

### 3. HUD Chips (`tessera-shell`)

- The Bluetooth cell draws a **themed on/off glyph** (`bluetooth-active-symbolic`
  / `bluetooth-disabled-symbolic`, with a runtime-registered Bluetooth rune as
  the vector fallback because lens ships no Bluetooth `Icon`) and, when a link
  is up, the **connected peripheral's name**. The on/off *word* is gone: the
  glyph carries the state, and the label slot names the device.
- Cell gutters are widened so each status reads as its own group: the
  inter-cell `CELL_GAP` is now 10 px while the intra-cell icon↔label gap stays
  ~4 px, so an icon always sits nearer its own label than the next cell's glyph.

### 4. Control Center (`tessera-shell`)

The Bluetooth tile becomes expandable (chevron), mirroring Wi-Fi. Its body is
`render_bluetooth_detail_view`: a Back/Scan/power header plus a scrollable
device list with per-row **Connect** (row click), **Disconnect** (connected),
**Pair** (in-range, unpaired), and **Forget** (paired) actions. The Wi-Fi and
Bluetooth detail views are mutually exclusive, and `Escape` peels the open
detail view before it closes the panel.

### Invariants & Behavioral Boundaries

- `[INV-BT-NONBLOCK]`: No synchronous D-Bus call, fork, or blocking wait on the
  compositor main or render thread; all BlueZ I/O runs on the subsystem worker.
- `[INV-BT-DAEMON-NEUTRAL]`: Presentation and core models never expose an
  `org.bluez` object path, interface name, or BlueZ-specific state string.
- `[INV-BT-HONEST-ABSENCE]`: A missing service or adapter reports `Unavailable`
  with an empty device list; the subsystem never fabricates a radio or a device.
- `[INV-BT-BOUNDED-DISCOVERY]`: Every `StartDiscovery` is paired with a bounded
  stop; discovery can never latch on indefinitely.
- `[INV-BT-ADDITIVE]`: New `SystemStatus` fields default on a peer that predates
  them, so no protocol version bump is required.

## Rejected Alternatives & Negative Knowledge

### Option A — Fork `bluetoothctl` / keep `rfkill`
- **Why considered**: it is what the code already did; it is one line per call
  and needs no D-Bus knowledge.
- **Why rejected**: it targets the wrong layer. `rfkill block/unblock` flips a
  soft-block *below* BlueZ — it cannot power an adapter that BlueZ reports as
  unpowered, cannot pair, connect, or enumerate a device, and gives no error
  back to the caller about whether the daemon agreed. It also has no mockable
  seam, so the control surface stays untestable in CI.
- **Correction to an earlier draft of this ADR**: the first draft rejected this
  option as a *compositor-blocking* defect, echoing ADR-0162. That was false.
  `spawn_host_command` calls `.spawn()` and hands the child to a reaper thread;
  it never calls `.output()` and never waits. The fork was cheap and
  non-blocking. Recording the wrong reason here would let a future reader
  "fix" a problem that does not exist, or distrust the real one.

### Option C — Presentation-only
- **Why considered**: smallest diff; the HUD problem is cosmetic.
- **Why rejected**: it cannot be honest. With no backend there is no device
  name to show, no scanner to populate a list, and no way to connect — the
  panel would render an empty view or a fabricated one. It also leaves
  `SetBluetooth` forking, so the compositor-blocking defect remains. Partial
  parity is worse than either full parity or a clearly-labeled absence.

### Mirroring the wireless mock as the fallback
- **Why considered**: `WirelessHandle::spawn_auto` falls back to
  `MockWirelessBackend`, so the symmetric choice is a `MockBluetoothBackend`
  fallback.
- **Why rejected**: seeding fixture *networks* looks like a scan list; seeding
  fixture *devices* looks like real hardware and invites a user to try to
  connect to a phantom headset. The fallback is an honest, inert
  `UnavailableBackend` instead. Cleanup (control-center scratch render) uses a
  throwaway probe rather than shipping mock devices into a real session.

### Keeping the HUD On/Off word
- **Why considered**: it is localized and unambiguous.
- **Why rejected**: the glyph already conveys on/off, and the word consumed the
  one cell slot where the device name belongs. Removing it bought variable-width
  labels for the connected device at no information cost.

## Consequences

### Positive
- Bluetooth reaches full Wi-Fi parity: discovery, pairing, connection, and
  forgetting, all native and non-blocking, in one panel.
- The HUD answers the question users actually ask — *which device is
  connected* — while the glyph answers *is the radio on*.
- The status row regains grouping rhythm across every chip.
- New subsystem, models, and actions are covered by deterministic mock tests
  and additive protocol tests; `TESSERA_PREVIEW` seeds a connected device.

### Negative / Trade-offs
- A second D-Bus worker thread and a second backend trait to maintain; the
  duplication is deliberate (different daemons, different object models) but it
  is duplication.
- BlueZ device paths are derived from the adapter path plus the address
  (`dev_` + underscores), so the subsystem assumes standard BlueZ naming.
- Pairing policy (auto-`Trust` after `Pair`) is opinionated; a host that wants
  untrusted pairing would need a follow-up.
- The bounded discovery window is a fixed 20 s; a long manual scan re-arms it.
