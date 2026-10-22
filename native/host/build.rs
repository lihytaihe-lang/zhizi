//! Compiles the Rime C shim shared with the macOS host and links librime 1.16.1.
//! RIME_ROOT overrides the librime directory (include/ and lib/ inside).
use std::env;
use std::path::PathBuf;

fn main() {
    if env::var_os("CARGO_FEATURE_LIBRIME").is_none() {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repository = manifest.join("../..");
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let rime = env::var_os("RIME_ROOT").map(PathBuf::from).unwrap_or_else(|| {
        if windows {
            PathBuf::from(r"H:\zhizi\deps\librime-1.16.1\dist")
        } else {
            repository.join(".build-deps/librime/1.16.1")
        }
    });
    let shim = repository.join("native/macos/Sources/RimeShim");
    cc::Build::new()
        .file(shim.join("aim_rime_shim.c"))
        .include(shim.join("include"))
        .include(rime.join("include"))
        .warnings(false)
        .compile("aim_rime_shim");
    println!("cargo:rustc-link-search=native={}", rime.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=rime");
    println!("cargo:rerun-if-changed={}", shim.join("aim_rime_shim.c").display());
    println!("cargo:rerun-if-env-changed=RIME_ROOT");
}
