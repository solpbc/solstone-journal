// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash's timer: finish steps a stop interrupted, erase what is due.

use crate::copy::Output;
use std::path::Path;

use crate::CliRun;

/// Run the trash's timer once.
pub fn erase(args: &[String], journal: &Path) -> CliRun {
    erase_in(Output::canonical(), args, journal)
}

pub(crate) fn erase_in(copy: Output<'_>, args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: copy.line("journal.cli.maintenance.trash.usage", []),
            exit_code: 2,
        };
    }
    let swept = solstone_core_import::trash_kinds::sweep(journal, jiff::Timestamp::now());
    let mut stderr = String::new();
    for failure in &swept.failed {
        stderr.push_str(&copy.line(
            "journal.cli.maintenance.trash.error",
            [("failure", &failure)],
        ));
    }
    CliRun {
        stdout: copy.line(
            "journal.cli.maintenance.trash.erased",
            [
                ("erased", &swept.erased),
                ("finished", &swept.resumed),
                ("waiting", &swept.waiting),
                ("failed", &swept.failed.len()),
            ],
        ),
        stderr,
        exit_code: i32::from(!swept.failed.is_empty()),
    }
}
