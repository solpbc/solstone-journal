// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_local::install::lease;

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn journal_root() -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "solstone-local-installer-process-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create lease journal root");
    root
}

fn run_child(root: &std::path::Path) -> std::process::Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "--ignored", "lease_child_probe"])
        .env("SOLSTONE_LOCAL_LEASE_HELPER", root)
        .output()
        .expect("run lease child probe")
}

fn child_line(output: &std::process::Output) -> &str {
    assert!(
        output.status.success(),
        "lease child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::str::from_utf8(&output.stdout)
        .expect("child stdout is utf-8")
        .trim()
}

#[test]
fn two_real_processes_cannot_hold_the_same_lease() {
    let root = journal_root();
    let held = lease::acquire(&root, "local").unwrap().unwrap();
    assert_eq!(child_line(&run_child(&root)), "contended");
    drop(held);
    assert_eq!(child_line(&run_child(&root)), "acquired-and-released");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore]
fn lease_child_probe() {
    let root = match std::env::var("SOLSTONE_LOCAL_LEASE_HELPER") {
        Ok(root) => PathBuf::from(root),
        Err(_) => panic!("SOLSTONE_LOCAL_LEASE_HELPER must name the lease journal"),
    };
    match lease::acquire(&root, "local") {
        Ok(None) => println!("contended"),
        Ok(Some(_lease)) => println!("acquired-and-released"),
        Err(error) => {
            eprintln!("lease acquire failed: {error}");
            std::process::exit(2);
        }
    }
}
