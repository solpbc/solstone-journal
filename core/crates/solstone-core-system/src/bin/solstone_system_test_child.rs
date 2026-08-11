// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::Duration;

use solstone_core_system::lifecycle::SupervisorLifecycle;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    match mode.as_str() {
        // "-m" is llama-server's model flag; "--model" is parakeet-server's.
        // Both real binaries land here as this fixture's stand-in.
        "-m" | "--model" => launch_stub(args),
        "lines" => {
            println!("stdout-line");
            let _ = std::io::stderr().write_all(b"stderr-line\n");
        }
        "sleep" => std::thread::sleep(Duration::from_secs(30)),
        "hold-supervisor-lock" => {
            let journal = args.next().expect("journal path");
            let ready_path = args.next().expect("ready path");
            let _lifecycle = SupervisorLifecycle::boot(&journal).expect("acquire supervisor lock");
            std::fs::write(ready_path, "ready").expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "try-supervisor-lock" => {
            let journal = args.next().expect("journal path");
            let result_path = args.next().expect("result path");
            let value = match SupervisorLifecycle::boot(&journal) {
                Ok(_lifecycle) => "acquired",
                Err(error) if error.to_string() == "supervisor already running" => {
                    "already-running"
                }
                Err(error) => panic!("unexpected lifecycle error: {error}"),
            };
            std::fs::write(result_path, value).expect("write acquisition outcome");
        }
        "orphan-sweep-spawner" => {
            let journal = args.next().expect("journal path");
            let ready_path = args.next().expect("ready path");
            let holder_mode = args
                .next()
                .unwrap_or_else(|| "orphan-sweep-holder".to_owned());
            let executable = args
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_exe().expect("fixture executable"));
            let child = Command::new(executable)
                .args([&holder_mode, &journal, &ready_path])
                .env("SOLSTONE_JOURNAL", &journal)
                .spawn()
                .expect("spawn orphan holder");
            // This fixture's purpose is to orphan the holder on this process's exit.
            std::mem::forget(child);
        }
        "orphan-sweep-holder" | "orphan-sweep-holder-resists-term" => {
            let _journal = args.next().expect("journal path");
            let ready_path = args.next().expect("ready path");
            if mode == "orphan-sweep-holder-resists-term" {
                let mut signals = nix::sys::signal::SigSet::empty();
                signals.add(nix::sys::signal::Signal::SIGTERM);
                signals.thread_block().expect("block SIGTERM");
            }
            #[cfg(target_os = "linux")]
            std::fs::write("/proc/self/comm", "journal:holder\n").expect("set proc title");
            std::fs::write(ready_path, std::process::id().to_string()).expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "ready-sleep" => {
            let ready_path = args.next().expect("ready path");
            let millis: u64 = args
                .next()
                .expect("milliseconds")
                .parse()
                .expect("milliseconds integer");
            std::fs::write(ready_path, fixture_ready_marker()).expect("signal readiness");
            std::thread::sleep(Duration::from_millis(millis));
        }
        "continuous-lines" => {
            let ready_path = args.next().expect("ready path");
            std::fs::write(ready_path, fixture_ready_marker()).expect("signal readiness");
            for index in 0_u64.. {
                println!("line-{index}");
                std::io::stdout().flush().expect("flush stdout");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        "restart-once" => {
            let state_path = args.next().expect("state path");
            if std::path::Path::new(&state_path).exists() {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            } else {
                std::fs::write(&state_path, "exited").expect("record fixture exit");
                std::process::exit(1);
            }
        }
        "block-term-count" => {
            let ready_path = args.next().expect("ready path");
            let count_path = args.next().expect("count path");
            let mut signals = nix::sys::signal::SigSet::empty();
            signals.add(nix::sys::signal::Signal::SIGTERM);
            signals.thread_block().expect("block SIGTERM");
            std::fs::write(ready_path, "ready").expect("signal readiness");
            loop {
                let signal = signals.wait().expect("wait SIGTERM");
                if signal == nix::sys::signal::Signal::SIGTERM {
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&count_path)
                        .expect("open count file");
                    writeln!(file, "term").expect("record SIGTERM");
                    file.flush().expect("flush count");
                }
            }
        }
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

fn fixture_ready_marker() -> String {
    format!(
        "ready:{}:{}",
        std::env::var("SOL_SUPERVISOR_SPAWNED").unwrap_or_default(),
        std::process::id()
    )
}

fn launch_stub(mut args: impl Iterator<Item = String>) {
    let model_path = args.next().unwrap_or_default();
    let arguments = args.collect::<Vec<_>>();
    let port = arguments
        .windows(2)
        .find(|window| window[0] == "--port")
        .and_then(|window| window[1].parse::<u16>().ok());
    // Lifecycle tests may pass either the historic literal marker or a real
    // pinned model path whose fixture contents carry that marker.
    let model = std::fs::read_to_string(&model_path).unwrap_or(model_path);
    match model.trim() {
        "test-ready" | "test-ready-block-term" => {
            if model.trim() == "test-ready-block-term" {
                let mut signals = nix::sys::signal::SigSet::empty();
                signals.add(nix::sys::signal::Signal::SIGTERM);
                signals.thread_block().expect("block SIGTERM");
            }
            let listener = TcpListener::bind(("127.0.0.1", port.expect("launch port")))
                .expect("bind launch stub");
            for stream in listener.incoming() {
                let mut stream = stream.expect("accept health probe");
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    )
                    .expect("write health response");
            }
        }
        "test-hold" => std::thread::sleep(Duration::from_secs(30)),
        _ => std::process::exit(64),
    }
}
