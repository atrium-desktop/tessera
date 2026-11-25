//! XDG cursor themes, compiled from SVG at load time.
//!
//! Tessera renders cursors from **SVG** rather than the legacy Xcursor binary
//! format: SVG is what the freedesktop cursor specification names
//! (`cursors/<name>.svg`), it stays crisp at any scale, and it is the only format
//! an unprivileged user can author by hand.
//!
//! Both halves of a cursor theme are read:
//!
//! - `cursors/<name>.svg` is compiled by the `optics-svg` compiler
//!   into premultiplied pixels at exactly the size the output needs. There is no
//!   pre-baked bitmap, no subprocess, and no runtime SVG library beyond that one
//!   device-less compiler.
//! - `cursors/<name>.png` is still honoured as an *override*: a theme that ships
//!   pre-rendered sprites gets its own art, unchanged. This keeps conventional
//!   themes working without making bitmaps the contract.
//!
//! A full original vector cursor theme is embedded in the binary
//! (`assets/cursors/tessera-user-light`, original MIT SVG art maintained
//! in-tree) so a crisp standalone cursor always exists with zero external
//! files.
//!
//! Hotspot convention: the bundled art stamps each cursor's hotspot on the
//! `<svg>` root as `data-hotspot-x` / `data-hotspot-y`, in the SVG's own
//! (viewBox) coordinate space. The compiler hands those attributes back verbatim
//! and this module scales them to the rendered size.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tessera_assets::cursors::{self, EmbeddedDir};
use tessera_primitives::cursor_shape::xdg_names;

/// The largest sprite any cursor theme may produce.
///
/// A hardware cursor plane is a fixed-size rectangle (64 or 128 pixels on every
/// driver Tessera targets); a sprite larger than this cannot be scanned out and
/// would silently fall back to the software path on every frame. Clamping here
/// keeps the failure at load time and out of the frame loop.
const MAX_CURSOR_SPRITE: u32 = 256;

/// One rasterized cursor image: premultiplied BGRA8 pixels with its hotspot.
struct RasterCursor {
    width: u32,
    height: u32,
    xhot: u32,
    yhot: u32,
    pixels: Vec<u8>,
}

/// Compile an SVG sprite into premultiplied BGRA8 at `out` pixels square.
///
/// The theme's own colours are honoured; `currentColor` resolves to opaque white
/// so a theme that relies on it is still visible on both polarities instead of
/// vanishing into the background. `tessera-assets` owns the raster layout and
/// the stamped-hotspot convention.
fn compile_svg_cursor(document: &tessera_assets::VectorDocument, out: u32) -> Option<RasterCursor> {
    let out = out.clamp(1, MAX_CURSOR_SPRITE);
    let raster = document.rasterize(out).ok()?;
    let (xhot, yhot) = document.hotspot(out);
    Some(RasterCursor {
        width: raster.width(),
        height: raster.height(),
        xhot,
        yhot,
        pixels: raster.into_bgra(),
    })
}

/// Decode a pre-baked PNG sprite into premultiplied BGRA8, applying the stamped
/// hotspot scaled from the document's native space.
///
/// This is the override path: a theme that ships `<name>.png` declared the pixels
/// it wants, so decoding wins over compiling `<name>.svg`. The sprite is resized
/// only when the theme baked a different size than the output needs.
fn decode_png_cursor(
    png: &[u8],
    document: Option<&tessera_assets::VectorDocument>,
    out: u32,
) -> Option<RasterCursor> {
    let out = out.clamp(1, MAX_CURSOR_SPRITE);
    let raster = tessera_assets::decode_bytes(png, out).ok()?;
    let (xhot, yhot) = match document {
        Some(document) => document.hotspot(out),
        None => (0, 0),
    };
    Some(RasterCursor {
        width: raster.width(),
        height: raster.height(),
        xhot,
        yhot,
        pixels: raster.into_bgra(),
    })
}

/// Cursor theme search roots, in XDG priority order.
fn search_roots() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("XCURSOR_PATH") {
        let roots = std::env::split_paths(&path).collect::<Vec<_>>();
        if !roots.is_empty() {
            return roots;
        }
    }

    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(&home).join(".icons"));
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(&home).join(".local/share"));
        roots.push(data_home.join("icons"));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':').filter(|d| !d.is_empty()) {
        roots.push(PathBuf::from(dir).join("icons"));
    }
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    roots
}

/// Read a theme's `Inherits=` list from its `index.theme`, if present.
fn theme_inherits(theme_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(theme_dir.join("index.theme")) else {
        return Vec::new();
    };
    let mut inherits = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Inherits=") {
            for name in value.split([',', ':']) {
                let name = name.trim();
                if !name.is_empty() {
                    inherits.push(name.to_string());
                }
            }
        }
    }
    inherits
}

/// One link in a theme's search chain: either a filesystem `cursors/` directory
/// or the in-binary bundled theme.
enum Source {
    Dir(PathBuf),
    Bundled,
    BundledDark,
    BundledAiLight,
    BundledAiDark,
}

