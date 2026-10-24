// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
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
        let fixture = directory.path().join("lib/solstone-restic/restic");
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
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
fn controlled_runner_probe() {
    if std::env::var("CONTROLLED_RUNNER_PROBE").as_deref() != Ok("active") {
        return;
    }
    use std::io::Read;
    use std::os::fd::FromRawFd;
    let descriptor = |name| std::env::var(name).unwrap().parse::<i32>().unwrap();
    // SAFETY: this child exclusively owns its explicitly inherited descriptors.
    #[allow(unsafe_code)]
    let (mut ready, mut control) = unsafe {
        (
            fs::File::from_raw_fd(descriptor("READY_FD")),
            fs::File::from_raw_fd(descriptor("CONTROL_FD")),
        )
    };
    ready.write_all(&std::process::id().to_le_bytes()).unwrap();
    let mut release = [0];
    control.read_exact(&mut release).unwrap();
    println!("controlled runner finished");
}

#[test]
fn controlled_system_runner_cancels_and_reaps_its_actual_root() {
    use solstone_core_backup::archive_admission::ArchiveCancellation;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    for cancelled in [false, true] {
        let (mut ready_parent, ready_child) = UnixStream::pair().unwrap();
        ready_parent
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut control_parent, control_child) = UnixStream::pair().unwrap();
        let request = ToolRequest {
            program: std::env::current_exe().unwrap().into_os_string(),
            argv: vec![
                "--exact".into(),
                "controlled_runner_probe".into(),
                "--nocapture".into(),
            ],
            env: std::collections::BTreeMap::from([
                ("CONTROLLED_RUNNER_PROBE".into(), "active".into()),
                (
                    "READY_FD".into(),
                    ready_child.as_raw_fd().to_string().into(),
                ),
                (
                    "CONTROL_FD".into(),
                    control_child.as_raw_fd().to_string().into(),
                ),
            ]),
            timeout: Some(Duration::from_secs(5)),
            pass_fds: vec![ready_child.as_fd(), control_child.as_fd()],
            stdin: None,
        };
        let cancellation = ArchiveCancellation::new();
        thread::scope(|scope| {
            let worker = scope.spawn(|| SystemToolRunner.run_cancellable(&request, &cancellation));
            let mut pid = [0; 4];
            ready_parent.read_exact(&mut pid).unwrap();
            let pid = Pid::from_raw(u32::from_le_bytes(pid) as i32);
            if cancelled {
                cancellation.cancel();
            } else {
                control_parent.write_all(b"r").unwrap();
            }
            let result = worker.join().unwrap();
            if cancelled {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
            } else {
                let output = result.unwrap();
                assert_eq!(output.returncode, 0);
                assert!(
                    String::from_utf8(output.stdout)
                        .unwrap()
                        .contains("controlled runner finished")
                );
            }
            assert_eq!(
                nix::sys::wait::waitpid(pid, Some(nix::sys::wait::WaitPidFlag::WNOHANG)),
                Err(Errno::ECHILD)
            );
        });
    }
}

