# Network and Casting Hardware Acceptance

This is the hardware gate for the station-network service (`tessera-networkd`,
ADR-0242) and the Miracast caster. It complements the automated fake-daemon
suites: those prove protocol and policy, this gate proves the host's real
NetworkManager/iwd and radio.

## 1. Automated live checks

These tests are `#[ignore]`d because they require a live service. They never
connect, disconnect, or form a P2P group.

```bash
cargo build --locked -p tessera-network-system -p tessera-cast --bins
cargo nextest run -p tessera-network-system --run-ignored only \
  -E 'test(live_)' --no-capture
cargo nextest run -p tessera-tests --run-ignored only \
  -E 'test(live_)' --no-capture
./target/debug/tessera-cast probe
```

`tessera-network-system`'s live test opens the real NetworkManager backend,
subscribes over the service socket, asserts an available snapshot with a
Wi-Fi radio, and sends a real `ScanWifi`. The `tessera-tests` live test opens
the selected P2P authority and runs `StartFind`/`Peers`/`StopFind`.

Reference-host evidence (2026-11-17, NetworkManager with wpa_supplicant):

- `wlp0s20f3` station connected, `p2p-dev-wlp0s20f3` present and ready;
- live snapshot: `Wifi`, `wifi_enabled = true`, SSID reported, real signal bars;
- live P2P discovery: authority `NetworkManager`, no sink in range;
- `tessera-cast probe`: `wifi-p2p device ready`;
- a stopped iwd stays `inactive` when the service is explicitly started with
  `--backend iwd`; it reports unavailable instead of activating it;
- the profile CRUD live test skipped because the headless session has no
  polkit agent to create a transient profile; run it from a real desktop
  session.

The real-hardware test caught a `Settings.ListConnections` object-path bug that
the fake API had masked; keep this gate in the release checklist.

## 2. Station operations (isolated test AP required)

Do not run these on a host whose only uplink is the interface under test.
Use a disposable access point and a wired/out-of-band console.

```bash
# Creates and deletes a non-autoconnecting profile named tessera-live-test.
# Skips when polkit cannot authorize the create; run it in a desktop session.
cargo nextest run -p tessera-network-system --run-ignored only \
  -E 'test(live_networkmanager_profile)' --no-capture
```

- [ ] `auto` selects a running NetworkManager; `backend = "iwd"` selects a
      running iwd; a stopped daemon is reported unavailable and is **not**
      D-Bus-activated.
- [ ] Scan lists access points with correct security and signal bars.
- [ ] Connect to WPA2-PSK, WPA3-SAE, and open networks; disconnect; reconnect.
- [ ] Forget a saved network; verify the profile is gone from `nmcli`.
- [ ] Toggle auto-connect on a saved network; verify `connection.autoconnect`.
- [ ] Toggle the radio off/on; verify Wi-Fi disabled and restored.
- [ ] As an unprivileged user whose polkit policy denies settings changes,
      verify a named refusal is surfaced, not a retry loop or a fabricated
      success.
- [ ] On a wired-primary host, verify `NetworkState::Wired` and the physical
      interface name.
- [ ] Repeat on an iwd-only host with `backend = "iwd"` and `backend = "auto"`.
- [ ] Unplug/replug a USB Wi-Fi adapter; verify the service adopts the new
      device path without a session restart.

## 3. Casting group formation (real sink)

- [ ] On iwd-only and NetworkManager + iwd hosts: `probe`, `discover`, form a
      group with the reference TV, complete RTSP M1–M6/SETUP/PLAY, and tear
      down cleanly.
- [ ] On NetworkManager + wpa_supplicant: `probe` reports `wifi-p2p device
      ready`, `discover` lists the TV, `cast` forms the volatile profile and
      streams.
- [ ] During an active group, send `SIGINT` and `SIGTERM` to the CLI and to
      `tessera-castd`; verify `DeactivateConnection` and no residual
      `"Tessera cast"` profile/group.
- [ ] Change `[cast]` evidence and record the TV model, firmware, and host
      state in `docs/dev/runbooks/miracast.md`.

## 4. Release rule

A release that changes `tessera-network*`, the cast radio, or the WPA/WFD
mapping is not production-ready until sections 1 and 2 pass on the target
distribution, and section 3 passes when Miracast is advertised. Record the
date, host, distribution, NetworkManager/iwd versions, and TV model with the
release evidence.
