//! Product glyphs for shell chrome.
//!
//! The glyph geometry and its per-context registration live in
//! [`tessera_design::icons`] (Optics v0.0.88 bundles no icon pack). These
//! helpers keep the shell's historical call shape: they return the registered
//! id, or `None` before the glyph set was installed on the shell's context so
//! callers can fall back to a built-in.

use optics_ui::sys::optics_ui_icon_id;
use tessera_design::icons::Glyph;

fn registered(glyph: Glyph) -> Option<optics_ui_icon_id> {
    let id = glyph.id();
    (id != optics_ui_icon_id::OPTICS_UI_ICON_NONE).then_some(id)
}

/// The keyboard-backlight glyph id.
pub fn keyboard_backlight() -> Option<optics_ui_icon_id> {
    registered(Glyph::KeyboardBacklight)
}

/// The Bluetooth radio glyph id: the rune when `enabled`, struck through
/// when not (ADR-0175).
pub fn bluetooth(enabled: bool) -> Option<optics_ui_icon_id> {
    registered(Glyph::bluetooth(enabled))
}
