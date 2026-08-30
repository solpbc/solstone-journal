// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Tolerant readers with an explicit malformed-data posture.

use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::fd::AsFd;
use std::path::Path;

#[cfg(unix)]
use nix::errno::Errno;
use serde::de::DeserializeOwned;

use crate::errors::{MalformedDataError, ReadError};

/// Response to malformed non-empty JSON data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MalformedPolicy {
    /// Omit malformed data without emitting a log record.
    Skip,
    /// Omit malformed data and emit a warning.
    WarnAndSkip,
    /// Return a typed read error.
    Raise,
}

/// A JSONL value with its one-based source line number; `0` marks a caller-supplied default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlRecord<T> {
    pub value: T,
    pub line_number: usize,
}

/// Tolerant JSONL read results, including omitted malformed-record count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonlReadReport<T> {
    pub records: Vec<JsonlRecord<T>>,
    pub malformed_line_count: usize,
}

/// Ordered checkpoints for an identity-stable descriptor-bound byte read.
#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundReadPrimitive {
    /// The initial `fstatat` no-follow observation of the bound name.
    InitialNameObserve,
    /// The `openat` no-follow, nonblocking open of the observed name.
    Open,
    /// The `fstat` observation of the newly opened handle.
    OpenedHandleObserve,
    /// The `read_to_end` through the admitted handle.
    Read,
    /// The final `fstat` observation of the admitted handle.
    FinalHandleObserve,
    /// The final `fstatat` no-follow observation of the bound name.
    FinalNameObserve,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundReadTraceState {
    attempted: Vec<BoundReadPrimitive>,
    fault: Option<BoundReadFault>,
    fault_consumed: bool,
    barriers: Vec<BoundReadBarrier>,
    barriers_fired: usize,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundReadFault {
    primitive: BoundReadPrimitive,
    ordinal: usize,
    error: Errno,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
struct BoundReadBarrier {
    primitive: BoundReadPrimitive,
    ordinal: usize,
    callback: Box<dyn FnOnce()>,
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
thread_local! {
    static BOUND_READ_TRACE: std::cell::RefCell<Option<BoundReadTraceState>> = const {
        std::cell::RefCell::new(None)
    };
}

/// Run `op` with one injected errno at an ordinal bound-read checkpoint.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_read_fault<T>(
    primitive: BoundReadPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    BOUND_READ_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound read trace is already active"
        );
        *trace.borrow_mut() = Some(BoundReadTraceState {
            attempted: Vec::new(),
            fault: Some(BoundReadFault {
                primitive,
                ordinal,
                error: Errno::from_raw(raw_errno),
            }),
            fault_consumed: false,
            barriers: Vec::new(),
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_READ_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound read trace remains active")
    });
    (result, state.fault_consumed)
}

/// Run `op` with one injected errno and return its attempted checkpoint trace.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_read_fault_trace<T>(
    primitive: BoundReadPrimitive,
    ordinal: usize,
    raw_errno: i32,
    op: impl FnOnce() -> T,
) -> (T, Vec<BoundReadPrimitive>) {
    BOUND_READ_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound read trace is already active"
        );
        *trace.borrow_mut() = Some(BoundReadTraceState {
            attempted: Vec::new(),
            fault: Some(BoundReadFault {
                primitive,
                ordinal,
                error: Errno::from_raw(raw_errno),
            }),
            fault_consumed: false,
            barriers: Vec::new(),
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_READ_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound read trace remains active")
    });
    (result, state.attempted)
}

/// Run `op` with one deterministic bound-read barrier callback.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_bound_read_barrier<T>(
    primitive: BoundReadPrimitive,
    ordinal: usize,
    callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, bool) {
    BOUND_READ_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound read trace is already active"
        );
        *trace.borrow_mut() = Some(BoundReadTraceState {
            attempted: Vec::new(),
            fault: None,
            fault_consumed: false,
            barriers: vec![BoundReadBarrier {
                primitive,
                ordinal,
                callback: Box::new(callback),
            }],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_READ_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound read trace remains active")
    });
    (result, state.barriers_fired == 1)
}

