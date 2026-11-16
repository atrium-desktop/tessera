---
id: ADR-0239
title: "The Control Center is a right-anchored sheet, not a full-output canvas"
status: accepted
date: 2026-11-16
scope: shell/control-center
superseded_by: null
negative_knowledge: true
---

# ADR-0239: The Control Center is a right-anchored sheet, not a full-output canvas

- Status: Accepted
- Date: 2026-11-16
- Deciders: Tessera Maintainers
- Consulted: `crates/tessera-shell` (Control Center, `Chrome` contract,
  shell host), `crates/tessera` (runtime notification path)
- Supersedes: [ADR-0176](0176-control-center-painted-canvas-beyond-backdrop-cover.md)
  (the painted full-output canvas),
  [ADR-0169](0169-control-center-bento-grid-and-mpris-absorption.md) (the
  centered Bento main panel)
- Amends: [ADR-0221](0221-the-control-center-tray-is-a-horizontal-paging-lane.md)
  (the tray lane moves into the controls card's footer and its popover opens
  above), [ADR-0115](0115-command-panel-desktop-behavior-scope.md)
- Related: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md),
  [ADR-0227](0227-a-timed-chrome-strip-owns-a-retirement-window.md),
  [ADR-0238](0238-settings-is-a-standalone-application-behind-its-own-scope.md)

---

## Context and Problem Statement

The Control Center painted an opaque canvas over the whole output and
scattered seven surfaces across it: a profile chip at top-left, a clock at
top-center, a 260×200 notification stream at top-right, a centered 820×620
tabbed panel, the tray at bottom-left, and a work-mode band and power knob at
bottom-right.

- **It broke ADR-0213's first driver.** Chrome must not cost more than its
  area. A full-output opaque canvas made every reveal and every ring drain a
  full-output repaint for a surface whose content covers a fraction of it.
- **It hid the desktop** the controls act on, and it occluded the Dock while
  open.
- **Eye travel spanned the output.** On a large display, the clock, the
  notifications, the controls, and the power button sat in four corners.
- **It carried debris:** four mutually exclusive `*_expanded` flags for one
  piece of state, a dead Agent Workspaces snapshot, debug-build fake
  notifications and tray items, three preview environment-variable aliases,
  a `CommandPanel` type alias and `command_panel_active` trait method,
  untranslated strings, and settings tabs that now belong to the Settings
  application (ADR-0238).

## Decision Drivers

- Cost proportional to painted area; damage stated as a footprint, never
  full output.
- One focal region near where the hand goes, with the desktop visible.
- Content-sized surfaces that scale from short laptops to large monitors.
- No legacy aliases, demo data, or dead state in production code.

## Considered Options

1. A right-anchored sheet of two content-sized cards.
2. One centered card over a dimming scrim.
3. Keep the full-output canvas with a regridded interior.

## Decision Outcome

Chosen option: **1**.

1. **Geometry** (`control_center::layout`, pure and tested). Two floating
   cards anchored to the right edge, inside the reserved bands the shell's
   persistent decorations publish: a 380 px **controls** card and a 360 px
   **notifications** card. Outputs at least 1024 px wide place notifications
   to the left of controls, top-aligned; narrower outputs stack them, giving
   the controls priority down to a 360 px floor and dropping a notifications
   card that would be shorter than 120 px. Each card is sized to its natural
   content height; only the controls card's middle content scrolls when the
   output caps it, while its work-mode and tray footers stay pinned.
2. **Controls card.** Header (persona disc, name, `@user · host`; Settings,
   Lock, and Power buttons with tooltips; Power discloses a Suspend · Restart
   · Power off strip, each through the system confirmation). Toggles (Wi-Fi,
   Bluetooth, Cast with disclosures; Do Not Disturb and Dark Mode). Faders
   only for hardware that answered (sound, brightness, microphone, keyboard
   backlight). Now playing when a player exists. The work-mode switch and the
   tray row are pinned to the card's bottom edge, outside the scroll, with
   the tray popover opening above; only the controls above them scroll on a
   short output. Wi-Fi, Bluetooth, Sound, and Cast are one `Page` enum;
   entering a page starts its scan and leaving Cast stops discovery.
3. **Notifications card.** The wall clock and date (the desktop has no top
   bar), a header with **Clear all**, and one fixed-height card per
   notification (app, summary, first body line), newest first. Dismissals
   leave as an ordered list, `ChromeEvents::dismissed_notifications`.
4. **Presentation.** No canvas, no scrim, no backdrop. The sheet declares no
   opaque cover and never occludes the desktop or claims exclusive
   presentation. The reveal is a critically damped fade plus a 24 px slide.
   While presented the sheet owns pointer and keyboard so a dismissing press
   never reaches the window beneath; Escape peels popover, session strip,
   page, then sheet.
5. **Damage.** `damage_regions` and `pointer_damage_regions` return the
   cards' footprint — bodies, elevation halo, and slide strip, rounded
   outward — plus an open tray popover. After a reveal settles, a bounded
   retirement window of four frames repaints the vacated pixels (ADR-0227).
6. **Clean break.** Removed: the canvas, the settings tabs, the
   `CommandPanel` and `CommandPanelColors` aliases,
   `Chrome::command_panel_active`, the `command_panel`/`panel` gesture
   names, the `chrome-command-panel` feature spelling, the
   `TESSERA_COMMAND_PANEL*` variables (use `TESSERA_PREVIEW=control-center`),
   and all demo notifications, tray items, and menus.

### Invariants & Behavioral Boundaries

- `[INV-CC-01]` The sheet's damage is always a stated footprint while active;
  it never answers `None` (full output).
- `[INV-CC-02]` The sheet never crosses a reserved band.
- `[INV-CC-03]` A fader row exists only for hardware or a service that
  answered.
- `[INV-CC-04]` Closing the sheet clears every latch that could keep it
  animating; reopening starts on the main page.

## Rejected Alternatives & Negative Knowledge

### Centered card over a scrim (option 2)
- **Why considered**: one coherent surface, still modal.
- **Why rejected**: a scrim is a full-output paint again, and it still hides
  the desktop the controls act on.

### Regridded full-output canvas (option 3)
- **Why considered**: keeps the immersive look.
- **Why rejected**: it keeps the cost and the occlusion, which were the
  problem, not the arrangement.

## Consequences

### Positive
- Repaint cost follows the cards' area; reveals no longer repaint the output.
- Controls, notifications, and the clock read as one region near the edge.
- The Dock stays visible; the sheet lays out against its real band.

### Negative / Trade-offs
- On very short outputs the middle of the controls card scrolls; the
  work-mode switch and tray row stay pinned.
- Do Not Disturb and Dark Mode render as icon tiles at the card's width;
  their names appear as tooltips and accessible labels.
