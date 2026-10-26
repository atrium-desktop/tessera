---
id: ADR-0191
title: "Casting P2P Belongs to the System Wi-Fi Authority; the Caster Is a Portal Client"
status: accepted
date: 2026-10-29
scope: runtime/wireless, protocol, docs
superseded_by: null
negative_knowledge: true
---

# 0191. Casting P2P Belongs to the System Wi-Fi Authority; the Caster Is a Portal Client

- Status: Accepted
- Date: 2026-10-29
- Deciders: Tessera Maintainers
- Amends: [ADR-0188](0188-out-of-process-casting-and-discovery-service.md), [ADR-0190](0190-wireless-backends-are-selected-and-capability-declared.md)
- Amended by: [ADR-0192](0192-first-party-miracast-caster.md) — the radio authority is whatever already owns the radio; NetworkManager is not required. The reference configuration stays on iwd, and the WFD session is implemented by a first-party caster.

---

## Context and Problem Statement

[ADR-0188](0188-out-of-process-casting-and-discovery-service.md) left three
questions open. Answering them together is necessary, because each answer
changes the others:

1. **Radio ownership.** Miracast discovery is Wi-Fi Direct (P2P), and Tessera
   already runs a native **iwd** station client
   (`crates/tessera/src/wireless/iwd.rs`). Who may own the radio's P2P role?
2. **The portal backend.** A `ScreenCast` `SourceType::VIRTUAL` implementation
   lives in the separate portal backend repository. What does the compositor owe
   it?
3. **A caster's scope.** Would an out-of-process caster need its own
   identity-bound IPC scope?

The ecosystem facts that decide it:

- The Linux Miracast stack speaks NetworkManager's `NMDeviceWifiP2P` (for
  example `gnome-network-displays`), and includes the Miracast protocol itself
  (RTSP negotiation, MPEG-TS/RTP, hardware encoding).
- NetworkManager's Wi-Fi Direct support requires the **wpa_supplicant** backend.
  A NetworkManager configured with iwd as its Wi-Fi backend has no working P2P
  path, because iwd does not expose NetworkManager's P2P D-Bus surface.
- iwd **does** ship P2P and WiFi Display interfaces — a
  `net.connman.iwd.p2p.ServiceManager` with `RegisterDisplayService`, and a
  `net.connman.iwd.p2p.Display` peer interface exposing `Source`, `Sink`,
  `Port`, `HasAudio`, `HasUIBC`, and `HasContentProtection` — so iwd can form
  P2P groups and advertise WFD capability. What it does not ship is the caster:
  its interface contract explicitly leaves WFD capability exchange, negotiation,
  Link Content Protection, and session start to the application, and no
  mainstream caster drives those interfaces.
- Two clients driving one radio is a channel-conflict and disconnect hazard, and
  neither daemon coordinates with the other.

## Decision Drivers

- Exactly one process owns the radio. Wi-Fi is a shared, stateful, single-radio
  resource; competing controllers is a correctness bug, not a configuration
  preference.
- Interoperate with the caster ecosystem rather than writing a parallel one.
- Keep the compositor out of radio control it does not need
  ([ADR-0001](0001-scope-and-responsibility-boundary.md)).
- Do not let a caster bypass the portal's user-consent flow.

## Considered Options

1. **Tessera never owns the radio; the system authority is NetworkManager with
   wpa_supplicant; the caster is a portal client and needs no compositor scope.**
2. **iwd owns the radio, including P2P; Tessera implements WFD negotiation.**
3. **Run iwd for station and NetworkManager for P2P on the same radio.**
4. **Give a caster a direct compositor scope.**

## Decision Outcome

Chosen option: **Option 1.**

### 1. Tessera never owns the Wi-Fi radio

The wireless subsystem is at most a **station client**: it may observe and
request an infrastructure link. It does not perform P2P discovery, group
formation, WFD negotiation, or AP hosting, and the `WirelessBackend` trait gains
no such verbs. This is what makes the radio-ownership question answerable at all:
Tessera is not a candidate owner, so it cannot conflict with whoever is.

### 2. The system Wi-Fi authority is whatever already owns the radio

**NetworkManager is not required by Miracast.** The protocol needs (a) a
controller that can form a Wi-Fi Direct group and (b) an implementation of the
WFD session. NetworkManager supplies only (a), and only in its wpa_supplicant
backend; the ecosystem caster supplies (b) and *demands* NetworkManager for (a).
That coupling belongs to the ecosystem, not to the protocol — and it is why
adopting the ecosystem caster forces NM + wpa_supplicant onto a machine that may
already have a working authority.

Measured on the reference machine (stock iwd, no NetworkManager, no
`wpa_supplicant` installed): iwd exposes `net.connman.iwd.p2p.Device`
(`GetPeers`, `RequestDiscovery`, `AvailableConnections = 1`) and
`net.connman.iwd.p2p.ServiceManager.RegisterDisplayService`. The radio half of
Miracast is therefore available with **no new daemon**.

The decision, corrected accordingly:

