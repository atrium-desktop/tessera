---
id: ADR-0255
title: "Pure Dock and Transient Glance HUD"
status: accepted
date: 2026-11-25
scope: shell/dock, shell/glance-hud, config, input, architecture
superseded_by: null
negative_knowledge: true
---

# 0255. Pure Dock and Transient Glance HUD

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Supersedes: [ADR-0218](0218-dock-reading-column-is-a-six-unit-bento-and-status-lives-on-the-dock.md) (the Dock Bento reading column and workspace pager are removed)
- Amends: [ADR-0213](0213-flat-chrome-and-removal-of-the-status-hud.md) (workspace position moves from the Dock's leading Bento cell to the transient Glance HUD and Overview rail)
- Related: [ADR-0239](0239-the-control-center-is-a-right-anchored-sheet.md), [ADR-0254](0254-dual-channel-reachability-and-component-mounted-hot-zones.md), [ADR-0214](0214-tiered-system-readings-and-a-barrier-only-at-the-end.md)

---

## Context and Problem Statement

[ADR-0218](0218-dock-reading-column-is-a-six-unit-bento-and-status-lives-on-the-dock.md)
placed a 6-unit Bento system-reading column (Wi-Fi, Bluetooth, Battery) and a
workspace pager onto the leading edge of the Dock. While pragmatic when the
Control Center was a full-output modal, that design introduced three persistent
problems:

1. **Semantic dilution**: The Dock mixed application launching/switching with
   hardware telemetry and workspace navigation, breaking symmetry and bloating
   layout geometry (`StripCell`, `reading_extent`, `rest_centre_estimate_with_cells`).
2. **Cramped targets**: 2–3 stacked Bento rows shrank to 18–28 px, unsuitable as
   real interactive controls.
3. **Autohide blindness**: Whenever the Dock was hidden or obscured, ambient
   status and workspace position disappeared with it.

With Control Center now a lightweight right-anchored sheet ([ADR-0239](0239-the-control-center-is-a-right-anchored-sheet.md))
and pointer reachability handled by Hot Zones ([ADR-0254](0254-dual-channel-reachability-and-component-mounted-hot-zones.md)),
the Dock no longer needs to host system telemetry or workspace navigation.

## Decision Drivers

- Keep the Dock strictly focused on applications (Launchpad, pinned apps,
  transient running apps) in symmetric horizontal (`Top`/`Bottom`) or vertical
  (`Left`/`Right`) strips.
- Preserve the zero-persistent-top-bar desktop while making ambient state
  (clock, workspace position, Wi-Fi, Bluetooth, battery, audio) glanceable in
  under a quarter second via keyboard (`Double Super`), workspace switch events,
  or pointer (top-edge Hot Zone).
- Never steal keyboard or pointer focus during a status glance.

## Considered Options

- **Restore a persistent top status bar.** Rejected: violates ADR-0213's
  principle that the top of the screen belongs to windows.
- **Keep Bento cells on the Dock as optional settings.** Rejected: retains dead
  layout complexity and splits the Dock's mental model.
- **Purify the Dock and introduce a read-only transient Glance HUD.** Chosen.

## Decision Outcome

Chosen option: **Purify the Dock and introduce a read-only transient Glance HUD**,
because it simplifies the Dock to a single responsibility while giving ambient
status and workspace position a dedicated, non-intrusive, zero-idle-cost surface.

### 1. Pure Dock

- `bento.rs`, `BentoColumn`, `BentoPager`, `StripCell`, and all reading/workspace
  state and hit-testing are completely removed from `tessera-shell`'s Dock.
- The Dock strip consists solely of: Launchpad tile + pinned application tiles
  + (section divider) + transient running application tiles.
- Supports all four edges (`Top`, `Bottom`, `Left`, `Right`): horizontal layout
  on `Top`/`Bottom`, vertical layout on `Left`/`Right`.

### 2. Transient Glance HUD

A dedicated, lightweight `GlanceHud` chrome component:
- **Strictly read-only & non-modal**: Never captures pointer or keyboard input
  (`pointer_capture == false`, `keyboard_capture == false`).
- **Content**: Compact horizontal pill anchored near the top centre of the output
  displaying:
  - Wall clock & short date;
  - Workspace indicator (dots/segments for current output when > 1 workspace,
    or current workspace badge);
  - Ambient machine state (Wi-Fi, Bluetooth, Battery percentage/charging, Audio
    volume/mute, Do Not Disturb) — drawn only for capabilities reported by the
    host `SystemStatus`.
- **Triggers**:
  1. **Double-tap `Super` (`Double Super`)**: Quick double press-release of bare
     `Super` within 300 ms (with no intervening non-modifier key) raises the
     Glance HUD for ~2.2 s, then fades it out smoothly.
  2. **Workspace switch events**: Switching workspaces (`Super+Left/Right` or
     three-finger swipe) automatically raises the Glance HUD so the user
     immediately sees their new workspace position.
  3. **Hot Zone trigger**: Mounted by default on the `Top` edge Hot Zone
     (`Hover` trigger) for pointer-only users.

### Invariants & Behavioral Boundaries

- `[INV-DOCK-PURE-01] Application-only strip`: The Dock renders and hit-tests
  only application tiles (Launchpad, pinned, transient); it holds no system
  telemetry or workspace pager state.
- `[INV-GLANCE-01] Non-capturing read-only glance`: `GlanceHud` never captures
  pointer or keyboard input and never mutates system state.
- `[INV-GLANCE-02] Zero idle cost`: When dismissed/faded out, `GlanceHud`
  submits zero damage and requests no animation frames.
- `[INV-GLANCE-03] Bare Double-Super disambiguation`: Double-tap `Super` fires
  only on two bare `Super` press/release pairs without any intervening chord key
  or pointer button, never interfering with `Super+<key>` shortcuts or
  `Super+drag`.

## Rejected Alternatives & Negative Knowledge

### Keeping workspace dots permanently floating in a corner
- **Why considered**: Constant visibility without any action.
- **Why rejected**: Adds permanent floating chrome over maximized windows and
  recreates the detached-chip clutter ADR-0213 removed.

### Making Glance HUD interactive with toggles
- **Why considered**: One-click Wi-Fi or workspace switching from the HUD.
- **Why rejected**: Turns a zero-focus glance surface into a second Control
  Center / top bar; interactive controls belong in Control Center and Overview.

## Consequences

### Positive
- Dock layout, hit-testing, and damage math become dramatically simpler and
  fully symmetric.
- Ambient status and workspace position remain accessible even when the Dock is
  hidden or obscured.
- `Double Super` provides an effortless, zero-interruption heads-up glance.

### Negative / Trade-offs
- Users who relied on constant peripheral visibility of battery or workspace dots
  now use `Double Super`, top-edge hover, or workspace-switch auto-reveal.
