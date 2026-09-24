// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn authority_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read native authority directory") {
        let entry = entry.expect("read native authority entry");
        let path = entry.path();
        if entry.file_type().expect("authority file type").is_dir() {
            authority_files(&path, files);
        } else if path
            .file_name()
            .is_some_and(|name| name == "authority.toml")
        {
            files.push(path);
        }
    }
}

#[test]
fn committed_inventory_tracks_native_authorities() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root");
    let mut paths = Vec::new();
    authority_files(&root.join("core/native-sol"), &mut paths);
    paths.sort();
    assert!(!paths.is_empty(), "native authority scan found no files");

    let mut digest = Sha256::new();
    for path in paths {
        let relative = path.strip_prefix(root).expect("authority under repo");
        digest.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        digest.update(fs::read(&path).expect("read native authority"));
        digest.update([0]);
    }
    let expected = format!("{:x}", digest.finalize());
    let inventory = fs::read_to_string(
        root.join("core/crates/solstone-core-sol-client/src/generated/inventory.rs"),
    )
    .expect("read committed native inventory");
    let recorded = inventory
        .lines()
        .find_map(|line| line.strip_prefix("// authority-source-sha256: "))
        .expect("regenerate inventory: source digest is missing");
    assert_eq!(
        recorded, expected,
        "native authorities changed; run make build-native-sol-inventory"
    );
}
