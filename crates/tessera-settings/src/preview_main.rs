//! Development-only inspection harness for Tessera Settings (ADR-0257).
//!
//! Built only when the non-default `dev-preview` feature is enabled and
//! excluded from distribution manifests by `xtask check-install-manifest`.

mod app;
mod preview;

fn main() {
    tessera_env::init("info");
    if let Err(error) = preview::run() {
        log::error!("settings preview: {error}");
        eprintln!("tessera-settings-preview: {error}");
        std::process::exit(1);
    }
}
