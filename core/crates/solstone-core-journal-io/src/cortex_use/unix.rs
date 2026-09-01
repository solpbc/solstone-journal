// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

use super::{
    CortexUseCandidateRead, CortexUseDestinationCheck, CortexUseFatal, CortexUseRefusal,
    CortexUseRequest, CortexUseRootIdentity, MAXIMUM_FIRST_ROW_BYTES, expected_active_use_id,
    expected_completed_use_id, parse_cortex_use_request,
};
use crate::JournalEntryKind;

const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);
const FILE_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

/// Ordered checkpoints for one bounded Cortex-use first-row read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CortexUseReadPrimitive {
    /// The initial no-follow observation of the active leaf.
    InitialNameObserve,
    /// The no-follow active-leaf open.
    Open,
    /// The regular-file and identity observation of the opened handle.
    OpenedHandleObserve,
    /// Reading through the bounded first newline.
    FirstRowRead,
    /// Rereading the exact first-row bytes through the same handle.
    FirstRowReread,
    /// The final identity observation of the opened handle.
    FinalHandleObserve,
    /// The final no-follow active-leaf observation.
    FinalNameObserve,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct CortexUseReadTraceState {
    attempted: Vec<CortexUseReadPrimitive>,
    fault: Option<CortexUseReadFault>,
    fault_consumed: bool,
    barriers: Vec<CortexUseReadBarrier>,
    barriers_fired: usize,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct CortexUseReadFault {
    primitive: CortexUseReadPrimitive,
    ordinal: usize,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct CortexUseReadBarrier {
    primitive: CortexUseReadPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
thread_local! {
    static CORTEX_USE_READ_TRACE: std::cell::RefCell<Option<CortexUseReadTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run an operation with one injected Cortex-use read fault.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_cortex_use_read_fault<T>(
    primitive: CortexUseReadPrimitive,
    ordinal: usize,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, consumed, _) = run_with_trace(
        Some(CortexUseReadFault { primitive, ordinal }),
        Vec::new(),
        operation,
    );
    (result, consumed)
}

/// Run an operation with one deterministic Cortex-use read barrier.
#[cfg(all(unix, feature = "test-hooks"))]
pub fn run_with_cortex_use_read_barrier<T>(
    primitive: CortexUseReadPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    operation: impl FnOnce() -> T,
) -> (T, bool) {
    let (result, _, fired) = run_with_trace(
        None,
        vec![CortexUseReadBarrier {
            primitive,
            ordinal,
            callback: Box::new(callback),
        }],
        operation,
    );
    (result, fired == 1)
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
fn run_with_trace<T>(
    fault: Option<CortexUseReadFault>,
    barriers: Vec<CortexUseReadBarrier>,
    operation: impl FnOnce() -> T,
) -> (T, bool, usize) {
    CORTEX_USE_READ_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "Cortex-use read trace is already active"
        );
        *trace.borrow_mut() = Some(CortexUseReadTraceState {
            attempted: Vec::new(),
            fault,
            fault_consumed: false,
            barriers,
            barriers_fired: 0,
        });
    });
    let result = operation();
    let state = CORTEX_USE_READ_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("Cortex-use read trace remains active")
    });
    (result, state.fault_consumed, state.barriers_fired)
}

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
    let directory = match open(talent_directory, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(directory) => directory,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    let initial = observe_name(&directory, leaf, CortexUseReadPrimitive::InitialNameObserve)?;
    if !is_regular(&initial) {
        return Err(CortexUseRefusal::CandidateNonregular);
    }
    let expected = identity(&initial);

    checkpoint(CortexUseReadPrimitive::Open)?;
    let descriptor = match openat(&directory, leaf, FILE_FLAGS, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    checkpoint(CortexUseReadPrimitive::OpenedHandleObserve)?;
    let opened = match fstat(&descriptor) {
        Ok(status) => status,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    if !is_regular(&opened) {
        return Err(CortexUseRefusal::CandidateNonregular);
    }
    if identity(&opened) != expected {
        return Err(CortexUseRefusal::CandidateIdentityChanged);
    }

    let mut file = File::from(descriptor);
    checkpoint(CortexUseReadPrimitive::FirstRowRead)?;
    let first_row = match read_first_row(&mut file) {
        Ok(Some(row)) => row,
        Ok(None) => return Err(CortexUseRefusal::InvalidRequest),
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    checkpoint(CortexUseReadPrimitive::FirstRowReread)?;
    match reread_first_row(&mut file, &first_row) {
        Ok(true) => {}
        Ok(false) => return Err(CortexUseRefusal::CandidateIdentityChanged),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(CortexUseRefusal::CandidateIdentityChanged);
        }
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    }

    checkpoint(CortexUseReadPrimitive::FinalHandleObserve)?;
    let final_handle = match fstat(&file) {
        Ok(status) => status,
        Err(_) => return Err(CortexUseRefusal::CandidateIo),
    };
    if !is_regular(&final_handle) || identity(&final_handle) != expected {
        return Err(CortexUseRefusal::CandidateIdentityChanged);
    }
    let final_name = match observe_name(&directory, leaf, CortexUseReadPrimitive::FinalNameObserve)
    {
        Ok(status) => status,
        Err(_) => return Err(CortexUseRefusal::CandidateIdentityChanged),
    };
    if !is_regular(&final_name) || identity(&final_name) != expected {
        return Err(CortexUseRefusal::CandidateIdentityChanged);
    }
    Ok(first_row)
}

pub(super) fn check_cortex_use_destination(
    talent_directory: &Path,
    request: &CortexUseRequest,
) -> CortexUseDestinationCheck {
    let directory = match open(talent_directory, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(directory) => directory,
        Err(_) => return CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationIo),
    };
    let completed = format!("{}.jsonl", request.use_id);
    match fstatat(&directory, completed.as_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(_) => CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationOccupied),
        Err(Errno::ENOENT) => CortexUseDestinationCheck::Vacant,
        Err(_) => CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationIo),
    }
}

