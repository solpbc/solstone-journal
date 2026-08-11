// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::{OsStr, OsString};

pub const USAGE: &str = concat!(
    "usage: journal journal-stats [-h] [--no-cache] [-v] [-d]",
    "\n"
);

pub const HELP: &str = concat!(
    "usage: journal journal-stats [-h] [--no-cache] [-v] [-d]\n",
    "\n",
    "Scan a solstone journal and generate statistics\n",
    "\n",
    "options:\n",
    "  -h, --help     show this help message and exit\n",
    "  --no-cache     Disable per-day caching (force re-scan all days)\n",
    "  -v, --verbose  Enable verbose output\n",
    "  -d, --debug    Enable debug logging\n",
);

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
            return Err(format!(
                "unrecognized arguments: {}",
                argument.to_string_lossy()
            ));
        }
    }
    Ok(ParseResult::Options(options))
}
