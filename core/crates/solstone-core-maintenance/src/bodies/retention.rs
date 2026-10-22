// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Daily cleanup of originals voice-only transcription could not delete right away.

use std::path::Path;
use std::time::Duration;

use solstone_core_retention::owner_voice::finish_owed_owner_voice_deletions;

use crate::{CliRun, HealthServices};

/// Retries per run for deletions that are only busy (a backup often holds the journal at the
/// daily mark), and the pause between them; both fit inside the routine's max runtime.
const BUSY_RETRIES: usize = 6;
const BUSY_PAUSE: Duration = Duration::from_secs(120);

pub(crate) fn owner_voice_originals(
    args: &[String],
    journal: &Path,
    services: &HealthServices,
) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: retention:owner-voice-originals takes no arguments.\n".to_owned(),
            exit_code: 2,
        };
    }
    let today = services.now.with_timezone(&services.zone).date_naive();
    let counts = finish_owed_owner_voice_deletions(
        journal,
        today,
        BUSY_RETRIES,
        BUSY_PAUSE,
        &chrono::Utc::now,
    );
    CliRun {
        stdout: format!(
            "owner-voice-originals: deleted {}, deleted without a record {}, already gone {}, still owed {}, kept for good {}, not provable {}, nothing owed {}, skipped while busy {}, events unreadable {}\n",
            counts.deleted,
            counts.deleted_unrecorded,
            counts.gone,
            counts.owed,
            counts.refused,
            counts.not_provable,
            counts.not_owed,
            counts.skipped,
            counts.unknown,
        ),
        stderr: String::new(),
        exit_code: 0,
    }
}
