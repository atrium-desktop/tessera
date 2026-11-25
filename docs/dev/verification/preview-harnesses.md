[Verification Index](index.md) > [Manual Verification Playbook](manual-verification.md) > Preview Harnesses

---

# Preview Harnesses: Authoring & Usage Guide

A **Preview Harness** is a Layer 2 development-only binary that renders a
production Tessera UI surface inside an ordinary Wayland window, decoupled from
compositor ownership, PAM, or physical hardware.

This document is the single reference for both **using** the existing preview
harnesses (`tessera-lock-preview`, `tessera-settings-preview`) and **authoring**
a new one to repository standards.

---

## 1. When to Build a Preview Harness

Follow the four-layer testing ladder in [Verification Standards](../standards/verification.md#2-the-verification-ladder):

| Surface kind | Layer 2 mechanism | Reason |
| :--- | :--- | :--- |
| **Standalone first-party app** (`tessera-lock`, `tessera-settings`, future apps) | **Separate `*-preview` binary** gated by `dev-preview` | Keeps mock/fault-injection code out of the shipping binary and lets the surface run directly on the host Wayland desktop. |
| **In-process compositor chrome** (Control Center, Dock, Overview, Switcher) | **Debug-gated presentation toggle** (`TESSERA_PREVIEW=...`) inside nested `tessera` | Chrome is rendered inside the compositor's own frame loop; there is no standalone client binary. |
| **Pure logic / state / codecs** | **Layer 1 unit tests** (`cargo nextest`) | Sub-second, zero GPU or Wayland dependency. |
| **Hardware / PAM / KMS scanout** | **Layer 3 Nested / Layer 4 VT-DRM** | A preview validates presentation and client-side state, never physical scanout or real PAM policy. |

**Rule of thumb:** build a `*-preview` harness whenever a standalone surface has
states that are expensive, dangerous, or impossible to trigger in nested mode
(authentication rejection, multi-display layouts, libinput capabilities, battery
ceilings, read-only scope refusal, responsive narrow widths).

---

## 2. Using Existing Preview Harnesses

### 2.1 Settings Preview (`tessera-settings-preview`)

Runs the **unmodified production Settings window and IPC `Link`** against an
in-process wire-level `tessera_ipc::Server` ([ADR-0257](../../adr/0257-explicit-ipc-socket-override-and-settings-preview-harness.md)):

```bash
# Default desktop preset
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview

# Multi-display preset on the Display page (exercises arrangement, mirror, 15s preview)
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview -- --preset multi-display display

# Compact (<640px) icon-rail layout + laptop touchpad & OnBattery material ceiling + read-only scope
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview -- --preset laptop --mode read-only --size 540x720 input

# Mock IPC server only — connect an unmodified production `tessera-settings` or `tessera` CLI
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview -- --mock-server-only --socket /tmp/mock-settings.sock
```

Key options:
- `--preset`: `desktop` (default), `multi-display`, `laptop`, `minimal`
- `--mode`: `live` (default), `read-only`, `reject-stale`, `reject-locked`, `disconnected`
- `--size WIDTHxHEIGHT`: fixed initial logical size (e.g. `540x720`, `960x680`, `1440x900`)
- `--ready-fd FD`: writes one byte (`1`) after the first presented frame
- `[PAGE]`: optional deep-link target (`display`, `input`, `power`, `appearance`, `dock`, `accessibility`, `users`, `window-rules`)

### 2.2 Lock Screen Preview (`tessera-lock-preview`)

Runs the **production lock-screen renderer and `LockState`** inside an ordinary
`xdg_toplevel` without creating an `ext_session_lock_v1` lock or calling PAM:

```bash
# Interactive default preview (any nonempty text + Enter accepts; Escape closes)
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview

# Hold a specific visual state and composition at a fixed logical size
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- --composition cinematic --state rejected --size 1280x800

# Interactive accept/reject loop with a fake password (never pass a real password)
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- --password test
```

Key options:
- `--composition`: `cinematic`, `centered`, `bsod`
- `--state`: `ready`, `typing`, `checking`, `rejected`, `unavailable`, `ambient`
- `--result` / `--password`: deterministic submission outcome or fake password match
- `--background PATH`: process-local wallpaper override
- `--size WIDTHxHEIGHT` and `--ready-fd FD`

### 2.3 Agent & Automated Pixel Capture

Every preview harness exposes a stable `dev.tessera.<Name>Preview` Wayland
`app_id`, `--size`, and `--ready-fd` so automation and AI agents can capture
pixel-exact screenshots without log scraping:

```bash
# Capture the running preview window from the outer Tessera session
APP_ID="dev.tessera.SettingsPreview"   # or "dev.tessera.LockPreview"
windows=$(cargo run --locked -p tessera -- window -j)
id=$(printf '%s' "$windows" | jq -r \
  --arg app "$APP_ID" 'map(select(.app_id == $app)) | last | .id')
region=$(printf '%s' "$windows" | jq -r \
  --arg app "$APP_ID" 'map(select(.app_id == $app)) | last | "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$id"
cargo run --locked -p tessera -- display capture --region "$region" /tmp/preview.png
```

---

## 3. Architecture & Authoring Contract (How to Write One)

Rust/Cargo natively supports a single crate hosting one shared library
(`src/lib.rs`) alongside both a production `[[bin]]` and a feature-gated
preview `[[bin]]`. Every preview harness in Tessera must satisfy the following
six contract rules.

### 3.1 Cargo Manifest Layout

Declare a **non-default** `dev-preview` feature and gate the preview binary with
`required-features`:

```toml
[features]
default = []
dev-preview = []

[[bin]]
name = "tessera-<name>"
path = "src/main.rs"

[[bin]]
name = "tessera-<name>-preview"
path = "src/preview_main.rs"
required-features = ["dev-preview"]
```

Because `dev-preview` is not in `default`, production builds (`cargo build
--release -p tessera-<name>`) never compile `src/preview_main.rs` or any module
it alone declares, and never emit the preview binary.

### 3.2 Zero Preview Code in Production

- `src/main.rs` must contain **zero** `#[cfg(feature = "dev-preview")]` branches
  and **zero** mock/simulated code paths.
- Keep shared UI/model/rendering code in `src/lib.rs` (and thin shared window
  plumbing such as `src/app/`), and isolate all harness-specific code under
  `src/preview_main.rs` + `src/preview/`.
- Prefer **wire-level mocking** (like `tessera-settings-preview` running a real
  in-process `tessera_ipc::Server`) whenever the production binary is an IPC
  client: it tests the real transport, reconnection, and scope negotiation for
  free. Only replace the window shell itself (like `tessera-lock-preview`) when
  the production shell uses a privileged/session-locking Wayland protocol
  (`ext_session_lock_v1`).

### 3.3 Standard CLI & Window Contract

Every `tessera-*-preview` binary must implement:

| Contract item | Requirement |
| :--- | :--- |
| **App ID** | `dev.tessera.<Name>Preview` (never reuse the production reverse-DNS app ID). |
| **Session isolation** | Never bind the production single-instance socket, never write to `~/.config/tessera`, never touch real PAM or host `tessera.sock`. Use ephemeral temp directories with RAII `Drop` cleanup. |
| **`--size WIDTHxHEIGHT`** | Set a deterministic initial logical size for compact, desktop, and ultrawide layout checks. |
| **`--ready-fd FD`** | Write one byte (`1`) and close `FD` immediately after the first presented frame. |
| **Preset / fault flags** | Expose explicit, parse-validated flags (`--preset`, `--mode`, `--state`, etc.) with `-h`/`--help` and `-V`/`--version`. |

### 3.4 Packaging & Architecture Gates (`xtask`)

After adding a new `tessera-<name>-preview` binary, update and run all workspace
gates:

1. **Register in `DEV_ONLY`**: add `"tessera-<name>-preview"` to `DEV_ONLY` in
   `xtask/src/tasks/check_install_manifest.rs` (and its unit test) so
   `scripts/install.sh` and `docs/dev/packaging.md` explicitly exclude it.
2. **Respect panic discipline (`INV-ARCH-51`)**: preview code inside a workspace
   crate is checked by `xtask check-panics`; avoid `.unwrap()`/`.expect()` in
   non-test harness code.
3. **Respect bootstrap admission (`ADR-0147`)**: `xtask check-bootstrap-admission`
   checks every binary entry point's references to `tessera_env::*`. Do not pull
   single-consumer items into `tessera-env`.
4. **Verify all gates and production exclusion**:

```bash
# 1. Unit, harness, and clippy checks
cargo nextest run --locked -p tessera-<name> --all-features
cargo clippy --locked -p tessera-<name> --all-features --all-targets -- -D warnings

# 2. Repository architecture gates
cargo run --locked -p xtask -- check-install-manifest
cargo run --locked -p xtask -- check-bootstrap-admission
cargo run --locked -p xtask -- check-boundaries
cargo run --locked -p xtask -- check-panics
cargo run --locked -p xtask -- check-docs

# 3. Mechanical production-exclusion check
prod_target=$(mktemp -d)
cargo build --locked --release --target-dir "$prod_target" \
  -p tessera-<name> --bin tessera-<name> --no-default-features
test -x "$prod_target/release/tessera-<name>"
test ! -e "$prod_target/release/tessera-<name>-preview"
rm -rf "$prod_target"
```

---

## 4. What Preview Does *Not* Replace

A passing preview check proves that the **surface itself** renders and handles
its state machine and client protocol cleanly. Before shipping a change that
affects compositor behavior or hardware, still escalate up the ladder:

- **Compositor visual/behavioral effect** (dock animations, backdrop materials,
  wallpaper, theme propagation, Control Center ↔ Settings sync) →
  [Layer 3: Nested Integration](manual-verification.md#4-scenario-2-nested-compositor-inside-an-existing-desktop)
  via `./scripts/run-nested-dev.sh`.
- **Cursor theme/size, physical outputs, libinput devices, KMS scanout, PAM** →
  [Layer 4: Direct VT/DRM](vt-drm-verification.md) via `./scripts/run-drm-dev.sh`.
