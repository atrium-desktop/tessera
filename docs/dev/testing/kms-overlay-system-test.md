[Testing Index](index.md) > [Manual Testing Playbook](manual-testing.md) > KMS Overlay System Test

---

# KMS Overlay System Test

This is the manual **KMS system test** for the one-candidate overlay path from
[ADR-0235](../../adr/0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md).
It validates real DRM/KMS behavior that unit, integration, and nested tests
cannot prove.

The test is gated by `TESSERA_OVERLAY_OFFLOAD=1`. Default runs keep overlay
allocation and presentation disabled and only produce audit telemetry.

Automated coverage before this hardware layer:

- **Unit/property**: `crates/tessera-composite/src/presentation/overlay.rs`
  covers candidate policy, rejection ordering, placement identity, and the
  Primary-repaint contract ([ADR-0236](../../adr/0236-overlay-placement-is-a-pure-state-machine.md)).
- **Backend unit**: `crates/tessera-platform/src/drm.rs` covers the pure
  overlay request values (source/destination 16.16 geometry, alpha, zpos,
  fence, damage support) without a DRM card.
- **Integration**: `tests/integration/composite_overlay.rs`
  exercises accepted-plan -> placement -> buffer update -> movement ->
  deactivation through the public presentation boundary.
- **Protocol E2E**: the existing `*_e2e` tests cover real clients against the
  real Wayland server, but not KMS scanout.

This runbook is the manual **KMS system test** layer. It is not a substitute
for a future automated HIL runner; it is the acceptance procedure until that
runner exists.

## What This Test Proves

- The candidate policy accepts only a topmost, single-surface, opaque,
  identity-mapped dma-buf window.
- The backend can import the candidate and validate Primary + Cursor + Overlay
  as one atomic `TEST_ONLY` request.
- The real commit scans out Primary + Cursor + Overlay together and retires the
  overlay client buffer with the page flip.
- Chrome, overview, switcher, capture, lock, VT switch, and hotplug correctly
  deactivate overlay and repaint Primary.
- Failure falls back to composition without leaving a hole or a stale client
  buffer.

It does **not** prove the final performance shape: the first path still
submits Primary alongside overlay content updates. Overlay-only commits are a
follow-up optimization.

## Prerequisites

- A seat manager (`seatd` or `logind`) and a user in the `seat` group.
- A free VT and, ideally, a second VT or SSH session for recovery.
- The repository checked out at the revision under test.
- Build the binary **before** switching VTs.
- Never run the DRM backend as root.

On a multi-GPU machine, identify the card that owns the display connectors and
set `TESSERA_DRM_DEVICE` explicitly.

## Build

From the graphical session:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo build --profile release-fast -p tessera
```

## Persistent Evidence

Write logs under the user state directory, not `/tmp`, so they survive a
reboot and can be read from the graphical session or another getty:

```bash
LOG_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test
mkdir -p "$LOG_DIR"
```

Use a timestamped file for each run:

```bash
LOG="$LOG_DIR/overlay-enabled-$(date +%Y%m%d-%H%M%S).log"
```

## Baseline Run (Overlay Disabled)

Use this only when you need to separate a normal DRM failure from an overlay
failure. Switch to a free VT (`Ctrl+Alt+F3`), log in as the seat user, and run:

```bash
cd "$(git rev-parse --show-toplevel)"
LOG_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/baseline-$(date +%Y%m%d-%H%M%S).log"

TESSERA_BACKEND=drm \
RUST_LOG='info,tessera_platform::drm=debug' \
target/release-fast/tessera 2>&1 | tee "$LOG"
```

Confirm the normal desktop works before enabling overlay:

- outputs light up at the expected mode;
- pointer, keyboard, focus, and window movement work;
- direct scanout still logs `direct scanout active` for an eligible full-output
  client;
- no repeated atomic-commit or page-flip errors.

Do not continue if the baseline fails.

## Audit Run (No Allocation)

Use this only when the overlay system test does not activate overlay and you
need to see why. It exercises candidate gathering, rejection telemetry, and the
complete-set `TEST_ONLY` probe. It never allocates or presents an overlay.

```bash
cd "$(git rev-parse --show-toplevel)"
LOG_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/audit-$(date +%Y%m%d-%H%M%S).log"

