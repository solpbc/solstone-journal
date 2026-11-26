// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! CLI and scheduled runner for the read-only email source (`--sync email`).

use std::path::{Path, PathBuf};

use solstone_core_email::store::{EMAIL_STREAM, journal_timezone};
use solstone_core_email::sync::{
    CreatedEmailSegment, status_snapshot, sync_live_gmail_with_publisher,
};
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::publish::{CreatedSegment, PublicationInput, PublicationStatus, publish};
use solstone_core_segment::{ImportSource, Kind, StreamHints};

use crate::cli_argv::{failure, success};

/// Publish one newly landed batch of email segments into the stream record,
/// search indexer queue, day health markers, and supervisor event bus.
pub fn publish_email_batch(root: &Path, run_id: &str, created: &[CreatedEmailSegment]) {
    if created.is_empty() {
        return;
    }
    let segments: Vec<CreatedSegment> = created
        .iter()
        .map(|seg| CreatedSegment {
            day: seg.day.clone(),
            segment: seg.segment.clone(),
            stream: EMAIL_STREAM.to_owned(),
            hints: StreamHints {
                kind: Some(Kind::Imported(ImportSource::Named("email".into()))),
                host: None,
                platform: None,
            },
        })
        .collect();
    let files: Vec<PathBuf> = created.iter().map(|seg| seg.file.clone()).collect();
    match publish(PublicationInput {
        journal: root,
        import_dir: None,
        import_id: run_id,
        importer: "email",
        revision: None,
        segments: &segments,
        files_created: &files,
        index_only: &[],
        may_write_record: None,
    }) {
        Ok(record) if record.status == PublicationStatus::Success => {}
        Ok(_) => {
            log::warn!("email batch publication reported partial or failed status for {run_id}");
        }
        Err(err) => {
            log::warn!("email batch publication failed for {run_id}: {err}");
        }
    }
}

/// Run `--sync email` from the importer CLI or the hourly scheduler.
pub(crate) fn run_cli_sync(
    journal_path: &Path,
    save: bool,
    dry_run: bool,
    scheduled: bool,
) -> CliRun {
    let zone = journal_timezone(journal_path);
    if save && !dry_run {
        let publisher = |run_id: &str, batch: &[CreatedEmailSegment]| {
            publish_email_batch(journal_path, run_id, batch);
        };
        if let Err(err) =
            sync_live_gmail_with_publisher(journal_path, scheduled, false, Some(&publisher))
        {
            return failure("", &format!("{err}\n"), 1);
        }
    }
    match status_snapshot(journal_path, &zone) {
        Ok(snapshot) => match serde_json::to_string(&snapshot) {
            Ok(json) => success(format!("{json}\n")),
            Err(err) => failure("", &format!("{err}\n"), 1),
        },
        Err(err) => failure("", &format!("{err}\n"), 1),
    }
}
