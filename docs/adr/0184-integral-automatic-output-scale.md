---
id: ADR-0184
title: "Integral Automatic Output Scale"
status: accepted
date: 2026-10-20
scope: desktop/output, platform/drm, wayland/output, config/reference
superseded_by: null
negative_knowledge: true
---

# 0184. Integral Automatic Output Scale

- Status: Accepted
- Date: 2026-10-20
- Deciders: Tessera Maintainers
- Amends: [ADR-0028](0028-output-and-monitor-model.md), [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md)

---

## Context and Problem Statement

A DRM session with no `[[output]] scale` entry derived its scale from physical
pixel density and quantized it to quarter steps: 1.25, 1.5, 2.75. The intent was
nicer logical density on HiDPI panels than the 100%/200% dichotomy allows. The
cost was never priced.

An output scale is fundamentally an integer:

- `wl_output.scale` is an `i32`.
- `wp_fractional_scale_v1.preferred_scale` is opt-in per surface. A client that
  binds `wp_fractional_scale_v1` and `wp_viewporter` can render at exactly the
  fractional density.
- Every client that does not — which includes GLFW, and therefore Minecraft
  and most games; also Xwayland, SDL's non-Wayland path, and any toolkit that
  has not adopted fractional scale — must allocate
  `ceil(scale) × logical_extent`.

So a 1.5 recommendation on a 3840×2160 panel configures the toplevel at
2560×1440 logical and the client allocates **5120×2880**: 1.78× the pixels of
the physical mode, downsampled back to 3840×2160 by the compositor, for no
visible benefit over 200%. At 1.25 on a 1920×1080 laptop panel the client
allocates 3072×1728: **2.56×**. That is a pure fill-rate tax on every frame the
client draws.

It is also disqualifying for presentation. `evaluate_surface` requires the
client's buffer extent to equal the output mode extent; it always did. A
`ceil`-scaled buffer can never equal the mode, so a covering, opaque, otherwise
perfect scanout candidate is rejected `geometry-mismatch` on every frame — the
zero-cost path is unreachable by construction on every output whose automatic
scale is fractional.

The two failure modes compound exactly where frame rate matters most: HiDPI
laptops, and any desktop whose panel is dense enough to trigger the ladder.

## Decision Drivers

- **One pixel grid identity**: `logical_extent × scale == physical_extent` must
  hold for the recommended configuration, so a client's buffer equals the mode
  and direct scanout stays reachable.
- **Do not tax clients for a density preference they cannot express.** The
  automatic recommendation must be expressible in the protocol subset every
  client implements.
- **Keep the recommendation honest about mainstream defaults**: 100% for
  ordinary panels, 200% for dense ones, which is what GNOME and KDE default to.
- **Do not remove user authority**: an explicitly configured fractional scale
  must remain representable and must be diagnosable.

## Decision Outcome

`tessera_desktop::output::automatic_scale` returns **integral scales only**. The
ladder picks the largest step in `1..=4` whose resulting logical pixel density
stays at or above the display class's floor (`105` PPI for internal panels,
`80` PPI for external ones). The floor, not a target density, is the whole
knob: it bounds "the UI got too small", while the clamp bounds "the UI got too
large".

Resulting behaviour:

| Panel | PPI | Automatic scale | Logical extent |
|---|---|---|---|
| 15.6" 1920×1080 internal | 141 | `1.0` | 1920×1080 |
| 27" 2560×1440 external | 109 | `1.0` | 2560×1440 |
| 27" 3840×2160 external | 163 | `2.0` | 1920×1080 |
| 14.4" 3072×1920 internal | 250 | `2.0` | 1536×960 |

`AUTO_SCALE_STEP`, `INTERNAL_TARGET_PPI`, and `EXTERNAL_TARGET_PPI` are removed:
there is no fractional quantization left to parameterize.

