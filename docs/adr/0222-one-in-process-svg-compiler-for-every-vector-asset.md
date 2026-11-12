---
id: ADR-0222
title: "One in-process SVG compiler replaces pre-baked cursor sprites and the rsvg-convert subprocess"
status: accepted
date: 2026-11-06
scope: compositor/cursor, shell/icons, dependencies
superseded_by: null
negative_knowledge: true
---

# 0222. One in-process SVG compiler for every vector asset

- Status: Accepted
- Date: 2026-11-06
- Deciders: Tessera Maintainers
- Amends: [ADR-0207](0207-compositor-dbus-isolation-and-prebaked-cursor-strategy.md)
  (its pre-baked-cursor half), [ADR-0122](0122-original-mit-cursor-theme-replaces-bibata.md)
  (the art contract is unchanged; only the rasterizer is)
- Related: Optics ADR-0150 (`docs/adr/0150-svg-vector-document-compiler.md` in that repository)
- Amended by: [ADR-0233](0233-theme-assets-decode-once-into-premultiplied-bgra.md)
  — the two icon decoders are now one crate, and the raster layout is
  premultiplied BGRA8 at the boundary

---

## Context and Problem Statement

Vector art reaches Tessera from three places, and until now each one answered the
"what rasterizes this?" question differently:

1. **Cursor themes.** ADR-0207 replaced runtime SVG rasterization with pre-baked
   64×64 PNG sprites: `scripts/prepare-tessera-cursors.py` runs `rsvg-convert`
   at authoring time and the compositor loads the bitmap. That is fast and it
   removed a Rust dependency, but it also broke the format the freedesktop cursor
   specification actually names, `cursors/<name>.svg`: `CursorTheme::load_shape`
   only ever looked for `<name>.png`, so a user who installed a *correct* SVG
   cursor theme got the bundled fallback for every shape, with a log line that
   claimed the theme was in use. The pre-bake also pinned one resolution
   (`rsvg-convert -w 64 -h 64`), so a 2× output has to upsample a 64px sprite.

2. **Application icons** (`crates/tessera/src/runtime/apps.rs`). SVG icons are
   rasterized by forking `rsvg-convert` per icon. On a host without librsvg,
   every SVG application icon silently disappears from the launcher. The fork is
   also an unbounded syscall on a path that runs per discovered application.

3. **Tray icons** (`crates/tessera-shell/src/tray/icon.rs`). Same subprocess, in
   a file whose own doc comment says it was "copied from the compositor's app
   icon cache" — two copies of one decoder, drifting independently.

Alongside those three, the SVG *parser* had been duplicated as well:
`crates/tessera/src/cursor.rs` and `crates/tessera-shell/src/chrome/agent_feedback.rs`
each carried their own `shape_candidates` table (byte-identical, 40 lines), their
own `SvgMeta`, and their own hand-written `<svg>` attribute scanners with three
behavioural divergences (hotspot default `(0,0)` vs `(64,28)`, a `width`/`height`
fallback in one and not the other, comma-separated `viewBox` parsing in one and
not the other).

So the same 40-line table existed twice, the same root-tag scrape existed twice
with different answers, and the same format had three different rasterizers — one
of which was a subprocess and one of which was a build step that produced a
resolution ceiling.

## Decision Drivers

- **One implementation per format.** The stack must not answer "what rasterizes
  SVG" differently in three places. Duplicated decoders drift silently; a shape
  added to one table and not the other renders the wrong arrow with no error.
- **The XDG contract is the SVG file.** A user authoring a cursor theme writes
  `cursors/<name>.svg`; that file must be what works, without a build step and
  without `rsvg-convert`.
- **No subprocess on a render path.** Forking a third-party rasterizer per icon is
  an unbounded syscall, a host dependency the compositor cannot verify, and an
  untestable seam.
- **Resolution independence.** A cursor sprite must be produced at the output's
  own scale, not upsampled from a fixed 64px bake.
- **Dependency hygiene** (ADR-0206, ADR-0209). The answer must not be a new
  third-party Rust crate tree in the compositor.

## Considered Options

- **Keep the pre-baked cursors and document "SVG-only themes are unsupported".**
  Rejected: it is a silent, user-visible wrong answer. The theme resolves, the
  log says it is in use, and every shape renders from the fallback.

