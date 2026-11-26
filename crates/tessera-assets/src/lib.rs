//! Theme asset decoding for the whole desktop (ADR-0233).
//!
//! Two questions must have exactly one answer in this stack:
//!
//! 1. **How are a theme asset's bytes read?** SVG text and SVGZ (SVG under
//!    gzip) are the two carriers of a vector document; [`vector`] reads both,
//!    with a bounded decompression step because a theme directory is untrusted
//!    input.
//! 2. **What layout does the renderer receive?** Every raster and every vector
//!    document is decoded into **premultiplied BGRA8**, the layout gfx samples
//!    through `Bgra8Unorm`. Callers never swap channels, premultiply, resize in
//!    the wrong alpha space, or guess a convention: those decisions exist here
//!    exactly once.
//!
//! The crate deliberately has no gfx/optics-ui edge. It owns bytes and pixels, not
//! GPU resources, so the compositor, the shell, and any future surface decode
//! an asset the same way without sharing a renderer.

pub mod cursors;
mod svg;
pub mod vector;

pub use vector::{SourceError, decompress_if_gzip, read_vector_source};

use std::path::{Path, PathBuf};

use tessera_image::{Filter, ImageError, RgbaImage};

/// Raster formats the image decoder handles.
///
/// This is the complete raster vocabulary: an extension outside this list is not
/// advertised to a caller and [`decode_file`] refuses it. XPM is deliberately
/// absent — the stack has no XPM decoder, and resolving a name to a file that
/// cannot be painted is a silent wrong answer (ADR-0222, `[INV-ASSET-06]`).
pub const RASTER_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Vector formats the in-process `librsvg` backend consumes.
pub const VECTOR_EXTENSIONS: &[&str] = &["svg", "svgz"];

/// Hard ceiling on a decoded raster edge, independent of the caller's policy.
///
/// Callers keep their own tighter budgets (application icons cap at 512, cursor
/// sprites at 256); this one stops an accidental request from allocating an
/// unbounded texture.
pub const MAX_EXTENT: u32 = 4096;

