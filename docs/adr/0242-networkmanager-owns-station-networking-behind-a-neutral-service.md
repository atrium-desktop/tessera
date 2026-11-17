---
id: ADR-0242
title: "NetworkManager Owns Station Networking Behind a Neutral Out-of-Process Service"
status: accepted
date: 2026-11-17
scope: runtime/network, desktop/system, config, packaging, shell/control-center, architecture
superseded_by: null
negative_knowledge: true
---

# 0242. NetworkManager Owns Station Networking Behind a Neutral Out-of-Process Service

- Status: Accepted
- Date: 2026-11-17
- Deciders: Tessera Maintainers
- Amends: [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md), [ADR-0190](0190-wireless-backends-are-selected-and-capability-declared.md), [ADR-0191](0191-casting-p2p-belongs-to-the-system-wifi-authority.md), [ADR-0200](0200-event-driven-wireless-subsystem-and-station-lifecycle.md), [ADR-0207](0207-compositor-dbus-isolation-and-prebaked-cursor-strategy.md)

---

## Context and Problem Statement

Tessera's network control surface was built around one backend: the native iwd
bridge in `crates/tessera/src/wireless/iwd.rs`. ADR-0162 chose a daemon-neutral
`WirelessBackend` seam, and ADR-0190 made an unavailable service honest instead
of fabricating fixture networks. Both were right, and both stopped short of the
actual deployment question:

- **A NetworkManager host had no Wi-Fi control.** `spawn_auto` was literally
  "iwd or unavailable". On a machine whose network is owned by NetworkManager —
  the default on most distributions — the control center reported wireless
  unavailable even though the radio was working.
- **The seam had no second backend.** ADR-0190 deliberately deferred the
  capability/candidate framework because there was no second candidate. That
  deferral is now discharged: NetworkManager is the candidate, and the
  selection policy must be real rather than a hardcoded `if`.
- **The station bridge broke the process boundary.** ADR-0207 established that
  system D-Bus clients (NetworkManager, BlueZ, iwd, UDisks) must live in
  companion processes, never in the display server. The iwd bridge was the
  counterexample; adding NetworkManager in-process would have doubled it.
- **The abstraction was only Wi-Fi-shaped.** The compositor also needs the
  primary link's coarse state (`wifi`/`wired`/`offline`) and interface name,
  which it previously guessed from `/sys/class/net`. NetworkManager is the
  authority for that answer when it is present.

This ADR settles the station side completely: NetworkManager is the preferred
authority, iwd remains a supported fallback, both live behind one neutral
contract in one out-of-process service, and the compositor consumes only that
service's snapshot.

## Decision Drivers

- **One authority, one radio.** Do not start a second network manager merely
  because the preferred one is present but unhealthy (ADR-0191's conflict
  warning).
- **The compositor renders facts, never daemon state.** No NetworkManager
  interface name, object path, device-state integer, or security flag may cross
  the service boundary (`[INV-NET-DAEMON-NEUTRAL]`, ADR-0162).
- **A missing service is reported, never simulated** (ADR-0190,
  `[INV-ARCH-82]`).
- **The display server links no system D-Bus client for networking**
  (ADR-0207).
- **Backend choice is explicit and testable.** `auto` has one defined
  preference and one defined fallback; an explicit choice never silently
  becomes another backend.
- **No speculative network domain.** Wired profiles, VPNs, DNS, IP routing, and
  hotspot management have no consumer in the control center today; adding them
  would mint capabilities and a large domain model that nothing reads
  (ADR-0159, ADR-0190).

## Decision Outcome

Chosen option: **NetworkManager is the preferred station backend; iwd remains a
fallback; both are selected by `tessera-networkd` and exposed to the compositor
through a pure, neutral contract.**

### 1. `tessera-network`: the pure contract

A new package, `crates/tessera-network`, owns:

- `NetworkBackend`, the synchronous contract (`scan`, `connect`, `disconnect`,
  `set_enabled`, `forget`, `set_auto_connect`, `refresh`, `poll`, `snapshot`).
  Synchronous is deliberate: the service runs it on a worker thread.
- `NetworkSnapshot`, the complete neutral reading (availability, backend kind,
  primary `NetworkState`, interface, radio/link state, SSID, deduplicated
  networks, last error).
