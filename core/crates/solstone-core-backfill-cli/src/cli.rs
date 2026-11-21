// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_cli_copy as copy;

use std::ffi::{OsStr, OsString};

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
                return Err(copy::display(
                    "journal.cli.backfill.flags.conflict",
                    [("flag", &next), ("previous", &previous)],
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
                return Err(copy::text("journal.cli.backfill.flags.missing"));
            };
            if value.to_string_lossy().starts_with('-') {
                return Err(copy::text("journal.cli.backfill.flags.missing"));
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
        return Err(copy::display(
            "journal.cli.backfill.flags.unknown",
            [("argument", &argument.to_string_lossy())],
        ));
    }

    Ok(ParseResult::Options(Options {
        day,
        commit: mode == Some("--commit"),
    }))
}
