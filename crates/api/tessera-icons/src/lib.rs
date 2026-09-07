//! freedesktop.org icon-theme lookup for tessera.
//!
//! Implements the lookup half of the
//! [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/):
//!
//! - Scale-aware resolution from `index.theme` directory metadata.
//! - Recursive theme inheritance with `hicolor` as the mandatory fallback.
//! - Unthemed fallbacks such as `/usr/share/pixmaps/<name>.png`.
//! - XDG base-directory search bases (`~/.icons`, `<data>/icons`, pixmaps).
//!
//! The crate is dependency-minimal (INI parsing only) and side-effect free
//! apart from filesystem reads. It exists so any consumer — application
//! launchers, the SNI tray, portrait assets — can resolve icons without
//! depending on desktop-entry parsing (see `tessera-desktop-entries`).

mod icon;
mod xdg;

pub use icon::{resolve_icon, resolve_icon_scaled};
pub use xdg::{icon_search_bases, xdg_data_dirs};

/// Default requested icon size. Large enough for a 72 logical-pixel launcher
/// icon on HiDPI outputs without visibly upscaling a 48-pixel source.
pub const DEFAULT_ICON_SIZE: u32 = 128;

/// Default icon theme used when the caller passes none. `hicolor` is the
/// spec-mandated final fallback for every theme chain.
pub const DEFAULT_ICON_THEME: &str = "hicolor";
