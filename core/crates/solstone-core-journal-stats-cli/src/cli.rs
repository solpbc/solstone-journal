// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) use_cache: bool,
    pub(crate) verbose: bool,
    pub(crate) debug: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParseResult {
    Help,
    Options(Options),
}

pub(crate) fn parse(args: &[OsString]) -> Result<ParseResult, String> {
    if args
        .iter()
        .any(|arg| arg == OsStr::new("-h") || arg == OsStr::new("--help"))
    {
        return Ok(ParseResult::Help);
    }

    let mut options = Options {
        use_cache: true,
        ..Options::default()
    };
    for argument in args {
        if argument == OsStr::new("--no-cache") {
            options.use_cache = false;
        } else if argument == OsStr::new("-v") || argument == OsStr::new("--verbose") {
            options.verbose = true;
        } else if argument == OsStr::new("-d") || argument == OsStr::new("--debug") {
            options.debug = true;
        } else {
            return Err(solstone_core_cli_copy::strings(
                "journal.cli.stats.arguments",
                [("argument", &argument.to_string_lossy())],
            ));
        }
    }
    Ok(ParseResult::Options(options))
}
