// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types, dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-journal-archive-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn journal(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("journal");
    fs::create_dir(&root).expect("create journal root");
    root
}

pub fn write(root: &Path, member: &str, bytes: &[u8]) -> PathBuf {
    let path = root.join(member);
    fs::create_dir_all(path.parent().expect("member has parent")).expect("create member parent");
    fs::write(&path, bytes).expect("write member");
    path
}

pub fn directory(root: &Path, member: &str) -> PathBuf {
    let path = root.join(member);
    fs::create_dir_all(&path).expect("create directory");
    path
}

pub fn valid_four_root_journal(temp: &TempDir) -> PathBuf {
    let root = journal(temp);
    write(&root, "chronicle/20260101/a.txt", b"a");
    write(&root, "chronicle/20260101/nested/b.txt", b"bb");
    write(&root, "entities/alice/entity.json", b"{}");
    write(&root, "facets/work/facet.json", b"{}");
    write(&root, "imports/import-1/source.bin", b"source");
    write(&root, "config/journal.json", b"{}");
    root
}

pub fn entry<'a>(
    source: &'a solstone_core_journal_archive::ArchiveSource,
    member: &str,
) -> &'a solstone_core_journal_archive::InventoryEntry {
    source
        .inventory()
        .entries()
        .iter()
        .find(|entry| entry.member_name().as_str() == member)
        .expect("inventory entry")
}
