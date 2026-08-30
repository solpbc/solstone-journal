// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use solstone_core_backup_runtime::engine::{
    backup_path_resolution_attempts, reset_backup_path_resolution_attempts,
};
use solstone_core_backup_runtime::hosted_runtime::HttpError;
use solstone_core_backup_runtime::{
    BackupServices, Clock, HttpRequest, HttpResponse, HttpTransport, JournalMaintenance,
    JournalMaintenanceError, SystemToolRunner, ToolOutput, ToolRequest, ToolRunner,
    backup_journal_resolved_hook_armed, install_backup_journal_resolved_hook,
    reset_backup_journal_resolved_hook, run_backup, run_restic,
};

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

struct PanicHttp;
impl HttpTransport for PanicHttp {
    fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
        panic!("HTTP must not be reached")
    }
}

struct PanicRunner;
impl ToolRunner for PanicRunner {
    fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
        panic!("runner must not be reached")
    }
}

struct RecordingHttp {
    urls: RefCell<Vec<String>>,
}
impl HttpTransport for RecordingHttp {
    fn execute(&self, request: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.urls.borrow_mut().push(request.url.clone());
        Err(HttpError::Unreachable)
    }
}

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        50
    }
    fn iso_week(&self) -> u8 {
        7
    }
}

struct Maintenance;
impl JournalMaintenance for Maintenance {
    fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
    fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
        Ok(())
    }
}

fn configure_byo(journal: &Path) {
    let destination = solstone_core_backup::Destination {
        repository: "s3:repo".into(),
        backend: "s3".into(),
        credentials: serde_json::json!({
            "access_key_id": "access",
            "secret_access_key": "secret"
        })
        .as_object()
        .expect("credentials object")
        .clone(),
    };
    solstone_core_backup::set_destination(journal, &destination).expect("destination writes");
    solstone_core_backup::generate_and_store_keys(journal).expect("keys write");
    solstone_core_backup::set_enabled(journal, true).expect("backup enables");
}

fn backup_services<'a>(
    runner: &'a dyn ToolRunner,
    http: &'a dyn HttpTransport,
    clock: &'a dyn Clock,
    maintenance: &'a Maintenance,
) -> BackupServices<'a> {
    BackupServices {
        runner,
        http,
        clock,
        restic_path: Some(Path::new("/fixture/bin/restic")),
        rclone_path: Some(Path::new("/fixture/bin/rclone")),
        version: "test",
        journal_maintenance: maintenance,
    }
}

#[test]
fn external_hook_fires_between_canonical_resolution_and_config_read() {
    reset_backup_journal_resolved_hook();
    let source = tempfile::tempdir().expect("source journal creates");
    configure_byo(source.path());
    let replacement = tempfile::tempdir().expect("replacement journal creates");
    configure_byo(replacement.path());
    let sandbox = tempfile::tempdir().expect("sandbox creates");
    let alias = sandbox.path().join("journal");
    symlink(source.path(), &alias).expect("alias creates");

    let source_path = source.path().to_path_buf();
    let replacement_path = replacement.path().to_path_buf();
    let hook_alias = alias.clone();
    let firings = Rc::new(Cell::new(0));
    let hook_firings = Rc::clone(&firings);
    install_backup_journal_resolved_hook(move || {
        hook_firings.set(hook_firings.get() + 1);
        solstone_core_backup::set_mode(&source_path, "operated").expect("source mode changes");
        solstone_core_backup::save_hosted_binding(
            &source_path,
            &solstone_core_backup::HostedBinding {
                broker_endpoint: "https://source-broker.example".into(),
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "prefix".into(),
                broker_token: "token".into(),
            },
        )
        .expect("source binding writes");
        fs::remove_file(&hook_alias).expect("old alias removes");
        symlink(&replacement_path, &hook_alias).expect("alias retargets");
    });

    let runner = PanicRunner;
    let http = RecordingHttp {
        urls: RefCell::new(Vec::new()),
    };
    let clock = FixedClock;
    let maintenance = Maintenance;
    let result = run_backup(
        &alias,
        &backup_services(&runner, &http, &clock, &maintenance),
    );

    assert_eq!(firings.get(), 1);
    assert!(!backup_journal_resolved_hook_armed());
    assert_eq!(result.status, "error");
    assert_eq!(result.error_reason.as_deref(), Some("broker_unreachable"));
    assert_eq!(http.urls.borrow().len(), 1);
    assert!(http.urls.borrow()[0].starts_with("https://source-broker.example/"));
}

