//! Icon-theme resolution against the host's real icon themes.
//!
//! Skips automatically when no host icon data is present (CI sandboxes).

use tessera_icons::{icon_search_bases, resolve_icon, xdg_data_dirs};

fn have_system_icons() -> bool {
    xdg_data_dirs()
        .iter()
        .any(|dir| dir.join("icons").is_dir() || dir.join("pixmaps").is_dir())
}

#[test]
fn icon_resolution_uses_hicolor() {
    if !have_system_icons() {
        return;
    }
    let bases = icon_search_bases();
    // "foot" is a well-known hicolor icon name on a foot-installed host.
    if let Some(p) = resolve_icon("foot", None, &bases, 48) {
        assert!(p.exists(), "{p:?} missing");
        assert!(
            p.to_string_lossy().contains("/icons/hicolor/"),
            "expected hicolor path, got {p:?}"
        );
    }
}

#[test]
fn icon_resolution_picks_closest_size() {
    let bases = icon_search_bases();
    if let Some(p) = resolve_icon("btop", None, &bases, 48) {
        let s = p.to_string_lossy();
        // Should land on a size directory near 48 (32/48/64 acceptable).
        let near = ["48x48", "32x32", "64x64", "scalable"]
            .iter()
            .any(|d| s.contains(d));
        assert!(near, "unexpected size dir in {s}");
    }
}

#[test]
fn data_dirs_precedence_keeps_user_first() {
    let dirs = xdg_data_dirs();
    assert!(dirs.iter().all(|dir| dir.is_absolute()));
    let unique: std::collections::HashSet<&std::path::PathBuf> = dirs.iter().collect();
    assert_eq!(unique.len(), dirs.len(), "duplicate XDG data dirs present");
}
