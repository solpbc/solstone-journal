// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only operational-log writer and single-use child capture.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{self, Seek, Write};
use std::path::Path;
use std::process::{Child, Command, CommandArgs, CommandEnvs};

#[cfg(unix)]
use crate::lease::SelfLease;

use super::reason::OplogFileIdentity;

#[cfg(unix)]
pub(super) type StagedLease = SelfLease;
#[cfg(windows)]
pub(super) type StagedLease = ();

/// Closed failure while duplicating an oplog writer for capture.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogWriterError {
    class: OplogWriterClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogWriterClass {
    CloneIo,
    CaptureStdout,
    CaptureStderr,
}

impl OplogWriterClass {
    const fn token(self) -> &'static str {
        match self {
            Self::CloneIo => "oplog_writer_clone_io",
            Self::CaptureStdout => "oplog_writer_capture_stdout",
            Self::CaptureStderr => "oplog_writer_capture_stderr",
        }
    }
}

impl OplogWriterError {
    const fn clone_io() -> Self {
        Self {
            class: OplogWriterClass::CloneIo,
        }
    }

    const fn capture_stdout() -> Self {
        Self {
            class: OplogWriterClass::CaptureStdout,
        }
    }

    const fn capture_stderr() -> Self {
        Self {
            class: OplogWriterClass::CaptureStderr,
        }
    }

    fn token(self) -> &'static str {
        self.class.token()
    }
}

impl fmt::Display for OplogWriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.token())
    }
}

impl fmt::Debug for OplogWriterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl Error for OplogWriterError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// Append-only writer bound to one published oplog inode.
pub struct OplogWriter {
    file: File,
    /// Held so the locked descriptor stays open for the writer's lifetime.
    #[cfg(unix)]
    #[allow(dead_code)]
    lease: SelfLease,
    identity: OplogFileIdentity,
    leaf: String,
}

/// In-process duplicate of an [`OplogWriter`]. Does not implement `Seek`.
pub struct OplogWriteHandle {
    file: File,
}

/// Single-use child launcher that captures stdout and stderr onto one published oplog.
///
/// Not `Clone` or `Copy`: `spawn` consumes the launcher, so two live siblings of one
/// configured capture cannot exist.
pub struct OplogChildCapture {
    command: Command,
}

impl fmt::Debug for OplogWriter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OplogWriter")
            .field("leaf", &self.leaf)
            .finish_non_exhaustive()
    }
}

impl OplogWriter {
    pub(super) fn new(
        file: File,
        lease: StagedLease,
        identity: OplogFileIdentity,
        leaf: String,
    ) -> Self {
        #[cfg(windows)]
        let _ = lease;
        Self {
            file,
            #[cfg(unix)]
            lease,
            identity,
            leaf,
        }
    }

    /// Canonical leaf name of the published file.
    pub fn leaf_name(&self) -> &str {
        &self.leaf
    }

    /// Identity of the published inode, captured at create.
    pub fn identity(&self) -> OplogFileIdentity {
        self.identity
    }

    /// Borrow the Unix descriptor for this published oplog while its lease is held.
    #[cfg(unix)]
    pub fn as_raw_fd(&self) -> std::os::fd::RawFd {
        use std::os::fd::AsRawFd as _;

        self.file.as_raw_fd()
    }

    /// Duplicate this writer for in-process stdout/stderr capture.
    ///
    /// The duplicate shares the open file description (Unix) or access mask
    /// (Windows). The advisory lock remains until the last such descriptor
    /// closes.
    pub fn try_clone_for_write(&self) -> Result<OplogWriteHandle, OplogWriterError> {
        let file = self
            .file
            .try_clone()
            .map_err(|_| OplogWriterError::clone_io())?;
        Ok(OplogWriteHandle { file })
    }

