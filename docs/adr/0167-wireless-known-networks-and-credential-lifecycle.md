---
id: ADR-0167
title: "Wireless Known Networks, Credential Lifecycle, and Profile Autonomy"
status: accepted
date: 2026-09-28
scope: live system, wireless subsystem, control center, security
superseded_by: null
negative_knowledge: true
---

# 0167. Wireless Known Networks, Credential Lifecycle, and Profile Autonomy

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md)

---

## Context and Problem Statement

Following [ADR-0162](0162-wireless-network-subsystem-and-iwd-bridge.md), the system established an asynchronous wireless network management subsystem with an `iwd` D-Bus bridge, scan triggers, password prompts, and status reflection in the Control Center.

However, the lifecycle of saved wireless profiles remained incomplete:
1. **Inability to Forget Networks**: Once a network profile was stored by the wireless daemon (e.g. `/var/lib/iwd/*.psk`), the user had no mechanism to discard stale credentials or decommission untrusted access points. When an access point changed its pre-shared key (PSK) or authentication scheme, the station suffered repeated authentication handshake failures with no recovery path other than manual root-level terminal intervention.
2. **Uncontrolled Automatic Reconnection**: All saved networks implicitly attempted connection whenever in range. Users could not disable automatic connection for cellular tethering hotspots, metered networks, or noisy public access points without deleting the profile entirely.
3. **Missing Frequency & Diagnostic Visibility**: Scanned networks lacked frequency band classification (2.4 GHz vs. 5 GHz), preventing users from distinguishing between dual-band SSIDs in dense radio environments.

We require a first-class, daemon-neutral, and uncompromising architecture for managing the lifecycle of known network profiles, credential revocation, and connection autonomy.

---

## Decision

We expand the Wireless Network Subsystem with profile lifecycle primitives, native `KnownNetwork` D-Bus bindings, and inline controls in the Control Center.

### 1. Domain Model and Action Primitives (`tessera-desktop`)

We extend `WifiNetwork` with profile autonomy and physical radio band fields:
- `auto_connect: bool`: Indicates whether the daemon automatically associates with this known network when discovered in scans.
- `frequency_mhz: Option<u32>`: Center radio frequency (e.g. 2412 for 2.4 GHz, 5180 for 5 GHz), enabling clear band indicators.

We introduce two explicit live mutation actions in `SystemAction`:
- `ForgetWifi { ssid: String }`: Discards stored credentials and daemon configuration files for the targeted network. If the station is actively associated with this network, the connection is immediately torn down.
- `SetWifiAutoConnect { ssid: String, auto_connect: bool }`: Toggles the daemon's autonomous background connection policy for a saved profile without discarding stored credentials.

Both actions enforce validation invariants (`ssid.trim().is_empty()` rejected).

### 2. Backend Abstraction and iwd Bridge (`tessera`)

We expand the asynchronous `WirelessBackend` trait:
```rust
pub trait WirelessBackend: Send + Sync {
    // ...
    fn forget(&self, ssid: &str) -> Result<(), String>;
    fn set_auto_connect(&self, ssid: &str, auto_connect: bool) -> Result<(), String>;
}
```

- **`IwdBackend` Implementation**:
  - Probes and monitors `net.connman.iwd.KnownNetwork` interfaces via D-Bus `ObjectManager`.
  - Maps SSIDs to their corresponding known network object paths.
  - `forget`: Invokes `net.connman.iwd.KnownNetwork.Forget()`, which purges the cryptographic configuration file from the host filesystem and triggers an immediate status refresh.
  - `set_auto_connect`: Updates the D-Bus property `AutoConnect` on `net.connman.iwd.KnownNetwork` via `org.freedesktop.DBus.Properties.Set`.
- **`MockWirelessBackend`**:
  - Implements deterministic state mutations for CI and headless testing: `forget` clears saved flags, resets auto-connect, and tears down active associations when targeted.

### 3. Control Center Presentation and Direct Ergonomics (`tessera-shell`)

The expanded Wi-Fi detail view in the Control Center incorporates inline management for known networks:
- **Frequency Badges**: Displays `2.4 GHz` or `5 GHz` tags alongside the SSID when radio metadata is available.
- **Auto-Connect Toggle**: For saved networks, provides a dedicated `Auto` toggle pill allowing one-click policy updates without opening nested configuration dialogs.
- **Forget Network Affordance**: Saved networks expose a distinct `Forget` button. Activating it dispatches `SystemAction::ForgetWifi`, immediately revoking local credentials and updating UI state.
- **Click Routing Safeguards**: Child button clicks within the pressable network row consume the interaction event, preventing accidental re-connection triggers when modifying profile settings.

---

## Consequences

### Positive
- Users can seamlessly self-heal broken Wi-Fi connections after password rotations without dropping to the Linux CLI.
- Ephemeral credentials (hotels, airports, conferences) can be purged on demand, preserving credential hygiene.
- Mobile tethering and metered Wi-Fi networks can remain saved without unwanted autonomous connection.
- Architecture remains completely decoupled from external tooling (`nmcli`, `iw`, `wpa_cli`).

### Negative / Trade-offs
- Slight expansion of the D-Bus object cache footprint in the background wireless worker thread to track `KnownNetwork` paths.

---

## Rejected Alternatives & Negative Knowledge

- **Shell Out to `iwctl known-networks <ssid> forget`**:
  - *Rejected*: Process forking introduces command-injection risks, latency jitter on the compositor thread, and breaks sandboxed container deployments.
- **Direct File Manipulation in `/var/lib/iwd/*.psk`**:
  - *Rejected*: The display server and shell must never require root filesystem access to daemon state directories. Bypassing the daemon's IPC breaks in-memory caches and leads to race conditions.
- **Modal Confirmation Dialog for Every Forget Operation**:
  - *Rejected*: Creates unnecessary UI friction in modern quick settings. If a user forgets a network by accident, re-entering the passphrase upon next connection is trivial and non-destructive.
