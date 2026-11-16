# How to Read Status and Use the Control Center

Tessera has no status bar. The top of the screen belongs to windows, and status
lives in two places: readings on the Dock, and transient readings that appear
when a value changes (ADR-0213, ADR-0214). Every related daily interaction
stays in one sheet, the Control Center (ADR-0080, ADR-0081, ADR-0083,
ADR-0115, ADR-0239). The sheet is the display-and-control surface for
desktop-computer behavior: the state and controls of the daily-use domains —
sound, displays, network and Bluetooth, power, the session, notifications, the
tray, and your account persona. Persistent settings live in the Settings
application. It is not a console for the compositor itself.

## Read the Dock's Reading Cell

The Dock leads with one or more reading cells, then the application tiles. The
shipped reading is the workspace pager: one tick per workspace, with the current
one drawn as a lit capsule. Reading cells do not magnify under the cursor and are
not launch targets — they are status.

This is why the reading lives here rather than in a band across the top of the
screen: you ask "which desktop am I on" while reaching for the Dock, and the
Dock's own reveal already puts the answer on screen at that moment. Tiled and
maximized windows run over the Dock's space, so nothing is reserved for it.

Press `Super+1` … `Super+9` to switch workspaces; the pager follows immediately.

## Read a Transient Reading

When a value changes on its own or in response to a hardware key — a battery
threshold, a wireless link, a peripheral connecting — a small reading appears
centred above the Dock for a couple of seconds and then leaves.

A reading draws a gauge for a level (battery, volume, brightness) and a headline
with an optional detail line otherwise. It cannot be clicked, never takes the
keyboard, and never stacks: a second change replaces the first reading rather
than adding to it. Nothing is lost when it leaves, because it was never history.

Battery reporting is graduated, because the thresholds are:

- every threshold **above the lowest** in `[battery] warn_at` raises a reading;
- the **lowest** threshold opens a modal barrier, because that is the crossing
  after which the machine suspends.

With the default `warn_at = [20, 5]` you get a reading at 20% and a barrier at
5%.

## Read Notification Popups

New notifications appear as a frameless strip at the top-right: plain
floating text, newest on top, no background panel. A popup is display-only
— it never captures clicks — and disappears on its own after three seconds.
The notification itself is not gone: it stays in the Control Center's
notifications list for up to an hour, where you can dismiss it. Do Not Disturb
suppresses popups while the list keeps accumulating.

A notification is not a reading, and the two never share a surface. A
notification is a message from somewhere else that you may want to act on, so
it is queued, readable, and dismissible. A reading is the machine acknowledging a
change, so it is transient and takes no input.

## Open the Control Center

Press `Super+S`, swipe down on the touchpad with four fingers, or press the
Dock's reading cell. The Control Center opens as a sheet against the right
edge of the screen; the desktop and the Dock stay visible beside it. It has
two cards:

- **Controls** (right): your persona, the Settings, Lock, and Power buttons,
  then toggles, sliders, and now playing. The work-mode switch and tray stay
  pinned in the card's footer, so only the controls above them scroll on a
  short screen.
- **Notifications** (left of controls, or under them on a narrow screen): the
  time and date over your notification list.

Close it with `Super+S` again, `Escape`, a click anywhere outside the cards,
or a four-finger swipe up. `Escape` closes one layer at a time — a tray menu,
the power menu, a detail page — before it closes the sheet.

Bind a different key with a `[[keybind]]` entry whose action is
`control_center`:

```toml
[[keybind]]
mods = ["super"]
key = "d"
action = "control_center"
```

For development, open it at startup with
`TESSERA_PREVIEW=control-center cargo run -p tessera`, and post sample
notifications with `tessera notification send "Summary" "Body"`.

## Use the Header

The header shows your avatar (a photo, a live 3D VRM model, or your
initials), display name, and `@user · host`. Its buttons:

- **⚙ Settings** opens the Settings application and closes the sheet.
- **🔒 Lock** locks the session.
- **⏻ Power** reveals **Suspend**, **Restart**, and **Power off**. Each asks
  for confirmation before anything happens.

Hover a button to see its name.

## Adjust Quick Settings

- **Wi-Fi** and **Bluetooth** tiles toggle their radio; their chevron opens
  the network or device list.
- **Cast** opens the list of wireless displays.
- **Do Not Disturb** (bell) suppresses popups while keeping the list.
- **Dark Mode** (moon or sun) switches the desktop color scheme.
- **Sound**, **Brightness**, **Microphone**, and **Keyboard** sliders appear
  only when the hardware or sound server answers. The sound and microphone
  icons mute; their chevrons open the audio page.
- **Now playing** appears while a media player is running.
- **Work mode** chooses how the session behaves when idle; hover a segment for
  its behavior.

## Choose the Audio Output

The sound slider's chevron opens the output picker (ADR-0215). Its rows are the
outputs the sound server itself reports — built-in speakers, headphones, HDMI
ports, USB interfaces, connected Bluetooth headsets — with a check mark on the
one playback currently goes to. Clicking another row asks the server to move
playback there.

