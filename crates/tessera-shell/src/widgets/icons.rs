//! Runtime-registered product glyphs for shell chrome.
//!
//! Lens ships a fixed built-in icon table (`lens::sys::lens_icon_id`) that has
//! no keyboard-hardware vocabulary; the closest match for a keyboard backlight
//! was `LENS_ICON_EDIT` (a pencil in a box), which reads as "edit", not
//! "illumination". `lens::register_svg_icon` lets a host register its own SVG
//! at runtime and then draw it through every icon widget exactly like a
//! built-in (`lens_icon_register_svg`, Optics `libs/lens/src/icon_runtime.c`).
//!
//! These glyphs are registered lazily on the UI thread, once per process. The
//! lens registry is process-global and never reclaimed, so the ids are stable
//! for the life of the process; [`keyboard_backlight`] caches the id in a
//! `OnceLock` and returns `None` if registration ever fails, letting callers
//! fall back to a built-in rather than drawing nothing.

use std::sync::OnceLock;

use lens::sys::lens_icon_id;

/// Keyboard outline with three illumination rays above it — the keyboard
/// backlight glyph. Drawn in the built-in icon set's 2/24 stroke weight; the
/// lens SVG parser ignores the document's own paint and draws in the theme
/// colour, so this is stroke-only geometry with no `fill`.
const KEYBOARD_BACKLIGHT_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
<path d="M3 14h18v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z"/>
<path d="M6 17.5h1"/>
<path d="M9.5 17.5h5"/>
<path d="M17 17.5h1"/>
<path d="M7 4v3"/>
<path d="M12 3v4"/>
<path d="M17 4v3"/>
</svg>"##;

static KEYBOARD_BACKLIGHT: OnceLock<Option<lens_icon_id>> = OnceLock::new();

/// The keyboard-backlight icon id, registering the glyph on first use.
///
/// Returns `None` if the SVG was rejected by the lens parser; callers should
/// then fall back to a built-in icon id.
pub fn keyboard_backlight() -> Option<lens_icon_id> {
    *KEYBOARD_BACKLIGHT.get_or_init(|| lens::register_svg_icon(KEYBOARD_BACKLIGHT_SVG))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_backlight_registers_and_is_stable() {
        // Registration needs a live lens context only for *drawing*; the
        // registry itself is process-global, so a headless frame is enough to
        // prove the SVG parses and the id is reusable.
        let first = keyboard_backlight();
        assert!(first.is_some(), "keyboard backlight SVG must parse");
        assert_eq!(first, keyboard_backlight(), "id is cached across calls");
        // Runtime ids continue past the built-in table.
        assert!(first.unwrap().0 >= lens_icon_id::LENS_ICON_COUNT.0);
    }
}
