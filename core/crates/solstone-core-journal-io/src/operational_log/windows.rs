// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows exclusive stage, handle-bound no-replace rename, and bound lease probe.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::mem::{align_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
    ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND, GENERIC_READ, HANDLE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FileRenameInfo, SYNCHRONIZE,
    SetFileInformationByHandle,
};

use super::namespace::OplogDayHealth;
use super::reason::{NamedOccupant, OplogFileIdentity, StageError};
use crate::atomic::{ATOMIC_CANDIDATE_MARKER, publication_candidate_name};
use crate::lease::{LeaseProbe, SelfLease, acquire_self_lease, probe_file_lease};
use crate::windows_identity::file_link_count;
use crate::windows_ntcreate::nt_create_relative;
use crate::windows_sync_dir::validate_windows_regular_handle;

pub(super) struct StagedFile {
    pub file: File,
    pub stage_name: OsString,
    pub identity: OplogFileIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OplogRenameClass {
    Occupied,
    Unsupported,
    SourceAbsent,
    Ambiguous,
}

pub(super) fn stage_exclusive(
    health: &OplogDayHealth,
    dest: &OsStr,
) -> Result<StagedFile, StageError> {
    let sequence = std::process::id() as u128;
    let stage_name = publication_candidate_name(dest, ATOMIC_CANDIDATE_MARKER, &[sequence]);
    let handle = nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        stage_name.as_os_str(),
        // `LockFileEx` requires a handle opened with generic read or write
        // access. `FILE_APPEND_DATA` alone admits the append writer but is not
        // sufficient for the self-lease that guards this live stage.
        GENERIC_READ | FILE_APPEND_DATA | DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
    .map_err(|_| StageError::Allocate)?;
    let file = File::from(handle);
    let path = health
        .health()
        .diagnostic_entry_path(stage_name.as_os_str());
    let identity = match validate_windows_regular_handle(file.as_raw_handle(), &path) {
        Ok(identity) => identity,
        Err(_) => {
            drop(file);
            return Err(StageError::Leftover(stage_name));
        }
    };
    Ok(StagedFile {
        file,
        stage_name,
        identity: OplogFileIdentity::from_windows(identity.volume_serial(), identity.file_id()),
    })
}

pub(super) fn lease_staged(file: &File) -> io::Result<Option<SelfLease>> {
    acquire_self_lease(file).map_err(|error| io::Error::other(error.to_string()))
}

pub(super) fn rename_stage(
    health: &OplogDayHealth,
    staged: &StagedFile,
    dest: &OsStr,
) -> io::Result<()> {
    if super::create::force_publish_io() {
        return Err(io::Error::from_raw_os_error(ERROR_INVALID_FUNCTION as i32));
    }
    rename_open_stage_no_replace(health, &staged.file, dest)
}

pub(super) fn classify_windows_rename_error(error: &io::Error) -> OplogRenameClass {
    match error.raw_os_error() {
        Some(code) if code == ERROR_ALREADY_EXISTS as i32 => OplogRenameClass::Occupied,
        Some(code)
            if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32 =>
        {
            OplogRenameClass::SourceAbsent
        }
        Some(code)
            if code == ERROR_NOT_SUPPORTED as i32
                || code == ERROR_INVALID_FUNCTION as i32
                || code == ERROR_INVALID_PARAMETER as i32 =>
        {
            OplogRenameClass::Unsupported
        }
        _ => OplogRenameClass::Ambiguous,
    }
}

pub(super) fn inspect_named(health: &OplogDayHealth, name: &OsStr) -> io::Result<NamedOccupant> {
    let handle = match nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        name,
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return Ok(NamedOccupant::Absent);
        }
        Err(error) => return Err(error),
    };
    let path = health.health().diagnostic_entry_path(name);
    let identity = match validate_windows_regular_handle(handle.as_raw_handle(), &path) {
        Ok(identity) => identity,
        Err(_) => return Ok(NamedOccupant::Other),
    };
    let nlink = file_link_count(handle.as_raw_handle())?;
    Ok(NamedOccupant::Regular {
        identity: OplogFileIdentity::from_windows(identity.volume_serial(), identity.file_id()),
        nlink,
    })
}

pub(super) fn probe_named(health: &OplogDayHealth, leaf: &OsStr) -> LeaseProbe {
    let handle = match nt_create_relative(
        health.health().as_handle().as_raw_handle(),
        leaf,
        GENERIC_READ | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(handle) => handle,
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            return LeaseProbe::Indeterminate;
        }
        Err(_) => return LeaseProbe::Indeterminate,
    };
    let path = health.health().diagnostic_entry_path(leaf);
    if validate_windows_regular_handle(handle.as_raw_handle(), &path).is_err() {
        return LeaseProbe::Indeterminate;
    }
    let file = File::from(handle);
    probe_file_lease(&file)
}

pub(super) fn identity_of(file: &File) -> io::Result<OplogFileIdentity> {
    crate::windows_identity::file_identity(file.as_raw_handle()).map(|identity| {
        OplogFileIdentity::from_windows(identity.volume_serial(), identity.file_id())
    })
}

pub(super) fn nlink_of(file: &File) -> io::Result<u64> {
    file_link_count(file.as_raw_handle())
}

