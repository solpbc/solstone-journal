// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use chrono::{FixedOffset, TimeZone};

// This source remains a child of the binary-only capture module. Including it
// here drives the exact production descriptor primitive without importing the
// unrelated supervisor guard and rollover worker.
#[path = "../src/service_capture_io.rs"]
mod service_capture_io;

fn instant() -> chrono::DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .unwrap()
        .with_ymd_and_hms(2026, 8, 7, 12, 0, 0)
        .single()
        .unwrap()
}

fn assert_contains(path: &std::path::Path, expected: &[u8]) {
    assert!(
        fs::read(path)
            .unwrap()
            .windows(expected.len())
            .any(|bytes| bytes == expected),
        "{} did not contain {:?}",
        path.display(),
        expected
    );
}

#[test]
fn redirection_writes_both_targets_to_the_service_oplog() {
    const CHILD_JOURNAL: &str = "SOLSTONE_SERVICE_CAPTURE_CHILD_JOURNAL";
    if let Ok(journal) = std::env::var(CHILD_JOURNAL) {
        let journal = PathBuf::from(journal);
        let writer = service_capture_io::open_service_oplog(&journal, instant()).unwrap();
        fs::write(journal.join("capture-leaf"), writer.leaf_name()).unwrap();
        service_capture_io::redirect_both(&writer, None).unwrap();
        nix::unistd::write(std::io::stdout(), b"stdout bytes").unwrap();
        nix::unistd::write(std::io::stderr(), b"stderr bytes").unwrap();
        return;
    }

    let journal = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "redirection_writes_both_targets_to_the_service_oplog",
            "--nocapture",
        ])
        .env(CHILD_JOURNAL, journal.path())
        .status()
        .unwrap();

    assert!(status.success());
    let leaf = fs::read_to_string(journal.path().join("capture-leaf")).unwrap();
    let path = journal.path().join("chronicle/20260807/health").join(leaf);
    assert_contains(&path, b"stdout bytes");
    assert_contains(&path, b"stderr bytes");
}
