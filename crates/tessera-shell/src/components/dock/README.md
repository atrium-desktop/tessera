# tessera-dock

`tessera-dock` is the pure application dock chrome component for the tessera
compositor, built on the `Chrome` contract from `tessera-shell` and the shared
materials from `tessera-design` (ADR-0254, ADR-0255, ADR-0256).

## Responsibilities

- Draw a clean, symmetric bar of application tiles only — Launchpad, pinned
  applications, and transient running applications — with no system telemetry
  or workspace indicators (`[INV-DOCK-PURE-01]`).
- Support all four screen edges (`Top`, `Bottom` horizontal; `Left`, `Right`
  vertical), declaring `HotZoneSlotMask::EDGES` and `HotZoneTrigger::Hover`
  through `Chrome::hot_zone_capability`.
- Reveal and hide via a critically damped edge slide and opacity fade summoned
  by the unified eight-zone Hot Zone system (`ChromeCommand::SummonDock`), with
  zero collapsed-capsule footprint when hidden (`[INV-HOTZONE-05]`).
- Magnify tiles under the pointer with a damped-spring reflow wave and support
  drag-to-reorder, drag-to-pin/unpin, and drag-to-edge repositioning.
- Protect diagonal pointer transit into multi-instance Live Previews with a
  trapezoidal Safe Corridor, Safe Triangle neighbour freeze, retreat-latch
  suppression, and exit grace window (`[INV-DOCK-PREVIEW-01]`).
- Expose unambiguous window-level controls (Always-on-Top Pin toggle, persistent
  Pin badge, Close button, and right-click single-window menu) directly on each
  Live Preview card (`[INV-DOCK-PREVIEW-02]`).

## Related Documentation

- [ADR-0254: Dual-Channel Reachability and Component-Mounted Eight-Zone Hot Zones](../../../../../docs/adr/0254-dual-channel-reachability-and-component-mounted-hot-zones.md)
- [ADR-0255: Pure Dock and Transient Glance HUD](../../../../../docs/adr/0255-pure-dock-and-transient-glance-hud.md)
- [ADR-0256: Dock Preview Safe Corridor and Window-Level Card Actions](../../../../../docs/adr/0256-dock-preview-safe-corridor-and-window-level-actions.md)