TESSERA_BACKEND=drm \
RUST_LOG='info,tessera::runtime::scanout=debug,tessera::runtime::presentation=debug,tessera_platform::drm=debug,tessera_composite::presentation::overlay=debug' \
target/release-fast/tessera 2>&1 | tee "$LOG"
```

Inspect the audit:

```bash
grep -nE 'plane allocation|overlay candidate|overlay offload|overlay TEST_ONLY|kms-rejected' "$LOG"
```

Expected startup line:

```text
drm: plane allocation: N primary assigned, N cursor assigned, N overlay available
```

If no overlay candidate is ever accepted, the rejection labels in
[Failure Triage](#failure-triage) explain why.

## Enabled Run

```bash
cd "$(git rev-parse --show-toplevel)"
LOG_DIR=${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test
mkdir -p "$LOG_DIR"
LOG="$LOG_DIR/overlay-enabled-$(date +%Y%m%d-%H%M%S).log"

TESSERA_OVERLAY_OFFLOAD=1 \
TESSERA_BACKEND=drm \
RUST_LOG='info,tessera::runtime::scanout=debug,tessera::runtime::presentation=debug,tessera_platform::drm=debug,tessera_composite::presentation::overlay=debug' \
target/release-fast/tessera 2>&1 | tee "$LOG"
```

Multi-GPU:

```bash
TESSERA_OVERLAY_OFFLOAD=1 \
TESSERA_DRM_DEVICE=/dev/dri/card1 \
TESSERA_BACKEND=drm \
RUST_LOG='info,tessera::runtime::scanout=debug,tessera::runtime::presentation=debug,tessera_platform::drm=debug,tessera_composite::presentation::overlay=debug' \
target/release-fast/tessera 2>&1 | tee "$LOG"
```

Inspect the enabled run:

```bash
grep -nE 'overlay offload enabled|plane allocation|overlay offload active|overlay offload inactive|overlay TEST_ONLY rejected|overlay presentation rejected|kms-rejected' "$LOG"
```

Expected activation:

```text
drm: overlay offload enabled for hardware validation (complete-set TEST_ONLY still required)
drm: plane allocation: ...
... overlay offload active for window ...
```

Expected deactivation/fallback:

```text
... overlay offload inactive; compositing
... overlay TEST_ONLY rejected ...
... overlay offload blocked (kms-rejected) ...
... overlay presentation rejected; falling back to composition
```

## Candidate Setup

The first policy accepts one topmost, single-surface, opaque dma-buf toplevel
with identity geometry. It rejects floating windows that cast compositor
shadows, windows with transitions or attention feedback, and any window
overlapped by chrome, a software cursor, IME popup, drag icon, or client cursor
surface.

The reliable trigger is **two tiled windows with an opaque video client on
top**. Set `~/.config/tessera/config.toml`:

```toml
[layout]
default_tiled = true

[[window_rule]]
app_id = "mpv"
role = "tiled"
```

If `[layout]` or `[[window_rule]]` already exists, merge the keys instead of
duplicating the table.

Launch a terminal and a video client:

```bash
foot &
mpv --no-config --wayland-app-id=mpv --vo=gpu --gpu-context=wayland /path/to/video.mkv &
```

Focus the video window so it is the topmost tiled window. A fullscreen client
usually selects direct scanout instead; that is intentional and does not
exercise overlay.

Alternative single-surface dma-buf clients such as `weston-simple-egl` or
`vkcube-wayland` also work if they do not create subsurfaces or CSD.

## Verification Matrix

| Scenario | Action | Expected |
| --- | --- | --- |
| Activation | Two tiled windows, focus the opaque video window | `overlay offload active for window ...` |
| Direct-scanout precedence | Make the same client fullscreen, hide chrome | `direct scanout active`; no overlay placement or overlay commit |
| Buffer updates | Play, pause, seek, and scrub the video | No flicker, stale bands, or tearing |
| Chrome overlap | `Super+S` (command panel) | Overlay deactivates; `overlay offload inactive; compositing` |
| Overview | `Super+O` | Overlay deactivates; full scene repaints |
| Window switcher | `Super+Tab` | Overlay deactivates; switcher previews remain complete |
| Screenshot | `tessera display capture /tmp/overlay-active.png` | Capture includes the overlay content; overlay temporarily deactivates |
| Lock/unlock | `Super+L`, then unlock | No stale overlay region; unlock redraws the complete scene |
| VT switch | `Ctrl+Alt+F2`, then back to the tessera VT | Clean resume; at most one skipped frame |
| Hotplug | Unplug and replug the monitor or dock | Topology reprobes; no stale planes or crash |
| Cursor | Move the hardware cursor across the video | Smooth cursor; no overlay/cursor artifacts |
| Window change | Move, resize, close, or until the video window | Overlay deactivates and Primary repaints the old rectangle |
| Focus change | Raise a normal window above the video | Overlay deactivates; no hole under the raised window |

Useful commands:

```bash
tessera display
tessera display capture /tmp/overlay-active.png
```

## Failure Triage

Candidate policy rejection labels come from
`crates/tessera-composite/src/presentation/overlay.rs`:

| Label | Meaning | Next check |
| --- | --- | --- |
| `session-locked` | Session lock owns the frame | Unlock and retry |
| `capture-pending` | Screenshot/stream/Interaction Domain capture is active | Stop capture and retry |
| `screenshot-freeze` | Freeze session owns the frame | Close the selector/freeze session |
| `surface-transition` | A window/workspace transition is active | Wait for it to settle |
| `overview` / `window-switcher` | A compositor surface owns the scene | Close it |
| `no-candidate` | No eligible toplevel was gathered | Check the window is mapped and visible |
| `candidate-mismatch` | Candidate surface does not belong to the selected window | Report a bug |
| `not-topmost` | Another toplevel is above the candidate | Raise the candidate |
| `not-single-surface` | Subsurface/popup tree present | Use a single-surface client |
| `shm-surface` | Candidate or its tree uses SHM | Use a dma-buf client |
| `client-overlay` | IME, drag icon, or client cursor overlay is visible | Close it |
| `scene-feedback` | Shadow, attention pulse, modal dim, or transition | Use tiled/maximized window |
| `not-opaque` | Alpha or incomplete opaque region | Use XRGB or a full opaque region |
| `not-identity-mapped` | Scaling, crop, viewport, or non-normal transform | Use a 1:1 buffer |
| `color-managed` | Plane cannot express the surface color transform | Disable the color-managed path for now |
| `chrome-overlap` | Dock/panel/menu intersects the candidate | Hide chrome or use the work area |
| `software-cursor-overlap` | No hardware cursor plane while cursor is visible | Check cursor-plane allocation |
| `outside-output` | Rectangle is not wholly inside one output | Move the window |
| `no-overlay-plane` | No overlay plane discovered for this CRTC | Inspect the startup plane allocation log |
| `plane-format` | Format/modifier unsupported by the overlay plane | Inspect plane formats/modifiers |
| `kms-rejected` | Complete-set `TEST_ONLY` failed, including plane geometry, `zpos`, fence, or color constraints | Collect the kernel error and plane state |

`overlay presentation rejected; falling back to composition` means the
`TEST_ONLY` or real commit failed after policy acceptance. The runtime disables
overlay and retries a full composited frame. Collect the complete error line.

## Evidence To Collect

For a passing run, keep:

- the persistent log path used for the run
  (default `${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test/`);
- `uname -a`;
- `lspci -k | grep -EA3 'VGA|3D'`;
- `tessera display`;
- the candidate client (`mpv`/`weston-simple-egl`/`vkcube-wayland`) and
  whether it used XRGB or ARGB;
- the exact scenarios exercised and their timestamps.

For a failing run, also include:

```bash
LOG=${XDG_STATE_HOME:-$HOME/.local/state}/tessera/kms-system-test/overlay-enabled-<timestamp>.log
grep -nE 'plane allocation|overlay|kms-rejected|TEST_ONLY|drm:' "$LOG"
```

If `drm_info` is installed, include its plane list:

```bash
drm_info > /tmp/drm-info.txt
```

## Exit Criteria

Removing the `TESSERA_OVERLAY_OFFLOAD` gate and enabling overlay offload by
default may happen only after:

- [ ] Activation and deactivation work on at least one real KMS driver.
- [ ] The complete-set `TEST_ONLY` is accepted for the candidate format.
- [ ] Video updates show no flicker, tearing, stale bands, or holes.
- [ ] Chrome, overview, switcher, screenshot, lock, VT switch, and hotplug all
      deactivate and repaint correctly.
- [ ] Cursor motion remains smooth with no overlay/cursor artifacts.
- [ ] Multi-output movement places the overlay on the correct CRTC or falls
      back safely.
- [ ] Buffer release produces no use-after-free, stuck buffers, or frame
      callbacks stalled behind the overlay path.
- [ ] The remaining `kms-rejected` and policy rejection counts are understood
      and documented.

The architecture is already ratified by
[ADR-0235](../../adr/0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md);
this runbook is the hardware gate for changing the default policy from
`Disabled` to enabled.

## Related Documents

- [Manual Testing Playbook](manual-testing.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Rendering and KMS Plane Reference](../../reference/rendering.md)
- [How to Run tessera on Bare Metal](../../how-to/bare-metal-drm.md)
- [ADR-0234: Output layers are plane-allocation contracts](../../adr/0234-output-layers-are-plane-allocation-contracts-not-scene-layers.md)
- [ADR-0235: Overlay offload candidate policy](../../adr/0235-overlay-offload-is-a-topmost-opaque-single-surface-candidate.md)
- [ADR-0237: A direct-scanned Primary excludes overlay offload](../../adr/0237-a-direct-scanned-primary-excludes-overlay-offload.md)
