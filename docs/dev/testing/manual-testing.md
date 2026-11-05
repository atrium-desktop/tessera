[Testing Index](index.md) > Manual Testing Playbook

---

# Manual Testing Playbook

This playbook provides actionable, scenario-based manual testing recipes for
Tessera. It is designed to be concise, task-focused, and copy-paste ready.

For governing engineering standards and architectural rules, see
[Testing Standards](../standards/testing.md). For deep dive specifications, see
[Surface Testing](surface-testing.md) and [VT/DRM Manual Testing](vt-drm-testing.md).

---

## 1. Scenario Decision Matrix

Select the narrowest testing scenario that covers your change:

| Verification Target | Recommended Scenario | External Dependencies | Execution Overhead |
| :--- | :--- | :--- | :--- |
| **Logic, protocol codecs, schemas, config, IPC, audit** | **[Scenario 0: Flux-Free Logic](#scenario-0-flux-free-rapid-logic-verification-zero-c-dependencies)** | None (Pure Rust toolchain only) | Sub-second |
| **Lock screen UI, avatar, credential interaction** | **[Scenario 1A: Lock Screen Preview](#1a-lock-screen-preview)** | Wayland host only (Zero compositor required) | Instant window |
| **3D Persona avatar rendering & framing** | **[Scenario 1B: Avatar Preview](#1b-3d-persona--vrm-avatar-preview)** | GPU (Headless snapshot dump) | Instant dump |
| **Compositor runtime, window management, chrome, layout** | **[Scenario 2: Nested Compositor](#scenario-2-testing-inside-an-existing-desktop-nested-mode)** | Full host Wayland desktop + Optics C libraries | Single isolated client window |
| **Inner Wayland client protocols, dmabuf, xdg-shell** | **[Scenario 3: Inner Client Injection](#scenario-3-inner-client-injection--protocol-testing)** | Running nested instance | Client process |
| **Physical KMS scanout, VT switching, multi-GPU** | **[Scenario 4: Direct VT/DRM](#scenario-4-direct-vtdrm-hardware-testing)** | Physical Linux TTY + rootless seat | Bare-metal session switch |
| **Full workspace unit & doc suite** | **[Scenario 5: Full Workspace Sanity](#scenario-5-full-workspace-unit-sanity)** | Full native stack + Optics | ~10-20 seconds |

---

## 2. Scenario 0: Flux-Free Rapid Logic Verification (Zero C Dependencies)

### Purpose

Validate core business logic, protocol serializations, audit chains, and config
parsing without installing or building the native C23/Meson Optics stack (`flux`,
`vista`, `lens`, `iris`).

### Execution Command

```bash
# Using cargo-nextest (recommended)
cargo nextest run --locked \
  -p tessera-desktop -p tessera-primitives -p tessera-authority -p tessera-audit \
  -p tessera-config -p tessera-ipc -p tessera-protocol -p tessera-launch-services \
  -p tessera-env -p tessera-cli -p tessera-mcp -p tessera-idle \
  -p tessera-wayland

# Fallback using standard cargo test
cargo test --locked \
  -p tessera-desktop -p tessera-primitives -p tessera-authority -p tessera-audit \
  -p tessera-config -p tessera-ipc -p tessera-protocol -p tessera-launch-services \
  -p tessera-env -p tessera-cli -p tessera-mcp -p tessera-idle \
  -p tessera-wayland
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
- For deep harness conventions, see [Surface Testing](surface-testing.md).

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

---

## 4. Scenario 2: Testing Inside an Existing Desktop (Nested Mode)

### Purpose

Run and test the production Tessera compositor as a regular client window inside
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
TESSERA_COMMAND_PANEL_OPEN=1 \
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

### Essential Parameter Explanations

- `TESSERA_BACKEND=nested`: Forces nested presentation mode inside the existing
  display server.
- `XDG_DATA_HOME=/tmp/tessera-dev`: **Crucial isolation**. Prevents
  `AuditError::Locked` caused by an active host session holding an exclusive
  `flock` on `events-v2.jsonl`.
- `TESSERA_COMMAND_PANEL_OPEN=1`: Automatically opens the Command Panel at
  startup with mock notifications in debug builds, bypassing the host compositor's
  interception of the global `Super+S` shortcut.
- `XDG_DATA_DIRS`: Preserves desktop asset and application discovery when
  `XDG_DATA_HOME` is redirected.

> [!NOTE]
> Only one process can own `$XDG_RUNTIME_DIR/tessera.sock`. A nested instance
> started beside an active host session will log an address-in-use warning and
> run cleanly without IPC.

For complete nested backend details, see
[Nested Backend Development](../nested-backend.md).

---

## 5. Scenario 3: Inner Client Injection & Protocol Testing

### Purpose

Test application launch, buffer negotiation, and window state inside the nested
compositor.

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

## 6. Scenario 4: Direct VT/DRM Hardware Testing

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
see the detailed guide: [VT/DRM Manual Testing](vt-drm-testing.md).

---

## 7. Scenario 5: Full Workspace Unit Sanity

Before opening a pull request or pushing changes, verify all logic and doc tests
across the full repository:

```bash
# Full parallel unit tests
cargo nextest run --locked --workspace

# Documentation code block tests
cargo test --doc --locked --workspace

# Or test a single crate under edit
cargo nextest run --locked -p tessera-shell --lib
```

---

## 8. 30-Second Manual Smoke Checklist

Confirm these key points before opening a pull request:

- [ ] **Startup**: Compositor window appears without panics or swapchain errors.
- [ ] **Chrome**: Background wallpaper and Dock render cleanly at scale.
- [ ] **Shortcuts & Panels**: Command Panel or Settings open and respond to focus.
- [ ] **Client Interaction**: Inner client window opens, accepts keyboard/pointer
      input, and resizes properly.
- [ ] **Dynamic Resize**: Resizing the outer host window updates the nested
      output resolution seamlessly.
- [ ] **Teardown**: Pressing `Ctrl+C` terminates cleanly without hanging threads
      or orphaned sockets.