- `BackendChoice` (`auto` / `networkmanager` / `iwd`) and `BackendKind`.
- `MockNetworkBackend` for preview and headless tests.
- The framed JSON control protocol (`Request`/`Event`, protocol version 1).

It has no `zbus`, no NetworkManager, no iwd, and no socket I/O.

### 2. `tessera-network-system`: the service and the backends

A second package, `crates/tessera-network-system`, owns every system D-Bus
connection used for networking:

- `NetworkManagerBackend` reads `org.freedesktop.NetworkManager` device,
  access-point, active-connection, and settings objects and translates them
  once into `NetworkSnapshot`.
- `IwdBackend` is the relocated, unchanged iwd bridge (`net.connman.iwd`)
  behind the same contract.
- `tessera-networkd` is the long-lived process. It selects a backend, runs it
  on a worker thread, publishes snapshots to subscribers over
  `$XDG_RUNTIME_DIR/tessera-networkd.sock` (mode 0600), and accepts the
  station verbs.
- `DaemonBackend` parses `auto`, `networkmanager`, `iwd`, and the
  preview/test-only `mock`.

The compositor never links this package.

### 3. The compositor supervises and consumes

`crates/tessera/src/network.rs` replaces the old in-process wireless module.
`NetworkHandle` supervises `tessera-networkd` exactly as `CastHandle` supervises
`tessera-castd`, reconnects after a transport failure, and applies pushed
`NetworkSnapshot`s. `SystemAction` and the control center are unchanged: they
still speak `SetWifi`, `ScanWifi`, `ConnectWifi`, `DisconnectWifi`,
`ForgetWifi`, `SetWifiAutoConnect`, and `SystemStatus` still carries the same
fields.

The runtime merges the snapshot through
`runtime/system.rs::apply_network_snapshot`. While the service answers, it is
authoritative for `wifi_*`, `network`, and `network_interface`. When it does not,
the Wi-Fi fields are cleared and the `/sys/class/net` probe remains the coarse
fallback for a wired link, exactly as before.

### 4. Selection policy

`[network] backend` is a new TOML section:

```toml
[network]
backend = "auto" # "networkmanager" | "iwd"
```

- `auto` prefers NetworkManager. It falls through to iwd **only when
  NetworkManager is not running**. Neither service is D-Bus-activated: a
  stopped daemon is an operator decision, and starting one on a host where the
  other owns the radio would put two managers on one radio.
- `networkmanager` and `iwd` are absolute. A missing named service yields an
  inert unavailable backend with a named reason; there is no silent fallback.
- `mock` is not representable in TOML. It is reachable only through the
  explicit preview path (`--backend mock`).
- Selection is read at session start. Changing `[network] backend` takes effect
  on the next session; the daemon does not hot-swap radio managers.

### 5. NetworkManager translation rules

- A single Wi-Fi device is selected (`DeviceType == 2`); multiple radios are
  future work, not silently merged.
- Access points are deduplicated by SSID; the strongest signal supplies the
  bars/frequency, and a connected AP keeps the row connected.
- `Flags | WpaFlags | RsnFlags` map to `Open`, `WpaPsk`, or `Enterprise`. A
  privacy-only/WEP AP is `WpaPsk` (secured), never `Open`.
- Signal strength maps from NM's 0–100 `Strength` to the model's 0–4 bars.
- Saved profiles come from `Settings.ListConnections` / `GetSettings`
  (`802-11-wireless.ssid`, `connection.autoconnect`). `forget` and
  `set_auto_connect` affect **every** profile matching the SSID.
- `connect` activates an existing profile; if none exists it builds an
  in-memory settings dictionary (`802-11-wireless`, optional
  `802-11-wireless-security` with `key-mgmt = "wpa-psk"`, IPv4/IPv6 `auto`) and
  calls `AddAndActivateConnection`.
- Enterprise/802.1X connect is refused with a named error: the neutral
  `connect(ssid, passphrase)` verb carries no identity or EAP method, and
  guessing one would be worse than refusing.
- A failed refresh clears the Wi-Fi reading and reports `available = false`
  with the error. Last-known state is never presented as current.
- The primary connection type supplies the coarse `NetworkState`; a Wi-Fi link
  that is up is `Wifi`, a non-Wi-Fi primary is `Wired`, otherwise `Offline`.

### 6. P2P stays a separate role, with one authority

