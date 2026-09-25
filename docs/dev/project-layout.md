# Project Layout

Tessera uses a flat Cargo workspace. Every package lives directly under
`crates/` and its directory name matches its package name. Package names carry
the architectural role; directory nesting does not grant dependency rights.

## Source tree

```text
crates/
  tessera                 composition root binary, display resource lifecycle, and session orchestration
  tessera-audit           bounded, integrity-checked durable event storage
  tessera-authority       actor identity, credentials, grants, ceilings, capabilities, and observation leases
  tessera-avatar          profile contract, portrait assets, and VRM runtime
  tessera-platform        DRM/KMS, nested display, seat, and input adapters
  tessera-env             shared process environment, tracing/log bridge, and runtime hygiene
  tessera-cli             domain-oriented management command library and headless tessera-ctl tool
  tessera-config          declarative configuration schema, loader, and live reload
  tessera-design          product design tokens, themes, and glass materials
  tessera-desktop         pure desktop state, windows, workspaces, layouts, and deterministic rules
  tessera-i18n            locale negotiation and translated chrome message catalog
  tessera-idle            out-of-process idle detection and inhibition sidecar
  tessera-ipc             Unix socket transport, framing, client, and server connection handling
  tessera-launch-services desktop application discovery, icon lookup, process launching, and intent search (ADR-0164)
  tessera-lock            standalone session lock client
  tessera-mcp             Model Context Protocol adapter daemon
  tessera-primitives      foundational physical geometry, color, buffer descriptor, identity, and input primitives (ADR-0164)
  tessera-protocol        versioned wire DTOs and audit vocabulary (zero socket/GPU dependencies)
  tessera-render          GPU composition, damage tracking, scanout, streams, and capture encoding
  tessera-shell           Lens UI host and built-in chrome components (dock, hud, pivot, control-center, settings, tray)
  tessera-wallpaper       desktop wallpaper and continuous parallax runtime
  tessera-wayland         Wayland protocol handlers and client state machines
  tessera-wayland-protocols generated Wayland C ABI interface tables
tooling/xtask             repository checks, boundaries verification, and automation
```

## Ownership rules

`tessera-primitives` contains foundational stable physical and geometric values only. Desktop policy
belongs to `tessera-desktop`; authority and identity policy belongs to
`tessera-authority`; external compatibility belongs to `tessera-protocol`.
None of those packages may depend on sockets, Wayland, GPU bindings, or UI.

The `tessera` binary is the composition root. It owns display engine resources
(host, device, presentation surface, canvas, renderer, and Wayland server) with
strict drop order, executes desktop use cases, applies configuration, and
supervises child adapters. Shell components in `tessera-shell` emit typed events;
they do not mutate the server or write configuration directly.

Wire protocol types live in `tessera-protocol` with zero network, filesystem,
or serialization dependencies. Unix framing, socket paths, and client/server
abstractions live in `tessera-ipc`. Durable event storage lives in
`tessera-audit`.

Application discovery, icon lookup, process launching, and intent search live
in `tessera-launch-services` (ADR-0164). GPU capture encoding is integrated into `tessera-render`.

Create a new package only for an independent public API, process, feature or
platform boundary, resource owner, or compile-time dependency restriction. A
large module with one owner remains a module.

## Dependency verification

The boundary check reads Cargo manifests and validates against explicit capability edges:

```bash
cargo run -p xtask -- check-boundaries
```

The check runs in CI. Full workspace validation uses `cargo nextest run`.
