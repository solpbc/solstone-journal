// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};

use solstone_core_segment::is_solstone_up;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-segment-supervisor-loopback-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_port(root: &Path, contents: &str) {
    let health = root.join("health");
    fs::create_dir(&health).unwrap();
    fs::write(health.join("convey.port"), contents).unwrap();
}

#[test]
fn recorded_convey_listener_marks_solstone_up() {
    let temporary = TempDir::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    write_port(
        temporary.path(),
        &listener.local_addr().unwrap().port().to_string(),
    );
    assert!(is_solstone_up(temporary.path()));
}

#[test]
fn reserved_refused_port_is_not_up() {
    let temporary = TempDir::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    write_port(temporary.path(), &port.to_string());
    assert!(!is_solstone_up(temporary.path()));
}

#[test]
fn missing_port_file_is_not_up() {
    let temporary = TempDir::new();
    assert!(!is_solstone_up(temporary.path()));
}

#[test]
fn malformed_port_file_is_not_up() {
    let temporary = TempDir::new();
    write_port(temporary.path(), "not-a-port");
    assert!(!is_solstone_up(temporary.path()));
}
