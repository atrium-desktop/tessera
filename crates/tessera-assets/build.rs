//! Discover system `librsvg-2.0` + `cairo` and generate embedded cursor tables.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    pkg_config::Config::new()
        .atleast_version("2.48")
        .probe("librsvg-2.0")
        .expect("librsvg-2.0 >= 2.48 development files are required to build tessera-assets");
    pkg_config::Config::new()
        .probe("cairo")
        .expect("cairo development files are required to build tessera-assets");

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
