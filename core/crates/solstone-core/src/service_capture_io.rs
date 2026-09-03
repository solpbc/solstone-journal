// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Oplog creation and descriptor redirection owned by service capture.

use std::io;
use std::path::Path;

use chrono::{DateTime, FixedOffset};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{OplogFormat, OplogWriter, create_oplog_at},
};

pub(super) fn open_service_oplog(
    journal: &Path,
    opened: DateTime<FixedOffset>,
) -> Result<OplogWriter, String> {
    let root = JournalRoot::open(journal)
        .map_err(|error| format!("open journal for service capture: {error}"))?;
    create_oplog_at(root, "service", "supervisor", OplogFormat::Log, opened)
        .map_err(|error| format!("create service capture oplog: {error}"))
}

pub(super) fn redirect_both(next: &OplogWriter, previous: Option<&OplogWriter>) -> io::Result<()> {
    redirect_both_with(next, previous, duplicate_stdout, duplicate_stderr)
}

pub(super) fn redirect_both_with(
    next: &OplogWriter,
    previous: Option<&OplogWriter>,
    duplicate_stdout: impl Fn(&OplogWriter) -> io::Result<()>,
    duplicate_stderr: impl Fn(&OplogWriter) -> io::Result<()>,
) -> io::Result<()> {
    duplicate_stdout(next)?;
    if let Err(error) = duplicate_stderr(next) {
        if let Some(previous) = previous {
            let _ = duplicate_stdout(previous);
        }
        return Err(error);
    }
    Ok(())
}

fn duplicate_stdout(writer: &OplogWriter) -> io::Result<()> {
    // `dup2` is the required process-wide redirection primitive: library code
    // that writes directly to fd 1 or 2 cannot otherwise participate.
    nix::unistd::dup2_stdout(writer).map_err(io::Error::from)
}

fn duplicate_stderr(writer: &OplogWriter) -> io::Result<()> {
    nix::unistd::dup2_stderr(writer).map_err(io::Error::from)
}
