// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Apple Health export through the ordinary import lifecycle, into body
//! windows. `docs/BODY_SEGMENTS.md` is the contract.
//!
//! No owner route reaches this yet: every owner command still saves a bundle, and
//! this function's one caller outside tests is `examples/apple_body_segments.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use solstone_core_import::publish::{PublicationStatus, read_publication_record};
use solstone_core_import::text::TextCreated;
use solstone_core_import_sources::apple_health::{
    LOCK_REL, PUBLISHING_MARKER_REL, STREAM, facet_file_text, plan, read_export,
    read_or_create_zone, summary_text,
};
use solstone_core_import_sources::save;
use solstone_core_journal_io::{
    AtomicWriteOptions, LockOptions, atomic_replace, hold_lock, remove_file,
};

use crate::import_publication::{
    ImportFinish, ImportTerminalInput, finish_import_attempt_indexing,
};
use crate::registry::now_ms;
use solstone_core_format::content::body_records::BODY_RECORDS_FILE;
use solstone_core_import::RegistrySource;

const RAW_RETENTION: &str = "discard";
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// What one run was asked to do.
pub struct AppleSegmentsRequest<'a> {
    pub source: &'a Path,
    /// `YYYYMMDD_HHMMSS`, as every import's id.
    pub timestamp: &'a str,
    pub facet: Option<&'a str>,
}

/// What one run did.
#[derive(Debug)]
pub struct AppleSegmentsOutcome {
    pub import_id: String,
    pub summary: String,
    pub published: u64,
    /// Whether this run is known to have finished: nothing is left to repair.
    pub finished: bool,
}

fn window_dir(journal: &Path, day: &str, key: &str) -> PathBuf {
    journal.join("chronicle").join(day).join(STREAM).join(key)
}

/// Publish an Apple export as body windows. Runs under the source's lock.
pub fn run_apple_health_segments(
    journal: &Path,
    request: &AppleSegmentsRequest<'_>,
) -> Result<AppleSegmentsOutcome, String> {
    if let Some(facet) = request.facet {
        crate::cli_argv::require_enabled_facet(facet, journal)?;
    }
    let export = read_export(request.source).map_err(|error| error.to_string())?;
    std::fs::create_dir_all(journal.join("imports")).map_err(|error| error.to_string())?;
    let _lock = hold_lock(
        journal.join(LOCK_REL),
        LockOptions {
            timeout: LOCK_TIMEOUT,
            mode: Some(0o600),
            ..LockOptions::default()
        },
    )
    .map_err(|_| "apple_health_lock_unavailable".to_owned())?;
    let zone = read_or_create_zone(journal)?;
    let requested =
        solstone_core_import::validate_timestamp(request.timestamp).map_err(|e| e.to_string())?;
    let bound = solstone_core_import::bind_import_record(journal, &requested, Some(request.source))
        .map_err(|error| error.to_string())?;
    let import_id = bound.import_id.as_str().to_owned();
    if solstone_core_import::refuse_if_live_running(journal, &import_id, STREAM).is_some() {
        return Err("this import is already running".to_owned());
    }
    let generation = solstone_core_import::admit_running_attempt(
        journal,
        &import_id,
        now_ms(),
        Some("apple_health"),
    )
    .map_err(|error| error.to_string())?
    .generation;
    let finish = |input, index_only: &[PathBuf]| {
        finish_import_attempt_indexing(
            journal,
            &import_id,
            generation,
            RegistrySource::AppleHealth.name(),
            input,
            index_only,
        )
    };

    // From here a failure closes the attempt, or a re-run would read as still running.
    let abort = |error: String| {
        let _ = finish(ImportTerminalInput::Failed(&[]), &[]);
        error
    };
    let marker = journal.join(PUBLISHING_MARKER_REL);
    let repair = marker.exists();
    let private = AtomicWriteOptions { mode: Some(0o600) };
    atomic_replace(&marker, b"", private).map_err(|error| abort(error.to_string()))?;

    let mut planned = plan(journal, &export, zone, request.facet, repair).map_err(&abort)?;
    if let Some(facet) = request.facet {
        for (day, key) in &planned.facet_windows {
            let path = window_dir(journal, day, key).join("talents/facets.json");
            std::fs::create_dir_all(path.parent().expect("facets file has a parent"))
                .map_err(|error| abort(error.to_string()))?;
            atomic_replace(&path, facet_file_text(facet).as_bytes(), private)
                .map_err(|error| abort(error.to_string()))?;
        }
    }
    let rendered = planned.rendered();
    let written = save::write_rendered(journal, Some(&import_id), &rendered);
    let created: Vec<TextCreated> = planned
        .publish
        .iter()
        // Binding a window that holds no records would make it not body-only.
        .filter(|(day, key)| {
            window_dir(journal, day, key)
                .join(BODY_RECORDS_FILE)
                .is_file()
        })
        .map(|(day, key)| TextCreated {
            day: day.clone(),
            segment: key.clone(),
            stream: STREAM.to_owned(),
            hints: save::stream_hints(RegistrySource::AppleHealth),
            path: window_dir(journal, day, key).join(BODY_RECORDS_FILE),
        })
        .collect();
    if let Some(error) = written.error {
        let _ = finish(ImportTerminalInput::Failed(&created), &[]);
        return Err(format!("{}: {}", error.path.display(), error.detail));
    }

    let summary = summary_text(
        &export,
        &planned,
        rendered.files.len() as u64,
        request.facet,
        zone,
    );
    let summary_path = journal.join("imports").join(&import_id).join("summary.md");
    atomic_replace(&summary_path, summary.as_bytes(), private)
        .map_err(|error| abort(error.to_string()))?;
    let source_hash = solstone_core_import::hash_source(request.source)
        .map_err(|error| abort(error.to_string()))?;
    let mut days: Vec<String> = planned.publish.iter().map(|(day, _)| day.clone()).collect();
    days.sort_unstable();
    days.dedup();
    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal,
        import_id: &import_id,
        source_type: "apple_health",
        source_hash: &source_hash,
        entry_count: rendered.entries,
        days_affected: &days,
        files_created: &[],
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: Some(RAW_RETENTION),
    })
    .map_err(|error| abort(error.to_string()))?;

    let index_only = [summary_path];
    match finish(ImportTerminalInput::Success(&created), &index_only) {
        Ok(ImportFinish::Applied) => {}
        Ok(ImportFinish::Stale) => return Err("this import was superseded".to_owned()),
        Err(error) => return Err(error.to_string()),
    }
    // `Applied` also covers a stopped run and a publication that failed, so only
    // the attempt's own result and the written record can clear the marker.
    let finished = solstone_core_import::project_import_result(journal, &import_id).status
        == solstone_core_import::ProjectionStatus::Success
        && read_publication_record(&journal.join("imports").join(&import_id))
            .ok()
            .flatten()
            .is_some_and(|record| record.status == PublicationStatus::Success);
    if finished {
        remove_file(journal, PUBLISHING_MARKER_REL).map_err(|error| error.to_string())?;
    }
    Ok(AppleSegmentsOutcome {
        import_id,
        summary,
        published: planned.publish.len() as u64,
        finished,
    })
}

#[cfg(test)]
#[path = "body_segments_tests.rs"]
mod tests;
