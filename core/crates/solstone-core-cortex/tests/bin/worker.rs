// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cortex as _;
use std::env;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    let mode = env::var("CORTEX_WORKER_MODE").unwrap_or_else(|_| "finish".into());
    match mode.as_str() {
        "finish" => finish(),
        "pwd" => pwd(),
        "stdin-fail" => stdin_fail(),
        "process-group" => process_group(),
        "grandchild-sleep" => loop {
            thread::sleep(Duration::from_secs(30));
        },
        "responsive-stop" => signal_child(true),
        "ignore-term" => signal_child(false),
        "sleep-exit" => {
            write_ready();
            thread::sleep(Duration::from_millis(300));
        }
        "sleep-long" => {
            write_ready();
            thread::sleep(Duration::from_secs(30));
        }
        other => panic!("unknown CORTEX_WORKER_MODE {other}"),
    }
}

fn finish() {
    if let Ok(marker) = env::var("CORTEX_MARKER") {
        let _ = fs::write(marker, "x");
    }
    println!("{{\"event\":\"finish\"}}");
}

fn pwd() {
    if let Ok(path) = env::var("CORTEX_CWD") {
        let cwd = env::current_dir().expect("cwd");
        fs::write(path, cwd.to_string_lossy().as_bytes()).expect("cwd receipt");
    }
    println!("{{\"event\":\"finish\"}}");
}

fn stdin_fail() {
    if let Ok(path) = env::var("CORTEX_CHILD_PID") {
        fs::write(path, std::process::id().to_string()).expect("pid receipt");
    }
}

// The grandchild is deliberately left unwaited: this worker exits while the
// descendant survives in the inherited process group, which is exactly the
// observable `captured_process_group_survives_direct_child_reap` asserts.
// The integration target's guard tears the whole group down afterwards.
#[allow(clippy::zombie_processes)]
fn process_group() {
    let ready = env::var("CORTEX_DESCENDANT_READY").expect("CORTEX_DESCENDANT_READY");
    let child = Command::new(env::current_exe().expect("current exe"))
        .env("CORTEX_WORKER_MODE", "grandchild-sleep")
        .spawn()
        .expect("grandchild");
    fs::write(ready, child.id().to_string()).expect("descendant receipt");
}

fn write_ready() {
    if let Ok(path) = env::var("CORTEX_READY") {
        fs::write(path, "ready").expect("ready receipt");
    }
}

fn signal_child(exit_on_term: bool) {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm");
        write_ready();
        term.recv().await;
        if let Ok(path) = env::var("CORTEX_SIGNALS") {
            let _ = fs::write(path, "TERM");
        }
        if !exit_on_term {
            std::future::pending::<()>().await;
        }
    });
}
