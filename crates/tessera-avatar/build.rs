//! Propagate the Optics native-library paths to this crate's test harness.
//!
//! The avatar runtime links gfx; relay the rpaths published by the binding
//! crates so test binaries load the same native libraries the build linked.
//! The scene runtime behind the `persona` feature (`tessera-scene`) is
//! compiled in statically and publishes none.

fn main() {
    let mut rpath_dirs = Vec::new();
    for var in ["DEP_OPTICS_GFX_RS_RPATHS", "DEP_OPTICS_GFX_RPATHS"] {
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
