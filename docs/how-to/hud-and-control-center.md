# How to Use the HUD and the Control Center

Tessera presents system status as a minimal HUD and keeps every related
interaction in one modal panel, the control center (ADR-0080, ADR-0081,
ADR-0083, ADR-0114). The panel is the display-and-control surface for
desktop-computer behavior (ADR-0115): the state and controls of the
daily-use domains — sound, displays, network and Bluetooth, power, the
session, notifications, the tray, your account persona, machine resources,
and desktop preferences. It is not a console for the compositor itself.

## Read the HUD Chips

Two frosted chips float over the desktop along the top edge:

- **Left** — network status (the associated Wi-Fi network's name when the
  live link is wireless), Bluetooth (a themed on/off glyph, plus the connected
  peripheral's name while a link is up — the radio state is carried by the
  glyph, so the label slot names *which* device), the speaker's level (or
  "Muted"), and battery, the StatusNotifierItem tray row (excess items collapse
  into a `+N` indicator), then the clock and the notification count.
- **Center** — one dot per workspace; the active workspace's dot is larger
  and brighter.

The chips reserve no space: windows tile and maximize underneath them, and
clicks fall straight through to the windows below. The HUD chips are strictly
passive telemetry monitors and cannot be clicked or activated. All interactive
controls, configuration toggles, and detail views belong exclusively to the
control center (ADR-0080). Moving the cursor near a
chip fades it out; moving away fades it back in. A fullscreen window hides
the HUD entirely.

Set `enabled = false` under `[hud]` in `config.toml` to turn the HUD off at
the next compositor launch.

## Read Notification Popups

New notifications appear as a frameless strip at the top-right: plain
floating text, newest on top, no background panel. A popup is display-only
— it never captures clicks — and disappears on its own after three seconds.
The notification itself is not gone: it stays in the command panel's
notifications list (and counts toward the HUD bell) for up to an hour,
where you can dismiss it. Do Not Disturb suppresses popups while the list
keeps accumulating.

## Open the Control Center

Press `Super+S`, or swipe down on the touchpad with four fingers. The
control center opens as a boundless floating HUD layout over an opaque background.
The background and elevated surfaces follow the desktop's light or dark
appearance, while system blue marks active controls. The main control panel
sits in the center, with a compact profile chip at the top-left,
notifications at the top-right, and the machine telemetry and tray column at
the right. The HUD and Dock hide while the control center owns the screen and return
after it has fully closed.

Close it with `Super+S` again, `Escape`, a click on the scrim, the close
button at the right end of the tab bar, or a four-finger swipe up.

Bind a different key with a `[[keybind]]` entry whose action is
`control_center`:

```toml
[[keybind]]
mods = ["super"]
key = "d"
action = "control_center"
```

### Preview and Debug Routing

For development, visual inspection, and verification without input gestures or keybindings:

```bash
TESSERA_PREVIEW=control-center cargo run -p tessera
```

This runs a fully mocked, zero-side-effect component sandbox:
- **Rich baseline telemetry**: Pre-populates realistic state (88% battery charging, Wi-Fi connected to `Home-WiFi-5G` with full scan list, 65% volume, 80% screen brightness, 66% keyboard backlight, Bluetooth active, and seeded notification cards);
- **Zero host interference**: Volume, brightness, Wi-Fi, Bluetooth, and power actions update in-memory state and render immediately without spawning subprocesses (`wpctl`, `brightnessctl`, `rfkill`, `systemctl`) or altering host wireless daemons;
- **State isolation**: Isolates dock pins, configuration, and audit journals within a disposable temporary sandbox directory.

## Read the Profile and Machine Monitors

The **profile chip (top-left)** shows your avatar (with live 3D VRM rendering
and VRMA animation playback when configured), display name, and account groups.

The **main control panel (center)** holds System quick controls (Sound, Brightness,
Connectivity, Desktop) and modular settings tabs.

The **machine monitor (right-center)** shows a live hardware summary: chassis
pictogram, CPU load with a recent-history sparkline, GPU, memory, network
rates, disk, and battery, pinned above the StatusNotifierItem tray.

The **notifications panel (top-right)** presents retained notification history
with click-to-dismiss.

## Switch Tabs

Click a tab in the main panel's flat tab bar — **System** plus one tab
per available settings module. The active tab uses the system-blue accent.
Long tab bodies scroll inside the main panel.

