---
id: ADR-0200
title: "Event-Driven Wireless Subsystem, D-Bus Signal Propagation, and Unconditional Station Association Telemetry"
status: accepted
date: 2026-11-01
scope: runtime/wireless, shell/hud, shell/control-center, desktop/system
superseded_by: null
negative_knowledge: true
---

# 0200. Event-Driven Wireless Subsystem, D-Bus Signal Propagation, and Unconditional Station Association Telemetry

- Status: Accepted
- Date: 2026-11-01
- Deciders: Tessera Maintainers
- Amends: [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md), [ADR-0190](0190-wireless-backends-are-selected-and-capability-declared.md)

---

## Context and Problem Statement

ADR-0162 introduced the asynchronous `WirelessBackend` abstraction and the native `iwd` D-Bus bridge. ADR-0190 enforced that unavailable wireless services must be honestly reported as `None` rather than fabricated with mock fixtures.

However, two architectural flaws and one presentation gate rendered the wireless subsystem unresponsive to real-world Wi-Fi changes:

1. **Static In-Memory Cache and Missing Event Propagation**:
   The native `IwdBackend` populated its internal cache only once at initialization (`open()`) and during direct user-initiated commands (`scan`, `connect`). The background worker thread in `WirelessHandle` drained command requests and read cached getters (`state()`, `active_ssid()`, `networks()`). Because the background loop had **neither D-Bus signal subscription nor transitional event polling**, any state transition occurring outside Tessera (e.g. host auto-connection on boot, AP roaming, external disconnects via `iwctl`, or signal quality shifts) was never perceived.
2. **Asynchronous Connection Completion Drop**:
   When `ConnectWifi` was dispatched from Control Center, `IwdBackend::connect` called `net.connman.iwd.Network.Connect` over D-Bus and immediately triggered `self.refresh()`. Because 802.11 association, 4-way WPA handshake, and DHCP negotiation take hundreds of milliseconds to several seconds, iwd reported `State = "connecting"` with no connected network at that exact instant. Once connection succeeded moments later, no event handler refreshed the state, permanently stranding the connection in `Connecting` with `active_ssid = None`.
3. **Spurious Presentation Gating in the HUD**:
   `tessera-shell`'s `wifi_label` displayed the connected SSID only if `status.network == NetworkState::Wifi`. On hosts with active virtual or secondary interfaces (e.g. `docker0`, `tailscale0`, `tun0`, bridge/veth interfaces) whose sysfs operstate was `up`, `detect_network()` classified the primary network state as `Wired`. This silenced the Wi-Fi label in the HUD chip even when the Wi-Fi station link was associated and active.

## Decision Drivers

- **Zero-Wakeup Event-Driven Lifecycle**: D-Bus signal subscriptions (`PropertiesChanged`, `InterfacesAdded`, `InterfacesRemoved`) must drive state reconciliation. CPU-waking periodic polling is rejected as power-inefficient and battery-draining.
- **Transitional Convergence Guarantee**: Asynchronous verbs (`connect`, `scan`) must actively resolve their in-flight state until reaching terminal states (`Connected`, `Disconnected`, or failure).
- **Authoritative Station Association**: When the station interface has an active associated SSID, the HUD and Control Center must display that association without gating on secondary Ethernet or virtual tunnel presence.

## Decision Outcome

### 1. Event-Driven D-Bus Signal Listener for `IwdBackend`

`IwdBackend` establishes an active D-Bus match rule for signals from `net.connman.iwd` (`PropertiesChanged` on `Station`/`Device`/`Network`, and `InterfacesAdded`/`InterfacesRemoved` on `ObjectManager`). When an event is signaled:
- The backend automatically re-queries managed objects and updates the shared cache.
- The `WirelessHandle` worker snapshot updates immediately, triggering compositor-side dirty damage and broadcasting `publish_system_status_parts`.

### 2. Active In-Flight Transition Resolution

In `WirelessHandle`:
- When link state is `WifiLinkState::Connecting` or `WifiLinkState::Scanning`, the worker thread continues to evaluate transitional state completion on subsequent wakeups until arriving at a terminal state (`Connected`, `Disconnected`, or idle).
- Added `WirelessCommand::Refresh` and `WirelessHandle::request_refresh()` to allow explicit out-of-cycle synchronization.

### 3. Unconditional Wi-Fi Telemetry in HUD & Reconcile

- In `tessera-shell`: `wifi_label` displays `status.wifi_ssid` whenever a non-empty SSID is present, eliminating the erroneous `network == NetworkState::Wifi` gate that allowed virtual or wired adapters to suppress wireless station telemetry.
- In `tessera`: When `wireless_snap.active_ssid` is present, the runtime reconciles `system_status.wifi_state = WifiLinkState::Connected` and aligns `system_status.network = NetworkState::Wifi`.

## Consequences

### Positive
- Wi-Fi state transitions (connecting, associating, disconnecting, switching networks, roaming) reflect immediately in both Control Center and the HUD bar.
- Fully event-driven via D-Bus signals; idle power consumption is zero without waking CPU cores.
- Works natively with minimal standalone `iwd` setups without requiring NetworkManager.
