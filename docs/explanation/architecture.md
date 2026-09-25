# Architecture

Tessera is a Linux Wayland compositor with a compositor-owned desktop shell.
Flux provides GPU rendering and Lens provides immediate-mode UI primitives;
Tessera owns protocol, policy, lifecycle, and platform integration.

The current architecture is recorded by [ADR-0158](../adr/0158-clean-break-workspace-boundaries.md), [ADR-0159](../adr/0159-package-boundaries-follow-capabilities.md), [ADR-0160](../adr/0160-consolidate-application-subsystem.md), [ADR-0161](../adr/0161-rename-backend-to-platform.md), [ADR-0163](../adr/0163-decoupled-backdrop-blur-cache-and-continuous-fade.md), and [ADR-0164](../adr/0164-universal-domain-pruning-and-primitives-canonization.md).

## Domain Topology & Dependency Flow

```text
                    ┌──────────────────────────────┐
                    │      tessera-shell           │  [Chrome & View Presentation]
                    │ (Dock/HUD/Control/Pivot UI)  │
                    └──────────────┬───────────────┘
                                   │ (Render snapshots & actions)
                    ┌──────────────▼───────────────┐
                    │      tessera-desktop         │  [Spatial Desktop State]
                    │(Window tree/Workspaces/Rules)│
                    └──────┬───────────────┬───────┘
                           │               │
            (Intent & exec)│               │(Render pass scene)
                           ▼               ▼
      ┌───────────────────────┐         ┌─────────────────────────┐
      │tessera-launch-services│         │     tessera-render      │  [GPU Optics & Composition]
      │(XDG apps/icons/bwrap) │         │(Vulkan / Kawase / Prism)│
      └───────────┬───────────┘         └────────────┬────────────┘
                  │                                  │
                  │ (Process leases)                 │ (KMS Framebuffer)
                  ▼                                  ▼
      ┌───────────────────────┐         ┌─────────────────────────┐
      │   tessera-authority   │         │    tessera-platform     │  [Hardware Backends]
      │(Leases / A11y / Grants)         │  (Linux DRM/KMS/evdev)  │
      └───────────┬───────────┘         └────────────┬────────────┘
                  │                                  │
                  │ (Protocol serialization)         │ (Physical displays)
                  ▼                                  ▼
      ┌───────────────────────┐         ┌─────────────────────────┐
      │tessera-wayland/proto  │         │   tessera-primitives    │  [Axiomatic Primitives]
      │ (IPC & Wayland wire)  │◄────────┤(Geometry/Color/FourCC)  │
      └───────────────────────┘         └─────────────────────────┘
```

## Resource ownership

`tessera` (`crates/tessera`) is the composition root and main compositor binary.
It owns the display engine lifecycle (host, device, presentation surface,
canvas, renderer, and Wayland server) with strict teardown ordering, session
orchestration, configuration application, shell registration, IPC handler
implementation, security decisions, and process supervision.

`tessera-shell` owns the Lens host and built-in UI chrome components (`dock`,
`hud`, `pivot`, `control_center`, `settings`, and shared `widgets`). Components
emit typed events. The compositor decides whether an event is authorized and
applies the corresponding desktop mutation.

## Domain boundaries

Foundational geometry, physical colors, buffer descriptors, identities, and input primitives live in `tessera-primitives` (ADR-0164).
Window, workspace, layout, desktop preferences, and interactive accessibility/automation targets live in `tessera-desktop`.
Application discovery, scale-aware icon-theme lookup, detached process execution,
and headless intent search live in `tessera-launch-services` (ADR-0164).
Actor authority, credentials, grants, observation leases, and interaction-domain policy live in `tessera-authority`.

The external compatibility boundary is `tessera-protocol`. It contains
versioned wire DTOs and audit vocabulary, and does not contain sockets,
threads, filesystem persistence, or GPU types. Unix framing, socket paths,
client connectivity, and the server dispatch loop live in `tessera-ipc`.
Durable integrity-checked event storage lives in `tessera-audit`.

## Platform and presentation

`tessera-wayland` owns Wayland object lifecycle, while
`tessera-wayland-protocols` contains generated protocol tables.
`tessera-platform` owns DRM/KMS, nested display, seat, and input adapters.
`tessera-render` owns buffer import, composition, damage, scanout, stream
production, and GPU readback capture encoding.

Presentation code receives delivery and security decisions through session
callbacks. It cannot open sockets, resolve credentials, or choose which
consumer is allowed to observe a frame.

## Dependency direction

The dependency graph moves from stable values and domain rules toward platform,
presentation, shell, and finally session/application composition. The protocol
does not depend on the compositor. Shell components do not depend on Wayland or
renderer internals. The session is the only place allowed to assemble all
subsystems.

`cargo run -p xtask -- check-boundaries` verifies these rules from Cargo
manifests. This makes the package name and manifest dependency graph the source
of truth rather than a second directory taxonomy.

## Actor boundary

Agent support is an authority projection over desktop capabilities. The broker
authenticates a principal, resolves its scope, validates observation leases,
and dispatches authorized actions through the same session use cases that serve
human input. Planning, model inference, and long-term agent memory remain
outside Tessera. The MCP adapter is an independent process.

## Adding a subsystem

Use a module when code has one owner and no independent dependency or process
boundary. Create a package when the code needs an independently evolving API,
an independent process or feature, distinct platform resources, or a compiler-
enforced dependency restriction. Keep the public surface narrow and make the
owning subsystem perform all effects.
