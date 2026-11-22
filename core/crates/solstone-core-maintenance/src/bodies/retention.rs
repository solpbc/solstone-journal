// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Daily cleanup of originals voice-only transcription could not delete right away.

use crate::copy::Output;
use std::path::Path;
use std::time::Duration;

use solstone_core_retention::owner_voice::finish_owed_owner_voice_deletions_checked;

use crate::{CliRun, HealthServices};

/// Retries per run for deletions that are only busy (a backup often holds the journal at the
/// daily mark), and the pause between them; both fit inside the routine's max runtime.
const BUSY_RETRIES: usize = 6;
const BUSY_PAUSE: Duration = Duration::from_secs(120);

pub(crate) fn owner_voice_originals(
    copy: Output<'_>,
    args: &[String],
    journal: &Path,
    services: &HealthServices,
) -> CliRun {
    owner_voice_originals_in(copy, args, journal, services, &|| Ok(()))
}

/// Use the same cleanup from an app-owned worker, retaining its Stop checkpoint.
pub fn owner_voice_originals_checked(
    args: &[String],
    journal: &Path,
    services: &HealthServices,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> CliRun {
    owner_voice_originals_in(Output::canonical(), args, journal, services, checkpoint)
}

fn owner_voice_originals_in(
    copy: Output<'_>,
    args: &[String],
    journal: &Path,
    services: &HealthServices,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> CliRun {
    if !args.is_empty() {
        return CliRun {
            stdout: String::new(),
            stderr: copy.line("journal.cli.maintenance.retention.usage", []),
            exit_code: 2,
        };
    }
    let today = services.now.to_zoned_datetime(services.zone.clone()).date();
    let counts = match finish_owed_owner_voice_deletions_checked(
        journal,
        today,
        BUSY_RETRIES,
        BUSY_PAUSE,
        &jiff::Timestamp::now,
        checkpoint,
    ) {
        Ok(counts) => counts,
        Err(error) => {
            return CliRun {
                stdout: String::new(),
                stderr: copy.line(
                    "journal.cli.maintenance.retention.owed",
                    [("error", &error)],
                ),
                exit_code: 1,
            };
        }
    };
    if let Err(error) = solstone_core_import::audio_originals::finish_owed(journal, checkpoint) {
        return CliRun {
            stdout: String::new(),
            stderr: copy.line(
                "journal.cli.maintenance.retention.import.owed",
                [("error", &error)],
            ),
            exit_code: 1,
        };
    }
    CliRun {
        stdout: copy.line(
            "journal.cli.maintenance.retention.summary",
            [
                ("deleted", &counts.deleted),
                ("deleted_unrecorded", &counts.deleted_unrecorded),
                ("gone", &counts.gone),
                ("owed", &counts.owed),
                ("refused", &counts.refused),
                ("not_provable", &counts.not_provable),
                ("not_owed", &counts.not_owed),
                ("skipped", &counts.skipped),
                ("unknown", &counts.unknown),
            ],
        ),
        stderr: String::new(),
        exit_code: i32::from(counts.unknown > 0 || counts.deleted_unrecorded > 0),
    }
}
