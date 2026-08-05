// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;

pub fn generate_wire() -> PathBuf {
    if let Some(path) = env::var_os("SOLSTONE_GENERATE_WIRE") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "SOLSTONE_GENERATE_WIRE is not a file: {}",
            path.display()
        );
        return path;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .ancestors()
        .find(|candidate| {
            candidate.join(".git").exists()
                && candidate.join("pyproject.toml").is_file()
                && candidate.join("solstone").is_dir()
        })
        .expect("generate-wire integration tests require a checkout root");
    let path = root.join(".venv/bin/solstone-generate-wire");
    assert!(
        path.is_file(),
        "missing {}; set SOLSTONE_GENERATE_WIRE or run make install",
        path.display()
    );
    path
}
