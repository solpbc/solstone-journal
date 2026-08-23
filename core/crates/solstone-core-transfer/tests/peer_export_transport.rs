// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Outbound peer-export HTTP contracts against a local stub.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

struct StubServer {
    base_url: String,
    posts: Arc<AtomicU64>,
}

impl StubServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let posts = Arc::new(AtomicU64::new(0));
        let thread_posts = posts.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                if request.starts_with("POST ") {
                    thread_posts.fetch_add(1, Ordering::SeqCst);
                }
                let body = b"{}";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        Self {
            base_url: format!("http://{address}"),
            posts,
        }
    }

    fn posts(&self) -> u64 {
        self.posts.load(Ordering::SeqCst)
    }
}

fn write_segment(journal: &Path, relative: &str) {
    let path = journal.join(relative);
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("audio.jsonl"), "{}\n").unwrap();
}

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn create(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-peer-export-transport-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn assert_named_default_refusal(result: &solstone_core_transfer::PeerExportAreaResult, posts: u64) {
    let error = result.error.as_deref().expect("area error");
    assert!(
        error
            .contains("named stream directory \"_default\" cannot be spelled as a record identity"),
        "{error}"
    );
    assert!(!error.contains("not UTF-8 representable"), "{error}");
    assert!(!error.contains("Exception during"), "{error}");
    assert_eq!(result.sent, 0);
    assert_eq!(posts, 0);
}

#[test]
fn named_default_at_different_key_refuses_export_before_any_post() {
    let journal = tempfile::tempdir().unwrap();
    write_segment(journal.path(), "chronicle/20260101/080000_300");
    write_segment(journal.path(), "chronicle/20260101/_default/090000_300");
    let stub = StubServer::start();
    let result = solstone_core_transfer::test_hooks::export_segments_result(
        journal.path(),
        &stub.base_url,
        &["20260101".to_owned()],
        false,
    );
    assert_named_default_refusal(&result, stub.posts());
}

#[cfg(unix)]
#[test]
fn named_default_identity_failure_escapes_control_characters_in_the_area_error() {
    let scratch = ScratchDir::create("journal\nroot");
    assert!(
        scratch.path.exists(),
        "newline journal root was not created: {}",
        scratch.path.display()
    );
    write_segment(&scratch.path, "chronicle/20260101/080000_300");
    write_segment(&scratch.path, "chronicle/20260101/_default/090000_300");
    let stub = StubServer::start();
    let result = solstone_core_transfer::test_hooks::export_segments_result(
        &scratch.path,
        &stub.base_url,
        &["20260101".to_owned()],
        false,
    );
    let error = result.error.as_deref().expect("area error");
    assert!(
        !error.contains('\n') && !error.contains('\r'),
        "raw control leaked into transfer diagnostic: {error:?}"
    );
    assert_eq!(error.lines().count(), 1, "{error:?}");
    assert!(error.contains("\\n"), "{error}");
    assert_named_default_refusal(&result, stub.posts());
}