impl Source {
    fn files(&self) -> &'static EmbeddedDir {
        match self {
            Source::Bundled => &cursors::USER_LIGHT,
            Source::BundledDark => &cursors::USER_DARK,
            Source::BundledAiLight => &cursors::AI_LIGHT,
            Source::BundledAiDark => &cursors::AI_DARK,
            // A directory has no embedded table; callers reach it through
            // `read` below.
            Source::Dir(_) => &cursors::USER_LIGHT,
        }
    }

    /// Read `<name>.<ext>` from this source.
    fn read(&self, name: &str, ext: &str) -> Option<Cow<'_, [u8]>> {
        match self {
            Source::Dir(dir) => std::fs::read(dir.join(format!("{name}.{ext}")))
                .ok()
                .map(Cow::Owned),
            _ => self
                .files()
                .get_file(format!("cursors/{name}.{ext}"))
                .map(Cow::Borrowed),
        }
    }

    /// The compiled SVG document for `name`, if this source provides one.
    ///
    /// `.svgz` is tried after `.svg`, because it is the same document under gzip
    /// and a theme may ship either. Decompression goes through the shared asset
    /// helper so a bundled asset and a filesystem theme obey one rule.
    ///
    /// Parsing happens once per source/name lookup; the caller (`load_shape`)
    /// walks the chain and takes the first hit, so a theme that only carries
    /// `<name>.png` costs two failed file reads and no parse.
    fn document(&self, name: &str) -> Option<tessera_assets::VectorDocument> {
        for extension in tessera_assets::VECTOR_EXTENSIONS {
            let Some(bytes) = self.read(name, extension) else {
                continue;
            };
            let Ok(text) = tessera_assets::decompress_if_gzip(bytes.into_owned()) else {
                continue; /* a corrupt .svgz must not hide a valid .svg */
            };
            if let Ok(document) = tessera_assets::VectorDocument::parse(&text) {
                return Some(document);
            }
        }
        None
    }
}

/// A resolved cursor theme: the search chain of candidate sources.
pub struct CursorTheme {
    chain: Vec<Source>,
    size: u32,
}

impl CursorTheme {
    /// Resolve a theme by name and preferred size. The search chain follows
    /// `index.theme` inheritance, breadth-first (cycles cut), and always ends with
    /// the bundled tessera-user-light theme so a cursor exists even when nothing
    /// is installed.
    pub fn resolve(name: &str, size: u32) -> CursorTheme {
        let roots = search_roots();
        let mut chain = Vec::new();
        let mut queue = vec![name.to_string()];
        let mut seen = std::collections::HashSet::new();
        while let Some(theme) = queue.pop() {
            if !seen.insert(theme.clone()) {
                continue;
            }
            match theme.as_str() {
                "tessera-user-light" | "Tessera" => {
                    chain.push(Source::Bundled);
                    continue;
                }
                "tessera-user-dark" => {
                    chain.push(Source::BundledDark);
                    continue;
                }
                "tessera-ai-light" => {
                    chain.push(Source::BundledAiLight);
                    continue;
                }
                "tessera-ai-dark" => {
                    chain.push(Source::BundledAiDark);
                    continue;
                }
                _ => {}
            }
            for root in &roots {
                let dir = root.join(&theme);
                if dir.is_dir() {
                    for inherited in theme_inherits(&dir) {
                        if !seen.contains(&inherited) {
                            queue.push(inherited);
                        }
                    }
                    chain.push(Source::Dir(dir.join("cursors")));
                    break;
                }
            }
        }
        chain.push(Source::Bundled);
        CursorTheme {
            chain,
            size: size.max(1),
        }
    }

    /// Whether any filesystem theme directory matched (i.e. the bundled fallback
    /// is not the only source in the chain).
    fn has_filesystem(&self) -> bool {
        self.chain.iter().any(|s| matches!(s, Source::Dir(_)))
    }

    /// How many filesystem sources can actually paint something.
    ///
    /// Two reasons a resolved theme directory contributes nothing: it ships only
    /// Xcursor binaries (unsupported by design), or it ships SVG the compiler
    /// rejected. Both are worth a log line, and both are distinguishable here.
    fn filesystem_sources(&self) -> (usize, usize) {
        let mut usable = 0;
        let mut total = 0;
        for source in &self.chain {
            let Source::Dir(dir) = source else {
                continue;
            };
            total += 1;
            let has_art = std::fs::read_dir(dir).is_ok_and(|mut entries| {
                entries.any(|entry| {
                    entry.ok().is_some_and(|entry| {
                        entry
                            .path()
                            .extension()
                            .is_some_and(|ext| matches!(ext.to_str(), Some("svg" | "svgz" | "png")))
                    })
                })
            });
            if has_art {
                usable += 1;
            }
        }
        (usable, total)
    }