    /// Append `bytes` to the published oplog.
    ///
    /// Publication is the append. The checkpoint runs immediately before
    /// `write_all`, and a refusal writes no bytes. A failed `write_all` restores
    /// the previous length when that truncate succeeds and returns the write
    /// error. A failed truncate after a partial append is returned as-is: the
    /// published bytes are not described as unchanged.
    pub fn append_bytes_checked(
        &mut self,
        bytes: &[u8],
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> io::Result<()> {
        checkpoint().map_err(|detail| io::Error::new(io::ErrorKind::Interrupted, detail))?;
        let start = self.file.stream_position()?;
        if let Err(error) = write_published_bytes(&mut self.file, bytes) {
            restore_published_length(&mut self.file, start)?;
            return Err(error);
        }
        self.file.flush()
    }

    /// Configure `command` so both stdout and stderr append to this writer.
    ///
    /// Stdout is duplicated first. A stdout failure returns immediately. A stderr
    /// failure drops the stdout duplicate before returning. The original writer
    /// is unchanged in both cases.
    pub fn prepare_child_capture(
        &self,
        mut command: Command,
    ) -> Result<OplogChildCapture, OplogWriterError> {
        if force_capture_stdout_fail() {
            return Err(OplogWriterError::capture_stdout());
        }
        let stdout = duplicate_for_capture(&self.file).ok_or(OplogWriterError::capture_stdout())?;
        if force_capture_stderr_fail() {
            return Err(OplogWriterError::capture_stderr());
        }
        let stderr = duplicate_for_capture(&self.file).ok_or(OplogWriterError::capture_stderr())?;
        command.stdout(std::process::Stdio::from(stdout));
        command.stderr(std::process::Stdio::from(stderr));
        Ok(OplogChildCapture { command })
    }
}

#[cfg(unix)]
impl std::os::fd::AsFd for OplogWriter {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.file.as_fd()
    }
}

impl OplogChildCapture {
    pub fn get_program(&self) -> &OsStr {
        self.command.get_program()
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        self.command.get_args()
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.command.get_envs()
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.command.get_current_dir()
    }

    pub fn spawn(mut self) -> io::Result<Child> {
        self.command.spawn()
    }
}

impl Write for OplogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Write for OplogWriteHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

fn write_published_bytes(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    if FORCE_PARTIAL_APPEND.with(std::cell::Cell::get) {
        if !bytes.is_empty() {
            file.write_all(&bytes[..1])?;
        }
        return Err(io::Error::new(io::ErrorKind::WriteZero, "partial append"));
    }
    file.write_all(bytes)
}

