# ADR-0162: Wireless Network Subsystem Abstraction and iwd Bridge

- Status: Accepted
- Date: 2026-09-18
- Scope: Live system network controls, wireless management, and Control Center UI
- Amends: [ADR-0060](0060-statusbar-system-controls-and-live-system-ipc.md), [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md), [ADR-0115](0115-command-panel-desktop-behavior-scope.md)

## Context

Prior to this decision, the Control Center and Status Bar provided rudimentary Wi-Fi controls:
- Wi-Fi radio toggling relied on executing external `nmcli radio wifi` processes from within the runtime.
- SSID detection relied on probing `iwgetid` or parsing `iw link` commands.
- There was no ability to trigger wireless channel scans, discover neighboring access points, inspect signal strength, or connect to new networks requiring passphrases.

Modern Linux distributions running minimal and performant Wayland environments increasingly rely on `iwd` (Intel Wireless Daemon) instead of NetworkManager. Directly hardcoding `iwd` CLI invocations into the compositor or shell would repeat historical mistakes:
1. Shell command execution (`std::process::Command`) introduces process fork overhead and risks blocking the display server's main event and render loop.
2. Hardcoding any single daemon prevents running in alternative environments (e.g., NetworkManager-managed setups) and prevents automated headless unit/integration testing in CI environments lacking wireless hardware.
3. Passing credentials via command-line arguments violates basic credential hygiene.

## Decision

We establish an uncompromised, long-term, daemon-neutral **Wireless Network Subsystem** with an async backend abstraction, an `iwd` D-Bus bridge, and expandable detail presentation in the Control Center.

### 1. Domain Models and Protocol Contract (`tessera-desktop`)

We define clean, daemon-neutral wireless data types in `tessera-desktop::system`:

- `WifiSecurity`: `Open`, `WpaPsk` (WPA2/WPA3 Personal), `Enterprise` (802.1X).
- `WifiLinkState`: `Disabled`, `Disconnected`, `Scanning`, `Connecting`, `Connected`.
- `WifiNetwork`:
  - `ssid: String`: Human-readable network identifier.
  - `signal_bars: u8`: Normalized 0..=4 signal strength indicator.
  - `security: WifiSecurity`: Encryption requirements.
  - `is_connected: bool`: True if the station is currently associated with this network.
  - `is_saved: bool`: True if credentials or profiles are already persisted on the host.
- Extend `SystemStatus`:
  - `wifi_state: WifiLinkState`
  - `wifi_networks: Vec<WifiNetwork>`: Sorted list of scanned access points.
- Extend `SystemAction`:
  - `ScanWifi`: Explicit scan request.
  - `ConnectWifi { ssid: String, passphrase: Option<String> }`: Connect intent.
  - `DisconnectWifi`: Disconnect intent.

### 2. Wireless Subsystem Architecture and Backend Trait (`tessera`)

We introduce an asynchronous `WirelessBackend` trait:

```rust
#[async_trait::async_trait]
pub trait WirelessBackend: Send + Sync + 'static {
    async fn scan(&self) -> Result<(), WirelessError>;
    async fn connect(&self, ssid: &str, passphrase: Option<&str>) -> Result<(), WirelessError>;
    async fn disconnect(&self) -> Result<(), WirelessError>;
    async fn set_enabled(&self, enabled: bool) -> Result<(), WirelessError>;
}
```

- **`IwdBackend`**: Native D-Bus implementation communicating with `net.connman.iwd`:
  - Uses `net.connman.iwd.Station` for scanning, network listing, and link management.
  - Registers a lightweight `net.connman.iwd.Agent` on D-Bus to respond to `RequestPassphrase` callbacks when authenticating against secured networks.
- **`MockWirelessBackend`**: Headless mock implementation providing deterministic AP fixtures, simulated scan latencies, and failure injections for unit and integration testing without physical radios.
- **Async Event Pump**: Background tasks emit high-level `WirelessEvent`s (e.g. AP list refreshed, link state changed, credential requested) over Tokio channels to the compositor runtime, completely isolated from frame presentation.

### 3. Control Center Presentation and User Journey (`tessera-shell`)

The Control Center Quick Controls Wi-Fi tile evolves into an expandable interactive tile:
1. **Collapsed State (Default)**:
   - When disconnected or disabled: Displays Wi-Fi icon in inactive tone, subtitle displays "Off" or "Not Connected".
   - When connected: Displays active accent color, subtitle prominently displays the active SSID.
   - An expansion affordance (chevron `>`) toggles the detail flyout.
2. **Expanded State (Discovery & Selection)**:
   - Reveals an inline scrollable list of scanned Wi-Fi networks sorted by signal strength.
   - Each entry displays the SSID, signal bar glyph (0..=4), security lock icon, and connection status.
   - Top action row includes manual refresh/scan trigger with active spinner indicator.
3. **Connection & Credential Flow**:
   - Clicking an already-connected network prompts to disconnect.
   - Clicking an open or known/saved network immediately issues `ConnectWifi` with no prompt.
   - Clicking a secured, unconfigured network routes the user through the native secure credential entry flow (`SecretPrompt`), masking secret characters and zeroizing buffers on dismissal.

### Invariants & Behavioral Boundaries

- `[INV-NET-NONBLOCK]`: No synchronous network I/O, D-Bus blocking calls, or process spawning is permitted on the compositor main thread or render loop.
- `[INV-NET-DAEMON-NEUTRAL]`: Presentation and core models must never expose D-Bus object paths, iwd-specific state names, or backend-dependent artifacts.
- `[INV-NET-CRED-ZEROIZE]`: Passphrases and credentials must be wrapped in zeroizing memory containers and discarded immediately after dispatch.

## Consequences

### Positive
- Fully native, fluid Wi-Fi discovery and connection experience in Control Center without external windows.
- First-class support for `iwd` on lightweight modern setups with zero reliance on legacy tools like `nmcli` or `wireless-tools`.
- Fully testable in CI and headless containers via `MockWirelessBackend`.
- Clean separation between presentation, state orchestration, and OS communication.

### Negative / Trade-offs
- Expands the Control Center state space to include list expansion and active network selection state.
- Requires managing D-Bus service availability and potential fallback states if `iwd` is not active on the host.