#[test]
fn timeout_scrubs_partial_output_and_passes_live_key_fd() {
    run_bounded(|| {
        #[cfg(target_os = "macos")]
        let (timeout, return_ceiling) = (Duration::from_millis(600), Duration::from_millis(900));
        #[cfg(not(target_os = "macos"))]
        let (timeout, return_ceiling) = (Duration::from_millis(200), Duration::from_millis(400));
        let directory = tempfile::tempdir().unwrap();
        let fixture = directory.path().join("lib/solstone-restic/restic");
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
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
        let fixture = directory.path().join("lib/solstone-restic/restic");
        fs::create_dir_all(fixture.parent().unwrap()).unwrap();
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

#[test]
#[ignore = "requires an admitted copied package and synthetic direct S3 fixture"]
fn packaged_owned_archive_entry() {
    use solstone_core_backup::archive_activity::PreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{
        AdmissionError, ArchiveBudget, ArchiveCancellation,
    };
    use solstone_core_backup::archive_readback::ReadbackError;
    use solstone_core_backup_runtime::admit_prepared_archive;
    use solstone_core_journal_io::{
        JournalRoot,
        media::{Cleanup, MediaDirectory, MediaStage, digest_reader},
    };
    use std::os::unix::fs::MetadataExt;
    use std::sync::Arc;

    struct ProbeClock(Cell<i64>);
    impl Clock for ProbeClock {
        fn now_unix(&self) -> i64 {
            self.0.get()
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct ProbeRunner;
    impl ProbeRunner {
        fn record(result: std::io::Result<ToolOutput>) -> std::io::Result<ToolOutput> {
            if let Ok(output) = &result
                && output.returncode != 0
            {
                eprintln!(
                    "OWNED_ARCHIVE_TOOL_FAILURE {}",
                    serde_json::json!({"returncode":output.returncode,
                        "stdout":String::from_utf8_lossy(&output.stdout),
                        "stderr":String::from_utf8_lossy(&output.stderr)})
                );
            }
            result
        }
    }
    impl ToolRunner for ProbeRunner {
        fn run(&self, request: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            Self::record(SystemToolRunner.run(request))
        }
        fn run_cancellable(
            &self,
            request: &ToolRequest<'_>,
            cancellation: &ArchiveCancellation,
        ) -> std::io::Result<ToolOutput> {
            Self::record(SystemToolRunner.run_cancellable(request, cancellation))
        }
    }
    let input_path =
        std::env::var_os("OWNED_ARCHIVE_PROBE_JSON").expect("explicit fixture required");
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let member =
        solstone_core_installed_payload::resolve_current_member("lib/solstone-restic/restic")
            .unwrap();
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let clock = ProbeClock(Cell::new(started));
    let cancellation = ArchiveCancellation::new();
    let http = RecordingHttp {
        urls: RefCell::new(Vec::new()),
    };
    let services = BackupServices {
        runner: &ProbeRunner,
        http: &http,
        clock: &clock,
        restic: Some((member.path()).into()),
        rclone_path: None,
        version: "2.0.40",
        journal_maintenance: &Maintenance,
    };
    let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
    let operation = PreparedArchiveOperation::prepare(root.clone(), budget, &clock).unwrap();
    let carrier = operation.carrier().clone();
    let activity = operation.activity().clone();
    let carrier_identity = || {
        let observed = fs::metadata(carrier.diagnostic_path()).unwrap();
        (observed.dev(), observed.ino())
    };
    let original_carrier_identity = carrier_identity();
    let parent = MediaDirectory::open(root.clone(), Path::new("media")).unwrap();
    let fresh = || {
        MediaStage::create(parent.clone(), activity.clone())
            .unwrap()
            .create_target()
            .unwrap()
    };
    let original_files = input["files"].as_array().unwrap();
    let copy = |index: usize| {
        let file = &original_files[index];
        let name = std::ffi::OsStr::new(file["name"].as_str().unwrap());
        let bytes = fs::read(root.canonical_path().join("media").join(name)).unwrap();
        let digest = digest_reader(&mut bytes.as_slice()).unwrap();
        assert_eq!(digest.sha256, file["sha256"].as_str().unwrap());
        assert_eq!(digest.size, file["bytes"].as_u64().unwrap());
        MediaStage::create(parent.clone(), activity.clone())
            .unwrap()
            .create_named_target(name)
            .unwrap()
            .copy_from(&parent, name, &digest)
            .unwrap()
    };
    assert_eq!(original_files.len(), 2);
    let mut prepared_sources = vec![copy(0), copy(1)];
    let mut duplicate_one = copy(0);
    let mut duplicate_two = copy(0);
    let mut wrong_content = {
        let name = std::ffi::OsStr::new(original_files[0]["name"].as_str().unwrap());
        let path = root.canonical_path().join("media").join(name);
        let original = fs::read(&path).unwrap();
        let mut wrong = original.clone();
        wrong[0] ^= 1;
        fs::write(&path, &wrong).unwrap();
        let digest = digest_reader(&mut wrong.as_slice()).unwrap();
        let prepared = MediaStage::create(parent.clone(), activity.clone())
            .unwrap()
            .create_named_target(name)
            .unwrap()
            .copy_from(&parent, name, &digest)
            .unwrap();
        fs::write(&path, original).unwrap();
        prepared
    };
    let sources = prepared_sources
        .iter_mut()
        .map(|p| p.archive_source().unwrap())
        .collect::<Vec<_>>();
    let duplicates = [
        duplicate_one.archive_source().unwrap(),
        duplicate_two.archive_source().unwrap(),
    ];
    let wrong_source = wrong_content.archive_source().unwrap();
    let initial_fd_flags = sources
        .iter()
        .map(|source| {
            source
                .descriptor()
                .map(|fd| nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFD).unwrap())
        })
        .collect::<Vec<_>>();
    for flags in initial_fd_flags.iter().flatten() {
        assert_ne!(flags & nix::fcntl::FdFlag::FD_CLOEXEC.bits(), 0);
    }
    let mut archive = admit_prepared_archive(operation, &services).unwrap();
    assert!(Arc::ptr_eq(archive.carrier(), &carrier));
    archive.carrier().revalidate().unwrap();
    assert_eq!(carrier_identity(), original_carrier_identity);
    assert_eq!(
        archive.repository_id(),
        input["repository_id"].as_str().unwrap()
    );
    let snapshot = archive.archive_owned(&sources).unwrap();
    let duplicate_snapshot = archive.archive_owned(&duplicates).unwrap();
    let mut reader = archive.into_read_operation().unwrap();
    assert!(Arc::ptr_eq(reader.carrier(), &carrier));
    assert!(Arc::ptr_eq(reader.activity(), &activity));
    reader.carrier().revalidate().unwrap();
    assert_eq!(carrier_identity(), original_carrier_identity);
    let mut results = Vec::new();
    for source in &sources {
        let (exact, mut prepared) = reader
            .read_owned_snapshot(&snapshot.snapshot_id, source, fresh())
            .unwrap();
        assert_eq!(prepared.digest(), source.digest());
        assert_eq!(exact.name, source.name().to_str().unwrap());
        assert_eq!(exact.sha256, source.digest().sha256);
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
        results.push(
            serde_json::json!({"name":exact.name, "archive_path":exact.archive_path,
            "bytes":exact.bytes, "sha256":exact.sha256}),
        );
    }
    let mut duplicate = reader
        .read_owned_snapshot(&duplicate_snapshot.snapshot_id, &sources[0], fresh())
        .err()
        .unwrap();
    assert_eq!(duplicate.reason(), ReadbackError::SnapshotNodeUnavailable);
    assert_eq!(duplicate.cleanup(), Cleanup::Complete);
    let mut wrong = reader
        .read_owned_snapshot(&snapshot.snapshot_id, &wrong_source, fresh())
        .err()
        .unwrap();
    assert_eq!(wrong.reason(), ReadbackError::NativeFailure);
    assert_eq!(wrong.cleanup(), Cleanup::Complete);
    let mut issued_target = fresh();
    drop(issued_target.helper_transport().unwrap());
    let mut issued = reader
        .read_owned_snapshot(&snapshot.snapshot_id, &sources[0], issued_target)
        .err()
        .unwrap();
    assert_eq!(issued.reason(), ReadbackError::NativeFailure);
    assert_eq!(issued.cleanup(), Cleanup::Refused);
    let mut invalid = reader
        .read_owned_snapshot("latest", &sources[0], fresh())
        .err()
        .unwrap();
    assert_eq!(invalid.reason(), ReadbackError::InvalidSelection);
    assert_eq!(invalid.cleanup(), Cleanup::Complete);
    let mut changed_bytes = fs::read(wrong_source.path()).unwrap();
    changed_bytes[0] ^= 2;
    fs::write(wrong_source.path(), changed_bytes).unwrap();
    let mut changed = reader
        .read_owned_snapshot(&snapshot.snapshot_id, &wrong_source, fresh())
        .err()
        .unwrap();
    assert_eq!(changed.reason(), ReadbackError::NativeFailure);
    assert_eq!(changed.cleanup(), Cleanup::Complete);
    clock.0.set(
        started + solstone_core_backup::archive_admission::ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64,
    );
    let mut exhausted = reader
        .read_owned_snapshot(&snapshot.snapshot_id, &sources[0], fresh())
        .err()
        .unwrap();
    assert_eq!(
        exhausted.reason(),
        ReadbackError::Admission(AdmissionError::DeadlineExceeded)
    );
    assert_eq!(exhausted.cleanup(), Cleanup::Complete);
    cancellation.cancel();
    let mut cancelled = reader
        .read_owned_snapshot(&snapshot.snapshot_id, &sources[0], fresh())
        .err()
        .unwrap();
    assert_eq!(
        cancelled.reason(),
        ReadbackError::Admission(AdmissionError::Cancelled)
    );
    assert_eq!(cancelled.cleanup(), Cleanup::Complete);
    for (source, before) in sources.iter().zip(&initial_fd_flags) {
        source.revalidate().unwrap();
        assert_eq!(
            source
                .descriptor()
                .map(|fd| nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFD).unwrap()),
            *before
        );
    }
    assert!(http.urls.borrow().is_empty());
    println!(
        "OWNED_ARCHIVE_RESULT {}",
        serde_json::json!({"snapshot_id":snapshot.snapshot_id,
        "duplicate_snapshot_id":duplicate_snapshot.snapshot_id, "repository_id":snapshot.repository_id,
        "files":results, "original_budget_exhausted":true, "original_stop_observed":true,
        "duplicate_and_wrong_bytes_refused":true, "issued_target_cleanup":"refused",
        "original_carrier_identity":{"device":original_carrier_identity.0,
            "inode":original_carrier_identity.1},"same_physical_carrier":true})
    );
    drop(sources);
    drop(duplicates);
    drop(wrong_source);
    for source in &mut prepared_sources {
        assert_eq!(source.cleanup(), Cleanup::Complete);
    }
    assert_eq!(duplicate_one.cleanup(), Cleanup::Complete);
    assert_eq!(duplicate_two.cleanup(), Cleanup::Complete);
    // Deliberately mutated owned input remains subject to Native cleanup refusal.
    let _ = wrong_content.cleanup();
}

#[test]
#[cfg(feature = "full-tests")]
#[ignore = "requires an admitted copied package and synthetic direct S3 race fixture"]
fn packaged_owned_archive_race_entry() {
    use solstone_core_backup::archive_activity::PreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{
        ARCHIVE_BACKUP_TIMEOUT_SECONDS, AdmissionError, ArchiveBudget, ArchiveCancellation,
    };
    use solstone_core_backup::archive_readback::ReadbackError;
    use solstone_core_backup_runtime::admit_prepared_archive;
    use solstone_core_journal_io::{
        JournalRoot,
        media::{Cleanup, MediaDigest, MediaDirectory, MediaStage, digest_reader},
    };
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;
    use std::sync::Arc;

    enum Cut {
        Discovery {
            barrier: PathBuf,
            source: PathBuf,
            change: bool,
        },
        Dump {
            target: PathBuf,
            source: PathBuf,
            expected: MediaDigest,
        },
        Cancel {
            source: PathBuf,
            cancellation: ArchiveCancellation,
        },
        Deadline {
            source: PathBuf,
            deadline: i64,
        },
    }
    struct RaceClock {
        now: Cell<i64>,
        cut: RefCell<Option<Cut>>,
        fired: Cell<bool>,
        mutation: RefCell<Option<serde_json::Value>>,
    }
    impl RaceClock {
        fn mutate(&self, source: &Path) {
            let before = fs::metadata(source).unwrap();
            let mut bytes = fs::read(source).unwrap();
            bytes[0] ^= 1;
            fs::write(source, bytes).unwrap();
            let after = fs::metadata(source).unwrap();
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
            *self.mutation.borrow_mut() = Some(serde_json::json!({
                "device":before.dev().to_string(), "inode":before.ino().to_string(),
                "same_native_identity":true
            }));
        }
    }
    impl Clock for RaceClock {
        fn now_unix(&self) -> i64 {
            let sampled = self.now.get();
            let cut = self.cut.borrow_mut().take();
            if let Some(cut) = cut {
                let ready = match &cut {
                    Cut::Discovery { barrier, .. } => barrier.is_file(),
                    Cut::Dump {
                        target, expected, ..
                    } => fs::metadata(target).is_ok_and(|m| m.len() == expected.size),
                    Cut::Cancel { .. } | Cut::Deadline { .. } => true,
                };
                if !ready {
                    *self.cut.borrow_mut() = Some(cut);
                } else {
                    self.fired.set(true);
                    match cut {
                        Cut::Discovery { source, change, .. } => {
                            if change {
                                self.mutate(&source);
                            }
                        }
                        Cut::Dump {
                            target,
                            source,
                            expected,
                        } => {
                            let bytes = fs::read(target).unwrap();
                            assert_eq!(digest_reader(&mut bytes.as_slice()).unwrap(), expected);
                            self.mutate(&source);
                        }
                        Cut::Cancel {
                            source,
                            cancellation,
                        } => {
                            self.mutate(&source);
                            cancellation.cancel();
                        }
                        Cut::Deadline { source, deadline } => {
                            self.mutate(&source);
                            self.now.set(deadline);
                        }
                    }
                }
            }
            // Preserve this pre-observation sample; a deadline cut expires the
            // next sample after the real Native observation has failed.
            sampled
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }

    let input_path = std::env::var_os("OWNED_ARCHIVE_PROBE_JSON").unwrap();
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let scenario = input["scenario"].as_str().unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let clock = RaceClock {
        now: Cell::new(started),
        cut: RefCell::new(None),
        fired: Cell::new(false),
        mutation: RefCell::new(None),
    };
    let cancellation = ArchiveCancellation::new();
    let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
    let operation = PreparedArchiveOperation::prepare(root.clone(), budget, &clock).unwrap();
    let carrier = operation.carrier().clone();
    let activity = operation.activity().clone();
    let member =
        solstone_core_installed_payload::resolve_current_member("lib/solstone-restic/restic")
            .unwrap();
    let http = RecordingHttp {
        urls: RefCell::new(Vec::new()),
    };
    let services = BackupServices {
        runner: &SystemToolRunner,
        http: &http,
        clock: &clock,
        restic: Some((member.path()).into()),
        rclone_path: None,
        version: "2.0.40",
        journal_maintenance: &Maintenance,
    };
    let parent = MediaDirectory::open(root.clone(), Path::new("media")).unwrap();
    let file = &input["files"][0];
    let name = std::ffi::OsStr::new(file["name"].as_str().unwrap());
    let original = fs::read(root.canonical_path().join("media").join(name)).unwrap();
    let digest = digest_reader(&mut original.as_slice()).unwrap();
    assert_eq!(digest.sha256, file["sha256"].as_str().unwrap());
    assert_eq!(digest.size, file["bytes"].as_u64().unwrap());
    let source_stage = MediaStage::create(parent.clone(), activity.clone()).unwrap();
    let retained_source_path = source_stage.diagnostic_path().join(name);
    let mut copied = source_stage
        .create_named_target(name)
        .unwrap()
        .copy_from(&parent, name, &digest)
        .unwrap();
    let source = copied.archive_source().unwrap();
    let mut archive = admit_prepared_archive(operation, &services).unwrap();
    assert!(Arc::ptr_eq(archive.carrier(), &carrier));
    assert_eq!(
        archive.repository_id(),
        input["repository_id"].as_str().unwrap()
    );
    let snapshot = archive
        .archive_owned(std::slice::from_ref(&source))
        .unwrap();
    let mut reader = archive.into_read_operation().unwrap();
    assert!(Arc::ptr_eq(reader.carrier(), &carrier));
    assert!(Arc::ptr_eq(reader.activity(), &activity));
    let stage = MediaStage::create(parent, activity.clone()).unwrap();
    let target_path = stage.diagnostic_path().join("payload");
    let target = stage.create_target().unwrap();
    assert!(!target.helper_transport_issued());
    let source_path = source.path().to_owned();
    let barrier = PathBuf::from(input["barrier"].as_str().unwrap());
    let selected_snapshot = if scenario == "oversize" {
        input["oversized_snapshot_id"].as_str().unwrap()
    } else {
        &snapshot.snapshot_id
    };
    *clock.cut.borrow_mut() = match scenario {
        "discovery_positive" | "discovery_change" => {
            assert!(!barrier.exists());
            assert!(!Path::new(input["arm"].as_str().unwrap()).exists());
            fs::write(
                input["arm"].as_str().unwrap(),
                serde_json::to_vec(&serde_json::json!({"snapshot_id":selected_snapshot})).unwrap(),
            )
            .unwrap();
            Some(Cut::Discovery {
                barrier: barrier.clone(),
                source: source_path,
                change: scenario == "discovery_change",
            })
        }
        "dump_change" => Some(Cut::Dump {
            target: target_path.clone(),
            source: source_path,
            expected: digest.clone(),
        }),
        "native_cancel" => Some(Cut::Cancel {
            source: source_path,
            cancellation: cancellation.clone(),
        }),
        "native_deadline" => Some(Cut::Deadline {
            source: source_path,
            deadline: started + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64,
        }),
        "oversize" => None,
        _ => panic!("unknown explicit race scenario"),
    };
    let before_native = reader.source_observations_for_test();
    let result = reader.read_owned_snapshot(selected_snapshot, &source, target);
    let after_native = reader.source_observations_for_test();
    if matches!(scenario, "native_cancel" | "native_deadline") {
        assert_eq!(after_native, (before_native.0 + 1, before_native.1 + 1));
    }
    let observation = if scenario == "discovery_positive" {
        let (exact, mut prepared) = result.unwrap();
        assert_eq!(prepared.digest(), &digest);
        assert!(clock.fired.get());
        source.revalidate().unwrap();
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
        serde_json::json!({"archive_path":exact.archive_path, "bytes":exact.bytes,
            "sha256":exact.sha256, "target_cleanup":"complete", "source_valid":true})
    } else {
        let mut failure = result.err().unwrap();
        let expected = match scenario {
            "dump_change" | "discovery_change" => ReadbackError::NativeFailure,
            "native_cancel" => ReadbackError::Admission(AdmissionError::Cancelled),
            "native_deadline" => ReadbackError::Admission(AdmissionError::DeadlineExceeded),
            "oversize" => ReadbackError::ToolFailed,
            _ => unreachable!(),
        };
        assert_eq!(failure.reason(), expected);
        assert_eq!(clock.fired.get(), scenario != "oversize");
        let prepared = failure.prepared_digest_for_test().cloned();
        let issued = failure.target_transport_issued_for_test();
        if scenario == "dump_change" {
            assert_eq!(prepared.as_ref(), Some(&digest));
            assert_eq!(issued, None);
            let bytes = fs::read(&target_path).unwrap();
            assert_eq!(digest_reader(&mut bytes.as_slice()).unwrap(), digest);
        } else {
            assert_eq!(prepared, None);
            assert_eq!(issued, Some(false));
            assert_eq!(fs::metadata(&target_path).unwrap().len(), 0);
        }
        assert_eq!(source.revalidate().is_err(), scenario != "oversize");
        assert_eq!(failure.cleanup(), Cleanup::Complete);
        serde_json::json!({"reason":format!("{:?}",failure.reason()),
            "prepared_digest":prepared, "target_transport_issued":issued,
            "target_cleanup":"complete", "source_valid":scenario=="oversize"})
    };
    if scenario.starts_with("discovery_") {
        let event: serde_json::Value = serde_json::from_slice(&fs::read(barrier).unwrap()).unwrap();
        assert_eq!(event["snapshot_id"], selected_snapshot);
    }
    assert!(http.urls.borrow().is_empty());
    drop(source);
    let source_cleanup = if scenario == "discovery_positive" || scenario == "oversize" {
        assert_eq!(copied.cleanup(), Cleanup::Complete);
        "complete"
    } else {
        // Keep the intentionally mutated private copy as qualification evidence.
        // Native exact-identity cleanup is not an immutability observation.
        assert!(retained_source_path.is_file());
        "not_requested"
    };
    println!(
        "OWNED_ARCHIVE_RACE_RESULT {}",
        serde_json::json!({"scenario":scenario, "snapshot_id":snapshot.snapshot_id,
            "selected_snapshot_id":selected_snapshot, "repository_id":snapshot.repository_id,
            "cut_fired":clock.fired.get(), "mutation":*clock.mutation.borrow(),
            "observation":observation, "source_cleanup":source_cleanup,
            "native_observations_before":before_native, "native_observations_after":after_native,
            "retained_source_path":retained_source_path})
    );
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
        restic: Some((Path::new("/fixture/bin/restic")).into()),
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
        restic: Some((Path::new("/fixture/bin/restic")).into()),
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

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an admitted copied package and synthetic localhost HTTP fixtures"]
fn packaged_controlled_http_entry() {
    use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation};
    use solstone_core_backup::owned_dns::{OwnedDnsFamily, resolve_owned_http_host};
    use solstone_core_backup_runtime::{UreqHttpTransport, admit_archive_with_budget};
    use solstone_core_journal_io::JournalRoot;
    use std::io::Read;
    use std::net::ToSocketAddrs;
    use std::os::fd::FromRawFd;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct ProbeClock {
        base: AtomicI64,
        started: Instant,
    }
    impl Clock for ProbeClock {
        fn now_unix(&self) -> i64 {
            self.base.load(Ordering::SeqCst) + self.started.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct CountedRepositoryConfig(Cell<usize>);
    impl ToolRunner for CountedRepositoryConfig {
        fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            panic!("owning admission must preserve its controlled runner token")
        }
        fn run_cancellable(
            &self,
            request: &ToolRequest<'_>,
            _: &ArchiveCancellation,
        ) -> std::io::Result<ToolOutput> {
            assert!(request.argv.iter().any(|arg| arg == "cat"));
            assert!(request.argv.iter().any(|arg| arg == "config"));
            self.0.set(self.0.get() + 1);
            Ok(ToolOutput {
                returncode: 0,
                stdout: serde_json::to_vec(&serde_json::json!({"id":"a".repeat(64)})).unwrap(),
                stderr: vec![],
            })
        }
    }
    struct BrokerHttp<'a> {
        clock: &'a ProbeClock,
        after_http: &'a str,
        observed: RefCell<Option<String>>,
    }
    impl HttpTransport for BrokerHttp<'_> {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("ordinary HTTP must not be reached")
        }
        fn execute_cancellable(
            &self,
            request: &HttpRequest,
            cancellation: &ArchiveCancellation,
        ) -> Result<HttpResponse, HttpError> {
            let result = UreqHttpTransport.execute_cancellable(request, cancellation);
            *self.observed.borrow_mut() = Some(match &result {
                Ok(response) => format!("status:{}", response.status),
                Err(error) => format!("{error:?}"),
            });
            if self.after_http == "cancel" {
                cancellation.cancel();
            }
            if self.after_http == "expire" {
                self.clock.base.store(
                    10_000
                        + solstone_core_backup::archive_admission::ARCHIVE_BACKUP_TIMEOUT_SECONDS
                            as i64,
                    Ordering::SeqCst,
                );
            }
            result
        }
    }
    let input_path =
        std::env::var_os("CONTROLLED_HTTP_PROBE_JSON").expect("explicit fixture required");
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let cancellation = ArchiveCancellation::new();
    if input["pre_cancel"].as_bool().unwrap_or(false) {
        cancellation.cancel();
    }
    let control_fd = input["control_fd"]
        .as_i64()
        .map(|fd| i32::try_from(fd).unwrap());
    let started = Instant::now();
    let result = thread::scope(|scope| {
        let publisher = control_fd.map(|fd| {
            // SAFETY: this isolated probe exclusively owns the explicitly inherited socket.
            #[allow(unsafe_code)]
            let mut control = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            control
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let cancellation = cancellation.clone();
            scope.spawn(move || {
                let mut byte = [0];
                control.read_exact(&mut byte).unwrap();
                if byte == *b"c" {
                    cancellation.cancel();
                } else {
                    assert_eq!(byte, *b"r");
                }
            })
        });
        let execute = || match input["kind"].as_str().unwrap() {
            "dns" => {
                let host = input["host"].as_str().unwrap();
                let port = input["port"].as_u64().unwrap() as u16;
                let result = resolve_owned_http_host(
                    host,
                    port,
                    OwnedDnsFamily::Any,
                    Instant::now() + Duration::from_secs(5),
                    &cancellation,
                );
                match result {
                    Ok(addresses) => {
                        let native = format!("{host}:{port}")
                            .to_socket_addrs()
                            .unwrap()
                            .take(16)
                            .collect::<Vec<_>>();
                        assert_eq!(addresses, native);
                        serde_json::json!({"addresses":addresses.iter().map(ToString::to_string).collect::<Vec<_>>()})
                    }
                    Err(error) => serde_json::json!({"error":format!("{:?}",error.kind())}),
                }
            }
            "http" => {
                let request = HttpRequest {
                    method: input["method"].as_str().unwrap().into(),
                    url: input["url"].as_str().unwrap().into(),
                    headers: input["headers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|pair| {
                            (
                                pair[0].as_str().unwrap().into(),
                                pair[1].as_str().unwrap().into(),
                            )
                        })
                        .collect(),
                    body: input["body"].as_str().unwrap().as_bytes().to_vec(),
                    timeout: Duration::from_millis(input["timeout_ms"].as_u64().unwrap()),
                };
                match UreqHttpTransport.execute_cancellable(&request, &cancellation) {
                    Ok(response) => {
                        serde_json::json!({"status":response.status,"headers":response.headers,"body":String::from_utf8(response.body).unwrap()})
                    }
                    Err(error) => serde_json::json!({"error":format!("{error:?}")}),
                }
            }
            "broker" => {
                let root = Arc::new(
                    JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap(),
                );

                let runner = CountedRepositoryConfig(Cell::new(0));
                let restic = solstone_core_installed_payload::resolve_current_member(
                    "lib/solstone-restic/restic",
                )
                .unwrap();
                let rclone = solstone_core_installed_payload::resolve_current_member(
                    "lib/solstone-rclone/rclone",
                )
                .unwrap();
                let clock = ProbeClock {
                    base: AtomicI64::new(10_000),
                    started: Instant::now(),
                };
                let http = BrokerHttp {
                    clock: &clock,
                    after_http: input["after_http"].as_str().unwrap_or(""),
                    observed: RefCell::new(None),
                };
                let services = BackupServices {
                    runner: &runner,
                    http: &http,
                    clock: &clock,
                    restic: Some((restic.path()).into()),
                    rclone_path: Some(rclone.path()),
                    version: "2.0.40",
                    journal_maintenance: &Maintenance,
                };
                let budget = ArchiveBudget::new_cancellable(&clock, cancellation.clone()).unwrap();
                if let Some(remaining) = input["original_remaining_seconds"].as_i64() {
                    clock.base.store(10_000 + solstone_core_backup::archive_admission::ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - remaining, Ordering::SeqCst);
                }
                let result = admit_archive_with_budget(root, &services, budget);
                let outcome = match result {
                    Ok(archive) => serde_json::json!({"repository_id":archive.repository_id()}),
                    Err(error) => serde_json::json!({"error":format!("{error:?}")}),
                };
                serde_json::json!({"outcome":outcome,"controlled_http_result":http.observed.borrow().clone(),"repository_config_calls":runner.0.get(),"runner":"synthetic counted config only; no native launch"})
            }
            _ => panic!("unknown explicit probe fixture"),
        };
        let result = if input["nested_runtime"].as_bool().unwrap_or(false) {
            tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .unwrap()
                .block_on(async { execute() })
        } else {
            execute()
        };
        if let Some(publisher) = publisher {
            publisher.join().unwrap();
        }
        result
    });
    println!(
        "CONTROLLED_HTTP_RESULT {}",
        serde_json::json!({"elapsed_ms":started.elapsed().as_millis(),"cancelled":cancellation.is_cancelled(),"result":result})
    );
}

#[cfg(all(target_os = "linux", feature = "full-tests"))]
#[test]
#[ignore = "requires an admitted copied package and synthetic direct-read broker fixture"]
fn packaged_direct_archive_read_entry() {
    use solstone_core_backup::archive_activity::PreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{
        ARCHIVE_BACKUP_TIMEOUT_SECONDS, ArchiveBudget, ArchiveCancellation,
    };
    use solstone_core_backup_runtime::{UreqHttpTransport, admit_prepared_archive_read};
    use solstone_core_journal_io::JournalRoot;
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    struct OriginalClock {
        base: AtomicI64,
        started: Instant,
    }
    impl Clock for OriginalClock {
        fn now_unix(&self) -> i64 {
            self.base.load(Ordering::SeqCst) + self.started.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    struct NoGenericRunner;
    impl ToolRunner for NoGenericRunner {
        fn run(&self, _: &ToolRequest<'_>) -> std::io::Result<ToolOutput> {
            panic!("direct installed read must not call the generic runner")
        }
    }
    let input_path = std::env::var_os("DIRECT_READ_PROBE_JSON").expect("explicit fixture required");
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let expected_path = Path::new(input["restic_path"].as_str().unwrap());
    let original = OriginalClock {
        base: AtomicI64::new(10_000),
        started: Instant::now(),
    };
    let replacement = OriginalClock {
        base: AtomicI64::new(1_999_999_999),
        started: Instant::now(),
    };
    let cancellation = ArchiveCancellation::new();
    let budget = ArchiveBudget::new_cancellable(&original, cancellation.clone()).unwrap();
    let remaining = input["remaining_seconds"].as_i64().unwrap_or(8);
    original.base.store(
        10_000 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64 - remaining,
        Ordering::SeqCst,
    );
    let operation = PreparedArchiveOperation::prepare(root.clone(), budget, &original).unwrap();
    let carrier = operation.carrier().clone();
    let identity = fs::metadata(carrier.diagnostic_path()).unwrap();
    let physical = (identity.dev(), identity.ino());
    let started = Instant::now();
    let observed = thread::scope(|scope| {
        let publisher = input["control_fd"].as_i64().map(|raw| {
            // SAFETY: this isolated probe owns this explicitly inherited socket.
            #[allow(unsafe_code)]
            let mut control =
                unsafe { std::os::unix::net::UnixStream::from_raw_fd(i32::try_from(raw).unwrap()) };
            control
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let original = &original;
            let cancellation = &cancellation;
            let root = &root;
            scope.spawn(move || {
                let mut byte = [0];
                control.read_exact(&mut byte).unwrap();
                match byte[0] {
                    b'c' => cancellation.cancel(),
                    b'e' => original.base.store(
                        10_000 + ARCHIVE_BACKUP_TIMEOUT_SECONDS as i64,
                        Ordering::SeqCst,
                    ),
                    b'f' => {
                        solstone_core_backup::set_enabled(root.canonical_path(), false).unwrap();
                    }
                    b'r' => {}
                    _ => panic!("unknown fixture control"),
                }
                control.write_all(b"a").unwrap();
            })
        });
        let services = BackupServices {
            runner: &NoGenericRunner,
            http: &UreqHttpTransport,
            clock: &replacement,
            restic: Some((expected_path).into()),
            rclone_path: None,
            version: "2.0.40",
            journal_maintenance: &Maintenance,
        };
        let result = admit_prepared_archive_read(
            operation,
            input["repository_id"].as_str().unwrap().into(),
            &services,
        );
        let observed = match result {
            Ok(read) => {
                assert!(Arc::ptr_eq(read.carrier(), &carrier));
                assert!(Arc::ptr_eq(read.journal_root(), &root));
                let revalidation = read.revalidate_admission();
                if input["expire_after_admission"].as_bool().unwrap_or(false) {
                    original.base.store(
                        input["credential_expiry"].as_i64().unwrap(),
                        Ordering::SeqCst,
                    );
                }
                serde_json::json!({"repository_id":read.repository_id(),
                    "same_original_carrier":true,"same_original_root":true,
                    "revalidation":format!("{revalidation:?}"),
                    "after_cap":format!("{:?}",read.revalidate_admission())})
            }
            Err(failure) => {
                assert!(Arc::ptr_eq(failure.carrier().unwrap(), &carrier));
                assert!(Arc::ptr_eq(failure.journal_root(), &root));
                if let Some(readback) = failure.readback() {
                    assert!(Arc::ptr_eq(readback.carrier(), &carrier));
                }
                serde_json::json!({"error":format!("{:?}",failure.error()),
                    "same_original_carrier":true,"same_original_root":true,
                    "readback":failure.readback().map(|error|format!("{:?}",error.reason())),
                    "effects":{"broker":failure.effects().broker_requested,
                        "write":failure.effects().write_repository_requested,
                        "read":failure.effects().read_repository_requested}})
            }
        };
        if let Some(publisher) = publisher {
            publisher.join().unwrap();
        }
        observed
    });
    let identity = fs::metadata(carrier.diagnostic_path()).unwrap();
    assert_eq!((identity.dev(), identity.ino()), physical);
    carrier.revalidate().unwrap();
    println!(
        "DIRECT_READ_RESULT {}",
        serde_json::json!({"result":observed,"elapsed_ms":started.elapsed().as_millis(),
            "same_physical_carrier":true,"carrier_device":physical.0,"carrier_inode":physical.1,
            "cancelled":cancellation.is_cancelled(),"original_now":original.now_unix(),
            "services_now":replacement.now_unix(),"rclone":null,
            "limit":"admission only; no native retirement or next-admission claim"})
    );
}

#[cfg(all(target_os = "linux", feature = "full-tests"))]
#[test]
#[ignore = "requires a copied current package and synthetic direct-finish fixture"]
fn packaged_direct_read_finish_entry() {
    use solstone_core_backup::archive_activity::PreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation};
    use solstone_core_backup::archive_readback::{ExactArchiveFile, ReadbackError};
    use solstone_core_backup_runtime::{
        DirectReadFinishState, UreqHttpTransport, admit_prepared_archive_read_installed,
    };
    use solstone_core_journal_io::JournalRoot;
    use solstone_core_journal_io::media::{Cleanup, MediaDirectory, MediaStage};
    use std::sync::Arc;

    struct OwnedClock(Instant);
    impl Clock for OwnedClock {
        fn now_unix(&self) -> i64 {
            10_000 + self.0.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    let input_path =
        std::env::var_os("DIRECT_FINISH_PROBE_JSON").expect("explicit fixture required");
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let mode = input["mode"].as_str().unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(OwnedClock(Instant::now()));
    let cancellation = ArchiveCancellation::new();
    let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation).unwrap();
    let original =
        PreparedArchiveOperation::prepare_owned(Arc::clone(&root), budget, Arc::clone(&clock))
            .unwrap();
    assert!(Arc::ptr_eq(original.owned_clock().unwrap(), &clock));
    let lease = Arc::downgrade(original.activity());
    let carrier = Arc::downgrade(original.carrier());
    let path = Path::new(input["restic_path"].as_str().unwrap());
    let repository = input["repository_id"].as_str().unwrap();
    let installed = admit_prepared_archive_read_installed(
        original,
        repository.into(),
        path,
        "2.0.40",
        &UreqHttpTransport,
    )
    .unwrap();
    let (mut read, finish) = installed.into_parts();
    assert!(Arc::ptr_eq(read.carrier(), finish.carrier()));
    assert!(Arc::ptr_eq(read.owned_clock().unwrap(), &clock));
    assert!(Arc::ptr_eq(finish.owned_clock(), &clock));
    let parent = MediaDirectory::open(Arc::clone(&root), Path::new("media")).unwrap();
    let file = ExactArchiveFile {
        repository_id: repository.into(),
        snapshot_id: input["snapshot_id"].as_str().unwrap().into(),
        name: input["name"].as_str().unwrap().into(),
        archive_path: input["archive_path"].as_str().unwrap().into(),
        bytes: input["bytes"].as_u64().unwrap(),
        sha256: input["sha256"].as_str().unwrap().into(),
    };
    let result = if mode == "panic" {
        let target = MediaStage::create(parent, Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _actual_read = read;
            let _actual_target = target;
            panic!("explicit lower fixture unwind with an actual native target");
        }));
        assert!(panic.is_err());
        let unknown = finish.into_unknown();
        let unknown = thread::spawn(move || unknown).join().unwrap();
        assert_eq!(unknown.state(), DirectReadFinishState::Unknown);
        assert!(Arc::ptr_eq(unknown.owned_clock(), &clock));
        assert!(Arc::ptr_eq(unknown.journal_root(), &root));
        assert!(carrier.upgrade().is_some());
        assert!(lease.upgrade().is_some());
        assert!(unknown.retirement().is_none());
        let next = PreparedArchiveOperation::prepare_owned(
            Arc::clone(&root),
            ArchiveBudget::new(clock.as_ref()).unwrap(),
            Arc::clone(&clock),
        );
        assert!(next.is_err());
        drop(next);
        serde_json::json!({"state":"Unknown","same_original_owner":true,"same_original_clock":true,"replacement_refused":true,"retirement":false})
    } else {
        if mode == "sticky" {
            let bad = ExactArchiveFile {
                snapshot_id: "f".repeat(64),
                ..ExactArchiveFile {
                    repository_id: file.repository_id.clone(),
                    snapshot_id: file.snapshot_id.clone(),
                    name: file.name.clone(),
                    archive_path: file.archive_path.clone(),
                    bytes: file.bytes,
                    sha256: file.sha256.clone(),
                }
            };
            assert_ne!(bad.snapshot_id, file.snapshot_id);
            let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
                .unwrap()
                .create_target()
                .unwrap();
            let mut failed = match read.read_exact(&bad, target) {
                Err(failed) => failed,
                Ok(_) => panic!("missing snapshot must fail"),
            };
            assert_eq!(failed.reason(), ReadbackError::ToolFailed);
            assert_eq!(failed.cleanup(), Cleanup::Complete);
            drop(failed);
        } else {
            assert_eq!(mode, "complete");
        }
        let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let mut prepared = read.read_exact(&file, target).unwrap();
        assert_eq!(prepared.digest().sha256, file.sha256);
        assert_eq!(prepared.digest().size, file.bytes);
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
        drop(prepared);
        drop(parent);
        if mode == "sticky" {
            let held = match read.settle_direct() {
                Err(held) => held,
                Ok(_) => panic!("valid later read must not clear failed query"),
            };
            assert_eq!(held.reason(), ReadbackError::ToolFailed);
            let held = match held.into_owned_hold() {
                Ok(held) => held,
                Err(_) => panic!("actual owned clock must close"),
            };
            assert!(Arc::ptr_eq(held.carrier(), finish.carrier()));
            let custody = thread::spawn(move || (held, finish)).join().unwrap();
            assert!(carrier.upgrade().is_some());
            assert!(lease.upgrade().is_some());
            let next = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            );
            assert!(next.is_err());
            drop(next);
            assert!(Arc::ptr_eq(custody.1.owned_clock(), &clock));
            serde_json::json!({"state":"Held","reason":"ToolFailed","later_read_succeeded":true,"same_original_owner":true,"same_original_clock":true,"replacement_refused":true,"retirement":false})
        } else {
            let settled = match read.settle_direct() {
                Ok(settled) => settled,
                Err(held) => panic!("actual direct read remained held: {}", held.reason()),
            };
            let completed = match finish.finish(settled).into_owned_hold() {
                Ok(completed) => completed,
                Err(_) => panic!("actual owned finish must close"),
            };
            assert_eq!(completed.state(), DirectReadFinishState::Completed);
            assert!(Arc::ptr_eq(completed.owned_clock(), &clock));
            assert!(lease.upgrade().is_some());
            let receipt = completed.retirement().unwrap();
            assert!(receipt.receipt.failures().is_empty());
            let retirement = format!(
                "{:?}/{:?}/{:?}",
                receipt.receipt.effect(),
                receipt.receipt.root_durability(),
                receipt.receipt.confirmation()
            );
            let before_release = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            );
            assert!(before_release.is_err());
            drop(before_release);
            drop(completed);
            assert!(lease.upgrade().is_none());
            let next = PreparedArchiveOperation::prepare_owned(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            )
            .unwrap();
            let second = admit_prepared_archive_read_installed(
                next,
                repository.into(),
                path,
                "2.0.40",
                &UreqHttpTransport,
            )
            .unwrap();
            let (second_read, second_finish) = second.into_parts();
            let settled = match second_read.settle_direct() {
                Ok(settled) => settled,
                Err(held) => panic!("second actual read remained held: {}", held.reason()),
            };
            let second = match second_finish.finish(settled).into_owned_hold() {
                Ok(second) => second,
                Err(_) => panic!("second actual owner must close"),
            };
            assert_eq!(second.state(), DirectReadFinishState::Completed);
            drop(second);
            serde_json::json!({"state":"Completed","same_original_owner":true,"same_original_clock":true,"retirement":retirement,"held_lease_refused_next":true,"second_installed_admission":true,"second_completed":true,"target_cleanup":"Complete"})
        }
    };
    println!(
        "DIRECT_FINISH_RESULT {}",
        serde_json::json!({"mode":mode,"result":result,"limit":"lower direct only; no consuming RestoreRunOutcome or Consumer keeper/generation qualification"})
    );
}

