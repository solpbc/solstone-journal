// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Daily cleanup of originals voice-only transcription could not delete right away.

use std::path::Path;
use std::time::Duration;

use solstone_core_retention::owner_voice::finish_owed_owner_voice_deletions_checked;

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
    owner_voice_originals_checked(args, journal, services, &|| Ok(()))
}

/// Use the same cleanup from an app-owned worker, retaining its Stop checkpoint.
pub fn owner_voice_originals_checked(
    args: &[String],
    journal: &Path,
    services: &HealthServices,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: "usage: retention:owner-voice-originals takes no arguments.\n".to_owned(),
            exit_code: 2,
        };
    }
    let today = services.now.with_timezone(&services.zone).date_naive();
    let counts = match finish_owed_owner_voice_deletions_checked(
        journal,
        today,
        BUSY_RETRIES,
        BUSY_PAUSE,
        &chrono::Utc::now,
        checkpoint,
    ) {
        Ok(counts) => counts,
        Err(error) => {
            return CliRun {
                stdout: String::new(),
                stderr: format!("owner voice original cleanup incomplete: {error}\n"),
                exit_code: 1,
            };
        }
    };
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
        exit_code: i32::from(counts.unknown > 0 || counts.deleted_unrecorded > 0),
    }
}
