// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, argv-sensitive restore fixtures for dependent crate tests.

use std::io;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use solstone_core_backup::BackupError;

use crate::engine::RestoreRecorder;
use crate::readiness::RESTIC_VERSION;
use crate::runner::{ToolOutput, ToolRequest, ToolRunner};

const REFUSED_LATEST_EXIT: i32 = 97;
const REFUSED_UNEXPECTED_EXIT: i32 = 98;

pub struct ArgvResticFixture {
    catalog: String,
    restore: ToolOutput,
    check: ToolOutput,
    calls: Mutex<Vec<Vec<String>>>,
    refusals: Mutex<Vec<Vec<String>>>,
}

impl ArgvResticFixture {
    pub fn new(catalog: impl Into<String>, restore: ToolOutput, check: ToolOutput) -> Self {
        Self {
            catalog: catalog.into(),
            restore,
            check,
            calls: Mutex::new(vec![]),
            refusals: Mutex::new(vec![]),
        }
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().expect("fixture calls lock").clone()
    }

    pub fn refusals(&self) -> Vec<Vec<String>> {
        self.refusals.lock().expect("fixture refusals lock").clone()
    }

    fn refuse(&self, argv: Vec<String>, code: i32) -> io::Result<ToolOutput> {
        self.refusals
            .lock()
            .expect("fixture refusals lock")
            .push(argv);
        Ok(ToolOutput {
            returncode: code,
            stdout: vec![],
            stderr: vec![],
        })
    }
}

impl ToolRunner for ArgvResticFixture {
    fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
        let argv = request
            .argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        self.calls
            .lock()
            .expect("fixture calls lock")
            .push(argv.clone());
        match argv.first().map(String::as_str) {
            Some("version") if argv == ["version"] => Ok(ToolOutput {
                returncode: 0,
                stdout: format!("restic {RESTIC_VERSION}\n").into_bytes(),
                stderr: vec![],
            }),
            Some("snapshots") if argv.iter().any(|argument| argument == "latest") => {
                self.refuse(argv, REFUSED_LATEST_EXIT)
            }
            Some("snapshots") if argv == ["snapshots", "--json"] => Ok(ToolOutput {
                returncode: 0,
                stdout: self.catalog.as_bytes().to_vec(),
                stderr: vec![],
            }),
            Some("restore")
                if matches!(
                    argv.as_slice(),
                    [command, snapshot, target, _, json]
                        if command == "restore"
                            && snapshot.contains(':')
                            && target == "--target"
                            && json == "--json"
                ) =>
            {
                Ok(self.restore.clone())
            }
            Some("check") if argv == ["check"] => Ok(self.check.clone()),
            _ => self.refuse(argv, REFUSED_UNEXPECTED_EXIT),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestoreRecord {
    pub status: String,
    pub time: Value,
    pub reason: Value,
    pub scope: String,
    pub day: Value,
    pub segments_selected: Value,
    pub segments_restored: Value,
    pub files_expected: Value,
    pub files_restored: Value,
    pub bytes_expected: Value,
    pub bytes_restored: Value,
}

pub struct RestoreRecorderSpy {
    fail: bool,
    calls: Mutex<Vec<RestoreRecord>>,
}

impl RestoreRecorderSpy {
    pub fn new() -> Self {
        Self {
            fail: false,
            calls: Mutex::new(vec![]),
        }
    }

    pub fn failing() -> Self {
        Self {
            fail: true,
            calls: Mutex::new(vec![]),
        }
    }

    pub fn calls(&self) -> Vec<RestoreRecord> {
        self.calls.lock().expect("restore recorder lock").clone()
    }
}

impl Default for RestoreRecorderSpy {
    fn default() -> Self {
        Self::new()
    }
}

impl RestoreRecorder for RestoreRecorderSpy {
    fn record(
        &self,
        _: &Path,
        status: &str,
        time: Value,
        reason: Value,
        scope: &str,
        day: Value,
        segments_selected: Value,
        segments_restored: Value,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) -> Result<(), BackupError> {
        self.calls
            .lock()
            .expect("restore recorder lock")
            .push(RestoreRecord {
                status: status.to_owned(),
                time,
                reason,
                scope: scope.to_owned(),
                day,
                segments_selected,
                segments_restored,
                files_expected,
                files_restored,
                bytes_expected,
                bytes_restored,
            });
        if self.fail {
            Err(BackupError::InvalidRestoreStatus)
        } else {
            Ok(())
        }
    }
}