#[cfg(all(target_os = "linux", feature = "full-tests"))]
#[test]
#[ignore = "requires a copied current package and synthetic historical-read fixture"]
fn packaged_historical_archive_read_entry() {
    use solstone_core_backup::archive_activity::PreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation};
    use solstone_core_backup::archive_readback::{HistoricalArchiveFile, ReadbackError};
    use solstone_core_backup_runtime::{
        UreqHttpTransport, admit_prepared_archive_current_read_installed,
        admit_prepared_archive_read_installed,
    };
    use solstone_core_journal_io::JournalRoot;
    use solstone_core_journal_io::media::{Cleanup, MediaDirectory, MediaStage};
    use std::io::Read;
    use std::os::fd::FromRawFd;
    use std::sync::Arc;

    struct OwnedClock(Instant);
    impl Clock for OwnedClock {
        fn now_unix(&self) -> i64 {
            10_000 + self.0.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    let input_path =
        std::env::var_os("HISTORICAL_READ_PROBE_JSON").expect("explicit fixture required");
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let mode = input["mode"].as_str().unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(OwnedClock(Instant::now()));
    let cancellation = ArchiveCancellation::new();
    let original = PreparedArchiveOperation::prepare_owned(
        Arc::clone(&root),
        ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap(),
        Arc::clone(&clock),
    )
    .unwrap();
    let carrier = Arc::clone(original.carrier());
    let restic = Path::new(input["restic_path"].as_str().unwrap());
    let admitted = if mode == "known-mismatch" {
        admit_prepared_archive_read_installed(
            original,
            "f".repeat(64),
            restic,
            "2.0.40",
            &UreqHttpTransport,
        )
    } else {
        admit_prepared_archive_current_read_installed(
            original,
            restic,
            "2.0.40",
            &UreqHttpTransport,
        )
    };
    let installed = match admitted {
        Ok(installed) => installed,
        Err(refusal) => {
            assert!(Arc::ptr_eq(refusal.carrier().unwrap(), &carrier));
            assert!(Arc::ptr_eq(refusal.journal_root(), &root));
            assert!(Arc::ptr_eq(
                refusal.readback().unwrap().owned_clock().unwrap(),
                &clock
            ));
            let reason = refusal.readback().unwrap().reason();
            let refusal = refusal.into_owned_hold().unwrap();
            let refusal = thread::spawn(move || refusal).join().unwrap();
            assert!(Arc::ptr_eq(refusal.carrier().unwrap(), &carrier));
            assert!(Arc::ptr_eq(refusal.journal_root(), &root));
            println!(
                "HISTORICAL_READ_RESULT {}",
                serde_json::json!({"admission_error":format!("{reason:?}"),"same_original_owner":true,"closed_send":true})
            );
            return;
        }
    };
    let (mut read, finish) = installed.into_parts();
    assert_eq!(
        read.repository_id(),
        input["repository_id"].as_str().unwrap()
    );
    assert!(Arc::ptr_eq(read.carrier(), &carrier));
    assert!(Arc::ptr_eq(read.journal_root(), &root));
    assert!(Arc::ptr_eq(read.owned_clock().unwrap(), &clock));
    assert!(Arc::ptr_eq(finish.carrier(), &carrier));
    assert!(Arc::ptr_eq(finish.owned_clock(), &clock));
    let input_files = input["files"].as_array().unwrap();
    let files: Vec<_> = input_files
        .iter()
        .map(|file| HistoricalArchiveFile {
            name: file["name"].as_str().unwrap(),
            bytes: file["bytes"].as_u64().unwrap(),
            sha256: file["sha256"].as_str().unwrap(),
        })
        .collect();
    let publisher = input["control_fd"].as_i64().map(|raw| {
        // SAFETY: the disposable probe owns this explicitly inherited control socket.
        #[allow(unsafe_code)]
        let mut control =
            unsafe { std::os::unix::net::UnixStream::from_raw_fd(i32::try_from(raw).unwrap()) };
        control
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let cancellation = cancellation.clone();
        thread::spawn(move || {
            let mut byte = [0];
            control.read_exact(&mut byte).unwrap();
            assert_eq!(byte[0], b'c');
            cancellation.cancel();
            control.write_all(b"a").unwrap();
        })
    });
    if mode == "sticky-input" {
        assert!(matches!(
            read.discover_historical_snapshot("latest", &files),
            Err(ReadbackError::InvalidSelection)
        ));
        let duplicate = [
            HistoricalArchiveFile {
                name: files[0].name,
                bytes: files[0].bytes,
                sha256: files[0].sha256,
            },
            HistoricalArchiveFile {
                name: files[0].name,
                bytes: files[0].bytes,
                sha256: files[0].sha256,
            },
        ];
        assert!(matches!(
            read.discover_historical_snapshot(input["selector"].as_str().unwrap(), &duplicate),
            Err(ReadbackError::InvalidSelection)
        ));
    }
    let discovery = read.discover_historical_snapshot(input["selector"].as_str().unwrap(), &files);
    if let Some(publisher) = publisher {
        publisher.join().unwrap();
    }
    let result = match discovery {
        Ok(observed) => {
            assert_eq!(observed.snapshot_id, input["snapshot_id"].as_str().unwrap());
            assert_eq!(
                observed.archive_parent,
                input["archive_parent"].as_str().unwrap()
            );
            assert_eq!(observed.files.len(), input_files.len());
            let parent = MediaDirectory::open(Arc::clone(&root), Path::new("media")).unwrap();
            for (file, expected) in observed.files.iter().zip(input_files) {
                assert_eq!(file.name, expected["name"].as_str().unwrap());
                assert_eq!(
                    file.archive_path,
                    expected["archive_path"].as_str().unwrap()
                );
                assert_eq!(file.sha256, expected["sha256"].as_str().unwrap());
                let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
                    .unwrap()
                    .create_target()
                    .unwrap();
                let mut prepared = read.read_exact(file, target).unwrap();
                assert_eq!(prepared.digest().size, file.bytes);
                assert_eq!(prepared.digest().sha256, file.sha256);
                assert_eq!(prepared.cleanup(), Cleanup::Complete);
            }
            serde_json::json!({"snapshot_id":observed.snapshot_id,"archive_parent":observed.archive_parent,"verified_files":observed.files.len()})
        }
        Err(reason) => serde_json::json!({"discovery_error":format!("{reason:?}")}),
    };
    let (closed, _closed_owner): (_, Box<dyn Send>) =
        if mode == "sticky-input" || result.get("discovery_error").is_some() {
            let held = match read.settle_direct() {
                Err(held) => held,
                Ok(_) => panic!("failed discovery cannot settle"),
            };
            if mode == "sticky-input" {
                assert_eq!(held.reason(), ReadbackError::InvalidSelection);
            }
            let reason = held.reason();
            assert!(Arc::ptr_eq(held.carrier(), &carrier));
            let closed = match held.into_owned_hold() {
                Ok(closed) => closed,
                Err(_) => panic!("owned clock must transfer"),
            };
            let closed = thread::spawn(move || closed).join().unwrap();
            assert!(Arc::ptr_eq(closed.carrier(), &carrier));
            (
                serde_json::json!({"held_reason":format!("{reason:?}"),"closed_send":true}),
                Box::new(closed),
            )
        } else {
            let closed = match read.into_owned_hold() {
                Ok(closed) => closed,
                Err(_) => panic!("owned clock must transfer"),
            };
            let closed = thread::spawn(move || closed).join().unwrap();
            assert!(Arc::ptr_eq(closed.carrier(), &carrier));
            assert!(Arc::ptr_eq(closed.owned_clock(), &clock));
            (serde_json::json!({"closed_send":true}), Box::new(closed))
        };
    let outside = finish.into_unknown();
    assert!(Arc::ptr_eq(outside.owned_clock(), &clock));
    assert!(Arc::ptr_eq(outside.journal_root(), &root));
    assert!(outside.retirement().is_none());
    println!(
        "HISTORICAL_READ_RESULT {}",
        serde_json::json!({"result":result,"custody":closed,"same_original_owner":true,"same_original_clock":true,"cancelled":cancellation.is_cancelled(),"limit":"discovery/read only; no publication, retirement, restart or consuming keeper credit"})
    );
}

#[cfg(all(target_os = "linux", feature = "full-tests"))]
#[test]
#[ignore = "requires a copied current package and synthetic owned-read fixture"]
fn packaged_total_owned_read_finish_entry() {
    use solstone_core_backup::archive_activity::OwnedPreparedArchiveOperation;
    use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation};
    use solstone_core_backup::archive_readback::{ExactArchiveFile, ReadbackError};
    use solstone_core_backup_runtime::{
        DirectReadFinishState, UreqHttpTransport,
        admit_prepared_archive_owned_current_read_installed,
        admit_prepared_archive_owned_read_installed,
    };
    use solstone_core_journal_io::JournalRoot;
    use solstone_core_journal_io::media::{Cleanup, MediaDirectory, MediaStage};
    use std::sync::Arc;

    struct OwnedClock(Instant);
    impl Clock for OwnedClock {
        fn now_unix(&self) -> i64 {
            10_000 + self.0.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    let input_path = std::env::var_os("TOTAL_OWNED_READ_PROBE_JSON").unwrap();
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let mode = input["mode"].as_str().unwrap();
    assert!(["complete", "sticky", "mismatch"].contains(&mode));
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(OwnedClock(Instant::now()));
    let clock_weak = Arc::downgrade(&clock);
    let cancellation = ArchiveCancellation::new();
    let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation).unwrap();
    let original =
        match OwnedPreparedArchiveOperation::prepare(Arc::clone(&root), budget, Arc::clone(&clock))
        {
            Ok(original) => original,
            Err(failure) => panic!("owned preparation: {}", failure.reason()),
        };
    assert!(Arc::ptr_eq(original.owned_clock(), &clock));
    let lease = Arc::downgrade(original.activity());
    let carrier = Arc::downgrade(original.carrier());
    let path = Path::new(input["restic_path"].as_str().unwrap());
    let repository = input["repository_id"].as_str().unwrap();
    let admitted = if mode == "complete" {
        admit_prepared_archive_owned_current_read_installed(
            original,
            path,
            "2.0.40",
            &UreqHttpTransport,
        )
    } else {
        admit_prepared_archive_owned_read_installed(
            original,
            repository.into(),
            path,
            "2.0.40",
            &UreqHttpTransport,
        )
    };
    let installed = match admitted {
        Ok(installed) => installed,
        Err(failure) => panic!("owned admission: {}", failure.error()),
    };
    let (mut read, finish) = installed.into_parts();
    assert_eq!(read.repository_id(), repository);
    assert!(Arc::ptr_eq(read.carrier(), finish.carrier()));
    assert!(Arc::ptr_eq(read.owned_clock(), &clock));
    assert!(Arc::ptr_eq(finish.owned_clock(), &clock));
    assert!(Arc::ptr_eq(read.as_read().owned_clock().unwrap(), &clock));
    let parent = MediaDirectory::open(Arc::clone(&root), Path::new("media")).unwrap();
    let file = ExactArchiveFile {
        repository_id: repository.into(),
        snapshot_id: input["snapshot_id"].as_str().unwrap().into(),
        name: input["name"].as_str().unwrap().into(),
        archive_path: input["archive_path"].as_str().unwrap().into(),
        bytes: input["bytes"].as_u64().unwrap(),
        sha256: input["sha256"].as_str().unwrap().into(),
    };
    if mode == "sticky" {
        let bad = ExactArchiveFile {
            repository_id: file.repository_id.clone(),
            snapshot_id: "f".repeat(64),
            name: file.name.clone(),
            archive_path: file.archive_path.clone(),
            bytes: file.bytes,
            sha256: file.sha256.clone(),
        };
        assert_ne!(bad.snapshot_id, file.snapshot_id);
        let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let mut failed = match read.read_exact(&bad, target) {
            Err(failed) => failed,
            Ok(_) => panic!("actual absent snapshot must fail"),
        };
        assert_eq!(failed.reason(), ReadbackError::ToolFailed);
        assert_eq!(failed.cleanup(), Cleanup::Complete);
        drop(failed);
    }
    let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
        .unwrap()
        .create_target()
        .unwrap();
    let mut prepared = read.read_exact(&file, target).unwrap();
    assert_eq!(prepared.digest().sha256, file.sha256);
    assert_eq!(prepared.digest().size, file.bytes);
    assert_eq!(prepared.cleanup(), Cleanup::Complete);
    drop(prepared);
    drop(parent);
    let result = if mode == "sticky" {
        let held = match read.settle_direct() {
            Err(held) => held,
            Ok(_) => panic!("later valid read cannot clear original failed query"),
        };
        assert_eq!(held.reason(), ReadbackError::ToolFailed);
        assert!(Arc::ptr_eq(held.carrier(), finish.carrier()));
        assert!(Arc::ptr_eq(held.owned_clock(), &clock));
        let custody = thread::spawn(move || (held, finish)).join().unwrap();
        assert!(carrier.upgrade().is_some() && lease.upgrade().is_some());
        let next = OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            ArchiveBudget::new(clock.as_ref()).unwrap(),
            Arc::clone(&clock),
        );
        assert!(next.is_err());
        drop(next);
        assert!(Arc::ptr_eq(custody.0.owned_clock(), &clock));
        serde_json::json!({"state":"Held","reason":"ToolFailed","later_read_succeeded":true,
            "same_original_owner":true,"same_original_clock":true,"replacement_refused":true,
            "retirement":false,"total_closed_return":true})
    } else {
        let settled = match read.settle_direct() {
            Ok(settled) => settled,
            Err(held) => panic!("actual owned read held: {}", held.reason()),
        };
        let settled = thread::spawn(move || settled).join().unwrap();
        assert!(Arc::ptr_eq(settled.owned_clock(), &clock));
        if mode == "mismatch" {
            let other_root = Arc::new(
                JournalRoot::open(Path::new(input["other_journal"].as_str().unwrap())).unwrap(),
            );
            let other_clock: Arc<dyn Clock + Send + Sync> = Arc::new(OwnedClock(Instant::now()));
            assert!(!Arc::ptr_eq(&clock, &other_clock));
            let other = match OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&other_root),
                ArchiveBudget::new(other_clock.as_ref()).unwrap(),
                Arc::clone(&other_clock),
            ) {
                Ok(other) => other,
                Err(failure) => panic!("other preparation: {}", failure.reason()),
            };
            let other = match admit_prepared_archive_owned_read_installed(
                other,
                repository.into(),
                path,
                "2.0.40",
                &UreqHttpTransport,
            ) {
                Ok(other) => other,
                Err(failure) => panic!("other admission: {}", failure.error()),
            };
            let (other_read, wrong_finish) = other.into_parts();
            assert!(!Arc::ptr_eq(settled.carrier(), wrong_finish.carrier()));
            let other_hold = other_read.into_owned_hold();
            let held = wrong_finish.finish_owned(settled);
            assert_eq!(held.state(), DirectReadFinishState::Held);
            assert_eq!(held.reason(), Some(ReadbackError::NativeFailure));
            assert!(held.retirement().is_none());
            // After the original outside owner/clock are gone, ONLY the retained
            // actual pre-release read keeps its original carrier and clock alive.
            drop(finish);
            drop(clock);
            let custody = thread::spawn(move || (held, other_hold)).join().unwrap();
            assert!(carrier.upgrade().is_some() && lease.upgrade().is_some());
            let original_clock = clock_weak.upgrade().unwrap();
            assert!(Arc::ptr_eq(custody.0.owned_clock(), &other_clock));
            assert!(Arc::ptr_eq(
                carrier.upgrade().unwrap().journal_root(),
                &root
            ));
            let next = OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&root),
                ArchiveBudget::new(original_clock.as_ref()).unwrap(),
                original_clock,
            );
            assert!(next.is_err());
            drop(next);
            serde_json::json!({"state":"Held","reason":"NativeFailure","actual_read_succeeded":true,
                "same_original_owner":true,"same_original_clock":true,"replacement_refused":true,
                "retirement":false,"total_closed_return":true,"different_real_owner":true,
                "different_real_clock":true,"read_survives_outside_owner_drop":true})
        } else {
            let completed = finish.finish_owned(settled);
            let completed = thread::spawn(move || completed).join().unwrap();
            assert_eq!(completed.state(), DirectReadFinishState::Completed);
            assert!(Arc::ptr_eq(completed.owned_clock(), &clock));
            let receipt = completed.retirement().unwrap();
            assert!(receipt.receipt.failures().is_empty());
            let retirement = format!(
                "{:?}/{:?}/{:?}",
                receipt.receipt.effect(),
                receipt.receipt.root_durability(),
                receipt.receipt.confirmation()
            );
            let before_release = OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            );
            assert!(before_release.is_err());
            drop(before_release);
            drop(completed);
            assert!(lease.upgrade().is_none());
            let next = match OwnedPreparedArchiveOperation::prepare(
                Arc::clone(&root),
                ArchiveBudget::new(clock.as_ref()).unwrap(),
                Arc::clone(&clock),
            ) {
                Ok(next) => next,
                Err(failure) => panic!("second preparation: {}", failure.reason()),
            };
            let second = match admit_prepared_archive_owned_read_installed(
                next,
                repository.into(),
                path,
                "2.0.40",
                &UreqHttpTransport,
            ) {
                Ok(second) => second,
                Err(failure) => panic!("second admission: {}", failure.error()),
            };
            let (second_read, second_finish) = second.into_parts();
            let second_settled = match second_read.settle_direct() {
                Ok(settled) => settled,
                Err(held) => panic!("second settlement: {}", held.reason()),
            };
            let second = second_finish.finish_owned(second_settled);
            assert_eq!(second.state(), DirectReadFinishState::Completed);
            drop(second);
            serde_json::json!({"state":"Completed","same_original_owner":true,"same_original_clock":true,
                "retirement":retirement,"held_lease_refused_next":true,"second_installed_admission":true,
                "second_completed":true,"target_cleanup":"Complete","total_closed_return":true,
                "current_repository_capture":true})
        }
    };
    println!(
        "TOTAL_OWNED_READ_RESULT {}",
        serde_json::json!({"mode":mode,"result":result,
        "limit":"lower typed direct only; no Native/Proof actor, Consumer keeper, early unwind or forced writer grant"})
    );
}
#[cfg(all(target_os = "linux", feature = "full-tests"))]
#[test]
#[ignore = "requires a copied current package and synthetic direct creator fixture"]
fn packaged_owned_archive_creator_entry() {
    use solstone_core_backup::archive_activity::{
        OwnedPreparedArchiveOperation, PreparedArchiveOperation,
    };
    use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation};
    use solstone_core_backup::archive_readback::{ExactArchiveFile, ReadbackError};
    use solstone_core_backup_runtime::{
        ArchiveRemoteEffects, DirectReadFinishState, admit_prepared_archive,
        admit_prepared_archive_owned_creator_installed,
    };
    use solstone_core_journal_io::{
        JournalRoot,
        activity::ActivityLease,
        media::{Cleanup, MediaDirectory, MediaStage, PreparedMedia, digest_reader},
    };
    use std::sync::Arc;

    struct OwnedClock(Instant);
    impl Clock for OwnedClock {
        fn now_unix(&self) -> i64 {
            10_000 + self.0.elapsed().as_secs() as i64
        }
        fn iso_week(&self) -> u8 {
            1
        }
    }
    fn copy_native(
        parent: &Arc<MediaDirectory>,
        activity: Arc<ActivityLease>,
        input: &serde_json::Value,
    ) -> PreparedMedia {
        let name = std::ffi::OsStr::new(input["name"].as_str().unwrap());
        let bytes = fs::read(input["path"].as_str().unwrap()).unwrap();
        let digest = digest_reader(&mut bytes.as_slice()).unwrap();
        assert_eq!(digest.size, input["bytes"].as_u64().unwrap());
        assert_eq!(digest.sha256, input["sha256"].as_str().unwrap());
        MediaStage::create(Arc::clone(parent), activity)
            .unwrap()
            .create_named_target(name)
            .unwrap()
            .copy_from(parent, name, &digest)
            .unwrap()
    }
    let input_path = std::env::var_os("OWNED_CREATOR_PROBE_JSON").unwrap();
    let input: serde_json::Value = serde_json::from_slice(&fs::read(input_path).unwrap()).unwrap();
    let mode = input["mode"].as_str().unwrap();
    assert!(
        [
            "complete",
            "active-error",
            "no-writer",
            "unknown-zero",
            "unknown-output",
            "pre-refusal"
        ]
        .contains(&mode)
    );
    let root = Arc::new(JournalRoot::open(Path::new(input["journal"].as_str().unwrap())).unwrap());
    let clock: Arc<dyn Clock + Send + Sync> = Arc::new(OwnedClock(Instant::now()));
    let cancellation = ArchiveCancellation::new();
    let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone()).unwrap();
    let path = Path::new(input["restic_path"].as_str().unwrap());
    let repository = input["repository_id"].as_str().unwrap();
    let m = &input["existing"];
    let file = ExactArchiveFile {
        repository_id: repository.into(),
        snapshot_id: m["snapshot_id"].as_str().unwrap().into(),
        name: m["name"].as_str().unwrap().into(),
        archive_path: m["archive_path"].as_str().unwrap().into(),
        bytes: m["bytes"].as_u64().unwrap(),
        sha256: m["sha256"].as_str().unwrap().into(),
    };
    let parent = MediaDirectory::open(Arc::clone(&root), Path::new("media")).unwrap();

    if mode.starts_with("unknown-") {
        let original =
            PreparedArchiveOperation::prepare_owned(Arc::clone(&root), budget, Arc::clone(&clock))
                .unwrap();
        let carrier = Arc::clone(original.carrier());
        let http = RecordingHttp {
            urls: RefCell::new(Vec::new()),
        };
        let services = BackupServices {
            runner: &SystemToolRunner,
            http: &http,
            clock: clock.as_ref(),
            restic: Some((path).into()),
            rclone_path: None,
            version: "fixture",
            journal_maintenance: &Maintenance,
        };
        let mut admitted = admit_prepared_archive(original, &services).unwrap();
        assert!(Arc::ptr_eq(
            admitted.prepared_operation().carrier(),
            &carrier
        ));
        let mut copied = if mode == "unknown-output" {
            Some(copy_native(
                &parent,
                Arc::clone(admitted.activity()),
                &input["native"],
            ))
        } else {
            None
        };
        let snapshot = copied.as_mut().map(|copied| {
            admitted
                .archive_owned(&[copied.archive_source().unwrap()])
                .unwrap()
        });
        let mut read = admitted.into_read_operation().unwrap();
        assert!(Arc::ptr_eq(read.carrier(), &carrier));
        assert!(Arc::ptr_eq(read.owned_clock().unwrap(), &clock));
        if let (Some(snapshot), Some(copied)) = (snapshot.as_ref(), copied.as_mut()) {
            let source = copied.archive_source().unwrap();
            let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
                .unwrap()
                .create_target()
                .unwrap();
            let (exact, mut restored) = read
                .read_owned_snapshot(&snapshot.snapshot_id, &source, target)
                .unwrap();
            assert_eq!(exact.sha256, source.digest().sha256);
            assert_eq!(restored.cleanup(), Cleanup::Complete);
        }
        let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let mut restored = read.read_exact(&file, target).unwrap();
        assert_eq!(restored.digest().sha256, file.sha256);
        assert_eq!(restored.cleanup(), Cleanup::Complete);
        drop(restored);
        if let Some(copied) = copied.as_mut() {
            assert_eq!(copied.cleanup(), Cleanup::Complete);
        }
        drop(copied);
        drop(parent);
        let held = match read.settle_direct() {
            Err(held) => held,
            Ok(_) => panic!("generic output cannot qualify creator"),
        };
        assert_eq!(held.reason(), ReadbackError::ToolFailed);
        let held = match held.into_owned_hold() {
            Ok(held) => held,
            Err(_) => panic!("original owned clock must close"),
        };
        let custody = thread::spawn(move || held).join().unwrap();
        assert!(Arc::ptr_eq(custody.carrier(), &carrier));
        assert!(Arc::ptr_eq(custody.owned_clock(), &clock));
        assert!(http.urls.borrow().is_empty());
        println!(
            "OWNED_CREATOR_RESULT {}",
            serde_json::json!({"mode":mode,"state":"Held","reason":"ToolFailed",
            "same_original_owner":true,"same_original_clock":true,"good_read":true,"generic_writer":snapshot.is_some(),
            "retirement":false,"closed_thread_hop":true})
        );
        return;
    }

    let original =
        match OwnedPreparedArchiveOperation::prepare(Arc::clone(&root), budget, Arc::clone(&clock))
        {
            Ok(original) => original,
            Err(failure) => panic!("original preparation: {}", failure.reason()),
        };
    let weak_lease = Arc::downgrade(original.activity());
    let original_carrier = Arc::downgrade(original.carrier());
    let installed = match admit_prepared_archive_owned_creator_installed(
        original,
        path,
        None,
        "test",
        &solstone_core_backup_runtime::UreqHttpTransport,
    ) {
        Ok(installed) => installed,
        Err(failure) => panic!("creator admission: {}", failure.error()),
    };
    let (mut admitted, finish) = installed.into_parts();
    assert_eq!(admitted.repository_id(), repository);
    assert!(Arc::ptr_eq(admitted.carrier(), finish.carrier()));
    assert!(Arc::ptr_eq(
        admitted.prepared_operation().owned_clock().unwrap(),
        &clock
    ));
    assert!(Arc::ptr_eq(finish.owned_clock(), &clock));
    let mut copied = if ["complete", "active-error"].contains(&mode) {
        Some(copy_native(
            &parent,
            Arc::clone(admitted.activity()),
            &input["native"],
        ))
    } else {
        None
    };
    let sentinel = fs::File::open(input["sentinel"].as_str().unwrap()).unwrap();
    nix::fcntl::fcntl(
        &sentinel,
        nix::fcntl::FcntlArg::F_SETFD(nix::fcntl::FdFlag::empty()),
    )
    .unwrap();
    let sentinel_flags = nix::fcntl::fcntl(&sentinel, nix::fcntl::FcntlArg::F_GETFD).unwrap();
    assert_eq!(sentinel_flags & nix::fcntl::FdFlag::FD_CLOEXEC.bits(), 0);
    let mut writer_failure = None;
    let snapshot = if let Some(copied) = copied.as_mut() {
        let sources = [copied.archive_source().unwrap()];
        let descriptor = sources[0].descriptor().unwrap();
        let before = nix::fcntl::fcntl(descriptor, nix::fcntl::FcntlArg::F_GETFD).unwrap();
        assert_ne!(before & nix::fcntl::FdFlag::FD_CLOEXEC.bits(), 0);
        eprintln!(
            "OWNED_CREATOR_PENDING_INPUT {}",
            serde_json::json!({"pid":std::process::id(),
            "source_fd":descriptor.as_raw_fd(),"source_path":sources[0].path(),"source_parent_fd_flags":before,
            "sentinel_fd":sentinel.as_raw_fd(),"sentinel_path":input["sentinel"],"sentinel_flags":sentinel_flags})
        );
        let result = admitted.archive_owned(&sources);
        assert_eq!(
            nix::fcntl::fcntl(descriptor, nix::fcntl::FcntlArg::F_GETFD).unwrap(),
            before
        );
        admitted.revalidate_local().unwrap();
        assert!(!cancellation.is_cancelled());
        match result {
            Ok(snapshot) => {
                assert_eq!(mode, "complete");
                Some(snapshot)
            }
            Err(failure) => {
                assert_eq!(mode, "active-error");
                assert_eq!(
                    failure.error,
                    solstone_core_backup_runtime::ArchiveError::Readback(ReadbackError::ToolFailed)
                );
                assert!(matches!(
                    failure.remote_effects,
                    ArchiveRemoteEffects::Attempted { .. }
                ));
                eprintln!(
                    "OWNED_CREATOR_WRITER_REFUSAL {:?} {:?}",
                    failure.error, failure.remote_effects
                );
                writer_failure = Some(format!("{:?}", failure.remote_effects));
                None
            }
        }
    } else if mode == "pre-refusal" {
        let failure = admitted.archive_owned(&[]).unwrap_err();
        assert_eq!(
            failure.error,
            solstone_core_backup_runtime::ArchiveError::InvalidSelection
        );
        assert_eq!(failure.remote_effects, ArchiveRemoteEffects::NotAttempted);
        writer_failure = Some("NotAttempted".into());
        None
    } else {
        None
    };
    assert_eq!(
        nix::fcntl::fcntl(&sentinel, nix::fcntl::FcntlArg::F_GETFD).unwrap(),
        sentinel_flags
    );
    drop(sentinel);
    let mut read = match admitted.into_read_operation() {
        Ok(read) => read,
        Err(failure) => panic!("same owner handoff: {}", failure.error()),
    };
    assert!(Arc::ptr_eq(read.carrier(), finish.carrier()));
    assert!(Arc::ptr_eq(read.owned_clock(), &clock));
    if let (Some(snapshot), Some(copied)) = (snapshot.as_ref(), copied.as_mut()) {
        let source = copied.archive_source().unwrap();
        let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let (exact, mut restored) = read
            .read_owned_snapshot(&snapshot.snapshot_id, &source, target)
            .unwrap();
        assert_eq!(exact.name, input["native"]["name"].as_str().unwrap());
        assert_eq!(restored.digest(), source.digest());
        assert_eq!(restored.cleanup(), Cleanup::Complete);
    }
    if mode != "no-writer" {
        let target = MediaStage::create(Arc::clone(&parent), Arc::clone(read.activity()))
            .unwrap()
            .create_target()
            .unwrap();
        let mut restored = read.read_exact(&file, target).unwrap();
        assert_eq!(restored.digest().size, file.bytes);
        assert_eq!(restored.digest().sha256, file.sha256);
        assert_eq!(restored.cleanup(), Cleanup::Complete);
    }
    if let Some(copied) = copied.as_mut() {
        assert_eq!(copied.cleanup(), Cleanup::Complete);
    }
    drop(copied);
    drop(parent);
    let result = if ["active-error", "pre-refusal"].contains(&mode) {
        let held = match read.settle_direct() {
            Err(held) => held,
            Ok(_) => panic!("later read must not clear writer failure"),
        };
        let expected = if mode == "active-error" {
            ReadbackError::ToolFailed
        } else {
            ReadbackError::InvalidSelection
        };
        assert_eq!(held.reason(), expected);
        assert!(Arc::ptr_eq(held.carrier(), finish.carrier()));
        let custody = thread::spawn(move || (held, finish)).join().unwrap();
        assert!(Arc::ptr_eq(custody.0.owned_clock(), &clock));
        assert!(original_carrier.upgrade().is_some() && weak_lease.upgrade().is_some());
        let next = OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            ArchiveBudget::new(clock.as_ref()).unwrap(),
            Arc::clone(&clock),
        );
        assert!(next.is_err());
        drop(next);
        serde_json::json!({"state":"Held","reason":format!("{expected:?}"),"good_read":true,"writer_failure":writer_failure,
            "same_original_owner":true,"same_original_clock":true,"retirement":false,"next_refused":true,"closed_thread_hop":true})
    } else {
        let settled = match read.settle_direct() {
            Ok(settled) => settled,
            Err(held) => panic!("genuine creator settlement: {}", held.reason()),
        };
        let settled = thread::spawn(move || settled).join().unwrap();
        let completed = finish.finish_owned(settled);
        assert_eq!(completed.state(), DirectReadFinishState::Completed);
        let receipt = completed.retirement().unwrap();
        assert!(receipt.receipt.failures().is_empty());
        let retirement = format!(
            "{:?}/{:?}/{:?}",
            receipt.receipt.effect(),
            receipt.receipt.root_durability(),
            receipt.receipt.confirmation()
        );
        assert_eq!(retirement, "Removed/Synced/Confirmed");
        drop(completed);
        assert!(weak_lease.upgrade().is_none());
        let next = match OwnedPreparedArchiveOperation::prepare(
            Arc::clone(&root),
            ArchiveBudget::new(clock.as_ref()).unwrap(),
            Arc::clone(&clock),
        ) {
            Ok(next) => next,
            Err(failure) => panic!("next preparation: {}", failure.reason()),
        };
        let next = match admit_prepared_archive_owned_creator_installed(
            next,
            path,
            None,
            "test",
            &solstone_core_backup_runtime::UreqHttpTransport,
        ) {
            Ok(next) => next,
            Err(failure) => panic!("next creator admission: {}", failure.error()),
        };
        let (next_read, next_finish) = next.into_parts();
        let next_read = match next_read.into_read_operation() {
            Ok(read) => read,
            Err(failure) => panic!("next handoff: {}", failure.error()),
        };
        let next_settled = match next_read.settle_direct() {
            Ok(settled) => settled,
            Err(held) => panic!("next no-writer settlement: {}", held.reason()),
        };
        let next_completed = next_finish.finish_owned(next_settled);
        assert_eq!(next_completed.state(), DirectReadFinishState::Completed);
        drop(next_completed);
        serde_json::json!({"state":"Completed","good_read":mode=="complete","same_original_owner":true,
            "same_original_clock":true,"retirement":retirement,"next_admission":true,"next_completed":true,
            "closed_thread_hop":true,"snapshot_id":snapshot.as_ref().map(|snapshot| &snapshot.snapshot_id),
            "no_writer":mode=="no-writer"})
    };
    println!(
        "OWNED_CREATOR_RESULT {}",
        serde_json::json!({"mode":mode,"result":result,
        "limits":"lower offline Linux direct creator only; no Proof/Native actor, keeper, early unwind, restart or activation"})
    );
}
