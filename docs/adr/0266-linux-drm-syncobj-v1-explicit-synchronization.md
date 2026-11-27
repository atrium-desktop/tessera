---
id: ADR-0266
title: "Native wp_linux_drm_syncobj_v1 timeline explicit synchronization"
status: accepted
date: 2026-11-27
scope: wayland-protocols, wayland/extensions, wayland/protocol, wayland/server
superseded_by: null
negative_knowledge: true
---

# 0266. Native wp_linux_drm_syncobj_v1 timeline explicit synchronization

- Status: Accepted
- Date: 2026-11-27
- Deciders: Tessera Maintainers
- Amends: [ADR-0250](0250-fence-accurate-client-buffer-release-and-cached-mirror-composition.md)
  (adds DRM timeline syncobj acquire/release as a first-class explicit-sync
  transport alongside `zwp_linux_explicit_synchronization_v1`),
  [ADR-0265](0265-per-output-scoped-buffer-retirement-and-completion-fences.md)
  (extends per-output retired buffer dispatch to signal
  `wp_linux_drm_syncobj_v1` timeline release points)

---

## Context and Problem Statement

Tessera previously advertised only `zwp_linux_explicit_synchronization_v1`, an
unstable protocol deprecated upstream in favor of `wp_linux_drm_syncobj_v1`.
Modern Linux Wayland clients — including Chromium/Chrome, Firefox, Mesa EGL,
Vulkan WSI, and NVIDIA drivers — negotiate `wp_linux_drm_syncobj_v1` and fall
back to implicit DMA-BUF synchronization when it is absent. Without it,
GPU-backed browsers cannot use timeline explicit synchronization on Tessera.

## Decision Drivers

- **Modern client compatibility.** Support the ratified staging protocol
  (`staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml`) used by current Mesa,
  Vulkan WSI, Chromium, and Firefox.
- **Zero new crate dependency edges.** Keep `tessera-wayland` free of `libc`
  and `tessera-platform` dependencies ([ADR-0159](0159-package-boundaries-follow-capabilities.md)).
- **Protocol and lifecycle soundness.** Enforce all protocol errors, preserve
  latched timeline points after client timeline destruction, materialize
  acquire points cleanly, and guarantee release points are always signaled so
  clients never deadlock.

## Considered Options

1. **Implement `wp_linux_drm_syncobj_v1` natively in `tessera-wayland`**
   backed by the matching unprivileged DRM render node
   (`DRM_CAP_SYNCOBJ_TIMELINE == 1`), bridging acquire points to `sync_file`
   via `DRM_IOCTL_SYNCOBJ_TRANSFER` + `EXPORT_SYNC_FILE` and release points via
   `IMPORT_SYNC_FILE` + `DRM_IOCTL_SYNCOBJ_TRANSFER` (with immediate
   `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` when already signaled).
2. **Rely exclusively on implicit synchronization + `DMA_BUF_IOCTL_IMPORT_SYNC_FILE`.**

## Decision Outcome

Chosen option: **Option 1**, because modern Mesa/Vulkan WSI/Chromium clients
actively target `wp_linux_drm_syncobj_v1`, and timeline transfer integrates
directly into Tessera's existing `sync_file` renderer and KMS `IN_FENCE_FD` /
`OUT_FENCE_PTR` pipeline with zero extra CPU waits in the steady state.

### Concrete Design

1. **Protocol compilation & vtable admission (`crates/tessera-wayland-protocols`,
   `crates/tessera-wayland/src/ffi.rs`).**
   - Compile `staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml` into
     `tessera-wayland-protocols`.
   - Declare `wp_linux_drm_syncobj_manager_v1_interface_impl`,
     `wp_linux_drm_syncobj_timeline_v1_interface_impl`, and
     `wp_linux_drm_syncobj_surface_v1_interface_impl`, all admitted through
     `declare_vtables!` (`[INV-ARCH-50]`).
