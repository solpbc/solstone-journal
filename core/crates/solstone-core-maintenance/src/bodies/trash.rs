// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The trash's timer: finish steps a stop interrupted, erase what is due.

use std::path::Path;

use solstone_core_indexer_store::RetentionIndex;
use solstone_core_retention::recording_trash::Recordings;

use crate::CliRun;

/// Run the trash's timer once.
pub fn erase(args: &[String], journal: &Path) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: trash:erase takes no arguments.\n".to_owned(),
            exit_code: 2,
        };
    }
    let index = RetentionIndex::new(journal);
    let recordings = Recordings::new(&index);
    let swept = solstone_core_trash::sweep(
        journal,
        &[
            &recordings,
            &solstone_core_trash::previous_journal::PreviousJournal,
        ],
        chrono::Utc::now(),
    );
    let mut stderr = String::new();
    for failure in &swept.failed {
        stderr.push_str(&format!("trash: {failure}\n"));
    }
    CliRun {
        stdout: format!(
            "trash: erased {}, finished {}, waiting {}, failed {}\n",
            swept.erased,
            swept.resumed,
            swept.waiting,
            swept.failed.len()
        ),
        stderr,
        exit_code: i32::from(!swept.failed.is_empty()),
    }
}
