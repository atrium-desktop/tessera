# tessera-dock

`tessera-dock` is the macOS-style dock chrome component for the tessera compositor,
built on the `Chrome` contract from `tessera-shell` and the shared materials from
`tessera-design`.

## Responsibilities

- Draw a flat bottom-center bar of pinned application icons, led by two leading
  cells: the machine's own readings and, when several workspaces exist, an
  interactive workspace rail (ADR-0218).
- **The reading column** answers the three questions asked most often — which
  network, is the headset up, how much charge — from the surface the hand is
  already moving toward. It sits across the bar's *thickness*, not along its
  length: the thickness is divided into six units, each reading spans
  `6 / n` of them, and no reading may span fewer than two, so one, two, or three
  rows tile the plate exactly and three is both the arithmetic and the
  perceptual ceiling. The column costs one constant span of length whatever it
  holds, so a radio appearing never moves an icon. A reading is drawn only for a
  capability the machine reports; a machine with no radios and no battery keeps a
  pure, centered dock. Each row is a *shortcut*: pressing it opens the control
  center, which already owns the toggles and the device lists.
- When multiple workspaces exist, draw an interactive workspace rail beside the
  readings: a compact, sleek segmented capsule where inactive workspaces are
  subtle dots and the current one is lit. Supports direct click-to-switch and
  mouse-wheel scroll cycling. When only 1 workspace exists, it auto-hides with
  zero footprint.
- Hold one distance from its screen edge in both states, so the dock grows and
  shrinks in place instead of sliding toward the edge as it parks. Its reveal
  corridor is composed from that distance, the capsule's own body, and the
  approach band, so a pointer thrown at the physical screen edge still summons a
  surface that floats clear of it.
- Veto its own collapse while a gesture it started is unfinished: a drag in
  flight, an open context menu, or a held press.
- Fold running windows into their pinned tile by matching `app_id` against
  each entry's `Entry::match_keys`.
- Magnify the tile under the cursor and its neighbours with a damped-spring
  reflow wave, and grow new tiles in from a seed size.
- Offer the shared application context menu, including pin/unpin actions and
  the maximize/restore and always-on-top lifecycle rows.
- Emit launch, focus, pin-toggle, and open-control-center intents through
  `ChromeEvents`.

## Boundaries

The dock owns presentation and interaction state only. The composition root
(the `tessera` binary) resolves the pinned entries and owns the decoded icon
textures; the dock receives both as borrowed snapshots pushed through
`ChromeUpdate::AppCatalog`. It never mutates Wayland state, spawns
processes, or writes configuration.

The reading column is a *reading*: it draws whatever the host last pushed through
`ChromeUpdate::SystemStatus` and never infers a state from chrome's own
behaviour. It owns no threshold — battery warning levels are `[battery] warn_at`
policy applied by the runtime (ADR-0214) — and its press emits an open-only
intent rather than toggling a surface.

## Runtime Effect

Each frame the dock reports its reserved bottom edge, its painted damage
envelope, and whether its magnification springs still animate. Pin toggles are drained
by the main loop into the `[dock] pinned` configuration, which answers with a
refreshed application catalog. A reading press is drained into
`Shell::open_control_center`, which is deliberately open-only: pressing a reading
while the panel is open must not close it.

## Use

Push the application catalog, then register one dock with the shell:

```rust
shell.set_app_catalog(catalog);
shell.add(Box::new(tessera_dock::Dock::new()));
```

`Shell::add` seeds newly registered components with the current catalog and the
current system status, and `Shell::set_app_catalog` / `Shell::set_system_status`
fan replacements out to every component.

## Related Documentation

- [Dock and launcher operations](../../../../../docs/how-to/dock-and-launcher.md)
- [Chrome component contract](../../../../../docs/adr/0021-chrome-component-trait.md)
- [The dock's reading column](../../../../../docs/adr/0218-dock-reading-column-is-a-six-unit-bento-and-status-lives-on-the-dock.md)
- [Flat chrome and removal of the status HUD](../../../../../docs/adr/0213-flat-chrome-and-removal-of-the-status-hud.md)
- [Design system decision](../../../../../docs/adr/0046-design-system-crate.md)
