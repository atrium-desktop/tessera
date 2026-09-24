# tessera-primitives

Universal foundational physical, geometric, color, identity, and buffer primitives for Tessera.

## Responsibilities

- **Geometry (`geometry`)**: Primitive integer point, rectangle, size, transform, and surface geometry/mapping (`SurfaceGeometry`).
- **Color (`color`)**: Content color-space descriptions, primaries, transfer functions, and HDR/EDID specifications.
- **Buffer (`dmabuf`)**: Hardware buffer descriptions, DRM FourCC codes, DMA-BUF plane layout descriptors, and format modifiers.
- **Identity (`identity`)**: Strongly-typed opaque handles (`WindowId`, `WorkspaceId`, `OutputId`, `SeatId`, `InteractionDomainId`).
- **Input (`input`)**: Backend-agnostic raw keycodes, keyboard modifiers, and pointer buttons.
- **Stream (`stream`)**: Output capture stream frame descriptions and pixel geometries.
- Forbid unsafe code (`#![forbid(unsafe_code)]`).

## Boundaries

- Zero dependency (optional `serde` only).
- No I/O, no filesystem, no sockets, no process spawning.
- No GPU device, Ash, Vulkan, Flux, or Lens handles.
- No Wayland protocol C bindings.

## Related Documentation

- [ADR-0164: Universal domain pruning and primitives canonization](../../docs/adr/0164-universal-domain-pruning-and-primitives-canonization.md)
- [Architecture](../../docs/explanation/architecture.md)
