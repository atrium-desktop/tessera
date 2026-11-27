# tessera-material

`tessera-material` draws the frosted and liquid-glass chrome materials
(ADR-0253) on the Optics gfx public effect runtime. Optics carried these as
`optics-material` until Optics ADR-0174; Tessera owns them since ADR-0262.

## Responsibilities

- `FrostedFilter`: a non-distorting blurred, tinted body over a backdrop capture.
- `LiquidGlassFilter`: an analytic convex-lens body with rim refraction,
  highlights and per-group backdrop statistics.
- Each filter writes a slab-sized output image that the caller places under
  chrome; the slab is transparent outside its bodies.

## Layout

- `csrc/`: the C filters, built with `cc` against the Optics gfx public headers.
- `shaders/`: Slang compute sources. `spv/` holds the committed SPIR-V that the
  C embeds with `#embed`. Regenerate it with `scripts/compile-shaders.sh`.
- `src/lib.rs`: the safe Rust API and bindgen FFI (`sys`).

## Boundaries

- Only Optics public headers; GPU objects are refcounted gfx objects
  (`[INV-GFX-EXT-01]`, `[INV-GFX-EXT-02]`).
- Policy (which material, sigma, tint) belongs to the compositor runtime
  (`crates/tessera/src/runtime/backdrop.rs`).
