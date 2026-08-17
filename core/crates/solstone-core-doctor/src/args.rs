// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
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
pub struct DoctorUsageError(pub String);
/// The usage the owner-facing `journal doctor` error path prints.
/// It names `journal doctor`, not `solstone-core doctor`, because that is the
/// command the owner typed before the journal dispatcher execed the native sibling.
pub const USAGE: &str = concat!(
    "usage: journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\n",
    "                      [--readiness]\n",
);

/// The help `journal doctor` prints in the owner-facing command vocabulary.
/// It names `journal doctor`, not `solstone-core doctor`, because that is the
/// command the owner typed before the journal dispatcher execed the native sibling.
pub const HELP: &str = concat!(
    "usage: journal doctor [-h] [--verbose] [--json] [--jsonl] [--port PORT]\n",
    "                      [--readiness]\n",
    "\n",
    "Run solstone diagnostics.\n",
    "\n",
    "options:\n",
    "  -h, --help         show this help message and exit\n",
    "  --verbose          print every check result\n",
    "  --json             emit JSON instead of text\n",
    "  --jsonl            emit one-JSON-per-line events instead of text\n",
    "  --port PORT        port to probe (default: 5015)\n",
    "  --readiness        run the setup readiness battery\n",
    "\n",
    "If 'journal doctor' is unavailable (e.g. before 'make install' completes), run\n",
    "'python3 scripts/doctor.py' from the repo root for the same diagnostic.\n",
);
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
                    .ok_or_else(|| DoctorUsageError("--port requires an integer".into()))?
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
