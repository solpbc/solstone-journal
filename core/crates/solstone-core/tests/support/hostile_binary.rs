// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real copied binaries in marker-free paths for terminal-sanitization tests.

use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn copied_binary(binary_dir: &Path) -> PathBuf {
    fs::create_dir_all(binary_dir).expect("isolated binary directory");
    assert!(
        !binary_dir
            .ancestors()
            .any(|ancestor| ancestor.join("pyproject.toml").is_file()),
        "isolated binary is outside checkout markers"
    );
    let copy = binary_dir.join("solstone-core");
    fs::copy(env!("CARGO_BIN_EXE_solstone-core"), &copy).expect("copy real binary");
    let permissions = fs::metadata(env!("CARGO_BIN_EXE_solstone-core"))
        .expect("source binary metadata")
        .permissions();
    fs::set_permissions(&copy, permissions).expect("copied binary remains executable");
    copy
}

pub(super) fn hostile_binary(component: &str) -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("solstone-hostile-executable-")
        .tempdir_in("/var/tmp")
        .expect("isolated hostile binary root");
    let binary_dir = temporary.path().join(component);
    let binary = copied_binary(&binary_dir);
    (temporary, binary)
}
