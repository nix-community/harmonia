fn main() {
    if std::env::var("DOCS_RS").is_ok() {
        return;
    }

    pkg_config::probe_library("nix-store").unwrap();
    pkg_config::probe_library("nix-main").unwrap();
    pkg_config::probe_library("libsodium").unwrap();

    let includedir =
        pkg_config::get_variable("nix-store", "includedir").expect("Failed to get includedir");

    cxx_build::bridge("src/lib.rs")
        .file("src/nix.cpp")
        // TODO: fix this upstream
        .include(includedir + "/nix")
        .flag_if_supported("-std=c++2a")
        .flag_if_supported("-O2")
        .compile("libnixstore");
    println!("cargo:rerun-if-changed=include/nix.h");
    println!("cargo:rerun-if-changed=src/nix.cpp");
    println!("cargo:rerun-if-changed=src/lib.rs");
}
