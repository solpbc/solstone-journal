// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::operational_log::{
    LeaseProbe, OplogCreatePrimitive, OplogFormat, OplogWriter, admit_day_health_directory,
    create_oplog_with_test_timing, probe_oplog_identity, probe_oplog_lease,
    run_with_oplog_capture_stderr_fault, run_with_oplog_capture_stdout_fault,
    run_with_oplog_create_barrier,
};
use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_INFO, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo, OPEN_EXISTING,
    SetFileInformationByHandle,
};

const DAY: &str = "20260902";

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn io_error(context: &str) -> std::io::Error {
    std::io::Error::other(format!("{context}: {}", std::io::Error::last_os_error()))
}

fn fixed_instant() -> chrono::DateTime<FixedOffset> {
    FixedOffset::east_opt(0)
        .expect("UTC offset")
        .with_ymd_and_hms(2026, 9, 2, 16, 0, 0)
        .single()
        .expect("fixed receipt instant")
}

/// Create one product oplog writer under a fresh temporary journal and return it
/// alongside the admitted day-health handle and the published file's path.
fn create_and_publish(
    journal: &Path,
) -> (
    OplogWriter,
    solstone_core_journal_io::operational_log::OplogDayHealth,
    PathBuf,
) {
    let writer = create_oplog_with_test_timing(
        JournalRoot::open(journal).expect("admit oplog liveness root"),
        "id",
        "liveness",
        OplogFormat::Log,
        fixed_instant(),
        Duration::ZERO,
        Duration::ZERO,
    )
    .expect("create product operational-log writer");
    let health = admit_day_health_directory(
        JournalRoot::open(journal).expect("readmit oplog liveness root"),
        DAY,
    )
    .expect("admit oplog liveness day health");
    let published = journal
        .join("chronicle")
        .join(DAY)
        .join("health")
        .join(writer.leaf_name());
    (writer, health, published)
}

fn ping_command(count: &str) -> Command {
    let mut command = Command::new("ping");
    command.args(["-n", count, "127.0.0.1"]);
    command
}

fn assert_released_eventually(
    health: &solstone_core_journal_io::operational_log::OplogDayHealth,
    leaf: &OsStr,
    identity: solstone_core_journal_io::operational_log::OplogFileIdentity,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if probe_oplog_lease(health, leaf, identity) == LeaseProbe::Released {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "lease did not release before the deadline"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Open `path` for read, withholding `FILE_SHARE_WRITE`, without going through any
/// oplog writer. Used to prove the probe conservatively reports `Active` for any
/// foreign handle that withholds write sharing, not only for the product writer.
fn open_withholding_write_share(path: &Path) -> std::io::Result<OwnedHandle> {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `wide_path` is NUL-terminated; the returned handle is owned immediately
    // after the invalid-handle sentinel check.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_error("open foreign write-sharing-withheld handle"));
    }
    // SAFETY: `raw` is a valid uniquely owned handle after the invalid sentinel check.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

/// Open `path` with `DELETE` access and mark it for deletion. The file is not
/// actually removed while any handle (including this one) remains open.
fn mark_delete_pending(path: &Path) -> std::io::Result<OwnedHandle> {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `wide_path` is NUL-terminated; the returned handle is owned immediately
    // after the invalid-handle sentinel check.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            DELETE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io_error("open handle for delete-pending control"));
    }
    // SAFETY: `handle` stays valid for the synchronous call below.
    #[allow(unsafe_code)]
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    use std::os::windows::io::AsRawHandle;
    // SAFETY: `handle` is live and `info` is sized exactly for this information class.
    #[allow(unsafe_code)]
    let result = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if result == 0 {
        return Err(io_error("mark file delete-pending"));
    }
    Ok(handle)
}

#[test]
fn share_mode_active_while_live_released_after_every_duplicate_closes() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, _published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();
    let duplicate = writer.try_clone_for_write().expect("in-process duplicate");
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );
    assert_eq!(probe_oplog_identity(&health, identity), LeaseProbe::Active);
    drop(writer);
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active,
        "the in-process duplicate alone must keep the share-mode authority live"
    );
    drop(duplicate);
    assert_released_eventually(&health, OsStr::new(&leaf), identity);
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Released
    );
}

#[test]
fn named_probe_indeterminate_after_replacement_identity_probe_unaffected() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );

    let renamed = published.with_extension("displaced");
    std::fs::rename(&published, &renamed).expect("displace the live oplog pathname");
    std::fs::write(&published, b"foreign replacement").expect("plant a foreign file at the leaf");

    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Indeterminate,
        "a pathname now naming a different file must never be trusted for liveness"
    );
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Active,
        "the identity-bound probe never touches the pathname and must be unaffected"
    );

    drop(writer);
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Released,
        "identity liveness must still transition correctly after the writer drops"
    );
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Indeterminate,
        "the named probe stays untrustworthy at this leaf regardless of the original writer's state"
    );
}

