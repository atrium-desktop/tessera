//! Theme asset decoding for the whole desktop (ADR-0233).
//!
//! Two questions must have exactly one answer in this stack:
//!
//! 1. **How are a theme asset's bytes read?** SVG text and SVGZ (SVG under
//!    gzip) are the two carriers of a vector document; [`vector`] reads both,
//!    with a bounded decompression step because a theme directory is untrusted
//!    input.
//! 2. **What layout does the renderer receive?** Every raster and every vector
//!    document is decoded into **premultiplied BGRA8**, the layout flux samples
//!    through `Bgra8Unorm`. Callers never swap channels, premultiply, resize in
//!    the wrong alpha space, or guess a convention: those decisions exist here
//!    exactly once.
//!
//! The crate deliberately has no flux/lens edge. It owns bytes and pixels, not
//! GPU resources, so the compositor, the shell, and any future surface decode
//! an asset the same way without sharing a renderer.

pub mod vector;

pub use vector::{SourceError, decompress_if_gzip, read_vector_source};

use std::path::{Path, PathBuf};

use image::{RgbaImage, imageops::FilterType};
use thiserror::Error;

/// Raster formats the `image` decoder is compiled to handle.
///
/// This is the complete raster vocabulary: an extension outside this list is not
/// advertised to a caller and [`decode_file`] refuses it. XPM is deliberately
/// absent — the stack has no XPM decoder, and resolving a name to a file that
/// cannot be painted is a silent wrong answer (ADR-0222, `[INV-ASSET-06]`).
pub const RASTER_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Vector formats the reticle compiler consumes.
pub const VECTOR_EXTENSIONS: &[&str] = &["svg", "svgz"];

/// Hard ceiling on a decoded raster edge, independent of the caller's policy.
///
/// Callers keep their own tighter budgets (application icons cap at 512, cursor
/// sprites at 256); this one stops an accidental request from allocating an
/// unbounded texture.
pub const MAX_EXTENT: u32 = 4096;

