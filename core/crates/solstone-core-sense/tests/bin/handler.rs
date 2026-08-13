// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::process::Command;
use std::time::Duration;

fn main() {
    let arguments = std::env::args().collect::<Vec<_>>();
    if arguments
        .get(1)
        .is_some_and(|value| value == "--grandchild")
    {
        let path = arguments.get(2).expect("grandchild pid path");
        std::fs::write(path, std::process::id().to_string()).expect("write grandchild pid");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let file = arguments
        .iter()
        .find(|value| value.ends_with(".flac") || value.ends_with(".webm"))
        .expect("handler input file");
    let marker = format!("{file}.handler");
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
        .expect("handler log");
    writeln!(
        log,
        "{}",
        arguments.get(1).map(String::as_str).unwrap_or("unknown")
    )
    .expect("handler log write");
    for key in [
        "SOL_SEGMENT",
        "OBSERVER_NAME",
        "SEGMENT_META",
        "SOL_QUEUE_WAIT_MS",
    ] {
        if let Ok(value) = std::env::var(key) {
            writeln!(log, "{key}={value}").expect("handler environment log write");
        }
    }
    if file.contains("blocked") {
        std::process::exit(69);
    }
    if file.contains("fail") {
        std::process::exit(7);
    }
    if file.contains("sleep") {
        std::thread::sleep(Duration::from_secs(30));
    }
    if file.contains("grandchild") {
        let pid_file = format!("{file}.grandchild-pid");
        #[allow(clippy::zombie_processes)]
        // Intentional: the dispatcher must reap this orphaned child.
        let _grandchild = Command::new(std::env::current_exe().expect("handler exe"))
            .args(["--grandchild", &pid_file])
            .spawn()
            .expect("spawn grandchild");
        std::thread::sleep(Duration::from_secs(30));
    }
}
