# Development Environment Variables

Contributor-only reference and how-to guide for environment variable overrides
used in local development, nested compositor testing, asset validation, and UI debugging.

User-facing configuration belongs in `config.toml`; environment variables documented
here provide development-time escape hatches, backend overrides, and testing hooks.

## Quick Recipe: Nested Testing Inside a Live Tessera Session

When developing inside an active Tessera desktop session, start the nested instance
with an isolated data directory and auto-opened Control Center:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_PREVIEW=control-center \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

---

## Variable Reference

### 1. Compositor & Presentation Backend

| Variable | Values | Default | Purpose |
|----------|--------|---------|---------|
| `TESSERA_BACKEND` | `auto`, `nested`, `drm` | `auto` | Selects presentation host. In a graphical Wayland terminal, `auto` picks `nested`; on a bare TTY it selects `drm`. Force `nested` when running automated tests that must not fall back to DRM. |
| `TESSERA_DRM_DEVICE` | Path (e.g. `/dev/dri/card0`) | Auto-selected primary GPU | Explicitly selects the DRM primary and render device on multi-GPU systems. |

---

### 2. Chrome & Overlay UI Debugging

| Variable | Values | Purpose |
|----------|--------|---------|
| `TESSERA_PREVIEW` | `control-center`, `overview`, `pivot`, `switcher` | **Opens the named surface at startup** and isolates dock state in a temporary sandbox. Bypasses `Super+S` and other keystrokes the host compositor captures in nested sessions. Presentation only. |

> [!NOTE]
> No build seeds demo notifications or tray items. To fill the Control
> Center's notifications card, post real ones over IPC:
> `tessera notification send "Summary" "Body"`.

---

### 3. Persona, VRM & 3D Avatar Tooling

| Variable | Values | Purpose |
|----------|--------|---------|
| `TESSERA_AVATAR_DEBUG_DUMP` | File path (e.g. `/tmp/tessera-avatar.png`) | When running `tessera-shell` examples, writes a headless GPU-rendered snapshot of the active VRM avatar and camera framing to disk. |
| `TESSERA_AVATAR_DEBUG_ASSETS` | `1` | Forces fallback to the in-tree debug assets directory instead of XDG directories. |
| `TESSERA_AVATAR_DEBUG_TIME` | Seconds (float, e.g. `1.5`) | Simulates animation playback elapsed time when generating offscreen avatar snapshots. |

Example standalone VRM preview:

```bash
TESSERA_AVATAR_DEBUG_DUMP=/tmp/tessera-avatar.png \
TESSERA_AVATAR_DEBUG_TIME=1.0 \
cargo run -p tessera-shell --features persona --example debug_avatar
```

---

### 4. Wallpaper, Assets & Icon Theming

| Variable | Values | Purpose |
|----------|--------|---------|
| `TESSERA_WALLPAPER_MODEL` | `builtin`, path to `.glb` | Overrides the 3D scene wallpaper model. |
| `TESSERA_ICON_THEME` | Theme name (e.g. `Papirus-Dark`) | Highest-precedence override for application icon theme resolution. |

---

### 5. XDG Isolation & File Lock Protection

When running multiple compositor instances concurrently (e.g. a nested development
instance inside a live desktop session), these variables isolate volatile state:

| Variable | Recommended Dev Value | Purpose |
|----------|-----------------------|---------|
| `XDG_DATA_HOME` | `/tmp/tessera-dev` | Relocates the persistent principal and grant stores (`principals.json`, `grants.json`) so development runs do not mutate production agent credentials. |
| `XDG_DATA_DIRS` | `$HOME/.local/share:...` | Preserves user asset discovery (`~/.local/share/tessera/avatars/avatar.vrm`, icons, and applications) when `XDG_DATA_HOME` is redirected. |
| `XDG_CONFIG_HOME` | `/tmp/tessera-dev/config` | Isolates configuration changes from the host session's `config.toml`. |
| `TESSERA_IPC_SOCKET` | `/tmp/tessera-dev/tessera.sock` | Overrides the session IPC socket path for the compositor and first-party IPC clients (`tessera` CLI, `tessera-settings`, `tessera-idle`, `tessera-mcp`; [ADR-0257](../adr/0257-explicit-ipc-socket-override-and-settings-preview-harness.md)). Must be an absolute path; empty or relative values fail closed. Explicit `--socket` flags take precedence. Never alters scope allowlists or lock-screen refusal. |

---

### 6. Logging & Observability

| Variable | Values | Default | Purpose |
|----------|--------|---------|---------|
| `RUST_LOG` | `error`, `warn`, `info`, `debug`, `trace` | `info` | Directs logging verbosity per module (e.g. `RUST_LOG="info,tessera_backend=debug"`). |
| `TESSERA_LOG_FORMAT` | `text`, `json` | `text` | Switches logger output between human-readable text and structured JSON. |
