//! Theme-icon resolution for SNI items, run on the worker threads.
//!
//! An `IconName` arrives as a theme lookup plus a decode (raster in-process, SVG
//! compiled in process by the Optics reticle compiler) — far too slow for the
//! render thread. The watcher resolves names into BGRA8 pixmaps before publishing
//! them in the shared snapshot, so the shell only ever uploads ready pixels. The
//! resolution rules mirror the compositor's app-icon path
//! (`tessera::runtime::apps`).
//!
//! This is deliberately the *same* decoder shape as the compositor's, and it links
//! the same library: one SVG implementation for the whole desktop is the point of
//! `reticle` being a shared library (ADR-0150, Optics ADR-0161) rather than a
//! per-consumer parser.

use std::path::Path;

use super::TrayPixmap;

/// Scale theme-resolved SNI icons are looked up and rasterized at; the texture is
/// sampled down to the 18px cell glyph, so 2x keeps HiDPI crisp.
const TRAY_ICON_SCALE: u32 = 2;

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ is compiled by
/// the Optics reticle compiler; no subprocess and no host rasterizer is involved.
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];

/// Resolve an SNI `IconName` through the freedesktop icon theme and decode it into
/// unpremultiplied BGRA8 (the format flux samples), ready for GPU upload. Returns
/// `None` when the name is unknown or the file undecodable; the caller memoizes
/// the failure so the theme is not rescanned.
pub(super) fn resolve_theme_icon(name: &str) -> Option<TrayPixmap> {
    let path = tessera_launch_services::resolve_icon_scaled(name, None, &[], 24, TRAY_ICON_SCALE)?;
    decode_theme_pixmap(&path, TRAY_ICON_SCALE)
}

/// Decode one resolved theme path into the snapshot's BGRA8 pixmap.
///
/// Split from the lookup above so the decode policy — in-process decode, the
/// symbolic recolour, and the RGBA→BGRA swap — has exactly one implementation and
/// can be exercised against a file the test controls rather than a name the host
/// theme happens to carry.
fn decode_theme_pixmap(path: &Path, scale: u32) -> Option<TrayPixmap> {
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mut rgba = decode_icon(path, &ext, scale)?.to_rgba8();
    if is_symbolic_icon(path) {
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. Apply the shell's light foreground while preserving
        // every coverage value from SVG antialiasing (same treatment as the
        // compositor's HUD symbols).
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
    }
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for chunk in bgra.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    Some(TrayPixmap {
        width,
        height,
        bgra,
    })
}

/// Decode a resolved theme icon into RGBA8 pixels.
///
/// Raster formats are decoded in process. SVG is compiled at the size the theme
/// asked for — the output is pinned to `DEFAULT_ICON_SIZE * scale`, capped — so a
/// malformed or enormous vector source cannot dictate an unbounded GPU texture.
fn decode_icon(path: &Path, ext: &str, scale: u32) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = tessera_launch_services::DEFAULT_ICON_SIZE
        .saturating_mul(scale.max(1))
        .clamp(1, 512);
    let source = tessera_launch_services::read_vector_source(path).ok()?;
    let document = match reticle::Document::parse_bytes(&source, reticle::Limits::default()) {
        Ok(document) => document,
        Err(error) => {
            log::debug!("tray: SVG parse failed for {}: {error}", path.display());
            return None;
        }
    };
    let raster = document
        .rasterize(reticle::RasterizeOptions {
            width: target,
            height: target,
            supersample: 0,
            format: reticle::PixelFormat::Rgba8Premul,
            paint: reticle::PaintMode::theme(0xFFFF_FFFF),
            stroke_width: 0.0,
        })
        .ok()?;
    let mut pixels = raster.as_bytes().to_vec();
    // flux samples premultiplied BGRA8; the compiler produced premultiplied RGBA8.
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    image::RgbaImage::from_raw(raster.width(), raster.height(), pixels)
        .map(image::DynamicImage::ImageRgba8)
}

/// Symbolic icons (name ends in `-symbolic`) are the only theme icons the shell
/// recolours; regular SNI icons are full-color application artwork.
fn is_symbolic_icon(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("-symbolic"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbolic_icon_detection_uses_the_file_stem() {
        assert!(is_symbolic_icon(Path::new(
            "/usr/share/icons/Adwaita/symbolic/apps/foo-symbolic.svg"
        )));
        assert!(!is_symbolic_icon(Path::new(
            "/usr/share/icons/hicolor/48x48/apps/foo.png"
        )));
        // "symbolic" appearing elsewhere in the name does not count.
        assert!(!is_symbolic_icon(Path::new("symbolic.png")));
        assert!(!is_symbolic_icon(Path::new("foo-symbolicx.svg")));
    }

    #[test]
    fn an_svgz_icon_is_decompressed_before_compiling() {
        // `.svgz` is in the extension list this module advertises; without the
        // shared decompression step the compiler would be handed raw gzip and
        // every compressed theme icon would silently vanish from the panel.
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("tessera-tray-svgz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.svgz");
        let svg = b"<svg viewBox=\"0 0 24 24\"><rect width=\"24\" height=\"24\" \
                    fill=\"#00FF00\"/></svg>";
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(svg).unwrap();
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();

        let pixmap = decode_theme_pixmap(&path, 2).expect("svgz icon compiles");
        assert_eq!(pixmap.width, pixmap.height);
        let center =
            ((pixmap.height as usize / 2) * pixmap.width as usize + pixmap.width as usize / 2) * 4;
        // BGRA of the green rectangle: blue=0, green=255, red=0.
        assert!(pixmap.bgra[center + 1] > 200, "the svgz document painted");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_svg_icon_compiles_without_a_subprocess() {
        // The defect this replaces: panel icons forked `rsvg-convert`, so a host
        // without librsvg silently lost every symbolic tray icon. A document now
        // compiles in process, at the pinned size, with no external program.
        let dir = std::env::temp_dir().join(format!("tessera-tray-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-symbolic.svg");
        std::fs::write(
            &path,
            b"<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"9\" fill=\"#000000\"/>\
              </svg>",
        )
        .unwrap();

        let pixmap = decode_theme_pixmap(&path, 2).expect("svg icon compiles");
        assert_eq!(pixmap.width, pixmap.height);
        assert_eq!(
            pixmap.bgra.len(),
            (pixmap.width * pixmap.height * 4) as usize
        );
        // Symbolic recolouring ran: every covered pixel is the shell foreground,
        // and coverage itself survived.
        let center =
            ((pixmap.height as usize / 2) * pixmap.width as usize + pixmap.width as usize / 2) * 4;
        assert_eq!(pixmap.bgra[center + 3], 255, "the circle is opaque");
        assert_eq!(
            &pixmap.bgra[center..center + 3],
            &[248, 246, 246],
            "BGRA of the symbolic foreground"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