`tessera-castd` (and the `tessera-cast` CLI) remains the owner of the Wi-Fi
Display P2P role; `tessera-networkd` owns the **station** role only. This is a
role split, not two authorities for the same role.

`radio::open_system` selects the P2P authority from what is already running:

- NetworkManager wins when it exposes a ready `wifi-p2p` device
  (`DeviceType == 30`, state at least disconnected). That device exists only
  when NetworkManager drives wpa_supplicant for the adapter, so using it cannot
  conflict with iwd's P2P role.
- Otherwise iwd is used when it already owns `net.connman.iwd` and has a P2P
  service manager. NetworkManager with its **iwd backend** exposes no
  `wifi-p2p` device and lands here, as does an iwd-only host.

Neither the probe nor this selection may D-Bus-activate a daemon: the first
method call to a name with an activation file starts it, and an activated iwd
fights wpa_supplicant (observed; runbook L-025). Both probes ask
`NameHasOwner` first, so a stopped daemon is reported absent. The station
service follows the same rule; it reports a stopped NetworkManager or iwd as
unavailable instead of starting it.

The NetworkManager radio maps P2P operations onto
`Device.WifiP2P.StartFind`/`Peers`, `WifiP2PPeer.WfdIEs`, and
`AddAndActivateConnection2` with a **volatile, user-owned** `wifi-p2p` profile
(`connection.permissions = user:<login>`, so only `settings.modify.own` is
needed). The source's WFD IEs travel in `wifi-p2p.wfd-ies`; the profile is
deactivated on every session exit path and never saved. A group this session
did not form is left alone.