The picker is a live view, not a saved preference. The default marker moves only
after the server confirms the switch, so a stream that pins its own target keeps
playing where it is and the Control Center shows that instead of claiming the
move.
Use it when the automatic policy picked the wrong device on purpose — a Bluetooth
headset connecting while you are showing something to the room, for example. The
headset stays connected; only the routing moves.

When no sound server answers, the disclosure is absent rather than opening an
empty page. From a terminal, `tessera system status` lists the same inventory and
`tessera system audio-output <name>` applies the same switch.

## Choose the Microphone and See When It Is Live

The same page carries the capture side (ADR-0216). The microphone fader sits
above the output list and is labelled with the microphone applications will use;
its leading glyph is a mute toggle, separate from the sound (playback) mute so
that silencing one can never silence the other by mistake.

Below the fader, the **Inputs** list appears when the server reports more than
one microphone — the built-in array, a headset mic, a USB microphone or
interface, a connected Bluetooth headset. Clicking another row asks the server to
route new capture streams there; the check mark moves only once the server says
it did.

When any program is holding a microphone open, a **Microphone in use** marker
appears at the top of the page and the fader's label switches to the same
wording. That marker is a reading, not a setting: it reports a live link in the
sound server's own graph, so a browser tab, a recorder, and a sandboxed agent all
raise it identically, and it disappears only when the capture does. Recording a
window *with sound* captures the speakers' monitor rather than a microphone and
does not raise it.

From a terminal, `tessera system status` prints the capture level, the server's
mute, and whether anything is recording, and `tessera system mic-mute`,
`tessera system mic-volume <level>`, and `tessera system audio-input <name>`
apply the same changes.

## Manage Wireless Networks (Wi-Fi)

The Wi-Fi tile's chevron opens the network page (ADR-0162, ADR-0167):

- **Expand Details**: Click the chevron (`>`) on the Wi-Fi tile to reveal the inline network list. Discovered access points sort by signal strength with normalized signal indicators and radio band badges (`2.4 GHz` or `5 GHz`).
- **Scan**: Click the refresh button (`↻`) in the header row to trigger an active channel scan through the host wireless daemon (`iwd`).
- **Connect**: Click an open or saved network to associate immediately. Clicking an unconfigured secured network opens an inline passphrase field; typing the passphrase and pressing `Enter` or clicking **Connect** delivers credentials through the native D-Bus agent.
- **Forget Saved Profile**: Saved networks display an inline **Forget** button. Activating it purges credentials and profiles from `/var/lib/iwd/`. If actively connected, the link disconnects immediately.
- **Auto-Connect Toggle**: Saved profiles provide an **Auto** toggle pill. Disabling it prevents the daemon from automatically connecting when in range (useful for metered mobile hotspots).
- **Radio Power**: The header switch enables or disables the wireless radio interface without discarding configured networks.

## Manage Bluetooth Devices

The Bluetooth tile's chevron opens the device page (ADR-0175):

- **Expand Details**: Click the chevron (`>`) on the Bluetooth tile to reveal the inline device list. Connected peripherals sort first, then paired ones, then in-range-only discoveries.
- **Scan**: Click the refresh button (`↻`) in the header row to start a bounded (20-second) discovery scan through the host Bluetooth daemon (BlueZ). Discovery auto-stops when the window closes.
- **Connect / Disconnect**: Click a device to connect; the connected device shows a **Disconnect** button. Connecting to a trusted, already-paired device re-establishes its link.
- **Pair**: An in-range, unpaired device offers a **Pair** button. Pairing trusts the peripheral so BlueZ may reconnect it automatically on proximity.
- **Forget**: A paired device offers **Forget**, which removes its host-side pairing keys. If it is connected, the link drops immediately.
- **Radio Power**: The header switch powers the adapter on or off through the host Bluetooth daemon. Devices are not forgotten when the radio is off.

One detail page shows at a time, below the header. **Back** or `Escape` returns to the controls; reopening the sheet always starts on the controls.

## Edit Persistent Settings

Persistent settings — displays, input, power, appearance, the Dock, and
accessibility — live in the **Settings** application. Open it from the
header's ⚙ button, the launcher, or a terminal (`tessera-settings display`).
See the [Settings Reference](../reference/settings.md).

## Activate a Tray Item

The tray row sits at the bottom of the controls card. Left-click an item to
activate it; right-click to open its context menu above the icon. Items that expose a dbusmenu `Menu` object path render
the menu in place with submenu navigation; all others fall back to the
specification's `SecondaryActivate`.

When the icons exceed the row, the lane paginates: the chevron at each end
turns one whole page, and the chevron at an end you cannot reach (the leading
one on the first page, the trailing one on the last) is drawn dimmed and
refuses the press — it says "there is nothing further this way" rather than
pretending to be a live control. Turning the page closes an open context menu,
since the cell it belonged to is no longer on screen.

## Dismiss Notifications

Notifications list as cards — app, summary, and the first line of the body —
newest first. Click a card to dismiss it, or **Clear all** to dismiss every
one. The list scrolls and has no row cap: entries stay for up to an hour
after they were posted, long after their three-second popup has disappeared.
Do Not Disturb suppresses popups while keeping the list.

Four-finger swipes are compositor-owned and never reach applications. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut and gesture reference.
