# Control Center Component (`tessera-shell::components::control_center`)

Modal control center for the tessera compositor (ADR-0080, ADR-0115).

## Scope

The panel is the display-and-control surface for desktop-computer behavior
(ADR-0115): the domains daily desktop use involves — sound, displays,
network and Bluetooth, power, the session, notifications, the tray, the
user's persona, and desktop preferences. The scope test
is the user's computer, not this compositor: a surface belongs when its
subject exists on any desktop computer regardless of the compositor
implementation. Compositor-mechanism surfaces — window-tree internals,
protocol or IPC state, introspection, developer tooling — stay out and
remain CLI/MCP territory. Domains follow the user's model rather than the
implementing component, so window tiling and Agent Workspaces are in scope
while Interaction Domain lifecycle management is not. Machine-resource
monitoring is deliberately out of scope: an always-on utilization probe is
a standing performance cost that contradicts the panel's event-driven
open/close lifecycle, so the probe thread was retired with its surface.

The status HUD was removed (ADR-0213): the top of the screen belongs to windows,
and the readings it carried now live on the Dock's reading cells and in the
transient system OSD (ADR-0214). Every interaction the old status bar hosted lives
here instead — a full-screen modal overlay in a personal-info language over an
opaque, scheme-adaptive canvas. Solid elevated surfaces use grouped grays, quiet
separators, and system-blue interaction states, with bold sans-serif display
typography. The panel paints that canvas itself — a full-screen solid fill in
the lens `Backdrop` band, faded by the reveal — and requests no backdrop
capture, blur, frost, or liquid glass (ADR-0176), so its enter/exit is a
paint-only fade that never composites the desktop. It opens
through the `Super+S` keybinding or a four-finger touchpad swipe down (both
dispatched by the compositor's main loop), and closes on Escape, a click on
the background, the same keybinding, or a four-finger swipe up.

Surfaces:

- **Quick Controls tab (first)** — the daily toggles: volume (+ mute),
  brightness, always-on (the session idle inhibitor), and do-not-disturb,
  emitted as `SystemAction`s. The Wi-Fi, Bluetooth, and Cast tiles sit on the
  left of the Bento grid as expandable connectivity cards: each exposes a chevron
  that opens an inline detail view (network/device/display list with scan, connect,
  disconnect, and discovery actions) rooted in the daemon-neutral wireless,
  Bluetooth, and casting subsystems (ADR-0162, ADR-0175, ADR-0188, ADR-0192).
  Beside Cast, high-density compact quick action toggles provide instant switching
  for Do Not Disturb and Dark Mode without wasting grid real estate.
- **Settings module tabs** — one tab per available settings module
  (display, input, appearance, power), rendered from the registry inside
  the main panel; their `SettingsAction`s leave through
  `ChromeEvents::settings_actions` with the current snapshot revision.
- **Notifications** — the notification history as a frameless stream of
  items, newest first, with click-to-dismiss and a tail that fades out
  toward the bottom of the region. The queue retains one hour of entries;
  the frameless toast strip presents only a three-second window of them
  (ADR-0083).
- **Clock (top-center)** — locale wall-clock time at hero scale plus the
  weekday and date, frameless.
- **Tray shelf (bottom-left)** — the shared `tessera-shell::tray`
  StatusNotifierItem snapshot as a **one-row horizontal lane** anchored at the
  bottom-left corner (ADR-0221). Its length is **derived, never capped**: it
  runs from the left margin to the bottom band, so capacity scales with the
  display instead of sitting at a constant. When the items exceed the lane the
  shelf **paginates** with a chevron at each end; each end reports its own
  reachability, so the first page's back chevron and the last page's forward
  chevron are drawn dimmed and inert instead of pretending to be live. Hover and
  menu-open are the cell
  pressable's **own** background — never a sibling `place()` overlay, because an
  ABS node in the `Chrome` band occludes the flow widgets inside its subtree for
  hit-testing (Optics ADR-0060 — occlusion *is* the reversed hit-test order), so
  a plate drawn after the shelf silently swallows every cell's hover, left
  click, and right-click menu. Left-click `Activate`, right-click opens the
  host-rendered dbusmenu popover (or `SecondaryActivate` when the item exposes
  no `Menu` object). There is **no scroll viewport** ([INV-CHROME-03]): a page is
  a whole-number slice of the lane, never a fractional scroll offset, and the
  page index is clamped by the resolver so an item leaving can never strand the
  shelf on a page that no longer exists ([INV-CHROME-07]).
- **Work Mode segmented control (right-bottom)** — beside the power knob,
  filling the corner band: a frameless segmented control over the session
  power modes (`Balanced`, `Stay Awake`, `Dashboard Lock`) with a
  spring-driven sliding indicator for a light elastic bounce on switches
  and per-segment hover tooltips describing each mode's idle behavior.
- **Power knob & session cluster (right-bottom)** — the corner's right
  half: a circular power knob — one click requests power-off through the
  system-level confirmation chrome; dragging right pulls out a capsule
  whose far button is reboot, dragging left arms suspend — and a separate
  lock button.

## Layout

A boundless, floating layout over the solid grouped background, each
surface with its own reveal animation:

- **Main Control Panel (Center)** — the primary focal point: a solid capsule
  nav rail (Quick Controls plus one capsule per available settings module)
  beside the active tab's elevated surface; tab bodies scroll when they
  overflow.
- **Identity block (top-left)** — frameless user persona: a 56px avatar orb
  (live 3D VRM rendering and VRMA animation playback) inside a gapped cyan
  line ring, the display name at title scale, `@username · groups`, and the
  machine's hostname from the local account/kernel record.
- **Notifications stream (top-right)** — each notification as its own
  recessed card: a distinct background, hairline border, and 10px gaps so
  individuals read clearly; the tail fades out toward the bottom. The
  queue retains one hour of entries; the frameless toast strip presents
  only a three-second window of them (ADR-0083).
- **Clock (top-center)** — the locale wall-clock time at hero scale with
  the weekday and date beneath, drawn frameless between the identity block
  and the notification stream.
- **Tray shelf (bottom-left)** — StatusNotifierItem icons as a one-row
  horizontal lane whose length is derived from the display and the bottom band
  (ADR-0221): no fixed cap, no scroll viewport, and bounded reversible paging
  when the icons exceed the row. Hover and menu-open are the cell's own
  background plate, left-click activates, right-click opens the dbusmenu popover
  to the icon's right, and a page turn dismisses a popover whose cell left the
  page rather than floating it over the wrong icon.
- **Work Mode segmented control & power cluster (right-bottom)** — one
  horizontal band pinned into the corner: a frameless segmented control
  over the three session power modes, its active segment marked by a
  sliding pill indicator driven by an under-damped spring so switches land
  with a light elastic bounce (hover a segment for its idle-behavior
  tooltip); beside it, a circular power knob — one click requests
  power-off through the system-level confirmation (the same consent chrome
  portals use); dragging right pulls out a capsule whose far button is
  reboot, dragging left arms suspend — and a separate lock button.

## Boundaries

The component owns presentation and interaction state only. System snapshots
arrive through the `Chrome` trait each frame; intents leave through
`tessera_shell::ChromeEvents` and the shared tray command channel. It has no
D-Bus, Wayland, or process authority of its own. The feature-gated
`tessera_shell::persona` module supplies the same profile and ordered photo/VRM
contract used by the lock screen, including the internal VRM rendering
backend. This panel owns the camera composition, flat graphite fallback disc,
keyline, motion trigger, and reveal treatment around that content.
