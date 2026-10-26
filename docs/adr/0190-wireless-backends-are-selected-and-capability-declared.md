---
id: ADR-0190
title: "Wireless Backends Are Selected and Capability-Declared, Never Fabricated"
status: accepted
date: 2026-10-28
scope: runtime/wireless, desktop/system
superseded_by: null
negative_knowledge: true
---

# 0190. Wireless Backends Are Selected and Capability-Declared, Never Fabricated

- Status: Accepted
- Date: 2026-10-28
- Deciders: Tessera Maintainers
- Amends: [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md)

---

## Context and Problem Statement

The wireless subsystem already has the daemon-neutral seam the design calls
for: a `WirelessBackend` trait (`scan`/`connect`/`disconnect`/`set_enabled`/
`forget`/`set_auto_connect`/`state`/`networks`/`active_ssid`), a `WirelessHandle`
that runs the backend on a worker thread and publishes a `WirelessSnapshot`, a
`MockWirelessBackend` for preview and tests, and a backend-neutral vocabulary
(`WifiLinkState`, `WifiNetwork`, `WifiSecurity`) that lives in
`tessera-desktop::system` rather than in any daemon's terms.

Two things were missing, and one of them is a defect:

1. **Production fabricated device state.** `WirelessHandle::spawn_auto` fell back
   to `MockWirelessBackend` when the native iwd bridge did not open. The mock
   ships three fixture networks (`Home-WiFi-5G`, …), so a machine with no iwd —
   for example one using NetworkManager — showed a **plausible-looking list of
   networks that do not exist**. The Bluetooth subsystem had already rejected
   exactly this pattern ([ADR-0175](0175-bluetooth-peripheral-subsystem-and-bluez-bridge.md),
   "Mirroring the wireless mock as the fallback", rejected in favour of an inert
   `UnavailableBackend`); wireless was the inconsistency.
2. **No availability signal.** The trait exposed no way to say "no service
   backs me", so the compositor could not distinguish "wireless unavailable"
   from "disconnected with no networks found" — even though
   `SystemStatus::wifi_enabled: Option<bool>` (`None` = service unavailable)
   already models exactly that distinction.

This also matters for the casting work ([ADR-0188](0188-out-of-process-casting-and-discovery-service.md)):
the pending iwd-vs-NetworkManager/wpa_supplicant decision is a *backend
selection* decision, and it cannot be made honestly while selection is a
hardcoded `iwd-else-mock`.

## Decision Drivers

- A missing service must be reported, never simulated. Fabricated device state
  misleads the user and any consuming surface.
- Backend choice must be explicit and capability-gated, so a future
  NetworkManager/wpa_supplicant backend (and a Wi-Fi Direct role for casting)
  is a selection change, not a rewrite of consumers.
- One honest fallback convention across subsystems: wireless must match the
  Bluetooth decision already recorded.

## Considered Options

1. **Add an availability signal and an inert `UnavailableWirelessBackend`;
   production selects only daemon-backed candidates.**
2. **Keep the mock fallback.**
3. **Introduce a full capability/candidate-selection framework now, including
   Wi-Fi Direct (P2P) verbs.**
4. **Converge the two subsystems' "no service" types into one shared type.**

## Decision Outcome

Chosen option: **Option 1.**

### 1. `WirelessBackend::available()` and `UnavailableWirelessBackend`

The trait gains `fn available(&self) -> bool`; `WirelessSnapshot` carries it.
`UnavailableWirelessBackend` refuses every verb and returns empty reads with
`available() == false`. `spawn_auto` selects the native iwd bridge when it
opens, and **otherwise the unavailable backend** — the mock is reachable only
through the explicit preview entry point (`is_preview` / `TESSERA_PREVIEW`),
exactly as the Bluetooth subsystem already does.

### 2. The compositor reports unavailability, not absence

