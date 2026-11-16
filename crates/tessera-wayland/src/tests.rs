use super::*;

mod authority_scene;
mod clipboard_e2e;
mod dmabuf_server;
mod drag_e2e;
mod output_damage;
mod popup_e2e;
mod protocol_state;
mod transitions;

/// Source of a Wayland client probe under this crate's `tests/fixtures/`.
///
/// The probes are checked in beside the tests that compile them, so a missing
/// file is a broken checkout rather than an environment gap: fail, never skip.
fn probe_source(name: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    assert!(path.is_file(), "probe source {} is missing", path.display());
    path
}
