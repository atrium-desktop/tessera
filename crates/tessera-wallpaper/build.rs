//! Propagate the gfx and optics-scene native-library paths to this crate's test
//! harness.

fn main() {
    let mut emitted_dtags = false;
    for var in ["DEP_OPTICS_GFX_RPATHS", "DEP_OPTICS_SCENE_RPATHS"] {
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