#[test]
fn second_hook_install_fails_without_replacing_the_first() {
    reset_backup_journal_resolved_hook();
    let firings = Rc::new(Cell::new(0));
    let hook_firings = Rc::clone(&firings);
    install_backup_journal_resolved_hook(move || hook_firings.set(hook_firings.get() + 1));
    let second = std::panic::catch_unwind(|| install_backup_journal_resolved_hook(|| {}));
    assert!(second.is_err());
    assert!(backup_journal_resolved_hook_armed());

    let journal = tempfile::tempdir().expect("journal creates");
    let runner = PanicRunner;
    let http = PanicHttp;
    let clock = FixedClock;
    let maintenance = Maintenance;
    let _ = run_backup(
        journal.path(),
        &backup_services(&runner, &http, &clock, &maintenance),
    );
    assert_eq!(firings.get(), 1);
    assert!(!backup_journal_resolved_hook_armed());
}

#[test]
fn failed_resolution_clears_hook_instead_of_leaking_to_the_next_run() {
    reset_backup_journal_resolved_hook();
    let firings = Rc::new(Cell::new(0));
    let hook_firings = Rc::clone(&firings);
    install_backup_journal_resolved_hook(move || hook_firings.set(hook_firings.get() + 1));

    let sandbox = tempfile::tempdir().expect("sandbox creates");
    let runner = PanicRunner;
    let http = PanicHttp;
    let clock = FixedClock;
    let maintenance = Maintenance;
    let services = backup_services(&runner, &http, &clock, &maintenance);
    let first = run_backup(&sandbox.path().join("missing"), &services);
    assert_eq!(
        first.error_reason.as_deref(),
        Some("journal_path_unresolved")
    );
    assert!(!backup_journal_resolved_hook_armed());

    let _ = run_backup(sandbox.path(), &services);
    assert_eq!(firings.get(), 0);
}

/// Binds a real Unix socket, so this lives in the `test-hooks` integration harness
/// rather than `engine.rs`'s inline unit tests, which the routine `make ci` unit
/// harness must stay free of hard-boundary (network) resources.
#[test]
fn non_directory_journal_roots_are_rejected_before_runtime_dependencies() {
    let sandbox = tempfile::tempdir().expect("test sandbox creates");
    let regular = sandbox.path().join("regular");
    fs::write(&regular, b"not a journal").expect("regular file writes");
    let fifo = sandbox.path().join("fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo starts")
            .success(),
        "mkfifo succeeds"
    );
    let socket = sandbox.path().join("socket");
    let _listener = UnixListener::bind(&socket).expect("socket binds");

    let runner = PanicRunner;
    let http = PanicHttp;
    let clock = FixedClock;
    let maintenance = Maintenance;
    let services = BackupServices {
        runner: &runner,
        http: &http,
        clock: &clock,
        restic_path: Some(Path::new("/fixture/bin/restic")),
        rclone_path: None,
        version: "test",
        journal_maintenance: &maintenance,
    };

    for journal in [&regular, &fifo, &socket] {
        reset_backup_path_resolution_attempts();
        let result = run_backup(journal, &services);
        assert_eq!(result.status, "error");
        assert_eq!(
            result.error_reason.as_deref(),
            Some("journal_path_unresolved")
        );
        assert_eq!(backup_path_resolution_attempts(), 1);
    }
    assert!(
        !sandbox.path().join("config").exists(),
        "a non-directory journal root must not create sibling config artifacts"
    );
}
