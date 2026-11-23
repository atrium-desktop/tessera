//! Re-emit the Optics native-library rpaths for the root test harnesses.
//!
//! The suites link the compositor stack, whose shared libraries are only
//! resolved at run time. `rustc-link-arg` does not cross crate boundaries, so
//! the terminal test binaries must restate the search paths published by the
//! `-sys` crates (the same relay `crates/tessera/build.rs` performs for the
//! shipped binaries).

fn main() {
    let mut emitted_dtags = false;
    for var in [
        "DEP_OPTICS_GFX_RPATHS",
        "DEP_OPTICS_UI_RPATHS",
        "DEP_OPTICS_SVG_RPATHS",
        "DEP_OPTICS_SCENE_RPATHS",
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
