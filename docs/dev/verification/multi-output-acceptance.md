[Verification Index](index.md) > [Manual Verification Playbook](manual-verification.md) > Multi-Output Acceptance

---

# Multi-Output Acceptance

## Scope

The Layer 4 acceptance gate for output roles and mirroring
([ADR-0243](../../adr/0243-output-roles-and-mirroring.md),
[ADR-0246](../../adr/0246-mirror-scaling-falls-back-to-a-compositor-target.md)),
tentative display edits ([ADR-0244](../../adr/0244-tentative-display-edits-revert.md)),
absolute input binding ([ADR-0245](../../adr/0245-absolute-input-devices-bind-to-outputs.md)),
and per-output presentation
([ADR-0247](../../adr/0247-per-output-render-targets.md),
[ADR-0248](../../adr/0248-capture-composition-is-separate-from-presentation.md),
[ADR-0249](../../adr/0249-presentation-groups-and-per-output-pacing.md)).
Run it after the nested checks pass. It requires two real outputs on one DRM
card; a single-panel laptop cannot complete it.

## Build and identify hardware

```bash
meson compile -C ../optics/build
cargo build --locked --release -p tessera
for status in /sys/class/drm/card*-*/status; do
  printf '%s: %s\n' "$status" "$(cat "$status")"
done
```

Record the card and connector names. At least two connectors must read
`connected`.

## Recovery

Keep an SSH session or a second VT available before starting. If the outputs
become unusable, run `pgrep -a -x tessera` from it, verify the PID, and stop
that process with `kill -TERM <pid>`. Never unplug the only display that shows
the recovery console.

## Start with verbose output diagnostics

From a bare VT:

```bash
TESSERA_BACKEND=drm \
TESSERA_DRM_DEVICE=/dev/dri/card1 \
RUST_LOG=tessera=debug,tessera_platform=debug,tessera_wayland=debug \
target/release/tessera 2>&1 | tee /tmp/tessera-mirror.log
```

Replace `card1` with the card that owns the tested connectors.

## Matrix

### 1. Extend and mirror resolution

```bash
tessera display
tessera display role HDMI-A-1 extend
tessera display role HDMI-A-1 mirror eDP-1
tessera display confirm
```

Check:

- [ ] `tessera display` reports one `Extend` source and one `Mirror`.
- [ ] The mirror shows the source workspace, windows, chrome, and pointer.
- [ ] Workspace switching, window focus, and input target the source output;
      the mirror owns no bootstrap workspace.
- [ ] A mirror is never reported as primary.

### 2. Mirror letterbox

Use a source and target with different aspect ratios (for example 16:10
internal and 16:9 TV), then force a target mode with the other aspect:

```bash
tessera display
# Use Settings or the output config to select the mismatched mode.
```

Check:

- [ ] Content is centered with black bars; it is not stretched.
- [ ] The source and mirror keep the same logical window coordinates.
- [ ] `tessera display capture` contains the source content, not the bars.
- [ ] The log reports `mirror target active for <connector>` and the target
      frame is committed at native mode. KMS plane scaling must not be
      requested for the mirror.
- [ ] The source and mirror page flips retire separately.
- [ ] With the hardware cursor active, the pointer is visible at the same
      logical position on the mirror target.
- [ ] If target creation/commit fails, the log reports the target failure and
      the next frame returns the mirror to the union path.

### 3. Letterbox absolute input

Bind the touchscreen or pen to the mirror connector in `config.toml`:

```toml
[[input.mapping]]
device = "ELAN Touchscreen"   # exact libinput name or sysname
output = "HDMI-A-1"
```

Restart the compositor and test:

- [ ] Touch at the four corners of the visible content reaches the four
      desktop corners.
- [ ] Touching a black bar clamps at the nearest content edge; it never jumps
      to another output or a desktop corner.
- [ ] A relative mouse with the same device name still moves relatively.
- [ ] Unplugging the mirror falls back to union mapping; touching the source
      panel still works.
- [ ] `tessera display` still shows the source logical rect for the mirror.

### 4. Multiple simultaneous direct scanouts

Attach two outputs, arrange them as `Extend`, hide shell chrome, and open one
opaque dma-buf fullscreen client on each. Use two independent GL/Vulkan
clients, not two windows of one client.

Check:

- [ ] Both outputs enter direct scanout in the same frame.
- [ ] The log has one target-scoped commit per CRTC, not a union commit.
- [ ] Each client receives `wl_callback.done` at its own output's cadence.
- [ ] `wp_presentation_feedback` is delivered only for the targeted output.
- [ ] A window or chrome appearing over either output exits direct scanout for
      both coherently.
- [ ] A client that stops committing does not stall the other output's
      callback/damage scope.
- [ ] `TESSERA_OVERLAY_OFFLOAD` does not combine with direct primary scanout.

### 5. Tentative display revert

```bash
tessera display role HDMI-A-1 extend
# Do not run `tessera display confirm`.
```

Check:

- [ ] The previous role/mode/scale/position returns after 15 seconds.
- [ ] Closing Settings does not cancel the timer.
- [ ] `tessera display confirm` inside the window keeps the change.
- [ ] A second tentative edit replaces the pending record rather than
      stacking timers.
- [ ] A newly connected connector with no previous entry does not advertise a
      countdown it cannot honor.

### 6. Per-output presentation targets

- [ ] The log reports `output target active` for every physical connector.
- [ ] Normal desktop frames commit each output target at native mode; no
      union framebuffer is scanned on the ordinary path.
- [ ] A window or pointer change on one output leaves the other target's
      last image intact and does not force a full repaint of both.
- [ ] A screen capture or active stream composes the dedicated union capture
      image, uploads it into every output target, and still presents each CRTC
      through its own target. The capture image must not be KMS-scanned.
- [ ] Overview, window switcher, live previews, soft shadows, 3D wallpaper,
      and workspace slides render through per-output targets without selecting
      a union presentation path.
- [ ] A slow output does not pin the other output's target ring; page flips
      and callbacks remain independently scoped.
- [ ] Outputs in one presentation group flip in one atomic KMS request.
- [ ] A disjoint fast group continues while a slow group is blocked.
- [ ] Drag a window across outputs with different refresh rates; the groups
      merge while it spans, then split after it leaves the seam.
- [ ] `wp_presentation_feedback.refresh` follows the target output's refresh,
      not the session-wide slowest rate.

## Evidence

Keep `/tmp/tessera-mirror.log`, `tessera display --json` before and after, and
the exact connector/mode matrix. A pass requires all applicable boxes; record
the date, hardware, kernel driver, GPU, and firmware in the
[Support Matrix](support-matrix.md).

## Release rule

A change to output roles, mirroring, display configuration, absolute input
mapping, or the per-output presentation path is not release-ready until every
applicable section of this gate passes on two physical outputs.
