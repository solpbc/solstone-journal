// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Resolve one Windows managed-log alias without reopening an authority path.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;

use crate::errors::FlatDirectoryError;
use crate::managed_log_names::{canonical_payload_name, root_alias_name};
use crate::windows_identity::WindowsFileIdentity;
use crate::windows_managed_log_open::{
    ManagedLogOpenError, open_canonical_for_read, read_pointer_record_bounded,
};
use crate::windows_managed_log_record::{
    MAX_MANAGED_LOG_RECORD_BYTES, ManagedLogRecord, ManagedLogRecordError,
};
use crate::windows_sync_dir::WindowsFlatDirectory;

/// A canonical log file retained through the resolution result.
pub(crate) struct ResolvedManagedLog {
    pub(crate) record: ManagedLogRecord,
    pub(crate) identity: WindowsFileIdentity,
    pub(crate) file: File,
}

/// Resolve exactly one root alias and retain the verified canonical reader.
pub(crate) fn resolve_managed_log_record<F>(
    alias_parent: &WindowsFlatDirectory,
    alias_name: &OsStr,
    bind_day_health: F,
) -> Result<ResolvedManagedLog, ManagedLogResolveError>
where
    F: FnOnce(&str) -> Result<WindowsFlatDirectory, FlatDirectoryError>,
{
    let (_alias_identity, bytes) =
        read_pointer_record_bounded(alias_parent, alias_name, MAX_MANAGED_LOG_RECORD_BYTES)?;
    let record = ManagedLogRecord::parse(&bytes)?;
    let expected_alias = root_alias_name(record.name());
    if alias_name != expected_alias {
        return Err(ManagedLogResolveError::AliasRecordMismatch {
            actual: alias_name.to_os_string(),
            expected: expected_alias,
        });
    }

    // The record and artifact coordinate are now proven before the target opens.
    let day_health = bind_day_health(record.day())?;
    let canonical_name = canonical_payload_name(record.reference(), record.name());
    let opened =
        open_canonical_for_read(&day_health, &canonical_name, record.canonical_identity())?;
    Ok(ResolvedManagedLog {
        identity: opened.identity(),
        file: opened.into_file(),
        record,
    })
}

#[derive(Debug)]
pub(crate) enum ManagedLogResolveError {
    FlatDirectory(FlatDirectoryError),
    Open(ManagedLogOpenError),
    Record(ManagedLogRecordError),
    AliasRecordMismatch {
        actual: std::ffi::OsString,
        expected: std::ffi::OsString,
    },
}

impl From<FlatDirectoryError> for ManagedLogResolveError {
    fn from(error: FlatDirectoryError) -> Self {
        Self::FlatDirectory(error)
    }
}

impl From<ManagedLogOpenError> for ManagedLogResolveError {
    fn from(error: ManagedLogOpenError) -> Self {
        Self::Open(error)
    }
}

impl From<ManagedLogRecordError> for ManagedLogResolveError {
    fn from(error: ManagedLogRecordError) -> Self {
        Self::Record(error)
    }
}

impl fmt::Display for ManagedLogResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FlatDirectory(error) => error.fmt(formatter),
            Self::Open(error) => error.fmt(formatter),
            Self::Record(error) => error.fmt(formatter),
            Self::AliasRecordMismatch { actual, expected } => write!(
                formatter,
                "managed-log alias {actual:?} does not encode the record name (expected {expected:?})"
            ),
        }
    }
}

impl Error for ManagedLogResolveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FlatDirectory(error) => Some(error),
            Self::Open(error) => Some(error),
            Self::Record(error) => Some(error),
            Self::AliasRecordMismatch { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom};
    use std::os::windows::io::AsHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_TRAVERSE, OPEN_EXISTING,
    };

    use super::*;
    use crate::locking::open_windows_path;
    use crate::managed_log_names::canonical_payload_name;
    use crate::test_support::TempDir;
    use crate::windows_identity::file_identity;
    use crate::windows_managed_log_record::ManagedLogRecord;

    fn root_handle(path: &std::path::Path) -> std::fs::File {
        open_windows_path(
            path,
            FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
        .unwrap()
    }

    fn child(
        parent: &impl AsHandle,
        parent_path: &std::path::Path,
        name: &str,
    ) -> WindowsFlatDirectory {
        crate::windows_sync_dir::create_or_open_windows_flat_directory_bound(
            parent,
            OsStr::new(name),
            parent_path,
        )
        .unwrap()
    }

    #[test]
    fn resolver_returns_exact_retained_identity_and_refuses_a_replaced_payload() {
        let temporary = TempDir::new();
        let root = root_handle(temporary.path());
        let aliases = child(&root, temporary.path(), "aliases");
        let days = child(&root, temporary.path(), "days");
        let day = child(&days, &temporary.path().join("days"), "20260829");
        let _health = child(
            &day,
            &temporary.path().join("days").join("20260829"),
            "health",
        );
        let payload_name = canonical_payload_name("writer", "stream");
        let payload_path = temporary
            .path()
            .join("days")
            .join("20260829")
            .join("health")
            .join(&payload_name);
        fs::write(&payload_path, b"labelled original payload").unwrap();
        let payload = std::fs::File::open(&payload_path).unwrap();
        let identity =
            file_identity(std::os::windows::io::AsRawHandle::as_raw_handle(&payload)).unwrap();
        let record = ManagedLogRecord::new(
            1,
            "20260829".into(),
            "writer".into(),
            "stream".into(),
            identity,
        )
        .unwrap();
        let alias_name = crate::managed_log_names::root_alias_name("stream");
        fs::write(
            temporary.path().join("aliases").join(&alias_name),
            record.to_bytes().unwrap(),
        )
        .unwrap();

        let mut resolved = resolve_managed_log_record(&aliases, &alias_name, |_| {
            Ok(crate::windows_sync_dir::open_windows_flat_directory_bound(
                &day,
                OsStr::new("health"),
                &temporary.path().join("days").join("20260829"),
            )?
            .unwrap())
        })
        .unwrap();
        assert_eq!(resolved.identity, identity);
        fs::rename(&payload_path, payload_path.with_extension("old")).unwrap();
        fs::write(&payload_path, b"replacement payload").unwrap();
        resolved.file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        resolved.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"labelled original payload");
        assert!(
            resolve_managed_log_record(&aliases, &alias_name, |_| {
                Ok(crate::windows_sync_dir::open_windows_flat_directory_bound(
                    &day,
                    OsStr::new("health"),
                    &temporary.path().join("days").join("20260829"),
                )?
                .unwrap())
            })
            .is_err()
        );
    }

    #[test]
    fn resolver_rejects_an_alias_name_mismatch_before_binding_a_target() {
        let temporary = TempDir::new();
        let root = root_handle(temporary.path());
        let aliases = child(&root, temporary.path(), "aliases");
        let record = ManagedLogRecord::new(
            1,
            "20260829".into(),
            "writer".into(),
            "stream".into(),
            WindowsFileIdentity::from_parts(1, [1; 16]),
        )
        .unwrap();
        let wrong_alias = crate::managed_log_names::root_alias_name("other");
        fs::write(
            temporary.path().join("aliases").join(&wrong_alias),
            record.to_bytes().unwrap(),
        )
        .unwrap();
        let result = resolve_managed_log_record(&aliases, &wrong_alias, |_| {
            panic!("target callback must not run after alias record mismatch")
        });
        assert!(matches!(
            result,
            Err(ManagedLogResolveError::AliasRecordMismatch { .. })
        ));
    }
}