pub(super) fn inspect_cortex_use_root(
    root: &Path,
) -> Result<CortexUseRootIdentity, CortexUseFatal> {
    let observed = fstatat(AT_FDCWD, root, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    if JournalEntryKind::from_mode(SFlag::from_bits_truncate(observed.st_mode))
        != JournalEntryKind::Directory
    {
        return Err(CortexUseFatal::RootInspectionFailed);
    }
    let descriptor = open(root, DIRECTORY_FLAGS, Mode::empty())
        .map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    let opened = fstat(&descriptor).map_err(|_| CortexUseFatal::RootInspectionFailed)?;
    (identity(&observed) == identity(&opened) && is_directory(&opened))
        .then_some(CortexUseRootIdentity {
            unix: identity(&opened),
        })
        .ok_or(CortexUseFatal::RootInspectionFailed)
}

pub(super) fn revalidate_cortex_use_root(
    root: &Path,
    expected: &CortexUseRootIdentity,
) -> Result<(), CortexUseFatal> {
    (inspect_cortex_use_root(root)? == *expected)
        .then_some(())
        .ok_or(CortexUseFatal::RootInspectionFailed)
}

fn refused(refusal: CortexUseRefusal) -> CortexUseCandidateRead {
    CortexUseCandidateRead::Refused(refusal)
}

fn observe_name(
    directory: &impl std::os::fd::AsFd,
    active_leaf: &OsStr,
    primitive: CortexUseReadPrimitive,
) -> Result<FileStat, CortexUseRefusal> {
    checkpoint(primitive)?;
    fstatat(directory, active_leaf, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|_| CortexUseRefusal::CandidateIo)
}

fn is_regular(status: &FileStat) -> bool {
    SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG
}

fn is_directory(status: &FileStat) -> bool {
    SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFDIR
}

fn identity(status: &FileStat) -> (libc::dev_t, libc::ino_t) {
    (status.st_dev, status.st_ino)
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

#[cfg(all(unix, any(test, feature = "test-hooks")))]
fn checkpoint(primitive: CortexUseReadPrimitive) -> Result<(), CortexUseRefusal> {
    let (fault, barrier) = CORTEX_USE_READ_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (false, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        if state
            .fault
            .as_ref()
            .is_some_and(|fault| fault.primitive == primitive && fault.ordinal == ordinal)
        {
            state.fault.take();
            state.fault_consumed = true;
            (true, None)
        } else if let Some(index) = state
            .barriers
            .iter()
            .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal)
        {
            let barrier = state.barriers.remove(index);
            state.barriers_fired += 1;
            (false, Some(barrier.callback))
        } else {
            (false, None)
        }
    });
    if fault {
        return Err(match primitive {
            CortexUseReadPrimitive::OpenedHandleObserve
            | CortexUseReadPrimitive::FinalHandleObserve
            | CortexUseReadPrimitive::FinalNameObserve => {
                CortexUseRefusal::CandidateIdentityChanged
            }
            _ => CortexUseRefusal::CandidateIo,
        });
    }
    if let Some(barrier) = barrier {
        barrier();
    }
    Ok(())
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
fn checkpoint(_primitive: CortexUseReadPrimitive) -> Result<(), CortexUseRefusal> {
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsStr;
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::cortex_use::{
        CortexUseCandidateRead, CortexUseFatal, CortexUseRefusal, inspect_cortex_use_root,
        revalidate_cortex_use_root, talent_directory_name,
    };

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

    fn read(directory: &Path, leaf: &str) -> CortexUseCandidateRead {
        read_cortex_use_request(directory, OsStr::new(leaf))
    }

    #[test]
    fn accepts_native_and_historical_requests_including_active_legacy_ids() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(
            &directory,
            "one_active.jsonl",
            &request("conversation", "one"),
        );
        assert_eq!(
            read(&directory, "one_active.jsonl"),
            CortexUseCandidateRead::Accepted(CortexUseRequest {
                use_id: "one".into()
            })
        );
        active(
            &directory,
            "foo_active_active.jsonl",
            r#"{"event":"request","name":"conversation","use_id":"foo_active"}"#,
        );
        assert_eq!(
            read(&directory, "foo_active_active.jsonl"),
            CortexUseCandidateRead::Accepted(CortexUseRequest {
                use_id: "foo_active".into()
            })
        );
    }

    #[test]
    fn completed_reader_accepts_active_looking_leaf_when_content_matches_completed_id() {
        let temporary = tempdir().unwrap();
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
        let temporary = tempdir().unwrap();
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
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(&directory, "foo.jsonl", &request("conversation", "foo"));
        assert_eq!(
            read(&directory, "foo.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
    }

    #[test]
    fn completed_reader_refuses_when_the_completed_id_cannot_be_derived() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(&directory, ".jsonl", &request("conversation", "dot"));
        assert_eq!(
            read_cortex_use_completed_request(&directory, OsStr::new(".jsonl")),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
        use std::os::unix::ffi::OsStringExt;
        let non_utf8 = std::ffi::OsString::from_vec(b"x-\xff.jsonl".to_vec());
        active(
            &directory,
            "placeholder.jsonl",
            &request("conversation", "placeholder"),
        );
        fs::rename(
            directory.join("placeholder.jsonl"),
            directory.join(&non_utf8),
        )
        .unwrap();
        assert_eq!(
            read_cortex_use_completed_request(&directory, &non_utf8),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
    }

    #[test]
    fn rejects_invalid_first_rows_and_identity_projection_mismatches() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        for (case, event) in [
            ("event-null", "null"),
            ("event-other", "\"finish\""),
            ("event-bool", "true"),
            ("event-number", "1"),
            ("event-array", "[]"),
            ("event-object", "{}"),
        ] {
            let leaf = format!("{case}_active.jsonl");
            let row = format!(r#"{{"event":{event},"name":"conversation","use_id":"{case}"}}"#);
            active(&directory, &leaf, &row);
            assert_eq!(
                read(&directory, &leaf),
                CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest),
                "{case}"
            );
        }
        for (case, row) in [
            ("missing-name", r#"{"use_id":"missing-name"}"#),
            ("missing-id", r#"{"name":"conversation"}"#),
            ("empty-id", r#"{"name":"conversation","use_id":""}"#),
            ("empty-name", r#"{"name":"","use_id":"empty-name"}"#),
            ("wrong-name", r#"{"name":"other","use_id":"wrong-name"}"#),
            ("wrong-id", r#"{"name":"conversation","use_id":"other"}"#),
            ("malformed", "{"),
        ] {
            let leaf = format!("{case}_active.jsonl");
            active(&directory, &leaf, row);
            assert_eq!(
                read(&directory, &leaf),
                CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest),
                "{case}"
            );
        }
        fs::write(
            directory.join("missing-newline_active.jsonl"),
            request("conversation", "missing-newline"),
        )
        .unwrap();
        assert_eq!(
            read(&directory, "missing-newline_active.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
    }

    #[test]
    fn rejects_a_first_row_larger_than_64_kib_and_nonregular_leaves() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        let too_long = "x".repeat(MAXIMUM_FIRST_ROW_BYTES);
        fs::write(directory.join("one_active.jsonl"), format!("{too_long}\n")).unwrap();
        assert_eq!(
            read(&directory, "one_active.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::InvalidRequest)
        );
        fs::create_dir(directory.join("directory_active.jsonl")).unwrap();
        assert_eq!(
            read(&directory, "directory_active.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateNonregular)
        );
        std::os::unix::fs::symlink("missing", directory.join("link_active.jsonl")).unwrap();
        assert_eq!(
            read(&directory, "link_active.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateNonregular)
        );
        let fifo = directory.join("fifo_active.jsonl");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
        assert_eq!(
            read(&directory, "fifo_active.jsonl"),
            CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateNonregular)
        );
    }

    #[test]
    fn reread_and_final_name_checks_reject_replacement_and_corruption() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(
            &directory,
            "one_active.jsonl",
            &request("conversation", "one"),
        );
        let replacement = directory.join("replacement.jsonl");
        fs::write(
            &replacement,
            format!("{}\n", request("conversation", "one")),
        )
        .unwrap();
        let directory_for_barrier = directory.clone();
        let replacement_for_barrier = replacement.clone();
        let (result, _, fired) = run_with_trace(
            None,
            vec![CortexUseReadBarrier {
                primitive: CortexUseReadPrimitive::FinalNameObserve,
                ordinal: 1,
                callback: Box::new(move || {
                    fs::rename(
                        &replacement_for_barrier,
                        directory_for_barrier.join("one_active.jsonl"),
                    )
                    .unwrap();
                }),
            }],
            || read(&directory, "one_active.jsonl"),
        );
        assert_eq!(fired, 1);
        assert_eq!(
            result,
            CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateIdentityChanged)
        );

        active(
            &directory,
            "two_active.jsonl",
            &request("conversation", "two"),
        );
        let corrupt_directory = directory.clone();
        let (result, _, fired) = run_with_trace(
            None,
            vec![CortexUseReadBarrier {
                primitive: CortexUseReadPrimitive::FirstRowReread,
                ordinal: 1,
                callback: Box::new(move || {
                    fs::write(
                        corrupt_directory.join("two_active.jsonl"),
                        b"{\"name\":\"conversation\",\"use_id\":\"two\",\"changed\":true}\n",
                    )
                    .unwrap();
                }),
            }],
            || read(&directory, "two_active.jsonl"),
        );
        assert_eq!(fired, 1);
        assert_eq!(
            result,
            CortexUseCandidateRead::Refused(CortexUseRefusal::CandidateIdentityChanged)
        );
    }

    #[test]
    fn test_hook_faults_map_to_candidate_refusal_classes() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join(talent_directory_name("conversation"));
        fs::create_dir(&directory).unwrap();
        active(
            &directory,
            "one_active.jsonl",
            &request("conversation", "one"),
        );
        for (primitive, expected) in [
            (
                CortexUseReadPrimitive::InitialNameObserve,
                CortexUseRefusal::CandidateIo,
            ),
            (CortexUseReadPrimitive::Open, CortexUseRefusal::CandidateIo),
            (
                CortexUseReadPrimitive::FirstRowRead,
                CortexUseRefusal::CandidateIo,
            ),
            (
                CortexUseReadPrimitive::FinalNameObserve,
                CortexUseRefusal::CandidateIdentityChanged,
            ),
        ] {
            let (result, consumed, _) = run_with_trace(
                Some(CortexUseReadFault {
                    primitive,
                    ordinal: 1,
                }),
                Vec::new(),
                || read(&directory, "one_active.jsonl"),
            );
            assert!(consumed, "{primitive:?}");
            assert_eq!(
                result,
                CortexUseCandidateRead::Refused(expected),
                "{primitive:?}"
            );
        }
    }

    #[test]
    fn tail_growth_after_the_first_row_does_not_change_an_admitted_request() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        active(
            &directory,
            "one_active.jsonl",
            &request("conversation", "one"),
        );
        let path = directory.join("one_active.jsonl");
        let (result, _, fired) = run_with_trace(
            None,
            vec![CortexUseReadBarrier {
                primitive: CortexUseReadPrimitive::FirstRowReread,
                ordinal: 1,
                callback: Box::new(move || {
                    OpenOptions::new()
                        .append(true)
                        .open(path)
                        .unwrap()
                        .write_all(
                            b"{\"event\":\"complete-tail\"}\n{\"event\":\"tail-without-newline\"}",
                        )
                        .unwrap();
                }),
            }],
            || read(&directory, "one_active.jsonl"),
        );
        assert_eq!(fired, 1);
        assert_eq!(
            result,
            CortexUseCandidateRead::Accepted(CortexUseRequest {
                use_id: "one".into()
            })
        );
    }

    #[test]
    fn destination_is_no_follow_and_exactly_projected() {
        let temporary = tempdir().unwrap();
        let directory = temporary.path().join("conversation");
        fs::create_dir(&directory).unwrap();
        let request = CortexUseRequest {
            use_id: "one".into(),
        };
        assert_eq!(
            check_cortex_use_destination(&directory, &request),
            CortexUseDestinationCheck::Vacant
        );
        std::os::unix::fs::symlink("missing", directory.join("one.jsonl")).unwrap();
        assert_eq!(
            check_cortex_use_destination(&directory, &request),
            CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationOccupied)
        );
        fs::remove_file(directory.join("one.jsonl")).unwrap();
        let mut regular = fs::File::create(directory.join("one.jsonl")).unwrap();
        regular.write_all(b"completed").unwrap();
        assert_eq!(
            check_cortex_use_destination(&directory, &request),
            CortexUseDestinationCheck::Refused(CortexUseRefusal::DestinationOccupied)
        );
    }

    #[test]
    fn root_revalidation_requires_the_original_directory_identity() {
        let temporary = tempdir().unwrap();
        let root = temporary.path().join("talents");
        let replacement = temporary.path().join("replacement");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&replacement).unwrap();

        let identity = inspect_cortex_use_root(&root).unwrap();
        assert_eq!(revalidate_cortex_use_root(&root, &identity), Ok(()));

        fs::remove_dir(&root).unwrap();
        fs::rename(&replacement, &root).unwrap();
        assert_eq!(
            revalidate_cortex_use_root(&root, &identity),
            Err(CortexUseFatal::RootInspectionFailed)
        );
    }
}
