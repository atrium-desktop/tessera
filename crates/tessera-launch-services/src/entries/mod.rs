//! XDG application discovery for tessera.
//!
//! Implements the freedesktop.org
//! [Desktop Entry Specification](https://specifications.freedesktop.org/desktop-entry-spec/):
//! scan `applications/*.desktop` under `XDG_DATA_HOME` and `XDG_DATA_DIRS`,
//! parse each entry, pick the best locale, resolve the icon through
//! [`crate::icons`], and strip `Exec` field codes.

use std::path::PathBuf;

pub mod exec;
pub mod locale;
pub mod scan;

pub use exec::{expand_exec, expand_exec_tokens};
pub use locale::current_locale;
pub use scan::{
    enumerate_in, enumerate_in_with_theme, enumerate_in_with_theme_and_scale, parse_str,
};
pub use tessera_desktop::app::Entry;

use crate::icons::xdg_data_dirs;

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
    let dirs = xdg_data_dirs();
    enumerate_in_with_theme(&dirs, icon_theme)
}

/// Enumerate applications for an explicit icon theme and output scale.
pub fn enumerate_with_theme_and_scale(icon_theme: &str, icon_scale: u32) -> Vec<Entry> {
    let dirs = xdg_data_dirs();
    enumerate_in_with_theme_and_scale(&dirs, icon_theme, icon_scale)
}
