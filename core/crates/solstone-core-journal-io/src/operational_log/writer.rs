// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Append-only operational-log writer and stdio duplicates.

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::{self, Write};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;

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
    lease: Arc<SelfLease>,
    leaf: String,
}

/// Stdio-oriented duplicate of an [`OplogWriter`]. Does not implement `Seek`.
pub struct OplogStdioHandle {
    file: File,
    #[allow(dead_code)]
    lease: Arc<SelfLease>,
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
        Self {
            file,
            lease: Arc::new(lease),
            leaf,
        }
    }

    /// Canonical leaf name of the published file.
    pub fn leaf_name(&self) -> &str {
        &self.leaf
    }

    /// Duplicate this writer for stdout/stderr capture.
    ///
    /// The duplicate shares the open file description (Unix) or access mask
    /// (Windows) and keeps the self-lease alive until the last handle drops.
    pub fn try_clone_for_stdio(&self) -> Result<OplogStdioHandle, OplogWriterError> {
        let file = self
            .file
            .try_clone()
            .map_err(|_| OplogWriterError::clone_io())?;
        Ok(OplogStdioHandle {
            file,
            lease: Arc::clone(&self.lease),
        })
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

#[cfg(unix)]
impl AsRawFd for OplogWriter {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

#[cfg(unix)]
impl AsRawFd for OplogStdioHandle {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.file.as_raw_fd()
    }
}

#[cfg(windows)]
impl AsRawHandle for OplogWriter {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.file.as_raw_handle()
    }
}

#[cfg(windows)]
impl AsRawHandle for OplogStdioHandle {
    fn as_raw_handle(&self) -> std::os::windows::io::RawHandle {
        self.file.as_raw_handle()
    }
}