- **Tessera never owns the radio** (`[INV-ARCH-…]` lineage of §1), and that
  stands.
- **Which authority owns it is a deployment choice, not ours to force.**
  NetworkManager + wpa_supplicant is the choice that lets an *existing* caster
  work; iwd is the choice that requires writing the WFD session ourselves.
- Because we take the second path
  ([ADR-0192](0192-first-party-miracast-caster.md)), the reference
  configuration is **iwd**, and no NetworkManager is needed. A deployment that
  already runs NetworkManager can equally use the ecosystem caster; both remain
  valid, and neither may coexist with the other on one radio.

iwd's P2P/WFD interfaces are `[Experimental]`, and iwd documents one concrete
limitation: it "will refuse to connect to a dual-role peer if the local WFD
device is also dual-role capable". [ADR-0192](0192-first-party-miracast-caster.md)
registers as a Source-only device to stay on the permitted side of that
limitation, and validates it against a real sink before anything is built on it.

### 3. The caster is a portal client, so no caster scope exists

A caster (Miracast, AirPlay, Cast, Moonlight, WebRTC) talks to
`org.freedesktop.portal.ScreenCast` / `RemoteDesktop`; the **portal backend**
talks to the compositor through the identity-bound `atrium-portal` scope, which
already carries `ManageVirtualOutput` and `StreamOutput`. The caster therefore
needs **no compositor IPC scope**, and minting one would be a defect: it would
let a caster create virtual outputs and read frames *without* the portal's
user-consent and session mediation — the exact property the portal exists to
provide. Item 3 is resolved by removing the requirement, not by satisfying it.

The compositor therefore owes the portal backend nothing new: the contract is
the virtual-output and stream primitives of
[ADR-0186](0186-session-scoped-virtual-extend-outputs-and-offscreen-output-streaming.md),
pinned by `portal_scope_covers_the_cast_contract`.

### 4. The iwd bridge stays, as the station client

The native iwd bridge remains the station client and is now also the radio
authority the caster leans on for P2P
([ADR-0192](0192-first-party-miracast-caster.md)). The split is explicit: the
wireless subsystem keeps the **station** link
([ADR-0190](0190-wireless-backends-are-selected-and-capability-declared.md)),
the caster takes the **P2P** role, and iwd permits both because P2P has its own
device object and its own connection budget
(`net.connman.iwd.p2p.Device.AvailableConnections`). A machine that cannot cast
reports it through the capability model rather than failing obscurely later.

## Rejected Alternatives & Negative Knowledge

### 2. Mandate NetworkManager + wpa_supplicant and adopt the ecosystem caster
- **Why considered**: zero protocol code to write or maintain, and the ecosystem
  caster (`gnome-network-displays` and peers) already implements the WFD session.
- **Why rejected**: it converts a *caster* dependency into a *radio authority*
  dependency. On a stock iwd session it requires installing NetworkManager and
  `wpa_supplicant` and moving radio ownership away from the running authority,
  for a machine whose radio half already works
  ([ADR-0192](0192-first-party-miracast-caster.md) has the measurements). It also
  makes the casting feature set bounded by a third project's decisions. It
  remains a legitimate deployment for a machine that already runs
  NetworkManager, and nothing here forbids it.

### 3. iwd for station and NetworkManager for P2P on one radio
- **Why considered**: Keep the working iwd station path *and* get the ecosystem
  P2P path.
- **Why rejected**: Two daemons, one radio. They contend for the interface and
  neither coordinates: channel switches, dropped associations, and status that
  disagrees between two sources. A single-radio resource has a single owner.
  iwd's own P2P device object is the supported way to have both roles in *one*
  authority, which is what [ADR-0192](0192-first-party-miracast-caster.md) uses.

### 4. Give a caster a direct compositor scope
- **Why considered**: Fewer moving parts than routing consent through a portal.
- **Why rejected**: It bypasses portal consent and session mediation, and makes
  every caster a trusted compositor peer — the opposite of the
  [ADR-0128](0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)
  peer-identity model. It is the shortcut this ADR exists to refuse.

## Consequences

### Positive
- The radio-ownership conflict dissolves without mandating a daemon change:
  Tessera is not a contender, and the authority that already owns the radio
  keeps it.
- No caster scope is minted, so no new trusted peer and no consent bypass.
- The compositor owes the caster nothing beyond the primitives it already has,
  so the casting work cannot destabilise presentation.

### Negative / Trade-offs
- Because the WFD session is ours
  ([ADR-0192](0192-first-party-miracast-caster.md)), this project now owns RTSP,
  MPEG2-TS, RTP, and encoder plumbing — with the sink-compatibility treadmill
  that implies. That cost was accepted deliberately over mandating a radio
  change, not overlooked.
- A deployment that insists on the ecosystem caster must run NetworkManager with
  wpa_supplicant and must not also keep iwd as the station authority. That
  constraint belongs in release notes.
- iwd's P2P/WFD interfaces are `[Experimental]`; an iwd release may change them,
  and the caster's radio layer is the part that absorbs that churn.
