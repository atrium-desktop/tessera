# Control Center

The Control Center is the display-and-control surface for desktop-computer
behavior
([ADR-0115](../../adr/0115-command-panel-desktop-behavior-scope.md)): sound,
displays, network and Bluetooth, power, the session, notifications, the tray,
and the user's persona. A surface belongs when its subject exists on any
desktop computer regardless of the compositor. Compositor mechanism stays in
the CLI and MCP, and persistent settings live in the Settings application
([ADR-0238](../../adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md)).
Every proposed control justifies itself against this scope.

The presentation is a right-anchored sheet
([ADR-0239](../../adr/0239-the-control-center-is-a-right-anchored-sheet.md)).

## Composition

```text
 wide output (≥ 1024 px)                      narrow output
 ┌──────────── desktop ─────────────┐         ┌──── desktop ────┐
 │            ┌──────────┐┌────────┐│         │      ┌────────┐ │
 │            │ 14:32    ││ ◉ name ⚙🔒⏻│       │      │controls│ │
 │            │ Monday … ││ toggles ││         │      │        │ │
 │            │ Notifs ✕ ││ faders  ││         │      └────────┘ │
 │            │ ▢ ▢ ▢    ││ media   ││         │      ┌────────┐ │
 │            └──────────┘│ mode    ││         │      │ notifs │ │
 │                        │ tray    ││         │      └────────┘ │
 │                        └────────┘│         │                 │
 │            [ dock ]              │         │    [ dock ]     │
 └──────────────────────────────────┘         └─────────────────┘
```

- **One focal region.** Both cards sit against the right edge, top-aligned,
  so clock, notifications, and controls are read in one glance instead of
  across four corners.
- **Content-sized cards.** Each card is as tall as its content. When the
  output caps the controls card, only its middle sections scroll; the
  work-mode switch and tray row stay pinned in the footer. Fader rows exist
  only for hardware that answered, so a desktop without a backlight has a
  shorter card rather than a dead slider.
- **The desktop stays visible.** No canvas, scrim, or blur: the sheet is two
  floating panels with the shared elevation halo. Appearance changes made
  from it are seen on the live desktop.
- **Order of importance.** Header (who, Settings, Lock, Power) → toggles →
  faders → now playing → work mode → tray. Detail pages (Wi-Fi, Bluetooth,
  Sound, Cast) replace the content below the header, never stacking a second
  card.

## Visual Language

- Cards use the floating-panel role: the scheme's panel surface, a hairline
  border, `radii.panel_lg`, and the painted elevation halo.
- Controls use the Control Center palette (`ControlCenterColors`): grouped
  recessed surfaces, system-blue accent for active tiles, slider fills, and
  selection.
- Icon-only controls (header buttons, narrow tiles) always carry an
  accessible label and show it as a tooltip after a short dwell.
- Typography: the clock at hero size; headings at headline; tile titles at
  label; secondary text muted at footnote or caption.

## Motion

- The reveal is a critically damped spring (no overshoot) driving opacity
  and a 24 px slide from the right; reduced motion snaps it.
- Segmented controls cross-fade their highlight along the shared
  under-damped indicator spring.
- Tooltips appear after a 0.45 s dwell and leave at once.

## Cost

The sheet's paint and damage are bounded by its cards: footprint = bodies +
halo + slide strip, rounded outward; plus an open tray popover. After the
reveal settles, a four-frame retirement window repairs the vacated pixels.
The sheet never declares an opaque cover, so the compositor never elides the
desktop beneath it.