/// Why an asset could not be decoded.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// The file extension names no decoder this crate implements.
    #[error("unsupported theme asset extension {0:?}")]
    Unsupported(String),
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The raster decoder rejected the bytes.
    #[error(transparent)]
    Raster(#[from] image::ImageError),
    /// The vector source could not be read or decompressed.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The vector compiler rejected the document.
    #[error("vector document rejected: {0}")]
    Vector(#[from] reticle::Error),
    /// A buffer's extent does not match its pixel byte length.
    #[error("raster extent {width}x{height} does not match {len} pixel bytes")]
    Extent { width: u32, height: u32, len: usize },
}

/// One decoded asset in the renderer's native CPU layout.
///
/// `bgra` is `width * height * 4` bytes of **premultiplied** blue, green, red,
/// alpha. It is ready for `flux::Image::from_bytes(..., Bgra8Unorm, ...)` with
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

/// A parsed SVG/SVGZ document.
///
/// This is the only type in the desktop that owns a reticle document. Consumers
/// ask it to rasterize or to report its stamped hotspot; they never import the
/// compiler themselves, which is what makes "one asset rasterizer" a dependency
/// boundary rather than a convention (ADR-0233).
pub struct VectorDocument {
    inner: reticle::Document,
}

impl VectorDocument {
    /// Parse a vector source. SVGZ must already have been decompressed by
    /// [`read_vector_source`] or [`decompress_if_gzip`].
    pub fn parse(bytes: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            inner: reticle::Document::parse_bytes(bytes, reticle::Limits::default())?,
        })
    }

    /// Read and parse a vector file, transparently gunzipping `.svgz`.
    pub fn from_file(path: &Path) -> Result<Self, DecodeError> {
        Self::parse(&read_vector_source(path)?)
    }

    /// Compile the document into premultiplied BGRA8 at `extent` pixels square.
    pub fn rasterize(&self, extent: u32) -> Result<RasterImage, DecodeError> {
        rasterize_document(&self.inner, extent)
    }

    /// The document's viewport as `[x, y, width, height]` in user units.
    pub fn viewport(&self) -> [f32; 4] {
        self.inner.viewport()
    }

    /// A stamped hotspot scaled to `extent` pixels, clamped inside the sprite.
    ///
    /// A theme that stamps nothing gets the top-left corner, the XDG convention
    /// for "no hotspot data".
    pub fn hotspot(&self, extent: u32) -> (u32, u32) {
        let viewport = self.viewport();
        let scale_x = if viewport[2] > 0.0 {
            extent as f32 / viewport[2]
        } else {
            1.0
        };
        let scale_y = if viewport[3] > 0.0 {
            extent as f32 / viewport[3]
        } else {
            1.0
        };
        let axis = |value: Option<f32>, scale: f32| -> u32 {
            let pixels = value.unwrap_or(0.0) * scale;
            (pixels.round().max(0.0) as u32).min(extent.saturating_sub(1))
        };
        (
            axis(self.hotspot_axis("data-hotspot-x"), scale_x),
            axis(self.hotspot_axis("data-hotspot-y"), scale_y),
        )
    }

    /// A stamped hotspot as a fraction of the viewport in `0.0..=1.0`.
    pub fn normalized_hotspot(&self) -> (f32, f32) {
        let viewport = self.viewport();
        let axis = |value: Option<f32>, extent: f32| -> f32 {
            if extent > 0.0 {
                (value.unwrap_or(0.0) / extent).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        (
            axis(self.hotspot_axis("data-hotspot-x"), viewport[2]),
            axis(self.hotspot_axis("data-hotspot-y"), viewport[3]),
        )
    }

    fn hotspot_axis(&self, name: &str) -> Option<f32> {
        self.inner.attribute(name)?.trim().parse().ok()
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
/// bleed their hidden colour into neighbouring edges. The result is exactly
/// `extent` by `extent` (aspect is not preserved; icon cells are square).
pub fn decode_bytes(bytes: &[u8], extent: u32) -> Result<RasterImage, DecodeError> {
    let extent = clamp_extent(extent);
    let mut rgba = image::load_from_memory(bytes)?.to_rgba8();
    premultiply_rgba_in_place(&mut rgba);
    let scaled = if rgba.width() == extent && rgba.height() == extent {
        rgba
    } else {
        image::imageops::resize(&rgba, extent, extent, FilterType::Triangle)
    };
    Ok(premultiplied_rgba_to_bgra(scaled))
}

/// Compile a vector document into premultiplied BGRA8 at `extent` pixels square.
///
/// `PaintMode::theme` keeps a theme's own colours while resolving `currentColor`
/// to opaque white, so a document that paints only through `currentColor` still
/// renders instead of vanishing. Documents keep their aspect ratio and are
/// fitted into the square, centred, by the compiler.
fn rasterize_document(
    document: &reticle::Document,
    extent: u32,
) -> Result<RasterImage, DecodeError> {
    let extent = clamp_extent(extent);
    let pixmap = document.rasterize(reticle::RasterizeOptions {
        width: extent,
        height: extent,
        supersample: 0,
        format: reticle::PixelFormat::Bgra8Premul,
        paint: reticle::PaintMode::theme(0xFFFF_FFFF),
        stroke_width: 0.0,
    })?;
    raster_from_pixmap(&pixmap)
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
    for pixel in rgba.pixels_mut() {
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

fn raster_from_pixmap(pixmap: &reticle::Pixmap) -> Result<RasterImage, DecodeError> {
    let width = pixmap.width();
    let height = pixmap.height();
    let row_bytes = (width as usize).checked_mul(4).ok_or(DecodeError::Extent {
        width,
        height,
        len: pixmap.as_bytes().len(),
    })?;
    let stride = pixmap.stride() as usize;
    if stride < row_bytes {
        return Err(DecodeError::Extent {
            width,
            height,
            len: pixmap.as_bytes().len(),
        });
    }
    if stride == row_bytes {
        return RasterImage::new(width, height, pixmap.as_bytes().to_vec());
    }
    let expected = row_bytes
        .checked_mul(height as usize)
        .ok_or(DecodeError::Extent {
            width,
            height,
            len: pixmap.as_bytes().len(),
        })?;
    let mut bgra = Vec::with_capacity(expected);
    for row in 0..height as usize {
        let start = row * stride;
        let end = start + row_bytes;
        let Some(bytes) = pixmap.as_bytes().get(start..end) else {
            return Err(DecodeError::Extent {
                width,
                height,
                len: pixmap.as_bytes().len(),
            });
        };
        bgra.extend_from_slice(bytes);
    }
    RasterImage::new(width, height, bgra)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const RED_SVG: &[u8] =
        b"<svg viewBox=\"0 0 24 24\"><rect width=\"24\" height=\"24\" fill=\"#FF0000\"/></svg>";

    fn png(pixel: image::Rgba<u8>) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(1, 1, pixel)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("encode fixture");
        bytes
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
        // separate alpha: flux samples premultiplied texels.
        let raster = decode_bytes(&png(image::Rgba([255, 0, 0, 128])), 1).expect("png decodes");
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
}
