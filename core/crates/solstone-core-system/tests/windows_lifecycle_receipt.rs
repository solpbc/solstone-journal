// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};

use filetime::{FileTime, set_file_mtime};
use solstone_core_system::lifecycle::{
    AdmissionWaitClock, HeartbeatClassification, HeartbeatV2, RunId, StaleHeartbeatCollectionError,
    SupervisorLifecycle, SupervisorLiveness, SyncIncompleteSnapshotReason, SyncScanFailure,
    SyncTickOutcome, WriterId, run_with_windows_lifecycle_checkpoint,
    run_with_windows_lifecycle_deletion_attempt_witness,
    run_with_windows_lifecycle_deletion_failure, supervisor_liveness, v2_heartbeat_filename,
};
use solstone_core_system::process::{
    windows_filetime_value_from_raw_for_test,
    windows_launch_environment_preparation_receipt_for_test,
    windows_launch_path_preparation_receipt_for_test,
};
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdInfo,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, OPEN_EXISTING,
};

const NOW: f64 = 1_000_000.0;
const STALE_AGE_SECONDS: f64 = 86_401.0;

struct ReceiptClock {
    wall_seconds: f64,
    monotonic_seconds: f64,
}

impl ReceiptClock {
    fn new() -> Self {
        Self {
            wall_seconds: NOW,
            monotonic_seconds: 1.0,
        }
    }

    fn advance(&mut self) {
        self.monotonic_seconds += 1.0;
    }
}

impl AdmissionWaitClock for ReceiptClock {
    fn wall_seconds(&mut self) -> f64 {
        self.wall_seconds
    }

    fn monotonic_seconds(&mut self) -> f64 {
        self.monotonic_seconds
    }

    fn sleep_until(&mut self, _: f64) {
        panic!("runtime ticks must not sleep");
    }
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

fn supervisor_writer_id() -> WriterId {
    WriterId::parse("0123456789abcdef0123456789abcdef").expect("supervisor writer ID")
}

fn peer_writer_id() -> WriterId {
    WriterId::parse("fedcba9876543210fedcba9876543210").expect("peer writer ID")
}

fn stale_file_time() -> FileTime {
    FileTime::from_unix_time((NOW - STALE_AGE_SECONDS) as i64, 0)
}

fn phase_root(root: &Path, label: &str) -> tempfile::TempDir {
    let mut builder = tempfile::Builder::new();
    builder.prefix(label);
    builder.tempdir_in(root).expect("create phase journal")
}

fn write_stale_peer(root: &Path, run: &str) -> (String, Vec<u8>, PathBuf) {
    let heartbeat = HeartbeatV2::new(
        peer_writer_id(),
        RunId::parse(run).expect("peer run ID"),
        "receipt-peer".to_owned(),
        7,
        (NOW - STALE_AGE_SECONDS).to_string(),
        "test".to_owned(),
        15,
        root.display().to_string(),
    );
    let filename = v2_heartbeat_filename(&heartbeat.writer_id, &heartbeat.run_id);
    let body = serde_json::to_vec(&heartbeat).expect("serialize peer heartbeat");
    let path = root.join("health/sync").join(&filename);
    fs::create_dir_all(path.parent().expect("sync parent")).expect("create sync directory");
    fs::write(&path, &body).expect("write stale peer heartbeat");
    set_file_mtime(&path, stale_file_time()).expect("age stale peer heartbeat");
    (filename, body, path)
}

fn write_bounded_malformed_peer(root: &Path) -> (String, Vec<u8>, PathBuf) {
    let filename = "bounded-malformed.check".to_owned();
    let body = b"{".to_vec();
    let path = root.join("health/sync").join(&filename);
    fs::create_dir_all(path.parent().expect("sync parent")).expect("create sync directory");
    fs::write(&path, &body).expect("write bounded malformed peer heartbeat");
    set_file_mtime(&path, stale_file_time()).expect("age bounded malformed peer heartbeat");
    (filename, body, path)
}

fn boot(root: &Path) -> SupervisorLifecycle {
    SupervisorLifecycle::boot(root, supervisor_writer_id()).expect("boot Windows lifecycle")
}

fn tick(lifecycle: &mut SupervisorLifecycle, clock: &mut ReceiptClock) -> SyncTickOutcome {
    let outcome = lifecycle.tick_sync_with(None, clock);
    clock.advance();
    outcome
}

fn assert_completed(outcome: SyncTickOutcome) {
    assert!(
        matches!(
            outcome,
            SyncTickOutcome::Healthy | SyncTickOutcome::Conflict(_)
        ),
        "expected a completed tick, got {outcome:?}"
    );
}

fn open_attributes_handle(path: &Path) -> io::Result<OwnedHandle> {
    let wide_path = wide(path.as_os_str());
    // SAFETY: `path` is NUL-terminated and the successful handle is owned exactly once.
    #[allow(unsafe_code)]
    let raw = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `raw` passed the invalid-handle sentinel check and is uniquely owned here.
    #[allow(unsafe_code)]
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn filesystem_name(path: &Path) -> io::Result<String> {
    let handle = open_attributes_handle(path)?;
    let mut filesystem_name = [0u16; 256];
    let mut volume_name = [0u16; 256];
    let mut serial = 0;
    let mut maximum_component_length = 0;
    let mut flags = 0;
    // SAFETY: the name buffers are writable for their exact supplied lengths and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetVolumeInformationByHandleW(
            handle.as_raw_handle(),
            volume_name.as_mut_ptr(),
            volume_name.len() as u32,
            &mut serial,
            &mut maximum_component_length,
            &mut flags,
            filesystem_name.as_mut_ptr(),
            filesystem_name.len() as u32,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let terminator = filesystem_name
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "filesystem name is not terminated",
            )
        })?;
    String::from_utf16(&filesystem_name[..terminator])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "filesystem name is not UTF-16"))
}

