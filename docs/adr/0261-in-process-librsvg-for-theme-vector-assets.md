---
id: ADR-0261
title: "Theme and application SVGs rasterize in process through system librsvg and Cairo"
status: accepted
date: 2026-11-26
scope: assets/svg, compositor/icons, compositor/cursor, shell/tray, dependencies
superseded_by: null
negative_knowledge: true
---

# 0261. Theme and application SVGs rasterize in process through system librsvg and Cairo

- Status: Accepted
- Date: 2026-11-26
- Deciders: Tessera Maintainers
- Amends: [ADR-0222](0222-one-in-process-svg-compiler-for-every-vector-asset.md)
  (replaces `optics-svg` on the theme-asset path with in-process `librsvg-2.0` +
  `cairo` while preserving the no-subprocess `[INV-ASSET-03]` and single-boundary
  invariants), [ADR-0233](0233-theme-assets-decode-once-into-premultiplied-bgra.md)
  (`tessera-assets` drops its `optics-svg` dependency edge and owns the
  in-process `librsvg` + `cairo` boundary, plus aspect-preserving raster fitting)

---

## Context and Problem Statement

[ADR-0222](0222-one-in-process-svg-compiler-for-every-vector-asset.md) removed
the `rsvg-convert` subprocess and routed all desktop SVG assets (application
icons, tray icons, HUD symbols, and XDG cursor themes) through `optics-svg`,
later consolidated behind `tessera-assets` in
[ADR-0233](0233-theme-assets-decode-once-into-premultiplied-bgra.md).

In real Linux desktop sessions, `optics-svg` proved structurally unfit for
untrusted third-party application icons and icon themes. By design (Optics
ADR-0150), `optics-svg` is a minimal static vector compiler for UI-controlled
glyphs (`optics-ui` widget icons) and deliberately skips `<style>` CSS
stylesheets, `<use>` / `<symbol>`, `<clipPath>`, `<mask>`, `<filter>`, `<text>`,
`<image>`, `<pattern>`, and isolated group compositing. Auditing real installed
themes (Breeze, Papirus, hicolor, Flatpak exports) showed over 33% of SVG icon
files rely on skipped features — including Tessera's own Settings icon
(`com.hihusky.Tessera.Settings.svg`, which builds its gear teeth with `<use>`).
Skipping `<clipPath>` / `<mask>` causes unclipped shapes to flood the icon cell,
skipping `<style>` turns themed shapes solid black, and skipping `<use>` drops
primary geometry.

Meanwhile, `tessera-image` already established Tessera's native Linux codec
strategy: thin, zero-crate-tree C FFI directly against system libraries
(`libpng16`, `libturbojpeg`, `libwebp`, `libwebpdemux`), keeping `../optics`
strictly out of image codec ownership.

## Decision Drivers

- **Faithful rendering of arbitrary desktop artwork.** Application icons,
  icon themes, and XDG cursor themes come from third-party authors (Inkscape,
  Illustrator, Figma, Breeze, Adwaita, Papirus) and expect full SVG 1.1 / SVG 2
  semantics.
- **Clear repository responsibilities.** `../optics` owns rendering and its own
  controlled UI glyph compiler (`optics-svg` behind `optics-ui`); it does not
  own image codecs or a full web-grade SVG engine. Desktop asset decoding
  belongs in `tessera-assets`.
- **No subprocesses on asset paths (`[INV-ASSET-03]`).** Decoding must stay
  in-process on worker threads, never forking `rsvg-convert`.
- **Dependency hygiene (ADR-0206).** Avoid pulling a large third-party Rust
  crate graph (`resvg`/`usvg`/`tiny-skia`/`fontdb`/`rustybuzz`) into the
  workspace when the Linux desktop already ships `librsvg-2.0` and `cairo`.
- **Zero-conversion pixel contract (`[INV-ASSET-08]`).** Cairo's
  `CAIRO_FORMAT_ARGB32` on little-endian Linux is already premultiplied BGRA8,
  matching `optics-gfx`'s `Bgra8Unorm` layout with zero channel swaps or alpha
  conversions.
- **Safe handling of untrusted input.** Enforce explicit byte ceilings, keep
  libxml2 safety limits active (`RSVG_HANDLE_FLAGS_NONE`), and load from memory
  with no base URI so external filesystem/network references are blocked.

## Considered Options

- **Option 1: In-process C FFI to system `librsvg-2.0` (>= 2.48) + `cairo` inside
  `tessera-assets`.** Chosen.
- **Option 2: Add pure-Rust `resvg` (`usvg` + `tiny-skia`) to `tessera-assets`.**
  Rejected: reintroduces a heavy Rust dependency tree rejected by ADR-0206 and
  diverges from the system-library codec pattern already used by `tessera-image`.
