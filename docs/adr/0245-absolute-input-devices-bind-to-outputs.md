---
id: ADR-0245
title: "Absolute Input Devices Bind to Outputs and Use the Letterbox Content Rectangle"
status: accepted
date: 2026-11-18
scope: primitives/input, config, platform/drm, docs
superseded_by: null
negative_knowledge: true
---

# 0245. Absolute Input Devices Bind to Outputs and Use the Letterbox Content Rectangle

- Status: Accepted
- Date: 2026-11-18
- Amends: [ADR-0243](0243-output-roles-and-mirroring.md)

## Context

Mirroring fits the source aspect ratio into the target mode and leaves black
bars when the aspects differ. Absolute input devices — touchscreens,
graphics tablets, and absolute pointers — still normalized their coordinates
across the whole union framebuffer. On a 16:10 source shown on a 16:9 panel,
the top and bottom of the touch panel therefore addressed desktop edges while
the visible content occupied only the center. There was also no way to say
which physical panel a touch device belonged to when several outputs were
attached.

## Decision

`[input]` accepts repeated `[[input.mapping]]` entries:

```toml
[[input.mapping]]
device = "Wacom One by Wacom S"
output = "HDMI-A-1"
```

`device` is matched against the libinput device name exactly, then its stable
sysname. `output` is a connector identifier. The backend resolves the binding
on every event against the live `DisplaySet`; no cached rectangle survives a
hotplug or mode change.

An absolute event is normalized over its device range, then mapped into the
output's **content fraction**. For a mirror the content fraction is the fitted
destination rectangle in the target mode, so coordinates inside a bar clamp to
the nearest visible edge. The result is converted through the source output's
scale into global logical desktop coordinates. Relative events are untouched,
including relative motion from a device that also has an absolute mapping.

No mapping means the historical whole-union mapping. The mapping applies to
touch down/motion and absolute pointer motion; tabletop/tablet coordinate
handling remains on the same normalized union contract until it gains the same
device binding.

## Invariants

- `[INV-ARCH-145]` A mapped absolute coordinate always lands inside the
  output's logical content rect, never in a letterbox bar.
- `[INV-ARCH-146]` The binding is resolved against the live output set at
  event time; a stale mode/topology cannot redirect input.
- `[INV-ARCH-147]` Input Settings preserves the mapping array verbatim when
  it persists pointer/keyboard preferences.

## Consequences

- Touch and pen input can target a mirrored TV or projector correctly even
  when the panel aspect differs from the source.
- Multi-output absolute devices can be assigned explicitly instead of relying
  on union-wide normalization.
- The mapping is file-first; the Input Settings module does not yet provide a
  visual device-to-output picker. The field is validated for non-empty,
  bounded names and duplicate device bindings.
