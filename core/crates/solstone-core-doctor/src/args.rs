// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry;
use std::ffi::{OsStr, OsString};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorArgs {
    pub verbose: bool,
    pub json: bool,
    pub jsonl: bool,
    pub port: u16,
    pub feature: Option<String>,
    pub readiness: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorUsageError(pub String);
pub const USAGE: &str = "Usage: solstone-core doctor [--verbose] [--json | --jsonl] [--port PORT] [--feature NAME] [--readiness]\n";
pub fn parse_doctor_args(args: &[OsString]) -> Result<DoctorArgs, DoctorUsageError> {
    let mut parsed = DoctorArgs {
        verbose: false,
        json: false,
        jsonl: false,
        port: 5015,
        feature: None,
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
                    .ok_or_else(|| DoctorUsageError("--port requires an integer".into()))?
            }
            x if x == OsStr::new("--feature") => {
                i += 1;
                let name = args
                    .get(i)
                    .and_then(|v| v.to_str())
                    .ok_or_else(|| DoctorUsageError("--feature requires a name".into()))?;
                if !registry::feature_entries().any(|e| e.feature == Some(name)) {
                    let mut names = registry::feature_entries()
                        .filter_map(|e| e.feature)
                        .collect::<Vec<_>>();
                    names.sort_unstable();
                    let names = names.join(", ");
                    return Err(DoctorUsageError(format!(
                        "unknown feature {name:?}; known features: {names}"
                    )));
                }
                parsed.feature = Some(name.into())
            }
            _ => return Err(DoctorUsageError("unexpected argument".into())),
        }
        i += 1;
    }
    if parsed.json && parsed.jsonl {
        return Err(DoctorUsageError(
            "--json and --jsonl are mutually exclusive".into(),
        ));
    }
    Ok(parsed)
}