fn restore_published_length(file: &mut File, start: u64) -> io::Result<()> {
    #[cfg(any(test, feature = "test-hooks"))]
    if FORCE_TRUNCATE_FAULT.with(std::cell::Cell::get) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "truncate failed",
        ));
    }
    file.set_len(start)
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::fd::AsRawFd;

    use crate::JournalRoot;
    use crate::operational_log::{OplogFormat, create_oplog};

    #[test]
    fn raw_fd_borrows_the_live_published_writer_descriptor() {
        let journal = tempfile::tempdir().unwrap();
        let writer = create_oplog(
            JournalRoot::open(journal.path()).unwrap(),
            "writer",
            "raw-fd",
            OplogFormat::Log,
        )
        .unwrap();
        assert_eq!(writer.as_raw_fd(), writer.file.as_raw_fd());
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod outcome_fence {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use crate::JournalRoot;
    use crate::operational_log::{OplogFormat, create_oplog};

    fn find_leaf(root: &std::path::Path, leaf: &str) -> std::path::PathBuf {
        let mut stack = vec![root.to_path_buf()];
        let mut found = None;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.file_name().and_then(|name| name.to_str()) == Some(leaf) {
                    found = Some(path);
                }
            }
        }
        found.expect("published oplog")
    }

    #[test]
    fn heartbeat_outcome_append_cancelled_while_blocked_does_not_publish() {
        let journal = tempfile::tempdir().unwrap();
        let mut writer = create_oplog(
            JournalRoot::open(journal.path()).unwrap(),
            "heartbeat",
            "pass",
            OplogFormat::Log,
        )
        .unwrap();
        writer
            .append_bytes_checked(b"{\"event\":\"pass\"}\n", &|| Ok(()))
            .unwrap();
        let path = find_leaf(journal.path(), writer.leaf_name());
        let before = std::fs::read(&path).unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let worker = thread::spawn(move || {
            writer.append_bytes_checked(b"{\"event\":\"pass.outcome\"}\n", &|| {
                entered_flag.store(true, Ordering::Release);
                while !release_flag.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                Err("stopped".into())
            })
        });
        for _ in 0..1_000_000 {
            if entered.load(Ordering::Acquire) {
                break;
            }
            thread::yield_now();
        }
        assert!(
            entered.load(Ordering::Acquire),
            "outcome append checkpoint never started"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        release.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let published = String::from_utf8_lossy(&before);
        assert!(!published.contains("pass.outcome"));
    }

    #[test]
    fn heartbeat_owner_appends_the_outcome() {
        let journal = tempfile::tempdir().unwrap();
        let mut writer = create_oplog(
            JournalRoot::open(journal.path()).unwrap(),
            "heartbeat",
            "pass",
            OplogFormat::Log,
        )
        .unwrap();
        writer
            .append_bytes_checked(b"{\"event\":\"pass.outcome\"}\n", &|| Ok(()))
            .unwrap();
        let path = find_leaf(journal.path(), writer.leaf_name());
        let published = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(published.contains("pass.outcome"));
    }

    #[test]
    fn failed_truncation_after_a_partial_append_is_an_error() {
        let journal = tempfile::tempdir().unwrap();
        let mut writer = create_oplog(
            JournalRoot::open(journal.path()).unwrap(),
            "heartbeat",
            "pass",
            OplogFormat::Log,
        )
        .unwrap();
        writer
            .append_bytes_checked(b"{\"event\":\"pass\"}\n", &|| Ok(()))
            .unwrap();
        let path = find_leaf(journal.path(), writer.leaf_name());
        let before = std::fs::read(&path).unwrap();
        let error = super::run_with_partial_append_truncate_fault(|| {
            writer.append_bytes_checked(b"NEW-OUTCOME\n", &|| Ok(()))
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        let after = std::fs::read(&path).unwrap();
        assert_ne!(after, before);
        assert!(after.starts_with(&before));
        assert_eq!(*after.last().unwrap(), b'N');
    }

    #[test]
    fn heartbeat_root_replacement_publishes_on_neither_root() {
        let parent = tempfile::tempdir().unwrap();
        let journal = parent.path().join("journal");
        std::fs::create_dir(&journal).unwrap();
        let bound = JournalRoot::open(&journal).unwrap();
        let mut writer = create_oplog(
            JournalRoot::open(&journal).unwrap(),
            "heartbeat",
            "pass",
            OplogFormat::Log,
        )
        .unwrap();
        writer
            .append_bytes_checked(b"{\"event\":\"pass\"}\n", &|| Ok(()))
            .unwrap();
        let leaf = writer.leaf_name().to_owned();
        let path = find_leaf(&journal, &leaf);
        let before = std::fs::read(&path).unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let entered_flag = Arc::clone(&entered);
        let release_flag = Arc::clone(&release);
        let worker = thread::spawn(move || {
            writer.append_bytes_checked(b"{\"event\":\"pass.outcome\"}\n", &|| {
                entered_flag.store(true, Ordering::Release);
                while !release_flag.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                bound
                    .revalidate_canonical_binding()
                    .map_err(|error| error.to_string())
            })
        });
        for _ in 0..1_000_000 {
            if entered.load(Ordering::Acquire) {
                break;
            }
            thread::yield_now();
        }
        assert!(entered.load(Ordering::Acquire));
        let parked = parent.path().join("parked");
        if let Err(error) = std::fs::rename(&journal, &parked) {
            release.store(true, Ordering::Release);
            let result = worker.join().unwrap();
            assert!(
                cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32)),
                "{error}"
            );
            result.unwrap();
            assert!(!parked.exists());
            let mut expected = before;
            expected.extend_from_slice(b"{\"event\":\"pass.outcome\"}\n");
            assert_eq!(std::fs::read(&path).unwrap(), expected);
            std::fs::rename(&journal, &parked).expect("closed writer releases the namespace");
            return;
        }
        std::fs::create_dir(&journal).unwrap();
        release.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        let parked_leaf = find_leaf(&parked, &leaf);
        assert_eq!(std::fs::read(&parked_leaf).unwrap(), before);
        assert!(std::fs::read_dir(&journal).unwrap().next().is_none());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn heartbeat_deadline_during_append_publishes_nothing() {
        use std::time::{Duration, Instant};

        let journal = tempfile::tempdir().unwrap();
        let mut writer = create_oplog(
            JournalRoot::open(journal.path()).unwrap(),
            "heartbeat",
            "pass",
            OplogFormat::Log,
        )
        .unwrap();
        writer
            .append_bytes_checked(b"{\"event\":\"pass\"}\n", &|| Ok(()))
            .unwrap();
        let path = find_leaf(journal.path(), writer.leaf_name());
        let before = std::fs::read(&path).unwrap();
        let deadline = Instant::now() + Duration::from_millis(40);
        let error = writer
            .append_bytes_checked(b"{\"event\":\"pass.outcome\"}\n", &|| {
                while Instant::now() < deadline {
                    thread::yield_now();
                }
                if Instant::now() >= deadline {
                    Err("deadline".to_owned())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FORCE_CAPTURE_STDOUT_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_CAPTURE_STDERR_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_PARTIAL_APPEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_TRUNCATE_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `operation` so one append writes its first byte and then fails to truncate.
#[cfg(all(test, feature = "full-tests"))]
fn run_with_partial_append_truncate_fault<T>(operation: impl FnOnce() -> T) -> T {
    FORCE_PARTIAL_APPEND.with(|cell| cell.set(true));
    FORCE_TRUNCATE_FAULT.with(|cell| cell.set(true));
    let result = operation();
    FORCE_PARTIAL_APPEND.with(|cell| cell.set(false));
    FORCE_TRUNCATE_FAULT.with(|cell| cell.set(false));
    result
}

/// Force the next [`OplogWriter::prepare_child_capture`] to fail duplicating stdout.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_capture_stdout_fault<T>(operation: impl FnOnce() -> T) -> T {
    FORCE_CAPTURE_STDOUT_FAULT.with(|cell| cell.set(true));
    let result = operation();
    FORCE_CAPTURE_STDOUT_FAULT.with(|cell| cell.set(false));
    result
}

/// Force the next [`OplogWriter::prepare_child_capture`] to fail duplicating stderr.
#[cfg(any(test, feature = "test-hooks"))]
pub fn run_with_oplog_capture_stderr_fault<T>(operation: impl FnOnce() -> T) -> T {
    FORCE_CAPTURE_STDERR_FAULT.with(|cell| cell.set(true));
    let result = operation();
    FORCE_CAPTURE_STDERR_FAULT.with(|cell| cell.set(false));
    result
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_capture_stdout_fail() -> bool {
    FORCE_CAPTURE_STDOUT_FAULT.with(std::cell::Cell::get)
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_capture_stdout_fail() -> bool {
    false
}

#[cfg(any(test, feature = "test-hooks"))]
fn force_capture_stderr_fail() -> bool {
    FORCE_CAPTURE_STDERR_FAULT.with(std::cell::Cell::get)
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn force_capture_stderr_fail() -> bool {
    false
}

fn duplicate_for_capture(file: &File) -> Option<File> {
    #[cfg(unix)]
    {
        file.try_clone().ok()
    }
    #[cfg(windows)]
    {
        duplicate_inheritable_file(file)
    }
}

#[cfg(windows)]
fn duplicate_inheritable_file(file: &File) -> Option<File> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{DuplicateHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{FILE_APPEND_DATA, SYNCHRONIZE};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut duplicated = INVALID_HANDLE_VALUE;
    // SAFETY: source handle is a live `File`; output pointer is a local HANDLE.
    #[allow(unsafe_code)]
    let result = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            file.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicated,
            FILE_APPEND_DATA | SYNCHRONIZE,
            1,
            0,
        )
    };
    if result == 0 || duplicated == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: `duplicated` is an owned inheritable handle returned by DuplicateHandle.
    #[allow(unsafe_code)]
    Some(File::from(unsafe {
        OwnedHandle::from_raw_handle(duplicated)
    }))
}

#[cfg(test)]
mod no_raw_io {
    use std::marker::PhantomData;

    use super::{OplogChildCapture, OplogWriteHandle, OplogWriter};

    trait NoRawIo {
        fn token(&self) {}
    }

    impl<T> NoRawIo for PhantomData<T> {}

    #[cfg(unix)]
    #[allow(dead_code)]
    trait HasRawIo {
        fn token(&self) {}
    }

    #[cfg(unix)]
    impl<T: std::os::fd::AsRawFd> HasRawIo for PhantomData<T> {}

    #[cfg(windows)]
    #[allow(dead_code)]
    trait HasRawIo {
        fn token(&self) {}
    }

    #[cfg(windows)]
    impl<T: std::os::windows::io::AsRawHandle> HasRawIo for PhantomData<T> {}

    #[test]
    fn oplog_writer_handles_do_not_implement_raw_io() {
        PhantomData::<OplogWriter>.token();
        PhantomData::<OplogWriteHandle>.token();
        PhantomData::<OplogChildCapture>.token();
    }
}
