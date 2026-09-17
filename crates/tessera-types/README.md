# tessera-types

`tessera-types` defines foundational, zero-dependency stable value types shared
across the entire Tessera workspace.

## Responsibilities

- Define primitive integer geometry: `Point`, `Rect`, `Size`, and `Transform`.
- Define strongly typed opaque identifiers: `WindowId`, `WorkspaceId`, `OutputId`,
  `SeatId`, `InteractionDomainId`.
- Define raw input vocabulary: `KeyChar`, `KeyModifiers`, key symbols, and pointer buttons.
- Forbid unsafe code (`#![forbid(unsafe_code)]`).
- Optionally derive `serde` serialization through the `serde` feature.

## Boundaries

`tessera-types` is strictly effect-free and dependency-free:
- No filesystem, network, socket, or process I/O.
- No Wayland protocol or graphics/GPU bindings (no Vulkan, Flux, Lens).
- No domain state machines, window management policy, or layout logic (those belong in `tessera-desktop`).

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [Project Layout](../../docs/dev/project-layout.md)
- [ADR-0158: Clean-break workspace boundaries](../../docs/adr/0158-clean-break-workspace-boundaries.md)