An explicitly configured fractional scale (`[[output]] scale`) is still
accepted, because it is the user's decision and it is the only way to reach a
density the integer ladder cannot express for clients that *do* speak
`wp_fractional_scale_v1`. `Server::set_outputs` logs the concrete consequence at
`warn` when the effective scale is fractional: the integer multiplier legacy
clients will render at, and the fact that it blocks direct scanout. The
documentation states the same trade in
[Configuration Reference](../reference/config.md#outputs).

`integer_output_scale` (the `ceil` used for `wl_output.scale`) is unchanged: for
an integral scale it is the identity, and for an explicitly configured
fractional scale `ceil` is the only sound advertisement — it is the smallest
integer scale at which a legacy client renders crisply.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-73] Integral Recommendation Grid`: an automatic output scale is
  always an integer, so `logical_extent × scale == physical_extent` for every
  automatically configured output. Enforced by
  `test_inv_arch_73_*` in `crates/tessera-desktop/src/output.rs`, which sweeps a
  PPI range and asserts integrality, and by `automatic_scale` having no
  fractional arithmetic left.
- The ladder never recommends below 100% or above `MAX_AUTO_SCALE`.
- A configured fractional scale must not be silently rounded: it is applied
  verbatim to the geometry and reported at `warn`.

## Verification

- `crates/tessera-desktop/src/output.rs`: `automatic_scale` cases for internal
  and external panels, the integral sweep, and the implausible-metadata
  rejections.
- `crates/tessera-wayland/src/tests/output_damage.rs`: a configured scale still
  overrides the recommendation and still drives the logical geometry.
- Hardware: on a panel whose PPI previously produced a fractional
  recommendation, a full-output client's reported framebuffer size must equal
  the mode, and the log must show `direct scanout active` rather than
  `direct scanout blocked (geometry-mismatch)`.

## Consequences

### Positive
- The recommended configuration keeps client buffers on the physical grid: no
  1.78–2.56× fill-rate tax, and direct scanout is reachable on every panel.
- The recommendation matches mainstream desktop defaults instead of inventing a
  density the protocol cannot express.
- The fractional-scale cost is now a diagnosable, named warning instead of an
  invisible tax.

### Negative / Trade-offs
- A dense laptop panel now lands on 100% rather than a comfortable 125%. Users
  who want 125% can configure it explicitly and inherit the documented cost.
- A 4K external panel lands on 200% rather than 150%: text and chrome are
  larger than before on those panels. This is the trade that buys every client
  an exact 1:1 buffer.
- Removing the target-PPI constants changes a user-visible default; release
  notes must call it out (`CHANGELOG`).

## Rejected Alternatives & Negative Knowledge

### 1. Keep the fractional ladder and allow scaled direct scanout
- **Why considered**: A 5120×2880 buffer can be programmed onto a 3840×2160
  plane with `src_w`/`src_h` scaling; the plane would then downsample in
  hardware and the compositor would still skip its own render.
- **Why rejected**: Primary-plane scaling is not universally supported at
  arbitrary ratios, its filter quality is driver-specific and often bilinear at
  best, and the failure mode is a black screen or a blurry one on hardware we
  cannot enumerate. It also multiplies the client's GPU cost by 1.78× to reach
  the same displayed pixels — the tax this ADR removes.

### 2. Force `wp_fractional_scale_v1` on every client
- **Why considered**: Then any scale would be exact and the tax would vanish.
- **Why rejected**: The compositor cannot force a client to bind an optional
  protocol. GLFW, Xwayland, and every pre-2023 toolkit would still be on the
  `ceil` path, and the default would still be wrong for them.

### 3. Keep fractional recommendations but configure covering clients at a
scale-1 extent
- **Why considered**: "Fullscreen is special, give it the native grid."
- **Why rejected**: `wl_output.scale` is a global, not a per-surface property;
  a client chooses its buffer size from that global. The only per-surface
  mechanism is `wp_fractional_scale_v1.preferred_scale`, which is exactly the
  opt-in the legacy client does not have. The result would be a scale-1 buffer
  upscaled to the output — blurry, not native.

### 4. Round the recommendation to the nearest integer without a density floor
- **Why considered**: One line, no constants.
- **Why rejected**: `round(ppi / target)` collapses a 163 PPI 27" 4K panel to
  100% (unusable text) and a 141 PPI laptop to 100% while pushing a 250 PPI
  panel to 200% by accident. The floor is what makes the ladder express *why*
  a step is chosen, and it is the only tunable that survives new hardware
  classes.

### 5. Reject fractional scales outright
- **Why considered**: Simplest invariant: scale is always an integer.
- **Why rejected**: It removes the only way to reach 125%/150% for clients that
  do support `wp_fractional_scale_v1`, and it breaks existing configurations at
  load. Diagnose instead of forbid.
