// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only operational-log writer and single-use child capture.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
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

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static FORCE_CAPTURE_STDOUT_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FORCE_CAPTURE_STDERR_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
