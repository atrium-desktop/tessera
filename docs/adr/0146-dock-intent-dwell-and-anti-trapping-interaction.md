# ADR-0146: Dock Intent Dwell, Dynamic Bounds, and Anti-Trapping Interaction

- Status: Accepted
- Date: 2026-06-18

## Context

[ADR-0019](0019-dock-as-bottom-center-overlay.md) established the macOS-style
bottom-center Dock overlay, including autohide and window-collision collapse
mechanics. However, in real-world desktop usage, the collapsed Dock caused
persistent friction when users navigated content near the screen bottom:

1. **Zero-barrier accidental triggers (skimming penalty)**: Entering the
   collapsed indicator handle immediately requested expansion on the very first
   frame (0ms intent threshold). A transit cursor searching for client content
   near the bottom edge inadvertently triggered full Dock expansion.
2. **Hitbox inflation trap (premature capture)**: Once expansion reached 20%
   (`reveal >= 0.2`), the hit-testing corridor switched to the full resting
   geometry. A cursor stationary in the client work area was swallowed by the
   inflating hitbox, trapping the user and locking the Dock open.
3. **Punitive retreat timeout**: An autohidden Dock required an excessive
   2.5-second inactivity delay (`AUTOHIDE_IDLE_TIMEOUT`) before collapsing,
   forcing the user to move the cursor away and wait before regaining visibility
   and click access to the obscured application content.
4. **Premature pointer capture**: While morphing, the Dock captured pointer
   events across its future resting footprint rather than its rendered physical
   body, intercepting clicks intended for windows behind the transparent space.

## Decision

### 1. Intent dwell verification on collapsed indicator

A cursor entering the collapsed indicator must dwell continuously for at least
`AUTOHIDE_DWELL_THRESHOLD` (180ms) before expansion begins. Brief transit passes
(such as skimming downward to click a status bar or text input) accumulate dwell
progress without resetting idle timers; leaving the indicator resets dwell to
zero. The collapsed handle provides a subtle brightness lift proportional to
dwell progress, providing organic sensory feedback.

### 2. Live morphing panel hit-testing without premature inflation

During reveal and collapse transitions (`0.001 < reveal < 0.999`), pointer
capture and keep-revealed hit-testing are strictly confined to the actual live
morphing panel rectangle (`Dock::collapsed_panel_rect`) and its direct corridor
to the anchored screen edge. Pixels in the resting footprint that have not yet
been covered by the physical glass panel remain fully owned by the client
beneath, eliminating the hitbox inflation trap.

### 3. Directional retreat cancellation (early vector abort)

If the cursor moves away from the anchored edge into the client work area while
the Dock is mid-expansion, the expansion aborts immediately. The animation
blend reverses without waiting for an idle timeout, following the cursor's
retreat smoothly.

### 4. Two-tier exit timeouts (snappy quick dismissal)

The collapse delay is split into two behavioral tiers:
- **Snappy quick dismiss (150ms)**: When the Dock expands without user
  engagement (no tile clicks, tooltip dwells, menu opens, or drags), leaving
  the Dock collapses it after `AUTOHIDE_QUICK_DISMISS_TIMEOUT` (0.15s).
- **Standard interaction timeout (500ms)**: When the user has actively
  interacted with the Dock, leaving the Dock collapses it after
  `AUTOHIDE_IDLE_TIMEOUT` (reduced from 2.5s to 0.5s default).

### 5. Escape hatches: click-outside and Escape key dismissal

When an autohidden Dock is expanded, pressing `Escape` or clicking anywhere
outside the live Dock panel and its transient menus immediately collapses the
Dock. Outside clicks pass through directly to client windows without
interception.

## Alternatives

- **Rely strictly on physical screen edge push (0px hot edge only)**:
  Requiring the pointer to touch the physical edge pixel (`y == display.h - 1`)
  avoids skimming, but fails on floating dock styles with edge margins where the
  capsule is an explicit visual affordance. Combining dwell verification with the
  existing capsule maintains visual discoverability while eliminating misfires.
- **Configurable delay slider only**: Leaving the underlying level-triggered
  hitbox mechanics intact and merely adjusting timeouts would not solve the
  hitbox inflation trap or mid-flight retreat cancellation.

## Consequences

- Accidental transit cursor passes no longer trigger Dock expansion.
- The Dock never traps a cursor stationary in the application area above its
  rendered body.
- Retreating from an accidental reveal is instantaneous and friction-free.
- Default configuration values in `tessera-config` and `tessera-dock` synchronize
  on modern, responsive timing (`0.50s` standard `autohide_timeout`, `0.15s` quick exit, `0.18s` configurable `autohide_dwell`).
- Users can customize `[dock] autohide_dwell` in `config.toml` to adjust trigger sensitivity according to personal ergonomics.
