// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::PathBuf;
use std::time::Duration;

/// Which child output pipe produced a drained line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Process lifecycle events for a caller-owned transport adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    Spawned {
        reference: String,
        name: String,
        pid: u32,
        cmd: Vec<String>,
        log_path: PathBuf,
    },
    Line {
        reference: String,
        name: String,
        pid: u32,
        stream: OutputStream,
        line: String,
    },
    Exited {
        reference: String,
        name: String,
        pid: u32,
        exit_code: Option<i32>,
        duration: Duration,
        cmd: Vec<String>,
        log_path: PathBuf,
    },
}

/// Best-effort process event destination. Absence never affects child execution.
pub trait ProcessEventSink: Send + Sync {
    fn emit(&self, event: ProcessEvent);
}
