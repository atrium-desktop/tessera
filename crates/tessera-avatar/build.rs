//! Propagate the Optics native-library paths to this crate's test harness.
//!
//! The avatar runtime links Flux unconditionally and Vista behind the
//! `persona` feature; relay the rpaths published by the safe binding crates
//! (and by the `-sys` crates when the feature is on) so test binaries load the
//! same native libraries the build linked.

fn main() {
    let mut emitted_dtags = false;
    for var in [
        "DEP_FLUX_RS_RPATHS",
        "DEP_FLUX_RPATHS",
        "DEP_VISTA_RS_RPATHS",
        "DEP_VISTA_RPATHS",
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
