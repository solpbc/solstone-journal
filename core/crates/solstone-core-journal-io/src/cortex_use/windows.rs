// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::io::{AsHandle, AsRawHandle};
use std::path::Path;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GENERIC_READ};
use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_DATA, SYNCHRONIZE};

use super::{
    CortexUseCandidateRead, CortexUseDestinationCheck, CortexUseFatal, CortexUseRefusal,
    CortexUseRequest, CortexUseRootIdentity, MAXIMUM_FIRST_ROW_BYTES, expected_active_use_id,
    expected_completed_use_id, parse_cortex_use_request,
};
use crate::JournalRoot;
use crate::windows_identity::{WindowsFileIdentity, file_identity};
use crate::windows_ntcreate::nt_create_relative;
use crate::windows_sync_dir::validate_windows_regular_handle;

pub(super) fn read_cortex_use_request(
    talent_directory: &Path,
    active_leaf: &OsStr,
) -> CortexUseCandidateRead {
    read_observed_request(
        talent_directory,
        active_leaf,
        expected_active_use_id(active_leaf),
    )
}

pub(super) fn read_cortex_use_completed_request(
    talent_directory: &Path,
    completed_leaf: &OsStr,
) -> CortexUseCandidateRead {
    read_observed_request(
        talent_directory,
        completed_leaf,
        expected_completed_use_id(completed_leaf),
    )
}

fn read_observed_request(
    talent_directory: &Path,
    leaf: &OsStr,
    expected_use_id: Option<&str>,
) -> CortexUseCandidateRead {
    let first_row = match observe_stable_first_row(talent_directory, leaf) {
        Ok(first_row) => first_row,
        Err(refusal) => return refused(refusal),
    };
    let Some(expected_use_id) = expected_use_id else {
        return refused(CortexUseRefusal::InvalidRequest);
    };
    parse_cortex_use_request(
        talent_directory,
        expected_use_id,
        &first_row[..first_row.len() - 1],
    )
}

fn observe_stable_first_row(
    talent_directory: &Path,
    leaf: &OsStr,
) -> Result<Vec<u8>, CortexUseRefusal> {
    let directory = match JournalRoot::open(talent_directory).and_then(|directory| {
        directory.revalidate()?;
        Ok(directory)
    }) {
        Ok(directory) => directory,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    let path = talent_directory.join(leaf);
    let mut file = match open_regular(&directory, leaf, &path) {
        Ok((file, _)) => file,
        Err(refusal) => return Err(refusal),
    };
    let expected = match file_identity(file.as_raw_handle()) {
        Ok(identity) => identity,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    let first_row = match read_first_row(&mut file) {
        Ok(Some(row)) => row,
        Ok(None) => return Err(CortexUseRefusal::InvalidRequest),
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    match reread_first_row(&mut file, &first_row) {
        Ok(true) => {}
        Ok(false) | Err(_) => return Err(CortexUseRefusal::CandidateIdentityChanged),
    }
    if file_identity(file.as_raw_handle()).ok() != Some(expected) {
        return Err(CortexUseRefusal::CandidateIdentityChanged);
    }
    let final_identity = match open_regular(&directory, leaf, &path) {
        Ok((_, identity)) => identity,
        Err(_) => return Err(CortexUseRefusal::CandidateIdentityChanged),
    };
    if final_identity != expected || directory.revalidate().is_err() {
        return Err(CortexUseRefusal::CandidateIdentityChanged);
    }
    Ok(first_row)
}

pub(super) fn check_cortex_use_destination(
    talent_directory: &Path,
    request: &CortexUseRequest,
) -> CortexUseDestinationCheck {
    let directory = match JournalRoot::open(talent_directory).and_then(|directory| {
        directory.revalidate()?;
        Ok(directory)
    }) {
        Ok(directory) => directory,
        Err(_) => return CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationIo),
    };
    let completed = format!("{}.jsonl", request.use_id);
    match nt_create_relative(
        directory.as_handle().as_raw_handle(),
        OsStr::new(&completed),
        FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    ) {
        Ok(_) => CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationOccupied),
        Err(error)
            if matches!(
                error.raw_os_error(),
                Some(code)
                    if code == ERROR_FILE_NOT_FOUND as i32 || code == ERROR_PATH_NOT_FOUND as i32
            ) =>
        {
            CortexUseDestinationCheck::Vacant
        }
        Err(_) => CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationIo),
    }
}

