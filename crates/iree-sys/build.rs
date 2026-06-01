//! Links the Rust crate against the IREE runtime + HRX streaming static
//! archives produced by the hrx-system CMake build.
//!
//! The archive list (and its link order) is captured from the libhrx.so link
//! line into `iree_archives.txt` as paths relative to the CMake build dir. Set
//! `HRX_BUILD_DIR` to point at that build dir (default below). All archives are
//! wrapped in a single --start-group/--end-group so the linker resolves the
//! circular dependencies between IREE components.

use std::path::{Path, PathBuf};

fn main() {
    let build_dir = std::env::var("HRX_BUILD_DIR")
        .unwrap_or_else(|_| {
            format!("{}/github/hrx-system-build", std::env::var("HOME").unwrap())
        });
    let build_dir = PathBuf::from(build_dir);
    println!("cargo:rerun-if-env-changed=HRX_BUILD_DIR");
    println!("cargo:rerun-if-changed=iree_archives.txt");

    let list = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("iree_archives.txt"),
    )
    .expect("iree_archives.txt missing");

    // Each line is a path relative to the build dir (the file was captured with
    // absolute paths from one machine; we re-root every entry onto build_dir so
    // it is portable).
    let archives: Vec<PathBuf> = list
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            // Re-root: strip everything up to and including the build dir name.
            let p = Path::new(l);
            if p.is_absolute() {
                // find component "hrx-system-build" and rebuild from there
                let s = l.split("hrx-system-build/").last().unwrap_or(l);
                build_dir.join(s)
            } else {
                build_dir.join(l)
            }
        })
        .collect();

    for a in &archives {
        assert!(a.exists(), "archive missing (build hrx-system first?): {}", a.display());
    }

    // Emit a single grouped link directive. cargo doesn't natively support
    // --start-group around -l flags in order, so pass the archives as explicit
    // link args wrapped in a group.
    print!("cargo:rustc-link-arg=-Wl,--start-group");
    println!();
    for a in &archives {
        println!("cargo:rustc-link-arg={}", a.display());
    }
    println!("cargo:rustc-link-arg=-Wl,--end-group");

    // System libraries IREE needs.
    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=pthread");
    // libstdc++ for any C++ TUs in the runtime (flatcc/iree are C, but the HAL
    // amdgpu pieces may pull C++); harmless if unused.
    println!("cargo:rustc-link-lib=dylib=stdc++");
}