- **Re-add `resvg`/`usvg`/`tiny-skia` to the compositor.** Rejected by ADR-0206
  and ADR-0209, and it would leave the *shell* still forking `rsvg-convert`, so
  the format would still have two implementations.

- **Keep `rsvg-convert` but cache its output.** Rejected: caching makes the
  subprocess rarer, not absent. A first frame that misses the cache still blocks
  on a fork, and the host dependency remains.

- **Add an SVGTiny-style parser in the compositor.** Rejected: that is a third
  copy of exactly the code being consolidated, and the compositor is the wrong
  layer to own a document format.

- **Link the Optics SVG compiler (ADR-0150 in that tree).** Chosen.

## Decision Outcome

Tessera links **`svg`**, the Optics SVG vector-document compiler, and deletes
every other answer:

### 1. Cursors are compiled, not baked

`crates/tessera/src/cursor.rs` resolves each `wp_cursor_shape` through the
freedesktop name list and hands the theme's `cursors/<name>.svg` to
`svg::Document::parse_bytes`, then rasterizes it at exactly
`size * scale` pixels through `svg::Document::rasterize` in
`SVG_PIXELS_BGRA8_PREMUL` — the layout a DRM/KMS cursor plane reads.

A theme that also ships `<name>.png` keeps it: the bitmap wins, because the
artist declared those pixels. The hotspot always comes from the SVG's
`data-hotspot-*` attributes when there is one, so a theme that ships both has a
single source of truth for its hotspot.

`MAX_CURSOR_SPRITE` (256) is the load-time ceiling. A hardware cursor plane is a
fixed-size rectangle; a larger sprite cannot be scanned out and would silently
fall back to the software path every frame. Clamping at load keeps that failure
out of the frame loop.

### 2. Application and tray icons are compiled, not forked

`crates/tessera/src/runtime/apps.rs` and `crates/tessera-shell/src/tray/icon.rs`
both call the same library, at the same pinned size
(`DEFAULT_ICON_SIZE * scale`, capped at 512). There is no `Command::new`, no
`rsvg-convert`, and no librsvg on the host prerequisite list. The two decoders
keep their own *policy* (the shell recolours symbolic icons, the compositor
recolours HUD symbols) but share the one *mechanism*.

### 3. The shape vocabulary is a shared primitive

The `wp_cursor_shape_device_v1` → XDG file-name table moves to
`tessera_primitives::cursor_shape::xdg_names`, and both `crates/tessera` and
`crates/tessera-shell` read it from there. `tessera-primitives` is a pure value
layer with no `flux`/`lens` coupling, so a `&'static [&'static str]` table is
exactly what belongs in it.

### 4. The duplicated SVG scraping is deleted

`agent_feedback.rs` no longer has its own `shape_candidates`, `SvgMeta`,
`svg_meta`, `svg_open_tag`, `attr`, `num` or `viewbox`. It asks the compiled
document for its viewport and its `data-hotspot-*` attributes, which is the same
answer the cursor path gets, from the same parser.

## Invariants & Behavioral Boundaries

- `[INV-ASSET-01]` **One SVG implementation.** No Tessera crate parses SVG
  markup or shells out to a rasterizer. A consumer compiles a document through
  `svg` or it does not render the asset.
- `[INV-ASSET-02]` **The SVG file is the contract.** A cursor theme that ships
  only `cursors/<name>.svg` must render, at the requested scale, with its own
  hotspot. Pre-baked PNGs are an override, never a requirement.
- `[INV-ASSET-03]` **No subprocess on an asset path.** `rsvg-convert` and any
  other external rasterizer are absent from the tree; asset decoding is in
  process and therefore testable.
- `[INV-ASSET-04]` **Sprite extents are bounded at load time.** A cursor sprite
  is clamped to the scanout ceiling when it is compiled, not when it is drawn.
- `[INV-ASSET-05]` **One shape table.** The protocol-shape → XDG-name mapping
  lives in `tessera-primitives`; a second copy is a defect.
- `[INV-ASSET-06]` **An advertised extension is a working extension.** `.svgz` is
  in the extension list every asset path consults, so the shared reader must
  gunzip it; listing a format while feeding the compiler its compressed bytes
  makes the list a lie and silently drops the asset.

## Rejected Alternatives & Negative Knowledge