Discovery is verified against the reference TV on a NetworkManager +
wpa_supplicant host. Group formation and the session are covered by the fake
NetworkManager suites (`tests/integration/cast.rs`,
`tests/e2e/cast_networkmanager.rs`) but still need a physical-TV run.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-136] Network State Is Service-Owned`: The compositor MUST derive
  `wifi_*`, `network`, and `network_interface` from `NetworkSnapshot` while the
  service answers, and MUST NOT open NetworkManager or iwd itself. Enforced by
  dependency policy (`tessera` has no edge to `tessera-network-system`) and the
  absence of `zbus` in `tessera-network` / `crates/tessera/src/network.rs`.
- `[INV-ARCH-137] Backend Selection Never Falls Back Silently`: `auto` may try
  iwd only when NetworkManager is not running; an explicit choice yields
  unavailable, never the other backend. Enforced by
  `selection::tests::auto_falls_through_to_iwd_only_when_networkmanager_is_not_running`
  and `selection::tests::explicit_backends_never_fall_back`.
- `[INV-ARCH-82] No Fabricated Network State` (extended): an unavailable
  backend publishes empty reads with a reason; the mock is never a production
  fallback. Enforced by
  `backend::tests::test_inv_arch_82_unavailable_backend_fabricates_nothing` and
  the `DaemonBackend::Mock`-only preview path.

## Rejected Alternatives & Negative Knowledge

### Add a NetworkManager backend in-process behind `dbus-system`

- **Why considered**: It mirrors the existing iwd module and is the smallest
  code change.
- **Why rejected**: It repeats the ADR-0207 violation for the very daemon that
  ADR-0207 names. A NetworkManager crash or bus stall would enter the display
  server process, and the compositor would link a system D-Bus client on the
  network path.

### Keep iwd and add NetworkManager as a fallback in `spawn_auto`

- **Why considered**: It avoids moving code and keeps the existing module.
- **Why rejected**: On a NetworkManager host, iwd is either absent (so the
  fallback is the only path) or running as NetworkManager's backend (so two
  clients compete). Selection must prefer the system authority and be explicit.

### Prefer iwd and fall back to NetworkManager

- **Why considered**: The reference machine for casting runs iwd.
- **Why rejected**: The deployment direction is NetworkManager. It is also the
  authority that owns wired links, profiles, and connectivity; preferring a
  station-only daemon would make the common host the fallback.

### Shell out to `nmcli`

- **Why considered**: It is the shortest implementation and easy to debug.
- **Why rejected**: ADR-0162 already rejected process spawning from the
  compositor for blocking, credential, and hygiene reasons. It would also make
  the parsing seam a CLI text format instead of typed D-Bus values.

### Build the full network domain now (wired profiles, VPN, DNS, routes, AP mode)

- **Why considered**: NetworkManager exposes all of it, and "network" sounds
  like the complete stack.
- **Why rejected**: The control center has no consumer for any of those verbs.
  It would mint a large model and UI-less capabilities, the exact speculative
  generality ADR-0159/ADR-0190 refuse. Wired links are still reported through
  the coarse `NetworkState`.

### Drive P2P through `tessera-networkd` or a second station manager

- **Why considered**: One network process would appear simpler, and the
  station service already owns the system bus.
- **Why rejected**: Station and P2P are different roles with different
  lifetimes: P2P activation is per cast session and must be torn down with it,
  while station state is long-lived. `tessera-networkd` must not become a
  general "network daemon" that also holds the caster's media-session state.
  The `P2pRadio` port keeps the role boundary and lets iwd and NetworkManager
  implement it without a second station authority.

## Consequences

### Positive

- NetworkManager hosts get native Wi-Fi scan, connect, disconnect, forget, and
  auto-connect control, with real signal strength and frequency.
- On NetworkManager + wpa_supplicant hosts, Miracast discovery and group
  formation work through NetworkManager's `wifi-p2p` device; iwd-only hosts
  keep the hardware-validated reference path.
- The compositor no longer opens a system D-Bus client for networking; the
  process boundary matches ADR-0207.
- Backend choice is one explicit, testable rule, and the neutral contract now
  has two production implementations.
- A failed service reports unavailable and a reason instead of stale or
  fabricated state.
- The primary link state (`Wifi`/`Wired`/`Offline`) comes from the actual
  network authority when one answers.

### Negative / Trade-offs

- A new daemon binary (`tessera-networkd`) must be installed beside the
  compositor; packaging and the trusted-sibling lookup must include it.
- NetworkManager authorization is a deployment concern: a session that cannot
  modify settings gets a named refusal (polkit), which the service surfaces
  rather than retries.
- `auto` cannot use iwd when NetworkManager is present but broken. That is
  intentional: the failure is reported instead of creating a second authority.
- Enterprise Wi-Fi connect is unavailable through the current neutral verb.
  That remains an explicit capability boundary, not a silent degradation.
- NetworkManager P2P group formation has fake-daemon coverage but not yet a
  physical-TV run; the iwd P2P path is the hardware-validated one. The caster
  selects one authority and never treats station connectivity as evidence
  that P2P is available.
- `[network] backend` is read at session start; changing it requires a new
  session.
- `tessera` still carries `zbus` through other edges — Bluetooth under
  `dbus-system`, the cast control protocol's library, and the session-bus tray.
  Those are separate subsystems with their own isolation decisions; the network
  path no longer contributes a system-D-Bus edge or a daemon dependency.

## Verification

- `cargo nextest run -p tessera-network -p tessera-network-system`
  covers the contract, mock, protocol framing, daemon lifecycle, selection
  policy, NetworkManager translation, security mapping, profile commands, and
  the unavailable-state invariant.
- `cargo nextest run -p tessera-config` covers `[network] backend` parsing and
  rejection of `mock`/unknown values.
- `cargo nextest run -p tessera --lib` covers the runtime merge and supervisor
  event handling.
- `cargo nextest run -p tessera-cast` covers WFD IE decoding, the
  NetworkManager radio's pure mapping, and the session's group-teardown guard.
- `cargo nextest run -p tessera-tests -E 'test(cast)'` drives discovery, group
  formation, RTSP, RTP media, and teardown against a fake NetworkManager on a
  private bus.
- `cargo nextest run -p tessera-network-system --run-ignored only -E 'test(live_)'`
  verifies a real NetworkManager snapshot and scan; the matching ignored test
  in `tessera-tests` verifies real P2P discovery. Neither starts a stopped
  daemon.
- `cargo run -p xtask -- check-boundaries` proves the compositor has no
  dependency on `tessera-network-system`.
- `cargo run -p xtask -- check-adrs` validates this record's index and lineage.
- Manual hardware validation: on a NetworkManager host, scan, connect to WPA2
  and open networks, disconnect, forget, toggle the radio, and observe a wired
  primary link; on an iwd-only host, repeat with `backend = "iwd"` and
  `backend = "auto"`. For casting, run `tessera-cast probe`, then discover and
  form a group with the reference TV on both an iwd-only host and a
  NetworkManager + wpa_supplicant host.
