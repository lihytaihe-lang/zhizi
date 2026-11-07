//! Derives the per-source build identity consumed by the Zhixu index cache.
//!
//! The id changes whenever any core source, manifest, or bundled resource
//! changes, so a cache written by a different core build is rejected without
//! relying on anyone remembering to bump a constant. It never contains paths;
//! only sorted relative names and file bytes enter the hash.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn collect(directory: &Path, root: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            collect(&path, root, files);
        } else if file_type.is_file() {
            let Ok(bytes) = fs::read(&path) else {
                panic!("core build identity cannot read {}", path.display());
            };
            files.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
        }
    }
}

fn fnv1a64(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for part in parts {
        for &byte in *part {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Boundary byte so (ab, c) and (a, bc) never collide.
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut files = BTreeMap::new();
    collect(&root.join("src"), &root, &mut files);
    collect(&root.join("resources"), &root, &mut files);
    for name in ["Cargo.toml", "build.rs"] {
        let path = root.join(name);
        files.insert(PathBuf::from(name), fs::read(&path).unwrap());
    }
    let mut hash = fnv1a64(&[env::var("CARGO_PKG_VERSION").unwrap().as_bytes()]);
    for (path, bytes) in &files {
        hash = fnv1a64(&[&hash.to_le_bytes(), path.to_string_lossy().as_bytes(), bytes]);
    }
    println!("cargo:rustc-env=AGENT_IME_CORE_BUILD_ID={hash:016x}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=resources");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=build.rs");
}
