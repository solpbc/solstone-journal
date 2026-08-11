// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

pub const USAGE: &str = concat!(
    "usage: journal backfill-processing-records [-h] [--day DAY] [--commit |\n",
    "                                           --dry-run] [-v] [-d]\n",
);

pub const HELP: &str = concat!(
    "usage: journal backfill-processing-records [-h] [--day DAY] [--commit |\n",
    "                                           --dry-run] [-v] [-d]\n",
    "\n",
    "Backfill empty processing records onto stuck header-only native\n",
    "describe/transcribe outputs\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --day DAY      Day in YYYYMMDD format; defaults to all days\n",
    "  --commit       Rewrite eligible JSONL headers with empty processing records\n",
    "  --dry-run      Preview eligible outputs without writing changes\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub day: Option<String>,
    pub commit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseResult {
    Help,
    Options(Options),
}

pub fn parse(args: &[OsString]) -> Result<ParseResult, String> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("-h") || arg == OsStr::new("--help"))
    {
        return Ok(ParseResult::Help);
    }

    let mut day = None;
    let mut mode: Option<&str> = None;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if argument == OsStr::new("--commit") || argument == OsStr::new("--dry-run") {
            let next = if argument == OsStr::new("--commit") {
                "--commit"
            } else {
                "--dry-run"
            };
            if let Some(previous) = mode
                && previous != next
            {
                return Err(format!(
                    "argument {next}: not allowed with argument {previous}"
                ));
            }
            mode = Some(next);
            index += 1;
            continue;
        }
        if argument == OsStr::new("-v")
            || argument == OsStr::new("--verbose")
            || argument == OsStr::new("-d")
            || argument == OsStr::new("--debug")
        {
            index += 1;
            continue;
        }
        if argument == OsStr::new("--day") {
            let Some(value) = args.get(index + 1) else {
                return Err("argument --day: expected one argument".to_owned());
            };
            if value.to_string_lossy().starts_with('-') {
                return Err("argument --day: expected one argument".to_owned());
            }
            day = Some(value.to_string_lossy().into_owned());
            index += 2;
            continue;
        }
        if let Some(value) = argument.to_string_lossy().strip_prefix("--day=") {
            day = Some(value.to_owned());
            index += 1;
            continue;
        }
        return Err(format!(
            "unrecognized arguments: {}",
            argument.to_string_lossy()
        ));
    }

    Ok(ParseResult::Options(Options {
        day,
        commit: mode == Some("--commit"),
    }))
}
