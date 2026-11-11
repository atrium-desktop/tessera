# Tessera

Tessera is a Wayland compositor and desktop shell for Linux. It combines a
Vulkan-first renderer, native shell surfaces, panel-hosted system settings,
and scoped AI-agent workspaces behind explicit process and security
boundaries.

## Capabilities

- Vulkan rendering through the
  [Optics](https://github.com/ming2k/optics) stack
- Native status bar, Dock, full application launcher, Prism search,
  notifications, and multi-workspace window management
- Multi-output session lock, staged idle policy, lock-before-sleep, and
  physical display power management
- Nested Wayland development and direct DRM/KMS presentation
- Versioned local IPC with a command-line client and desktop portal backend
- Isolated Agent Interaction Domains with cgroup and capability boundaries,
  observation-bound actions, scoped physical input mediation, and hash-chained Actor audit

## Quick Start

The safest first run is a nested session inside an existing Wayland desktop.
Tessera requires Rust 1.88 or later and the native build dependencies listed in
the [Getting Started tutorial](docs/tutorials/01-getting-started.md).

Install the Optics `v<OPTICS_VERSION>` native libraries from a distribution package or
build the matching source release. Building Optics needs a C23 compiler,
Meson, and `slangc` (Slang 2026.17+; recent Vulkan SDKs bundle it):

```bash
git clone --branch "v<OPTICS_VERSION>" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion flux vista lens iris svg
```

From the Tessera repository root, start the compositor. Use the
`release-fast` profile for real desktop use: the debug profile is for stepping
through code, and its `opt-level = 1` cannot hold a 120 Hz 5 MP frame budget.

```bash
cargo build --locked -p tessera-idle -p tessera-lock
cargo run --locked --profile release-fast -p tessera
```

For a debug session, prefer `./scripts/run-nested-dev.sh` (it defaults to
`release-fast`) and set `TESSERA_DEV_PROFILE=debug` only when a debugger is
the point.

Nested output is composited by the **outer** session. If the outer compositor
is an installed build older than the source tree, refresh artifacts can
originate there even when the nested compositor is correct. Validate damage or
pacing fixes on the direct DRM session (a spare VT), or restart the outer
session on the same build.

`TESSERA_BACKEND=auto` is the default. A terminal with `WAYLAND_DISPLAY` set
opens a nested window; a login on a bare TTY selects direct DRM/KMS.
When developing nested inside an active Tessera session, specify an isolated
`XDG_DATA_HOME` (e.g. `XDG_DATA_HOME=/tmp/tessera-dev XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share`)
to prevent audit store lock contention with the host compositor.

### How the C libraries are selected

The Rust bindings (`flux`, `lens`, `transit`, `svg`, ...) are ordinary crates
compiled into the compositor binary. The C implementations are **shared
libraries**. The patched `*-sys` build scripts auto-discover the sibling
`../optics` checkout and prepend its `build/meson-uninstalled` directory to
`pkg-config`, then emit an RPATH to `build/libs/*`. A source-tree `cargo run`
therefore uses the freshly built C libraries, even though the system-wide
`pkg-config --modversion flux` still reports the installed release.

Set `FLUX_USE_INSTALLED=1` to link the system pkg-config database instead.
Pre-1.0 the bindings require the **exact** `X.Y.Z` of the C library: a
mismatch fails the build (or the dynamic loader, because the SONAME carries the
full pre-1.0 version) instead of silently reading a different descriptor
layout. A running compositor keeps the library inode it mapped at startup;
restart it after rebuilding Optics.

Source-tree Cargo commands do not install systemd, D-Bus, portal, desktop, or
icon metadata. The D-Bus-activated portal backend is built from the
independent
[xdg-desktop-portal-atrium repository](https://github.com/aegis-shell/xdg-desktop-portal-atrium)
and distributed as a compatibility-mapped optional package; the core
compositor runs without it. An installed core package can start the
compositor as a user service:

```bash
systemctl --user start --wait tessera.service
```

Greeters and TTY logins should use the `tessera-session` wrapper instead; it
additionally manages the login environment and the `graphical-session.target`
lifecycle.

## Controls

| Action | Shortcut or command |
|--------|---------------------|
| Open Applications | Click Launchpad or press `Super` |
| Open System Settings | Press `Super+S` and select a settings tab |
| Lock the session | Press `Super+L` |
| Inspect compositor state | Run `tessera window` |

## Documentation

- [Documentation home](docs/index.md)
- [Getting Started](docs/tutorials/01-getting-started.md)
- [Daily-use guides](docs/how-to/index.md)
- [Configuration reference](docs/reference/config.md)
- [Architecture](docs/explanation/architecture.md)
- [Agent Workspaces](docs/how-to/ai-workspaces.md)

## License

Project source code is licensed under the [MIT License](LICENSE). The
bundled cursor theme under `assets/cursors/` is original art generated
in-tree by `scripts/prepare-tessera-cursors.py` and is MIT-licensed like the
rest of the project.
