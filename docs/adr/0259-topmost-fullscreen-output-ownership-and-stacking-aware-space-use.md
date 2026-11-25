---
id: ADR-0259
title: "Topmost Fullscreen Output Ownership and Stacking-Aware SpaceUse"
status: accepted
date: 2026-11-25
scope: desktop/window, wayland/server, shell/chrome, shell/hot-zone, shell/dock
superseded_by: null
negative_knowledge: true
---

# 0259. Topmost Fullscreen Output Ownership and Stacking-Aware SpaceUse

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0064](0064-output-space-use-and-chrome-policy.md), [ADR-0134](0134-compositor-driven-fullscreen-for-any-toplevel.md), [ADR-0183](0183-geometric-output-ownership.md), [ADR-0254](0254-dual-channel-reachability-and-component-mounted-hot-zones.md)

---

## Context and Problem Statement

`SpaceUse::resolve` previously treated fullscreen ownership as an order-independent existential predicate: if *any* visible, non-minimized window on the current workspace declared `fullscreen` or geometrically covered the output, `SpaceUse` unconditionally returned `SpaceUse::Fullscreen` and short-circuited.

When a user switched from a fullscreen application (such as a game) to another non-fullscreen window on the same workspace (via `Alt+Tab`, launcher, or window switcher):

- The non-fullscreen window stacked cleanly *above* the fullscreen window, and the compositor already exited Direct Scanout to composite both surfaces.
- However, `SpaceUse` remained stuck at `SpaceUse::Fullscreen` because the buried fullscreen window was still visible and non-minimized.
- Consequently, the Dock, all eight Hot Zones (ADR-0254), and Control Center remained locked out even while the user was actively interacting with a normal desktop window on top of the fullscreen app — creating a dead-end for pointer-only workflows.

## Decision Drivers

- **Preserve overlapping stacking without forcibly hiding fullscreen clients**: Forcibly minimizing or unmapping a fullscreen game on focus loss breaks game engines (audio suspension, network disconnects, swapchain/EGL teardown) and prevents quick glance-and-return workflows. Users who want strict isolation already have workspaces.
- **Fullscreen chrome silence applies only to active topmost immersion**: Hot Zones, Dock, and Control Center should be silenced only while a fullscreen (or self-placed output-covering) window actually owns the top of the stacking order.
- **Immediate restoration on return**: Raising the fullscreen window back to the top must immediately restore `SpaceUse::Fullscreen`, silence chrome, and allow Direct Scanout on the next frame.
- **One pure Z-order rule in `tessera-desktop`**: No duplicate stacking heuristics in `tessera-shell` or `tessera`.

## Decision Outcome

`SpaceUse::resolve` now consumes visible occupants in **bottom-to-top Z-order** (matching `Server::live_surfaces()` and compositor paint order):

1. Minimized occupants are ignored.
2. An occupant claiming fullscreen (`fullscreen || (covers_output && self_placed && !maximized)`) sets the running state to `SpaceUse::Fullscreen`.
3. Any non-fullscreen occupant stacked *above* it supersedes the buried fullscreen claim:
   - a `maximized` occupant transitions the state to `SpaceUse::Maximized`;
   - a non-maximized floating/tiled occupant transitions `SpaceUse::Fullscreen` back to `SpaceUse::Available` (while preserving `SpaceUse::Maximized` if a maximized window is also stacked above the buried fullscreen window).
4. Raising the fullscreen window back to the top of the Z-order makes it the last occupant in the walk, immediately restoring `SpaceUse::Fullscreen`.

### Invariants & Behavioral Boundaries

- `[INV-SPACE-01] Topmost Fullscreen Ownership`: `SpaceUse::Fullscreen` holds if and only if the topmost visible, non-minimized window in stacking order claims fullscreen ownership (declared fullscreen or self-placed opaque full-output coverage). A non-fullscreen window raised above a fullscreen window unlocks chrome, Hot Zones, Dock, and Control Center without hiding or minimizing the fullscreen window beneath it.
- `[INV-SPACE-02] Buried Maximized Windows Do Not Leak Through Fullscreen`: When a maximized window is buried beneath a fullscreen window and a floating window is raised on top of the fullscreen window, `SpaceUse` resolves to `Available` rather than `Maximized`, because the buried maximized window is occluded by the fullscreen layer below the floating window.

## Rejected Alternatives & Negative Knowledge

- **Forcibly minimizing or hiding fullscreen windows when switching to another app.**
  Rejected: causes swapchain/audio/network regressions in games and media players, destroys the visual continuity of overlaying a quick chat/terminal/browser window over a game, and conflates window switching with workspace switching.
- **Keeping `SpaceUse::Fullscreen` active while non-fullscreen windows are stacked above it.**
  Rejected: traps pointer-driven users without Dock, Hot Zones, or Control Center while interacting with ordinary desktop windows, violating ADR-0254's dual-channel reachability contract.
