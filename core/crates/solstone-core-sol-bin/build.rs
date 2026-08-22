// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn parent(path: PathBuf, label: &str) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| panic!("{label} has no parent: {}", path.display()))
        .to_path_buf()
}

fn main() {
    let out_dir =
        PathBuf::from(env::var_os("OUT_DIR").expect("solstone launcher staging requires OUT_DIR"));
    let profile_dir = parent(
        parent(
            parent(out_dir, "OUT_DIR out directory"),
            "OUT_DIR package build directory",
        ),
        "OUT_DIR build directory",
    );
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .expect("solstone launcher staging requires CARGO_MANIFEST_DIR"),
    );
    let repository_root = parent(
        parent(
            parent(manifest_dir, "crate manifest directory"),
            "crates directory",
        ),
        "core directory",
    );
    let source = repository_root.join("scripts/root-launchers/solstone");
    let source = source.canonicalize().unwrap_or_else(|error| {
        panic!(
            "solstone launcher source could not be resolved at {}: {error}",
            source.display()
        )
    });
    println!("cargo:rerun-if-changed={}", source.display());

    let destination = profile_dir.join("solstone");
    fs::copy(&source, &destination).unwrap_or_else(|error| {
        panic!(
            "solstone launcher could not be copied from {} to {}: {error}",
            source.display(),
            destination.display()
        )
    });
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap_or_else(|error| {
        panic!(
            "solstone launcher permissions could not be set at {}: {error}",
            destination.display()
        )
    });
}