## Adjust Quick Settings

Open the panel and select the **System** tab. Controls are arranged in an ergonomic **2D Bento Grid** (ADR-0169):

- **Bento Top Cluster**: Network tiles (expandable Wi-Fi on top, expandable Bluetooth below) paired directly beside the integrated **MPRIS Now Playing card** (album art, track title, artist, and transport controls: Previous, Play/Pause, Next);
- **Quick Action Toggles**: Do Not Disturb and Dark Mode arranged side-by-side;
- **Continuous Faders**: Display brightness slider, sound volume slider with mute toggle, and an adaptive keyboard backlight slider (when supported by hardware, ADR-0168). Controls a host cannot serve read as unavailable or omit their fader entirely.

## Manage Wireless Networks (Wi-Fi)

Under the **System** tab, the Connectivity group features an expandable Wi-Fi tile (ADR-0162, ADR-0167):

- **Expand Details**: Click the chevron (`>`) on the Wi-Fi tile to reveal the inline network list. Discovered access points sort by signal strength with normalized signal indicators and radio band badges (`2.4 GHz` or `5 GHz`).
- **Scan**: Click the refresh button (`↻`) in the header row to trigger an active channel scan through the host wireless daemon (`iwd`).
- **Connect**: Click an open or saved network to associate immediately. Clicking an unconfigured secured network opens an inline passphrase field; typing the passphrase and pressing `Enter` or clicking **Connect** delivers credentials through the native D-Bus agent.
- **Forget Saved Profile**: Saved networks display an inline **Forget** button. Activating it purges credentials and profiles from `/var/lib/iwd/`. If actively connected, the link disconnects immediately.
- **Auto-Connect Toggle**: Saved profiles provide an **Auto** toggle pill. Disabling it prevents the daemon from automatically connecting when in range (useful for metered mobile hotspots).
- **Radio Power**: The header switch enables or disables the wireless radio interface without discarding configured networks.

## Manage Bluetooth Devices

Under the **System** tab, the Connectivity group features an expandable Bluetooth tile (ADR-0175):

- **Expand Details**: Click the chevron (`>`) on the Bluetooth tile to reveal the inline device list. Connected peripherals sort first, then paired ones, then in-range-only discoveries.
- **Scan**: Click the refresh button (`↻`) in the header row to start a bounded (20-second) discovery scan through the host Bluetooth daemon (BlueZ). Discovery auto-stops when the window closes.
- **Connect / Disconnect**: Click a device to connect; the connected device shows a **Disconnect** button. Connecting to a trusted, already-paired device re-establishes its link.
- **Pair**: An in-range, unpaired device offers a **Pair** button. Pairing trusts the peripheral so BlueZ may reconnect it automatically on proximity.
- **Forget**: A paired device offers **Forget**, which removes its host-side pairing keys. If it is connected, the link drops immediately.
- **Radio Power**: The header switch powers the adapter on or off through the host Bluetooth daemon. Devices are not forgotten when the radio is off.

Opening the Wi-Fi and Bluetooth detail views is mutually exclusive: the quick-controls body hosts one at a time, and `Escape` peels the open detail view before it closes the panel.

## Edit Persistent Settings

Select a settings module tab — **Display**, **Input**, **Appearance**,
**Dock**, or **Power Management**. Display, appearance, and power edits stage
locally and commit when you select their **Apply** button; input and dock
edits apply immediately. Committed changes persist to `config.toml`
through the compositor's revisioned settings transaction; see the
[Settings Reference](../reference/settings.md) for module routes, apply
policies, and backend availability.

## Activate a Tray Item

The tray grid is always visible at the bottom of the side column. Items
lay out in a grid whose column count adapts to the panel width.
Left-click an item to activate it; right-click to open its context menu.
Items that expose a dbusmenu `Menu` object path render the menu in place
with submenu navigation; all others fall back to the specification's
`SecondaryActivate`.

## Dismiss Notifications

The notifications list fills the rest of the side column and needs no
tab or section switch. Notifications list as cards with the summary and
body, newest first; click a card to dismiss it. The list scrolls and has
no row cap: entries stay for up to an hour after they were posted — long
after their three-second popup has disappeared. Do Not Disturb (on the
**System** tab) suppresses popups while keeping the list.

Four-finger swipes are compositor-owned and never reach applications. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut and gesture reference.
