// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::Write;
use std::process::Command;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    match mode.as_str() {
        "lines" => {
            println!("stdout-line");
            let _ = std::io::stderr().write_all(b"stderr-line\n");
        }
        "sleep" => std::thread::sleep(Duration::from_secs(30)),
        "block-term-sleep" => {
            let ready_path = args.next().expect("ready path");
            let mut signals = nix::sys::signal::SigSet::empty();
            signals.add(nix::sys::signal::Signal::SIGTERM);
            signals.thread_block().expect("block SIGTERM");
            std::fs::write(ready_path, "ready").expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "term-resistant-descendant" => {
            let ready_path = args.next().expect("ready path");
            let executable = std::env::current_exe().expect("fixture executable");
            let mut child = Command::new(executable)
                .args(["block-term-sleep", &ready_path])
                .spawn()
                .expect("spawn term-resistant descendant");
            let _ = child.wait();
        }
        "setsid-grandchild" => {
            let ready_path = args.next().expect("ready path");
            let executable = std::env::current_exe().expect("fixture executable");
            let mut child = Command::new(executable)
                .args(["escaped-sleep", &ready_path])
                .spawn()
                .expect("spawn escaped grandchild");
            let _ = child.wait();
        }
        "escaped-sleep" => {
            let ready_path = args.next().expect("ready path");
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            nix::unistd::setsid().expect("escape parent process group");
            // Publish the pid, not a literal: the escaped-descendant test has to
            // assert this process is *gone*, and "the parent exited cleanly" is
            // equally true of an implementation that never looked for it.
            std::fs::write(ready_path, std::process::id().to_string()).expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        _ => std::process::exit(64),
    }
}