#[test]
fn on_disk_leaf_case_mismatch_is_indeterminate_before_liveness_runs() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, _published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();
    let uppercased = leaf.to_ascii_uppercase();
    assert_ne!(
        leaf, uppercased,
        "the oplog leaf grammar always contains ascii letters"
    );

    // NTFS lookup by name is case-insensitive, so this open succeeds, but the
    // on-disk name it resolves to differs in case from the query: the identity
    // is consumed and matches, yet the exact-case control must still refuse
    // before the liveness oracle runs, per the named-form contract.
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&uppercased), identity),
        LeaseProbe::Indeterminate
    );
    // The exact original case still probes correctly.
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );
    drop(writer);
    assert_released_eventually(&health, OsStr::new(&leaf), identity);
}

#[test]
fn delete_pending_never_promotes_to_released() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, published) = create_and_publish(temporary.path());
    let identity = writer.identity();

    let pending = mark_delete_pending(&published).expect("mark the live oplog delete-pending");
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Active,
        "share conflict with the live writer still dominates while delete is pending"
    );

    drop(writer);
    // The writer is gone but `pending` still holds the name delete-pending: the
    // underlying bytes are not yet reclaimed. A correct probe must never read
    // this as `Released` from absence or EOF; it must stay indeterminate.
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Indeterminate,
        "delete-pending must never be promoted to Released by an absence heuristic"
    );
    drop(pending);
}

#[test]
fn foreign_write_sharing_withheld_handle_is_conservatively_active() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, published) = create_and_publish(temporary.path());
    let identity = writer.identity();
    drop(writer);
    assert_released_eventually(&health, OsStr::new("unused"), identity);

    let foreign = open_withholding_write_share(&published)
        .expect("open a foreign handle that withholds write sharing");
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Active,
        "the probe cannot distinguish a legitimate writer from any other write-sharing-withholding handle"
    );
    drop(foreign);
    assert_eq!(
        probe_oplog_identity(&health, identity),
        LeaseProbe::Released
    );
}

#[test]
fn prepare_child_capture_does_not_mutate_command_configuration() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, _health, _published) = create_and_publish(temporary.path());
    let mut command = Command::new("ping");
    command.args(["-n", "1", "127.0.0.1"]);
    command.env("OPLOG_LIVENESS_TEST_VAR", "1");
    command.current_dir(temporary.path());
    let capture = writer
        .prepare_child_capture(command)
        .expect("configure child capture");
    assert_eq!(capture.get_program(), OsStr::new("ping"));
    let args: Vec<_> = capture.get_args().collect();
    assert_eq!(
        args,
        vec![OsStr::new("-n"), OsStr::new("1"), OsStr::new("127.0.0.1")]
    );
    let envs: Vec<_> = capture.get_envs().collect();
    assert!(
        envs.iter()
            .any(|(key, value)| *key == OsStr::new("OPLOG_LIVENESS_TEST_VAR")
                && *value == Some(OsStr::new("1")))
    );
    assert_eq!(capture.get_current_dir(), Some(temporary.path()));
    let mut child = capture.spawn().expect("spawn configured child");
    assert!(child.wait().expect("wait for child").success());
}

#[test]
fn capture_duplicate_failure_leaves_the_writer_live() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (mut writer, health, _published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();

    let stdout_err = run_with_oplog_capture_stdout_fault(|| {
        match writer.prepare_child_capture(ping_command("1")) {
            Ok(_) => panic!("expected a forced stdout duplicate failure"),
            Err(error) => error,
        }
    });
    assert_eq!(stdout_err.to_string(), "oplog_writer_capture_stdout");

    let stderr_err = run_with_oplog_capture_stderr_fault(|| {
        match writer.prepare_child_capture(ping_command("1")) {
            Ok(_) => panic!("expected a forced stderr duplicate failure"),
            Err(error) => error,
        }
    });
    assert_eq!(stderr_err.to_string(), "oplog_writer_capture_stderr");

    writer
        .write_all(b"sentinel-after-duplicate-failure\n")
        .unwrap();
    writer.flush().unwrap();
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );
    drop(writer);
    assert_released_eventually(&health, OsStr::new(&leaf), identity);
}

#[test]
fn spawn_error_does_not_disturb_the_writer() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, _published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();

    let capture = writer
        .prepare_child_capture(Command::new(
            "oplog-liveness-test-nonexistent-executable.exe",
        ))
        .expect("configure a capture whose program does not exist");
    let error = capture
        .spawn()
        .expect_err("spawning a missing program must fail");
    // The error carries nothing recoverable: no launcher, no stream, no raw handle.
    let _ = error.kind();

    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );
    drop(writer);
    assert_released_eventually(&health, OsStr::new(&leaf), identity);
}

