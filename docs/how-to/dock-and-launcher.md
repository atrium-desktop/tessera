# How to Use the Dock, Launcher, and Prism

Start applications, switch between their windows, and use app-level window
actions from the compositor chrome.

## Identify a Dock Application

1. Move the pointer over an application icon in the Dock.
2. Keep the pointer on that icon for about 300 milliseconds.
3. Read the application name above the animated icon.

The name follows the icon while the Dock magnification settles. Moving to a
different icon restarts the delay, and leaving the Dock hides the name. The
full-screen launcher keeps application names below their cells instead of
using this hover label.

## Reveal a Hidden Dock

When `[dock] autohide` is on, or a maximized window covers the Dock's edge,
the Dock hides completely and leaves nothing on screen. Its edge Hot Zone
brings it back:

1. Move the pointer toward the middle of the Dock's screen edge. A thin tab
   flush with the edge fades in as you approach.
2. Rest the pointer against the edge for about 140 milliseconds. The Dock
   slides in. A quick sweep along the edge does not reveal it.
3. Move onto the Dock and use the required application tile.
4. To dismiss, move the cursor away (the Dock hides after a brief delay),
   click anywhere outside the Dock, or press `Escape`.

A maximized window forces this hidden mode regardless of `[dock] autohide`.
Moving the pointer back off the edge during the reveal cancels it. Only a
narrow strip against the screen edge triggers, so a window's own buttons near
that edge stay clickable.

A fullscreen window silences every Hot Zone, so the Dock cannot be summoned
until fullscreen ends. Minimized windows do not affect either policy.

The edge, the approach distance, and the dwell are configurable. See
[Hot Zones](../reference/config.md#hot-zones) in the Configuration Reference.

## Check Status at a Glance

The Dock holds only applications. To see the time, your workspace, and the
machine's Wi-Fi, Bluetooth, battery, volume and Do Not Disturb state, raise the
Glance HUD. See [How to Read Status and Use the Control
Center](hud-and-control-center.md#glance-at-status).

## Open the Launcher

Use any interaction:

- Click the leading `Applications` tile in the Dock.
- Press the default `Super+A` key binding.

Type to filter applications. Use the arrow keys to move through the grid,
press `Enter` to activate the selected application, or click an application
cell. Scroll or swipe to move between result pages: a mouse-wheel detent
turns one page, and a two-finger touchpad swipe needs a deliberate flick
(keep moving to walk further pages). Press `Escape` to close
the launcher.

See the [Configuration Reference](../reference/config.md#default-key-bindings)
to change the configurable launcher binding.

## Search with Prism

1. Press `Super+Space`.
2. Type part of an application name, description, desktop ID, or keyword.
3. Use `Up` and `Down` to select a result.
4. Press `Enter` to start the application or focus its running window.

Prism shows a compact result list without expanding the full application
library. Press `Escape`, press `Super+Space` again, or click outside the panel
to close it. Opening Prism closes the full launcher, and opening the launcher
closes Prism.

### Preview and Debug Routing

For development, visual inspection, and verification without input gestures or keybindings:

```bash
TESSERA_PREVIEW=pivot cargo run -p tessera
```

This runs a fully mocked, zero-side-effect component sandbox that boots directly into the Pivot search surface on startup and isolates state in a disposable temporary directory.

## Start or Focus an Application

1. Select an application in the Dock, launcher, or Prism.
2. If the application has a running window, wait for tessera to focus and raise
   that window.
3. If the application is not running, wait for tessera to start it.

Use the application menu when an application has several windows and you
need to choose one explicitly.

## Pin or Unpin a Dock Application

The Dock splits into two sections: pinned applications on the left, and
applications that are running but not pinned on the right of a divider. Both
sections show one tile per application, however many windows it has. A
transient tile disappears again when its last window closes.

An unconfigured Dock contains only the leading `Applications` tile. Start an
application from the launcher to make its transient tile available for
pinning.

Use the application menu:

1. Right-click a Dock tile.
2. Select `Keep in Dock` to pin a transient application, or `Remove from Dock`
   to unpin a pinned one.

Or drag the tile across the divider:

1. Press a tile and drag it. Neighbouring tiles shift aside to preview the
   landing slot.
2. Drop a transient tile inside the pinned strip to pin it at that position.
   Drop a pinned tile past the divider to unpin it; if it is still running,
   it stays as a transient tile.

Both paths write the change back to the `[dock] pinned` list in the
[Configuration Reference](../reference/config.md#dock). To opt into automatic
selection, empty the list and set `autopopulate = true`.

## Use the Application Menu

1. Right-click an application icon or launcher cell.
2. Select a window title to focus or restore that window.
3. Select `Open` or `New Window` to start another instance.
4. Select `Minimize Window` or `Minimize All Windows` to hide windows while
   keeping their clients alive.
5. Select `Always on Top` to keep a window above every normal window, or
   `Not Always on Top` to release it.
6. Select `Close Window` or `Close All Windows` to request a graceful close.

The Maximize/Restore and Always on Top rows appear only in the Dock menu, and
only while the application has exactly one window that is not a read-only
mirror, minimized, or fullscreen. With several windows, the menu cannot know
which one you mean, so it offers only application-wide actions. Use the window
controls on its live preview cards instead (see below). Windows kept on top
show `▲` after their title in the menu's window list.

An always-on-top window stays above normal windows but below compositor
chrome such as the Dock. The flag lasts until you clear it or the session
ends. Press `Super+Shift+T` to toggle it for the focused window.

The menu is anchored to the owning application icon or cell rather than the
pointer position. A Dock menu opens above its icon when space permits and
freezes the current Dock magnification so the menu does not move while you
use it. Menus near an output edge move or change side to remain visible.

Minimized windows remain in the menu. Select a minimized window title to
restore and focus it. An application with several windows lists each window
before the application-wide actions.

Press `Escape` or click outside the menu to dismiss it. When the menu is open
inside the launcher, the first `Escape` dismisses the menu and leaves the
launcher open; press `Escape` again to close the launcher.

## Control One Window from Its Live Preview

Rest the pointer on a running application's Dock tile to open live previews
of its windows. The previews stay open while you move from the tile toward
them, even when the path crosses neighbouring tiles.

1. Hover a preview card to show its window controls in the top-right corner.
2. Select `▲` to keep that window on top, or select it again to release it.
3. Select `×` to request a graceful close of that window.
4. Right-click the card for a menu that acts on that window only.

A window that is kept on top shows a lit `▲` badge on its card even when you
are not hovering it, so you can tell several windows of one application apart.

## Manage a Borderless Application Window

Use compositor gestures when an application does not provide its own title
bar or resize frame. Follow [How to Manage Borderless
Windows](window-management.md) for the move and resize procedures.
