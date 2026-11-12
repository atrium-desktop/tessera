# tessera-assets

Theme asset decoding for Tessera: SVG/SVGZ source reading and raster/vector
assets in the renderer's native pixel layout.

## Responsibilities

- **Vector sources (`vector`)**: Read SVG text and bounded SVGZ (gzip)
  decompression. One decompression rule for cursors, icons, and tray assets.
- **Vector documents (`VectorDocument`)**: The only type in the workspace that
  owns a reticle document. Rasterizes to premultiplied BGRA8 and reads the
  `data-hotspot-*` convention.
- **Rasters (`RasterImage`)**: PNG/JPEG/GIF/WebP decode, premultiplication before
  scaling, and the single premultiplied-BGRA8 upload layout.
- **Symbolic paint (`RasterImage::recolour_symbolic`)**: Colour a symbolic icon as
  `colour * coverage`, preserving premultiplied alpha.

Every result is premultiplied BGRA8: callers pass `as_bgra()` to
`flux::Image::from_bytes(..., Bgra8Unorm, ...)` without swapping channels or
converting alpha.

## Related Documentation

- [ADR-0233: Theme assets decode once, into premultiplied BGRA8](../../docs/adr/0233-theme-assets-decode-once-into-premultiplied-bgra.md)
- [ADR-0222: One in-process SVG compiler for every vector asset](../../docs/adr/0222-one-in-process-svg-compiler-for-every-vector-asset.md)
- [Project layout](../../docs/dev/project-layout.md)
