//! XDG application discovery for tessera.
//!
//! Implements the freedesktop.org
//! [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/):
//! scan `applications/*.desktop` under `XDG_DATA_HOME` and `XDG_DATA_DIRS`,
//! parse each entry, pick the best locale, resolve the icon through
//! [`tessera_icons`] (the shared icon-theme lookup), and strip
//! `Exec` field codes.
//!
//! The crate has no flux, lens, or Wayland dependency. Per the project's
//! placement rules (see `docs/dev/project-layout.md`), freedesktop/OS
//! integration that does not need the compositor types lives in its own crate
//! rather than in `tessera-shell`. The pure [`Entry`] model lives in
//! [`tessera_model::app`] so the shell chrome can render it without depending on
//! this crate; see ADR-0022.
//!
//! Scope:
//! - Flat enumeration of visible `Type=Application` entries, including
//!   exported file symlinks (no nested menu tree).
//! - Icon resolution delegates to [`tessera_icons`]: scale-aware lookup from
//!   `index.theme` directory metadata, recursive inheritance, `hicolor`, and
//!   unthemed fallbacks.
//! - `Exec` field codes are stripped; the result is tokenized for direct
//!   spawning in [`crate::expand_exec_tokens`] and shell-quoted for `sh -c`
//!   in [`crate::expand_exec`].

use std::path::PathBuf;

mod exec;
mod locale;
mod scan;

pub use exec::{expand_exec, expand_exec_tokens};
pub use locale::current_locale;
pub use scan::{
    enumerate_in, enumerate_in_with_theme, enumerate_in_with_theme_and_scale, parse_str,
};
/// Re-export of the shared entry model. Built here, read by `tessera-shell` and
/// `tessera-launch` without them taking a dependency on this crate's parser.
pub use tessera_model::app::Entry;
// Icon lookup is owned by `tessera-icons`; re-exported so existing
// `tessera_desktop_entries::resolve_icon*` callers keep one import site.
pub use tessera_icons::{
    DEFAULT_ICON_SIZE, DEFAULT_ICON_THEME, icon_search_bases, resolve_icon, resolve_icon_scaled,
    xdg_data_dirs,
};

/// Errors returned by application discovery.
#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse {0}: {1}")]
    Parse(PathBuf, String),
}

/// Enumerate all user-launchable applications found on the host.
///
/// Scans `$XDG_DATA_HOME/applications` and every `$XDG_DATA_DIRS` entry's
/// `applications/` subdirectory, in that order. The first file with a given
/// desktop id wins (user overrides system), matching the lookup precedence of
/// the desktop-entry spec. Entries with `Type != Application`, visibility
/// exclusions, a missing `Exec`, or an unresolvable `TryExec` are dropped.
pub fn enumerate() -> Vec<Entry> {
    let dirs = xdg_data_dirs();
    enumerate_in(&dirs)
}

/// Enumerate all user-launchable applications using an explicit icon theme.
pub fn enumerate_with_theme(icon_theme: &str) -> Vec<Entry> {
    let dirs = tessera_icons::xdg_data_dirs();
    enumerate_in_with_theme(&dirs, icon_theme)
}

/// Enumerate applications for an explicit icon theme and output scale.
pub fn enumerate_with_theme_and_scale(icon_theme: &str, icon_scale: u32) -> Vec<Entry> {
    let dirs = tessera_icons::xdg_data_dirs();
    enumerate_in_with_theme_and_scale(&dirs, icon_theme, icon_scale)
}
