// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy as copy;
use std::ffi::{OsStr, OsString};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorArgs {
    pub verbose: bool,
    pub json: bool,
    pub jsonl: bool,
    pub port: u16,
    pub readiness: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorUsageError(&'static str);
impl DoctorUsageError {
    pub fn message(&self) -> String {
        copy::text(self.0)
    }
}
pub fn help() -> String {
    copy::text("journal.cli.doctor.help")
}
pub fn parse_doctor_args(args: &[OsString]) -> Result<DoctorArgs, DoctorUsageError> {
    let mut parsed = DoctorArgs {
        verbose: false,
        json: false,
        jsonl: false,
        port: 5015,
        readiness: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_os_str() {
            x if x == OsStr::new("--verbose") => parsed.verbose = true,
            x if x == OsStr::new("--json") => parsed.json = true,
            x if x == OsStr::new("--jsonl") => parsed.jsonl = true,
            x if x == OsStr::new("--readiness") => parsed.readiness = true,
            x if x == OsStr::new("--port") => {
                i += 1;
                parsed.port = args
                    .get(i)
                    .and_then(|v| v.to_str())
                    .and_then(|v| v.parse().ok())
                    .ok_or(DoctorUsageError("journal.cli.doctor.usage.port"))?
            }
            _ => return Err(DoctorUsageError("journal.cli.doctor.usage.argument")),
        }
        i += 1;
    }
    if parsed.json && parsed.jsonl {
        return Err(DoctorUsageError("journal.cli.doctor.usage.formats"));
    }
    Ok(parsed)
}
