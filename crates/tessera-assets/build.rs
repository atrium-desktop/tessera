//! Propagate the Optics native-library paths to this crate's test harness.
//!
//! `tessera-assets` depends on the safe `reticle` binding and relays the
//! rpaths published by the underlying `-sys` crates. The `_RS` variants are
//! included so the relay keeps working if the safe bindings republish the
//! list through their own `links` metadata.

fn main() {
    let mut emitted_dtags = false;
    for var in [
        "DEP_RETICLE_RPATHS",
        "DEP_FLUX_RPATHS",
        "DEP_RETICLE_RS_RPATHS",
        "DEP_FLUX_RS_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            if !emitted_dtags {
                println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
                emitted_dtags = true;
            }
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
            }
        }
    }
}
