// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_import::{OwnerSource, observe_source_immutability};

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn owner_source_exposes_only_aggregate_metadata() {
    let tree = TempTree::new();
    fs::create_dir(tree.path().join("nested")).unwrap();
    fs::write(tree.path().join("nested/source.txt"), b"source").unwrap();

    let metadata = OwnerSource::new(tree.path()).metadata().unwrap();
    assert_eq!(metadata.entry_count(), 2);
    assert_eq!(metadata.total_bytes(), 6);
}

#[test]
fn immutability_harness_reports_a_deliberately_mutating_source() {
    let tree = TempTree::new();
    let source_file = tree.path().join("source.txt");
    fs::write(&source_file, b"before").unwrap();

    let report = observe_source_immutability(tree.path(), |_| {
        fs::write(&source_file, b"after").unwrap();
    })
    .unwrap();

    assert!(report.violated());
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let index = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-owner-source-{}-{index}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}
