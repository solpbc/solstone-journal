// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use solstone_core_journal_config::{ConfigLoadError, read_journal_config_bound};
use solstone_core_journal_io::{BoundReadPrimitive, JournalRoot, run_with_bound_read_barrier};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock follows epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-bound-config-socket-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary journal creates");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("temporary journal removes");
    }
}

#[test]
fn bound_config_reader_rejects_socket_substitution_before_open() {
    let root = TestRoot::new();
    let config_directory = root.path().join("config");
    fs::create_dir(&config_directory).expect("config directory creates");
    let config = config_directory.join("journal.json");
    fs::write(&config, br#"{"origin":"original"}"#).expect("config writes");
    let admitted = JournalRoot::open(root.path()).expect("journal root admits");

    let (result, fired) = run_with_bound_read_barrier(
        BoundReadPrimitive::Open,
        1,
        move || {
            fs::remove_file(&config).expect("config removes");
            let listener = UnixListener::bind(&config).expect("socket binds");
            drop(listener);
        },
        || read_journal_config_bound(&admitted),
    );

    assert!(fired, "bound read barrier fires");
    assert!(matches!(result, Err(ConfigLoadError::Corrupt { .. })));
}
