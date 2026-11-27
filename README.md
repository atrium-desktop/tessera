# Tessera

Tessera is a Wayland compositor and desktop shell for Linux. It combines a
Vulkan-first renderer, native shell surfaces, a standalone Settings
application, and scoped AI-agent workspaces behind explicit process and
security boundaries.

## Capabilities

- Vulkan rendering through the
  [Optics](https://github.com/ming2k/optics) stack
- Native Dock, full application launcher, Prism search, notifications, the
  Control Center, and multi-workspace window management
- Extend or mirror physical displays, with automatic topology for a newly
  attached TV or monitor
- Multi-output session lock, staged idle policy, lock-before-sleep, and
  physical display power management
- Nested Wayland development and direct DRM/KMS presentation
- Versioned local IPC with a command-line client and desktop portal backend
- Isolated Agent Interaction Domains with cgroup and capability boundaries,
  observation-bound actions, scoped physical input mediation, and hash-chained Actor audit

## Quick Start

The safest first run is a nested session inside an existing Wayland desktop.
Tessera requires Rust 1.89 or later and the native build dependencies listed in
the [Getting Started tutorial](docs/tutorials/01-getting-started.md).

Install the Optics `v0.0.88` native libraries from a distribution package or
build the matching source release. Building Optics needs a C23 compiler,
Meson, and `slangc` (Slang 2026.17+; recent Vulkan SDKs bundle it):

```bash
git clone --branch "v0.0.88" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion optics-gfx optics-ui optics-text
```

From the Tessera repository root, start the compositor. Use the
`release-fast` profile for real desktop use: the debug profile is for stepping
through code, and its `opt-level = 1` cannot hold a 120 Hz 5 MP frame budget.

```bash
cargo build --locked -p tessera-idle -p tessera-lock \
  -p tessera-network-system -p tessera-cast
cargo run --locked --profile release-fast -p tessera
```

The additional packages build the supervised companion binaries
(`tessera-networkd` and `tessera-castd`); the compositor locates them as
trusted siblings. Without `tessera-networkd` the Control Center reports the
network service as unavailable instead of showing stale state.

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

The Rust bindings (`optics-gfx`, `optics-ui`, `optics-motion`, ...) are ordinary crates
compiled into the compositor binary. The installer builds Optics with
`-Ddefault_library=static` and sets `OPTICS_STATIC=1`, so released Tessera
executables embed the archives (`liboptics_gfx.a`, `liboptics_ui.a`,
`liboptics_text.a`) directly, while OS
dependencies (`libvulkan`, `freetype`, `harfbuzz`, `fontconfig`, `fribidi`,
`libpng16`, `libturbojpeg`, `libwebp`, `librsvg-2`, `cairo`, etc.) remain
dynamically linked. A development build follows the link mode the
bindings select: archives on the pinned static-only tree, and the shared
objects from newer Optics sources.

The patched `*-sys` build scripts auto-discover the sibling `../optics` checkout
and prepend its `build/meson-uninstalled` directory to `pkg-config`. A
source-tree `cargo run` therefore links the libraries from `../optics/build`
without requiring a system Optics install; when the bindings choose the shared
objects, the emitted RPATH points at the same tree.

Set `OPTICS_GFX_USE_INSTALLED=1` (or the per-crate `OPTICS_*_USE_INSTALLED=1` variables) to
link against the system pkg-config database instead. `OPTICS_STATIC=1` requires
static archives and `OPTICS_DYNAMIC=1` forces the shared libraries with an
RPATH. Pre-1.0 the bindings require the **exact** `X.Y.Z` of the C library: a
mismatch fails the build instead of silently reading a different descriptor
layout.

Source-tree Cargo commands do not install systemd, D-Bus, portal, desktop, or
icon metadata. The D-Bus-activated portal backend is built from the
independent
[xdg-desktop-portal-atrium repository](https://github.com/atrium-desktop/xdg-desktop-portal-atrium)
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
| Open Settings | Press `Super+S`, then ⚙, or run `tessera-settings` |
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
bundled SVG cursor themes under `assets/cursors/` are original art maintained
in-tree and are MIT-licensed like the rest of the project.
