---
id: ADR-0258
title: "Client Cursor Hardware Plane Offload and Per-Output Overlay Parity"
status: accepted
date: 2026-11-25
scope: platform/drm, wayland/server, runtime/presentation, runtime/output_target_render, runtime/scanout, runtime/input
superseded_by: null
negative_knowledge: true
---

# 0258. Client Cursor Hardware Plane Offload and Per-Output Overlay Parity

- Status: Accepted
- Date: 2026-11-25
- Deciders: Tessera Maintainers
- Amends: [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md), [ADR-0219](0219-asynchronous-direct-scanout-and-wp-tearing-control.md), [ADR-0247](0247-per-output-render-targets.md)

---

## Context and Problem Statement

Wayland clients set cursors through two mechanisms: `wp_cursor_shape_v1` (compositor-owned theme sprites) and `wl_pointer.set_cursor(serial, surface, hotspot_x, hotspot_y)` (client-owned cursor `wl_surface`, used extensively by SDL2/SDL3, GLFW, games with custom cursors, and toolkits without cursor-shape).

Two compounding defects existed on the native DRM path:

1. **Per-output rendering dropped client cursor surfaces completely.** When a client attached a custom cursor surface, `pointer_set_cursor` set `runtime.cursor_hidden = true` (disabling the KMS hardware cursor plane) and exposed the surface via `overlay_frames()`. However, the per-output DRM render path (`render_output_targets` in `output_target_render.rs`, ADR-0247) called `draw_client_overlays(..., include_cursor: false, None)`. As a result, the KMS hardware cursor was disabled *and* the client cursor overlay was skipped, making custom cursors completely invisible on ordinary DRM frames.
2. **Custom client cursors needlessly disqualified Direct Scanout and cursor-only fast paths.** Even after fixing overlay drawing, treating every client cursor surface as a composited scene overlay forced `client_overlay_count > 0`, which rejected Direct Scanout (`ADR-0182`) and asynchronous tearing flips (`ADR-0219`) and turned every pointer motion event into a full Vulkan composite pass.

## Decision Drivers

- **Zero missing cursors on every presentation path**: Every `wl_pointer.set_cursor(surface)` cursor must be visible whether presented via KMS cursor plane, per-output render targets, union fallback, mirrors, screenshots, or streams.
- **Preserve Direct Scanout and async tearing for fullscreen games with custom cursors**: Games that display a custom Wayland cursor surface (menus, RTS/MOBA/RPG cursors) must retain Direct Scanout and out-of-band cursor-plane motion commits when the cursor fits the KMS cursor plane.
- **Never poison hardware cursor availability**: An oversized, rotated, viewported, or dma-buf client cursor must fall back cleanly to composited overlay rendering without triggering `disable_hardware_cursor()`.
- **No double cursors**: A client cursor surface is presented on *either* the KMS cursor plane *or* the composited overlay pass, never both.

## Decision Outcome

1. **Expose KMS cursor plane limits and client cursor snapshots.**
   - `Host::hardware_cursor_extent()` / `DrmHost::hardware_cursor_extent()` report the active KMS cursor plane maximum dimensions `(width, height)`.
   - `Server::client_cursor_surface()` exposes `ClientCursorSurface` (resource id, generation, SHM BGRA8 pixels, logical hotspot, `attach_offset`, `buffer_scale`, `transform`, and viewport fields) for the active human-seat cursor surface.
2. **Pure eligibility & HiDPI scaling seam (`cursor::prepare_client_hardware_cursor`).**
   - A client SHM cursor surface is eligible for KMS cursor-plane offload iff `transform == Transform::Normal`, no active viewport, tightly packed BGRA8 buffer, an exact integer ratio `output_scale / buffer_scale >= 1` (zero-copy `Cow::Borrowed` at 1:1; nearest-neighbor integer upscale for 1x game cursors on 2x HiDPI displays), physical size within `hardware_cursor_extent`, and physical hotspot `(hotspot - attach_offset) * output_scale` inside the sprite bounds.
   - Offload is active only on direct DRM frames without mirrors, live streams, pending captures, screenshot freezes, session lock, or virtual-output pointer residence.
3. **Unified plane ownership across presentation, scanout, damage, input, and per-output rendering.**
   - When `client_cursor_on_hw_plane` is true: the client cursor is uploaded to the KMS cursor plane (including the out-of-band `present_pointer_cursor` fast path), excluded from `client_overlay_count` (preserving Direct Scanout and `wp_tearing_control_v1` async flips), excluded from primary-plane `client_damage`, and excluded from `draw_client_overlays`.
   - When `client_cursor_on_hw_plane` is false: the KMS cursor plane is cleared and `draw_client_overlays(..., !client_cursor_on_hw_plane, None)` composites the client cursor surface on both `render_output_targets` and the union fallback path.

### Invariants & Behavioral Boundaries

- `[INV-CURSOR-01] Exclusive Single-Path Client Cursor Presentation`: A mapped client cursor surface is presented by exactly one path per frame — either KMS hardware cursor plane offload (`client_cursor_on_hw_plane == true`) or `draw_client_overlays(..., include_cursor: true, ...)` (`client_cursor_on_hw_plane == false`). Neither zero-draw nor double-draw is permitted.
- `[INV-CURSOR-02] Pre-Validated Hardware Cursor Offload`: Client cursor offload checks `hardware_cursor_extent`, transform, viewport, scale ratio, and hotspot bounds *before* calling `set_hardware_cursor`, so an ineligible client cursor falls back to overlay composition without disabling the hardware cursor plane for theme cursors.

## Rejected Alternatives & Negative Knowledge

- **Only changing `include_cursor` from `false` to `true` in `output_target_render.rs`.**
  Rejected as incomplete: it would restore visibility, but every fullscreen game with a custom Wayland cursor would still lose Direct Scanout, lose `wp_tearing_control_v1` async flips, and force full Vulkan composites on every pointer motion event.
- **Uploading arbitrary client cursor surfaces without pre-validation.**
  Rejected: `DrmHost::set_hardware_cursor` treats unsupported dimensions or out-of-bounds hotspots as a hardware-cursor failure and disables the plane with exponential backoff, which would needlessly degrade theme cursors too.