fn rename_open_stage_no_replace(
    health: &OplogDayHealth,
    stage_file: &File,
    dest_name: &OsStr,
) -> io::Result<()> {
    let wide: Vec<u16> = dest_name.encode_wide().collect();
    let extra = wide
        .len()
        .saturating_sub(1)
        .saturating_mul(size_of::<u16>());
    let bytes = size_of::<FILE_RENAME_INFO>()
        .checked_add(extra)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename buffer too large"))?;
    let words = bytes.div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let pointer = buffer.as_mut_ptr();
    debug_assert_eq!(
        pointer
            .cast::<u8>()
            .align_offset(align_of::<FILE_RENAME_INFO>()),
        0
    );
    // SAFETY: `buffer` is zeroed, aligned to `FILE_RENAME_INFO` (the `Vec<u64>`
    // allocation is at least pointer-width), sized for the fixed header plus
    // the inline filename, and live for this synchronous call.
    #[allow(unsafe_code)]
    unsafe {
        let info = pointer.cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = health.health().as_handle().as_raw_handle() as HANDLE;
        (*info).FileNameLength = (wide.len() * size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
        let result = SetFileInformationByHandle(
            stage_file.as_raw_handle(),
            FileRenameInfo,
            pointer.cast(),
            bytes as u32,
        );
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::os::windows::fs::symlink_file;
    use std::time::Duration;

    use chrono::DateTime;

    use super::super::create::{
        OPLOG_CREATE_ATTEMPTS, create_oplog_with_test_timing, run_with_oplog_file_ids,
    };
    use super::super::name::{
        derive_day_key_and_opened_field, file_id_hex, format_oplog_name, oplog_name_from_parts,
    };
    use super::super::namespace::admit_day_health_directory;
    use super::super::reason::{
        OplogIdentityObservation, OplogPublishReason, RetainedNamespaceState,
    };
    use super::super::{OplogCreateReason, OplogFormat};
    use crate::journal_root::JournalRoot;

    const ZERO: Duration = Duration::ZERO;
    const SOURCE: &str = "cortex";
    const RUN: &str = "daily-think";

    fn instant() -> DateTime<chrono::FixedOffset> {
        DateTime::parse_from_rfc3339("2026-09-01T16:42:33.381904Z").unwrap()
    }

    fn dest_for(file_id: [u8; 16]) -> String {
        let (_, opened) = derive_day_key_and_opened_field(instant());
        format_oplog_name(&oplog_name_from_parts(
            SOURCE,
            RUN,
            opened,
            file_id_hex(&file_id),
            OplogFormat::Log,
        ))
    }

    fn health_dir(root: &std::path::Path) -> std::path::PathBuf {
        let (day, _) = derive_day_key_and_opened_field(instant());
        root.join("chronicle").join(day).join("health")
    }

    #[test]
    fn eight_collisions_leave_one_stage_and_no_handle_disposition_delete() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path();
        let (day, _) = derive_day_key_and_opened_field(instant());
        admit_day_health_directory(JournalRoot::open(root).unwrap(), &day).unwrap();
        let dir = health_dir(root);
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x30 + index as u8; 16])
            .collect();
        for id in &ids {
            fs::write(dir.join(dest_for(*id)), b"preexisting").unwrap();
        }
        let error = run_with_oplog_file_ids(ids, || {
            create_oplog_with_test_timing(
                JournalRoot::open(root).unwrap(),
                SOURCE,
                RUN,
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "oplog_create_destination_exhaustion");
        assert_eq!(error.collisions().len(), 8);
        assert!(matches!(
            error.namespace(),
            RetainedNamespaceState::Established(_)
        ));
        // Product create never handle-deletes: the single leftover stage remains.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| {
                name.to_string_lossy().contains(".tmp")
                    && name != OsStr::new(".oplog-namespace.lock")
            })
            .collect();
        assert_eq!(leftovers.len(), 1);
        assert_eq!(
            error
                .observations()
                .iter()
                .filter(|observation| matches!(
                    observation,
                    OplogIdentityObservation::ForeignLanded(_)
                ))
                .count(),
            8
        );
        assert_eq!(
            error.reason(),
            OplogCreateReason::Publish(OplogPublishReason::DestinationExhaustion)
        );
    }

    #[test]
    fn reparse_point_at_candidate_is_reconciliation_not_a_foreign_collision() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path();
        let (day, _) = derive_day_key_and_opened_field(instant());
        admit_day_health_directory(JournalRoot::open(root).unwrap(), &day).unwrap();
        let dir = health_dir(root);
        let ids: Vec<[u8; 16]> = (0..OPLOG_CREATE_ATTEMPTS)
            .map(|index| [0x50 + index as u8; 16])
            .collect();
        let target = dir.join("reparse-target.bin");
        fs::write(&target, b"elsewhere").unwrap();
        // The first candidate destination is a reparse point, not a regular
        // file: the rename still reports Occupied (EEXIST), but the occupant
        // cannot be proven foreign or own, so this must classify as a
        // reconciliation failure, not a collision-history entry.
        symlink_file(&target, dir.join(dest_for(ids[0]))).unwrap();

        let error = run_with_oplog_file_ids(ids, || {
            create_oplog_with_test_timing(
                JournalRoot::open(root).unwrap(),
                SOURCE,
                RUN,
                OplogFormat::Log,
                instant(),
                ZERO,
                ZERO,
            )
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "oplog_create_reconciliation");
        assert!(
            error.collisions().is_empty(),
            "a reparse occupant is not a proven-foreign collision"
        );
        assert!(matches!(
            error.namespace(),
            RetainedNamespaceState::Established(_)
        ));
        assert_eq!(
            error.reason(),
            OplogCreateReason::Publish(OplogPublishReason::Reconciliation)
        );
    }

    #[test]
    fn windows_directory_sync_is_a_documented_no_op() {
        let source = include_str!("windows.rs");
        let production = source.split("\n#[cfg(all(test").next().unwrap_or(source);
        assert!(
            !production.contains("sync_dir("),
            "windows oplog create must not invoke a directory durability primitive"
        );
    }
}
