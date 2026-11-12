# Build & Run Guide

This document is the authoritative developer guide for compiling, running, and
verifying Tessera across nested desktop sessions, surface preview harnesses,
and bare-metal DRM/KMS hardware.

---

## 1. Prerequisites & Toolchain Setup

Tessera requires Rust 1.88+ and the native Optics graphics stack (`flux`, `vista`, `lens`, `iris`, `reticle`):

```bash
rustc --version                  # rustc >= 1.88
pkg-config --modversion flux vista lens iris reticle
```

When maintaining Tessera and Optics together in this `tessera-dev` worktree, ensure
`.cargo/config.toml` links against the sibling uninstalled Meson tree:

```bash
cp .cargo/optics-local.toml .cargo/config.toml
```

---

## 2. Compiling the Subsystems

Always compile the session sibling sidecars (`tessera-idle` and `tessera-lock`) into
the target directory so the compositor discovers them via `trusted_sibling_program`:

```bash
# Compile companion binaries
cargo build --locked -p tessera-idle -p tessera-lock

# Compile core compositor
cargo build --locked -p tessera
```

For release builds on hardware:

```bash
cargo build --locked --release -p tessera
```

---

## 3. Running the Compositor

### A. Nested Wayland Session (Standard Development Mode)

Run Tessera as a regular client window inside an existing graphical session (GNOME, Sway, Hyprland):

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_COMMAND_PANEL_OPEN=1 \
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

**Key Parameters**:
- `TESSERA_BACKEND=nested`: Forces nested presentation inside the existing host display server.
- `XDG_DATA_HOME=/tmp/tessera-dev`: Prevents `AuditError::Locked` contention with the host compositor.
- `TESSERA_COMMAND_PANEL_OPEN=1`: Automatically opens the Command Panel at startup for fast UI iteration.

### B. Inner Client Injection

While the nested compositor is running, open a separate terminal:

```bash
# Query inner display protocols
WAYLAND_DISPLAY=wayland-1 wayland-info

# Launch an inner terminal
WAYLAND_DISPLAY=wayland-1 foot
```

### C. Direct Bare-Metal DRM/KMS Session

To boot on physical display hardware:
1. Switch to a bare Linux TTY (`Ctrl+Alt+F3`) and log in.
2. Launch the release binary:
   ```bash
   TESSERA_BACKEND=drm RUST_LOG=info target/release/tessera
   ```

---

## 4. Standalone Surface Previews (Zero Compositor)

Test UI surfaces in isolation without launching the full display compositor:

```bash
# Lock Screen Preview
cargo run --locked -p tessera-lock --features dev-preview --bin tessera-lock-preview

# Persona / 3D Avatar Debug Dump
TESSERA_AVATAR_DEBUG_DUMP=/tmp/tessera-avatar.png \
cargo run --locked -p tessera-shell --features persona --example debug_avatar
```

---

## 5. Automated Verification & Testing

Verify changes before opening a pull request:

```bash
# Flux-free rapid logic tests (zero C / GPU dependencies, sub-second)
cargo nextest run --locked \
  -p tessera-desktop -p tessera-primitives -p tessera-authority -p tessera-audit \
  -p tessera-config -p tessera-ipc -p tessera-protocol -p tessera-launch-services \
  -p tessera-env -p tessera-cli -p tessera-mcp -p tessera-idle -p tessera-wayland

# Full workspace test suite
cargo nextest run --locked --workspace

# Lint & static analysis
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
```

---

## 6. Common Diagnostics

- **Address in Use (`tessera.sock`)**: When running nested alongside an active host Tessera session, the nested instance logs a harmless warning and runs cleanly without IPC.
- **Audit Store Lock (`events-v2.jsonl`)**: Always set `XDG_DATA_HOME=/tmp/tessera-dev` during nested runs.
