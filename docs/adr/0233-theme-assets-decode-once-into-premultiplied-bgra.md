---
id: ADR-0233
title: "Theme assets decode once, into premultiplied BGRA8"
status: accepted
date: 2026-11-12
scope: compositor/icons, compositor/cursor, shell/tray, dependencies
superseded_by: null
negative_knowledge: true
---

# 0233. Theme assets decode once, into premultiplied BGRA8

- Status: Accepted
- Date: 2026-11-12
- Deciders: Tessera Maintainers
- Amends: [ADR-0222](0222-one-in-process-svg-compiler-for-every-vector-asset.md)
  (its "the two decoders share the one mechanism" claim is now an actual
  dependency boundary, not two copies of the same function)

---

## Context and Problem Statement

Every installed application icon rendered with its red and blue channels
exchanged: Chrome's red fan arrived blue, its blue centre arrived orange, and
Firefox's orange flame arrived blue. The icon theme was not at fault; the theme
happened to supply SVG icons, and the SVG path swapped channels twice.

The swap was the visible symptom of a structural defect. The stack decoded the
same asset in two places:

- `crates/tessera/src/runtime/apps.rs`, for application and HUD icons;
- `crates/tessera-shell/src/tray/icon.rs`, for StatusNotifierItem icons.

Both carried a copy of the raster/vector extension lists, a copy of
`decode_icon`, and a copy of the RGBA→BGRA swap. The compositor's copy swapped
inside `decode_icon` and returned the result inside an `image::RgbaImage`; the
callers then swapped again at upload. Two swaps cancelled, so SVG art uploaded
as `Bgra8Unorm` with its red and blue channels reversed. The tray copy carried
the identical bug because it was copied, not shared.

The duplication also hid a second, wider defect: the stack had no single answer
to "is this pixel buffer premultiplied?".

| Path | Decoded alpha | Uploaded as | Result |
| --- | --- | --- | --- |
| SVG icons and cursors | premultiplied | `Bgra8Unorm` | correct |
| PNG icons | straight | `Bgra8Unorm` | transparent edges too bright |
| PNG cursors | straight | `Bgra8Unorm` | documented as premultiplied, but was not |
| Agent PNG sprites | straight | `Bgra8Unorm` | transparent edges too bright |
| SNI `IconPixmap` | straight | `Bgra8Unorm` | transparent edges too bright |

flux samples texels with a premultiplied-alpha convention (`flux_image_update_region_premultiply`
exists precisely so callers do not have to implement it). Four of the five paths
violated that convention. PNG decoding also scaled in straight-alpha space, so a
transparent pixel could bleed its hidden colour into an antialiased edge.

Finally, the advertised format lists were not honest. The theme resolver looked
for XPM, which no decoder in the stack can paint. Both icon decoders listed
`bmp`, `tiff` and `ico`, while the workspace's `image` build enables only
`png`, `jpeg`, `gif` and `webp`. ADR-0222's `[INV-ASSET-06]` ("an advertised
extension is a working extension") was already violated in the tree.

## Decision Drivers

- **One mechanism, not two copies with different policies.** Policy differences
  belong in the caller; channel order and alpha conventions do not.
- **One alpha convention at the renderer boundary.** Every decoded asset must
  arrive in the layout flux samples, with no caller-side post-processing.
- **Mechanical prevention, not a comment.** A future consumer must be unable to
  import the SVG compiler and reintroduce a second rasterizer without a
  dependency-policy failure.
- **Dependency hygiene** (ADR-0206, ADR-0209). The shared decoder must not drag
  `image` and `reticle` into `tessera-cli` and `tessera-mcp` merely because they
  enumerate desktop entries.
- **Honest format vocabulary.** A resolver may only return a file a decoder can
  paint.
- **Bounded, correctly-scaled output.** Decoding stays on the worker thread and
  produces a texture at the caller's pinned extent, premultiplied before scaling.

## Considered Options

- **Remove the inner swap in both copies and keep decoding in place.**
  Rejected: it fixes today's channels and leaves the class of defect intact. The
  next caller opts into straight alpha, or resizes before premultiplying, or
  adds a third copy, and no boundary notices.

