---
id: ADR-0219
title: "Asynchronous Direct Scanout and wp_tearing_control_v1 Presentation"
status: accepted
date: 2026-11-06
scope: wayland/server, wayland-protocols, composite/scanout, platform/drm
superseded_by: null
negative_knowledge: true
---

# 0219. Asynchronous Direct Scanout and wp_tearing_control_v1 Presentation

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Maintainers
- Amends: [ADR-0077](0077-presentation-domain-redraw-state-machine.md) (presentation domain redraw lifecycle), [ADR-0182](0182-fullscreen-frame-pacing-and-direct-scanout-fidelity.md) (fullscreen frame pacing and direct-scanout fidelity)
- Implements: `wp_tearing_control_v1` (Wayland Staging Protocol)

---

## Context and Problem Statement

Modern competitive gaming, high-framerate rendering engines, and low-latency interactive simulation clients (e.g. CS2, esports 3D titles, input-critical emulators) require immediate display updates. When presentation is strictly synchronized to the display controller's vertical blanking interval (VSync), submitted frames must wait for the next VBlank deadline. On a 60 Hz display, this introduces up to 16.6 ms of presentation latency; even on high-refresh 144 Hz or 240 Hz displays, the queue delay and frame pacing quantization add measurable input lag.

Furthermore, when an uncapped client runs at framerates that fluctuate around or below the display refresh rate (e.g. dipping to 55 FPS on a 60 Hz panel), VSync creates severe perceived stutter due to duplicated refresh cycles. In contrast, asynchronous page flips (allowing tearing) trade horizontal visual tear lines for instantaneous response and continuous visual motion.

Wayland standardized this presentation negotiation via `wp_tearing_control_v1`. However, Tessera previously lacked advertisement of the tearing control manager and atomic DRM asynchronous flip plumbing. Consequently, clients requesting asynchronous flips either failed to negotiate the capability or were unconditionally display-locked to VBlank.

## Decision Drivers

- **Zero Latency Penalty for Competitive Fullscreen Clients**: Fullscreen games explicitly requesting asynchronous presentation must achieve minimal input latency through hardware asynchronous page flips.
- **Strict Compositor Immune Boundary**: The desktop shell, window manager, multi-window composites, and layered glass materials must NEVER tear. Tearing must be strictly confined to exclusive direct scanout.
- **Double-Buffered State Protocol Conformance**: Presentation hints in `wp_tearing_control_v1` must be double-buffered and latch strictly upon `wl_surface.commit`.
- **Graceful Hardware/Driver Fallback**: If the underlying DRM/KMS driver or plane configuration cannot substantiate `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP` (or rejects an async commit), presentation must fall back to synchronized atomic flip (VSync) without dropping frames or failing the surface commit.

## Considered Options

1. **Advertise `wp_tearing_control_v1` and plumb `DRM_MODE_PAGE_FLIP_ASYNC` exclusively through Direct Scanout.**
2. **Global tearing toggle for composited frames.**
3. **Ignore `wp_tearing_control_v1` and retain strict tear-free presentation everywhere.**

## Decision Outcome

Chosen option: **Option 1**, because it provides uncompromised, esports-grade low latency for eligible fullscreen applications while upholding absolute visual integrity for the rest of the desktop environment.

### 1. Protocol Ingest and Double Buffering

- `wp_tearing_control_manager_v1` is advertised at version 1 as a stable/staging global in `server/lifecycle.rs`.
- `get_tearing_control` attaches a `wp_tearing_control_v1` resource to the target `SurfaceRec`. Attempting to attach multiple tearing control objects to the same surface raises `tearing_control_exists` (error code 0).
- `set_presentation_hint` records the requested hint (`Vsync = 0`, `Async = 1`) into `SurfaceRec.pending_presentation_hint`.
- The presentation hint latches to `SurfaceRec.presentation_hint` on `surface_commit`.
- When a `wp_tearing_control_v1` resource is destroyed, `pending_presentation_hint` reverts to `Vsync`. When the associated `SurfaceRec` is destroyed, the tearing control object becomes inert.

### 2. Physical Plane Admission

`SurfaceDmabuf` carries `pub presentation_hint: PresentationHint`. During primary plane planning:
- Only when `evaluate_scene` and `evaluate_surface` admit the client as an exclusive direct scanout candidate does the candidate's `presentation_hint` reach the display host.
- Any composited frame (e.g. windowed client, visible shell UI, active overview, software cursor, partial opacity) executes via the Vulkan compositor pass with `allow_async = false` and VSync synchronization.

### 3. DRM Asynchronous Atomic Page Flip

`tessera-platform` queries driver capability `DriverCapability::AtomicASyncPageFlip` upon card initialization.
During `present_scanout`:
- If `client.presentation_hint.is_async()` and the DRM driver supports atomic async flips, `commit_scanout` appends `AtomicCommitFlags::PAGE_FLIP_ASYNC` to `AtomicCommitFlags::NONBLOCK | AtomicCommitFlags::PAGE_FLIP_EVENT`.
- If the kernel rejects the asynchronous atomic commit (e.g. unsupported modifier, plane constraint, or driver limitation), the commit transparently retries with standard synchronous atomic flags, fulfilling graceful degradation.

### Invariants & Behavioral Boundaries

- `[INV-ARCH-109] Asynchronous Tearing Requires Exclusive Direct Scanout`: Asynchronous page flipping (`DRM_MODE_PAGE_FLIP_ASYNC`) is permitted if and only if the presentation target is an unoccluded, fully opaque, 1:1 physical fullscreen surface directly scanned out on KMS Primary Plane. The compositor composite pass must NEVER tear.
- `[INV-ARCH-110] Tearing Control Double Buffering and Graceful Driver Fallback`: `wp_tearing_control_v1` presentation hint updates are double-buffered and latch on `wl_surface.commit`. If the driver or plane does not substantiate `DRM_CAP_ATOMIC_ASYNC_PAGE_FLIP`, or if atomic commit rejects async flip, the commit must gracefully fall back to synchronized atomic flip (VSync) without dropping the frame or raising a protocol error.

## Rejected Alternatives & Negative Knowledge

### Global Tearing Toggle for Composited Frames
- **Why considered**: Some users wanted tearing in borderless windowed mode.
- **Why rejected**: A composited frame combines multiple client surfaces, wallpaper, and system chrome into one framebuffer. Asynchronous flipping on a composited framebuffer produces hideous tearing artifacts across window decorations, text fields, and shell docks, corrupting the system visual contract. Tearing is only physically valid when a single application owns 100% of the scanout plane.

### Silent Drop of wp_tearing_control_v1
- **Why considered**: Simplifies compositor logic by asserting that Wayland is strictly tear-free.
- **Why rejected**: Completely breaks low-latency competitive gaming and causes stutter in uncapped clients that drop below refresh rate, forcing users back to X11 or competing compositors.

## Consequences

### Positive
- Full parity with reference compositors (Gamescope, Sway, KWin, Hyprland) for low-latency gaming.
- Uncapped games achieve minimum input lag and smooth presentation without swapchain deadlocks (paired with `[INV-ARCH-70]`).
- Clean double-buffered protocol lifecycle with zero chance of dangling resource pointers or protocol violations.

### Negative / Trade-offs
- Displays without VRR (Variable Refresh Rate) may display visible horizontal tear seams during rapid camera movement in async mode (which is the intended behavior requested by the client).