When the snapshot is unavailable, the status reconcile clears
`wifi_enabled` to `None`, `wifi_ssid` to `None`, and `wifi_networks` to empty,
in both the host-poll merge and the real-time reconcile. `wifi_enabled = None`
is the established wire vocabulary for "the Wi-Fi radio service is
unavailable"; the UI already renders it, so no chrome change is required.

### 3. The richer selection/capability model is deferred, not pre-built

A `WirelessCapabilities` model (station now, Wi-Fi Direct P2P and AP later) and
a config-driven candidate list are the correct end state, but their consumer
does not exist: nothing in the tree performs P2P, and there is no second real
backend to select between. Building the framework now would mint a capability
that nothing reads and a config knob that selects nothing — the same
speculative generality this and every other capacity-adding decision has
refused. The seam they need is the `WirelessBackend` trait plus this ADR's
selection rule; both land *with* their consumer, before which
`spawn_auto`'s iwd-or-unavailable choice is complete.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-82] No Fabricated Wireless State`: The production wireless path
  MUST NOT expose a fixture backend's data. A backend that cannot reach a
  service reports `available() == false`, and the status then reports
  wireless unavailable with no networks. Enforced by `spawn_auto` never naming
  a mock, and verified by
  `test_inv_arch_82_unavailable_backend_fabricates_nothing`.

## Rejected Alternatives & Negative Knowledge

### 2. Keep the mock fallback
- **Why considered**: The mock keeps the HUD/control-center populated on
  machines without iwd, so the UI never looks broken.
- **Why rejected**: It populates them with networks that do not exist. A user
  cannot distinguish a real `Home-WiFi-5G` from a fixture, and `ConnectWifi`
  against it fails confusingly. This is the exact trade [ADR-0175](0175-bluetooth-peripheral-subsystem-and-bluez-bridge.md)
  already rejected for Bluetooth devices; wireless had simply not been
  brought into line.

### 3. Build the capability/candidate framework now, with P2P verbs
- **Why considered**: It is where the casting work is heading, and pre-building
  it would make the iwd/NetworkManager decision mechanical.
- **Why rejected**: A `p2p` capability with no reader and a candidate list with
  no second candidate are dead structure. The P2P seam, its capability, and its
  consumer (the caster, [ADR-0188](0188-out-of-process-casting-and-discovery-service.md))
  must land together; until then the trait's verbs are exactly the station
  operations that have consumers.

### 4. Converge the two subsystems' "no service" types
- **Why considered**: `BluetoothBackend` and `WirelessBackend` model the same
  situation two ways — Bluetooth through `enabled() -> Option<bool>` plus a
  `BluetoothLinkState::Unavailable` state, wireless through `available()` plus
  `wifi_enabled = None`. One shared availability type would make them
  isomorphic and stop the two seams drifting.
- **Why rejected**: The difference is load-bearing, not accidental. Bluetooth's
  state enum describes the *adapter*, so an `Unavailable` variant belongs in
  it; `WifiLinkState` describes a *link* (`Disabled`…`Connected`) and has no
  adapter-unavailable concept, so availability is a capability flag that the
  status maps onto the existing `wifi_enabled: Option<bool>`. Both are honest,
  and both are tested (`unavailable_backend_is_honest_and_inert`,
  `test_inv_arch_82_unavailable_backend_fabricates_nothing`). A shared type
  would be cosmetic and would pull two independent domain vocabularies together
  with no consumer. What the subsystems share is the *rule* — no production
  path may surface fixture data — not a type ([ADR-0159](0159-package-boundaries-follow-capabilities.md)).

## Consequences

### Positive
- Wireless matches the Bluetooth fallback rule: a missing service is reported,
  never simulated.
- `wifi_enabled = None` now carries a real meaning end to end, so no chrome
  change was needed.
- Backend selection is one explicit, testable place, ready to gain a second
  candidate.

### Negative / Trade-offs
- On a machine without iwd the control center shows wireless as unavailable
  rather than with a (fake) network list. This is the intended behaviour and a
  user-visible change, so it belongs in release notes.
- The richer capability model and a config-selectable backend remain future
  work, gated on a real second backend and the caster's P2P consumer.
