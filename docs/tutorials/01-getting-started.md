# Getting Started with Tessera

This tutorial builds Tessera, starts a nested compositor, opens the command
panel's settings tabs, and queries compositor state.
Run every command from the Tessera repository root inside an existing Wayland
session.

## Prerequisites

Install:

- Rust 1.88 or later;
- Meson, Ninja, `pkg-config`, a C23 compiler, libclang, and `slangc` (Slang
  2026.17+; recent Vulkan SDKs bundle it);
- Vulkan, Wayland, xkbcommon, libinput, and libseat development files; and
- the Optics `v<OPTICS_VERSION>` native libraries.

The independently sourced
[xdg-desktop-portal-atrium](https://github.com/aegis-shell/xdg-desktop-portal-atrium)
additionally requires PipeWire and SPA development files. The nested
compositor tutorial does not clone, build, or install that package.

Build and install the matching Optics release when the distribution does not
provide it:

```bash
git clone --branch "v<OPTICS_VERSION>" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion flux vista lens iris reticle
```

Each version printed by the final command must be exactly `<OPTICS_VERSION>`:
the pre-1.0 bindings and SONAME reject a different native release.

## Start the Nested Compositor

Build the two supervised session clients, then run:

```bash
cargo build --locked -p tessera-idle -p tessera-lock
cargo run --locked --profile release-fast -p tessera
```

The compositor is a real-time program. The `release-fast` profile keeps full
symbols but skips LTO, so it builds quickly enough for development while still
holding a 120 Hz 5 MP frame budget. Use `TESSERA_DEV_PROFILE=debug` with
`./scripts/run-nested-dev.sh` only when you need a debugger.

Because `WAYLAND_DISPLAY` is set, the default `TESSERA_BACKEND=auto` opens a
nested Tessera window. The background and the Dock confirm that the
session is ready.

## Open the Settings Tabs

Press `Super+S` inside the nested session. The command panel opens over a
dark blurred scrim. Select the **Display** or **Touchpad** tab in the main
panel's tab bar: the settings page renders in place and its edits commit
through the compositor's revisioned settings transaction. This verifies the
panel chrome, the hosted settings modules, and the commit path together.

## Query Compositor State

From a second terminal in the same user session, run:

```bash
cargo run --locked -p tessera -- display
cargo run --locked -p tessera -- window
```

Both commands print state from the running compositor. Stop Tessera with
`Ctrl+C`.

## Next Steps

- Use the [daily-use guides](../how-to/index.md) for window management,
  configuration, and display setup.
- Read the [Architecture](../explanation/architecture.md) for the component
  and security model.
