//! Propagate the Optics native-library paths to this crate's test harness.
//!
//! The avatar runtime links gfx unconditionally and optics-scene behind the
//! `persona` feature; relay the rpaths published by the safe binding crates
//! (and by the `-sys` crates when the feature is on) so test binaries load the
//! same native libraries the build linked.

fn main() {
    let mut rpath_dirs = Vec::new();
    for var in [
        "DEP_OPTICS_GFX_RS_RPATHS",
        "DEP_OPTICS_GFX_RPATHS",
        "DEP_OPTICS_SCENE_RS_RPATHS",
        "DEP_OPTICS_SCENE_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                if !rpath_dirs.iter().any(|existing| existing == dir) {
                    rpath_dirs.push(dir.to_owned());
                }
            }
        }
    }
    if !rpath_dirs.is_empty() {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        for dir in &rpath_dirs {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
        }
    }
}
