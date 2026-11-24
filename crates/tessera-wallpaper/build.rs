//! Propagate the gfx and optics-scene native-library paths to this crate's test
//! harness.

fn main() {
    let mut rpath_dirs = Vec::new();
    for var in ["DEP_OPTICS_GFX_RPATHS", "DEP_OPTICS_SCENE_RPATHS"] {
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
