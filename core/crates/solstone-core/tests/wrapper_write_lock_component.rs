// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-wrapper-write-lock-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("bin")).expect("bin directory");
        fs::create_dir_all(path.join("target")).expect("target directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ReapOnDrop(Option<Child>);

impl ReapOnDrop {
    fn spawn(bin_dir: &Path, target: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .arg("__wrapper-write")
            .arg(bin_dir)
            .arg(target)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wrapper-write child");
        Self(Some(child))
    }

    fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("child still held")
    }

    fn contention_line(&mut self) -> String {
        let stderr = self.child().stderr.take().expect("piped contention stderr");
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stderr).read_line(&mut line);
            let _ = sender.send((read, line));
        });
        let (read, line) = match receiver.recv_timeout(Duration::from_secs(8)) {
            Ok(result) => result,
            Err(error) => {
                let _ = self.child().kill();
                let _ = self.child().wait();
                let _ = reader.join();
                panic!("helper did not report lock contention before the deadline: {error}");
            }
        };
        reader.join().expect("contention reader joins");
        if read.expect("read contention line") == 0 {
            panic!(
                "helper exited before lock contention: {:?}",
                self.child().try_wait()
            );
        }
        line
    }

    fn wait(mut self) -> std::process::Output {
        self.0
            .take()
            .expect("child still held")
            .wait_with_output()
            .expect("join wrapper-write child")
    }
}

impl Drop for ReapOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct LockHolder(Option<Child>);

impl LockHolder {
    fn spawn(lock_path: &Path) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
            .arg("__wrapper-hold-lock")
            .arg(lock_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn wrapper lock holder");
        Self(Some(child))
    }

    fn wait_until_locked(&mut self) {
        let child = self.0.as_mut().expect("lock holder still held");
        let stdout = child.stdout.take().expect("piped lock holder stdout");
        let (sender, receiver) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send((read, line));
        });
        let (read, line) = match receiver.recv_timeout(Duration::from_secs(8)) {
            Ok(result) => result,
            Err(error) => {
                self.stop();
                let _ = reader.join();
                panic!("lock holder did not report readiness before the deadline: {error}");
            }
        };
        reader.join().expect("lock holder reader joins");
        if read.expect("read lock holder readiness") == 0 {
            panic!(
                "lock holder exited before readiness: {:?}",
                self.0.as_mut().expect("lock holder still held").try_wait()
            );
        }
        assert_eq!(line.trim_end(), "locked");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn assert_unwritten_wrapper(path: &Path, old: &[u8]) {
    assert_eq!(fs::read(path).expect("read wrapper"), old);
}

fn assert_written_once(path: &Path, old: &[u8], journal: &Path, sol_bin: &str) {
    let text = fs::read_to_string(path).expect("read written wrapper");
    assert_ne!(text.as_bytes(), old);
    assert!(text.starts_with("#!/bin/bash\n"));
    assert_eq!(text.matches("# managed-version:").count(), 1);
    assert!(text.contains("# managed-version: 7"));
    assert!(text.contains(&format!(
        ": \"${{SOLSTONE_JOURNAL:={}}}\"",
        journal.display()
    )));
    assert!(text.contains(&format!("SOL_BIN='{sol_bin}'")));
}

#[test]
fn wrapper_write_blocks_on_the_shared_lock_without_mutating_bytes() {
    let root = TestRoot::new();
    let bin = root.path().join("bin");
    let target = root.path().join("target");
    let sol = bin.join("solstone");
    let journal = bin.join("journal");
    fs::write(&sol, b"old sol bytes").expect("seed sol");
    fs::write(&journal, b"old journal bytes").expect("seed journal");

    let mut held = LockHolder::spawn(&bin.join(".sol.lock"));
    held.wait_until_locked();

    let mut child = ReapOnDrop::spawn(&bin, &target);
    let contended = child.contention_line();
    assert_eq!(contended.trim_end(), "contended");
    assert_unwritten_wrapper(&sol, b"old sol bytes");
    assert_unwritten_wrapper(&journal, b"old journal bytes");

    held.stop();
    let output = child.wait();
    assert!(
        output.status.success(),
        "wrapper-write child failed: {output:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout
        .lines()
        .rev()
        .find(|line| !line.is_empty())
        .unwrap_or("");
    assert_eq!(last, "wrapper-write-lock");

    assert_written_once(&sol, b"old sol bytes", &target, "/new/solstone");
    assert_written_once(&journal, b"old journal bytes", &target, "/new/journal");
    let residue = fs::read_dir(&bin)
        .expect("bin directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().contains(".tmp-"))
        .collect::<Vec<_>>();
    assert!(residue.is_empty(), "staging residue remains: {residue:?}");
}