/// Why an asset could not be decoded.
#[derive(Debug)]
pub enum DecodeError {
    /// The file extension names no decoder this crate implements.
    Unsupported(String),
    /// The file could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The raster decoder rejected the bytes.
    Raster(ImageError),
    /// The vector source could not be read or decompressed.
    Source(SourceError),
    /// The SVG backend rejected or failed to render the document.
    Vector(String),
    /// A buffer's extent does not match its pixel byte length.
    Extent { width: u32, height: u32, len: usize },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(ext) => write!(f, "unsupported theme asset extension {ext:?}"),
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Raster(err) => write!(f, "{err}"),
            Self::Source(err) => write!(f, "{err}"),
            Self::Vector(err) => write!(f, "vector document rejected: {err}"),
            Self::Extent { width, height, len } => {
                write!(
                    f,
                    "raster extent {width}x{height} does not match {len} pixel bytes"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Raster(err) => Some(err),
            Self::Source(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ImageError> for DecodeError {
    fn from(err: ImageError) -> Self {
        Self::Raster(err)
    }
}

impl From<SourceError> for DecodeError {
    fn from(err: SourceError) -> Self {
        Self::Source(err)
    }
}

/// One decoded asset in the renderer's native CPU layout.
///
/// `bgra` is `width * height * 4` bytes of **premultiplied** blue, green, red,
/// alpha. It is ready for `optics_gfx::Image::from_bytes(..., Bgra8Unorm, ...)` with
/// no post-processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterImage {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
}

impl RasterImage {
    /// Wrap an already-premultiplied BGRA8 buffer, validating its extent.
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self, DecodeError> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if width == 0 || height == 0 || expected != Some(bgra.len()) {
            return Err(DecodeError::Extent {
                width,
                height,
                len: bgra.len(),
            });
        }
        Ok(Self {
            width,
            height,
            bgra,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Premultiplied BGRA8 pixels.
    pub fn as_bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// Take ownership of the premultiplied BGRA8 pixels for a GPU upload.
    pub fn into_bgra(self) -> Vec<u8> {
        self.bgra
    }

    /// Paint a symbolic icon in one foreground colour.
    ///
    /// The input is premultiplied, so the replacement is too: each channel is
    /// `colour * coverage`, with the coverage byte kept as alpha. Writing a flat
    /// colour instead would make every antialiased edge brighter than its own
    /// alpha allows.
    pub fn recolour_symbolic(&mut self, rgb: [u8; 3]) {
        let [red, green, blue] = rgb;
        for pixel in self.bgra.chunks_exact_mut(4) {
            let alpha = pixel[3];
            pixel[0] = premultiply_channel(blue, alpha);
            pixel[1] = premultiply_channel(green, alpha);
            pixel[2] = premultiply_channel(red, alpha);
        }
    }
}

/// A parsed SVG/SVGZ document backed by in-process `librsvg`.
///
/// This is the only type in the desktop that owns a theme SVG document.
/// Consumers ask it to rasterize or to report its stamped hotspot; they never
/// link a vector renderer themselves, which is what makes "one asset
/// rasterizer" a dependency boundary rather than a convention (ADR-0233,
/// ADR-0261).
pub struct VectorDocument {
    inner: svg::SvgDocument,
}

impl VectorDocument {
    /// Parse a vector source. SVGZ must already have been decompressed by
    /// [`read_vector_source`] or [`decompress_if_gzip`].
    pub fn parse(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            inner: svg::SvgDocument::parse(bytes)?,
        })
    }

    /// Read and parse a vector file, transparently gunzipping `.svgz`.
    pub fn from_file(path: &Path) -> Result<Self, DecodeError> {
        Self::parse(&read_vector_source(path)?)
    }

    /// Rasterize the document into premultiplied BGRA8 at `extent` pixels square.
    pub fn rasterize(&self, extent: u32) -> Result<RasterImage, DecodeError> {
        self.inner.rasterize(clamp_extent(extent))
    }

    /// The document's viewport as `[x, y, width, height]` in user units.
    pub fn viewport(&self) -> [f32; 4] {
        self.inner.viewport()
    }

    /// A stamped hotspot scaled to `extent` pixels, clamped inside the sprite.
    ///
    /// Matches [`Self::rasterize`]'s uniform `CONTAIN` fit (centred inside the
    /// `extent × extent` square) and accounts for non-zero `viewBox` origins.
    /// A theme that stamps nothing gets the top-left corner, the XDG convention
    /// for "no hotspot data".
    pub fn hotspot(&self, extent: u32) -> (u32, u32) {
        let [vx, vy, vw, vh] = self.viewport();
        let (hx, hy) = (self.inner.hotspot_x(), self.inner.hotspot_y());
        if hx.is_none() && hy.is_none() {
            return (0, 0);
        }
        let extent_f = extent as f32;
        let scale = if vw > 0.0 && vh > 0.0 {
            (extent_f / vw).min(extent_f / vh)
        } else {
            1.0
        };
        let offset_x = (extent_f - vw * scale) * 0.5;
        let offset_y = (extent_f - vh * scale) * 0.5;
        let max_px = extent.saturating_sub(1);
        let map_axis = |value: Option<f32>, origin: f32, offset: f32| -> u32 {
            let Some(v) = value else {
                return 0;
            };
            let pixels = (v - origin) * scale + offset;
            (pixels.round().max(0.0) as u32).min(max_px)
        };
        (map_axis(hx, vx, offset_x), map_axis(hy, vy, offset_y))
    }

    /// A stamped hotspot as a fraction of the square sprite in `0.0..=1.0`.
    ///
    /// Matches [`Self::rasterize`]'s uniform `CONTAIN` fit and non-zero
    /// `viewBox` origins.
    pub fn normalized_hotspot(&self) -> (f32, f32) {
        let [vx, vy, vw, vh] = self.viewport();
        let (hx, hy) = (self.inner.hotspot_x(), self.inner.hotspot_y());
        if hx.is_none() && hy.is_none() {
            return (0.0, 0.0);
        }
        let max_dim = vw.max(vh);
        if !max_dim.is_finite() || max_dim <= 0.0 {
            return (0.0, 0.0);
        }
        let offset_x = (max_dim - vw) * 0.5;
        let offset_y = (max_dim - vh) * 0.5;
        let map_axis = |value: Option<f32>, origin: f32, offset: f32| -> f32 {
            let Some(v) = value else {
                return 0.0;
            };
            ((v - origin + offset) / max_dim).clamp(0.0, 1.0)
        };
        (map_axis(hx, vx, offset_x), map_axis(hy, vy, offset_y))
    }
}

/// Premultiply a BGRA8 buffer in place.
pub fn premultiply_bgra_in_place(bgra: &mut [u8]) {
    for pixel in bgra.chunks_exact_mut(4) {
        let alpha = pixel[3];
        pixel[0] = premultiply_channel(pixel[0], alpha);
        pixel[1] = premultiply_channel(pixel[1], alpha);
        pixel[2] = premultiply_channel(pixel[2], alpha);
    }
}

/// Decode a raster image (PNG, JPEG, GIF, WebP) into premultiplied BGRA8.
///
/// The source is premultiplied **before** scaling, so transparent pixels cannot
/// bleed their hidden colour into neighbouring edges. The output is `extent` by
/// `extent`; non-square sources are proportionally fitted and centred inside a
/// transparent square to match vector contain-fit behaviour.
pub fn decode_bytes(bytes: &[u8], extent: u32) -> Result<RasterImage, DecodeError> {
    let extent = clamp_extent(extent);
    let mut rgba = tessera_image::decode_memory(bytes)?;
    premultiply_rgba_in_place(&mut rgba);
    let fitted = fit_square_rgba(rgba, extent);
    Ok(premultiplied_rgba_to_bgra(fitted))
}

/// Fit a premultiplied RGBA image into an `extent × extent` square, preserving
/// aspect ratio with transparent letterboxing when the source is non-square.
fn fit_square_rgba(rgba: RgbaImage, extent: u32) -> RgbaImage {
    let (src_w, src_h) = rgba.dimensions();
    if src_w == extent && src_h == extent {
        return rgba;
    }
    if src_w == src_h {
        return tessera_image::resize(&rgba, extent, extent, Filter::Triangle);
    }
    if src_w == 0 || src_h == 0 {
        return RgbaImage::new(extent, extent);
    }
    let (dst_w, dst_h) = if src_w > src_h {
        let h = ((u64::from(src_h) * u64::from(extent) + u64::from(src_w) / 2) / u64::from(src_w))
            .clamp(1, u64::from(extent)) as u32;
        (extent, h)
    } else {
        let w = ((u64::from(src_w) * u64::from(extent) + u64::from(src_h) / 2) / u64::from(src_h))
            .clamp(1, u64::from(extent)) as u32;
        (w, extent)
    };
    let scaled = tessera_image::resize(&rgba, dst_w, dst_h, Filter::Triangle);
    let mut canvas = RgbaImage::new(extent, extent);
    let off_x = (extent - dst_w) / 2;
    let off_y = (extent - dst_h) / 2;
    let src_raw = scaled.as_raw();
    let dst_raw = canvas.as_mut_raw();
    let row_bytes = (dst_w as usize) * 4;
    for y in 0..dst_h as usize {
        let src_start = y * row_bytes;
        let dst_start = ((off_y as usize + y) * (extent as usize) + (off_x as usize)) * 4;
        if let (Some(src_row), Some(dst_row)) = (
            src_raw.get(src_start..src_start + row_bytes),
            dst_raw.get_mut(dst_start..dst_start + row_bytes),
        ) {
            dst_row.copy_from_slice(src_row);
        }
    }
    canvas
}

/// Decode a theme asset by its file extension.
///
/// Vector sources go through the shared bounded reader and [`VectorDocument`];
/// raster sources go through [`decode_bytes`]. Anything else is an explicit
/// [`DecodeError::Unsupported`], never a silent fallback.
pub fn decode_file(path: &Path, extent: u32) -> Result<RasterImage, DecodeError> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if VECTOR_EXTENSIONS.contains(&extension.as_str()) {
        VectorDocument::from_file(path)?.rasterize(extent)
    } else if RASTER_EXTENSIONS.contains(&extension.as_str()) {
        let bytes = std::fs::read(path).map_err(|source| DecodeError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        decode_bytes(&bytes, extent)
    } else {
        Err(DecodeError::Unsupported(extension))
    }
}

/// Whether an extension names a decoder in this crate.
pub fn supports_extension(extension: &str) -> bool {
    RASTER_EXTENSIONS
        .iter()
        .chain(VECTOR_EXTENSIONS)
        .any(|candidate| candidate.eq_ignore_ascii_case(extension))
}

fn clamp_extent(extent: u32) -> u32 {
    extent.clamp(1, MAX_EXTENT)
}

fn premultiply_channel(channel: u8, alpha: u8) -> u8 {
    ((u16::from(channel) * u16::from(alpha) + 127) / 255) as u8
}

fn premultiply_rgba_in_place(rgba: &mut RgbaImage) {
    for pixel in rgba.as_mut_raw().chunks_exact_mut(4) {
        let alpha = pixel[3];
        pixel[0] = premultiply_channel(pixel[0], alpha);
        pixel[1] = premultiply_channel(pixel[1], alpha);
        pixel[2] = premultiply_channel(pixel[2], alpha);
    }
}

fn premultiplied_rgba_to_bgra(rgba: RgbaImage) -> RasterImage {
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RasterImage {
        width,
        height,
        bgra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED_SVG: &[u8] =
        b"<svg viewBox=\"0 0 24 24\"><rect width=\"24\" height=\"24\" fill=\"#FF0000\"/></svg>";

    fn png(pixel: [u8; 4]) -> Vec<u8> {
        tessera_image::encode_png_rgba(1, 1, &pixel, false).expect("encode fixture")
    }

    #[test]
    fn a_vector_asset_is_premultiplied_bgra() {
        let document = VectorDocument::parse(RED_SVG).expect("red document parses");
        let raster = document.rasterize(24).expect("red document compiles");
        let centre = ((raster.height() as usize / 2) * raster.width() as usize
            + raster.width() as usize / 2)
            * 4;
        assert_eq!(
            &raster.as_bgra()[centre..centre + 4],
            &[0, 0, 255, 255],
            "BGRA of a red rectangle"
        );
    }

    #[test]
    fn hotspot_metadata_is_read_once_for_every_consumer() {
        let document = VectorDocument::parse(
            b"<svg viewBox=\"0 0 8 4\" data-hotspot-x=\"4\" data-hotspot-y=\"2\">\
              <rect width=\"8\" height=\"4\"/></svg>",
        )
        .expect("document parses");
        assert_eq!(document.hotspot(16), (8, 8));
        assert_eq!(document.normalized_hotspot(), (0.5, 0.5));
    }

    #[test]
    fn raster_alpha_is_premultiplied_at_the_boundary() {
        // Half-transparent red has to arrive as half-red, not full red with a
        // separate alpha: gfx samples premultiplied texels.
        let raster = decode_bytes(&png([255, 0, 0, 128]), 1).expect("png decodes");
        assert_eq!(raster.as_bgra(), &[0, 0, 128, 128]);
    }

    #[test]
    fn symbolic_recolour_tracks_coverage() {
        let mut raster = RasterImage::new(1, 1, vec![0, 0, 0, 128]).expect("pixel");
        raster.recolour_symbolic([246, 246, 248]);
        assert_eq!(raster.as_bgra(), &[124, 123, 123, 128]);
    }

    #[test]
    fn every_advertised_extension_has_a_decoder() {
        for extension in RASTER_EXTENSIONS.iter().chain(VECTOR_EXTENSIONS) {
            assert!(
                supports_extension(extension),
                "missing decoder: {extension}"
            );
        }
        assert!(!supports_extension("xpm"));
    }

    #[test]
    fn an_unknown_extension_is_a_refusal_not_a_fallback() {
        let path = std::path::Path::new("asset.xpm");
        assert!(matches!(
            decode_file(path, 24),
            Err(DecodeError::Unsupported(extension)) if extension == "xpm"
        ));
    }

    fn pixel_at(raster: &RasterImage, x: u32, y: u32) -> [u8; 4] {
        let off = ((y as usize) * (raster.width() as usize) + (x as usize)) * 4;
        let p = &raster.as_bgra()[off..off + 4];
        [p[0], p[1], p[2], p[3]]
    }

    #[test]
    fn svg_use_elements_render_all_rotated_instances() {
        // Regression for real-world icons like Tessera Settings whose gear teeth
        // are instantiated via `<use href="#tooth" transform="rotate(...)"/>`.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
            <g transform="translate(64 64)" fill="#ffffff">
                <g id="tooth"><rect x="-8" y="-56" width="16" height="20"/></g>
                <use href="#tooth" transform="rotate(90)"/>
                <use href="#tooth" transform="rotate(180)"/>
                <use href="#tooth" transform="rotate(270)"/>
            </g>
        </svg>"##;
        let raster = VectorDocument::parse(svg)
            .expect("svg with <use> parses")
            .rasterize(64)
            .expect("svg with <use> renders");
        // Top tooth (original), right (90°), bottom (180°), left (270°).
        assert_eq!(pixel_at(&raster, 32, 8), [255, 255, 255, 255]);
        assert_eq!(pixel_at(&raster, 55, 32), [255, 255, 255, 255]);
        assert_eq!(pixel_at(&raster, 32, 55), [255, 255, 255, 255]);
        assert_eq!(pixel_at(&raster, 8, 32), [255, 255, 255, 255]);
    }

    #[test]
    fn svg_css_style_classes_are_honoured() {
        // Breeze and Illustrator icons style shapes via `<style>` CSS classes.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <style>.accent { fill: #00ff00; }</style>
            <rect class="accent" width="24" height="24"/>
        </svg>"##;
        let raster = VectorDocument::parse(svg)
            .expect("styled svg parses")
            .rasterize(24)
            .expect("styled svg renders");
        assert_eq!(pixel_at(&raster, 12, 12), [0, 255, 0, 255]);
    }

    #[test]
    fn svg_clip_path_and_mask_confine_painted_pixels() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
            <defs>
                <clipPath id="left-half"><rect x="0" y="0" width="16" height="32"/></clipPath>
                <mask id="top-half"><rect x="0" y="0" width="32" height="16" fill="#ffffff"/></mask>
            </defs>
            <rect width="32" height="32" fill="#ff0000" clip-path="url(#left-half)" mask="url(#top-half)"/>
        </svg>"##;
        let raster = VectorDocument::parse(svg)
            .expect("clipped/masked svg parses")
            .rasterize(32)
            .expect("clipped/masked svg renders");
        // Top-left quadrant is red; all other quadrants must stay transparent.
        assert_eq!(pixel_at(&raster, 8, 8), [0, 0, 255, 255]);
        assert_eq!(pixel_at(&raster, 24, 8), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&raster, 8, 24), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&raster, 24, 24), [0, 0, 0, 0]);
    }

    #[test]
    fn svg_zero_opacity_and_display_none_stay_invisible() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <rect width="24" height="24" fill="#ff0000" opacity="0"/>
            <rect width="24" height="24" fill="#00ff00" display="none"/>
            <rect width="24" height="24" fill="#0000ff" visibility="hidden"/>
        </svg>"##;
        let raster = VectorDocument::parse(svg)
            .expect("invisible shapes svg parses")
            .rasterize(24)
            .expect("invisible shapes svg renders");
        assert_eq!(pixel_at(&raster, 12, 12), [0, 0, 0, 0]);
    }

