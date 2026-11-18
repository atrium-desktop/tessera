# Settings Reference

Persistent settings are edited in **Settings** (`tessera-settings`), a
standalone application that talks to the compositor over IPC. The Control
Center holds daily live controls only; its gear opens Settings.

## Access

| Action | Result |
|--------|--------|
| Control Center gear (`Super+S`, then ⚙) | Open Settings |
| `tessera-settings` | Open Settings |
| `tessera-settings PAGE` | Open Settings on one page, for example `tessera-settings display` |
| Launcher entry **Settings** | Open Settings |

Only one Settings window runs per session: a later invocation switches the
open window to its page and raises it. `Ctrl+F` focuses search, `Enter` in
search opens the first result, and `Escape` clears the search. Text fields use
the standard Wayland clipboard: `Ctrl+C` and `Ctrl+X` publish the selection,
and `Ctrl+V` pastes from any other client.

## Pages

| Page | Category | Apply behavior | Backend |
|------|----------|----------------|---------|
| `display` | Hardware | 15-second preview, then explicit Keep changes | Tessera output model and direct DRM backend: default topology, automatic/extend/mirror role, mode, scale, primary, arrangement |
| `input` | Hardware | Instant | Tessera input policy and libinput backend (keyboard repeat, mouse and touchpad) |
| `power` | System | Explicit Apply button | Tessera idle policy, session lock, output power, and logind sleep |
| `appearance` | Personalization | Explicit Apply button | Tessera desktop preferences and portal projection |
| `dock` | Personalization | Instant | Tessera `[dock]` configuration |
| `accessibility` | Personalization | Instant | Tessera accessibility preferences |
| `users` | System | Explicit | Not available yet; needs an AccountsService and authorization adapter |
| `window-rules` | System | Explicit | Not available yet; needs a revisioned window-rule backend |

Pages without a backend stay listed and say so, so the page set does not
change shape with the build. Search matches each page's title, its id, and
its keywords.

## Settings Transactions

Settings keeps one revisioned snapshot. It fetches the snapshot on connect
and again after every `SettingsChanged` event, so edits made by the CLI, the
Control Center's Dark Mode tile, or another client appear immediately. Edits
are submitted as typed actions carrying the revision they were drafted
against; the compositor validates, persists, applies, and journals each one.
A stale revision or backend failure shows a banner with the compositor's
reason, and the page returns to the authoritative state. Settings edits are
refused while the session is locked.

Instant pages emit an action per change while a control moves; Settings
sends only the newest draft of each kind. If the compositor link drops,
drafts are held and sent after the reconnect.

The wire schema is in [persistent settings](ipc.md#persistent-settings).
Settings, the CLI, and other IPC clients share one authority, one revision
counter, and one mutation journal.

## Permission

Under the default agent lockdown an anonymous IPC connection is query-only.
Settings claims the built-in `tessera-settings` scope, which the compositor
grants to `/usr/bin/tessera-settings`, `/usr/local/bin/tessera-settings`, and
— in a debug build — the `tessera-settings` binary beside the running
`tessera` executable (see `[ipc.scope_executables]` in the
[Configuration Reference](config.md)). The scope permits settings
transactions and focuses only a window whose app id is Settings, so a later
invocation can raise the running window. A copy of Settings the compositor
does not recognize opens **read-only**: it shows live settings, labels itself
read-only, and refuses each edit with the reason.

## Live Controls

| Surface | Access | Scope |
|---------|--------|-------|
| Control Center | `Super+S`; also over IPC | Wi-Fi, Bluetooth, Cast, Do Not Disturb, color scheme, volume, brightness, microphone, keyboard backlight, work mode, tray, notifications |

Live controls change service state, not persistent configuration. Their wire
schema is in [live system controls](ipc.md#live-system-controls).

[ADR-0238](../adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md)
records the move to a standalone application and the Settings scope;
[ADR-0239](../adr/0239-the-control-center-is-a-right-anchored-sheet.md)
records the Control Center sheet.
