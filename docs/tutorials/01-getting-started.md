# Getting Started with Tessera

This tutorial builds Tessera, starts a nested compositor, opens the Control
Center and Settings, and queries compositor state.
Run every command from the Tessera repository root inside an existing Wayland
session.

## Prerequisites

Install:

- Rust 1.89 or later;
- Meson, Ninja, `pkg-config`, a C23 compiler, libclang, and `slangc` (Slang
  2026.17+; recent Vulkan SDKs bundle it);
- Vulkan, Wayland, xkbcommon, libinput, and libseat development files; and
- the Optics `v0.0.82` native libraries.

The independently sourced
[xdg-desktop-portal-atrium](https://github.com/atrium-desktop/xdg-desktop-portal-atrium)
additionally requires PipeWire and SPA development files. The nested
compositor tutorial does not clone, build, or install that package.

Build and install the matching Optics release when the distribution does not
provide it:

```bash
git clone --branch "v0.0.82" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion optics-gfx optics-scene optics-ui optics-svg optics-text
```

Every version printed by that command must be exactly `0.0.82` (the tag is
`v0.0.82`; `pkg-config` reports the version without the prefix). The pre-1.0
bindings and SONAME reject a different native release.

## Start the Nested Compositor

Build the supervised session clients and companion services, then run:

```bash
cargo build --locked -p tessera-idle -p tessera-lock \
  -p tessera-network-system -p tessera-cast
cargo run --locked --profile release-fast -p tessera
```

`tessera-network-system` provides `tessera-networkd`, and `tessera-cast`
provides `tessera-castd`; the compositor supervises both as trusted siblings.
Without them the corresponding Control Center pages report the service as
unavailable rather than fabricating state.

The compositor is a real-time program. The `release-fast` profile keeps full
symbols but skips LTO, so it builds quickly enough for development while still
holding a 120 Hz 5 MP frame budget. Use `TESSERA_DEV_PROFILE=debug` with
`./scripts/run-nested-dev.sh` only when you need a debugger.

Because `WAYLAND_DISPLAY` is set, the default `TESSERA_BACKEND=auto` opens a
nested Tessera window. The background and the Dock confirm that the
session is ready.

## Open the Control Center and Settings

Press `Super+S` inside the nested session. The Control Center opens as a
sheet against the right edge, with the desktop still visible. Its ⚙ button
opens Settings, whose edits commit through the compositor's revisioned
settings transaction. A debug compositor recognizes the sibling development
binary; if the compositor and Settings come from different build directories,
allowlist the binary through `[ipc.scope_executables]` in the
[Configuration Reference](../reference/config.md#ipc).

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