/// Run `op` with two deterministic bound-read barrier callbacks.
#[cfg(all(unix, any(test, feature = "test-hooks")))]
pub fn run_with_two_bound_read_barriers<T>(
    first_primitive: BoundReadPrimitive,
    first_ordinal: usize,
    first_callback: impl FnOnce() + 'static,
    second_primitive: BoundReadPrimitive,
    second_ordinal: usize,
    second_callback: impl FnOnce() + 'static,
    op: impl FnOnce() -> T,
) -> (T, usize) {
    BOUND_READ_TRACE.with(|trace| {
        assert!(
            trace.borrow().is_none(),
            "bound read trace is already active"
        );
        *trace.borrow_mut() = Some(BoundReadTraceState {
            attempted: Vec::new(),
            fault: None,
            fault_consumed: false,
            barriers: vec![
                BoundReadBarrier {
                    primitive: first_primitive,
                    ordinal: first_ordinal,
                    callback: Box::new(first_callback),
                },
                BoundReadBarrier {
                    primitive: second_primitive,
                    ordinal: second_ordinal,
                    callback: Box::new(second_callback),
                },
            ],
            barriers_fired: 0,
        });
    });
    let result = op();
    let state = BOUND_READ_TRACE.with(|trace| {
        trace
            .borrow_mut()
            .take()
            .expect("bound read trace remains active")
    });
    (result, state.barriers_fired)
}

/// Read JSON, treating missing or empty files as `default` before malformed policy.
pub fn read_json<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    default: T,
    on_error: MalformedPolicy,
) -> Result<T, ReadError> {
    let path = path.as_ref();
    let contents = read_missing_as_empty(path)?;
    if contents.is_empty() {
        return Ok(default);
    }
    match serde_json::from_slice(&contents) {
        Ok(value) => Ok(value),
        Err(source) => malformed_or_default(path, None, source, default, on_error),
    }
}

/// Read JSONL, omitting blank lines and applying malformed policy per record.
pub fn read_jsonl<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    default: Vec<T>,
    on_error: MalformedPolicy,
) -> Result<Vec<T>, ReadError> {
    Ok(read_jsonl_with_report(path, default, on_error)?
        .records
        .into_iter()
        .map(|record| record.value)
        .collect())
}

/// Read JSONL with source line numbers and malformed-record accounting.
pub fn read_jsonl_with_report<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    default: Vec<T>,
    on_error: MalformedPolicy,
) -> Result<JsonlReadReport<T>, ReadError> {
    let path = path.as_ref();
    let contents = read_missing_as_empty(path)?;
    if contents.is_empty() {
        return Ok(JsonlReadReport {
            records: default
                .into_iter()
                .map(|value| JsonlRecord {
                    value,
                    line_number: 0,
                })
                .collect(),
            malformed_line_count: 0,
        });
    }
    let mut records = Vec::new();
    let mut malformed_line_count = 0;
    for (index, line) in contents.split(|byte| *byte == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(value) => records.push(JsonlRecord {
                value,
                line_number: index + 1,
            }),
            Err(source) => match on_error {
                MalformedPolicy::Raise => {
                    return Err(ReadError::Malformed(MalformedDataError {
                        path: path.to_path_buf(),
                        line: Some(index + 1),
                        source,
                    }));
                }
                MalformedPolicy::Skip => {
                    malformed_line_count += 1;
                }
                MalformedPolicy::WarnAndSkip => {
                    malformed_line_count += 1;
                    log::warn!(
                        "malformed JSONL record in {} at line {}: {source}",
                        path.display(),
                        index + 1
                    );
                }
            },
        }
    }
    Ok(JsonlReadReport {
        records,
        malformed_line_count,
    })
}

/// Read UTF-8 text, treating a missing path as `default`.
pub fn read_text(path: impl AsRef<Path>, default: String) -> Result<String, ReadError> {
    let path = path.as_ref();
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
        Err(source) => Err(io_error(path, source)),
    }
}

