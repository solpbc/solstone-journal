// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SERVICES: [&str; 4] = ["convey", "sense", "cortex", "spl"];

struct TempJournal(PathBuf);

impl TempJournal {
    fn new() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-supervisor-{stamp}"));
        fs::create_dir_all(root.join("config")).expect("config directory");
        fs::write(
            root.join("config/journal.json"),
            br#"{"setup":{"completed_at":1}}"#,
        )
        .expect("journal config");
        Self(root)
    }

    fn marker(&self, service: &str) -> PathBuf {
        self.0
            .join("health")
            .join(format!("fixture-{service}.marker"))
    }
}

impl Drop for TempJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChildGuard(Child);

impl ChildGuard {
    fn running(&mut self) -> bool {
        self.0.try_wait().expect("supervisor status").is_none()
    }

    fn terminate(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(self.0.id() as i32),
                nix::sys::signal::Signal::SIGTERM,
            )
            .expect("signal supervisor");
            for _ in 0..2_000 {
                if self.0.try_wait().expect("supervisor status").is_some() {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            panic!("supervisor did not exit after SIGTERM");
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.terminate();
        let _ = self.0.wait();
    }
}

fn start(journal: &TempJournal, args: &[&str], convey_argv: Option<String>) -> ChildGuard {
    let fixture = env!("CARGO_BIN_EXE_solstone-system-test-child");
    let mut command = Command::new(env!("CARGO_BIN_EXE_solstone-core"));
    command
        .args(["supervisor", "--journal"])
        .arg(&journal.0)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("SOLSTONE_LOCAL_BINARY", fixture)
        .env("SOLSTONE_SUPERVISOR_LOCAL_FIXTURE", "1")
        .env("SOLSTONE_PARAKEET_BINARY", fixture)
        .env("SOLSTONE_PARAKEET_MODEL", "test-ready")
        .env("SOLSTONE_SUPERVISOR_APP_FIXTURE", "1")
        .env("SOLSTONE_SUPERVISOR_APP_BINARY", fixture);
    if let Some(argv) = convey_argv {
        command.env("SOLSTONE_SUPERVISOR_APP_CONVEY_ARGV", argv);
    }
    ChildGuard(command.spawn().expect("supervisor starts"))
}

fn wait_for_markers(journal: &TempJournal, services: &[&str]) -> BTreeMap<String, Instant> {
    let mut observed = BTreeMap::new();
    for _ in 0..1_600 {
        for service in services {
            if !observed.contains_key(*service) && journal.marker(service).exists() {
                observed.insert((*service).to_owned(), Instant::now());
            }
        }
        if observed.len() == services.len() {
            return observed;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fixture markers did not appear: {observed:?}");
}

fn assert_marker_absent(journal: &TempJournal, service: &str) {
    for _ in 0..100 {
        assert!(
            !journal.marker(service).exists(),
            "unexpected {service} fixture marker"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_path(path: &Path) {
    for _ in 0..1_600 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("{} did not appear", path.display());
}

fn process_is_gone(pid: u32) -> bool {
    matches!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None),
        Err(nix::errno::Errno::ESRCH)
    )
}

fn fixture_pid(marker: &Path) -> u32 {
    fs::read_to_string(marker)
        .expect("read fixture marker")
        .trim()
        .rsplit(':')
        .next()
        .expect("fixture pid")
        .parse()
        .expect("numeric fixture pid")
}

fn fixture_process_running(parent_pid: u32, argument: &str) -> bool {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,command="])
        .output()
        .expect("list processes");
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let mut fields = line.split_whitespace();
        let _pid = fields.next();
        fields.next().and_then(|value| value.parse::<u32>().ok()) == Some(parent_pid)
            && fields.collect::<Vec<_>>().join(" ").contains(argument)
    })
}

#[test]
fn app_stack_markers_appear_in_launch_order() {
    let journal = TempJournal::new();
    let _child = start(&journal, &[], None);
    let observed = wait_for_markers(&journal, &SERVICES);
    assert!(observed["convey"] < observed["sense"]);
    assert!(observed["sense"] < observed["cortex"]);
    assert!(observed["cortex"] < observed["spl"]);
}

#[test]
fn never_ready_convey_waits_before_starting_the_remaining_stack() {
    let journal = TempJournal::new();
    let started = Instant::now();
    let mut child = start(&journal, &[], Some("sleep".to_owned()));
    let observed = wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(child.running());
    assert!(
        observed["sense"].duration_since(started) >= Duration::from_millis(100),
        "sense started before the Convey readiness wait"
    );
}

#[test]
fn convey_exit_during_startup_keeps_supervisor_running() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &[], Some("dies-on-startup".to_owned()));
    wait_for_markers(&journal, &["sense", "cortex", "spl"]);
    assert!(child.running());
}

#[test]
fn app_stack_opt_out_flags_suppress_only_their_service() {
    for (flag, absent, expected) in [
        ("--no-convey", "convey", &["sense", "cortex", "spl"][..]),
        ("--no-cortex", "cortex", &["convey", "sense", "spl"][..]),
        ("--no-spl", "spl", &["convey", "sense", "cortex"][..]),
    ] {
        let journal = TempJournal::new();
        let _child = start(&journal, &[flag], None);
        wait_for_markers(&journal, expected);
        assert_marker_absent(&journal, absent);
        assert!(journal.marker("sense").exists());
    }
}

#[test]
fn remote_mode_spawns_no_app_fixture_markers() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &["--remote", "https://example.test"], None);
    for service in SERVICES {
        assert_marker_absent(&journal, service);
    }
    assert!(child.running());
}

#[test]
fn app_fixture_receives_supervisor_spawned_environment() {
    let journal = TempJournal::new();
    let _child = start(&journal, &[], None);
    wait_for_markers(&journal, &["convey"]);
    assert!(
        fs::read_to_string(journal.marker("convey"))
            .expect("read Convey fixture marker")
            .contains("ready:1"),
        "fixture did not receive SOL_SUPERVISOR_SPAWNED=1"
    );
}

#[test]
fn shutdown_terminates_all_app_fixture_children() {
    let journal = TempJournal::new();
    let mut child = start(&journal, &[], None);
    wait_for_markers(&journal, &SERVICES);
    let pids = SERVICES
        .iter()
        .map(|service| fixture_pid(&journal.marker(service)))
        .collect::<Vec<_>>();
    child.terminate();
    assert!(pids.into_iter().all(process_is_gone));
}

#[test]
fn exited_convey_restarts_under_restart_policy() {
    let journal = TempJournal::new();
    let state_path = journal.0.join("restart-once");
    let convey_argv = format!("restart-once {}", state_path.display());
    let child = start(&journal, &[], Some(convey_argv));
    wait_for_path(&state_path);
    for _ in 0..200 {
        if fixture_process_running(child.0.id(), &state_path.display().to_string()) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("Convey fixture did not restart after its first exit");
}