- **Option 3: Expand `optics-svg` to implement CSS, `<use>`, `<clipPath>`,
  `<mask>`, `<filter>`, `<text>`, and `<image>`.** Rejected: violates Optics
  ADR-0150 by forcing a minimal Layer-0 vector compiler to absorb a CSS engine,
  offscreen compositing layers, filter pipelines, text shaping, and raster image
  decoding.

## Decision Outcome

Chosen option: **In-process C FFI to system `librsvg-2.0` + `cairo` inside
`tessera-assets`**.

1. **`tessera-assets` owns the single theme-SVG boundary.** `VectorDocument` is
   backed by `RsvgHandle` (`rsvg_handle_new_from_data`, 96 DPI,
   `rsvg_handle_render_document` onto a `CAIRO_FORMAT_ARGB32` image surface).
   `tessera-assets` removes its `optics-svg` and `optics-svg-sys` dependencies
   and discovers `librsvg-2.0 >= 2.48` and `cairo` via `pkg-config` in
   `build.rs`.
2. **Untrusted input isolation.** SVG inputs are bounded to 16 MiB (matching
   `.svgz` gunzip bounds) and loaded from memory with no base URI and default
   XML parser limits, preventing external file/network fetches and entity bombs.
3. **Theme `currentColor` and cursor metadata preserved.** A User-origin CSS
   rule (`svg:not([color]) { color: #ffffff; }`) keeps unresolved `currentColor`
   visible without overriding author `color` attributes, inline `style=""`, or
   `<style>` rules. Root `data-hotspot-x` / `data-hotspot-y` metadata and
   missing-`xmlns` normalization are handled once in `tessera-assets`.
4. **Aspect-preserving raster icons.** Non-square raster icons decoded via
   `decode_bytes` are premultiplied and contain-fitted inside a transparent
   `extent × extent` square, matching SVG contain-fit behaviour instead of
   stretching.
5. **Optics keeps its own UI glyph role.** `optics-ui` continues linking
   `liboptics_svg` internally for its controlled built-in and runtime widget
   glyphs (`optics_ui::register_svg_icon`); `tessera-assets` no longer couples
   theme assets to Optics.

### Invariants & Behavioral Boundaries

- `[INV-ASSET-09] One in-process system SVG decoder for theme assets.`
  `tessera-assets` is the sole package linking `librsvg-2.0` + `cairo` for
  desktop theme SVG decoding; no consumer forks `rsvg-convert` (`[INV-ASSET-03]`)
  or links a second SVG renderer for theme assets.
- `[INV-ASSET-10] Memory-only SVG loading with no base URI.` Theme SVGs are
  loaded via `rsvg_handle_new_from_data` with default security flags and a hard
  input-size ceiling, never via filesystem URI loaders that could resolve
  arbitrary external references.

## Rejected Alternatives & Negative Knowledge

### Extending `optics-svg` for arbitrary application icons
- **Why considered**: Avoided adding `librsvg-2.0` / `cairo` to the host
  prerequisite list.
- **Why rejected**: Real desktop themes rely heavily on `<style>`, `<use>`,
  `<clipPath>`, `<mask>`, `<filter>`, and full CSS/SVG2 gradient rules.
  Re-implementing full SVG in `optics-svg` would turn a minimal Layer-0 compiler
  into a browser-grade SVG/CSS/filter engine and duplicate `librsvg`.

### Adopting `resvg` in `tessera-assets`
- **Why considered**: Pure Rust, self-contained, and high SVG fidelity.
- **Why rejected**: Adds a large Rust crate graph (and font/text stacks if
  `<text>` is enabled), contradicting ADR-0206 and `tessera-image`'s
  Linux-native system library approach.

## Consequences

### Positive
- Third-party application icons, theme packs (Breeze, Papirus, Adwaita,
  hicolor), and XDG SVG cursor themes render faithfully with full SVG 1.1/2
  support (`<use>`, `<style>`, `<clipPath>`, `<mask>`, `<filter>`, gradients,
  `rgba`, `<text>`, embedded `<image>`).
- Zero new Rust crate dependencies; `tessera-assets` becomes completely
  independent of Optics bindings.
- Cairo's `ARGB32` buffer yields premultiplied BGRA8 directly on little-endian
  Linux with zero post-processing.

### Negative / Trade-offs
- Host builds and runtime packages require `librsvg-2.0` (>= 2.48) and `cairo`
  shared libraries (standard on Linux desktop distributions).
- Root `data-hotspot-*` cursor metadata is scraped from the root `<svg>` start
  tag because `librsvg` does not expose arbitrary `data-*` XML attributes.
