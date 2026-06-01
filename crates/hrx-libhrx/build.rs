//! Links the libhrx_rs cdylib against the IREE static archives.
//!
//! `cargo:rustc-link-arg` flags do NOT propagate transitively from the iree-sys
//! dependency's build script to this cdylib, so we re-emit them here, reusing
//! iree-sys's captured archive list (`../iree-sys/iree_archives.txt`). Same
//! re-rooting onto $HRX_BUILD_DIR + single --start-group as iree-sys/build.rs.

use std::path::{Path, PathBuf};

fn main() {
    let build_dir = std::env::var("HRX_BUILD_DIR").unwrap_or_else(|_| {
        format!("{}/github/hrx-system-build", std::env::var("HOME").unwrap())
    });
    let build_dir = PathBuf::from(build_dir);
    println!("cargo:rerun-if-env-changed=HRX_BUILD_DIR");

    let list_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("iree-sys")
        .join("iree_archives.txt");
    println!("cargo:rerun-if-changed={}", list_path.display());
    let list = std::fs::read_to_string(&list_path).expect("iree_archives.txt missing");

    let archives: Vec<PathBuf> = list
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let s = l.split("hrx-system-build/").last().unwrap_or(l);
            build_dir.join(s)
        })
        .collect();
    for a in &archives {
        assert!(a.exists(), "archive missing (build hrx-system first?): {}", a.display());
    }

    println!("cargo:rustc-link-arg=-Wl,--start-group");
    for a in &archives {
        println!("cargo:rustc-link-arg={}", a.display());
    }
    println!("cargo:rustc-link-arg=-Wl,--end-group");
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=pthread");
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
