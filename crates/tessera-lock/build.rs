//! Link the native session dependencies and preserve local Optics rpaths.

fn main() {
    let mut local_link_dirs = Vec::new();
    for var in [
        "DEP_OPTICS_UI_LINK_PATHS",
        "DEP_OPTICS_GFX_LINK_PATHS",
        "DEP_OPTICS_SCENE_LINK_PATHS",
        "DEP_OPTICS_UI_RPATHS",
        "DEP_OPTICS_GFX_RPATHS",
        "DEP_OPTICS_SCENE_RPATHS",
    ] {
        if let Ok(paths) = std::env::var(var) {
            for dir in paths.split(';').filter(|path| !path.is_empty()) {
                if !local_link_dirs.iter().any(|existing| existing == dir) {
                    local_link_dirs.push(dir.to_owned());
                }
            }
        }
    }

    // In local Optics mode an older system liboptics_gfx may still live in
    // `/usr/lib`. Put the binding-published Meson directories first so the
    // linker selects the intended native build.
    for dir in &local_link_dirs {
        println!("cargo:rustc-link-search=native={dir}");
    }

    pkg_config::Config::new()
        .atleast_version("1.20")
        .probe("wayland-client")
        .expect("tessera-lock requires wayland-client");
    pkg_config::Config::new()
        .probe("pam")
        .expect("tessera-lock requires PAM");

    let mut local_rpaths = Vec::new();
    for var in [
        "DEP_OPTICS_UI_RPATHS",
        "DEP_OPTICS_GFX_RPATHS",
        "DEP_OPTICS_SCENE_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            for dir in rpaths.split(';').filter(|path| !path.is_empty()) {
                if !local_rpaths.iter().any(|existing| existing == dir) {
                    local_rpaths.push(dir.to_owned());
                }
            }
        }
    }
    if !local_rpaths.is_empty() {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    }
    for dir in local_rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}
