// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;

pub fn retention_binary() -> PathBuf {
    if let Some(path) = env::var_os("SOLSTONE_RETENTION_BIN") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "SOLSTONE_RETENTION_BIN is not a file: {}",
            path.display()
        );
        return path;
    }
    let executable = env::current_exe().expect("test executable path");
    let path = executable
        .parent()
        .and_then(|path| path.parent())
        .expect("test executable has a target directory")
        .join("solstone-retention");
    assert!(
        path.is_file(),
        "missing {}. Build it with `cargo build -p solstone-core-retention-cli`, or set SOLSTONE_RETENTION_BIN.",
        path.display()
    );
    path
}
