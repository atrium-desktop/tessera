//! Propagate the gfx native-library path to this crate's test harness.

fn main() {
    let mut rpath_dirs = Vec::new();
    if let Ok(rpaths) = std::env::var("DEP_OPTICS_GFX_RPATHS") {
        for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
            if !rpath_dirs.iter().any(|existing| existing == dir) {
                rpath_dirs.push(dir.to_owned());
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
