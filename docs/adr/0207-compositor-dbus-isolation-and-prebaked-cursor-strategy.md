---
id: ADR-0207
title: "Compositor D-Bus Isolation and Pre-Baked Hardware Cursor Strategy"
status: accepted
date: 2026-11-03
scope: runtime, platform, cursor, dbus, architecture
superseded_by: null
negative_knowledge: true
---

# 0207. Compositor D-Bus Isolation and Pre-Baked Hardware Cursor Strategy

- Status: Accepted
- Date: 2026-11-03
- Deciders: Tessera Maintainers
- Amends: [ADR-0051](0051-portal-backend-dbus-bridge.md), [ADR-0070](0070-svg-cursors-with-bundled-bibata-fallback.md), [ADR-0122](0122-original-mit-cursor-theme-replaces-bibata.md)
- Related: [ADR-0206](0206-pure-compositor-dependency-hygiene-and-build-parallelism.md)

---

## Context and Problem Statement

Two architectural creep factors progressively burdened the Tessera compositor binary:

1. **In-Process D-Bus Proliferation**:
   ADR-0051 established that D-Bus dispatch must never link into the display server process because bus-level stalls, daemon crashes, or transport deadlocks threaten the entire Wayland compositor. Despite this, direct `zbus` bridges for BlueZ (`bluetooth/bluez.rs`) and IWD (`wireless/iwd.rs`) were added to the core compositor crate, forcing the primary binary to link the entire 86-crate `zbus` async runtime stack.
2. **KMS Hardware Cursor Rasterization vs. Engine Identity**:
   Linux DRM/KMS hardware cursor planes require linear raw BGRA8 memory buffers on the host CPU. A proposal arose to implement a general-purpose CPU software 2D vector rasterizer in `flux` (Optics) to satisfy KMS requirements without third-party crates like `resvg`.

## Decision Drivers

- **Process Isolation and Crash Boundary**: The Wayland compositor is critical system infrastructure. Network and peripheral service failures must not bring down the display server.
- **Engine Purity**: `flux` is an explicit Vulkan-first GPU engine. Polluting it with legacy 2D CPU software rasterization algorithms (scanline, edge tables, anti-aliasing math) violates its architectural identity.
- **Zero-Cost Hardware Cursor Scanout**: Cursors are fixed-size, low-count assets. Pre-baking or GPU-rendered readback satisfies KMS dumb buffers without carrying complex rasterizer libraries at runtime.

## Decision Outcome

### 1. Compositor D-Bus Decoupling

In `crates/tessera`, `zbus` is converted to an optional dependency controlled by the `dbus-system` feature. The compositor core speaks exclusively through abstract `BluetoothBackend` and `WirelessBackend` interfaces:
- In pure or decoupled builds, `spawn_auto()` instantiates inert `Unavailable` implementations without linking or initializing `zbus`.
- D-Bus communication is isolated to companion daemons (e.g. `tessera-castd`, future `tessera-systemd`) communicating over Unix Domain Sockets (`tessera-ipc`), restoring ADR-0051 compliance.

### 2. Pre-Baked Cursor Strategy (Rejecting Flux CPU Rasterization)

We explicitly reject implementing a CPU 2D software rasterizer in `flux`:
- **Pre-baking is canonized**: Standard cursor shapes are pre-rendered into static bitmaps / PNG assets. Cursors are loaded directly into hardware planes with zero runtime rasterization overhead.
- **Dynamic fallback via GPU**: If runtime SVG rendering of arbitrary vectors is ever required, the system uses Optics' existing `lens` vector path pipeline on a transient Vulkan offscreen target followed by a `vkCmdCopyImageToBuffer` readback into host memory, rather than maintaining a redundant CPU rasterizer codebase.

## Negative Knowledge

- **Never implement CPU software rasterization in a GPU engine to satisfy hardware cursor dumb buffers**: Hardware cursors are small (64×64 or 128×128) and static. Implementing a full CPU Bezier rasterizer in `flux` would add thousands of lines of maintenance liability for a problem solved with zero runtime cost by pre-baked assets.
- **Never embed system D-Bus client event loops in the display server process**: Always isolate system daemon protocols (BlueZ, IWD, NetworkManager, UDisks) to companion processes over IPC.