2. **Render-node discovery & UAPI (`crates/tessera-wayland/src/extensions/drm_syncobj.rs`).**
   - `SyncobjDevice::open_for_device(dmabuf_main_device)` opens the matching
     `/dev/dri/renderD*` node (by exact `rdev` or sysfs device match for
     primary DRM nodes) and verifies `DRM_CAP_SYNCOBJ_TIMELINE == 1`.
   - `wp_linux_drm_syncobj_manager_v1` (version 1) is advertised automatically
     whenever DMA-BUF explicit sync is supported and `SyncobjDevice` opened
     successfully (works in both direct DRM and nested Vulkan sessions).
   - Uses the Linux >= 5.2 16-byte `drm_syncobj_handle` + `DRM_IOCTL_SYNCOBJ_TRANSFER`
     UAPI with compile-time `size_of` assertions on every ioctl struct.
3. **Strict protocol validation & lifecycle.**
   - Mutually exclusive per surface with legacy
     `zwp_linux_surface_synchronization_v1` (`surface_exists` error).
   - Enforces all 6 `wp_linux_drm_syncobj_surface_v1` errors on commit:
     `no_surface`, `unsupported_buffer`, `no_buffer`, `no_acquire_point`,
     `no_release_point`, and `conflicting_points` (same timeline with
     `acquire_point >= release_point`).
   - `SyncobjTimeline` is `Rc`-owned; destroying the client timeline resource
     never invalidates committed acquire or release points.
   - Acquire points materialize non-blockingly at commit via
     `DRM_IOCTL_SYNCOBJ_TRANSFER` + `EXPORT_SYNC_FILE`; if a future timeline
     point has not yet been submitted at commit time, materialization is
     deferred lazily to sample time (with a bounded 2 ms
     `DRM_SYNCOBJ_WAIT_FLAGS_WAIT_AVAILABLE` fallback) so Wayland dispatch
     never stalls the 120 Hz frame loop.
   - `SyncobjRelease` transfers live completion `sync_file` fds directly onto
     the client's release timeline point (zero CPU stall), falls back to
     waiting + `DRM_IOCTL_SYNCOBJ_TIMELINE_SIGNAL` if transfer fails, and
     signals on `Drop` so teardown never deadlocks clients.

### Invariants & Behavioral Boundaries

- **`[INV-ARCH-159]` (Amended):** Buffers committed with
  `wp_linux_drm_syncobj_v1` release points must transfer live completion
  fences onto the release timeline (or wait before signaling immediately) and
  must signal immediately when superseded without presentation or on teardown.
  **Enforcement:** `test_inv_arch_159_syncobj_timeline_acquire_export_and_release_signal_roundtrip`.

## Rejected Alternatives & Negative Knowledge

### Option 2: Implicit synchronization only
- **Why considered**: Avoids adding DRM syncobj ioctls and a new Wayland global.
- **Why rejected**: Leaves modern Chromium/Mesa/Vulkan/NVIDIA clients on legacy
  implicit sync, which NVIDIA drivers and modern Vulkan WSI no longer
  guarantee to order cleanly without explicit timeline synchronization.

### Comparing imported timelines by `fstat(fd).st_ino`
- **Why considered**: Attempted to detect two separate `import_timeline` calls
  referring to the same kernel syncobj.
- **Why rejected**: Linux `anon_inode:syncobj_file` descriptors share a single
  singleton anonymous inode across every syncobj, so `st_ino` is identical for
  unrelated timelines and would spuriously raise `conflicting_points`. Timeline
  identity is compared by `Rc::ptr_eq` on the imported timeline object.

## Consequences

### Positive
- Modern Wayland GPU clients (including Flatpak Chrome 158, Firefox, Mesa, and
  Vulkan apps) natively negotiate `wp_linux_drm_syncobj_v1`.
- GPU-to-GPU release timeline signaling requires zero CPU waits on the steady
  path and skips redundant `DMA_BUF_IOCTL_IMPORT_SYNC_FILE` ioctls on
  explicitly synchronized buffers.

### Negative / Trade-offs
- Clients that commit a future acquire timeline point whose GPU submission is
  still unavailable at render sample time beyond the 2 ms bound fall back to
  sampling without an acquire fence; full asynchronous compositor-side
  transaction queuing via `DRM_IOCTL_SYNCOBJ_EVENTFD` remains a future
  enhancement if needed for heavily pipelined clients.