    #[test]
    fn svg_rgba_percentage_gradient_and_filter_render_accurately() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
            <defs>
                <linearGradient id="g" x1="0%" y1="0%" x2="0%" y2="100%">
                    <stop offset="0%" stop-color="#ff0000"/>
                    <stop offset="100%" stop-color="#0000ff"/>
                </linearGradient>
                <filter id="blur"><feGaussianBlur stdDeviation="1"/></filter>
            </defs>
            <rect width="64" height="64" fill="url(#g)"/>
            <rect x="20" y="20" width="24" height="24" fill="rgba(0, 255, 0, 0.5)" filter="url(#blur)"/>
        </svg>"##;
        let raster = VectorDocument::parse(svg)
            .expect("gradient+rgba+filter svg parses")
            .rasterize(64)
            .expect("gradient+rgba+filter svg renders");
        // Top is predominantly red, bottom is predominantly blue (vertical gradient).
        let top = pixel_at(&raster, 8, 4);
        let bottom = pixel_at(&raster, 8, 60);
        assert!(
            top[2] > 200 && top[0] < 60,
            "top should be red, got {top:?}"
        );
        assert!(
            bottom[0] > 200 && bottom[2] < 60,
            "bottom should be blue, got {bottom:?}"
        );
        // Center has blended green from the rgba overlay.
        let center = pixel_at(&raster, 32, 32);
        assert!(
            center[1] > 90,
            "center should include green, got {center:?}"
        );
    }

    #[test]
    fn svg_current_color_defaults_to_white_and_yields_to_author_color() {
        let unresolved =
            br#"<svg viewBox="0 0 16 16"><rect width="16" height="16" fill="currentColor"/></svg>"#;
        let raster = VectorDocument::parse(unresolved)
            .expect("unresolved currentColor parses")
            .rasterize(16)
            .expect("unresolved currentColor renders");
        assert_eq!(pixel_at(&raster, 8, 8), [255, 255, 255, 255]);

        let author_attr = br##"<svg viewBox="0 0 16 16" color="#00ff00"><rect width="16" height="16" fill="currentColor"/></svg>"##;
        let raster = VectorDocument::parse(author_attr)
            .expect("author color parses")
            .rasterize(16)
            .expect("author color renders");
        assert_eq!(pixel_at(&raster, 8, 8), [0, 255, 0, 255]);

        let author_style_attr = br##"<svg viewBox="0 0 16 16" style="color: #0000ff"><rect width="16" height="16" fill="currentColor"/></svg>"##;
        let raster = VectorDocument::parse(author_style_attr)
            .expect("author inline style color parses")
            .rasterize(16)
            .expect("author inline style color renders");
        assert_eq!(pixel_at(&raster, 8, 8), [255, 0, 0, 255]);

        let author_stylesheet = br##"<svg viewBox="0 0 16 16"><style>svg { color: #ff0000; }</style><rect width="16" height="16" fill="currentColor"/></svg>"##;
        let raster = VectorDocument::parse(author_stylesheet)
            .expect("author stylesheet color parses")
            .rasterize(16)
            .expect("author stylesheet color renders");
        assert_eq!(pixel_at(&raster, 8, 8), [0, 0, 255, 255]);

        let child_group = br##"<svg viewBox="0 0 16 16"><g color="#00ff00"><rect width="16" height="16" fill="currentColor"/></g></svg>"##;
        let raster = VectorDocument::parse(child_group)
            .expect("child group color parses")
            .rasterize(16)
            .expect("child group color renders");
        assert_eq!(pixel_at(&raster, 8, 8), [0, 255, 0, 255]);
    }

    #[test]
    fn non_square_raster_preserves_aspect_ratio_inside_square_cell() {
        // 4x2 solid red PNG fitted into 16x16 must occupy rows 4..12 and leave
        // top/bottom letterbox margins transparent.
        let mut pixels = Vec::with_capacity(4 * 2 * 4);
        for _ in 0..8 {
            pixels.extend_from_slice(&[255, 0, 0, 255]);
        }
        let encoded = tessera_image::encode_png_rgba(4, 2, &pixels, false).expect("encode 4x2");
        let raster = decode_bytes(&encoded, 16).expect("decode 4x2");
        assert_eq!((raster.width(), raster.height()), (16, 16));
        assert_eq!(pixel_at(&raster, 8, 0), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&raster, 8, 8), [0, 0, 255, 255]);
        assert_eq!(pixel_at(&raster, 8, 15), [0, 0, 0, 0]);
    }

    #[test]
    fn malformed_or_empty_svg_is_rejected() {
        assert!(matches!(
            VectorDocument::parse(b""),
            Err(DecodeError::Vector(_))
        ));
        assert!(matches!(
            VectorDocument::parse(b"not svg at all"),
            Err(DecodeError::Vector(_))
        ));
        assert!(matches!(
            VectorDocument::parse(b"<svg =x></svg>"),
            Err(DecodeError::Vector(_))
        ));
    }

    #[test]
    fn external_image_references_are_blocked_by_memory_loading() {
        // `[INV-ASSET-10]`: memory loading sets no base URI, so external file
        // references inside untrusted SVGs cannot read host files.
        let path =
            std::env::temp_dir().join(format!("tessera-assets-ext-ref-{}.png", std::process::id()));
        std::fs::write(&path, png([255, 0, 0, 255])).expect("write fixture png");
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
                <image href="file://{}" width="16" height="16"/>
            </svg>"#,
            path.display()
        );
        let raster = VectorDocument::parse(svg.as_bytes())
            .expect("svg with external image parses")
            .rasterize(16)
            .expect("svg with external image renders");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            pixel_at(&raster, 8, 8),
            [0, 0, 0, 0],
            "external file:// image must not be fetched"
        );
    }

    #[test]
    fn xml_prolog_bom_doctype_and_single_quoted_hotspots_parse() {
        let mut svg = Vec::from(&b"\xEF\xBB\xBF"[..]);
        svg.extend_from_slice(
            br##"<?xml version="1.0" encoding="UTF-8"?>
            <!-- leading comment -->
            <!DOCTYPE svg [ <!ENTITY unused "x"> ]>
            <svg viewBox='0 0 16 16' data-hotspot-x='4' data-hotspot-y='12'>
                <rect width='16' height='16' fill='#00ff00'/>
            </svg>"##,
        );
        let doc = VectorDocument::parse(&svg).expect("prolog + bom + doctype svg parses");
        assert_eq!(doc.hotspot(32), (8, 24));
        assert_eq!(doc.normalized_hotspot(), (0.25, 0.75));
        let raster = doc.rasterize(16).expect("prolog svg renders");
        assert_eq!(pixel_at(&raster, 8, 8), [0, 255, 0, 255]);
    }

    #[test]
    fn breeze_style_color_scheme_and_physical_units_without_viewbox() {
        // Real KDE/Breeze pattern: `.ColorScheme-Text` in `<style>` + `currentColor`.
        let breeze = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
            <style type="text/css" id="current-color-scheme">
                .ColorScheme-Text { color:#232629; }
            </style>
            <rect class="ColorScheme-Text" width="24" height="24" style="fill:currentColor"/>
        </svg>"##;
        let raster = VectorDocument::parse(breeze)
            .expect("breeze svg parses")
            .rasterize(24)
            .expect("breeze svg renders");
        // BGRA of #232629 => [0x29, 0x26, 0x23, 0xff]
        assert_eq!(pixel_at(&raster, 12, 12), [0x29, 0x26, 0x23, 0xff]);

        // No viewBox, physical `pt` units at 96 DPI: 24pt == 32px.
        let pt_svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24pt" height="24pt">
            <rect width="32" height="32" fill="#0000ff"/>
        </svg>"##;
        let doc = VectorDocument::parse(pt_svg).expect("pt svg parses");
        assert_eq!(doc.viewport(), [0.0, 0.0, 32.0, 32.0]);
        let raster = doc.rasterize(16).expect("pt svg renders");
        assert_eq!(pixel_at(&raster, 8, 8), [255, 0, 0, 255]);
    }

    #[test]
    fn hotspot_matches_contain_fit_for_non_square_and_offset_viewboxes() {
        // Non-square 8x4 viewBox fitted into 16x16 occupies y = 4..12, so
        // viewBox (0, 0) lands at pixel (0, 4), normalized (0.0, 0.25).
        let wide = VectorDocument::parse(
            b"<svg viewBox=\"0 0 8 4\" data-hotspot-x=\"0\" data-hotspot-y=\"0\">\
              <rect width=\"8\" height=\"4\"/></svg>",
        )
        .expect("wide svg parses");
        assert_eq!(wide.hotspot(16), (0, 4));
        assert_eq!(wide.normalized_hotspot(), (0.0, 0.25));

        // Non-zero viewBox origin: (-10, -10, 20, 20) puts (0, 0) at the centre.
        let offset = VectorDocument::parse(
            b"<svg viewBox=\"-10 -10 20 20\" data-hotspot-x=\"0\" data-hotspot-y=\"0\">\
              <rect x=\"-10\" y=\"-10\" width=\"20\" height=\"20\"/></svg>",
        )
        .expect("offset viewbox svg parses");
        assert_eq!(offset.hotspot(16), (8, 8));
        assert_eq!(offset.normalized_hotspot(), (0.5, 0.5));
    }

    #[test]
    fn concurrent_multithreaded_svg_rasterization_is_sound() {
        let cases: &[(&[u8], [u8; 4])] = &[
            (
                br##"<svg viewBox="0 0 16 16"><rect width="16" height="16" fill="#ff0000"/></svg>"##,
                [0, 0, 255, 255],
            ),
            (
                br##"<svg viewBox="0 0 16 16"><rect width="16" height="16" fill="#00ff00"/></svg>"##,
                [0, 255, 0, 255],
            ),
            (
                br##"<svg viewBox="0 0 16 16"><rect width="16" height="16" fill="#0000ff"/></svg>"##,
                [255, 0, 0, 255],
            ),
            (
                br##"<svg viewBox="0 0 16 16"><style>.c{fill:#ffff00}</style><rect class="c" width="16" height="16"/></svg>"##,
                [0, 255, 255, 255],
            ),
        ];

        let handles: Vec<_> = cases
            .iter()
            .map(|&(svg, expected)| {
                std::thread::spawn(move || {
                    for _ in 0..8 {
                        let raster = VectorDocument::parse(svg)
                            .expect("thread parse")
                            .rasterize(32)
                            .expect("thread rasterize");
                        assert_eq!(pixel_at(&raster, 16, 16), expected);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("worker thread panicked");
        }
    }

    #[test]
    fn decode_file_handles_svg_and_svgz_on_disk() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("tessera-assets-disk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let svg_path = dir.join("sample.svg");
        let svgz_path = dir.join("sample.svgz");
        std::fs::write(&svg_path, RED_SVG).expect("write svg");

        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(RED_SVG).expect("compress svgz");
        std::fs::write(&svgz_path, enc.finish().expect("finish svgz")).expect("write svgz");

        let from_svg = decode_file(&svg_path, 24).expect("decode .svg");
        let from_svgz = decode_file(&svgz_path, 24).expect("decode .svgz");
        assert_eq!(pixel_at(&from_svg, 12, 12), [0, 0, 255, 255]);
        assert_eq!(pixel_at(&from_svgz, 12, 12), [0, 0, 255, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