    /// Load the best cursor for a `wp_cursor_shape` value at `scale`, trying each
    /// candidate name down the inheritance chain.
    ///
    /// Per candidate name and source, a pre-baked `.png` wins when present (the
    /// theme declared its own pixels), otherwise the `.svg` is compiled. The
    /// hotspot always comes from the SVG metadata when there is one, so a theme
    /// that ships both keeps a single source of truth for its hotspot.
    fn load_shape(&self, shape: u32, scale: f32) -> Option<RasterCursor> {
        let out = ((self.size as f32 * scale.max(0.25)).round() as u32).max(1);
        for name in xdg_names(shape) {
            for source in &self.chain {
                let document = source.document(name);
                if let Some(png) = source.read(name, "png")
                    && let Some(raster) = decode_png_cursor(&png, document.as_ref(), out)
                {
                    return Some(raster);
                }
                if let Some(document) = document.as_ref()
                    && let Some(raster) = compile_svg_cursor(document, out)
                {
                    return Some(raster);
                }
            }
        }
        None
    }
}

/// One resolved, uploaded cursor ready to draw: a texture plus its hotspot.
pub struct LoadedCursor {
    pub image: optics_gfx::Image,
    /// Original premultiplied BGRA pixels retained for cursor-inclusive
    /// screenshots on nested backends, whose host cursor is not part of the
    /// compositor framebuffer.
    pub pixels: std::sync::Arc<[u8]>,
    /// Draw size in physical pixels (the image's natural size).
    pub width: f32,
    pub height: f32,
    /// Hotspot offset within the image, in physical pixels.
    pub xhot: f32,
    pub yhot: f32,
}

/// Upload and cache theme cursors per (shape, scale). Keys are cheap; themes are
/// reloaded only when the runtime's effective preference changes.
pub struct CursorCache {
    theme: Option<(String, u32, CursorTheme)>,
    cursors: HashMap<(u32, u32), Option<LoadedCursor>>,
    preference_theme: String,
    preference_size: u32,
}

impl Default for CursorCache {
    fn default() -> Self {
        Self {
            theme: None,
            cursors: HashMap::new(),
            preference_theme: "default".into(),
            preference_size: 24,
        }
    }
}

impl CursorCache {
    /// Install the runtime's already-resolved cursor preferences. Environment
    /// overrides are resolved once in the compositor settings pipeline, so this
    /// rendering cache has no independent source of truth.
    pub fn set_preferences(&mut self, theme: String, size: u32) {
        if self.preference_theme != theme || self.preference_size != size {
            self.preference_theme = theme;
            self.preference_size = size;
            self.theme = None;
            self.cursors.clear();
        }
    }

    /// The effective pair supplied by the compositor preference resolver.
    fn effective(&self) -> (String, u32) {
        (self.preference_theme.clone(), self.preference_size)
    }

    /// Half-extent bound of the largest cursor sprite the active shape can show
    /// at `scale`, in logical pixels — the damage footprint of the pointer
    /// position for pointer-only partial repaints. Uses the cached rasterization
    /// when present (no GPU work) and falls back to a generous multiple of the
    /// preference size otherwise, since the true sprite is only known after `get`
    /// has rasterized it at least once.
    pub fn sprite_extent(&self, shape: u32, scale: f32) -> Option<f32> {
        let key = (shape, (scale * 4.0).round() as u32);
        if let Some(Some(loaded)) = self.cursors.get(&key) {
            // The hotspot can sit anywhere in the sprite, so the position plus
            // this half-extent always contains the drawn pixels.
            return Some(
                loaded
                    .width
                    .max(loaded.height)
                    .max(loaded.xhot.max(loaded.yhot) * 2.0)
                    / 2.0
                    + 2.0,
            );
        }
        // Unrasterized yet (first frames, a hidden cursor): the preference size
        // scaled up with a wide safety factor covers theme variation.
        Some(self.preference_size as f32 * scale.clamp(1.0, 3.0) * 2.0)
    }

    /// The cursor for a `wp_cursor_shape` value at `scale`. The bundled Tessera
    /// theme guarantees a result for every standard shape, so this returns `None`
    /// only if rasterization itself fails.
    pub fn get(
        &mut self,
        device: &optics_gfx::Device,
        shape: u32,
        scale: f32,
    ) -> Option<&LoadedCursor> {
        let (name, size) = self.effective();
        if !matches!(&self.theme, Some((n, s, _)) if *n == name && *s == size) {
            let theme = CursorTheme::resolve(&name, size);
            if matches!(
                name.as_str(),
                "tessera-user-light" | "tessera-user-dark" | "Tessera"
            ) {
                log::info!("cursor: using bundled {name:?} at {size} logical px");
            } else if theme.has_filesystem() {
                let (usable, total) = theme.filesystem_sources();
                if usable < total {
                    log::warn!(
                        "cursor: {}/{total} directory of theme {name:?} contains no svg or png \
                         cursors (Xcursor binaries are unsupported); those shapes fall back to \
                         the bundled tessera-user-light theme",
                        total - usable,
                    );
                } else {
                    log::info!(
                        "cursor: using SVG theme {name:?} at {size} logical px; missing shapes \
                         fall back to the bundled tessera-user-light theme"
                    );
                }
            } else {
                log::info!(
                    "cursor: theme {name:?} not installed; using bundled tessera-user-light theme \
                     at {size} logical px"
                );
            }
            self.theme = Some((name, size, theme));
            self.cursors.clear();
        }
        let key = (shape, (scale * 4.0).round() as u32);
        let theme = &self.theme.as_ref().expect("theme just set").2;
        self.cursors
            .entry(key)
            .or_insert_with(|| {
                let img = theme.load_shape(shape, scale)?;
                let pixels: std::sync::Arc<[u8]> = img.pixels.into();
                let image = optics_gfx::Image::from_bytes(
                    device,
                    img.width,
                    img.height,
                    optics_gfx::Format::Bgra8Unorm,
                    &pixels,
                )
                .ok()?;
                Some(LoadedCursor {
                    image,
                    pixels,
                    width: img.width as f32,
                    height: img.height as f32,
                    xhot: img.xhot as f32,
                    yhot: img.yhot as f32,
                })
            })
            .as_ref()
    }
}