fn folded_file_id(identifier: [u8; 16]) -> u64 {
    let first = u64::from_le_bytes(
        identifier[..8]
            .try_into()
            .expect("a Windows file ID has a 64-bit first half"),
    );
    let second = u64::from_le_bytes(
        identifier[8..]
            .try_into()
            .expect("a Windows file ID has a 64-bit second half"),
    );
    first ^ second
}

fn file_identity(path: &Path) -> (u64, u64) {
    let identity = full_file_identity(path);
    (
        identity.volume_serial_number,
        folded_file_id(identity.file_id),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeFileIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

fn full_file_identity(path: &Path) -> NativeFileIdentity {
    let handle = open_attributes_handle(path).expect("open identity handle");
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is writable for its exact size and the handle is valid.
    #[allow(unsafe_code)]
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    assert_ne!(result, 0, "query file identity for {}", path.display());
    NativeFileIdentity {
        volume_serial_number: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    }
}

fn completed_scan_identity(lifecycle: &SupervisorLifecycle, filename: &str) -> (u64, u64) {
    let observation = lifecycle
        .last_completed_sync_result()
        .expect("completed scan result")
        .snapshot
        .files
        .get(OsStr::new(filename))
        .expect("file observed in completed scan");
    (observation.entry.device, observation.entry.inode)
}

fn phase_a_two_tick_deletion(root: &Path) {
    let temporary = phase_root(root, "stale-heartbeat-phase-a-");
    let (_filename, _body, peer) =
        write_stale_peer(temporary.path(), "11111111111111111111111111111111");
    let mut lifecycle = boot(temporary.path());
    let mut clock = ReceiptClock::new();

    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(
        peer.exists(),
        "first stale-GC tick must retain the candidate"
    );
    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(!peer.exists(), "second unchanged stale-GC tick must delete");
}

fn phase_b_bounded_malformed_is_never_deleted(root: &Path) {
    let temporary = phase_root(root, "stale-heartbeat-phase-b-");
    let (filename, body, peer) = write_bounded_malformed_peer(temporary.path());
    let pre_tick_identity = file_identity(&peer);
    let mut lifecycle = boot(temporary.path());
    let mut clock = ReceiptClock::new();

    let ((), deletion_attempts) = run_with_windows_lifecycle_deletion_attempt_witness(|| {
        assert_completed(tick(&mut lifecycle, &mut clock));
        let completed = lifecycle
            .last_completed_sync_result()
            .expect("bounded malformed tick completed");
        let observation = completed
            .peer_observations
            .iter()
            .find(|observation| observation.source_filename == OsStr::new(&filename))
            .expect("bounded malformed peer observed");
        assert!(matches!(
            &observation.classification,
            HeartbeatClassification::BoundedMalformed
        ));
        assert_eq!(
            completed_scan_identity(&lifecycle, &filename),
            pre_tick_identity,
            "completed-scan identity must match the pre-tick native identity after tick 1"
        );
        assert_eq!(fs::read(&peer).expect("read bounded malformed peer"), body);
        assert!(peer.exists());

        assert_completed(tick(&mut lifecycle, &mut clock));
        let completed = lifecycle
            .last_completed_sync_result()
            .expect("bounded malformed tick completed");
        let observation = completed
            .peer_observations
            .iter()
            .find(|observation| observation.source_filename == OsStr::new(&filename))
            .expect("bounded malformed peer observed after second tick");
        assert!(matches!(
            &observation.classification,
            HeartbeatClassification::BoundedMalformed
        ));
        assert_eq!(
            completed_scan_identity(&lifecycle, &filename),
            pre_tick_identity,
            "completed-scan identity must match the pre-tick native identity after tick 2"
        );
        assert_eq!(
            fs::read(&peer).expect("retain bounded malformed peer"),
            body
        );
        assert!(peer.exists());
    });
    assert_eq!(
        deletion_attempts, 0,
        "bounded malformed heartbeats must never reach deletion"
    );
}

fn phase_c_incomplete_scan_resets_candidates(root: &Path) {
    let temporary = phase_root(root, "stale-heartbeat-phase-c-");
    let mut lifecycle = boot(temporary.path());
    let mut clock = ReceiptClock::new();

    let (_control_name, _control_body, control) =
        write_stale_peer(temporary.path(), "22222222222222222222222222222222");
    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(
        control.exists(),
        "disarmed checkpoint scan must complete normally"
    );
    fs::remove_file(&control).expect("remove disarmed-control heartbeat");

    let (raced_name, _raced_body, raced) =
        write_stale_peer(temporary.path(), "33333333333333333333333333333333");
    let (_survivor_name, _survivor_body, survivor) =
        write_stale_peer(temporary.path(), "44444444444444444444444444444444");
    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(raced.exists(), "raced heartbeat has a first-tick candidate");
    assert!(
        survivor.exists(),
        "survivor heartbeat has a first-tick candidate"
    );

    let (outcome, fired) = run_with_windows_lifecycle_checkpoint(
        "scan-before-observed-read",
        OsStr::new(&raced_name),
        move || fs::remove_file(&raced).expect("delete raced heartbeat at checkpoint"),
        || tick(&mut lifecycle, &mut clock),
    );
    assert!(fired, "enumeration-to-read checkpoint did not fire");
    assert!(matches!(
        outcome,
        SyncTickOutcome::CompleteScanFailure(SyncScanFailure::IncompleteSnapshot {
            reason: SyncIncompleteSnapshotReason::DisappearedDuringObservation,
            ..
        })
    ));
    assert!(
        survivor.exists(),
        "incomplete scan must not collect the survivor"
    );

    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(
        survivor.exists(),
        "first successful recovery tick must establish fresh evidence"
    );
    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(
        !survivor.exists(),
        "second successful recovery tick must collect the survivor"
    );
}

fn phase_d_replacement_identity_requires_two_fresh_ticks(root: &Path) {
    let temporary = phase_root(root, "stale-heartbeat-phase-d-");
    let (replaced_name, body, replaced) =
        write_stale_peer(temporary.path(), "55555555555555555555555555555555");
    let (_control_name, _control_body, control) =
        write_stale_peer(temporary.path(), "66666666666666666666666666666666");
    let original_mtime = FileTime::from_last_modification_time(
        &fs::metadata(&replaced).expect("replacement fixture metadata"),
    );
    let mut lifecycle = boot(temporary.path());
    let mut clock = ReceiptClock::new();

    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(replaced.exists());
    assert!(control.exists());
    let original_identity = file_identity(&replaced);
    let observed_original_identity = completed_scan_identity(&lifecycle, &replaced_name);
    assert_eq!(
        observed_original_identity, original_identity,
        "completed-scan identity must match the native identity cross-check"
    );

    let replacement = replaced.with_extension("replacement");
    fs::write(&replacement, &body).expect("write replacement heartbeat");
    set_file_mtime(&replacement, original_mtime).expect("preserve replacement mtime");
    fs::remove_file(&replaced).expect("remove original heartbeat before replacement");
    fs::rename(&replacement, &replaced).expect("install replacement heartbeat");
    assert_eq!(
        fs::read(&replaced).expect("read replacement heartbeat"),
        body
    );
    assert_eq!(
        FileTime::from_last_modification_time(
            &fs::metadata(&replaced).expect("replacement metadata"),
        ),
        original_mtime,
        "replacement must retain the original mtime"
    );
    let replacement_identity = file_identity(&replaced);
    assert_ne!(
        replacement_identity, original_identity,
        "replacement must have a distinct production-equivalent native identity"
    );

    assert_completed(tick(&mut lifecycle, &mut clock));
    let observed_replacement_identity = completed_scan_identity(&lifecycle, &replaced_name);
    assert_ne!(
        observed_replacement_identity, observed_original_identity,
        "completed-scan identity must change for the replacement"
    );
    assert_eq!(
        observed_replacement_identity, replacement_identity,
        "completed-scan replacement identity must match the native identity cross-check"
    );
    assert!(
        replaced.exists(),
        "identity replacement must not satisfy the second candidate tick"
    );
    assert!(
        !control.exists(),
        "unchanged-identity control must delete on its second tick"
    );
    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(
        !replaced.exists(),
        "replacement must delete on its second fresh unchanged tick"
    );
}

fn phase_e_deletion_failure_reports_and_recovers(root: &Path) {
    let temporary = phase_root(root, "stale-heartbeat-phase-e-");
    let (filename, body, peer) =
        write_stale_peer(temporary.path(), "77777777777777777777777777777777");
    let retained_identity = full_file_identity(&peer);
    let mut lifecycle = boot(temporary.path());
    let mut clock = ReceiptClock::new();

    assert_completed(tick(&mut lifecycle, &mut clock));
    assert!(peer.exists(), "first phase-E tick establishes a candidate");
    let ((outcome, deletion_attempts), failure_fired) =
        run_with_windows_lifecycle_deletion_failure(|| {
            run_with_windows_lifecycle_deletion_attempt_witness(|| tick(&mut lifecycle, &mut clock))
        });
    assert!(failure_fired, "phase-E deletion failure did not fire");
    match outcome {
        SyncTickOutcome::StaleHeartbeatCollectionFailure(
            StaleHeartbeatCollectionError::WindowsRemoval {
                filename: failed_filename,
                reason,
            },
        ) => {
            assert_eq!(failed_filename, OsStr::new(&filename));
            assert_eq!(reason, "injected Windows lifecycle deletion failure");
        }
        outcome => panic!("phase-E fault must fail the collecting tick, got {outcome:?}"),
    }
    assert_eq!(
        deletion_attempts, 1,
        "phase-E fault must follow exactly one identity-verified deletion attempt"
    );
    assert_eq!(fs::read(&peer).expect("phase-E peer remains on disk"), body);
    assert_eq!(
        full_file_identity(&peer),
        retained_identity,
        "failed deletion must retain the exact native file identity"
    );

    let (outcome, deletion_attempts) =
        run_with_windows_lifecycle_deletion_attempt_witness(|| tick(&mut lifecycle, &mut clock));
    assert_completed(outcome);
    assert_eq!(
        deletion_attempts, 0,
        "first recovery tick must not attempt deletion"
    );
    assert!(
        peer.exists(),
        "first post-release tick establishes fresh evidence"
    );
    let (outcome, deletion_attempts) =
        run_with_windows_lifecycle_deletion_attempt_witness(|| tick(&mut lifecycle, &mut clock));
    assert_completed(outcome);
    assert_eq!(
        deletion_attempts, 1,
        "second recovery tick must attempt deletion exactly once"
    );
    assert!(!peer.exists(), "second post-release tick deletes the peer");
}

fn phase_f_process_instance_mutation_control(root: &Path) {
    let temporary = phase_root(root, "process-instance-phase-f-");
    let _lifecycle = boot(temporary.path());
    let process_instance_path = temporary.path().join("health/supervisor.process_instance");
    assert_eq!(
        supervisor_liveness(temporary.path()),
        SupervisorLiveness::Up
    );

    let original = fs::read(&process_instance_path).expect("read published process instance");
    let mut mutated: serde_json::Value =
        serde_json::from_slice(&original).expect("parse published process instance");
    let token = mutated["birth"]["filetime"]
        .as_u64()
        .expect("published Windows FILETIME");
    mutated["birth"]["filetime"] = serde_json::Value::from(token.wrapping_add(1));
    fs::write(
        &process_instance_path,
        serde_json::to_vec(&mutated).expect("serialize mutated process instance"),
    )
    .expect("write mutated process instance");
    assert_eq!(
        supervisor_liveness(temporary.path()),
        SupervisorLiveness::Down
    );

    fs::write(&process_instance_path, "not JSON").expect("write malformed process token");
    assert_eq!(
        supervisor_liveness(temporary.path()),
        SupervisorLiveness::Unverifiable
    );

    let mut unknown: serde_json::Value =
        serde_json::from_slice(&original).expect("reparse published process instance");
    unknown["birth"] = serde_json::json!({"kind": "future-kind", "filetime": 1});
    fs::write(
        &process_instance_path,
        serde_json::to_vec(&unknown).expect("serialize unknown process token"),
    )
    .expect("write unknown process token");
    assert_eq!(
        supervisor_liveness(temporary.path()),
        SupervisorLiveness::Unverifiable
    );
}

fn stale_heartbeat_cleanup_receipt(root: &Path) {
    assert_eq!(
        windows_filetime_value_from_raw_for_test(0x0123_4567, 0x89ab_cdef),
        0x0123_4567_89ab_cdef,
        "native receipt must execute the production raw-FILETIME conversion boundary"
    );
    phase_a_two_tick_deletion(root);
    phase_b_bounded_malformed_is_never_deleted(root);
    phase_c_incomplete_scan_resets_candidates(root);
    phase_d_replacement_identity_requires_two_fresh_ticks(root);
    phase_e_deletion_failure_reports_and_recovers(root);
    phase_f_process_instance_mutation_control(root);
}

#[test]
#[ignore = "requires a native NTFS filesystem"]
fn ntfs_stale_heartbeat_cleanup_receipt() {
    let root = tempfile::tempdir().expect("create NTFS receipt root");
    assert_eq!(filesystem_name(root.path()).unwrap(), "NTFS");
    stale_heartbeat_cleanup_receipt(root.path());
    println!("JOURNAL_WIN_CI_NTFS_STALE_HEARTBEAT_CLEANUP=executed/pass");
    println!("JOURNAL_WIN_CI_NTFS_STALE_HEARTBEAT_CLEANUP_FILESYSTEM=NTFS");
}

#[test]
#[ignore = "requires the native ReFS fixture selected by win-ci.cmd"]
fn refs_stale_heartbeat_cleanup_receipt() {
    let root = std::env::var_os("SOLSTONE_JOURNAL_WIN_REFS_ROOT")
        .map(PathBuf::from)
        .expect("ReFS stale-heartbeat receipt requires SOLSTONE_JOURNAL_WIN_REFS_ROOT");
    assert_eq!(filesystem_name(&root).unwrap(), "ReFS");
    let temporary = tempfile::Builder::new()
        .prefix("solstone-refs-stale-heartbeat-")
        .tempdir_in(&root)
        .expect("create ReFS receipt root");
    stale_heartbeat_cleanup_receipt(temporary.path());
    println!("JOURNAL_WIN_CI_REFS_STALE_HEARTBEAT_CLEANUP=executed/pass");
    println!("JOURNAL_WIN_CI_REFS_STALE_HEARTBEAT_CLEANUP_FILESYSTEM=ReFS");
}

#[test]
#[ignore = "requires native Windows UTF-16 and ordinal APIs"]
fn windows_launch_environment_preparation_receipt() {
    windows_launch_environment_preparation_receipt_for_test()
        .expect("exercise production Windows environment preparation");
    println!("JOURNAL_WIN_CI_LAUNCH_ENVIRONMENT_PREPARATION=executed/pass");
}

#[test]
#[ignore = "requires native Windows path APIs"]
fn windows_launch_path_preparation_receipt() {
    windows_launch_path_preparation_receipt_for_test()
        .expect("exercise production Windows path preparation");
    println!("JOURNAL_WIN_CI_LAUNCH_PATH_PREPARATION=executed/pass");
}
