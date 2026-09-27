---
id: ADR-0169
title: "Control Center 2D Bento Grid and MPRIS Media Player Absorption"
status: accepted
date: 2026-09-28
scope: control center, shell, layout, mpris, media
superseded_by: null
negative_knowledge: true
---

# 0169. Control Center 2D Bento Grid and MPRIS Media Player Absorption

- Status: Accepted
- Date: 2026-09-28
- Deciders: Tessera Maintainers & Core Architects
- Amends: [ADR-0044](0044-dock-and-control-center-crates.md), [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md), [ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md), [ADR-0155](0155-workspace-tiering-shell-lexicon-and-state-canonization.md)

---

## Context and Problem Statement

The desktop Control Center previously positioned its components across a 9-region spatial anchor system. Under this layout, media playback controls (`MPRIS`) were relegated to an isolated, standalone card floating in the screen's left-bottom corner (`MEDIA_W = 260.0, MEDIA_H = 148.0`).

This split layout created severe ergonomics and architectural problems:
1. **Interaction and Visual Fragmentation**: While the user's focus naturally settled on the central control panel, an isolated media card in the far bottom-left corner fractured visual attention. Modulating volume and skipping tracks required pointer traversal across disparate screen quadrants.
2. **1D Linear Stack Bottlenecks**: Inside the central Quick Controls section, widgets were arranged in a strict 1D vertical stack (`column_ex` of `row_ex` pairs followed by full-width horizontal faders). Adding new controls (e.g. keyboard backlight, VPN, performance profiles) caused vertical height explosion, exceeding screen bounds on compact displays.
3. **Screen Clutter and Spatial Inefficiency**: The isolated left-bottom anchor occupied valuable screen real estate, obstructing wallpaper artwork and background window visibility while requiring complex anti-collision and squeeze logic against the central panel.

We require a unified 2D modular grid architecture ("Bento Grid") that absorbs media playback directly into the primary quick controls surface.

---

## Decision

We dissolve the independent left-bottom media anchor, re-architect the Quick Controls surface as a unified **2D Bento Grid**, and embed MPRIS Now Playing directly alongside network and display controls.

### 1. Dissolve Standalone Left-Bottom Media Anchor

We eliminate the floating media panel from `cluster_bounds`:
- `cluster_bounds` now yields 7 clean regions: Profile (top-left), Notifications (top-right), Clock (top-center), Tray (left-middle), Main Surface (center), Work Mode (right-bottom), and Power Session (right-bottom).
- Constants `MEDIA_W` and `MEDIA_H` and outer click-away collision tests for the bottom-left media card are permanently retired.
- The screen left-bottom quadrant remains completely unobstructed.

### 2. 2D Bento Grid Architecture in Quick Controls (`tessera-shell`)

The Quick Controls section transitions from a linear vertical stack into a high-density 2D modular Bento Grid:
- **Top Bento Cluster**:
  - **Left Column (2×2 unit span, height: $2 \times \text{tile\_h} + \text{gap}$)**:
    - Stacked wide tiles: Wi-Fi tile (with live SSID and flyout expansion) on top, Bluetooth tile (with connection status) below.
  - **Right Column (2×2 unit span, matching height)**:
    - Dedicated **MPRIS Now Playing Bento Card**: Integrates playback state, album art / identity icon, song title, artist, and transport controls (Previous, Play/Pause, Next) directly beside the network controls.
- **Middle Grid Row**:
  - Twin action tiles: Do Not Disturb and Dark Mode arranged side-by-side.
- **Bottom Fader Rows**:
  - Screen Brightness fader (`tessera-hud-quick-brightness`).
  - Sound Volume & Mute fader (`tessera-hud-quick-volume`), placed in direct visual proximity to the media player above it.
  - Keyboard Backlight fader (`tessera-hud-quick-kbd-brightness`), rendered conditionally when supported by hardware (ADR-0168).

### 3. Integrated Media Player Transport Routing

The integrated media card consumes `self.media` (`MediaHandle`) directly:
- When active media players exist on the session D-Bus (`org.mpris.MediaPlayer2.*`), the card displays live metadata and enables previous/play/pause/next buttons.
- In preview and offline mode, the card renders a clean, uncompromised empty or demo state.
- Button presses within the card dispatch asynchronous non-blocking commands via the worker channel.

---

## Consequences

### Positive
- **Cohesive Workflow**: Volume modulation and media transport reside within the same visual boundary, eliminating pointer ping-pong across the desktop.
- **Spatial Elegance**: The left-bottom screen corner is freed, restoring clean geometry to the wallpaper and desktop background.
- **Compact Vertical Footprint**: The 2D Bento layout compresses two network tiles and the full media player into a single 126px vertical band, saving over 140px of vertical space.
- **Unified Click Boundary**: Simplifies click-away dismiss testing by eliminating external floating card hitboxes.

### Negative / Trade-offs
- The Quick Controls top row occupies a fixed 2-column layout; screens narrower than 320px must clamp column widths.

---

## Rejected Alternatives & Negative Knowledge

- **Keep Media in Left-Bottom Anchor and Mirror in Quick Controls**:
  - *Rejected*: Duplicate interactive surfaces violate single-source-of-truth principles and introduce visual redundancy.
- **Full-Width Media Bar in Quick Controls**:
  - *Rejected*: Placing a full-width media bar pushes all toggles and sliders downward by 80px+, exacerbating vertical height pressure. Side-by-side placement against the 2 stacked network tiles achieves perfect square aspect ratio balance.
