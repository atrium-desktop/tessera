[Verification Index](index.md) > [Manual Verification Playbook](manual-verification.md) > Surface Verification

---

# Surface Verification

How to verify the interactive shell surfaces on each ladder layer: the Control
Center (compositor chrome, ADR-0080), Settings (`tessera-settings`), and the
lock screen (the `tessera-lock` production surface). A result from one layer
never stands in for a result from another.

## Scope

Ladder layers 2–4 for the interactive shell surfaces. Start with the cheapest
layer that can fail for the change and finish on a physical session only when
the change crosses a hardware or authentication boundary. The ladder, the
presentation-toggle and inspection-harness rules, and the shared nested
isolation block are defined in
[Verification Standards](../standards/verification.md); unit-test rules are in
[Testing Standards](../standards/testing.md#4-writing-unit-tests).

## Verify the Control Center

### Unit Tests

Control Center presentation logic follows the
[unit-test rules](../standards/testing.md#4-writing-unit-tests): pure logic, no
GPU or live sources. Run the suite:

```bash
cargo check --locked -p tessera-shell
cargo nextest run --locked -p tessera-shell --lib
```

The suite covers card bounds and stacking on small displays, reduced-motion
reveal snapping, the constraint that the sheet never requests backdrop
effects, formatting helpers, and persona profile resolution. It cannot catch rendering
faults such as a frame arena overflow; escalate to the preview layer when the
change touches rendering.

### Control Center Preview in a Nested Session

The Control Center is compositor chrome, so its preview runs the production compositor
with a presentation toggle rather than a harness binary.

The host compositor intercepts global `Super` shortcuts in nested sessions, so
open the sheet one of two ways.

Auto-open on startup (zero keystrokes) — the shared isolation block plus one
toggle, debug builds only:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_PREVIEW=control-center \
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

No build seeds demo notifications or tray items. Post real notifications over
IPC when the card needs content:
`tessera notification send "Summary" "Body"`. Close and reopen the sheet with
`Super+S` when the host does not claim it, `Escape`, or a press outside both
cards.

Non-Super keybinding — key combinations such as `Ctrl+Alt+S` reach the nested
window directly. Add a development keybinding in `config.toml`:

```toml
[[keybind]]
mods = ["ctrl", "alt"]
key = "s"
action = "control_center"
```

Nested limits: the instance cannot modeset physical outputs or configure
libinput devices, so validate display and touchpad settings under DRM/KMS.
Follow [Nested Backend Development](../nested-backend.md) for socket discovery,
process replacement, and backend limits.

### Capture Control Center Pixels for an Agent

Keep the physical session unlocked. Start the nested compositor with the sheet
auto-opened, then drive the outer session's CLI from another terminal:

```bash
windows=$(cargo run --locked -p tessera -- window -j)

nested_id=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "tessera" and .title == "tessera")) | last | .id')

nested_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "tessera" and .title == "tessera")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$nested_id"
cargo run --locked -p tessera -- display capture \
  --region "$nested_region" \
  /tmp/tessera-nested-control-center.png
```

Focus before capture so the terminal does not raise itself above the target.
Capture queues asynchronous PNG encoding: treat the image as ready only after
the destination exists and has a nonzero size. On HiDPI outputs the PNG
dimensions are the physical-pixel equivalent of the logical region.

## Verify Settings

Settings (`tessera-settings`, [ADR-0238](../../adr/0238-settings-is-a-standalone-application-behind-its-own-scope.md)) follows the same ladder and inspection-harness contract ([ADR-0257](../../adr/0257-explicit-ipc-socket-override-and-settings-preview-harness.md)).

### Unit and Wire-Level Mock Tests

```bash
cargo nextest run --locked -p tessera-settings --all-features
cargo nextest run --locked -p tessera-tests desktop_settings
```

### Run the Settings Preview

The Settings preview is a development-only binary gated by the non-default `dev-preview` feature. It runs an in-process wire-level `tessera_ipc::Server` so the production Settings window, two-connection `Link`, scope negotiation, revisioned transactions, and `SettingsChanged` broadcasts all run end to end without a compositor:

```bash
cargo run --locked -p tessera-settings \
  --features dev-preview \
  --bin tessera-settings-preview -- [OPTIONS] [PAGE]
```

The Preview creates an ordinary `xdg_toplevel` with app ID `dev.tessera.SettingsPreview` and never touches the production single-instance socket.

Preview options:

| Option | Values | Purpose |
|--------|--------|---------|
| `--preset` | `desktop`, `multi-display`, `laptop`, `minimal` | Selects a deterministic hardware/config snapshot (including multi-output, touchpad, and `OnBattery` material-ceiling states unavailable in nested mode). |
| `--mode` | `live`, `read-only`, `reject-stale`, `reject-locked`, `disconnected` | Injects scope refusal, stale-revision conflict, lock-screen refusal, or transport disconnect. |
| `--size` | `WIDTHxHEIGHT` | Fixed initial logical window size (e.g. `540x720` for compact `<640px` icon rail, `960x680` standard, `1440x900` wide). |
| `--ready-fd` | Descriptor | Writes one byte after the first presented frame for log-free agent pixel capture. |
| `--mock-server-only` | Flag (requires `--socket PATH`) | Runs only the wire-level mock IPC server so an unmodified production `tessera-settings --socket PATH` or `tessera --socket PATH` can be driven against it. |
| `--socket` | Path | Explicit mock IPC socket path (defaults to an ephemeral temp directory). |

### Capture Settings Preview Pixels for an Agent

```bash
windows=$(cargo run --locked -p tessera -- window -j)

preview_id=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.SettingsPreview")) | last | .id')

preview_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.SettingsPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$preview_id"
cargo run --locked -p tessera -- display capture \
  --region "$preview_region" \
  /tmp/tessera-settings-preview.png
```

### Nested Integration: Live Compositor Feedback

Start an isolated nested compositor (isolating `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, and `TESSERA_IPC_SOCKET`, and authorizing the workspace-built `tessera-settings` binary in the isolated config):

```bash
./scripts/run-nested-dev.sh
```

Then either run Settings **split-screen** on the host desktop targeting the nested compositor's IPC socket:

```bash
TESSERA_IPC_SOCKET=/tmp/tessera-dev/tessera.sock \
  target/release-fast/tessera-settings appearance
```

or **inside** the nested compositor by adding `WAYLAND_DISPLAY=wayland-N` (logged by the nested compositor) to validate window decorations, clipboard, scale, and self-focus.

## Verify the Lock Screen

### Run the Lock Preview

The lock preview follows the
[inspection-harness rule](../standards/verification.md#4-writing-rules): a development-only
binary target in the `tessera-lock` package, selected by the non-default
`dev-preview` feature:

```bash
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- [OPTIONS]
```

Use `cargo build --locked` with the same package, feature, and target when a
script needs the stable path `target/debug/tessera-lock-preview`.

The Preview creates an ordinary `xdg_toplevel` with app ID
`dev.tessera.LockPreview`. It uses the production `LockState`, identity loader,
avatar resources, and renderer, but it never creates an `ext_session_lock_v1`
object or calls PAM. Enter any nonempty text and press `Enter` to simulate an
accepted result; press `Escape` to close without submitting. `Backspace` and
`Ctrl+U` exercise the production credential-editing behavior.

Preview options:

| Option | Values | Purpose |
|--------|--------|---------|
| `--composition` | `cinematic`, `centered`, `bsod` | Selects the complete UI composition. `--style` is a compatibility alias. |
| `--background` | Image path | Overrides the background for this process only; `bsod` ignores it. An invalid image falls back to the bundled artwork. |
| `--state` | `ready`, `typing`, `checking`, `rejected`, `unavailable`, `ambient` | Holds a state for inspection. |
| `--result` | `accepted`, `rejected`, `unavailable` | Result of a later interactive submission; use for a deterministic one-state inspection. |
| `--password` | Fake value | Only this exact value closes the Preview; any other nonempty value produces the production rejection state. Use for an interactive accept/reject loop. |
| `--size` | `WIDTHxHEIGHT` | Fixed logical window size for small, standard, ultrawide, and portrait layout checks. |
| `--ready-fd` | Descriptor | Writes one byte after the first presented frame, for automation that must not parse logs. |

`--password` and `--result` are mutually exclusive. Never pass a real account
password: command-line arguments may be retained in shell history and visible
in the process list. The fake value never reaches PAM or Tessera configuration.

The Preview reads `[lock_screen]` from the normal Tessera configuration; the
options above scope a single process instead of editing that file. For
translated lock copy, set the process locale:

```bash
LC_ALL=zh_CN.UTF-8 \
cargo run --locked -p tessera-lock \
  --features dev-preview \
  --bin tessera-lock-preview -- --state unavailable
```

Interactive submissions retain the checking state briefly so the selected
composition's validation feedback can be inspected. From submission until the
result arrives, the shared lock state freezes typing, deletion, clearing, and
repeated submission while retaining the entered marks as locked feedback. If
`Enter` is pressed during rejection backoff, the attempt remains pending and
starts automatically at the retry deadline. Cinematic's cool light sweep runs
at a fixed rate from the credential rail's left edge to its right, independent
of password length and backoff; the production lock keeps the same feedback
active until PAM replies.

What to capture per composition — capture both compositions after changing
shared typography, identity, credential, or status rendering:

- `cinematic`: an empty-password frame (neutral rail, no marks), a typing
  frame (marks only, never empty slots implying a length), and a rejected
  frame (red rail, no text error). Use a short recording for the rejection
  shake.
- `bsod`: engaged, typing, checking, and rejected states at compact
  (`390x844`), desktop (`1280x800`), and ultra (`3440x1440`) size. The
  composition has no input box: the sad face, wrapped headline, keystroke
  counter, and support block must stay inside the side margins, and the QR
  module must keep its quiet zone clear of the support lines. The counter
  narrates the authentication state — the entered-character count while
  typing, a static verifying line while checking, and zero after rejection —
  and a rejected attempt also updates the stop code in the support block.

Production `tessera-lock` uses the independent `[lock_screen.background]`
configuration described in the
[Configuration Reference](../../reference/config.md#lock-screen).

### Capture Lock Preview Pixels for an Agent

Keep the physical session unlocked. Start the Preview in one terminal, then
drive the outer session's CLI from another:

```bash
windows=$(cargo run --locked -p tessera -- window -j)

preview_id=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.LockPreview")) | last | .id')

preview_region=$(printf '%s' "$windows" | jq -r \
  'map(select(.app_id == "dev.tessera.LockPreview")) | last |
    "\(.position.x),\(.position.y),\(.size.w),\(.size.h)"')

cargo run --locked -p tessera -- window focus "$preview_id"
cargo run --locked -p tessera -- display capture \
  --region "$preview_region" \
  /tmp/tessera-lock-preview.png
```

The capture readiness and HiDPI notes from the Control Center recipe apply unchanged.

### Nested Integration: the Production Lock

Use Preview results only for presentation. Verify session-lock behavior with
the unmodified production target inside nested Tessera, using the shared
isolation block:

```bash
mkdir -p /tmp/tessera-dev/tessera
TESSERA_BACKEND=nested \
XDG_DATA_HOME=/tmp/tessera-dev \
XDG_DATA_DIRS=$HOME/.local/share:/usr/local/share:/usr/share \
cargo run --locked -p tessera
```

Copy the inner socket name from the startup log:

```text
server: listening on WAYLAND_DISPLAY=wayland-N
```

Start the production locker on that socket:

```bash
WAYLAND_DISPLAY=wayland-N \
cargo run --locked -p tessera-lock --bin tessera-lock --no-default-features
```

The inner compositor is now genuinely locked. The unlocked outer compositor
can capture the nested Tessera window for review without weakening the inner
capture policy.

Enter the account password to test the complete PAM transition. For a visual
or protocol-only test, stop the entire nested compositor to recover. Do not
kill only `tessera-lock` after secure confirmation: the inner compositor must
retain its fail-closed frame. Follow
[Nested Backend Development](../nested-backend.md) for socket discovery, process
isolation, and nested-backend limits.

## Verify a Physical Session

Run physical checks only after preview and nested checks pass. Keep
another TTY available, and perform these checks in order:

1. Lock and unlock from the active desktop.
2. Confirm capture refusal on the locked surface and display-off.
3. Suspend and resume while locked; confirm the fail-closed frame survives
   and input is required again.
4. Repeat on every physical output.

Validate the exact PAM service with `pamtester` first. For the Control Center,
physical sessions validate the display and touchpad settings that nested mode
can only observe. For the full hardware procedure, see
[VT/DRM Verification](vt-drm-verification.md).

## Select Checks by Change

| Change | Required checks |
|--------|-----------------|
| Control Center layout, color, typography or localized copy | Unit tests plus sheet preview at representative sizes, scales, locales, and visual states |
| Control Center quick controls, detail pages, tray, or menu navigation | Sheet preview: open and close, enter and leave detail pages, descend and back in menus, tray activation |
| Notification card or live-source readings | Sheet preview with notifications posted over IPC and live source rendering |
| Lock layout, color, typography or localized copy | Preview both compositions at representative sizes, scales, locales and visual states |
| Independent lock background or artwork scrim | Preview with `--background`, then start a fresh production lock using `[lock_screen.background]` |
| Stop-screen (`bsod`) composition | Preview engaged, typing, checking, and rejected states at compact, desktop, and ultra sizes; confirm the keystroke counter and stop code track each state |
| Avatar image, VRM or animation | Preview initial frame, motion, hot reload and fallback avatar |
| Credential state machine or authentication feedback | `tessera-lock` unit tests plus Preview submission results |
| Lock-surface lifecycle or input routing | Nested lock, including client exit and outer capture |
| PAM service or account policy | `pamtester` followed by physical lock and unlock |
| Capture or ScreenCast policy | Nested policy check followed by physical-session verification |
| Idle, display-off, suspend or resume | Physical DRM/KMS session |

## Verify Production Exclusion

The `dev-preview` feature changes target availability, not production lock
logic. Build the production target explicitly in a fresh target directory and
assert that only it exists:

```bash
tessera_prod_target=$(mktemp -d)
cargo build --locked --release \
  --target-dir "$tessera_prod_target" \
  -p tessera-lock --bin tessera-lock --no-default-features

test -x "$tessera_prod_target/release/tessera-lock"
test ! -e "$tessera_prod_target/release/tessera-lock-preview"

cargo build --locked --release \
  --target-dir "$tessera_prod_target" \
  -p tessera-settings --bin tessera-settings --no-default-features

test -x "$tessera_prod_target/release/tessera-settings"
test ! -e "$tessera_prod_target/release/tessera-settings-preview"
```

Distribution builds must not enable `dev-preview`, and install manifests must
name `target/release/tessera-lock` explicitly. Do not install binaries through a
`target/release/tessera-*` wildcard.