/// Client-provided cursor surface (`wl_pointer.set_cursor`) prepared for
/// direct upload onto the KMS hardware cursor plane.
pub(crate) struct PreparedClientCursor<'a> {
    pub id: usize,
    pub generation: u64,
    pub pixels: Cow<'a, [u8]>,
    pub size: (u32, u32),
    pub hotspot: (u32, u32),
}

/// Why a mapped SHM client cursor surface could not be offloaded onto the KMS
/// hardware cursor plane and therefore fell back to composited overlay drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientCursorRejectReason {
    NonNormalTransform,
    ActiveViewport,
    InvalidDimensions,
    PixelBufferMismatch,
    NonIntegerScale,
    ExceedsPlaneExtent,
    HotspotOutOfBounds,
}

impl std::fmt::Display for ClientCursorRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonNormalTransform => f.write_str("non-normal buffer transform"),
            Self::ActiveViewport => f.write_str("active wp_viewport crop/scale"),
            Self::InvalidDimensions => f.write_str("non-positive dimensions or scale"),
            Self::PixelBufferMismatch => f.write_str("pixel buffer length mismatch"),
            Self::NonIntegerScale => f.write_str("non-integer output/buffer scale ratio"),
            Self::ExceedsPlaneExtent => f.write_str("exceeds KMS cursor plane extent"),
            Self::HotspotOutOfBounds => f.write_str("hotspot outside physical sprite bounds"),
        }
    }
}

/// Fast zero-allocation eligibility check matching [`prepare_client_hardware_cursor`].
pub(crate) fn can_offload_client_hardware_cursor(
    surface: &tessera_wayland::ClientCursorSurface<'_>,
    output_scale: f32,
    max_extent: (u32, u32),
) -> bool {
    validate_client_hardware_cursor(surface, output_scale, max_extent).is_ok()
}

struct ValidatedClientCursor {
    src_w: u32,
    src_h: u32,
    factor: u32,
    dst_w: u32,
    dst_h: u32,
    hotspot: (u32, u32),
}

fn validate_client_hardware_cursor(
    surface: &tessera_wayland::ClientCursorSurface<'_>,
    output_scale: f32,
    max_extent: (u32, u32),
) -> Result<ValidatedClientCursor, ClientCursorRejectReason> {
    if surface.transform != tessera_primitives::Transform::Normal {
        return Err(ClientCursorRejectReason::NonNormalTransform);
    }
    if surface.viewport_src.is_some() || surface.viewport_dst.is_some() {
        return Err(ClientCursorRejectReason::ActiveViewport);
    }
    if surface.width <= 0
        || surface.height <= 0
        || surface.buffer_scale <= 0
        || !output_scale.is_finite()
        || output_scale <= 0.0
    {
        return Err(ClientCursorRejectReason::InvalidDimensions);
    }
    let src_w = surface.width as u32;
    let src_h = surface.height as u32;
    let expected_len = src_w
        .checked_mul(src_h)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .ok_or(ClientCursorRejectReason::InvalidDimensions)?;
    if surface.pixels.len() != expected_len {
        return Err(ClientCursorRejectReason::PixelBufferMismatch);
    }
    let ratio = output_scale / (surface.buffer_scale as f32);
    let factor = ratio.round();
    if factor < 1.0 || (ratio - factor).abs() > 0.001 {
        return Err(ClientCursorRejectReason::NonIntegerScale);
    }
    let factor = factor as u32;
    let (Some(dst_w), Some(dst_h)) = (src_w.checked_mul(factor), src_h.checked_mul(factor)) else {
        return Err(ClientCursorRejectReason::ExceedsPlaneExtent);
    };
    if dst_w == 0 || dst_h == 0 || dst_w > max_extent.0 || dst_h > max_extent.1 {
        return Err(ClientCursorRejectReason::ExceedsPlaneExtent);
    }
    let (Some(logical_hot_x), Some(logical_hot_y)) = (
        surface.hotspot.x.checked_sub(surface.attach_offset.x),
        surface.hotspot.y.checked_sub(surface.attach_offset.y),
    ) else {
        return Err(ClientCursorRejectReason::HotspotOutOfBounds);
    };
    let phys_hot_x = (logical_hot_x as f32 * output_scale).round() as i64;
    let phys_hot_y = (logical_hot_y as f32 * output_scale).round() as i64;
    if phys_hot_x < 0
        || phys_hot_y < 0
        || phys_hot_x >= i64::from(dst_w)
        || phys_hot_y >= i64::from(dst_h)
    {
        return Err(ClientCursorRejectReason::HotspotOutOfBounds);
    }
    Ok(ValidatedClientCursor {
        src_w,
        src_h,
        factor,
        dst_w,
        dst_h,
        hotspot: (phys_hot_x as u32, phys_hot_y as u32),
    })
}

