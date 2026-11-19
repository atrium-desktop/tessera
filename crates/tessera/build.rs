//! Re-emit the native-library paths published by the Optics `-sys` crates via
//! their `links` metadata. This keeps the final binary correct for both
//! system-installed libraries and the opt-in local Optics override.
//! `rustc-link-arg` does not propagate across crates, so the terminal binary
//! must re-emit the paths itself.

fn main() {
    // Keep the Optics paths selected by the -sys crates ahead of unrelated
    // search paths introduced by dependencies such as Vulkan or FreeType
    // (tessera-platform emits `-L /usr/lib`, where stale system copies of the
    // optics libs may live). Emitting them from the terminal binary gives
    // them the intended link-search precedence.
    let mut link_dirs = Vec::new();
    for var in [
        "DEP_LENS_LINK_PATHS",
        "DEP_RETICLE_LINK_PATHS",
        "DEP_FLUX_LINK_PATHS",
        "DEP_VISTA_LINK_PATHS",
        "DEP_PRISM_LINK_PATHS",
        "DEP_LENS_RPATHS",
        "DEP_RETICLE_RPATHS",
        "DEP_FLUX_RPATHS",
        "DEP_VISTA_RPATHS",
        "DEP_PRISM_RPATHS",
    ] {
        if let Ok(paths) = std::env::var(var) {
            for dir in paths.split(';').filter(|s| !s.is_empty()) {
                if !link_dirs.iter().any(|existing| existing == dir) {
                    link_dirs.push(dir.to_owned());
                }
            }
        }
    }
    for dir in &link_dirs {
        println!("cargo:rustc-link-search=native={dir}");
    }

    let mut rpath_dirs = Vec::new();
    for var in [
        "DEP_FLUX_RPATHS",
        "DEP_VISTA_RPATHS",
        "DEP_LENS_RPATHS",
        "DEP_RETICLE_RPATHS",
        "DEP_PRISM_RPATHS",
    ] {
        if let Ok(rpaths) = std::env::var(var) {
            for dir in rpaths.split(';').filter(|s| !s.is_empty()) {
                if !rpath_dirs.iter().any(|existing| existing == dir) {
                    rpath_dirs.push(dir.to_owned());
                }
            }
        }
    }
    if !rpath_dirs.is_empty() {
        // DT_RPATH (not DT_RUNPATH) so the search also covers transitive
        // NEEDED libs (libflux is liblens's NEEDED, not ours).
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        for dir in &rpath_dirs {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
        }
    }
}