/// Read an identity-stable regular file beneath an already-bound parent.
///
/// `Ok(None)` is returned only when the initial no-follow name observation is
/// absent. This never opens a parent via `AT_FDCWD`; any later disappearance,
/// substitution, or non-regular entry is an error. The checks do not guarantee
/// stable bytes against an in-place rewrite of the same inode, or protect
/// against privileged-device substitution beyond the initial type rejection.
#[cfg(unix)]
pub fn read_bytes_bound(
    directory: &impl AsFd,
    name: &std::ffi::OsStr,
) -> Result<Option<Vec<u8>>, ReadError> {
    use nix::fcntl::{AtFlags, OFlag, openat};
    use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};

    let path = Path::new(name);
    let checkpoint_error = |primitive| {
        checkpoint(primitive)
            .map_err(|source| io_error(path, io::Error::from_raw_os_error(source as i32)))
    };
    let require_regular = |status: &FileStat| {
        if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT == SFlag::S_IFREG {
            Ok(())
        } else {
            Err(io_error(
                path,
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "bound name is not a regular file",
                ),
            ))
        }
    };
    let same_identity = |status: &FileStat, expected| (status.st_dev, status.st_ino) == expected;

    checkpoint_error(BoundReadPrimitive::InitialNameObserve)?;
    let initial = match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Err(Errno::ENOENT) => return Ok(None),
        Err(source) => {
            return Err(io_error(path, io::Error::from_raw_os_error(source as i32)));
        }
        Ok(status) => status,
    };
    require_regular(&initial)?;
    let expected = (initial.st_dev, initial.st_ino);

    checkpoint_error(BoundReadPrimitive::Open)?;
    let fd = match openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(source) => {
            return Err(io_error(path, io::Error::from_raw_os_error(source as i32)));
        }
    };
    checkpoint_error(BoundReadPrimitive::OpenedHandleObserve)?;
    let opened =
        fstat(&fd).map_err(|source| io_error(path, io::Error::from_raw_os_error(source as i32)))?;
    require_regular(&opened)?;
    if !same_identity(&opened, expected) {
        return Err(bound_read_identity_changed(path));
    }
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::new();
    checkpoint_error(BoundReadPrimitive::Read)?;
    file.read_to_end(&mut bytes)
        .map_err(|source| io_error(path, source))?;

    checkpoint_error(BoundReadPrimitive::FinalHandleObserve)?;
    let final_handle = fstat(&file)
        .map_err(|source| io_error(path, io::Error::from_raw_os_error(source as i32)))?;
    require_regular(&final_handle)?;
    if !same_identity(&final_handle, expected) {
        return Err(bound_read_identity_changed(path));
    }

    checkpoint_error(BoundReadPrimitive::FinalNameObserve)?;
    let final_name = fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW)
        .map_err(|source| io_error(path, io::Error::from_raw_os_error(source as i32)))?;
    require_regular(&final_name)?;
    if !same_identity(&final_name, expected) {
        return Err(bound_read_identity_changed(path));
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn bound_read_identity_changed(path: &Path) -> ReadError {
    io_error(
        path,
        io::Error::new(
            io::ErrorKind::InvalidData,
            "bound name changed while reading",
        ),
    )
}

#[cfg(all(unix, any(test, feature = "test-hooks")))]
fn checkpoint(primitive: BoundReadPrimitive) -> Result<(), Errno> {
    let (fault, barrier) = BOUND_READ_TRACE.with(|trace| {
        let mut trace = trace.borrow_mut();
        let Some(state) = trace.as_mut() else {
            return (None, None);
        };
        state.attempted.push(primitive);
        let ordinal = state
            .attempted
            .iter()
            .filter(|candidate| **candidate == primitive)
            .count();
        let inject = state
            .fault
            .as_ref()
            .is_some_and(|fault| fault.primitive == primitive && fault.ordinal == ordinal);
        if inject {
            let fault = state.fault.take().expect("matching fault is present");
            state.fault_consumed = true;
            (Some(fault.error), None)
        } else {
            let barrier = state
                .barriers
                .iter()
                .position(|barrier| barrier.primitive == primitive && barrier.ordinal == ordinal);
            if let Some(index) = barrier {
                let barrier = state.barriers.remove(index);
                state.barriers_fired += 1;
                (None, Some(barrier.callback))
            } else {
                (None, None)
            }
        }
    });
    if let Some(error) = fault {
        return Err(error);
    }
    if let Some(callback) = barrier {
        callback();
    }
    Ok(())
}

#[cfg(all(unix, not(any(test, feature = "test-hooks"))))]
fn checkpoint(_primitive: BoundReadPrimitive) -> Result<(), Errno> {
    Ok(())
}

/// Read raw bytes, treating a missing path as `default`.
pub fn read_bytes(path: impl AsRef<Path>, default: Vec<u8>) -> Result<Vec<u8>, ReadError> {
    let path = path.as_ref();
    match fs::read(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
        Err(source) => Err(io_error(path, source)),
    }
}

fn read_missing_as_empty(path: &Path) -> Result<Vec<u8>, ReadError> {
    match fs::read(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(io_error(path, source)),
    }
}

fn malformed_or_default<T>(
    path: &Path,
    line: Option<usize>,
    source: serde_json::Error,
    default: T,
    on_error: MalformedPolicy,
) -> Result<T, ReadError> {
    match on_error {
        MalformedPolicy::Raise => Err(ReadError::Malformed(MalformedDataError {
            path: path.to_path_buf(),
            line,
            source,
        })),
        MalformedPolicy::Skip => Ok(default),
        MalformedPolicy::WarnAndSkip => {
            log::warn!("malformed JSON data in {}: {source}", path.display());
            Ok(default)
        }
    }
}

fn io_error(path: &Path, source: io::Error) -> ReadError {
    ReadError::Io {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::sync::{Mutex, Once, OnceLock};

    use log::{Level, LevelFilter, Log, Metadata, Record};
    use nix::errno::Errno;
    use nix::fcntl::{AT_FDCWD, OFlag, openat};
    use nix::sys::stat::{Mode, SFlag, makedev, mknod};
    use nix::unistd::mkfifo;

    use super::*;
    use crate::test_support::TempDir;

    struct TestLogger;

    static LOGGER: TestLogger = TestLogger;
    static LOGGER_INIT: Once = Once::new();
    static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

    impl Log for TestLogger {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.level() <= Level::Warn
        }

        fn log(&self, record: &Record<'_>) {
            if self.enabled(record.metadata()) {
                LOGS.get_or_init(|| Mutex::new(Vec::new()))
                    .lock()
                    .unwrap()
                    .push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    fn install_logger() {
        LOGGER_INIT.call_once(|| {
            log::set_logger(&LOGGER).unwrap();
            log::set_max_level(LevelFilter::Warn);
        });
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap()
            .clear();
    }

    #[test]
    fn missing_and_empty_json_return_defaults_before_policy() {
        let temporary = TempDir::new();
        let missing = temporary.path().join("missing.json");
        let empty = temporary.path().join("empty.json");
        fs::write(&empty, []).unwrap();

        assert_eq!(
            read_json::<Vec<u8>>(&missing, vec![1], MalformedPolicy::Raise).unwrap(),
            vec![1]
        );
        assert_eq!(
            read_json::<Vec<u8>>(&empty, vec![2], MalformedPolicy::Raise).unwrap(),
            vec![2]
        );
        assert_eq!(
            read_jsonl::<u8>(&missing, vec![3], MalformedPolicy::Raise).unwrap(),
            vec![3]
        );
        assert_eq!(
            read_jsonl::<u8>(&empty, vec![4], MalformedPolicy::Raise).unwrap(),
            vec![4]
        );
    }

    #[test]
    fn read_bytes_preserves_binary_contents_and_uses_missing_default() {
        let temporary = TempDir::new();
        let path = temporary.path().join("data.bin");
        fs::write(&path, [0, 255, 1]).unwrap();

        assert_eq!(read_bytes(&path, Vec::new()).unwrap(), vec![0, 255, 1]);
        assert_eq!(
            read_bytes(temporary.path().join("missing.bin"), vec![9]).unwrap(),
            vec![9]
        );
    }

    #[test]
    fn read_bytes_bound_returns_none_when_missing() {
        use nix::fcntl::{AT_FDCWD, OFlag, openat};
        use nix::sys::stat::Mode;

        let temporary = TempDir::new();
        fs::write(temporary.path().join("data.bin"), [0, 255, 1]).unwrap();
        let directory = openat(
            AT_FDCWD,
            temporary.path(),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .unwrap();
        assert_eq!(
            read_bytes_bound(&directory, std::ffi::OsStr::new("data.bin")).unwrap(),
            Some(vec![0, 255, 1])
        );
        assert_eq!(
            read_bytes_bound(&directory, std::ffi::OsStr::new("missing.bin")).unwrap(),
            None
        );
    }

    #[test]
    fn malformed_jsonl_policy_controls_omission_and_line_number() {
        install_logger();
        let temporary = TempDir::new();
        let path = temporary.path().join("records.jsonl");
        fs::write(&path, b"1\nnot-json\n2\n").unwrap();

        let error = read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::Raise).unwrap_err();
        match error {
            ReadError::Malformed(error) => assert_eq!(error.line, Some(2)),
            ReadError::Io { .. } => panic!("expected malformed data"),
        }
        assert_eq!(
            read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::Skip).unwrap(),
            vec![1, 2]
        );
        let report =
            read_jsonl_with_report::<u8>(&path, Vec::new(), MalformedPolicy::Skip).unwrap();
        assert_eq!(report.malformed_line_count, 1);
        assert_eq!(
            report.records,
            vec![
                JsonlRecord {
                    value: 1,
                    line_number: 1,
                },
                JsonlRecord {
                    value: 2,
                    line_number: 3,
                },
            ]
        );
        assert_eq!(
            read_jsonl::<u8>(&path, Vec::new(), MalformedPolicy::WarnAndSkip).unwrap(),
            vec![1, 2]
        );
        assert!(
            LOGS.get()
                .unwrap()
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.contains("line 2"))
        );
    }

    fn open_bound_directory(path: &std::path::Path) -> std::os::fd::OwnedFd {
        openat(
            AT_FDCWD,
            path,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::empty(),
        )
        .expect("open bound test directory")
    }

    fn read_bound(directory: &std::os::fd::OwnedFd) -> Result<Option<Vec<u8>>, ReadError> {
        read_bytes_bound(directory, OsStr::new("record"))
    }

    fn write_record(root: &std::path::Path, bytes: &[u8]) {
        fs::write(root.join("record"), bytes).expect("test record writes");
    }

    #[test]
    fn read_bytes_bound_rejects_initial_fifo_without_opening() {
        let fifo = TempDir::new();
        mkfifo(&fifo.path().join("record"), Mode::from_bits_truncate(0o600)).expect("FIFO creates");
        let directory = open_bound_directory(fifo.path());
        let (result, open_attempted) =
            run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
                read_bound(&directory)
            });
        assert!(result.is_err());
        assert!(!open_attempted, "initial FIFO must not reach open");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_bytes_bound_rejects_initial_device_when_the_fixture_is_permitted() {
        let temporary = TempDir::new();
        let device = temporary.path().join("record");
        match mknod(
            &device,
            SFlag::S_IFCHR,
            Mode::from_bits_truncate(0o600),
            makedev(1, 3),
        ) {
            Ok(()) => {}
            Err(Errno::EPERM | Errno::EACCES) => return,
            Err(error) => panic!("device fixture creates: {error}"),
        }
        let directory = open_bound_directory(temporary.path());
        let (result, open_attempted) =
            run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
                read_bound(&directory)
            });
        assert!(result.is_err());
        assert!(!open_attempted, "initial device must not reach open");
    }

    #[test]
    fn read_bytes_bound_rejects_fifo_and_removal_before_open() {
        let fifo = TempDir::new();
        write_record(fifo.path(), b"original");
        let fifo_path = fifo.path().join("record");
        let directory = open_bound_directory(fifo.path());
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Open,
            1,
            move || {
                fs::remove_file(&fifo_path).expect("record removes");
                mkfifo(&fifo_path, Mode::from_bits_truncate(0o600)).expect("FIFO creates");
            },
            || read_bound(&directory),
        );
        assert!(fired);
        assert!(result.is_err());

        let removed = TempDir::new();
        write_record(removed.path(), b"original");
        let removed_path = removed.path().join("record");
        let directory = open_bound_directory(removed.path());
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Open,
            1,
            move || fs::remove_file(&removed_path).expect("record removes"),
            || read_bound(&directory),
        );
        assert!(fired);
        assert!(result.is_err());
    }

    #[test]
    fn read_bytes_bound_rejects_different_inode_replacement_before_open() {
        let temporary = TempDir::new();
        write_record(temporary.path(), b"original");
        let replacement = temporary.path().join("replacement");
        fs::write(&replacement, b"replacement").expect("replacement writes");
        let target = temporary.path().join("record");
        let directory = open_bound_directory(temporary.path());
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Open,
            1,
            move || fs::rename(&replacement, &target).expect("replacement installs"),
            || read_bound(&directory),
        );
        assert!(fired);
        assert!(result.is_err());
    }

    #[test]
    fn read_bytes_bound_rejects_name_removal_and_replacement_after_open() {
        let removed = TempDir::new();
        write_record(removed.path(), b"original");
        let removed_path = removed.path().join("record");
        let directory = open_bound_directory(removed.path());
        let (result, fired) = run_with_bound_read_barrier(
            BoundReadPrimitive::Read,
            1,
            move || fs::remove_file(&removed_path).expect("record removes"),
            || read_bound(&directory),
        );
        assert!(fired);
        assert!(result.is_err());

        let replaced = TempDir::new();
        write_record(replaced.path(), b"original");
        let target = replaced.path().join("record");
        let aside = replaced.path().join("record.aside");
        let replacement = replaced.path().join("replacement");
        fs::write(&replacement, b"replacement").expect("replacement writes");
        let expected_replacement = replacement.clone();
        let directory = open_bound_directory(replaced.path());
        let (result, barriers_fired) = run_with_two_bound_read_barriers(
            BoundReadPrimitive::Read,
            1,
            move || {
                fs::rename(&target, &aside).expect("original moves aside");
                fs::rename(&replacement, &target).expect("replacement installs");
            },
            BoundReadPrimitive::FinalNameObserve,
            1,
            move || assert!(!expected_replacement.exists(), "replacement was installed"),
            || read_bound(&directory),
        );
        assert_eq!(barriers_fired, 2);
        assert!(result.is_err());
    }

    #[test]
    fn read_bytes_bound_fault_traces_match_protocol_prefixes() {
        let primitives = [
            BoundReadPrimitive::InitialNameObserve,
            BoundReadPrimitive::Open,
            BoundReadPrimitive::OpenedHandleObserve,
            BoundReadPrimitive::Read,
            BoundReadPrimitive::FinalHandleObserve,
            BoundReadPrimitive::FinalNameObserve,
        ];
        for raw_errno in [Errno::EIO as i32, Errno::EACCES as i32] {
            for (index, primitive) in primitives.iter().copied().enumerate() {
                let temporary = TempDir::new();
                write_record(temporary.path(), b"original");
                let directory = open_bound_directory(temporary.path());
                let (result, attempted) =
                    run_with_bound_read_fault_trace(primitive, 1, raw_errno, || {
                        read_bound(&directory)
                    });
                assert!(
                    result.is_err(),
                    "{primitive:?} fault unexpectedly read bytes"
                );
                assert_eq!(
                    attempted,
                    primitives[..=index].to_vec(),
                    "{primitive:?} trace did not end at its injected checkpoint"
                );
            }
        }
    }

    #[test]
    fn read_bytes_bound_preserves_initial_absence_and_unchanged_bytes() {
        let absent = TempDir::new();
        let directory = open_bound_directory(absent.path());
        assert_eq!(read_bound(&directory).unwrap(), None);

        let present = TempDir::new();
        write_record(present.path(), &[0, 255, 1]);
        let directory = open_bound_directory(present.path());
        assert_eq!(read_bound(&directory).unwrap(), Some(vec![0, 255, 1]));
    }
}