- **Move decoding into `tessera-launch-services`, which already owns icon-theme
  lookup and vector-source reading.** Rejected: `tessera-cli` and `tessera-mcp`
  depend on launch services for application discovery and intent search. Giving
  that crate `image` and `reticle` edges would make a headless CLI link the SVG
  compiler and native Optics graphics libraries to list a `.desktop` file.

- **Move decoding into `tessera-shell` and have the compositor call up to it.**
  Rejected: it makes the chrome host the owner of filesystem asset decoding and
  ties the composition root's icon path to a UI crate. The decoder is a lower
  capability than either consumer.

- **Add `tessera-assets` and make it the only crate with a `reticle` edge.**
  Chosen.

## Decision Outcome

Tessera adds **`tessera-assets`**, a pure data crate with no flux/lens edge. It
owns bytes and pixels; consumers own GPU uploads.

### 1. One decode entry point per asset shape

- `read_vector_source(path)` and `decompress_if_gzip(bytes)` move out of
  `tessera-launch-services::vector_source`. Bound-expansion rules are unchanged,
  and the `.svgz` bomb ceiling still lives with the reader.
- `VectorDocument::parse` / `from_file` is the only type in the desktop that owns
  a `reticle::Document`. It rasterizes, reports its viewport, and reads the
  `data-hotspot-*` convention. Cursor themes and Agent sprites no longer carry
  their own hotspot readers.
- `decode_file(path, extent)` dispatches by extension: vector sources through
  `VectorDocument`, raster sources through `decode_bytes`.
- `decode_bytes(bytes, extent)` decodes a PNG/JPEG/GIF/WebP, premultiplies,
  scales in premultiplied space, and returns `RasterImage`.
- `RasterImage` is `width * height * 4` bytes of premultiplied BGRA8, validated
  on construction. `as_bgra` is exactly what `flux::Image::from_bytes(...,
  Bgra8Unorm, ...)` accepts.
- `RasterImage::recolour_symbolic` paints a symbolic icon as `colour * coverage`,
  preserving the premultiplied contract instead of writing a flat RGB value over
  partial alpha.
- `premultiply_bgra_in_place` is the single conversion for transport payloads
  such as SNI's ARGB32.

### 2. Every consumer uploads the same layout

- `tessera::runtime::apps` decodes application icons, the embedded default icon,
  and HUD symbols through `tessera-assets`, then uploads `as_bgra()` unchanged.
- `tessera::cursor` compiles SVGs and decodes PNG overrides through
  `tessera-assets`, including hotspots.
- `tessera-shell::tray` decodes theme names and converts SNI `IconPixmap`
  payloads to premultiplied BGRA through the same crate.
- `tessera-shell::chrome::agent_feedback` uploads SVG and PNG sprites from the
  same decode path.

### 3. The resolver advertises only decodable formats

`tessera_launch_services::ICON_EXTENSIONS` is `png`, `svg`, `svgz`. XPM is
retired: no decoder exists, and the spec naming a third legacy format does not
make a silent fallback acceptable. The compositor asserts every resolver
extension against `tessera_assets::supports_extension`.

### 4. The boundary is enforced by Cargo, not convention

`reticle` appears in exactly one package's normal dependency list:
`tessera-assets`. `tessera` and `tessera-shell` keep a direct `reticle-sys` edge
only because their build scripts need `DEP_RETICLE_RPATHS` for test binaries;
they no longer link the Rust compiler crate. A future crate that wants to
rasterize SVG must either call `tessera-assets` or add itself to
`tooling/xtask/dependency-policy.toml`, where the change is reviewed as an
architectural edge.

## Invariants & Behavioral Boundaries

- `[INV-ASSET-07] One rasterization boundary.` `tessera-assets` is the only
  package with a normal `reticle` dependency; no other crate parses or rasterizes
  theme vector assets. **Enforcement:** `cargo run -p xtask -- check-boundaries`
  rejects any new `reticle` normal edge outside the policy entry.

