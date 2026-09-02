// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only operational-log writer and stdio duplicates.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};

use crate::lease::SelfLease;

/// Closed failure while duplicating an oplog writer for stdio capture.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct OplogWriterError {
    class: OplogWriterClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OplogWriterClass {
    CloneIo,
}

impl OplogWriterClass {
    const fn token(self) -> &'static str {
        match self {
            Self::CloneIo => "oplog_writer_clone_io",
        }
    }
}

impl OplogWriterError {
    const fn clone_io() -> Self {
        Self {
            class: OplogWriterClass::CloneIo,
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
    #[allow(dead_code)]
    lease: SelfLease,
    leaf: String,
}

/// Stdio-oriented duplicate of an [`OplogWriter`]. Does not implement `Seek`.
pub struct OplogStdioHandle {
    file: File,
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
    pub(super) fn new(file: File, lease: SelfLease, leaf: String) -> Self {
        Self { file, lease, leaf }
    }

    /// Canonical leaf name of the published file.
    pub fn leaf_name(&self) -> &str {
        &self.leaf
    }

    /// Duplicate this writer for in-process stdout/stderr capture.
    ///
    /// The duplicate shares the open file description (Unix) or access mask
    /// (Windows). The advisory lock remains until the last such descriptor
    /// closes.
    pub fn try_clone_for_stdio(&self) -> Result<OplogStdioHandle, OplogWriterError> {
        let file = self
            .file
            .try_clone()
            .map_err(|_| OplogWriterError::clone_io())?;
        Ok(OplogStdioHandle { file })
    }

    /// Duplicate this writer as a child-process stdio stream.
    ///
    /// No raw descriptor is exposed. The lock stays held on the shared open
    /// file description (Unix) or inheritable handle (Windows) until the last
    /// remaining duplicate, including the child's, is closed.
    pub fn duplicate_locked_stdio(&self) -> Result<std::process::Stdio, OplogWriterError> {
        #[cfg(unix)]
        {
            let file = self
                .file
                .try_clone()
                .map_err(|_| OplogWriterError::clone_io())?;
            Ok(std::process::Stdio::from(file))
        }
        #[cfg(windows)]
        {
            duplicate_inheritable_stdio(&self.file)
        }
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

impl Write for OplogStdioHandle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(windows)]
fn duplicate_inheritable_stdio(file: &File) -> Result<std::process::Stdio, OplogWriterError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, INVALID_HANDLE_VALUE,
    };
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
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if result == 0 || duplicated == INVALID_HANDLE_VALUE {
        return Err(OplogWriterError::clone_io());
    }
    // SAFETY: `duplicated` is an owned inheritable handle returned by DuplicateHandle.
    #[allow(unsafe_code)]
    let file = File::from(unsafe { OwnedHandle::from_raw_handle(duplicated) });
    Ok(std::process::Stdio::from(file))
}

#[cfg(test)]
mod no_raw_io {
    use std::marker::PhantomData;

    use super::{OplogStdioHandle, OplogWriter};

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
    fn oplog_writer_and_stdio_handle_do_not_implement_raw_io() {
        PhantomData::<OplogWriter>.token();
        PhantomData::<OplogStdioHandle>.token();
    }
}
