# tessera-scene

`tessera-scene` provides renderer-independent surface, color, and stream
descriptions for Tessera.

## Responsibilities

- Define `SurfaceGeometry`: buffer scale, transform, viewport source crop,
  destination scale, and damage coordinate mapping.
- Define color management primitives: color spaces, primaries, transfer functions,
  and SDR/HDR luminance characteristics.
- Define hardware buffer descriptions: DRM format FourCC codes, DMA-BUF plane
  layouts, and format modifier enumeration without linking `libdrm`.
- Define EDID display descriptors: physical chromaticity points and timings.
- Define output frame and stream geometries: `StreamGeometry`, crop rectangles,
  and presentation frame parameters.
- Forbid unsafe code (`#![forbid(unsafe_code)]`).

## Boundaries

`tessera-scene` defines descriptive models only:
- No Vulkan, Ash, Flux, or GPU device access.
- No Wayland protocol C bindings (no `wl_resource`).
- No compositor scene graph mutations, z-ordering, or window state (those belong in `tessera-desktop`).
- No GPU buffer import or shader compilation (those belong in `tessera-render`).

## Related Documentation

- [Architecture](../../docs/explanation/architecture.md)
- [ADR-0014: Buffer transform and viewport crop](../../docs/adr/0014-buffer-transform-and-viewport-crop.md)
- [ADR-0129: Color management](../../docs/adr/0129-color-management.md)