- **Never let a resolved theme silently fall back per shape.** The old code
  logged "using SVG theme" and then painted the bundled theme for every shape.
  A theme that contributes nothing usable is now reported by counting its usable
  sources, so the log says what happened.

- **Never make a pre-baked bitmap the contract for a format whose specification
  names the vector file.** A build step that must have run is not a format
  contract; it is a deployment checklist that users cannot satisfy.

- **Never fork a rasterizer for an asset.** It is a host dependency the process
  cannot verify, an unbounded syscall, and it cannot produce a coverage mask or a
  BGRA sprite without a second conversion.

- **Never keep two decoders for one format because they "only" differ in
  policy.** The duplicated `attr`/`viewbox` pair in this tree had already
  diverged on three behaviours; the policy difference belonged in the caller, not
  in a second parser.

## What the first version got wrong

Three defects survived the initial pass and are recorded because each one was a
gap in *verification*, not in intent:

1. **`opacity` never reached the pixels.** The Optics compiler parsed element and
   group opacity into the shape and then painted every shape at the caller's
   global alpha. Cursors and icons are opaque almost everywhere, so nothing looked
   wrong until an asset with a deliberate half-transparent detail was probed.
2. **Stroke width was scaled twice.** Same cause, wider blast radius: every theme
   stroke was `scale²` thick, so icons at a non-1.0 device scale had fattened
   outlines while the 1.0 case looked perfect. A single-scale test would have
   passed.
3. **`.svgz` was advertised and never decompressed.** `SVG_ICON_EXTS` listed it;
   no path gunzipped it. A gzip stream is not SVG text, so every compressed theme
   asset failed to parse and fell back — silently, exactly like the original
   defect this ADR exists to fix.

All three now have regression tests on the *observable* output (rasterized alpha,
effective stroke width across four scales, and a real gzip fixture), and the
decompression lives in one place (`tessera-launch-services::vector_source`) with
its own bounded-expansion tests, because a `.svgz` in a theme directory is
untrusted input and a gzip bomb must not expand without a ceiling.

## Consequences

### Positive

- An XDG theme directory containing only `cursors/*.svg` renders correctly, at the
  output's own scale, with `data-hotspot-*` honoured — the defect that motivated
  this record, now asserted by `a_pure_svg_theme_resolves_without_any_prebake`.
- A host without librsvg loses nothing: application icons, HUD symbols and tray
  icons are all compiled in process.
- 80 lines of duplicated SVG scraping and a 40-line duplicated shape table are
  gone; the remaining shape policy is a shared primitive with its own tests.
- Cursor sprites are produced at `size * scale` instead of being upsampled from a
  64px bake, so HiDPI cursors are sharp.
- `.svgz` works wherever `.svg` does, through one bounded decompression rule, so a
  theme compressed for distribution renders identically.

### Negative / Trade-offs

- The compositor and the shell now link `libsvg` (and, transitively, `libflux`).
  That is the intended cost of one implementation; it is one library, not a
  dependency tree.
- `scripts/prepare-tessera-cursors.py` still runs `rsvg-convert` to produce the
  bundled theme's PNGs. Those PNGs are now an *override* for the bundled art, not
  the mechanism; regenerating them is optional and the SVGs alone are sufficient.
- Group opacity and `<use>`/`<text>`/masks are outside the compiler's surface
  (see Optics ADR-0150). Cursor and icon themes practically never use them;
  a theme that does loses that content rather than approximating it.
- Each cursor shape is compiled once per `(shape, scale)` and cached as a GPU
  texture, exactly as the pre-baked path did, so the steady-state cost is
  unchanged. The first frame pays one parse per distinct shape.

## References

- Optics ADR-0150, `docs/adr/0150-svg-vector-document-compiler.md` in the Optics
  repository — the compiler's own record: the flat shape IR, the paint model, the
  ceilings, and the format boundary. It is not linked here because each repository
  keeps its own append-only ADR registry and a cross-repository link would resolve
  to nothing in either one's index.
- [ADR-0207](0207-compositor-dbus-isolation-and-prebaked-cursor-strategy.md) —
  whose cursor half this supersedes while keeping its D-Bus isolation.
- [ADR-0122](0122-original-mit-cursor-theme-replaces-bibata.md) — the bundled art
  and its hotspot-stamping convention, both unchanged.
- The freedesktop cursor specification (`cursors/<name>.svg`, `Inherit=`).