pub(super) fn inspect_cortex_use_root(
    root: &Path,
) -> Result<CortexUseRootIdentity, CortexUseFatal> {
    let root = JournalRoot::open(root).map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    root.revalidate()
        .map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    let identity = file_identity(root.as_handle().as_raw_handle())
        .map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    Ok(CortexUseRootIdentity { windows: identity })
}

pub(super) fn revalidate_cortex_use_root(
    root: &Path,
    expected: &CortexUseRootIdentity,
) -> Result<(), CortexUseFatal> {
    (inspect_cortex_use_root(root)? == *expected)
        .then_some(())
        .ok_or(CortexUseFatal::RootInspectionFailed)
}

fn open_regular(
    directory: &JournalRoot,
    name: &OsStr,
    path: &Path,
) -> Result<(File, WindowsFileIdentity), CortexUseRefusal> {
    let handle = nt_create_relative(
        directory.as_handle().as_raw_handle(),
        name,
        GENERIC_READ | FILE_READ_ATTRIBUTES | FILE_READ_DATA | SYNCHRONIZE,
        FILE_OPEN,
        FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
    )
    .map_err(|_| CortexUseRefusal::CandidateIo)?;
    let file = File::from(handle);
    let identity = validate_windows_regular_handle(file.as_raw_handle(), path)
        .map_err(|_| CortexUseRefusal::CandidateNonregular)?;
    Ok((file, identity))
}

fn refused(refusal: CortexUseRefusal) -> CortexUseCandidateRead {
    CortexUseCandidateRead::Refused(refusal)
}

fn read_first_row(file: &mut File) -> io::Result<Option<Vec<u8>>> {
    let mut first_row = Vec::new();
    loop {
        if first_row.len() == MAXIMUM_FIRST_ROW_BYTES {
            return Ok(None);
        }
        let mut byte = [0; 1];
        match file.read(&mut byte)? {
            0 => return Ok(None),
            _ => {
                first_row.push(byte[0]);
                if byte[0] == b'\n' {
                    return Ok(Some(first_row));
                }
            }
        }
    }
}

fn reread_first_row(file: &mut File, expected: &[u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(0))?;
    let mut observed = vec![0; expected.len()];
    file.read_exact(&mut observed)?;
    Ok(observed == expected)
}

#[cfg(all(test, windows))]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::cortex_use::{CortexUseCandidateRead, CortexUseRefusal};

    fn active(directory: &Path, leaf: &str, row: &str) {
        fs::write(
            directory.join(leaf),
            format!("{row}\n{{\"event\":\"tail\"}}\n"),
        )
        .unwrap();
    }

    fn request(name: &str, use_id: &str) -> String {
        format!(r#"{{"name":"{name}","use_id":"{use_id}"}}"#)
    }

    #[test]
    fn completed_reader_accepts_active_looking_leaf_when_content_matches_completed_id() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(
            &directory,
            "alpha_active.jsonl",
            &request("conversation", "alpha_active"),
        );
        assert_eq!(
            read_cortex_use_completed_request(&directory, OsStr::new("alpha_active.jsonl")),
            CortexUseCandidateRead::Accepted(CortexUseRequest {
                use_id: "alpha_active".into()
            })
        );
    }

    #[test]
    fn completed_reader_accepts_a_plain_completed_leaf() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(&directory, "alpha.jsonl", &request("conversation", "alpha"));
        assert_eq!(
            read_cortex_use_completed_request(&directory, OsStr::new("alpha.jsonl")),
            CortexUseCandidateRead::Accepted(CortexUseRequest {
                use_id: "alpha".into()
            })
        );
    }

    #[test]
    fn active_reader_refuses_when_the_active_id_cannot_be_derived() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(&directory, "foo.jsonl", &request("conversation", "foo"));
        assert_eq!(
            read_cortex_use_request(&directory, OsStr::new("foo.jsonl")),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
    }

    #[test]
    fn completed_reader_refuses_when_the_completed_id_cannot_be_derived() {
        let temporary = TempDir::new().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(&directory, ".jsonl", &request("conversation", "dot"));
        assert_eq!(
            read_cortex_use_completed_request(&directory, OsStr::new(".jsonl")),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
        active(&directory, "foo.txt", &request("conversation", "foo"));
        assert_eq!(
            read_cortex_use_completed_request(&directory, OsStr::new("foo.txt")),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
    }
}
