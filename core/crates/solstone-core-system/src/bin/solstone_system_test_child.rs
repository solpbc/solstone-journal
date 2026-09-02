// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
use solstone_core_system::lifecycle::acknowledge_hosted_child_admission;
use solstone_core_system::lifecycle::{SupervisorLifecycle, WriterId};
use solstone_core_system::process::{ManagedProcess, SpawnOptions, apply_parent_death_kill};
#[cfg(unix)]
use std::path::Path;

fn writer_id() -> WriterId {
    WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID")
}

fn main() {
    #[cfg(unix)]
    {
        if let Some(journal) = std::env::var_os("SOLSTONE_JOURNAL")
            && let Err(error) = acknowledge_hosted_child_admission(Path::new(&journal))
        {
            eprintln!("hosted fixture child admission failed: {error}");
            std::process::exit(78);
        }
    }
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
        #[cfg(windows)]
        "echo-stdin" => {
            let mut input = Vec::new();
            std::io::stdin()
                .read_to_end(&mut input)
                .expect("read bounded helper input");
            std::io::stdout()
                .write_all(&input)
                .expect("write bounded helper output");
        }
        #[cfg(windows)]
        "write-stdout" => {
            let count = args
                .next()
                .expect("output byte count")
                .parse::<usize>()
                .expect("numeric output byte count");
            std::io::stdout()
                .write_all(&vec![b'x'; count])
                .expect("write bounded stdout fixture");
        }
        #[cfg(windows)]
        "environment-present" => {
            let name = args.next().expect("environment variable name");
            println!(
                "{}",
                if std::env::var_os(name).is_some() {
                    "present"
                } else {
                    "absent"
                }
            );
        }
        #[cfg(windows)]
        "current-directory" => {
            println!(
                "{}",
                std::env::current_dir()
                    .expect("read child current directory")
                    .display()
            );
        }
        #[cfg(windows)]
        "exit-code" => {
            let code = args
                .next()
                .expect("exit code")
                .parse::<u32>()
                .expect("numeric exit code");
            std::process::exit(code as i32);
        }
        #[cfg(windows)]
        "probe-handle-absent" => {
            use windows_sys::Win32::Foundation::{ERROR_INVALID_HANDLE, GetHandleInformation};

            let raw = args
                .next()
                .expect("handle value")
                .parse::<usize>()
                .expect("numeric handle value") as *mut _;
            let mut flags = 0;
            // SAFETY: this intentionally probes only the numeric handle value
            // supplied by the parent; the API writes one HANDLE_FLAGS result.
            #[allow(unsafe_code)]
            let result = unsafe { GetHandleInformation(raw, &raw mut flags) };
            if result != 0 {
                println!("present");
            } else if std::io::Error::last_os_error().raw_os_error()
                == Some(ERROR_INVALID_HANDLE as i32)
            {
                println!("absent");
            } else {
                eprintln!(
                    "unexpected GetHandleInformation failure: {}",
                    std::io::Error::last_os_error()
                );
                std::process::exit(65);
            }
        }
        #[cfg(windows)]
        "job-tree-root" => {
            let root_ready = args.next().expect("root readiness path");
            let grandchild_ready = args.next().expect("grandchild readiness path");
            let executable = std::env::current_exe().expect("fixture executable");
            let mut grandchild = Command::new(executable)
                .args(["job-tree-grandchild", &grandchild_ready])
                .spawn()
                .expect("spawn Job-tree grandchild");
            std::fs::write(root_ready, std::process::id().to_string())
                .expect("publish Job-tree root PID");
            let _ = grandchild.wait();
        }
        #[cfg(windows)]
        "job-tree-grandchild" => {
            let ready_path = args.next().expect("grandchild readiness path");
            std::fs::write(ready_path, std::process::id().to_string())
                .expect("publish Job-tree grandchild PID");
            std::thread::sleep(Duration::from_secs(30));
        }
        "host-death-managed" => {
            let ready_path = args.next().expect("ready path");
            let journal_root = args.next().expect("journal root");
            let executable = std::env::current_exe().expect("fixture executable");
            let process = ManagedProcess::spawn(
                vec![
                    executable.to_string_lossy().into_owned(),
                    "sleep".to_owned(),
                ],
                SpawnOptions {
                    journal_root: PathBuf::from(journal_root),
                    reference: "host-death".to_owned(),
                    day: None,
                    sink: None,
                    environment: Default::default(),
                },
            )
            .expect("spawn host-death managed child");
            std::fs::write(&ready_path, process.pid().to_string()).expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "host-death-direct" => {
            let ready_path = args.next().expect("ready path");
            let executable = std::env::current_exe().expect("fixture executable");
            let mut command = Command::new(executable);
            command.arg("sleep");
            apply_parent_death_kill(&mut command);
            // This process is about to be killed externally to simulate host
            // death; the child outliving it (or being reaped by PDEATHSIG) is
            // exactly what the test observes, so it is never wait()ed on here.
            #[allow(clippy::zombie_processes)]
            let child = command.spawn().expect("spawn host-death direct child");
            std::fs::write(&ready_path, child.id().to_string()).expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "hold-supervisor-lock" => {
            let journal = args.next().expect("journal path");
            let ready_path = args.next().expect("ready path");
            let _lifecycle =
                SupervisorLifecycle::boot(&journal, writer_id()).expect("acquire supervisor lock");
            std::fs::write(ready_path, "ready").expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        "try-supervisor-lock" => {
            let journal = args.next().expect("journal path");
            let result_path = args.next().expect("result path");
            let value = match SupervisorLifecycle::boot(&journal, writer_id()) {
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
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    let mut signals = nix::sys::signal::SigSet::empty();
                    signals.add(nix::sys::signal::Signal::SIGTERM);
                    signals.thread_block().expect("block SIGTERM");
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                std::process::exit(64);
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
        "ready-sleep-crash-once" => {
            let ready_path = args.next().expect("ready path");
            let state_path = args.next().expect("state path");
            let port_path = args.next().expect("port path");
            std::fs::write(&ready_path, fixture_ready_marker()).expect("signal readiness");
            write_fixture_port_file(std::path::Path::new(&port_path));
            match std::fs::read_to_string(&state_path).ok().as_deref() {
                None => {
                    std::fs::write(&state_path, "initial").expect("record initial fixture run");
                    loop {
                        std::thread::park();
                    }
                }
                Some("initial") => {
                    std::fs::write(&state_path, "crashed").expect("record fixture crash");
                    std::process::exit(1);
                }
                Some("crashed") => loop {
                    std::thread::park();
                },
                Some(_) => panic!("unexpected crash-once fixture state"),
            }
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
        "ready-park" => {
            let ready_path = args.next().expect("ready path");
            std::fs::write(ready_path, fixture_ready_marker()).expect("signal readiness");
            loop {
                std::thread::park();
            }
        }
        "always-exit" => std::process::exit(1),
        "always-tempfail" => std::process::exit(75),
        "fail-count-then-park" => {
            let state_path = args.next().expect("state path");
            let failure_count = args
                .next()
                .expect("failure count")
                .parse::<u32>()
                .expect("numeric failure count");
            let exit_code = args
                .next()
                .expect("exit code")
                .parse::<i32>()
                .expect("numeric exit code");
            let attempts = std::fs::read_to_string(&state_path)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(0)
                + 1;
            std::fs::write(&state_path, attempts.to_string()).expect("record fixture attempt");
            if attempts <= failure_count {
                std::process::exit(exit_code);
            }
            loop {
                std::thread::park();
            }
        }
        "fail-count-then-healthy-exit-then-park" => {
            let state_path = args.next().expect("state path");
            let failure_count = args
                .next()
                .expect("failure count")
                .parse::<u32>()
                .expect("numeric failure count");
            let exit_code = args
                .next()
                .expect("exit code")
                .parse::<i32>()
                .expect("numeric exit code");
            let attempts = std::fs::read_to_string(&state_path)
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
                .unwrap_or(0)
                + 1;
            std::fs::write(&state_path, attempts.to_string()).expect("record fixture attempt");
            if attempts <= failure_count {
                std::process::exit(exit_code);
            }
            if attempts == failure_count + 1 {
                std::thread::sleep(Duration::from_secs(61));
                std::process::exit(0);
            }
            loop {
                std::thread::park();
            }
        }
        "never-ready" => loop {
            std::thread::park();
        },
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
        #[cfg(any(target_os = "linux", target_os = "macos"))]
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
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        "block-term-count" => std::process::exit(64),
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        "block-term-sleep" => {
            let ready_path = args.next().expect("ready path");
            let mut signals = nix::sys::signal::SigSet::empty();
            signals.add(nix::sys::signal::Signal::SIGTERM);
            signals.thread_block().expect("block SIGTERM");
            std::fs::write(ready_path, "ready").expect("signal readiness");
            std::thread::sleep(Duration::from_secs(30));
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        "block-term-sleep" => std::process::exit(64),
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

fn write_fixture_port_file(port_path: &std::path::Path) {
    let temporary = port_path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, "5015").expect("write fixture port");
    std::fs::rename(temporary, port_path).expect("replace fixture port");
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
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    let mut signals = nix::sys::signal::SigSet::empty();
                    signals.add(nix::sys::signal::Signal::SIGTERM);
                    signals.thread_block().expect("block SIGTERM");
                }
                #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                std::process::exit(64);
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
