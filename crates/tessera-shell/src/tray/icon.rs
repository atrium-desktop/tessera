//! Theme-icon resolution for SNI items, run on the worker threads.
//!
//! An `IconName` arrives as a theme lookup plus a decode (raster in-process, SVG
//! compiled in process by the Optics reticle compiler) — far too slow for the
//! render thread. The watcher resolves names into premultiplied BGRA8 pixmaps
//! before publishing them in the shared snapshot, so the shell only ever uploads
//! ready pixels (ADR-0233).
//!
//! The decode itself lives in `tessera-assets`: the compositor's app-icon path
//! and the tray path cannot drift into different channel orders or alpha
//! conventions because they call the same function.

use std::path::Path;

use super::TrayPixmap;

/// Scale theme-resolved SNI icons are looked up and rasterized at; the texture is
/// sampled down to the 18px cell glyph, so 2x keeps HiDPI crisp.
const TRAY_ICON_SCALE: u32 = 2;

/// Resolve an SNI `IconName` through the freedesktop icon theme and decode it into
/// premultiplied BGRA8, ready for GPU upload. Returns `None` when the name is
/// unknown or the file undecodable; the caller memoizes the failure so the theme
/// is not rescanned.
pub(super) fn resolve_theme_icon(name: &str) -> Option<TrayPixmap> {
    let path = tessera_launch_services::resolve_icon_scaled(name, None, &[], 24, TRAY_ICON_SCALE)?;
    decode_theme_pixmap(&path, TRAY_ICON_SCALE)
}

/// Decode one resolved theme path into the snapshot's pixmap.
///
/// Split from the lookup above so the shell's *policy* — the pinned extent and
/// the symbolic recolour — is exercised against a file the test controls rather
/// than a name the host theme happens to carry. The *mechanism* is shared.
fn decode_theme_pixmap(path: &Path, scale: u32) -> Option<TrayPixmap> {
    let extent = tessera_launch_services::DEFAULT_ICON_SIZE
        .saturating_mul(scale.max(1))
        .clamp(1, 512);
    let mut raster = tessera_assets::decode_file(path, extent)
        .inspect_err(|error| log::debug!("tray: {} did not decode: {error}", path.display()))
        .ok()?;
    if is_symbolic_icon(path) {
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. Apply the shell's light foreground while
        // preserving every coverage value (same treatment as the compositor's
        // HUD symbols).
        raster.recolour_symbolic([246, 246, 248]);
    }
    Some(TrayPixmap {
        width: raster.width(),
        height: raster.height(),
        bgra: raster.into_bgra(),
    })
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
    fn a_full_colour_svg_keeps_red_and_blue_in_their_channels() {
        // Regression: the pre-ADR-0233 tray decoder swapped RGBA→BGRA and the
        // upload path swapped again, so every full-colour SVG icon arrived with
        // red and blue exchanged. A green-only fixture cannot see that; a red
        // one can.
        let dir = std::env::temp_dir().join(format!("tessera-tray-rgb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test-red.svg");
        std::fs::write(
            &path,
            b"<svg viewBox=\"0 0 24 24\"><rect width=\"24\" height=\"24\" \
              fill=\"#FF0000\"/></svg>",
        )
        .unwrap();

        let pixmap = decode_theme_pixmap(&path, 2).expect("svg icon compiles");
        let center =
            ((pixmap.height as usize / 2) * pixmap.width as usize + pixmap.width as usize / 2) * 4;
        assert_eq!(
            &pixmap.bgra[center..center + 4],
            &[0, 0, 255, 255],
            "BGRA of a red rectangle"
        );

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