- `[INV-ASSET-08] Premultiplied BGRA8 is the only upload layout.` Every decoded
  theme asset reaches a `Bgra8Unorm` upload as premultiplied BGRA8, with no
  caller-side channel swap or alpha conversion. **Enforcement:**
  `tessera-assets::tests::{a_vector_asset_is_premultiplied_bgra,
  raster_alpha_is_premultiplied_at_the_boundary,
  symbolic_recolour_tracks_coverage}`, plus the tray's full-colour SVG channel
  test and the SNI ARGB conversion test.

- `[INV-ASSET-06]` (ADR-0222, retained) **An advertised format has a decoder.**
  The resolver's extension list is checked against the decoder in
  `every_resolved_icon_extension_has_a_decoder`.

## Rejected Alternatives & Negative Knowledge

- **Never return an `image::DynamicImage` from a shared decoder whose contract is
  "pixels".** The type encodes neither channel order nor premultiplication, so
  every caller was free to read the buffer differently. That freedom is exactly
  how one function produced two contradictory layouts. `RasterImage` names its
  layout.

- **Never keep two decoders because their policies differ.** The tray recolours
  symbolic icons; the compositor recolours HUD symbols. That difference belongs
  in the caller. The duplicated mechanism had already drifted into a channel
  swap and four alpha conventions.

- **Never advertise a format because a specification names it.** XPM was
  resolvable and unpaintable; `bmp`/`tiff`/`ico` were listed with their `image`
  features disabled. A resolver that returns an undecodable path turns a format
  into a silent fallback.

- **Never resize straight-alpha pixels.** Transparent pixels carry meaningless
  colour. Premultiply first; then every filter tap and every antialiased edge is
  a weighted average of visible light, not of hidden colour.

- **Never premultiply a buffer twice.** Consumers upload `RasterImage::as_bgra()`
  directly. The first pass of this work removed the double channel swap but left
  the PNG paths straight-alpha, so the visible defect changed rather than
  disappeared; the consolidated crate is the actual fix.

## Consequences

### Positive

- Full-colour SVG icons render with their authored colours. The red channel that
  exposed the defect is asserted at the tray boundary and in `tessera-assets`.
- Transparent edges of PNG icons, PNG cursors, Agent sprites, and SNI pixmaps
  composite correctly, because they are premultiplied before scaling and before
  upload.
- One decoder, one alpha convention, and one hotspot reader replace two decoders
  and two hotspot readers.
- `tessera-cli` and `tessera-mcp` no longer transitively pull `flate2` through
  launch services, and no non-asset crate links the Rust `reticle` crate.
- The resolver can no longer return a format the decoder rejects.

### Negative / Trade-offs

- Raster icons are now pinned to the caller's extent (`DEFAULT_ICON_SIZE *
  scale`, capped at 512) instead of being uploaded at whatever native size the
  theme shipped. This makes texture bounds explicit and matches the SVG path; a
  theme that ships a 24px PNG on a 2× output is upscaled on the worker thread
  rather than on the GPU.
- An XPM-only theme no longer resolves. No decoder exists, and a silent
  fallback is worse than an explicit miss.
- The SNI conversion assumes the specification's linear ARGB32, as the previous
  code did in its docs. A producer that ships premultiplied pixmaps would now be
  darkened at the edges; the same producer was already wrong against the
  documented contract.
- A new workspace package and dependency-policy entry exist. That is the price
  of making the boundary mechanical.

## Verification

- `cargo test -p tessera-assets` — premultiplied SVG, premultiplied PNG, symbolic
  coverage, hotspot parsing, `.svgz` reader, extension honesty.
- `cargo test -p tessera-shell --lib tray` — full-colour SVG channel order, SNI
  ARGB conversion, symbolic recolour, `.svgz`.
- `cargo test -p tessera-shell --lib agent_feedback` — sprite coverage is
  preserved across polarities after the shared decode.
- `cargo test -p tessera --lib cursor` — SVG and PNG cursors still resolve, keep
  their hotspots, and honour the scanout ceiling.
- `cargo test -p tessera --lib runtime::apps` — default icon decodes, resolver
  extensions all decode, extent policy is bounded.
- `cargo run -p xtask -- check-boundaries` — `reticle` has exactly one normal
  dependency edge.
- `cargo run -p xtask -- check-adrs` — this record, its index entry and its
  lineage links resolve.
