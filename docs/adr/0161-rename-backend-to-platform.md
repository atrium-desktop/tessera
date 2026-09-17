# ADR-0161: Rename tessera-backend to tessera-platform

- Status: Accepted
- Date: 2026-09-17
- Scope: Host display hardware, input device, and seat session management
- Amends: [ADR-0158](0158-clean-break-workspace-boundaries.md), [ADR-0159](0159-package-boundaries-follow-capabilities.md)

## Context

The crate previously named `tessera-backend` abstracts the compositor's
presentation target and raw input sources:
- Linux kernel DRM/KMS modesetting, CRTC management, hardware plane allocation, and page flips;
- Kernel `evdev` and `libinput` event polling for keyboards, mice, touchpads, and gestures;
- `libseat` / logind session management, device permission acquisition, and VT console switching;
- Nested Wayland host window presentation via Vulkan `VkSurfaceKHR` for development environments.

The name "backend" was inherited from legacy display server terminology
(e.g., Weston and wlroots "backends"). In modern software engineering,
"backend" almost universally connotes server-side, database, or network
services, conflicting fundamentally with the crate's actual role as the
lowest-level physical hardware and OS driver interface.

## Decision

Rename `tessera-backend` to **`tessera-platform`**.

This establishes an unambiguous architectural triad for the display server
foundation:
1. **`tessera-platform`**: Physical hardware and OS interface (DRM/KMS displays,
   input devices, seat session, and nested development windows).
2. **`tessera-render`**: GPU composition, damage tracking, and texture caching.
3. **`tessera-wayland`**: Wayland client protocol handlers and window state machines.

The crate's internal `Host` abstraction (`struct Host`, `Host::open(...)`)
remains the primary interface exposed to the composition root.

### Invariants & Boundaries

- `tessera-platform` owns the OS device nodes, kernel mode-setting, and raw
  input events. It performs no Wayland protocol dispatch to clients and no GPU
  canvas composition.
- The composition root (`tessera`) opens and drives the platform host.

## Consequences

- Eliminates semantic ambiguity between "backend" (historically hardware output)
  and network/IPC services.
- Adopts the standard terminology used in modern engine architectures
  (OS platform adaptation layer).
- Package name, manifest, and imports are updated across the workspace.
