---
id: ADR-0163
title: "Decoupled Backdrop Blur Cache and Continuous Chrome Fade"
status: accepted
date: 2026-09-19
scope: core/rendering, shell/control-center
superseded_by: null
negative_knowledge: true
---

# 0163. Decoupled Backdrop Blur Cache and Continuous Chrome Fade

- Status: Accepted
- Date: 2026-09-19
- Deciders: Tessera Graphics & Shell Architects
- Consulted: Compositor & Rendering Team
- Informed: System Team

---

## Context and Problem Statement

When opening and closing the Control Center (and surfaces declaring a full-screen `BackdropCover`), users observed severe dropped frames, hitching, and visual judder.

Investigation revealed four compounding defects:
1. **Unnecessary Scene Re-blurring**: In `BackdropGraphExecutor::recompute_effects`, whenever `BackdropPlan::Recompute` ran (e.g. on material opacity changes), the executor unconditionally re-dispatched the 4-pass Dual-Kawase pyramid blur across the full physical resolution (1080p/1440p/4K), even though the underlying desktop scene capture and blur radius were 100% static and unchanged.
2. **Quantization Artifacts (`stepped_fade`)**: To mask the recompute cost, a prior workaround quantized the fade into 8 discrete steps (`(fade * 8.0).round() / 8.0`). This produced 8 separate 15ms GPU frametime spikes while causing the backdrop to visually step and stutter at 8 FPS against smooth 60 FPS sliding UI panels.
3. **Phase Mismatch**: `advance(dt)` was called during `Chrome::render()`, *after* `self.shell.backdrop_layers()` was queried for the frame. As a result, the backdrop's declared reveal lagged the foreground chrome geometry by 1 frame.
4. **Coarse Damage Invalidation**: Tooltip micro-interactions in the Control Center fell back to conservative `None` (full output damage), forcing continuous full-output repaints during hover reveals.

## Decision Drivers

- **Zero-Drop High-Refresh Execution**: Animation transitions must sustain locked 60/120/144 FPS (<1.5ms per frame GPU time).
- **Continuous Fluidity**: Optical effects (frost and scrim wash) must blend continuously without discrete quantization steps.
- **Architectural Soundness**: Preserve ADR-0149's model where blur radius stays constant across the fade, while decoupling expensive filter passes (Dual-Kawase pyramid) from lightweight material blending (Prism compute dispatch).
- **Correct Temporal Phase**: Chrome state and backdrop declarations must share identical timestamps without phase lag.

## Considered Options

- **Option 1 (Chosen)**: Decouple blur execution from material recompute in `flux` and `tessera`; borrow cached blurred output on `Recompute`; eliminate `stepped_fade` in favour of continuous hardware interpolation; synchronize state updates to `prepare_backdrop`.
- **Option 2**: Downsample the entire capture buffer (`BACKDROP_DOWNSAMPLE = 4`).
- **Option 3**: Fade the composite image via alpha blending on output composition.

## Decision Outcome

Chosen option: **Option 1**.

1. **Blur Slot Caching**:
   - `flux_blur_filter` tracks slot output validity across recording frames.
   - `flux_blur_filter_current` allows borrowing the slot's valid blurred texture without re-recording the 4 compute passes of the Dual-Kawase pyramid.
   - `BackdropGraphExecutor::recompute_effects` reuses the blurred output during material-only updates (`force_blur: false`), cutting per-frame GPU time from >15ms to ~0.2ms.
2. **Continuous Hardware Fade**:
   - Removed `stepped_fade` from `BackdropCover::region`. Opacity and scrim wash now interpolate smoothly on every frame.
   - Prism's compute shader blends the sharp desktop and cached blurred texture at native display refresh rates.
3. **Temporal Alignment**:
   - `ControlCenter` implements `Chrome::prepare_backdrop`, advancing its animation clocks and watchers before backdrop declarations and damage policy are queried.
4. **Localized Tooltip Damage**:
   - `animated_damage_region` bounds work-mode and session tooltip hover reveals to their cluster regions with headroom padding, preventing unneeded full-screen damage.

### Invariants & Behavioral Boundaries

- `[INV-RENDER-01] Material Recompute Blur Reuse`: When a frame is planned as `BackdropPlan::Recompute`, `BackdropGraphExecutor` MUST reuse the slot's valid blurred image and MUST NOT re-record the scene blur passes unless the capture or blur radius has been invalidated.
- `[INV-CHROME-01] Phase-Aligned Chrome Step`: Chrome components declaring dynamic backdrop regions MUST advance their animation clocks in `prepare_backdrop` so that backdrop declarations and render passes evaluate with zero frame lag.

## Rejected Alternatives & Negative Knowledge

### Option 2: Downsampling Backdrop Capture (`BACKDROP_DOWNSAMPLE = 4`)
- **Why considered**: Downsampling reduces the pixel fill-rate for Gaussian blur.
- **Why rejected**: Rejected by [ADR-0143](0143-explicit-offscreen-composition-dag.md). Liquid-glass refraction samples the sharp capture directly; downsampling produces a blurry smear behind sharp glass edges and distorts subpixel UI alignment.

### Option 3: Fading Cover via Output Alpha Blending
- **Why considered**: Drawing the final composite with alpha over the sharp desktop avoids executing any shaders on fade.
- **Why rejected**: Rejected by [ADR-0149](0149-backdrop-covers-fade-with-chrome.md). Compositing the cover as a translucent sheet leaves the layered material's output non-opaque, breaking downstream layers that sample `resolved` offscreen targets and causing premultiplied darkening artifacts.

## Consequences

### Positive
- Control Center enter and exit animations run at a locked 60/120/144 FPS with no dropped frames.
- Backdrop scrim and blur opacity fade continuously and seamlessly.
- Zero 1-frame temporal jitter between sliding panels and background frost.
- Tooltip animations no longer trigger full-screen repaints.

### Negative / Trade-offs
- `flux_blur_filter` maintains an internal validity state per frame slot, requiring that frame slot lifecycles adhere to Vulkan fence retirements.