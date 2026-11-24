//! Propagate the Optics native-library paths to this crate's test harness.
//!
//! `tessera-assets` depends on the safe `optics-svg` binding and relays the
//! rpaths published by the underlying `-sys` crates. The `_RS` variants are
//! included so the relay keeps working if the safe bindings republish the
//! list through their own `links` metadata.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let mut rpath_dirs = Vec::new();
    for var in [
        "DEP_OPTICS_SVG_RPATHS",
        "DEP_OPTICS_GFX_RPATHS",
        "DEP_OPTICS_SVG_RS_RPATHS",
        "DEP_OPTICS_GFX_RS_RPATHS",
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

    generate_embedded_cursors();
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if path.is_file() {
            println!("cargo:rerun-if-changed={}", path.display());
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                out.push((rel_str, path));
            }
        }
    }
}

fn generate_embedded_cursors() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let cursors_root = manifest_dir.join("../../assets/cursors");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    let themes = [
        ("USER_LIGHT", "tessera-user-light"),
        ("USER_DARK", "tessera-user-dark"),
        ("AI_LIGHT", "tessera-ai-light"),
        ("AI_DARK", "tessera-ai-dark"),
    ];

    let mut code = String::new();
    for (const_name, dir_name) in themes {
        let theme_root = cursors_root.join(dir_name);
        let mut files = Vec::new();
        collect_files(&theme_root, &theme_root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let _ = writeln!(
            code,
            "pub static {const_name}: EmbeddedDir = EmbeddedDir(&["
        );
        for (rel, abs) in files {
            let abs_canonical = abs.canonicalize().unwrap_or(abs);
            let _ = writeln!(
                code,
                "    ({rel:?}, include_bytes!({:?})),",
                abs_canonical.to_string_lossy()
            );
        }
        let _ = writeln!(code, "]);");
    }

    std::fs::write(out_dir.join("embedded_cursors.rs"), code).expect("write embedded_cursors.rs");
}
