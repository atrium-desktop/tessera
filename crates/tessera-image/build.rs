fn main() {
    println!("cargo:rustc-link-lib=png16");
    println!("cargo:rustc-link-lib=turbojpeg");
    println!("cargo:rustc-link-lib=webp");
    println!("cargo:rustc-link-lib=webpdemux");
}
