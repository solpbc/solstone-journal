// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_import::observe_source_immutability;
use solstone_core_import_sources::MODULE_STUBS;

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);

#[test]
fn source_stubs_leave_the_owner_source_unchanged() {
    let tree = TempTree::new();
    fs::write(tree.path().join("source.txt"), b"source").unwrap();

    // This positive direction is trivially true because every source is a stub. A later staging test
    // promotes it to real behavior; the negative twin in the import crate proves detection works.
    let report = observe_source_immutability(tree.path(), |_| {
        for (_, stub) in MODULE_STUBS {
            assert!(stub().is_err());
        }
    })
    .unwrap();

    assert!(!report.violated());
}

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let index = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-import-sources-immutability-{}-{index}",
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
