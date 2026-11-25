---
id: ADR-0256
title: "Dock Preview Safe Corridor and Window-Level Card Actions"
status: accepted
date: 2026-11-25
scope: shell/dock, shell/component, desktop/keybind, architecture
superseded_by: null
negative_knowledge: true
---

# 0256. Dock Preview Safe Corridor and Window-Level Card Actions

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0146](0146-dock-intent-dwell-and-anti-trapping-interaction.md) (adds Safe Corridor pointer transit protection and disables retreat-latch cancellation while moving toward an active Live Preview), [ADR-0084](0084-session-scoped-always-on-top-window-band.md) (moves per-window Always-on-Top and Maximize controls to Dock Live Preview Cards for multi-instance apps and adds a compositor keybind action)

---

## Context and Problem Statement

Two concrete interaction defects surfaced on multi-instance applications in the
Dock:

1. **Premature Dock collapse and preview loss on transit**: When a running
   application has multiple windows, its Live Preview panel expands horizontally
   far beyond the owning icon (and often beyond the Dock's resting bounds).
   Moving the pointer diagonally from the icon toward an outer preview card
   failed for four compounding reasons:
   - `gap_bridge` only covered the narrow axis-aligned gap directly between
     `owner` and `surface`, leaving diagonal transit paths outside the bridge;
   - brushing an adjacent tile immediately switched `hovered_tile` and destroyed
     the active preview;
   - `AUTOHIDE_QUICK_DISMISS_TIMEOUT` (0.15 s) collapsed the Dock before a
     pointer could recover from a brief edge skim;
   - `detect_cursor_retreat` treated upward movement above `rest_bounds.y`
     (directly toward the preview panel) as retreating from the Dock and latched
     collapse (`dismiss_latched = true`).

2. **Ambiguous Always-on-Top / Maximize target on multi-instance apps**:
   [ADR-0084](0084-session-scoped-always-on-top-window-band.md) placed the
   Always-on-Top toggle inside the Dock's application-level `AppMenu`, guessing
   one target window (`activated`, else last window). When an application has
   multiple windows, users cannot tell or choose which instance will be pinned
   or maximized.

## Decision Drivers

- Reaching any card of a multi-instance Live Preview must feel rock-solid:
  diagonal pointer motion from icon to outer preview card must never collapse
  the Dock or swap the preview to a neighbour tile.
- Window-level lifecycle actions (`Always on Top`, `Maximize/Restore`, `Close`)
  must bind unambiguously to a specific `WindowId`.
- Keyboard-only users must be able to toggle `Always on Top` on the focused
  window without using the Dock.

## Considered Options

- **Enlarge the rectangular `gap_bridge` to the union bounding box.** Rejected:
  still allows neighbour tile hover to steal focus during diagonal transit and
  does not fix `detect_cursor_retreat` misfires.
- **Add nested submenus per window inside `AppMenu`.** Rejected: high pointer
  precision cost compared to acting directly on the visual Live Preview cards
  already showing each window.
- **Safe Triangle corridor + retreat suppression + Preview Card window controls
  + focused-window keybind.** Chosen.

## Decision Outcome

Chosen option: **Safe Triangle corridor + retreat suppression + Preview Card
window controls + focused-window keybind**.

### 1. Preview Safe Corridor & Anti-Collapse Discipline

- **Safe Corridor (Convex/Trapezoid Transit Envelope)**: While a Live Preview is
  open, the hover keep-alive region includes the preview panel (with a generous
  10 px forgiveness margin), the owning tile, and the trapezoidal corridor
  connecting the owning tile's edge to the full width/height of the preview
  panel's facing edge.
- **Safe Triangle Neighbour Freeze**: While the pointer moves within the triangle
  formed by the previous pointer position (or owner tile) and the two facing
  corners of the preview panel toward the panel, `hovered_tile` is frozen so
  skimming across adjacent Dock tiles does not swap or destroy the preview.
- **Retreat Suppression**: Whenever `over_hover_surface` is true or the pointer
  is inside the preview Safe Corridor, `retreating` is unconditionally `false`.
- **Interaction Timeout**: An active Live Preview marks the reveal session as
  interacted (using full `autohide_timeout`, never 0.15 s quick dismiss) and
  holds a brief exit grace period (200 ms) before dismissing the preview on
  accidental boundary skims.

### 2. Window-Level Controls on Live Preview Cards

- Each Live Preview card exposes unambiguous per-window controls bound to its
  exact `WindowId`:
  - **Hover action buttons** in the card's top-right corner: **Always on Top
    (Pin)** toggle and **Close (`×`)** button.
  - **Persistent Pin indicator**: Any window with `always_on_top == true` shows
    its highlighted Pin badge even when not hovered, making pinned instances
    immediately identifiable across multiple windows.
  - **Right-click on a Preview Card**: Opens `AppMenu` scoped to that single
    window (Always on Top, Maximize/Restore, Minimize, Close).

### 3. Clean Multi-Instance `AppMenu` & Keybind

- When an application has **1 live window**, `AppMenu` retains single-window
  Maximize/Restore and Always-on-Top rows (zero ambiguity).
- When an application has **> 1 live windows**, `AppMenu` omits the ambiguous
  single-window Maximize/Restore and Always-on-Top rows, keeping only app-level
  and batch actions (window list with pin markers, New Window, Pin/Unpin,
  Minimize All, Close All).
- Adds `Action::ToggleAlwaysOnTop` to `tessera_desktop::keybind::Action` (default
  binding `Super+Shift+T`, configurable via `[[keybind]]`).

### Invariants & Behavioral Boundaries

- `[INV-DOCK-PREVIEW-01] Safe transit`: Moving a pointer along any straight line
  from the owning tile centre to any card of an open Live Preview keeps
  `keeps_revealed == true`, `retreating == false`, and `hovered_tile` unchanged.
- `[INV-DOCK-PREVIEW-02] Unambiguous window targeting`: Multi-instance `AppMenu`
  never guesses a single window for Maximize/Restore or Always-on-Top; every
  per-window toggle carries an explicitly selected `WindowId`.

## Rejected Alternatives & Negative Knowledge

### Keeping single-window fallback in multi-instance `AppMenu`
- **Why considered**: Preserves muscle memory for single-action right click.
- **Why rejected**: Silently targets an arbitrary window (`live.iter().rev()`)
  whenever focus is on another application, violating user predictability.

## Consequences

### Positive
- Multi-instance Live Previews are effortless to reach without accidental Dock
  collapse or preview flicker.
- Always-on-Top and Close are one click away on the exact window thumbnail, and
  pinned state is visually obvious.
- Keyboard users gain direct `ToggleAlwaysOnTop` control.

### Negative / Trade-offs
- Preview card hit-testing gains two small button hitboxes (Pin, Close) that
  take precedence over card-click focus.
