# tessera-settings

Tessera Settings: the standalone application that edits the compositor's
persistent settings ([ADR-0238](../../docs/adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md)).

```bash
tessera-settings            # open Settings
tessera-settings display    # open Settings on the Display page
```

A running window receives the page and raises itself instead of a second
window opening.

## Layout

- **Library** (`src/lib.rs`): everything that needs no display server.
  - `module` — the `SettingsModule` contract and `ModuleRegistry`: stable ids
    (also the deep-link segment), localized titles, icons, categories, search
    keywords, apply policy, and backend availability.
  - `catalog` — the built-in modules in sidebar order: `display`, `input`,
    `power`, `appearance`, `dock`, `accessibility`, and the not-yet-backed
    `users` and `window-rules` pages, which say so honestly.
  - `host` — `SettingsHost`, the window's model and layout: categorized
    sidebar (an icon rail below 640 px), search over titles, ids, and
    keywords, the page column, the link-state line, and the rejection
    banner. It also owns the outbox, which keeps the newest draft per action
    kind and holds drafts while the link is down.
- **Binary** (`src/main.rs`, `src/app/`): the Wayland window and its plumbing.
  - `app/mod.rs` — one `xdg_toplevel` with server-side decorations,
    fractional scale, cursor shapes, and seat input.
  - `app/gpu.rs` — Flux/Lens rendering of the host into the window.
  - `app/input.rs` — seat events folded into one Lens input per frame.
  - `app/clipboard.rs` — the standard Wayland clipboard behind Lens text
    fields (`Ctrl+C`, `Ctrl+X`, `Ctrl+V`).
  - `app/link.rs` — the compositor link (below).
  - `app/instance.rs` — the per-session single-instance socket.

## Boundaries

Settings reaches the compositor only over `$XDG_RUNTIME_DIR/tessera.sock`;
it has no Rust dependency on the compositor. Modules own presentation and
local drafts and never read configuration, probe hardware, or touch a
transport.

The link keeps two connections. The **command** connection claims the
built-in `tessera-settings` scope, fetches snapshots, commits typed
`SettingsAction`s with the observed revision, and renews its lease. The
**watch** connection subscribes to events and turns every `SettingsChanged`
into a refresh, so edits made elsewhere reach the open window. Both reconnect
with bounded backoff.

The compositor grants the scope to `/usr/bin/tessera-settings`,
`/usr/local/bin/tessera-settings`, and — in a debug build — the
`tessera-settings` binary beside the running compositor;
`[ipc.scope_executables]` replaces that list. The scope permits settings
transactions and focuses only a Settings-owned window, which is how a second
invocation raises the running one. A build the compositor refuses opens
**read-only**: it shows live settings and refuses edits visibly.

## Keyboard

| Key | Action |
|-----|--------|
| `Ctrl+F` | Focus the search field |
| `Enter` (in search) | Open the first result |
| `Escape` | Clear the search |
| `Ctrl+C`, `Ctrl+X` | Copy or cut the selected text |
| `Ctrl+V` | Paste text from the Wayland clipboard |
| `Ctrl+W`, `Ctrl+Q` | Close the window |

## Related Documentation

- [Settings Reference](../../docs/reference/settings.md)
- [First-Party Application Development](../../docs/dev/first-party-applications.md)
- [IPC Reference: persistent settings](../../docs/reference/ipc.md#persistent-settings)
