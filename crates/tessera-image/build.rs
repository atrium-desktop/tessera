//! Discover system image codec libraries via `pkg-config`.

fn main() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    for lib in ["libpng16", "libturbojpeg", "libwebp", "libwebpdemux"] {
        pkg_config::Config::new().probe(lib).unwrap_or_else(|err| {
            panic!("{lib} development files are required to build tessera-image: {err}")
        });
    }
}
