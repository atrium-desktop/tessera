# 0158. Clean-break workspace boundaries

- Status: Accepted
- Date: 2026-09-17
- Scope: workspace package architecture
- Deciders: Tessera maintainers

## Context

The workspace had two independent hierarchies: Cargo package names and
`api/core/design/panels/services/apps` directories. Those directories mixed
different concepts and allowed a package's location to grant dependencies that
its responsibility did not justify. Shared state, IPC, presentation, and the
composition root consequently accumulated unrelated ownership.

## Decision

Use a flat `crates/` workspace. Package identity and dependency policy are
defined by the package name and checked from Cargo manifests. Keep a package
only when it provides a real API, process, platform boundary, feature boundary,
or resource-ownership boundary.

The workspace separates stable values (`tessera-types`, `tessera-scene`),
desktop and authority domains (`tessera-desktop`, `tessera-authority`,
`tessera-semantic`), versioned wire protocol (`tessera-protocol`), IPC framing
and client/server communication (`tessera-ipc`), audit logging (`tessera-audit`),
platform access (`tessera-backend`, `tessera-wayland`), rendering and capture
(`tessera-render`), shell chrome (`tessera-shell`), and the unified desktop
composition root (`crates/tessera`). Display engine resources (host, GPU device,
presentation surface, canvas, renderer, and teardown order) are owned within
`tessera` with borrowed capability lifetimes, eliminating artificial crate
wrappers while maintaining strict drop safety.

### Invariants and behavioral boundaries

- Package directories are one level below `crates/` and match package names.
- Stable value and domain packages cannot depend on platform, GPU, socket, or
  UI implementations.
- The wire protocol contains DTOs and protocol vocabulary only. Unix framing,
  server connections, and clients live in `tessera-ipc`. Durable audit storage
  lives in `tessera-audit`.
- The composition root `tessera` owns the display engine lifecycle, Wayland host,
  GPU device, presentation surface, canvas, renderer, and strict server teardown
  order.
- Shell components communicate through the component contract and cannot depend
  on Wayland or renderer internals.
- New boundaries require independent consumers, resource ownership, or a
  compile-time dependency restriction; a file split alone is insufficient.

## Consequences

Package discovery is immediate, Cargo metadata is the source of truth, and
dependency checks remain valid when a package is renamed or moved. Protocol
compatibility is no longer coupled to internal `Window` ownership, and GPU
presentation code no longer masquerades as a pure API layer.

The migration changes internal package names and requires one coordinated
workspace build. External users of unpublished packages must update imports.

## Rejected Alternatives and negative knowledge

### Keep the tier directories

This preserves a visual taxonomy that Cargo does not understand and guarantees
that new packages will eventually be forced into the wrong category.

### Merge the whole compositor into one crate

This removes useful platform and feature boundaries, increases unsafe-code
blast radius, and makes independent protocol and rendering tests expensive.

### Keep one universal state crate

It creates a high-fan-in dependency sink where desktop policy, authority,
semantic state, and GPU descriptions evolve together for unrelated reasons.

### Keep `tessera-engine` as a re-export facade

A facade without ownership does not enforce lifecycle or hide implementation;
the engine must own the resources or it should not exist.

## Verification

`cargo check -p tessera`, `cargo run -p xtask -- check-boundaries`, and the
workspace formatter are required migration gates. Full workspace `nextest` is
the final delivery gate.
