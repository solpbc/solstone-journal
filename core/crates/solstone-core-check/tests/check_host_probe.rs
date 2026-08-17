// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Real-host identity probe with an owned, bounded process group.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use solstone_core_check::gather_host_inputs;

const MODE: &str = "SOLSTONE_CHECK_HOST_PROBE_MODE";
const READY_FILE: &str = "SOLSTONE_CHECK_HOST_PROBE_READY_FILE";
const HOST_PROBE_BOUND: Duration = Duration::from_secs(25);
const FIXTURE_TIMEOUT: Duration = Duration::from_millis(50);
const GROUP_CLEANUP_BOUND: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq, Eq)]
enum WaitOutcome {
    Exited(ExitStatus),
    TimedOut,
}

struct ProcessGroupChild {
    child: Option<Child>,
    group: Pid,
}

impl ProcessGroupChild {
    fn spawn(mut command: Command) -> Self {
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let child = command.spawn().expect("spawn owned host-probe child");
        let group =
            Pid::from_raw(i32::try_from(child.id()).expect("child PID fits process-group ID"));
        Self {
            child: Some(child),
            group,
        }
    }

    fn wait_bounded(&mut self, timeout: Duration) -> WaitOutcome {
        let deadline = Instant::now() + timeout;
        loop {
            match self
                .child
                .as_mut()
                .expect("owned child")
                .try_wait()
                .expect("poll owned host-probe child")
            {
                Some(status) => {
                    self.child.take();
                    self.terminate_group();
                    self.assert_group_gone();
                    return WaitOutcome::Exited(status);
                }
                None if Instant::now() >= deadline => {
                    self.terminate_and_reap();
                    self.assert_group_gone();
                    return WaitOutcome::TimedOut;
                }
                None => thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    fn terminate_group(&self) {
        match killpg(self.group, Signal::SIGKILL) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => panic!("terminate host-probe process group: {error}"),
        }
    }

    fn terminate_and_reap(&mut self) {
        self.terminate_group();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn assert_group_gone(&self) {
        let deadline = Instant::now() + GROUP_CLEANUP_BOUND;
        loop {
            match killpg(self.group, None::<Signal>) {
                Err(Errno::ESRCH) => return,
                Ok(()) | Err(Errno::EPERM) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Ok(()) | Err(Errno::EPERM) => {
                    panic!("host-probe process group survived cleanup")
                }
                Err(error) => panic!("inspect host-probe process group: {error}"),
            }
        }
    }
}

impl Drop for ProcessGroupChild {
    fn drop(&mut self) {
        if self.child.is_some() {
            self.terminate_and_reap();
            self.assert_group_gone();
        }
    }
}

fn self_command(mode: &str, test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().expect("host-probe test executable"));
    command
        .args(["--exact", test, "--test-threads=1"])
        .env_clear()
        .env(MODE, mode);
    command
}

struct ReadyFile(std::path::PathBuf);

impl Drop for ReadyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn wait_for_ready(path: &std::path::Path) {
    let deadline = Instant::now() + GROUP_CLEANUP_BOUND;
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "host-probe descendant never reached its positive-control receipt"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn gathered_host_inputs_use_the_actual_platform_identity() {
    match std::env::var(MODE).ok().as_deref() {
        Some("probe") => {
            let inputs = gather_host_inputs(std::path::Path::new("."), "test-version");
            #[cfg(target_os = "linux")]
            assert_eq!(inputs.platform.os, "Linux");
            #[cfg(target_os = "macos")]
            assert_eq!(inputs.platform.os, "Darwin");
            let expected_arch = if cfg!(target_os = "macos") && std::env::consts::ARCH == "aarch64"
            {
                "arm64"
            } else {
                std::env::consts::ARCH
            };
            assert_eq!(inputs.platform.arch, expected_arch);
            assert!(
                !inputs.platform.os_version.is_empty(),
                "real host OS-version probe returned no identity"
            );
            assert_eq!(inputs.version, "test-version");
        }
        Some(mode) => panic!("unexpected host-probe mode {mode}"),
        None => {
            let mut child = ProcessGroupChild::spawn(self_command(
                "probe",
                "gathered_host_inputs_use_the_actual_platform_identity",
            ));
            let WaitOutcome::Exited(status) = child.wait_bounded(HOST_PROBE_BOUND) else {
                panic!("real host probe exceeded {HOST_PROBE_BOUND:?}");
            };
            assert!(status.success(), "real host probe child failed: {status}");
        }
    }
}

#[test]
fn host_probe_timeout_reaps_its_owned_descendant_group() {
    match std::env::var(MODE).ok().as_deref() {
        Some("hang-parent") => {
            let ready_file = std::env::var_os(READY_FILE).expect("descendant readiness file");
            let mut command = self_command(
                "hang-descendant",
                "host_probe_timeout_reaps_its_owned_descendant_group",
            );
            command.env(READY_FILE, ready_file);
            let mut descendant = command
                .spawn()
                .expect("spawn host-probe timeout descendant");
            let _ = descendant.wait();
        }
        Some("hang-descendant") => {
            let ready_file = std::env::var_os(READY_FILE).expect("descendant readiness file");
            std::fs::write(ready_file, b"ready").expect("write descendant readiness receipt");
            loop {
                thread::park();
            }
        }
        Some(mode) => panic!("unexpected host-probe timeout mode {mode}"),
        None => {
            let ready_file = ReadyFile(std::env::temp_dir().join(format!(
                "solstone-check-host-probe-{}.ready",
                std::process::id()
            )));
            let _ = std::fs::remove_file(&ready_file.0);
            let mut command = self_command(
                "hang-parent",
                "host_probe_timeout_reaps_its_owned_descendant_group",
            );
            command.env(READY_FILE, &ready_file.0);
            let mut child = ProcessGroupChild::spawn(command);
            wait_for_ready(&ready_file.0);
            assert_eq!(child.wait_bounded(FIXTURE_TIMEOUT), WaitOutcome::TimedOut);
        }
    }
}
