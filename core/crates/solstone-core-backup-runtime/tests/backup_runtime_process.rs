// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use solstone_core_backup_runtime::{SystemToolRunner, run_restic};

const OUTER_DEADLINE: Duration = Duration::from_secs(2);

fn run_bounded<T: Send + 'static>(body: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
        let _ = sender.send(result);
    });
    receiver
        .recv_timeout(OUTER_DEADLINE)
        .expect("test exceeded outer deadline")
        .unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

fn write_fixture(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn wait_until_dead(pid: i32) {
    let started = Instant::now();
    loop {
        match kill(Pid::from_raw(pid), None) {
            Ok(()) => {
                assert!(
                    started.elapsed() < Duration::from_secs(1),
                    "descendant {pid} still live"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(Errno::ESRCH) => return,
            Err(err) => panic!("kill({pid}, 0) failed: {err}"),
        }
    }
}

fn read_pid(path: &Path) -> i32 {
    fs::read_to_string(path)
        .unwrap()
        .trim()
        .parse()
        .expect("descendant pid")
}

#[test]
fn real_fixture_process_observes_whitelisted_environment() {
    run_bounded(|| {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        write_fixture(
            &fixture,
            "#!/bin/sh\nprintf '%s' \"$RESTIC_REPOSITORY:$RESTIC_PASSWORD:$LEAK\"\n",
        );
        let result = run_restic(
            &SystemToolRunner,
            &[],
            "repo",
            "password",
            &fixture,
            None,
            false,
            None,
            Some(Duration::from_secs(1)),
            &[],
        )
        .unwrap();
        assert_eq!(result.stdout, "repo:[redacted]:");
    });
}

#[test]
fn timeout_scrubs_partial_output_and_passes_live_key_fd() {
    run_bounded(|| {
        #[cfg(target_os = "macos")]
        let (timeout, return_ceiling) = (Duration::from_millis(600), Duration::from_millis(900));
        #[cfg(not(target_os = "macos"))]
        let (timeout, return_ceiling) = (Duration::from_millis(200), Duration::from_millis(400));
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        let pidfile = directory.path().join("sleep.pid");
        write_fixture(
            &fixture,
            "#!/bin/sh\nsleep 1 &\necho $! > \"$2\"\ncat /dev/fd/$1\nprintf ' PASSWORD' >&2\nwait\n",
        );
        let (reader, writer) = nix::unistd::pipe().unwrap();
        let mut writer = std::fs::File::from(writer);
        writer.write_all(b"PIPE_KEY").unwrap();
        drop(writer);
        let fd = reader.as_raw_fd();
        let started = Instant::now();
        let result = run_restic(
            &SystemToolRunner,
            &[fd.to_string(), pidfile.to_string_lossy().into_owned()],
            "repo",
            "PASSWORD",
            &fixture,
            None,
            true,
            None,
            Some(timeout),
            &[reader.as_fd()],
        )
        .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.returncode, 124);
        assert!(result.stdout.contains("PIPE_KEY"));
        assert_eq!(result.stderr, " [redacted]");
        assert_eq!(result.json, None);
        assert!(
            elapsed < return_ceiling,
            "timeout returned in {elapsed:?}, expected < {return_ceiling:?}"
        );
        wait_until_dead(read_pid(&pidfile));
    });
}

#[test]
fn natural_exit_terminates_orphaned_descendant() {
    run_bounded(|| {
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("fixture");
        let pidfile = directory.path().join("sleep.pid");
        write_fixture(&fixture, "#!/bin/sh\nsleep 5 &\necho $! > \"$1\"\n");
        let started = Instant::now();
        let result = run_restic(
            &SystemToolRunner,
            &[pidfile.to_string_lossy().into_owned()],
            "repo",
            "PASSWORD",
            &fixture,
            None,
            false,
            None,
            None,
            &[],
        )
        .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.returncode, 0);
        assert!(
            elapsed < Duration::from_millis(400),
            "natural exit returned in {elapsed:?}, expected < 400ms"
        );
        wait_until_dead(read_pid(&pidfile));
    });
}
