//! Compile Tessera's vendored scene runtime and renderer (ADR-0262) against
//! the Optics gfx public headers and generate their Rust bindings.
//!
//! The scene shaders are precompiled SPIR-V under `spv/` (regenerate with
//! `scripts/compile-shaders.sh`), embedded into the C translation units with
//! C23 `#embed`, so building Tessera needs no shader compiler.

use std::path::PathBuf;

use optics_build::{bindgen, write_bindings};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let include = manifest.join("csrc/include");
    let spv = manifest.join("spv");
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-changed=spv");
    println!("cargo:rerun-if-changed=wrapper.h");

    let include_dirs = gfx_include_dirs();

    let mut build = cc::Build::new();
    build
        .std("c23")
        .warnings(true)
        .extra_warnings(true)
        .flag("-fvisibility=hidden")
        .flag(format!("--embed-dir={}", spv.display()))
        .include(&include)
        .include(manifest.join("csrc/src"));
    for dir in &include_dirs {
        build.include(dir);
    }
    for file in [
        "animation.c",
        "glb.c",
        "json.c",
        "render.c",
        "scene_graph.c",
    ] {
        build.file(manifest.join("csrc/src").join(file));
    }
    build.compile("tessera_scene");
    println!("cargo:rustc-link-lib=m");

    // The same bindgen conventions the Optics -sys crates use.
    let bindings = bindgen::Builder::default()
        .rust_edition(bindgen::RustEdition::Edition2024)
        .header("wrapper.h")
        .clang_args(
            include_dirs
                .iter()
                .map(|dir| format!("-I{}", dir.display())),
        )
        .clang_arg("-std=c23")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: false,
        })
        .derive_default(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .clang_arg(format!("-I{}", include.display()))
        .allowlist_function("tessera_scene_.*")
        .allowlist_type("tessera_scene")
        .allowlist_type("tessera_scene_.*")
        .allowlist_var("TESSERA_SCENE_.*")
        .blocklist_type("optics_gfx_.*")
        .blocklist_function("optics_gfx_.*")
        .blocklist_var("OPTICS_GFX_.*")
        .blocklist_type("Vk.*")
        .layout_tests(true)
        .generate()
        .expect("bindgen failed to generate tessera-scene bindings");
    write_bindings(&bindings);

    relay_gfx_rpaths();
}

/// Let this crate's own test binaries load the gfx shared library in the
/// dynamic development link mode (`OPTICS_DYNAMIC=1`); static builds publish
/// no rpaths and this is a no-op.
fn relay_gfx_rpaths() {
    let Ok(rpaths) = std::env::var("DEP_OPTICS_GFX_RPATHS") else {
        return;
    };
    let dirs: Vec<&str> = rpaths.split(';').filter(|dir| !dir.is_empty()).collect();
    if dirs.is_empty() {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    for dir in dirs {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
    }
}

/// Include directories for the Optics gfx public headers and Vulkan.
///
/// Linking is owned by `optics-gfx-sys`; this only finds headers. A linked
/// development worktree points `OPTICS_SOURCE_DIR` at the sibling checkout
/// (`.cargo/optics-local.toml`); otherwise the installed `optics-gfx`
/// pkg-config package (or a Meson build tree named by `OPTICS_BUILD_DIR`)
/// supplies them.
fn gfx_include_dirs() -> Vec<PathBuf> {
    for var in ["OPTICS_SOURCE_DIR", "OPTICS_BUILD_DIR", "PKG_CONFIG_PATH"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let mut dirs = Vec::new();
    if let Some(root) = std::env::var_os("OPTICS_SOURCE_DIR").map(PathBuf::from) {
        let include = root.join("libs/gfx/include");
        if include.join("optics/gfx/core.h").exists() {
            dirs.push(include);
        }
    }
    if dirs.is_empty() {
        if let Some(build) = std::env::var_os("OPTICS_BUILD_DIR").map(PathBuf::from) {
            let uninstalled = build.join("meson-uninstalled");
            let path = std::env::var_os("PKG_CONFIG_PATH").unwrap_or_default();
            let mut paths = vec![uninstalled];
            paths.extend(std::env::split_paths(&path));
            // SAFETY: build scripts are single-threaded at this point.
            unsafe {
                std::env::set_var(
                    "PKG_CONFIG_PATH",
                    std::env::join_paths(paths).expect("PKG_CONFIG_PATH"),
                );
            }
        }
        let gfx = pkg_config::Config::new()
            .cargo_metadata(false)
            .probe("optics-gfx")
            .unwrap_or_else(|err| panic!("the Optics gfx headers are required: {err}"));
        dirs.extend(gfx.include_paths);
    }
    if let Ok(vulkan) = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("vulkan")
    {
        dirs.extend(vulkan.include_paths);
    }
    dirs
}
