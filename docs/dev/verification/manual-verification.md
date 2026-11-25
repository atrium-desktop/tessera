[Verification Index](index.md) > Manual Verification Playbook

---

# Manual Verification Playbook

Scenario-based recipes for running Tessera during development and checking a
change by hand, ordered by the
[verification ladder](../standards/verification.md#2-the-verification-ladder).
Every block is copy-paste ready from the repository root.

For first-time build and startup, see the [Build & Run Guide](../build-and-run.md).
For area-specific depth, see [Surface Verification](surface-verification.md)
and [VT/DRM Verification](vt-drm-verification.md). Record results in the
[Support Matrix](support-matrix.md).

---

## 1. Scenario Decision Matrix

Select the cheapest scenario that can fail for your change:

| Verification Target | Recommended Scenario | External Dependencies | Execution Overhead |
| :--- | :--- | :--- | :--- |
| **Logic, protocol codecs, schemas, config, IPC, audit** | **[Scenario 0: Native-Free Logic](#2-scenario-0-native-free-rapid-logic-verification-zero-c-dependencies)** | None (Pure Rust toolchain only) | Sub-second |
| **Lock screen UI, avatar, credential interaction** | **[Scenario 1A: Lock Screen Preview](#1a-lock-screen-preview)** | Wayland host only (Zero compositor required) | Instant window |
| **3D Persona avatar rendering & framing** | **[Scenario 1B: Avatar Preview](#1b-3d-persona--vrm-avatar-preview)** | GPU (Headless snapshot dump) | Instant dump |
| **Settings UI, presets, compact rail, fault states** | **[Scenario 1D: Settings Preview](#1d-settings-preview-wire-level-mock-ipc)** | Wayland host only (Zero compositor required) | Instant window |
| **Compositor runtime, window management, chrome, layout** | **[Scenario 2: Nested Compositor](#4-scenario-2-nested-compositor-inside-an-existing-desktop)** | Full host Wayland desktop + Optics C libraries | Single isolated client window |
| **Inner Wayland client protocols, dmabuf, xdg-shell** | **[Scenario 3: Inner Client Injection](#5-scenario-3-inner-client-injection--protocol-checks)** | Running nested instance | Client process |
| **Physical KMS scanout, VT switching, multi-GPU** | **[Scenario 4: Direct VT/DRM](#6-scenario-4-direct-vtdrm-hardware-session)** | Physical Linux TTY + rootless seat | Bare-metal session switch |
| **Full workspace unit & doc suite** | **[Scenario 5: Full Workspace Test Suite](#7-scenario-5-full-workspace-test-suite)** | Full native stack + Optics | ~10-20 seconds |

---

## 2. Scenario 0: Native-Free Rapid Logic Verification (Zero C Dependencies)

### Purpose

Validate core business logic, protocol serializations, audit chains, and config
parsing without installing or building the native C23/Meson Optics stack (`optics-gfx`,
`optics-scene`, `optics-ui`, `optics-svg`, `optics-text`).

### Execution Command

```bash
cargo nextest run --locked \
  -p tessera-desktop -p tessera-primitives -p tessera-authority -p tessera-audit \
  -p tessera-config -p tessera-ipc -p tessera-protocol -p tessera-launch-services \
  -p tessera-env -p tessera-control -p tessera-mcp -p tessera-idle \
  -p tessera-wayland

# Root integration/e2e suites with native modules filtered out
cargo nextest run --locked -p tessera-tests --no-default-features
```

### Essential Parameter Explanations

- **Zero C Library Requirement**: Targets only crates decoupled from Optics bindings,
  running cleanly in any environment with only Rust installed.
- **Protocol Code Generation**: To test `tessera-wayland-protocols` code generation,
  simply run `cargo build --locked -p tessera-wayland-protocols`.

---

## 3. Scenario 1: Fast Surface Previews (Zero Compositor)

### 1A. Lock Screen Preview

Test lock screen layout, avatars, input animations, and credential handling
without touching PAM or locking the actual host session:

```bash
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview
```

- Type any text and press `Enter` to simulate successful authentication.
- Press `Escape` to close the preview window.
- `Backspace` and `Ctrl+U` test production credential buffer edits.
- Pass `--background <path>` to test independent wallpaper rendering.
- For deep harness conventions, see [Surface Verification](surface-verification.md).

### 1B. 3D Persona & VRM Avatar Preview

Inspect VRM rendering and camera framing:

```bash
TESSERA_AVATAR_DEBUG_DUMP=/tmp/tessera-avatar.png \
cargo run -p tessera-shell --features persona --example debug_avatar
```

View the generated snapshot at `/tmp/tessera-avatar.png`.

### 1C. Automated Headless Surface Capture for Agents

While a preview window is running, take a pixel-exact cropped snapshot via CLI
without GUI screenshot utilities:

```bash
windows=$(cargo run --locked -p tessera -- window -j)
preview_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.LockPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- display capture \
  --region "$preview_region" \
  /tmp/tessera-lock-preview.png
```

### 1D. Settings Preview (Wire-Level Mock IPC)

Inspect Settings pages, responsive `<640px` rail layout, multi-display/touchpad controls, and rejection banners without starting a compositor ([ADR-0257](../../adr/0257-explicit-ipc-socket-override-and-settings-preview-harness.md)):

```bash
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview -- --preset multi-display display
```

- Presets: `--preset desktop|multi-display|laptop|minimal`
- Fault/authority modes: `--mode live|read-only|reject-stale|reject-locked|disconnected`
- Fixed sizes: `--size 540x720` (compact icon rail), `--size 960x680` (standard), `--size 1440x900` (wide)
- Run `--mock-server-only --socket /tmp/mock-settings.sock` to test an unmodified production `tessera-settings --socket /tmp/mock-settings.sock` or `tessera --socket /tmp/mock-settings.sock`.
- For full harness options and agent capture recipes, see [Surface Verification](surface-verification.md#verify-settings).

---

## 4. Scenario 2: Nested Compositor Inside an Existing Desktop

### Purpose

Run the production Tessera compositor as a regular client window inside
an existing graphical Wayland session (GNOME, Sway, Hyprland, or a host Tessera
desktop).

### Prerequisites

Build the supervised session binaries once into the same target directory so
the compositor's runtime discovery (`trusted_sibling_program`) finds them:

```bash
cargo build --locked -p tessera-idle -p tessera-lock
```

### Execution Command

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_PREVIEW=control-center \
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

### Essential Parameter Explanations

- `TESSERA_BACKEND=nested`: Forces nested presentation mode inside the existing
  display server.
- `XDG_DATA_HOME=/tmp/tessera-dev`: Isolates agent principal/grant stores from
  the host session.
- `TESSERA_PREVIEW=control-center`: Opens the Control Center at startup,
  bypassing the host compositor's interception of the global `Super+S`
  shortcut. Post sample notifications with `tessera notification send`.
- `XDG_DATA_DIRS`: Preserves desktop asset and application discovery when
  `XDG_DATA_HOME` is redirected.

> [!NOTE]
> Only one process can own `$XDG_RUNTIME_DIR/tessera.sock`. A nested instance
> started beside an active host session will log an address-in-use warning and
> run cleanly without IPC.

For complete nested backend details, see
[Nested Backend Development](../nested-backend.md).

---

## 5. Scenario 3: Inner Client Injection & Protocol Checks

### Purpose

Check application launch, buffer negotiation, and window state inside the
nested compositor.

### Execution

1. Start the nested compositor (Scenario 2) and note the inner display printed
   in the log:
   ```text
   server: listening on WAYLAND_DISPLAY=wayland-1
   ```
2. Open a separate terminal and target the inner display:

```bash
# Launch a terminal inside the nested session
WAYLAND_DISPLAY=wayland-1 foot

# Query inner Wayland protocols and dmabuf v4 tranches
WAYLAND_DISPLAY=wayland-1 wayland-info

# Launch a Flatpak client inside
flatpak run --env=WAYLAND_DISPLAY=wayland-1 <app_id>
```

---

## 6. Scenario 4: Direct VT/DRM Hardware Session

### Purpose

Verify real KMS atomic modesetting, direct scanout, display hotplug, and
VT switching on physical GPU hardware.

### Execution

1. **Build before switching VTs** (never compile in bare VT to avoid compiler latency and hangs):
   ```bash
   cargo build --locked --release -p tessera
   ```
2. Switch to a bare TTY (`Ctrl+Alt+F3`), log in, and run:
   ```bash
   TESSERA_BACKEND=drm RUST_LOG=info target/release/tessera
   ```
   *(For multi-GPU systems, specify `TESSERA_DRM_DEVICE=/dev/dri/card1`)*.

For extensive hardware verification steps (mixed-refresh rates, atomic planes),
see the detailed guide: [VT/DRM Verification](vt-drm-verification.md). For the
gated overlay candidate, complete-set `TEST_ONLY`, and deactivation matrix, see
[How to Validate KMS Overlay Offload](../../how-to/kms-overlay-validation.md).

---

## 7. Scenario 5: Full Workspace Test Suite

Before opening a pull request, run the automated suites across the workspace.
Runner and layout rules are in [Testing Standards](../standards/testing.md):

```bash
# Full parallel unit, integration, and e2e tests
cargo nextest run --locked --workspace

# Or test a single crate under edit
cargo nextest run --locked -p tessera-shell --lib
```

---

## 8. 30-Second Smoke Checklist

Run [Scenario 2](#4-scenario-2-nested-compositor-inside-an-existing-desktop)
and confirm each point before opening a pull request:

- [ ] **Startup**: Compositor window appears without panics or swapchain errors.
- [ ] **Chrome**: Background wallpaper and Dock render cleanly at scale.
- [ ] **Shortcuts & Panels**: Control Center or Settings open and respond to focus.
- [ ] **Client Interaction**: Inner client window opens, accepts keyboard/pointer
      input, and resizes properly.
- [ ] **Dynamic Resize**: Resizing the outer host window updates the nested
      output resolution seamlessly.
- [ ] **Teardown**: Pressing `Ctrl+C` terminates cleanly without hanging threads
      or orphaned sockets.

When a run changes what is known about an area, update its row in the
[Support Matrix](support-matrix.md) in the same change.
