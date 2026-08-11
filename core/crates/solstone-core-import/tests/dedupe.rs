// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_import::dedupe::hash_source;

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn directory_hash_uses_path_component_order_not_rendered_string_order() {
    // import_reference_oracles.json:65-87: `sub/a.txt` precedes `sub.txt`.
    let root = std::env::temp_dir().join(format!(
        "solstone-w1b-hash-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub.txt"), b"x").unwrap();
    fs::write(root.join("sub/a.txt"), b"xy").unwrap();
    assert_eq!(
        hash_source(&root).unwrap().as_str(),
        "b4c867b6d93347e772bb59b59be1619e56d895f251b573465b58457677ed572a"
    );
    fs::remove_dir_all(root).unwrap();
}