#[test]
fn two_independent_captures_stay_active_until_both_children_exit() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let (writer, health, _published) = create_and_publish(temporary.path());
    let leaf = writer.leaf_name().to_owned();
    let identity = writer.identity();

    let first = writer
        .prepare_child_capture(ping_command("2"))
        .expect("configure first capture");
    let second = writer
        .prepare_child_capture(ping_command("4"))
        .expect("configure second capture");
    drop(writer);

    let mut first_child = first.spawn().expect("spawn first child");
    let mut second_child = second.spawn().expect("spawn second child");
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active
    );
    assert!(first_child.wait().expect("wait first child").success());
    assert_eq!(
        probe_oplog_lease(&health, OsStr::new(&leaf), identity),
        LeaseProbe::Active,
        "the second child family must keep the share-mode authority live alone"
    );
    assert!(second_child.wait().expect("wait second child").success());
    drop(first_child);
    drop(second_child);
    assert_released_eventually(&health, OsStr::new(&leaf), identity);
}

#[test]
fn after_admission_before_publish_is_reachable_lease_phase_primitives_are_not() {
    let after_admission = Arc::new(AtomicBool::new(false));
    let after_stage_before_lease = Arc::new(AtomicBool::new(false));
    let lease = Arc::new(AtomicBool::new(false));
    let after_lease_before_publish = Arc::new(AtomicBool::new(false));

    let temporary = tempfile::tempdir().expect("temporary directory");
    let journal = temporary.path().to_path_buf();

    {
        let after_admission = Arc::clone(&after_admission);
        run_with_oplog_create_barrier(
            OplogCreatePrimitive::AfterAdmissionBeforePublish,
            move || after_admission.store(true, Ordering::SeqCst),
            || {
                let after_stage_before_lease = Arc::clone(&after_stage_before_lease);
                run_with_oplog_create_barrier(
                    OplogCreatePrimitive::AfterStageBeforeLease,
                    move || after_stage_before_lease.store(true, Ordering::SeqCst),
                    || {
                        let lease = Arc::clone(&lease);
                        run_with_oplog_create_barrier(
                            OplogCreatePrimitive::Lease,
                            move || lease.store(true, Ordering::SeqCst),
                            || {
                                let after_lease_before_publish =
                                    Arc::clone(&after_lease_before_publish);
                                run_with_oplog_create_barrier(
                                    OplogCreatePrimitive::AfterLeaseBeforePublish,
                                    move || {
                                        after_lease_before_publish.store(true, Ordering::SeqCst)
                                    },
                                    || {
                                        create_oplog_with_test_timing(
                                            JournalRoot::open(&journal)
                                                .expect("admit trace-barrier root"),
                                            "id",
                                            "trace",
                                            OplogFormat::Log,
                                            fixed_instant(),
                                            Duration::ZERO,
                                            Duration::ZERO,
                                        )
                                        .expect("create succeeds without the unix lease phase")
                                    },
                                )
                            },
                        )
                    },
                )
            },
        );
    }

    assert!(
        after_admission.load(Ordering::SeqCst),
        "AfterAdmissionBeforePublish must be reached on Windows"
    );
    assert!(
        !after_stage_before_lease.load(Ordering::SeqCst),
        "AfterStageBeforeLease is a Unix-only lease-phase barrier"
    );
    assert!(
        !lease.load(Ordering::SeqCst),
        "Lease is a Unix-only checkpoint"
    );
    assert!(
        !after_lease_before_publish.load(Ordering::SeqCst),
        "AfterLeaseBeforePublish is a Unix-only lease-phase barrier"
    );
}

// `ERROR_LOCK_VIOLATION` classification (a competing byte-range `LockFileEx` lock
// colliding with the by-ID probe's append range while share-mode itself would
// otherwise admit the open) is not covered here: constructing that collision
// deterministically needs a second byte-range lock held at the exact offset the
// append-mode probe would touch, which this fixture cannot force reliably without
// executing on real Windows I/O timing this suite cannot observe from this host.
// The classifier's `_ => Indeterminate` fallback (`windows_liveness.rs`) already
// covers every non-success, non-`ERROR_SHARING_VIOLATION` code including this one;
// this is a documented gap in test evidence, not in the production classifier.

#[test]
#[ignore = "source-origin marker for the native Windows gate"]
fn journal_win_ci_windows_oplog_liveness_marker() {
    println!("JOURNAL_WIN_CI_TARGET_WINDOWS_OPLOG_LIVENESS=executed/pass");
}
