// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("core crate has repository parent")
        .to_path_buf()
}

fn collect_shell_html(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("crate directory is readable") {
        let entry = entry.expect("crate directory entry is readable");
        let path = entry.path();
        if path.is_dir() {
            collect_shell_html(&path, files);
        } else if path.file_name().is_some_and(|name| name == "shell.html") {
            files.push(path);
        }
    }
}

#[test]
fn convey_shell_html_has_one_canonical_source() {
    let root = repository_root();
    let crates = root.join("core/crates");
    let canonical = crates.join("solstone-core-convey-shell/assets/static/shell.html");
    let mut shell_files = Vec::new();
    collect_shell_html(&crates, &mut shell_files);
    shell_files.sort();

    assert_eq!(shell_files, vec![canonical]);
}
