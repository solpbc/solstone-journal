// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use solstone_core_local::admission::{acquire_local_slot, admission_dir};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

fn journal_root() -> PathBuf {
    let suffix = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "solstone-local-admission-cross-{}-{suffix}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("health")).expect("create journal health directory");
    root
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repository root")
        .to_path_buf()
}

fn python() -> PathBuf {
    let repository = repository_root();
    let venv = repository.join(".venv/bin/python3");
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

#[test]
fn rust_waits_for_a_python_held_slot() {
    let journal = journal_root();
    let script = concat!(
        "import os, sys\n",
        "sys.path.insert(0, os.environ['SOLSTONE_REPO_ROOT'])\n",
        "from solstone.think.providers.local_admission import acquire_local_slot\n",
        "import time\n",
        "with acquire_local_slot(1, 2.0):\n",
        "    print('ready', flush=True)\n",
        "    time.sleep(0.45)\n"
    );
    let mut child = Command::new(python())
        .args(["-c", script])
        .env("SOLSTONE_JOURNAL", &journal)
        .env("SOLSTONE_REPO_ROOT", repository_root())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start Python admission holder");
    let stdout = child.stdout.take().expect("Python stdout");
    let mut lines = BufReader::new(stdout).lines();
    assert_eq!(
        lines.next().expect("ready line").expect("ready text"),
        "ready"
    );

    let started = Instant::now();
    let permit = acquire_local_slot(
        &admission_dir(&journal),
        1,
        Some(Duration::from_secs(2)),
        false,
    )
    .expect("Rust obtains the released slot");
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "Rust acquired before Python released its flock"
    );
    drop(permit);
    assert!(child.wait().expect("reap Python holder").success());
    let _ = std::fs::remove_dir_all(journal);
}
