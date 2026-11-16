# Control Center Component (`tessera-shell::components::control_center`)

The Control Center: daily controls, the tray, and notifications in one sheet
anchored to the right edge of the output
([ADR-0239](../../../../../docs/adr/0239-the-control-center-is-a-right-anchored-sheet.md)).

## Scope

The sheet is the display-and-control surface for desktop-computer behavior
([ADR-0115](../../../../../docs/adr/0115-command-panel-desktop-behavior-scope.md)):
sound, displays, network and Bluetooth, power, the session, notifications, the
tray, and the user's persona. The scope test is the user's computer, not this
compositor. Compositor mechanism — window-tree internals, IPC state,
introspection — stays in the CLI and MCP.

Persistent settings are out of scope. They belong to the standalone Settings
application
([ADR-0238](../../../../../docs/adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md)),
which the header's gear opens. The one exception is the Dark Mode tile: the
color scheme is a daily control, so the tile commits a complete
desktop-preferences transaction built from the last settings snapshot, and
does nothing before that snapshot arrives.

## Layout

Two floating cards, sized to their content and kept inside the reserved bands
of the shell's persistent decorations (`layout.rs`, pure and tested):

| Output | Arrangement |
|--------|-------------|
| ≥ 1024 px wide | Notifications (360 px) left of controls (380 px), top-aligned |
| narrower | Notifications stacked under controls; controls keep at least 360 px, and a notifications card shorter than 120 px is dropped |

**Controls card**, top to bottom:

- **Header** — the persona disc (photo, live VRM, or initials; shared with the
  lock screen), display name, and `@user · host`; then Settings, Lock, and
  Power icon buttons with tooltips. Power discloses a session strip —
  Suspend, Restart, Power off — and every action leaves through the
  system-level confirmation.
- **Toggles** — Wi-Fi and Bluetooth (with disclosures to their pages), Cast
  (opens the receiver list), Do Not Disturb, and Dark Mode.
- **Faders** — sound, brightness, microphone, and keyboard backlight, each
  only when its hardware or service answered. Stepped backlights get tiers
  (ADR-0168). The microphone keeps its own mute (ADR-0216).
- **Now playing** — when an MPRIS player exists.
- **Work mode** — Balanced, Stay Awake, or Dashboard, with each mode's idle
  behavior as a tooltip. It is pinned above the tray, outside the scroll.
- **Tray** — StatusNotifierItem icons pinned to the card's bottom edge, paged
  when they overflow (ADR-0221); right-click opens the dbusmenu popover above
  the icon.

Wi-Fi, Bluetooth, Sound, and Cast replace everything below the header with a
detail page that has a Back button. Entering a page starts its scan; leaving
Cast stops discovery; leaving Wi-Fi drops a half-typed passphrase.

**Notifications card** — the wall clock and date, a **Clear all** header, and
one card per notification (app, summary, first body line), newest first.
Pressing a card dismisses it.

## Interaction

Opens with `Super+S`, a four-finger swipe down, or a press on the Dock's
reading cells. Closes with the same binding, a four-finger swipe up, a press
outside both cards, or `Escape`. `Escape` peels one layer at a time: tray
popover, session strip, detail page, sheet. While presented the sheet owns the
pointer and keyboard, so a dismissing press never reaches a window.

## Rendering

No canvas, scrim, or backdrop: the desktop stays visible. The reveal is a
critically damped fade with a 24 px slide. Damage is always the cards'
footprint — bodies, elevation halo, and slide, rounded outward to whole
pixels — plus an open tray popover; a four-frame retirement window repairs
the vacated pixels after the reveal settles (ADR-0227).

## Boundaries

The component owns presentation and interaction state only. Snapshots arrive
through the `Chrome` trait; intents leave through `ChromeEvents` (system
actions, the scheme transaction, dismissed notifications, lock, launching
Settings) and the shared tray command channel. It has no D-Bus, Wayland, or
process authority of its own.