/// Validate and prepare a client SHM cursor surface for KMS hardware cursor
/// plane offload.
///
/// Returns `Err(reason)` when the surface cannot be represented losslessly on
/// the hardware cursor plane. In those cases the caller falls back to
/// compositing the cursor surface as an overlay without disabling the hardware
/// cursor plane for theme cursors.
pub(crate) fn prepare_client_hardware_cursor<'a>(
    surface: &tessera_wayland::ClientCursorSurface<'a>,
    output_scale: f32,
    max_extent: (u32, u32),
) -> Result<PreparedClientCursor<'a>, ClientCursorRejectReason> {
    let ValidatedClientCursor {
        src_w,
        src_h,
        factor,
        dst_w,
        dst_h,
        hotspot,
    } = validate_client_hardware_cursor(surface, output_scale, max_extent)?;

    let pixels = if factor == 1 {
        Cow::Borrowed(surface.pixels)
    } else {
        let dst_len = dst_w
            .checked_mul(dst_h)
            .and_then(|pixels| pixels.checked_mul(4))
            .map(|bytes| bytes as usize)
            .ok_or(ClientCursorRejectReason::ExceedsPlaneExtent)?;
        let mut scaled = vec![0u8; dst_len];
        let src_row_bytes = src_w as usize * 4;
        let dst_row_bytes = dst_w as usize * 4;
        let f = factor as usize;
        for y in 0..src_h as usize {
            let src_row = &surface.pixels[y * src_row_bytes..(y + 1) * src_row_bytes];
            let first_dst_y = y * f;
            let first_dst_row =
                &mut scaled[first_dst_y * dst_row_bytes..(first_dst_y + 1) * dst_row_bytes];
            for x in 0..src_w as usize {
                let px = &src_row[x * 4..(x + 1) * 4];
                for fx in 0..f {
                    let dst_off = (x * f + fx) * 4;
                    first_dst_row[dst_off..dst_off + 4].copy_from_slice(px);
                }
            }
            for fy in 1..f {
                let src_start = first_dst_y * dst_row_bytes;
                let dst_start = (first_dst_y + fy) * dst_row_bytes;
                scaled.copy_within(src_start..src_start + dst_row_bytes, dst_start);
            }
        }
        Cow::Owned(scaled)
    };

    Ok(PreparedClientCursor {
        id: surface.id,
        generation: surface.generation,
        pixels,
        size: (dst_w, dst_h),
        hotspot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_has_at_least_one_candidate_name() {
        for shape in [1, 4, 9, 13, 18, 23, 26, 32] {
            assert!(!xdg_names(shape).is_empty(), "shape {shape}");
        }
        assert_eq!(xdg_names(1)[0], "default");
        assert_eq!(xdg_names(9)[0], "text");
        assert_eq!(xdg_names(36)[0], "all-resize");
    }

    #[test]
    fn resolution_always_appends_the_bundled_fallback() {
        // No filesystem theme by this name exists on the test host, yet the chain
        // must still resolve a cursor via the bundled theme.
        let theme = CursorTheme::resolve("tessera-nonexistent-theme-xyz", 24);
        assert!(
            theme.chain.iter().any(|s| matches!(s, Source::Bundled)),
            "bundled fallback is always present"
        );
    }

    #[test]
    fn filesystem_theme_without_svg_is_reported_as_unusable() {
        let tmp = std::env::temp_dir().join(format!("tessera-cursor-test-{}", std::process::id()));
        let cursors = tmp.join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        // A conventional Xcursor theme: cursor files with no .svg and no .png.
        std::fs::write(cursors.join("left_ptr"), b"Xcur binary data").unwrap();
        let theme = CursorTheme {
            chain: vec![Source::Dir(cursors.clone()), Source::Bundled],
            size: 24,
        };
        assert!(theme.has_filesystem());
        assert_eq!(
            theme.filesystem_sources(),
            (0, 1),
            "a binary-only theme contributes no usable art"
        );

        // Dropping an SVG into the theme makes it usable without any pre-bake.
        std::fs::write(
            cursors.join("left_ptr.svg"),
            b"<svg viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\"/></svg>",
        )
        .unwrap();
        assert_eq!(theme.filesystem_sources(), (1, 1));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn bundled_polarities_cover_shapes_without_moving_hotspots() {
        let light = CursorTheme::resolve("tessera-user-light", 24);
        let dark = CursorTheme::resolve("tessera-user-dark", 24);
        assert!(matches!(light.chain.first(), Some(Source::Bundled)));
        assert!(matches!(dark.chain.first(), Some(Source::BundledDark)));
        for shape in 1..=36 {
            for scale in [1.0, 1.5, 2.0] {
                let a = light.load_shape(shape, scale).expect("light shape");
                let b = dark.load_shape(shape, scale).expect("dark shape");
                assert_eq!((a.xhot, a.yhot), (b.xhot, b.yhot));
                assert_eq!((a.width, a.height), (b.width, b.height));
                assert_ne!(a.pixels, b.pixels, "polarity must change the pixels");
                assert!(a.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
                assert!(
                    a.pixels
                        .chunks_exact(4)
                        .zip(b.pixels.chunks_exact(4))
                        .all(|(a, b)| a[3] == b[3]),
                    "polarity preserves coverage"
                );
            }
        }
        let legacy = CursorTheme::resolve("Tessera", 24)
            .load_shape(1, 1.0)
            .unwrap();
        assert_eq!(legacy.pixels, light.load_shape(1, 1.0).unwrap().pixels);
    }

    #[test]
    fn ai_bundled_polarities_cover_shapes() {
        let ai_light = CursorTheme::resolve("tessera-ai-light", 24);
        let ai_dark = CursorTheme::resolve("tessera-ai-dark", 24);
        assert!(matches!(
            ai_light.chain.first(),
            Some(Source::BundledAiLight)
        ));
        assert!(matches!(ai_dark.chain.first(), Some(Source::BundledAiDark)));
        for shape in 1..=36 {
            let a = ai_light.load_shape(shape, 1.0).expect("ai light shape");
            let b = ai_dark.load_shape(shape, 1.0).expect("ai dark shape");
            assert_eq!((a.xhot, a.yhot), (b.xhot, b.yhot));
            assert_eq!((a.width, a.height), (b.width, b.height));
            assert_ne!(a.pixels, b.pixels, "polarity must change pixels");
            assert!(a.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
        }

        // The user and Agent default cursors must be visually distinguishable:
        // the whole point of the dedicated AI theme.
        let user = CursorTheme::resolve("tessera-user-light", 24)
            .load_shape(1, 1.0)
            .unwrap();
        let ai = ai_light.load_shape(1, 1.0).unwrap();
        assert_ne!(
            user.pixels, ai.pixels,
            "AI cursor must have distinct geometry from user cursor"
        );
    }

    #[test]
    fn bundled_theme_compiles_the_default_cursor() {
        let theme = CursorTheme::resolve("tessera-nonexistent-theme-xyz", 24);
        let img = theme
            .load_shape(1, 1.0)
            .expect("bundled default (left_ptr) cursor compiles");
        assert_eq!(img.width, 24);
        assert_eq!(img.height, 24);
        assert_eq!(img.pixels.len(), (img.width * img.height * 4) as usize);
        assert!(img.xhot < img.width && img.yhot < img.height);
        // The rasterizer must produce visible art, not a correctly-sized blank
        // buffer. BGRA: alpha is every 4th byte.
        let opaque = img.pixels.chunks_exact(4).any(|px| px[3] > 0);
        assert!(opaque, "compiled cursor has at least one opaque pixel");
    }

    #[test]
    fn hotspot_scales_with_render_size() {
        let theme = CursorTheme::resolve("tessera-nonexistent-theme-xyz", 32);
        let img = theme.load_shape(1, 1.0).expect("bundled left_ptr");
        // left_ptr hotspot (64, 28) in 256-space -> 8, 4 at 32px.
        assert_eq!(img.xhot, 8, "x hot scales: 64 * 32 / 256 = 8");
        assert_eq!(img.yhot, 4, "y hot scales: 28 * 32 / 256 = 3.5 -> 4");
    }

    #[test]
    fn sprite_size_is_clamped_to_the_scanout_ceiling() {
        // The source art is 256px; requesting far more must not allocate a giant
        // sprite, it is clamped to the hardware cursor ceiling.
        let theme = CursorTheme::resolve("tessera-nonexistent-theme-xyz", 4096);
        let img = theme.load_shape(9, 4.0).expect("bundled text cursor");
        assert_eq!(img.width, MAX_CURSOR_SPRITE);
        assert!(img.width <= MAX_CURSOR_SPRITE);
    }

    #[test]
    fn an_svgz_theme_asset_is_decompressed() {
        // `.svgz` is the same document under gzip and appears in the cursor
        // specification's extension list. Advertising it while feeding the
        // parser raw gzip made every svgz cursor silently unresolvable.
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!("tessera-cursor-svgz-{}", std::process::id()));
        let cursors = tmp.join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        let svg = b"<svg viewBox=\"0 0 256 256\" data-hotspot-x=\"64\" data-hotspot-y=\"28\">\
                    <rect width=\"256\" height=\"256\" fill=\"#123456\"/></svg>";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(svg).unwrap();
        std::fs::write(cursors.join("default.svgz"), encoder.finish().unwrap()).unwrap();

        let theme = CursorTheme {
            chain: vec![Source::Dir(cursors)],
            size: 24,
        };
        let raster = theme.load_shape(1, 1.0).expect("svgz theme compiles");
        assert_eq!((raster.width, raster.height), (24, 24));
        assert!(
            (raster.xhot, raster.yhot) == (6, 3),
            "hotspot from the svgz"
        );
        assert!(
            raster.pixels.chunks_exact(4).any(|px| px[3] > 0),
            "svgz asset produced visible art"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_corrupt_svgz_does_not_hide_a_valid_png() {
        // A broken `.svgz` must fall through to the sibling bitmap, not abort the
        // whole lookup: one bad file in a theme must not blank the cursor.
        let tmp =
            std::env::temp_dir().join(format!("tessera-cursor-badsvgz-{}", std::process::id()));
        let cursors = tmp.join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        std::fs::write(cursors.join("default.svgz"), b"\x1f\x8bnot really gzip").unwrap();
        let png =
            tessera_image::encode_png_rgba(1, 1, &[10, 200, 30, 255], false).expect("encode png");
        std::fs::write(cursors.join("default.png"), &png).unwrap();

        let theme = CursorTheme {
            chain: vec![Source::Dir(cursors)],
            size: 16,
        };
        let raster = theme.load_shape(1, 1.0).expect("png still resolves");
        let center = (8 * 16 + 8) * 4;
        // BGRA of RGB(10,200,30): blue=30, green=200, red=10.
        assert!(
            raster.pixels[center + 1] > 150,
            "the PNG painted, not the svgz"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_png_override_wins_over_the_sibling_svg() {
        // A theme that ships pre-rendered art declared those pixels; compiling its
        // SVG instead would silently ignore the artist's own bake.
        let tmp =
            std::env::temp_dir().join(format!("tessera-cursor-override-{}", std::process::id()));
        let cursors = tmp.join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        std::fs::write(
            cursors.join("default.svg"),
            b"<svg viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\" fill=\"#FF0000\"/></svg>",
        )
        .unwrap();
        // A 1x1 opaque white PNG: distinguishable from the red square above.
        let png =
            tessera_image::encode_png_rgba(1, 1, &[255, 255, 255, 255], false).expect("encode png");
        std::fs::write(cursors.join("default.png"), &png).unwrap();

        let theme = CursorTheme {
            chain: vec![Source::Dir(cursors)],
            size: 16,
        };
        let raster = theme.load_shape(1, 1.0).expect("cursor");
        // The PNG is white; the SVG is red. BGRA channel order.
        let center = (8 * 16 + 8) * 4;
        assert!(
            raster.pixels[center + 2] > 200 && raster.pixels[center] > 200,
            "the theme's own PNG must win over the sibling SVG"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_pure_svg_theme_resolves_without_any_prebake() {
        // The defect this module exists to close: an XDG theme directory that
        // contains only .svg must paint, with no rsvg-convert and no PNG bake.
        let tmp =
            std::env::temp_dir().join(format!("tessera-cursor-svgonly-{}", std::process::id()));
        let cursors = tmp.join("cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        std::fs::write(
            cursors.join("default.svg"),
            b"<svg viewBox=\"0 0 256 256\" data-hotspot-x=\"64\" data-hotspot-y=\"28\">\
              <path d=\"M64 28L192 142L134 146L164 210L137 223L106 157L64 198Z\" fill=\"#F5F7FA\" \
              stroke=\"#202630\" stroke-width=\"10\" stroke-linejoin=\"round\"/></svg>",
        )
        .unwrap();
        let theme = CursorTheme {
            chain: vec![Source::Dir(cursors)],
            size: 24,
        };
        let raster = theme.load_shape(1, 1.0).expect("svg-only theme compiles");
        assert_eq!((raster.width, raster.height), (24, 24));
        assert!(raster.pixels.chunks_exact(4).any(|px| px[3] > 0));
        // Hotspot (64,28) in 256-space at 24px -> (6, 3).
        assert_eq!((raster.xhot, raster.yhot), (6, 3));
        std::fs::remove_dir_all(&tmp).ok();
    }

    fn test_client_cursor(
        width: i32,
        height: i32,
        pixels: &[u8],
        hotspot: (i32, i32),
        attach_offset: (i32, i32),
        buffer_scale: i32,
    ) -> tessera_wayland::ClientCursorSurface<'_> {
        tessera_wayland::ClientCursorSurface {
            id: 42,
            generation: 7,
            width,
            height,
            pixels,
            hotspot: tessera_primitives::Point {
                x: hotspot.0,
                y: hotspot.1,
            },
            attach_offset: tessera_primitives::Point {
                x: attach_offset.0,
                y: attach_offset.1,
            },
            buffer_scale,
            transform: tessera_primitives::Transform::Normal,
            viewport_src: None,
            viewport_dst: None,
        }
    }

    #[test]
    fn client_hardware_cursor_borrows_zero_copy_on_exact_scale_match() {
        let pixels = vec![0xAA; 16 * 16 * 4];
        let surface = test_client_cursor(16, 16, &pixels, (4, 5), (1, 2), 1);
        assert!(can_offload_client_hardware_cursor(&surface, 1.0, (64, 64)));
        let prepared =
            prepare_client_hardware_cursor(&surface, 1.0, (64, 64)).expect("eligible 1:1 cursor");
        assert!(matches!(prepared.pixels, Cow::Borrowed(_)));
        assert_eq!(prepared.size, (16, 16));
        assert_eq!(prepared.hotspot, (3, 3));
        assert_eq!((prepared.id, prepared.generation), (42, 7));
    }

    #[test]
    fn client_hardware_cursor_upscales_integer_hidpi_ratio() {
        // 2x2 BGRA8 cursor at buffer_scale=1 on a 2x HiDPI output -> 4x4 physical.
        let pixels = [
            1, 2, 3, 255, 4, 5, 6, 255, // row 0
            7, 8, 9, 255, 10, 11, 12, 255, // row 1
        ];
        let surface = test_client_cursor(2, 2, &pixels, (1, 1), (0, 0), 1);
        assert!(can_offload_client_hardware_cursor(&surface, 2.0, (64, 64)));
        let prepared = prepare_client_hardware_cursor(&surface, 2.0, (64, 64))
            .expect("eligible 2x integer upscale");
        assert!(matches!(prepared.pixels, Cow::Owned(_)));
        assert_eq!(prepared.size, (4, 4));
        assert_eq!(prepared.hotspot, (2, 2));
        assert_eq!(prepared.pixels.len(), 4 * 4 * 4);
        // Top-left 2x2 block repeats pixel (0,0) = [1, 2, 3, 255].
        assert_eq!(&prepared.pixels[0..8], &[1, 2, 3, 255, 1, 2, 3, 255]);
        assert_eq!(&prepared.pixels[16..24], &[1, 2, 3, 255, 1, 2, 3, 255]);
        // Bottom-right pixel (3,3) repeats source pixel (1,1) = [10, 11, 12, 255].
        assert_eq!(&prepared.pixels[60..64], &[10, 11, 12, 255]);
    }

    #[test]
    fn client_hardware_cursor_rejects_ineligible_surfaces_cleanly() {
        let pixels = vec![0xFF; 32 * 32 * 4];
        // Oversized after 2x HiDPI scaling (64x64 exceeds 64x32 limit).
        let surface = test_client_cursor(32, 32, &pixels, (0, 0), (0, 0), 1);
        assert!(!can_offload_client_hardware_cursor(&surface, 2.0, (64, 32)));
        assert_eq!(
            prepare_client_hardware_cursor(&surface, 2.0, (64, 32)).err(),
            Some(ClientCursorRejectReason::ExceedsPlaneExtent)
        );

        // Non-integer scale ratio (1.5x).
        assert!(!can_offload_client_hardware_cursor(&surface, 1.5, (64, 64)));
        assert_eq!(
            prepare_client_hardware_cursor(&surface, 1.5, (64, 64)).err(),
            Some(ClientCursorRejectReason::NonIntegerScale)
        );

        // Hotspot outside sprite (negative after attach_offset or >= size).
        let bad_hot = test_client_cursor(32, 32, &pixels, (0, 0), (4, 0), 1);
        assert!(!can_offload_client_hardware_cursor(&bad_hot, 1.0, (64, 64)));
        assert_eq!(
            prepare_client_hardware_cursor(&bad_hot, 1.0, (64, 64)).err(),
            Some(ClientCursorRejectReason::HotspotOutOfBounds)
        );

        // Non-normal transform or active viewport.
        let mut rotated = test_client_cursor(32, 32, &pixels, (0, 0), (0, 0), 1);
        rotated.transform = tessera_primitives::Transform::Rotate90;
        assert!(!can_offload_client_hardware_cursor(&rotated, 1.0, (64, 64)));
        assert_eq!(
            prepare_client_hardware_cursor(&rotated, 1.0, (64, 64)).err(),
            Some(ClientCursorRejectReason::NonNormalTransform)
        );

        let mut viewported = test_client_cursor(32, 32, &pixels, (0, 0), (0, 0), 1);
        viewported.viewport_dst = Some(tessera_primitives::Size { w: 16, h: 16 });
        assert!(!can_offload_client_hardware_cursor(
            &viewported,
            1.0,
            (64, 64)
        ));
        assert_eq!(
            prepare_client_hardware_cursor(&viewported, 1.0, (64, 64)).err(),
            Some(ClientCursorRejectReason::ActiveViewport)
        );

        // Invalid dimensions and pixel buffer length mismatch.
        let zero_dim = test_client_cursor(0, 32, &pixels, (0, 0), (0, 0), 1);
        assert_eq!(
            prepare_client_hardware_cursor(&zero_dim, 1.0, (64, 64)).err(),
            Some(ClientCursorRejectReason::InvalidDimensions)
        );
        let short_buf = test_client_cursor(32, 32, &pixels[..16], (0, 0), (0, 0), 1);
        assert_eq!(
            prepare_client_hardware_cursor(&short_buf, 1.0, (64, 64)).err(),
            Some(ClientCursorRejectReason::PixelBufferMismatch)
        );
    }
}
