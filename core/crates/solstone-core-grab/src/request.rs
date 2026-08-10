// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Input supplied by the future CLI adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GrabRequest {
    pub tokens: Vec<String>,
    pub out: Option<PathBuf>,
    pub force: bool,
}

/// Completed machine payload and its pre-rendered human representation.
#[derive(Clone, Debug, PartialEq)]
pub struct GrabOutput {
    pub payload: Value,
    pub human: String,
}

/// Explicit diagnostic boundary for the inherited JSONL-read behavior.
pub trait GrabDiagnostics {
    fn malformed_jsonl(&mut self, path: &Path, line: usize, error: &str);
    fn read_error(&mut self, path: &Path, error: &std::io::Error);
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecordingDiagnostics {
    pub malformed: Vec<(PathBuf, usize, String)>,
    pub read_errors: Vec<(PathBuf, String)>,
}

impl GrabDiagnostics for RecordingDiagnostics {
    fn malformed_jsonl(&mut self, path: &Path, line: usize, error: &str) {
        self.malformed
            .push((path.to_path_buf(), line, error.to_owned()));
    }

    fn read_error(&mut self, path: &Path, error: &std::io::Error) {
        self.read_errors
            .push((path.to_path_buf(), error.to_string()));
    }
}
