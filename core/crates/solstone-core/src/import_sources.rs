// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Top-level dispatch for source bodies that depend on the import contract.

use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Map;
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_import::cli_render::CliRun;
use solstone_core_import::publish::NativePublicationOperations;
use solstone_core_import::{
    AttemptFacts, AttemptState, ImportError, ImportResult, RegistrySource, cli_render,
};
use solstone_core_import_host::cli_argv::RegistryDispatch;
use solstone_core_import_host::import_publication::{
    ImportFinish, ImportTerminalInput, finish_import_attempt,
};
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, ArchiveMergeResult, FullReindexRequester, ReindexStatus, RetryDisposition,
    merge_journal_archive, plan_journal_archive, validate_archive_preflight,
};
use solstone_core_import_sources::{
    ImportSourcesError, MergeMutationState, chatgpt, claude, document, gemini, ics, image,
    obsidian, save, strava,
};
use solstone_core_journal_io::{DEFAULT_LOCK_TIMEOUT, LockOptions, hold_lock};

struct SupervisorRescan {
    journal: PathBuf,
}

impl FullReindexRequester for SupervisorRescan {
    fn request_full_reindex(&self) -> Result<bool, String> {
        send_indexer_rescan(&self.journal);
        Ok(true)
    }
}

/// Best-effort: ask the supervisor to rescan newly landed content. An
/// unavailable Callosum socket is logged and leaves the import successful.
fn send_indexer_rescan(journal: &Path) {
    let mut extra = Map::new();
    extra.insert(
        "cmd".to_owned(),
        serde_json::Value::from(solstone_core_system::partition::canonical_journal_command(
            ["indexer", "--rescan"],
        )),
    );
    let envelope = CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    };
    let Ok(mut line) = serde_json::to_string(&envelope) else {
        return;
    };
    line.push('\n');
    let sender = CallosumOneShotSender::new(
        journal.join("health").join("callosum.sock"),
        Duration::from_secs(1),
    );
    if sender.send_line(&line).is_err() {
        log::warn!("indexer rescan was not queued: Callosum socket unavailable");
    }
}

const PDF_WORKER_TIMEOUT: Duration = Duration::from_secs(90);

#[cfg(test)]
pub fn run(dispatch: RegistryDispatch, journal: &Path) -> CliRun {
    run_bound(dispatch, journal).0
}

/// Run one registry import and return the directory id when one was claimed.
///
/// Dry-run and preview return `None` and do not allocate. The id is the
/// selected directory, which can differ from the source timestamp.
pub fn run_bound(dispatch: RegistryDispatch, journal: &Path) -> (CliRun, Option<String>) {
    let mut selected = None;
    let zone = solstone_core_journal_config::owner_zone(journal);
    let cli = match dispatch.source {
        RegistrySource::Ics => run_save(dispatch, journal, &mut selected, |path| {
            ics::preview(path, &zone)
        }),
        RegistrySource::Obsidian => run_save(dispatch, journal, &mut selected, |path| {
            obsidian::preview(path, &zone)
        }),
        RegistrySource::Claude => run_save(dispatch, journal, &mut selected, |path| {
            claude::preview(path, &zone)
        }),
        RegistrySource::Chatgpt => run_save(dispatch, journal, &mut selected, |path| {
            chatgpt::preview(path, &zone)
        }),
        RegistrySource::Gemini => run_save(dispatch, journal, &mut selected, |path| {
            gemini::preview(path, &zone)
        }),
        RegistrySource::Document => run_document(dispatch, journal, &mut selected),
        RegistrySource::Image => run_image(dispatch, journal, &mut selected),
        RegistrySource::JournalArchive => run_archive(dispatch, journal, &mut selected),
        RegistrySource::Strava => run_strava(dispatch, journal, &mut selected),
        RegistrySource::AppleHealth | RegistrySource::Oura => {
            unreachable!("resolver preempts body")
        }
    };
    (cli, selected)
}

fn bind_dispatch(journal: &Path, dispatch: &RegistryDispatch) -> Result<String, String> {
    let requested = solstone_core_import::validate_timestamp(&dispatch.timestamp)
        .map_err(|error| error.to_string())?;
    // An archive timestamp is the attempt id. A second run of that id, including
    // one whose zip bytes differ, is a new generation of the stored record.
    if dispatch.source == RegistrySource::JournalArchive
        && solstone_core_import::read_provenance(journal, requested.as_str())
            .map_err(|error| error.to_string())?
            .is_some()
    {
        return Ok(requested.as_str().to_owned());
    }
    let bound = solstone_core_import::bind_import_record(
        journal,
        &requested,
        Some(dispatch.media.as_path()),
    )
    .map_err(|error| error.to_string())?;
    Ok(bound.import_id.as_str().to_owned())
}

fn claim_dispatch_id(
    journal: &Path,
    dispatch: &mut RegistryDispatch,
    selected: &mut Option<String>,
) -> Result<(), String> {
    let import_id = bind_dispatch(journal, dispatch)?;
    dispatch.timestamp = import_id.clone();
    *selected = Some(import_id);
    Ok(())
}

/// Record in awareness that `import_id` finished during this invocation, so Home
/// can offer the finished import and the pulse knows the owner has imported.
///
/// `since_ms` is when the invocation began: an import that finished earlier (a
/// refused re-run, an idempotent no-op) is not recorded again. Best-effort: a
/// failure here is logged and leaves the import successful.
pub fn record_finished_import(journal: &Path, import_id: &str, since_ms: u64) {
    let now = chrono::Utc::now()
        .with_timezone(&solstone_core_journal_config::owner_zone(journal))
        .fixed_offset();
    if let Err(error) = record_finished_import_at(journal, import_id, since_ms, now) {
        log::warn!("import {import_id} finished but was not recorded in awareness: {error}");
    }
}

fn record_finished_import_at(
    journal: &Path,
    import_id: &str,
    since_ms: u64,
    now: chrono::DateTime<chrono::FixedOffset>,
) -> Result<(), String> {
    let projection = solstone_core_import::project_import_result(journal, import_id);
    let finished_now = projection
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.finished_at_ms)
        .is_some_and(|finished| finished >= since_ms);
    if projection.status != solstone_core_import::ProjectionStatus::Success || !finished_now {
        return Ok(());
    }
    let entries = projection
        .entries_written
        .and_then(|count| i64::try_from(count).ok())
        .unwrap_or(0);
    solstone_core_facets::record_import(
        journal,
        &projection.source_type,
        Some(&projection.source_display),
        entries,
        &now.naive_local().format("%Y%m%dT%H:%M:%S").to_string(),
        &now.date_naive().format("%Y%m%d").to_string(),
        now.timestamp_millis(),
    )
    .map(drop)
    .map_err(|error| error.to_string())
}

/// Save a source that renders into text segments: conversation exports, calendars, notes.
///
/// Admit, render, write, then publish and record under the import lock. Segment keys come
/// from the source's own timestamps, so importing the same source again rewrites the same
/// files: there is nothing to deduplicate, and every run records its own outcome.
fn run_save<E>(
    mut dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
    preview: impl FnOnce(&Path) -> Result<solstone_core_import::ImportPreview, E>,
) -> CliRun
where
    E: std::fmt::Display,
{
    let source = dispatch.source;
    let name = source.name();
    if dispatch.dry_run {
        return match preview(&dispatch.media) {
            Ok(preview) => success(cli_render::source_preview(source, &preview)),
            Err(error) => failure(format!("{name} preview failed: {error}\n")),
        };
    }
    if let Err(error) = claim_dispatch_id(journal, &mut dispatch, selected) {
        return failure(format!("{name} import failed: {error}\n"));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, source) {
        return refused;
    }
    let import_id = dispatch.timestamp.as_str();
    let generation =
        match solstone_core_import::admit_running_attempt(journal, import_id, now_ms(), Some(name))
        {
            Ok(facts) => facts.generation,
            Err(error) => return failure(format!("{name} import failed: {error}\n")),
        };
    let finish = |input| finish_import_attempt(journal, import_id, generation, name, input);
    let source_hash = match solstone_core_import::hash_source(&dispatch.media) {
        Ok(hash) => hash,
        Err(error) => {
            let _ = finish(ImportTerminalInput::Failed(&[]));
            return failure(format!("{name} import failed: {error}\n"));
        }
    };

    let zone = solstone_core_journal_config::owner_zone(journal);
    let rendered = match save::render(source, &dispatch.media, import_id, zone)
        .expect("run_save is only dispatched for sources save renders")
    {
        Ok(rendered) if rendered.files.is_empty() => {
            let _ = finish(ImportTerminalInput::Failed(&[]));
            return failure(format!("{name} import failed: found nothing to import\n"));
        }
        Ok(rendered) => rendered,
        Err(detail) => {
            let _ = finish(ImportTerminalInput::Failed(&[]));
            return failure(format!("{name} import failed: {detail}\n"));
        }
    };
    let written = save::write_rendered(journal, Some(import_id), &rendered);
    let manifest = match written.error {
        Some(error) => Err(error.to_string()),
        None => write_save_manifest(
            journal,
            import_id,
            &rendered,
            written.entries,
            &written.created,
            &source_hash,
        ),
    };
    if let Err(detail) = manifest {
        let _ = finish(ImportTerminalInput::Failed(&written.created));
        return failure(format!("{name} import failed: {detail}\n"));
    }
    match finish(ImportTerminalInput::Success(&written.created)) {
        Ok(ImportFinish::Applied) => {}
        Ok(ImportFinish::Stale) => {
            return failure(format!(
                "{name} import failed: attempt {import_id}:{generation} was superseded\n"
            ));
        }
        Err(error) => return failure(format!("{name} import failed: {error}\n")),
    }
    // The terminal record is the authority on whether publication held, not the write.
    let projection = solstone_core_import::project_import_result(journal, import_id);
    if projection.status != solstone_core_import::ProjectionStatus::Success {
        return failure(format!(
            "{name} import saved to your journal, but it could not confirm the entries are ready to search; run it again\n"
        ));
    }
    success(cli_render::source_import_complete(
        source,
        &ImportResult {
            entries_written: written.entries,
            entities_seeded: 0,
            files_created: written
                .created
                .iter()
                .map(|file| file.path.to_string_lossy().into_owned())
                .collect(),
            errors: Vec::new(),
            summary: written.summary,
            hard_failures: Vec::new(),
            segments: None,
            date_range: projection.date_range,
            merge_summary: None,
            principal_collision: None,
            merge_log_path: None,
            merge_staging_path: None,
            raw_retention: None,
        },
    ))
}

fn write_save_manifest(
    journal: &Path,
    import_id: &str,
    rendered: &save::RenderedImport,
    entry_count: u64,
    created: &[solstone_core_import::text::TextCreated],
    source_hash: &solstone_core_import::SourceHash,
) -> Result<(), String> {
    let mut days_affected = created
        .iter()
        .map(|file| file.day.clone())
        .collect::<Vec<_>>();
    days_affected.sort();
    days_affected.dedup();
    let files_created = created
        .iter()
        .map(|file| file.path.display().to_string())
        .collect::<Vec<_>>();
    solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
        journal_root: journal,
        import_id,
        source_type: rendered.source.name(),
        source_hash,
        entry_count,
        days_affected: &days_affected,
        files_created: &files_created,
        imported_via: "native",
        link_id: None,
        observer_handle: None,
        raw_retention: None,
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn cli_producer_request<'a>(
    journal: &'a Path,
    dispatch: &'a RegistryDispatch,
    generation: u64,
) -> solstone_core_import_sources::NativeProducerRequest<'a> {
    solstone_core_import_sources::NativeProducerRequest {
        journal_root: journal,
        source_path: &dispatch.media,
        import_id: &dispatch.timestamp,
        source: dispatch.source,
        revision: None,
        password: None,
        force: dispatch.force,
        expected_generation: Some(generation),
        heartbeat_interval: None,
    }
}

fn run_document(
    mut dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
) -> CliRun {
    #[cfg(windows)]
    let worker = match document::WindowsPdfWorker::from_verified_package(PDF_WORKER_TIMEOUT) {
        Ok(worker) => worker,
        Err(error) => return failure(format!("{error}\n")),
    };
    #[cfg(not(windows))]
    let worker_path = match pdf_worker_sibling() {
        Ok(path) => path,
        Err(error) => return failure(format!("{error}\n")),
    };
    #[cfg(not(windows))]
    let worker = document::SystemPdfWorker::new(worker_path, PDF_WORKER_TIMEOUT);
    if dispatch.dry_run {
        let preview = document::preview(
            document::DocumentPreviewRequest {
                source: &dispatch.media,
                password: None,
                now: SystemTime::now(),
                zone: solstone_core_journal_config::owner_zone(journal),
            },
            &worker,
        );
        return success(cli_render::source_preview(dispatch.source, &preview));
    }
    let model_client = match solstone_core_generate::OneShotClient::sibling() {
        Ok(client) => client,
        Err(error) => {
            return failure(format!(
                "{} import failed: {error}\n",
                dispatch.source.name()
            ));
        }
    };
    let model = document::SystemDocumentModelClient::new(model_client);
    if let Err(error) = claim_dispatch_id(journal, &mut dispatch, selected) {
        return failure(format!(
            "{} import failed: {error}\n",
            dispatch.source.name()
        ));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, dispatch.source) {
        return refused;
    }
    let started_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let facts = match solstone_core_import::admit_running_attempt(
        journal,
        &dispatch.timestamp,
        started_at_ms,
        Some("document"),
    ) {
        Ok(f) => f,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let publication = NativePublicationOperations;
    let req = cli_producer_request(journal, &dispatch, facts.generation);
    let outcome = match solstone_core_import_sources::run_native_producer(
        req,
        &solstone_core_import_sources::NullWireClient,
        &worker,
        &model,
        &publication,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            model.finish();
            return failure(format!(
                "{} import failed: {error}\n",
                dispatch.source.name()
            ));
        }
    };
    model.finish();
    let proj = solstone_core_import::project_import_result(journal, &dispatch.timestamp);
    let mut errors = outcome.errors;
    if let Some(err) = proj.error
        && !errors.contains(&err)
    {
        errors.push(err);
    }
    let result = ImportResult {
        entries_written: outcome.entries_written,
        entities_seeded: 0,
        files_created: outcome
            .files_created
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        errors,
        summary: format!("imported {} PDF documents", outcome.entries_written),
        hard_failures: Vec::new(),
        segments: None,
        date_range: proj.date_range.clone(),
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    };
    render_result(dispatch.source, result)
}

fn run_image(
    mut dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
) -> CliRun {
    if dispatch.dry_run {
        return success(cli_render::source_preview(
            dispatch.source,
            &image::preview(
                &dispatch.media,
                solstone_core_journal_config::owner_zone(journal),
            ),
        ));
    }
    if let Err(error) = claim_dispatch_id(journal, &mut dispatch, selected) {
        return failure(format!(
            "{} import failed: {error}\n",
            dispatch.source.name()
        ));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, dispatch.source) {
        return refused;
    }
    let started_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let facts = match solstone_core_import::admit_running_attempt(
        journal,
        &dispatch.timestamp,
        started_at_ms,
        Some("image"),
    ) {
        Ok(f) => f,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let wire = image::SystemWireClient;
    let req = cli_producer_request(journal, &dispatch, facts.generation);
    let outcome = match solstone_core_import_sources::run_native_producer(
        req,
        &wire,
        &solstone_core_import_sources::NullPdfWorker,
        &solstone_core_import_sources::NullDocumentModelClient,
        &NativePublicationOperations,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            return failure(format!(
                "{} import failed: {error}\n",
                dispatch.source.name()
            ));
        }
    };
    let proj = solstone_core_import::project_import_result(journal, &dispatch.timestamp);
    let mut errors = outcome.errors;
    if let Some(err) = proj.error
        && !errors.contains(&err)
    {
        errors.push(err);
    }
    let result = ImportResult {
        entries_written: outcome.entries_written,
        entities_seeded: 0,
        files_created: outcome
            .files_created
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        errors,
        summary: "imported 1 image".to_owned(),
        hard_failures: Vec::new(),
        segments: None,
        date_range: proj.date_range.clone(),
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: None,
    };
    render_result(dispatch.source, result)
}

pub type RecordCompletedAttemptFn<'a> = dyn Fn(&Path, &str, u64, u64, Option<u64>, Option<String>) -> Result<AttemptFacts, ImportError>
    + 'a;

pub type RecordUnconfirmedAttemptFn<'a> =
    dyn Fn(&Path, &str, u64, u64, Option<String>) -> Result<AttemptFacts, ImportError> + 'a;

pub struct ArchiveTerminalSeams<'a> {
    pub after_admit: Option<&'a (dyn Fn() + 'a)>,
    pub record_completed: &'a RecordCompletedAttemptFn<'a>,
    pub record_unconfirmed: &'a RecordUnconfirmedAttemptFn<'a>,
}

const DEFAULT_ARCHIVE_TERMINAL_SEAMS: ArchiveTerminalSeams<'static> = ArchiveTerminalSeams {
    after_admit: None,
    record_completed: &solstone_core_import::record_completed_attempt_unlocked,
    record_unconfirmed: &solstone_core_import::record_unconfirmed_attempt_unlocked,
};

fn run_archive(
    dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
) -> CliRun {
    run_archive_with_seams(dispatch, journal, selected, &DEFAULT_ARCHIVE_TERMINAL_SEAMS)
}

fn run_archive_with_seams(
    mut dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
    seams: &ArchiveTerminalSeams<'_>,
) -> CliRun {
    if dispatch.dry_run {
        return match plan_journal_archive(&dispatch.media) {
            Ok(plan) => success(cli_render::source_preview(dispatch.source, &plan.into())),
            Err(error) => failure(format!(
                "{} preview failed: {error}\n",
                dispatch.source.name()
            )),
        };
    }
    let options = ArchiveMergeOptions {
        working_root: solstone_core_import_sources::archive::archive_merge_working_root(journal),
        ..ArchiveMergeOptions::default()
    };
    if let Err(error) = validate_archive_preflight(&dispatch.media, &options) {
        return archive_failure(dispatch.source, error);
    }
    if let Err(error) = claim_dispatch_id(journal, &mut dispatch, selected) {
        return failure(format!(
            "{} import failed: {error}\n",
            dispatch.source.name()
        ));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, dispatch.source) {
        return refused;
    }
    let started_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let facts = match solstone_core_import::admit_running_attempt(
        journal,
        &dispatch.timestamp,
        started_at_ms,
        Some("journal_archive"),
    ) {
        Ok(f) => f,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    if let Some(hook) = seams.after_admit {
        hook();
    }
    let _import_lock = match solstone_core_import::hold_import_lock(journal, &dispatch.timestamp) {
        Ok(lock) => lock,
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let provenance = match solstone_core_import::read_provenance(journal, &dispatch.timestamp) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return failure(format!(
                "{} import failed: missing provenance for attempt {}\n",
                dispatch.source.name(),
                dispatch.timestamp
            ));
        }
        Err(err) => {
            return failure(format!("{} import failed: {err}\n", dispatch.source.name()));
        }
    };
    let current_facts = match solstone_core_import::get_attempt_facts(&provenance) {
        Some(f) => f,
        None => {
            return failure(format!(
                "{} import failed: missing attempt facts for {}\n",
                dispatch.source.name(),
                dispatch.timestamp
            ));
        }
    };
    if current_facts.generation != facts.generation || current_facts.state != AttemptState::Running
    {
        return failure(format!(
            "{} import failed: attempt {}:{} was superseded\n",
            dispatch.source.name(),
            dispatch.timestamp,
            facts.generation
        ));
    }
    let merge_result = merge_journal_archive(
        &dispatch.media,
        journal,
        &options,
        Some(&SupervisorRescan {
            journal: journal.to_path_buf(),
        }),
    );
    let finished_at_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let duration_ms = Some(finished_at_ms.saturating_sub(facts.started_at_ms));
    if let Ok(outcome) = &merge_result
        && let Err(err) = solstone_core_import::record_import_results_unlocked(
            journal,
            &dispatch.timestamp,
            archive_merge_results(outcome, &facts.attempt_id),
        )
    {
        return failure(format!(
            "{} import failed: failed to record merge results: {err}\n",
            dispatch.source.name()
        ));
    }
    match merge_result {
        Ok(outcome) => match outcome.retry_disposition {
            RetryDisposition::Applied => {
                let rec = (seams.record_completed)(
                    journal,
                    &dispatch.timestamp,
                    facts.generation,
                    finished_at_ms,
                    duration_ms,
                    None,
                );
                if let Err(err) = rec {
                    return failure(format!(
                        "{} import failed: failed to record completed attempt: {err}\n",
                        dispatch.source.name()
                    ));
                }
                success(cli_render::source_archive_merge_complete(
                    dispatch.source,
                    outcome.merge_summary.segments_copied,
                    outcome.merge_summary.imports_copied,
                    outcome.merge_summary.entities_created,
                    outcome.merge_summary.entities_merged,
                    outcome.merge_summary.facets_created,
                    outcome.merge_summary.facets_merged,
                ))
            }
            RetryDisposition::IdempotentNoop => {
                let rec = (seams.record_completed)(
                    journal,
                    &dispatch.timestamp,
                    facts.generation,
                    finished_at_ms,
                    duration_ms,
                    None,
                );
                if let Err(err) = rec {
                    return failure(format!(
                        "{} import failed: failed to record completed attempt: {err}\n",
                        dispatch.source.name()
                    ));
                }
                success(cli_render::source_archive_already_present(dispatch.source))
            }
            RetryDisposition::Incomplete => {
                let rec = (seams.record_unconfirmed)(
                    journal,
                    &dispatch.timestamp,
                    facts.generation,
                    finished_at_ms,
                    Some(solstone_core_import::IMPORT_UNCONFIRMED_REASON.to_owned()),
                );
                if let Err(err) = rec {
                    return failure(format!(
                        "{} import failed: failed to record attempt: {err}\n",
                        dispatch.source.name()
                    ));
                }
                failure(cli_render::source_archive_incomplete(
                    dispatch.source,
                    &archive_incomplete_detail(&outcome),
                ))
            }
        },
        Err(error) => {
            let reason = match error.mutation_state() {
                MergeMutationState::NotMutated => solstone_core_import::IMPORT_FAILED_REASON,
                MergeMutationState::MayHaveMutated | MergeMutationState::Unknown => {
                    solstone_core_import::IMPORT_UNCONFIRMED_REASON
                }
            };
            let rec = (seams.record_unconfirmed)(
                journal,
                &dispatch.timestamp,
                facts.generation,
                finished_at_ms,
                Some(reason.to_owned()),
            );
            if let Err(err) = rec {
                return failure(format!(
                    "{} import failed: {error}; failed to record attempt: {err}\n",
                    dispatch.source.name()
                ));
            }
            archive_failure(dispatch.source, error)
        }
    }
}

/// What the import detail shows for a merge: its counts, what each part of the archive
/// did, and whether the archive names a different owner. Every run replaces the last.
fn archive_merge_results(
    outcome: &ArchiveMergeResult,
    attempt_id: &str,
) -> Map<String, serde_json::Value> {
    let staged_entities = outcome
        .entity_dispositions
        .iter()
        .filter(|d| d.disposition.is_set_aside())
        .map(|d| solstone_core_import::StagedEntityRecord {
            source_id: d.source_id.clone(),
            source_name: d.source_name.clone(),
            staging_path: d
                .staging_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        })
        .collect::<Vec<_>>();
    let staged_entities =
        (!staged_entities.is_empty()).then(|| solstone_core_import::StagedEntityList {
            attempt_id: attempt_id.to_owned(),
            entities: staged_entities,
        });
    solstone_core_import::journal_archive_result_metadata(
        outcome.entries_written,
        outcome.entities_seeded,
        serde_json::to_value(&outcome.merge_summary).unwrap_or(serde_json::Value::Null),
        outcome
            .principal_collision
            .as_ref()
            .and_then(|p| serde_json::to_value(p).ok()),
        outcome.decision_log_path.to_string_lossy().into_owned(),
        outcome.staging_path.to_string_lossy().into_owned(),
        &outcome.errors,
        staged_entities,
    )
}

fn archive_incomplete_detail(outcome: &ArchiveMergeResult) -> String {
    if !outcome.errors.is_empty() {
        return outcome.errors.join("; ");
    }
    if let ReindexStatus::NotAccepted { detail } = &outcome.reindex_status {
        return detail.clone();
    }
    format!(
        "segments_skipped={} segments_errored={} entities_staged={}",
        outcome.merge_summary.segments_skipped,
        outcome.merge_summary.segments_errored,
        outcome.merge_summary.entities_staged,
    )
}

/// Thin wrapper over the shared guard in `solstone-core-import`, which the generic audio and
/// text producers also use. One guard, so the two can never disagree about liveness.
fn refuse_if_live_running(
    journal: &Path,
    import_id: &str,
    source: RegistrySource,
) -> Option<CliRun> {
    solstone_core_import::refuse_if_live_running(journal, import_id, source.name())
        .map(|message| failure(format!("{message}\n")))
}

fn render_result(source: RegistrySource, result: ImportResult) -> CliRun {
    if result.hard_failures.is_empty() && (result.entries_written > 0 || result.errors.is_empty()) {
        success(cli_render::source_import_complete(source, &result))
    } else {
        failure(cli_render::source_import_failure(source, &result))
    }
}

fn archive_failure(source: RegistrySource, error: ImportSourcesError) -> CliRun {
    failure(format!("{} import failed: {error}\n", source.name()))
}

fn pdf_worker_sibling() -> Result<PathBuf, String> {
    let current = env::current_exe().map_err(|error| error.to_string())?;
    let parent = current
        .parent()
        .ok_or_else(|| "current executable has no parent".to_owned())?;
    let path = parent.join(format!("solstone-core-pdf{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("missing sibling executable {}", path.display()))
    }
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stderr: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

const STRAVA_RAW_RETENTION: &str = "discard";

pub(crate) fn run_strava(
    dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
) -> CliRun {
    let media = dispatch.media.clone();
    let dry_run = dispatch.dry_run;
    let run = run_strava_internal(dispatch, journal, selected, None);
    if !dry_run {
        discard_uploaded_download(journal, Path::new(&media));
    }
    run
}

/// A Strava download uploaded through the import page is staged under the
/// journal's `imports/`. Only its workout list is ever read, and the download
/// itself is never kept: once the run ends, successfully or not, the staged copy
/// is removed. A file the owner pointed the command at anywhere else is left
/// alone.
fn discard_uploaded_download(journal: &Path, media: &Path) {
    let (Ok(imports), Ok(media)) = (journal.join("imports").canonicalize(), media.canonicalize())
    else {
        return;
    };
    if media.starts_with(&imports) && media.is_file() {
        let _ = std::fs::remove_file(&media);
    }
}

#[cfg(test)]
pub struct StravaRunHook<'a> {
    pub fault: Option<&'a strava::ReadFault>,
    pub after_lock: Option<&'a (dyn Fn(&Path) + 'a)>,
    pub before_apply: Option<&'a (dyn Fn(&Path) + 'a)>,
    pub lock_timeout: Option<std::time::Duration>,
    pub stop_before_finish: bool,
}

#[cfg(test)]
pub fn run_strava_hooked(
    dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
    hook: Option<&StravaRunHook<'_>>,
) -> CliRun {
    run_strava_internal(dispatch, journal, selected, hook)
}

fn run_strava_internal(
    mut dispatch: RegistryDispatch,
    journal: &Path,
    selected: &mut Option<String>,
    #[cfg(test)] hook: Option<&StravaRunHook<'_>>,
    #[cfg(not(test))] _hook: Option<()>,
) -> CliRun {
    let source = dispatch.source;
    let name = source.name();
    let action_label = if dispatch.dry_run {
        "preview"
    } else {
        "import"
    };

    // 1. read_zone_record
    let pre_lock_zone = match strava::read_zone_record(journal) {
        Ok(z) => z,
        Err(e) => return failure(format!("{name} {action_label} failed: {e}\n")),
    };

    // 2. read_workouts at DEFAULT_LIST_CAP
    let workouts = match strava::read_workouts(&dispatch.media, strava::DEFAULT_LIST_CAP) {
        Ok(w) => w,
        Err(e) => return failure(format!("{name} {action_label} failed: {e}\n")),
    };

    #[cfg(test)]
    let fault = hook.and_then(|h| h.fault);
    #[cfg(not(test))]
    let fault = None;

    // 3. place as fail-fast
    let pre_placement = match strava::place(journal, &workouts.workouts, pre_lock_zone, fault) {
        Ok(p) => p,
        Err(e) => return failure(format!("{name} {action_label} failed: {e}\n")),
    };

    // 4. dry_run
    if dispatch.dry_run {
        let mut summary = strava::format_count_summary(&pre_placement.counts);
        let owner_z = solstone_core_journal_config::owner_zone(journal);
        if pre_placement.zone.name() != owner_z.name() {
            summary.push_str(&format!(" zone={}", pre_placement.zone.name()));
        }
        let date_range = match (&pre_placement.first_day, &pre_placement.last_day) {
            (Some(f), Some(l)) => (f.clone(), l.clone()),
            _ => (String::new(), String::new()),
        };
        let preview = solstone_core_import::ImportPreview {
            date_range,
            item_count: pre_placement.counts.new_workouts as u64,
            entity_count: pre_placement.counts.tiles_to_create as u64,
            summary,
        };
        return success(cli_render::source_preview(source, &preview));
    }

    // 5. Ignore dispatch.force. No same-file skip.

    // 6. hold_lock
    #[cfg(test)]
    let lock_timeout = hook
        .and_then(|h| h.lock_timeout)
        .unwrap_or(DEFAULT_LOCK_TIMEOUT);
    #[cfg(not(test))]
    let lock_timeout = DEFAULT_LOCK_TIMEOUT;

    let _source_lock = match hold_lock(
        journal.join("imports/.strava"),
        LockOptions {
            timeout: lock_timeout,
            mode: Some(0o600),
            ..LockOptions::default()
        },
    ) {
        Ok(lock) => lock,
        Err(_) => return failure(format!("{name} import failed: strava_lock_unavailable\n")),
    };

    #[cfg(test)]
    if let Some(after_lock) = hook.and_then(|h| h.after_lock) {
        after_lock(journal);
    }

    // 7. Re-read the zone record & place again
    let post_lock_zone = match strava::read_zone_record(journal) {
        Ok(z) => z,
        Err(_) => return failure(format!("{name} import failed: strava_zone_unrecognised\n")),
    };
    let placement = match strava::place(journal, &workouts.workouts, post_lock_zone, fault) {
        Ok(p) => p,
        Err(e) => return failure(format!("{name} import failed: {e}\n")),
    };

    // 8. claim_dispatch_id, refuse_if_live_running, admit_running_attempt
    if let Err(error) = claim_dispatch_id(journal, &mut dispatch, selected) {
        return failure(format!("{name} import failed: {error}\n"));
    }
    if let Some(refused) = refuse_if_live_running(journal, &dispatch.timestamp, source) {
        return refused;
    }

    let import_id = dispatch.timestamp.as_str();
    let generation = match solstone_core_import::admit_running_attempt(
        journal,
        import_id,
        now_ms(),
        Some("strava"),
    ) {
        Ok(facts) => facts.generation,
        Err(error) => return failure(format!("{name} import failed: {error}\n")),
    };

    let finish = |input| finish_import_attempt(journal, import_id, generation, name, input);

    // 9. create_zone_record if absent
    if post_lock_zone.is_none()
        && let Err(error) = strava::create_zone_record(journal, placement.zone)
    {
        let _ = finish(ImportTerminalInput::Failed(&[]));
        return failure(format!("{name} import failed: {error}\n"));
    }

    // 10. before_apply callback
    #[cfg(test)]
    if let Some(before_apply) = hook.and_then(|h| h.before_apply) {
        before_apply(journal);
    }

    // 11. apply, then write_rendered
    let (rendered, _) = strava::apply(&placement, import_id);
    let written = save::write_rendered(journal, Some(import_id), &rendered);

    // 12. Write error / reconciliation
    let written_keys: HashSet<(String, String)> = written
        .created
        .iter()
        .map(|file| (file.day.clone(), file.segment.clone()))
        .collect();
    let skipped_keys: HashSet<(String, String)> = written.skipped_deleted.iter().cloned().collect();
    let (corrected_counts, planned_publish) = strava::reconcile_written(
        &placement,
        &written_keys,
        &skipped_keys,
        written.error.is_some(),
    );

    let mut publish_slice: Vec<solstone_core_import::text::TextCreated> = Vec::new();
    for p in planned_publish {
        if let Some(c) = written
            .created
            .iter()
            .find(|c| c.day == p.day && c.segment == p.segment)
        {
            publish_slice.push(c.clone());
        } else {
            publish_slice.push(solstone_core_import::text::TextCreated {
                day: p.day.clone(),
                segment: p.segment.clone(),
                stream: "import.strava".to_owned(),
                hints: save::stream_hints(RegistrySource::Strava),
                path: PathBuf::from(format!(
                    "chronicle/{}/import.strava/{}/workout.json",
                    p.day, p.segment
                )),
            });
        }
    }

    if let Some(save_error) = written.error {
        let _ = finish(ImportTerminalInput::Failed(&publish_slice));
        let count_line = strava::format_count_summary(&corrected_counts);
        return failure(format!(
            "{name} import failed: {save_error}\n{count_line}\n"
        ));
    }

    // 13. stop_before_finish
    #[cfg(test)]
    if hook.map(|h| h.stop_before_finish).unwrap_or(false) {
        return success(String::new());
    }

    // 14. No write error: manifest
    let source_hash = match solstone_core_import::hash_source(&dispatch.media) {
        Ok(hash) => hash,
        Err(error) => {
            let _ = finish(ImportTerminalInput::Failed(&[]));
            return failure(format!("{name} import failed: {error}\n"));
        }
    };

    let mut days_affected: Vec<String> = placement
        .tiles
        .iter()
        .filter_map(|t| match t {
            strava::TileAction::Created { day, segment, .. } => {
                let key = (day.clone(), segment.clone());
                if written_keys.contains(&key) && !skipped_keys.contains(&key) {
                    Some(day.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    days_affected.sort();
    days_affected.dedup();

    let manifest_res =
        solstone_core_import::write_manifest(&solstone_core_import::ManifestWriteRequest {
            journal_root: journal,
            import_id,
            source_type: "strava",
            source_hash: &source_hash,
            entry_count: corrected_counts.new_workouts as u64,
            days_affected: &days_affected,
            files_created: &[],
            imported_via: "native",
            link_id: None,
            observer_handle: None,
            raw_retention: Some(STRAVA_RAW_RETENTION),
        });
    if let Err(error) = manifest_res {
        let _ = finish(ImportTerminalInput::Failed(&publish_slice));
        return failure(format!("{name} import failed: {error}\n"));
    }

    // 15. finish Success
    match finish(ImportTerminalInput::Success(&publish_slice)) {
        Ok(ImportFinish::Applied) => {}
        Ok(ImportFinish::Stale) => {
            return failure(format!(
                "{name} import failed: attempt {import_id}:{generation} was superseded\n"
            ));
        }
        Err(error) => return failure(format!("{name} import failed: {error}\n")),
    }

    let projection = solstone_core_import::project_import_result(journal, import_id);
    if projection.status != solstone_core_import::ProjectionStatus::Success {
        return failure(format!(
            "{name} import saved to your journal, but it could not confirm the entries are ready to search; run it again\n"
        ));
    }

    let mut summary = strava::format_count_summary(&corrected_counts);
    let owner_z = solstone_core_journal_config::owner_zone(journal);
    if placement.zone.name() != owner_z.name() {
        summary.push_str(&format!(" zone={}", placement.zone.name()));
    }

    let result = ImportResult {
        entries_written: corrected_counts.new_workouts as u64,
        entities_seeded: 0,
        files_created: vec![],
        errors: Vec::new(),
        summary,
        hard_failures: Vec::new(),
        segments: None,
        date_range: projection.date_range,
        merge_summary: None,
        principal_collision: None,
        merge_log_path: None,
        merge_staging_path: None,
        raw_retention: Some(STRAVA_RAW_RETENTION.to_owned()),
    };
    render_result(source, result)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

    use super::*;

    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn image_dispatch(image_path: &Path, timestamp: &str) -> RegistryDispatch {
        RegistryDispatch {
            source: RegistrySource::Image,
            media: image_path.to_path_buf(),
            timestamp: timestamp.to_owned(),
            dry_run: false,
            force: false,
        }
    }

    fn image_time(img: &Path) -> (String, String) {
        let modified = img.metadata().unwrap().modified().unwrap();
        let dt: chrono::DateTime<chrono::Local> = modified.into();
        (
            dt.format("%Y%m%d").to_string(),
            format!("{}_0", dt.format("%H%M%S")),
        )
    }

    #[test]
    fn image_publication_advances_stream_and_dirties_the_day_before_success() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        let dispatch = image_dispatch(&img, "20260809_090000");

        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("imported 1 image"));
        assert!(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("stream.json")
                .is_file()
        );
        assert!(matches!(
            read_health_marker(journal.path(), &day, HealthMarkerKind::Stream).unwrap(),
            // Touched when the original is installed and again by publication.
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 2
        ));
    }

    #[test]
    fn image_stream_publication_failure_is_terminal_and_recorded() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("stream.json"),
        )
        .unwrap();
        fs::write(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("original.png"),
            TINY_PNG,
        )
        .unwrap();

        let dispatch = image_dispatch(&img, "20260809_090000");
        let run = run(dispatch, journal.path());

        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("image import failed:"));
        assert!(run.stderr.contains("publication failed"), "{}", run.stderr);
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.path().join("imports/20260809_090000/imported.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(record["status"], "failure");
        assert_eq!(
            record["segments"][0]["outcome"]["status"],
            "failed_at_marker_write"
        );
    }

    #[test]
    fn image_day_marker_blocked_at_install_is_terminal_and_the_original_stays() {
        let journal = tempfile::tempdir().unwrap();
        let img = journal.path().join("sample.png");
        fs::write(&img, TINY_PNG).unwrap();
        let (day, seg) = image_time(&img);
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("health/stream.updated"),
        )
        .unwrap();

        let dispatch = image_dispatch(&img, "20260809_090000");
        let run = run(dispatch, journal.path());

        // The marker is touched as soon as the original is installed, so a blocked marker
        // stops the import there: typed, terminal, and the owner's original is retained.
        // (A marker that fails later, during publication, is covered by the producer test
        // `a_day_marker_that_fails_at_publication_is_terminal_after_the_content_is_installed`.)
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("image import failed:"),
            "{}",
            run.stderr
        );
        assert!(run.stderr.contains("remains installed"), "{}", run.stderr);
        assert!(
            journal
                .path()
                .join("chronicle")
                .join(&day)
                .join("import.image")
                .join(&seg)
                .join("original.png")
                .is_file()
        );
    }

    #[test]
    fn image_publication_lifecycle_success() {
        let journal = tempfile::tempdir().unwrap();
        let image_path = journal.path().join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();

        let dispatch = image_dispatch(&image_path, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn a_finished_import_is_recorded_once_in_awareness() {
        let journal = tempfile::tempdir().unwrap();
        let image_path = journal.path().join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();
        let run = run(
            image_dispatch(&image_path, "20260809_090000"),
            journal.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-09T19:30:00-06:00").unwrap();

        // An import that finished before this invocation began is not recorded.
        super::record_finished_import_at(journal.path(), "20260809_090000", u64::MAX, now).unwrap();
        let imports = solstone_core_facets::load_imports(journal.path()).unwrap();
        assert_ne!(imports["has_imported"], true, "{imports}");

        super::record_finished_import_at(journal.path(), "20260809_090000", 0, now).unwrap();
        let imports = solstone_core_facets::load_imports(journal.path()).unwrap();
        assert_eq!(imports["has_imported"], true);
        assert_eq!(imports["import_count"], 1);
        assert_eq!(imports["last_completed"], "20260809T19:30:00");
        assert!(
            imports["last_result_summary"]
                .as_str()
                .is_some_and(|summary| summary.ends_with("Image")),
            "{imports}"
        );
        assert!(journal.path().join("awareness/20260809.jsonl").is_file());
    }

    #[test]
    fn image_cli_refuses_a_live_running_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let image_path = journal.path().join("test.png");
        fs::write(&image_path, TINY_PNG).unwrap();
        let requested = solstone_core_import::validate_timestamp("20260809_090000").unwrap();
        let bound =
            solstone_core_import::bind_import_record(journal.path(), &requested, Some(&image_path))
                .unwrap();
        assert_eq!(bound.import_id, requested);
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let facts = solstone_core_import::admit_running_attempt(
            journal.path(),
            "20260809_090000",
            now_ms,
            Some("image"),
        )
        .unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Running);

        let dispatch = image_dispatch(&image_path, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stderr.contains("image import failed:"));
        assert!(run.stderr.contains("already running"), "{}", run.stderr);

        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let after = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(after.generation, 1);
        assert_eq!(after.state, solstone_core_import::AttemptState::Running);
    }

    const OTHER_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 96, 96, 248, 255, 31,
        0, 3, 2, 1, 255, 230, 119, 11, 174, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    fn write_utc_zone(journal: &Path) {
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            br#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    fn set_mtime(path: &Path, modified: std::time::SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn chronicle_originals(journal: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let chronicle = journal.join("chronicle");
        if !chronicle.exists() {
            return found;
        }
        fn walk(dir: &Path, found: &mut Vec<(PathBuf, Vec<u8>)>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.file_name().and_then(|name| name.to_str()) == Some("original.png") {
                    found.push((path.clone(), fs::read(&path).unwrap()));
                }
            }
        }
        walk(&chronicle, &mut found);
        found.sort_by(|left, right| left.0.cmp(&right.0));
        found
    }

    #[test]
    fn two_same_time_images_keep_separate_records_and_the_source_day() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let modified = std::time::SystemTime::from(
            chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 6, 16, 12, 0, 0).unwrap(),
        );
        let first = journal.path().join("first.png");
        let second = journal.path().join("second.png");
        fs::write(&first, TINY_PNG).unwrap();
        fs::write(&second, OTHER_PNG).unwrap();
        set_mtime(&first, modified);
        set_mtime(&second, modified);
        let stamp = "20260616_235959";

        let (first_run, first_id) = run_bound(image_dispatch(&first, stamp), journal.path());
        assert_eq!(first_run.exit_code, 0, "{}", first_run.stderr);
        assert_eq!(first_id.as_deref(), Some(stamp));
        let (second_run, second_id) = run_bound(image_dispatch(&second, stamp), journal.path());
        assert_eq!(second_run.exit_code, 0, "{}", second_run.stderr);
        assert_eq!(second_id.as_deref(), Some("20260617_000000"));

        for id in [stamp, "20260617_000000"] {
            let projection = solstone_core_import::project_import_result(journal.path(), id);
            assert_eq!(
                projection.status,
                solstone_core_import::ProjectionStatus::Success,
                "{id} {projection:?}"
            );
            let metadata = solstone_core_import::read_import_metadata(journal.path(), id).unwrap();
            assert_eq!(metadata["source_timestamp"], stamp);
            assert_eq!(metadata["import_id"], id);
        }
        let photos = chronicle_originals(journal.path());
        assert_eq!(photos.len(), 2, "{photos:?}");
        assert_eq!(photos[0].1, TINY_PNG);
        assert_eq!(photos[1].1, OTHER_PNG);
        assert!(
            photos[0]
                .0
                .ends_with("chronicle/20260616/import.image/120000_0/original.png"),
            "{}",
            photos[0].0.display()
        );
        assert!(
            photos[1]
                .0
                .ends_with("chronicle/20260616/import.image/120001_0/original.png"),
            "{}",
            photos[1].0.display()
        );
        assert!(!journal.path().join("chronicle/20260617").exists());

        let (again, again_id) = run_bound(image_dispatch(&first, stamp), journal.path());
        assert_eq!(again.exit_code, 0, "{}", again.stderr);
        assert_eq!(again_id.as_deref(), Some(stamp));
        assert_eq!(chronicle_originals(journal.path())[0].1, TINY_PNG);
        assert_eq!(
            solstone_core_import::read_import_metadata(journal.path(), stamp).unwrap()["source_hash"],
            solstone_core_import::hash_source(&first).unwrap().as_str()
        );
        assert!(!journal.path().join("imports/20260617_000001").exists());

        let preview_path = journal.path().join("preview.png");
        fs::write(&preview_path, OTHER_PNG).unwrap();
        let mut preview = image_dispatch(&preview_path, stamp);
        preview.dry_run = true;
        let (preview_run, preview_id) = run_bound(preview, journal.path());
        assert_eq!(preview_run.exit_code, 0, "{}", preview_run.stderr);
        assert!(preview_id.is_none());
        assert!(!journal.path().join("imports/20260617_000001").exists());
    }

    fn write_test_archive(dir: &Path, name: &str, members: &[(&str, &[u8])]) -> PathBuf {
        let archive = dir.join(name);
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in members {
            use std::io::Write;
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
        archive
    }

    fn archive_dispatch(archive_path: &Path, timestamp: &str) -> RegistryDispatch {
        RegistryDispatch {
            source: RegistrySource::JournalArchive,
            media: archive_path.to_path_buf(),
            timestamp: timestamp.to_owned(),
            dry_run: false,
            force: false,
        }
    }

    fn journal_family_snapshot(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut snapshot = std::collections::BTreeMap::new();
        for family in ["chronicle", "entities", "facets", "imports"] {
            let path = root.join(family);
            if !path.exists() {
                continue;
            }
            fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.filter_map(Result::ok) {
                        let path = entry.path();
                        if path.is_dir() {
                            walk(&path, files);
                        } else if path.is_file() {
                            files.push(path);
                        }
                    }
                }
            }
            let mut files = Vec::new();
            walk(&path, &mut files);
            for entry in files {
                let relative = entry
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                snapshot.insert(relative, fs::read(entry).unwrap());
            }
        }
        snapshot
    }

    #[test]
    fn archive_dry_run_previews_without_admission() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );
        let mut dispatch = archive_dispatch(&archive, "20260809_090000");
        dispatch.dry_run = true;
        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("preview:"));

        // Confirm no attempt was admitted
        let meta =
            solstone_core_import::read_provenance(journal.path(), "20260809_090000").unwrap();
        assert!(meta.is_none());
    }

    #[test]
    fn archive_invalid_fails_preflight_without_admission() {
        let journal = tempfile::tempdir().unwrap();
        let invalid_archive = journal.path().join("invalid.zip");
        fs::write(&invalid_archive, b"not a zip").unwrap();

        let dispatch = archive_dispatch(&invalid_archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("journal_archive import failed:"));

        // Confirm no attempt was admitted
        let meta =
            solstone_core_import::read_provenance(journal.path(), "20260809_090000").unwrap();
        assert!(meta.is_none());
    }

    #[test]
    fn archive_success_records_completed_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[
                (
                    "chronicle/20260809/090000_10/stream.json",
                    b"foreign stream data",
                ),
                (
                    "entities/person/entity.json",
                    br#"{"id":"person","name":"Person","type":"Person"}"#,
                ),
            ],
        );
        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("import complete"), "{}", run.stdout);

        // Confirm completed attempt facts are recorded
        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Completed);
        assert!(facts.finished_at_ms.is_some());
        assert!(facts.failure_reason.is_none());

        // Confirm source_hint is journal_archive and recognized by RegistrySource
        assert_eq!(
            meta.get("source_hint"),
            Some(&serde_json::Value::String("journal_archive".into()))
        );
        assert_eq!(
            RegistrySource::from_name("journal_archive"),
            Some(RegistrySource::JournalArchive)
        );

        // Confirm no imported.json was created
        assert!(
            !journal
                .path()
                .join("imports/20260809_090000/imported.json")
                .exists()
        );

        // Confirm foreign stream.json was copied with exact bytes
        assert_eq!(
            fs::read(
                journal
                    .path()
                    .join("chronicle/20260809/090000_10/stream.json")
            )
            .unwrap(),
            b"foreign stream data"
        );

        // Confirm projection projects Success
        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Success);
        assert_eq!(proj.source_type, "journal_archive");
    }

    #[test]
    fn archive_idempotent_second_run_is_completed_success() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );
        let dispatch1 = archive_dispatch(&archive, "20260809_090000");
        let run1 = run(dispatch1, journal.path());
        assert_eq!(run1.exit_code, 0, "{}", run1.stderr);

        // Second run on same journal
        let dispatch2 = archive_dispatch(&archive, "20260809_090001");
        let run2 = run(dispatch2, journal.path());
        assert_eq!(run2.exit_code, 0, "{}", run2.stderr);
        assert!(
            run2.stdout.to_lowercase().contains("already present"),
            "{}",
            run2.stdout
        );

        let meta2 =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090001").unwrap();
        let facts2 = solstone_core_import::get_attempt_facts(&meta2).unwrap();
        assert_eq!(facts2.generation, 1);
        assert_eq!(facts2.state, solstone_core_import::AttemptState::Completed);

        let proj2 = solstone_core_import::project_import_result(journal.path(), "20260809_090001");
        assert_eq!(
            proj2.status,
            solstone_core_import::ProjectionStatus::Success
        );
    }

    #[test]
    fn archive_cli_refuses_live_running_attempt() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let facts = solstone_core_import::admit_running_attempt(
            journal.path(),
            "20260809_090000",
            now_ms,
            Some("journal_archive"),
        )
        .unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Running);

        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stderr.contains("journal_archive import failed:"));
        assert!(run.stderr.contains("already running"), "{}", run.stderr);
    }

    #[test]
    fn archive_merge_incomplete_records_unconfirmed() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("entities/person")).unwrap();
        fs::write(
            journal.path().join("entities/person/entity.json"),
            br#"{"id":"person","name":"Person 1","summary":"Diff 1"}"#,
        )
        .unwrap();

        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person 2","summary":"Diff 2"}"#,
            )],
        );

        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        assert!(
            run.stderr.contains("journal_archive import incomplete:"),
            "{}",
            run.stderr
        );

        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Unconfirmed);
        assert_eq!(
            facts.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_UNCONFIRMED_REASON)
        );

        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(
            proj.status,
            solstone_core_import::ProjectionStatus::Unconfirmed
        );
    }

    #[test]
    fn archive_merge_failure_not_mutated_records_failed() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );
        // Put a regular file at `entities` in journal to cause merge to fail with NotMutated
        fs::write(journal.path().join("entities"), b"blocker").unwrap();
        let before_snapshot = journal_family_snapshot(journal.path());

        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stderr.contains("journal_archive import failed:"));

        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Unconfirmed);
        assert_eq!(
            facts.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_FAILED_REASON)
        );

        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(proj.status, solstone_core_import::ProjectionStatus::Failed);

        let after_snapshot = journal_family_snapshot(journal.path());
        assert_eq!(
            after_snapshot
                .into_iter()
                .filter(|(k, _)| !k.starts_with("imports/20260809_090000"))
                .collect::<std::collections::BTreeMap<_, _>>(),
            before_snapshot
        );
    }

    #[test]
    fn archive_merge_failure_may_have_mutated_records_unconfirmed() {
        let journal = tempfile::tempdir().unwrap();
        // Existing foreign stream.json in chronicle
        fs::create_dir_all(journal.path().join("chronicle/20260809/080000_10")).unwrap();
        fs::write(
            journal
                .path()
                .join("chronicle/20260809/080000_10/stream.json"),
            b"foreign stream data",
        )
        .unwrap();
        // Block stream update marker directory after segment publish
        fs::create_dir_all(
            journal
                .path()
                .join("chronicle/20260809/health/stream.updated"),
        )
        .unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[("chronicle/20260809/120000_60/stream.json", b"stream data")],
        );

        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);

        // Copied chronicle/20260809/120000_60/stream.json bytes equal archive member (b"stream data"). Foreign stream unchanged.
        assert_eq!(
            fs::read(
                journal
                    .path()
                    .join("chronicle/20260809/120000_60/stream.json")
            )
            .unwrap(),
            b"stream data"
        );
        assert_eq!(
            fs::read(
                journal
                    .path()
                    .join("chronicle/20260809/080000_10/stream.json")
            )
            .unwrap(),
            b"foreign stream data"
        );

        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(facts.generation, 1);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Unconfirmed);
        assert_eq!(
            facts.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_UNCONFIRMED_REASON)
        );

        let proj = solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        assert_eq!(
            proj.status,
            solstone_core_import::ProjectionStatus::Unconfirmed
        );
    }

    #[test]
    fn archive_supersession_vs_mutating_control() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );

        let before_snapshot = journal_family_snapshot(journal.path());
        let dispatch1 = archive_dispatch(&archive, "20260809_090000");

        // Run 1: superseded right after admission
        let journal_path = journal.path().to_path_buf();
        let superseding_hook = move || {
            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            solstone_core_import::admit_running_attempt(
                &journal_path,
                "20260809_090000",
                now_ms + 10,
                Some("journal_archive"),
            )
            .unwrap();
        };

        let seams = ArchiveTerminalSeams {
            after_admit: Some(&superseding_hook),
            ..DEFAULT_ARCHIVE_TERMINAL_SEAMS
        };
        let run1 = run_archive_with_seams(dispatch1, journal.path(), &mut None, &seams);
        assert_ne!(run1.exit_code, 0, "{}", run1.stderr);

        // Verify generation 2 was not overwritten and target data was not mutated
        let meta1 =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts1 = solstone_core_import::get_attempt_facts(&meta1).unwrap();
        assert_eq!(facts1.generation, 2);

        let after_snapshot = journal_family_snapshot(journal.path());
        assert_eq!(
            after_snapshot
                .into_iter()
                .filter(|(k, _)| !k.starts_with("imports/20260809_090000"))
                .collect::<std::collections::BTreeMap<_, _>>(),
            before_snapshot
        );

        // Run 2: control run without supersession hook mutates the target
        let dispatch2 = archive_dispatch(&archive, "20260809_090001");
        let run2 = run(dispatch2, journal.path());
        assert_eq!(run2.exit_code, 0, "{}", run2.stderr);
        assert!(journal.path().join("entities/person/entity.json").exists());
    }

    #[test]
    fn archive_death_after_admit_reads_interrupted() {
        let journal = tempfile::tempdir().unwrap();
        let start_ms = 1_000_000u64;
        solstone_core_import::admit_running_attempt(
            journal.path(),
            "20260809_090000",
            start_ms,
            Some("journal_archive"),
        )
        .unwrap();

        // Held past the wall-clock bound -> still Running: a slow archive is alive.
        let clock_past_bound =
            (start_ms + solstone_core_import::RUNNING_ATTEMPT_BOUND_MS + 500) as f64 / 1000.0;
        let proj_running = solstone_core_import::projection::project_import_result_with_clock(
            journal.path(),
            "20260809_090000",
            clock_past_bound,
        );
        assert_eq!(
            proj_running.status,
            solstone_core_import::ProjectionStatus::Running
        );

        // The producer dies without recording an end -> Unconfirmed at once, inside the bound.
        solstone_core_import::release_attempt(journal.path(), "20260809_090000");
        let clock_inside_bound = (start_ms + 500) as f64 / 1000.0;
        let proj_unconfirmed = solstone_core_import::projection::project_import_result_with_clock(
            journal.path(),
            "20260809_090000",
            clock_inside_bound,
        );
        assert_eq!(
            proj_unconfirmed.status,
            solstone_core_import::ProjectionStatus::Unconfirmed
        );
        assert_eq!(
            proj_unconfirmed.error.as_deref(),
            Some(solstone_core_import::IMPORT_UNCONFIRMED_REASON)
        );
        assert_eq!(proj_unconfirmed.error_stage.as_deref(), Some("interrupted"));
        assert_ne!(
            proj_unconfirmed.error.as_deref(),
            Some("Import never completed")
        );
    }

    #[test]
    fn legacy_no_attempt_vs_admitted_row() {
        let journal = tempfile::tempdir().unwrap();
        // Row A: no-attempt archive-shaped row (task_id set, no attempt)
        let row_a_dir = journal.path().join("imports/20260809_080000");
        fs::create_dir_all(&row_a_dir).unwrap();
        let row_a_meta = serde_json::json!({
            "task_id": "20260809_080000",
            "source_type": "journal_archive",
            "source_hint": "journal_archive",
            "upload_timestamp": 1_000_000,
        });
        fs::write(
            row_a_dir.join("import.json"),
            serde_json::to_vec(&row_a_meta).unwrap(),
        )
        .unwrap();

        let clock = 2_000_000.0;
        let proj_a = solstone_core_import::projection::project_import_result_with_clock(
            journal.path(),
            "20260809_080000",
            clock,
        );
        assert_eq!(
            proj_a.status,
            solstone_core_import::ProjectionStatus::Failed
        );
        assert_eq!(proj_a.error.as_deref(), Some("Import never completed"));
        assert_eq!(proj_a.error_stage.as_deref(), Some("timeout"));

        // Row B: admitted completed archive merge (CLI 0, attempt completed, projection Success)
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );
        let dispatch_b = archive_dispatch(&archive, "20260809_090000");
        let run_b = run(dispatch_b, journal.path());
        assert_eq!(run_b.exit_code, 0, "{}", run_b.stderr);

        let meta_b =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts_b = solstone_core_import::get_attempt_facts(&meta_b).unwrap();
        assert_eq!(facts_b.state, solstone_core_import::AttemptState::Completed);

        let proj_b = solstone_core_import::projection::project_import_result_with_clock(
            journal.path(),
            "20260809_090000",
            clock,
        );
        assert_eq!(
            proj_b.status,
            solstone_core_import::ProjectionStatus::Success
        );
        assert!(proj_b.error.is_none());

        // Confirm Row A metadata was not mutated
        let raw_a_bytes = fs::read(row_a_dir.join("import.json")).unwrap();
        let raw_a_val: serde_json::Value = serde_json::from_slice(&raw_a_bytes).unwrap();
        assert!(raw_a_val.get("attempt").is_none());
    }

    #[test]
    fn archive_merge_records_its_results_and_the_owner_identity_warning() {
        let journal = tempfile::tempdir().unwrap();
        fs::create_dir_all(journal.path().join("entities/owner")).unwrap();
        fs::write(
            journal.path().join("entities/owner/entity.json"),
            br#"{"id":"owner","name":"Owner","type":"Person","is_principal":true}"#,
        )
        .unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[
                (
                    "entities/other/entity.json",
                    br#"{"id":"other","name":"Qxjvplmzt","type":"Person","is_principal":true}"#,
                ),
                ("chronicle/20260809/120000_60/stream.json", b"stream data"),
            ],
        );
        let run = run(
            archive_dispatch(&archive, "20260809_090000"),
            journal.path(),
        );

        let projection =
            solstone_core_import::project_import_result(journal.path(), "20260809_090000");
        let collision = projection
            .principal_collision
            .expect("owner identity warning");
        assert_eq!(collision["target_name"], "Owner", "{}", run.stderr);
        assert_eq!(collision["source_name"], "Qxjvplmzt");
        let summary = projection.merge_summary.expect("merge summary");
        assert_eq!(summary["segments_copied"], 1);
        assert!(summary["entities_created"].is_u64());
        assert_eq!(projection.entries_written, Some(1));
        assert_eq!(projection.source_type, "journal_archive");
        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        assert!(meta["merge_log_path"].is_string());
        assert!(meta["merge_staging_path"].is_string());
    }

    #[test]
    fn archive_terminal_recorder_stub_failure() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "chronicle/20260809/090000_10/stream.json",
                b"foreign stream data",
            )],
        );
        let dispatch = archive_dispatch(&archive, "20260809_090000");

        let stubbed_record_completed =
            |path: &Path, _t: &str, _g: u64, _f: u64, _d: Option<u64>, _e: Option<String>| {
                Err(solstone_core_import::ImportError::LockFailed {
                    path: path.to_path_buf(),
                    message: "mock lock failed".into(),
                })
            };

        let seams = ArchiveTerminalSeams {
            record_completed: &stubbed_record_completed,
            ..DEFAULT_ARCHIVE_TERMINAL_SEAMS
        };

        let run = run_archive_with_seams(dispatch, journal.path(), &mut None, &seams);
        assert_ne!(run.exit_code, 0, "{}", run.stderr);

        // Content was merged, foreign stream.json bytes unchanged
        assert_eq!(
            fs::read(
                journal
                    .path()
                    .join("chronicle/20260809/090000_10/stream.json")
            )
            .unwrap(),
            b"foreign stream data"
        );

        // imported.json was not created
        assert!(
            !journal
                .path()
                .join("imports/20260809_090000/imported.json")
                .exists()
        );

        // Attempt in import.json remains Running (not completed) because terminal record failed
        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_ne!(facts.state, solstone_core_import::AttemptState::Completed);
        assert_eq!(facts.state, solstone_core_import::AttemptState::Running);
    }

    #[test]
    fn archive_lock_failed_cli_non_zero_and_attempt_failed() {
        let journal = tempfile::tempdir().unwrap();
        let archive = write_test_archive(
            journal.path(),
            "valid.zip",
            &[(
                "entities/person/entity.json",
                br#"{"id":"person","name":"Person","type":"Person"}"#,
            )],
        );

        // Create a directory at the sidecar lockfile location to force archive-merge lock acquisition failure
        fs::create_dir_all(journal.path().join("health/locks/archive-merge.lock")).unwrap();

        let before_snapshot = journal_family_snapshot(journal.path());
        let dispatch = archive_dispatch(&archive, "20260809_090000");
        let run = run(dispatch, journal.path());
        assert_ne!(run.exit_code, 0, "{}", run.stderr);
        let expected_lock_prefix = format!(
            "archive merge lock failed at {}",
            journal.path().join("health/locks/archive-merge").display()
        );
        assert!(
            run.stderr.contains(&expected_lock_prefix),
            "stderr did not contain {:?}; was: {}",
            expected_lock_prefix,
            run.stderr
        );

        // Attempt recorded as failed
        let meta =
            solstone_core_import::read_import_metadata(journal.path(), "20260809_090000").unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_eq!(facts.state, solstone_core_import::AttemptState::Unconfirmed);
        assert_eq!(
            facts.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_FAILED_REASON)
        );

        let after_snapshot = journal_family_snapshot(journal.path());
        assert_eq!(
            after_snapshot
                .into_iter()
                .filter(|(k, _)| !k.starts_with("imports/20260809_090000"))
                .collect::<std::collections::BTreeMap<_, _>>(),
            before_snapshot
        );
    }

    #[test]
    fn cli_producer_request_has_no_heartbeat() {
        let root = Path::new("/tmp/test-journal");
        let media = PathBuf::from("/tmp/sample.png");
        let dispatch = RegistryDispatch {
            source: solstone_core_import::RegistrySource::Image,
            media,
            timestamp: "20260408_120000".to_owned(),
            dry_run: false,
            force: false,
        };
        let req = cli_producer_request(root, &dispatch, 1);
        assert_eq!(req.heartbeat_interval, None);
    }

    fn populate_test_a_target(journal: &Path) {
        fs::create_dir_all(journal.join("entities/ambig-one")).unwrap();
        fs::write(
            journal.join("entities/ambig-one/entity.json"),
            br#"{"id":"ambig-one","name":"Qxjvplmzt","type":"Person"}"#,
        )
        .unwrap();
        fs::create_dir_all(journal.join("entities/ambig-two")).unwrap();
        fs::write(
            journal.join("entities/ambig-two/entity.json"),
            br#"{"id":"ambig-two","name":"Qxjvplmzt","type":"Person"}"#,
        )
        .unwrap();
        fs::create_dir_all(journal.join("entities/id-collision")).unwrap();
        fs::write(
            journal.join("entities/id-collision/entity.json"),
            br#"{"id":"id-collision","name":"Nrmqexist","type":"Person"}"#,
        )
        .unwrap();
        fs::create_dir_all(journal.join("entities/merge-target")).unwrap();
        fs::write(
            journal.join("entities/merge-target/entity.json"),
            br#"{"id":"merge-target","name":"Zzyzxmerge","type":"Person"}"#,
        )
        .unwrap();
        fs::write(
            journal.join("entities/retired.json"),
            br#"{"ids":{"held-aside":{"state":"deleted","dir":"held-aside"}}}"#,
        )
        .unwrap();
    }

    #[test]
    fn archive_merge_staged_entities_three_runs_test_a() {
        let journal = tempfile::tempdir().unwrap();
        populate_test_a_target(journal.path());
        let import_id = "20260809_090000";

        let archive1_entries: &[(&str, &[u8])] = &[
            (
                "entities/ambig-source/entity.json",
                br#"{"id":"ambig-source","name":"Qxjvplmzt","type":"Person"}"#,
            ),
            (
                "entities/held-aside/entity.json",
                br#"{"id":"held-aside","name":"Hld","type":"Person"}"#,
            ),
            (
                "entities/id-collision/entity.json",
                br#"{"id":"id-collision","name":"","type":"Person"}"#,
            ),
            (
                "entities/merge-source/entity.json",
                br#"{"id":"merge-source","name":"Zzyzxmerge","type":"Person","aka":["Zzyzxextra"]}"#,
            ),
            (
                "entities/create-source/entity.json",
                br#"{"id":"create-source","name":"New","type":"Person"}"#,
            ),
            ("chronicle/20260809/120000_60/stream.json", b"stream data"),
        ];

        let archive1 = write_test_archive(journal.path(), "run1.zip", archive1_entries);
        let run1 = run(archive_dispatch(&archive1, import_id), journal.path());
        assert_eq!(run1.exit_code, 1, "{}", run1.stderr);

        let meta1 = solstone_core_import::read_import_metadata(journal.path(), import_id).unwrap();
        let facts1 = solstone_core_import::get_attempt_facts(&meta1).unwrap();
        assert_eq!(
            facts1.state,
            solstone_core_import::AttemptState::Unconfirmed
        );
        assert_eq!(
            facts1.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_UNCONFIRMED_REASON)
        );
        assert_eq!(facts1.attempt_id, format!("{import_id}:1"));

        // Ground truth from clone merge
        let clone_journal = tempfile::tempdir().unwrap();
        populate_test_a_target(clone_journal.path());
        let clone_archive = write_test_archive(clone_journal.path(), "clone.zip", archive1_entries);
        let clone_merge = merge_journal_archive(
            &clone_archive,
            clone_journal.path(),
            &ArchiveMergeOptions::default(),
            None,
        )
        .unwrap();

        let summary1 = meta1.get("merge_summary").unwrap();
        assert_eq!(summary1["entities_staged"], 3);
        assert!(summary1["entities_merged"].as_u64().unwrap() >= 1);
        assert!(summary1["entities_created"].as_u64().unwrap() >= 1);

        let proj1 = solstone_core_import::project_import_result(journal.path(), import_id);
        let staged_proj1 = proj1.staged_entities.expect("projected staged entities");
        assert_eq!(staged_proj1.omitted, 0);

        let staged_dispositions: Vec<_> = clone_merge
            .entity_dispositions
            .iter()
            .filter(|d| d.disposition.is_set_aside())
            .collect();
        assert_eq!(staged_dispositions.len(), 3);
        assert_eq!(staged_dispositions[0].source_id, "ambig-source");
        assert_eq!(
            staged_dispositions[0].disposition,
            solstone_core_import_sources::archive::EntityDispositionKind::StagedAmbiguous
        );
        assert_eq!(staged_dispositions[1].source_id, "held-aside");
        assert_eq!(
            staged_dispositions[1].disposition,
            solstone_core_import_sources::archive::EntityDispositionKind::StagedDeletedHere
        );
        assert_eq!(staged_dispositions[2].source_id, "id-collision");
        assert_eq!(
            staged_dispositions[2].disposition,
            solstone_core_import_sources::archive::EntityDispositionKind::StagedIdCollision
        );
        assert_eq!(staged_proj1.rows.len(), 3);

        for (i, disposition) in staged_dispositions.iter().enumerate() {
            assert_eq!(staged_proj1.rows[i].source_id, disposition.source_id);
            let staged_file_path = PathBuf::from(&staged_proj1.rows[i].staging_path);
            let file_bytes = fs::read(&staged_file_path).unwrap();
            let file_val: serde_json::Value = serde_json::from_slice(&file_bytes).unwrap();
            let expected_label = file_val
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&disposition.source_id);
            assert_eq!(staged_proj1.rows[i].source_name, expected_label);
        }

        // Run 2: same import id
        fs::write(
            journal.path().join("entities/retired.json"),
            br#"{"ids":{"z-merged-away":{"state":"merged","dir":"z-merged-away","successor":"survivor"}}}"#,
        )
        .unwrap();
        let archive2 = write_test_archive(
            journal.path(),
            "run2.zip",
            &[
                (
                    "entities/a-staged/entity.json",
                    br#"{"id":"a-staged","name":"Qxjvplmzt","type":"Person"}"#,
                ),
                (
                    "entities/z-merged-away/entity.json",
                    br#"{"id":"z-merged-away","name":"Z","type":"Person"}"#,
                ),
                ("chronicle/20260809/130000_60/stream.json", b"stream data"),
            ],
        );
        let run2 = run(archive_dispatch(&archive2, import_id), journal.path());
        assert_ne!(run2.exit_code, 0, "{}", run2.stderr);
        let meta2 = solstone_core_import::read_import_metadata(journal.path(), import_id).unwrap();
        let facts2 = solstone_core_import::get_attempt_facts(&meta2).unwrap();
        assert_eq!(facts2.attempt_id, format!("{import_id}:2"));
        assert_eq!(
            facts2.failure_reason.as_deref(),
            Some(solstone_core_import::IMPORT_FAILED_REASON)
        );
        let proj2 = solstone_core_import::project_import_result(journal.path(), import_id);
        assert!(proj2.staged_entities.is_none());
        assert_eq!(
            meta2["staged_entities"]["attempt_id"],
            format!("{import_id}:1")
        );

        let runs_dir = journal.path().join("imports/archive-merge-work/runs");
        let mut matching_logs = Vec::new();
        if let Ok(run_entries) = fs::read_dir(runs_dir) {
            for entry in run_entries.flatten() {
                let log_file = entry.path().join("decision-log.jsonl");
                if let Ok(content) = fs::read_to_string(log_file)
                    && content.contains("a-staged")
                {
                    let has_committed_staged = content.lines().any(|line| {
                        serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .is_some_and(|row| {
                                row.get("state").and_then(serde_json::Value::as_str)
                                    == Some("committed")
                                    && row
                                        .get("detail")
                                        .and_then(|d| d.get("staged"))
                                        .and_then(serde_json::Value::as_bool)
                                        == Some(true)
                            })
                    });
                    if has_committed_staged {
                        matching_logs.push(entry.path());
                    }
                }
            }
        }
        assert_eq!(
            matching_logs.len(),
            1,
            "exactly one decision-log.jsonl with a-staged and committed staged: true"
        );

        // Run 3: same import id, segment-only archive
        let archive3 = write_test_archive(
            journal.path(),
            "run3.zip",
            &[("chronicle/20260809/090000_10/stream.json", b"stream data")],
        );
        let run3 = run(archive_dispatch(&archive3, import_id), journal.path());
        assert_eq!(run3.exit_code, 0, "{}", run3.stderr);
        let meta3 = solstone_core_import::read_import_metadata(journal.path(), import_id).unwrap();
        let facts3 = solstone_core_import::get_attempt_facts(&meta3).unwrap();
        assert_eq!(facts3.state, solstone_core_import::AttemptState::Completed);
        assert!(meta3.get("staged_entities").is_none());
        let proj3 = solstone_core_import::project_import_result(journal.path(), import_id);
        assert!(proj3.staged_entities.is_none());
    }

    #[test]
    fn archive_merge_staged_unconfirmed_terminal_failure_test_b() {
        let journal = tempfile::tempdir().unwrap();
        populate_test_a_target(journal.path());
        let import_id = "20260809_090000";

        let archive = write_test_archive(
            journal.path(),
            "staged_b.zip",
            &[
                (
                    "entities/ambig-source/entity.json",
                    br#"{"id":"ambig-source","name":"Qxjvplmzt","type":"Person"}"#,
                ),
                (
                    "entities/held-aside/entity.json",
                    br#"{"id":"held-aside","name":"Hld","type":"Person"}"#,
                ),
                (
                    "entities/id-collision/entity.json",
                    br#"{"id":"id-collision","name":"","type":"Person"}"#,
                ),
                (
                    "entities/merge-source/entity.json",
                    br#"{"id":"merge-source","name":"Zzyzxmerge","type":"Person","aka":["Zzyzxextra"]}"#,
                ),
                (
                    "entities/create-source/entity.json",
                    br#"{"id":"create-source","name":"New","type":"Person"}"#,
                ),
                ("chronicle/20260809/120000_60/stream.json", b"stream data"),
            ],
        );
        let dispatch = archive_dispatch(&archive, import_id);

        let stubbed_record_unconfirmed =
            |path: &Path, _t: &str, _g: u64, _f: u64, _e: Option<String>| {
                Err(solstone_core_import::ImportError::LockFailed {
                    path: path.to_path_buf(),
                    message: "mock lock failed".into(),
                })
            };

        let seams = ArchiveTerminalSeams {
            record_unconfirmed: &stubbed_record_unconfirmed,
            ..DEFAULT_ARCHIVE_TERMINAL_SEAMS
        };

        let run = run_archive_with_seams(dispatch, journal.path(), &mut None, &seams);
        assert_ne!(run.exit_code, 0, "{}", run.stderr);

        let meta = solstone_core_import::read_import_metadata(journal.path(), import_id).unwrap();
        let facts = solstone_core_import::get_attempt_facts(&meta).unwrap();
        assert_ne!(facts.state, solstone_core_import::AttemptState::Completed);

        let proj = solstone_core_import::project_import_result(journal.path(), import_id);
        assert_ne!(proj.status, solstone_core_import::ProjectionStatus::Success);
    }

    #[test]
    fn chat_reimport_with_tombstone_filters_manifest_and_adjusts_counts() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;

        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());

        // Build ChatGPT export zip
        let zip_path = journal.path().join("chatgpt_export.zip");
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("conversations.json", SimpleFileOptions::default())
            .unwrap();
        let conversations_json = serde_json::json!([
            {
                "title": "Distinct Title A",
                "current_node": "m3",
                "mapping": {
                    "m1": {
                        "id": "m1",
                        "parent": null,
                        "message": {
                            "author": { "role": "user" },
                            "content": { "parts": ["Distinct preview message A"] },
                            "create_time": 1767225600.0
                        }
                    },
                    "m2": {
                        "id": "m2",
                        "parent": "m1",
                        "message": {
                            "author": { "role": "assistant" },
                            "content": { "parts": ["Message A2"] },
                            "create_time": 1767225660.0
                        }
                    },
                    "m3": {
                        "id": "m3",
                        "parent": "m2",
                        "message": {
                            "author": { "role": "user" },
                            "content": { "parts": ["Message A3"] },
                            "create_time": 1767312000.0
                        }
                    }
                }
            },
            {
                "title": "Distinct Title B",
                "current_node": "n2",
                "mapping": {
                    "n1": {
                        "id": "n1",
                        "parent": null,
                        "message": {
                            "author": { "role": "user" },
                            "content": { "parts": ["Message B1"] },
                            "create_time": 1767398400.0
                        }
                    },
                    "n2": {
                        "id": "n2",
                        "parent": "n1",
                        "message": {
                            "author": { "role": "assistant" },
                            "content": { "parts": ["Message B2"] },
                            "create_time": 1767398460.0
                        }
                    }
                }
            }
        ]);
        zip.write_all(conversations_json.to_string().as_bytes())
            .unwrap();
        zip.finish().unwrap();

        let chat_dispatch = |path: &Path, stamp: &str| RegistryDispatch {
            source: RegistrySource::Chatgpt,
            media: path.to_path_buf(),
            timestamp: stamp.to_owned(),
            dry_run: false,
            force: false,
        };

        // Run 1
        let (run1, id1) = run_bound(chat_dispatch(&zip_path, "20260104_100000"), journal.path());
        assert_eq!(run1.exit_code, 0, "{}", run1.stderr);
        let id1 = id1.unwrap();
        let run1_manifest_path = journal
            .path()
            .join(format!("imports/{id1}/content_manifest.jsonl"));
        let run1_manifest_bytes = fs::read(&run1_manifest_path).unwrap();

        // Tombstone the segment under chronicle/20260101/
        let day1_dir = journal.path().join("chronicle/20260101/import.chatgpt");
        for entry in fs::read_dir(&day1_dir).unwrap() {
            let seg_dir = entry.unwrap().path();
            if seg_dir.is_dir() {
                for child in fs::read_dir(&seg_dir).unwrap() {
                    let child_path = child.unwrap().path();
                    if child_path.is_file() {
                        fs::remove_file(&child_path).unwrap();
                    }
                }
                fs::write(seg_dir.join("tombstone.json"), b"{}").unwrap();
            }
        }

        // Run 2
        let (run2, id2) = run_bound(chat_dispatch(&zip_path, "20260104_110000"), journal.path());
        assert_eq!(run2.exit_code, 0, "{}", run2.stderr);
        let id2 = id2.unwrap();

        // Run 2 assertions
        assert!(run2.stdout.contains("entries_written=3"), "{}", run2.stdout);
        assert!(
            run2.stdout
                .contains("imported 3 messages from 2 conversations across 2 days"),
            "{}",
            run2.stdout
        );

        let manifest2_meta: serde_json::Value = serde_json::from_slice(
            &fs::read(journal.path().join(format!("imports/{id2}/manifest.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest2_meta["entry_count"], 3);
        let days_affected = manifest2_meta["days_affected"].as_array().unwrap();
        assert_eq!(
            days_affected,
            &vec![serde_json::json!("20260102"), serde_json::json!("20260103")]
        );
        let files_created = manifest2_meta["files_created"].as_array().unwrap();
        assert_eq!(files_created.len(), 2);

        let run2_manifest_path = journal
            .path()
            .join(format!("imports/{id2}/content_manifest.jsonl"));
        let run2_manifest_content = fs::read_to_string(&run2_manifest_path).unwrap();
        let run2_rows: Vec<&str> = run2_manifest_content.lines().collect();
        assert_eq!(run2_rows.len(), 1);
        assert!(run2_rows[0].contains("Distinct Title B"));
        assert!(!run2_manifest_content.contains("Distinct Title A"));
        assert!(!run2_manifest_content.contains("Distinct preview message A"));

        // Run 1's manifest bytes are unchanged
        assert_eq!(fs::read(&run1_manifest_path).unwrap(), run1_manifest_bytes);

        // Control journal, no tombstone
        let control_journal = tempfile::tempdir().unwrap();
        write_utc_zone(control_journal.path());
        let (ctrl_run, ctrl_id) = run_bound(
            chat_dispatch(&zip_path, "20260104_100000"),
            control_journal.path(),
        );
        assert_eq!(ctrl_run.exit_code, 0, "{}", ctrl_run.stderr);
        let ctrl_id = ctrl_id.unwrap();

        assert!(
            ctrl_run.stdout.contains("entries_written=5"),
            "{}",
            ctrl_run.stdout
        );
        assert!(
            ctrl_run
                .stdout
                .contains("imported 5 messages from 2 conversations across 3 days"),
            "{}",
            ctrl_run.stdout
        );
        let ctrl_manifest_meta: serde_json::Value = serde_json::from_slice(
            &fs::read(
                control_journal
                    .path()
                    .join(format!("imports/{ctrl_id}/manifest.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(ctrl_manifest_meta["entry_count"], 5);

        let ctrl_manifest_content = fs::read_to_string(
            control_journal
                .path()
                .join(format!("imports/{ctrl_id}/content_manifest.jsonl")),
        )
        .unwrap();
        let ctrl_rows: Vec<&str> = ctrl_manifest_content.lines().collect();
        assert_eq!(ctrl_rows.len(), 2);
    }

    fn strava_dispatch(csv_path: &Path, timestamp: &str) -> RegistryDispatch {
        RegistryDispatch {
            source: RegistrySource::Strava,
            media: csv_path.to_path_buf(),
            timestamp: timestamp.to_owned(),
            dry_run: false,
            force: false,
        }
    }

    fn write_custom_zone(journal: &Path, zone: &str) {
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            format!(r#"{{"identity":{{"timezone":"{zone}"}}}}"#),
        )
        .unwrap();
    }

    const HEADER_ENGLISH_103: &str = "Activity ID,Activity Date,Activity Name,Activity Type,Activity Description,Elapsed Time,Distance,Max Heart Rate,Relative Effort,Commute,Activity Private Note,Activity Gear,Filename,Athlete Weight,Bike Weight,Elapsed Time,Moving Time,Distance,Max Speed,Average Speed,Elevation Gain,Elevation Loss,Elevation Low,Elevation High,Max Grade,Average Grade,Average Positive Grade,Average Negative Grade,Max Cadence,Average Cadence,Max Heart Rate,Average Heart Rate,Max Watts,Average Watts,Calories,Max Temperature,Average Temperature,Relative Effort,Total Work,Number of Runs,Uphill Time,Downhill Time,Other Time,Perceived Exertion,Type,Start Time,Weighted Average Power,Power Count,Prefer Perceived Exertion,Perceived Relative Effort,Commute,Total Weight Lifted,From Upload,Grade Adjusted Distance,Weather Observation Time,Weather Condition,Weather Temperature,Apparent Temperature,Dewpoint,Humidity,Weather Pressure,Wind Speed,Wind Gust,Wind Bearing,Precipitation Intensity,Sunrise Time,Sunset Time,Moon Phase,Bike,Gear,Precipitation Probability,Precipitation Type,Cloud Cover,Weather Visibility,UV Index,Weather Ozone,Jump Count,Total Grit,Average Flow,Flagged,Average Elapsed Speed,Dirt Distance,Newly Explored Distance,Newly Explored Dirt Distance,Activity Count,Total Steps,Carbon Saved,Pool Length,Training Load,Intensity,Average Grade Adjusted Pace,Timer Time,Total Cycles,Recovery,With Pet,Competition,Long Run,For a Cause,With Kid,Downhill Distance,Total Sets,Total Reps,Media\n";
    const HEADER_GERMAN_92: &str = "Aktivitäts-ID,Aktivitätsdatum,Name der Aktivität,Aktivitätsart,Aktivitätsbeschreibung,Verstrichene Zeit,Distanz,Max. Herzfrequenz,Relative Leistung,Pendeln,Hinweis zur Privatsphäre für Aktivitäten,Aktivitätsausrüstung,Dateiname,Sportlergewicht,Fahrradgewicht,Verstrichene Zeit,Bewegungszeit,Distanz,Höchstgeschw.,Durchschnittliche Geschwindigkeit,Höhenzunahme,Höhenunterschied,Min. Höhe,Max. Höhe,Max. Steigung,Durchschnittliche Steigung,Durchschnittliche positive Steigung,Durchschnittliche negative Steigung,Max. Tritt-/Schrittfrequenz,Durchschnittliche Trittfrequenz,Max. Herzfrequenz,Durchschnittliche Herzfrequenz,Max. Watt,Durchschnittliche Watt,Kalorien,Max. Temperatur,Durchschnittliche Temperatur,Relative Leistung,Gesamtarbeit,Anzahl Läufe,Bergaufzeit,Bergabzeit,Andere Zeit,Gefühlte Anstrengung,Art,Startzeit,Gewichtete durchschnittliche Leistung,Leistungszahl,Gefühlte Anstrengung verwenden,Gefühlte relative Leistung,Pendeln,Insgesamt gestemmtes Gewicht,Von Upload,Auf Steigung angepasste Distanz,Wetterbeobachtungszeit,Wetterlage,Wetter: Temperatur,Scheinbare Temperatur,Taupunkt,Luftfeuchtigkeit,Wetter: Druck,Windgeschwindigkeit,Windböe,Windrichtung,Niederschlagsintensität,Sonnenaufgangszeit,Sonnenuntergangszeit,Mondphase,Fahrrad,Ausrüstung,Niederschlagswahrscheinlichkeit,Niederschlagsart,Wolkendecke,Wetter: Sichtbarkeit,UV-Index,Wetter: Ozon,Sprunganzahl,Schwierigkeit insgesamt,Durchschnittlicher Flow,Markiert,Durchschnittsgeschwindigkeit im Aufzeichnungszeitraum,Auf Schotter zurückgelegte Distanz,Neu getestete Distanz,Neu getestete Schotterdistanz,Aktivitätsanzahl,Schritte insgesamt,Eingesparte CO₂-Emissionen,Pool-Länge,Trainingsbelastung,Intensität,Durchschnittliches auf Steigung angepasstes Tempo,Medien\n";
    const HEADER_ENGLISH_86: &str = "Activity ID,Activity Date,Activity Name,Activity Type,Activity Description,Elapsed Time,Distance,Max Heart Rate,Relative Effort,Commute,Activity Private Note,Activity Gear,Filename,Athlete Weight,Bike Weight,Elapsed Time,Moving Time,Distance,Max Speed,Average Speed,Elevation Gain,Elevation Loss,Elevation Low,Elevation High,Max Grade,Average Grade,Average Positive Grade,Average Negative Grade,Max Cadence,Average Cadence,Max Heart Rate,Average Heart Rate,Max Watts,Average Watts,Calories,Max Temperature,Average Temperature,Relative Effort,Total Work,Number of Runs,Uphill Time,Downhill Time,Other Time,Perceived Exertion,Type,Start Time,Weighted Average Power,Power Count,Prefer Perceived Exertion,Perceived Relative Effort,Commute,Total Weight Lifted,From Upload,Grade Adjusted Distance,Weather Observation Time,Weather Condition,Weather Temperature,Apparent Temperature,Dewpoint,Humidity,Weather Pressure,Wind Speed,Wind Gust,Wind Bearing,Precipitation Intensity,Sunrise Time,Sunset Time,Moon Phase,Bike,Gear,Precipitation Probability,Precipitation Type,Cloud Cover,Weather Visibility,UV Index,Weather Ozone,Jump Count,Total Grit,Average Flow,Flagged,Average Elapsed Speed,Dirt Distance,Newly Explored Distance,Newly Explored Dirt Distance,Activity Count,Media\n";

    fn build_csv(header: &str, rows: &[&[(&str, usize, &str)]]) -> String {
        let cols: Vec<&str> = header.trim_end_matches('\n').split(',').collect();
        let mut out = header.to_owned();

        for row in rows {
            let mut col_occurrences: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            let mut line_fields = Vec::with_capacity(cols.len());

            for col in &cols {
                let occ = col_occurrences.entry(col).or_insert(0);
                *occ += 1;
                let cur_occ = *occ;

                let val = row
                    .iter()
                    .find(|(name, o, _)| name == col && *o == cur_occ)
                    .map(|(_, _, v)| *v)
                    .unwrap_or("");
                line_fields.push(val);
            }

            out.push_str(&line_fields.join(","));
            out.push('\n');
        }

        out
    }

    fn make_en_csv_103(rows: &[(&str, &str, &str, u64, f64)]) -> String {
        let row_refs: Vec<Vec<(&str, usize, String)>> = rows
            .iter()
            .map(|(id, date_str, name, elapsed, dist)| {
                vec![
                    ("Activity ID", 1, id.to_string()),
                    ("Activity Date", 1, format!("\"{date_str}\"")),
                    ("Activity Name", 1, (*name).to_string()),
                    ("Activity Type", 1, "Run".to_string()),
                    ("Elapsed Time", 1, "9999".to_string()),
                    ("Elapsed Time", 2, elapsed.to_string()),
                    ("Moving Time", 1, elapsed.to_string()),
                    ("Distance", 1, "99.9".to_string()),
                    ("Distance", 2, dist.to_string()),
                    ("Elevation Gain", 1, "100".to_string()),
                    ("Average Heart Rate", 1, "140".to_string()),
                    ("Max Heart Rate", 1, "999".to_string()),
                    ("Max Heart Rate", 2, "160".to_string()),
                    ("Average Watts", 1, "200".to_string()),
                    ("Weighted Average Power", 1, "210".to_string()),
                    ("Calories", 1, "350".to_string()),
                    ("Commute", 1, "false".to_string()),
                    ("Commute", 2, "true".to_string()),
                    ("Filename", 1, format!("activities/{id}.gpx")),
                ]
            })
            .collect();

        let row_tuples: Vec<Vec<(&str, usize, &str)>> = row_refs
            .iter()
            .map(|r| r.iter().map(|(n, o, v)| (*n, *o, v.as_str())).collect())
            .collect();

        let row_slices: Vec<&[(&str, usize, &str)]> =
            row_tuples.iter().map(|r| r.as_slice()).collect();
        build_csv(HEADER_ENGLISH_103, &row_slices)
    }

    fn make_de_csv_92(rows: &[(&str, &str, &str, u64, &str)]) -> String {
        let row_refs: Vec<Vec<(&str, usize, String)>> = rows
            .iter()
            .map(|(id, date_str, name, elapsed, dist)| {
                let dist_val = if dist.contains(',') && !dist.starts_with('"') {
                    format!("\"{dist}\"")
                } else {
                    (*dist).to_string()
                };
                vec![
                    ("Aktivitäts-ID", 1, id.to_string()),
                    ("Aktivitätsdatum", 1, format!("\"{date_str}\"")),
                    ("Name der Aktivität", 1, (*name).to_string()),
                    ("Aktivitätsart", 1, "Run".to_string()),
                    ("Verstrichene Zeit", 1, "9999".to_string()),
                    ("Verstrichene Zeit", 2, elapsed.to_string()),
                    ("Bewegungszeit", 1, elapsed.to_string()),
                    ("Distanz", 1, "\"10,20\"".to_string()),
                    ("Distanz", 2, dist_val),
                    ("Höhenzunahme", 1, "100".to_string()),
                    ("Durchschnittliche Herzfrequenz", 1, "140".to_string()),
                    ("Max. Herzfrequenz", 1, "999".to_string()),
                    ("Max. Herzfrequenz", 2, "160".to_string()),
                    ("Durchschnittliche Watt", 1, "200".to_string()),
                    (
                        "Gewichtete durchschnittliche Leistung",
                        1,
                        "210".to_string(),
                    ),
                    ("Kalorien", 1, "350".to_string()),
                    ("Pendeln", 1, "false".to_string()),
                    ("Pendeln", 2, "true".to_string()),
                    ("Dateiname", 1, format!("activities/{id}.gpx")),
                ]
            })
            .collect();

        let row_tuples: Vec<Vec<(&str, usize, &str)>> = row_refs
            .iter()
            .map(|r| r.iter().map(|(n, o, v)| (*n, *o, v.as_str())).collect())
            .collect();

        let row_slices: Vec<&[(&str, usize, &str)]> =
            row_tuples.iter().map(|r| r.as_slice()).collect();
        build_csv(HEADER_GERMAN_92, &row_slices)
    }

    #[test]
    fn strava_preview_success_and_date_range() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[
            (
                "101",
                "Aug 10, 2026, 06:30:00 AM",
                "Morning Run",
                300,
                5000.0,
            ),
            (
                "102",
                "Aug 12, 2026, 07:00:00 AM",
                "Morning Ride",
                600,
                15000.0,
            ),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let mut dispatch = strava_dispatch(&csv_file, "20260812_090000");
        dispatch.dry_run = true;

        let (run, id) = run_bound(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(id.is_none());
        assert!(
            run.stdout.contains("strava preview: date_range=20260810..20260812 items=2 entities=3 summary=new=2 tiles_to_create=3"),
            "{}",
            run.stdout
        );
    }

    #[test]
    fn strava_preview_unparseable_date_fails() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Not A Date", "Bad Run", 300, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let mut dispatch = strava_dispatch(&csv_file, "20260812_090000");
        dispatch.dry_run = true;

        let (run, _) = run_bound(dispatch, journal.path());
        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("strava preview failed:"));
    }

    #[test]
    fn strava_import_english_single_and_multi_day() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[
            (
                "101",
                "Aug 10, 2026, 06:30:00 AM",
                "Morning Run",
                300,
                5000.0,
            ),
            (
                "102",
                "Aug 11, 2026, 07:00:00 AM",
                "Morning Ride",
                600,
                15000.0,
            ),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let dispatch = strava_dispatch(&csv_file, "20260811_090000");
        let (run, id) = run_bound(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let import_id = id.unwrap();
        assert!(
            run.stdout
                .contains("strava import complete: entries_written=2")
        );

        // Check chronicle segment files
        let tile_day1 = journal
            .path()
            .join("chronicle/20260810/import.strava/063000_300");
        assert!(tile_day1.join("workout.json").is_file());

        let tile_day2_1 = journal
            .path()
            .join("chronicle/20260811/import.strava/070000_300");
        let tile_day2_2 = journal
            .path()
            .join("chronicle/20260811/import.strava/070500_300");
        assert!(tile_day2_1.join("workout.json").is_file());
        assert!(tile_day2_2.join("workout.json").is_file());

        // Check manifest
        let manifest_path = journal
            .path()
            .join(format!("imports/{import_id}/manifest.json"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["source_type"], "strava");
        assert_eq!(manifest["entry_count"], 2);
        assert_eq!(manifest["raw_retention"], "discard");
        assert!(manifest["source_hash"].as_str().is_some());
        assert!(journal.path().join("imports/.strava-zone.json").is_file());
    }

    #[test]
    fn strava_import_german_headers_and_dates() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_de_csv_92(&[("201", "10.08.2026, 06:30:00", "Morgenlauf", 300, "5,0")]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let dispatch = strava_dispatch(&csv_file, "20260810_090000");
        let (run, id) = run_bound(dispatch, journal.path());
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(id.is_some());
        assert!(
            run.stdout
                .contains("strava import complete: entries_written=1")
        );

        let tile = journal
            .path()
            .join("chronicle/20260810/import.strava/063000_300");
        assert!(tile.join("workout.json").is_file());
    }

    #[test]
    fn strava_shuffled_rows_produce_identical_placement() {
        let journal1 = tempfile::tempdir().unwrap();
        let journal2 = tempfile::tempdir().unwrap();
        write_utc_zone(journal1.path());
        write_utc_zone(journal2.path());

        let row_a = ("301", "Aug 10, 2026, 06:00:00 AM", "Run A", 300, 5000.0);
        let row_b = ("302", "Aug 10, 2026, 12:00:00 PM", "Run B", 300, 5000.0);
        let row_c = ("303", "Aug 11, 2026, 08:00:00 AM", "Run C", 300, 5000.0);

        let csv1 = journal1.path().join("activities.csv");
        let csv2 = journal2.path().join("activities.csv");

        fs::write(&csv1, make_en_csv_103(&[row_a, row_b, row_c]).as_bytes()).unwrap();
        fs::write(&csv2, make_en_csv_103(&[row_c, row_a, row_b]).as_bytes()).unwrap();

        let (run1, id1) = run_bound(strava_dispatch(&csv1, "20260811_120000"), journal1.path());
        let (run2, id2) = run_bound(strava_dispatch(&csv2, "20260811_120000"), journal2.path());

        assert_eq!(run1.exit_code, 0);
        assert_eq!(run2.exit_code, 0);
        assert_eq!(id1, id2);

        // Verify tiles match
        for key in ["060000_300", "120000_300"] {
            let p1 = journal1
                .path()
                .join("chronicle/20260810/import.strava")
                .join(key)
                .join("workout.json");
            let p2 = journal2
                .path()
                .join("chronicle/20260810/import.strava")
                .join(key)
                .join("workout.json");
            assert_eq!(
                fs::read_to_string(p1).unwrap(),
                fs::read_to_string(p2).unwrap()
            );
        }
    }

    #[test]
    fn strava_reimport_idempotent_and_skips_unchanged() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[(
            "401",
            "Aug 10, 2026, 06:30:00 AM",
            "Morning Run",
            300,
            5000.0,
        )]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        // Run 1
        let (run1, id1) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);
        assert!(run1.stdout.contains("entries_written=1"));
        let id1 = id1.unwrap();

        // Run 2 on same CSV
        let (run2, id2) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("entries_written=0"));
        let id2 = id2.unwrap();
        assert_ne!(id1, id2);

        // Content of workout.json retains original import_id
        let workout_json: serde_json::Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("chronicle/20260810/import.strava/063000_300/workout.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(workout_json["import_id"], id1);
    }

    #[test]
    fn strava_reimport_with_tombstone_preserves_deletion() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[
            ("501", "Aug 10, 2026, 06:00:00 AM", "Run 1", 300, 5000.0),
            ("502", "Aug 10, 2026, 12:00:00 PM", "Run 2", 300, 5000.0),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        // Run 1
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);

        // Place tombstone on Run 1
        let tile1 = journal
            .path()
            .join("chronicle/20260810/import.strava/060000_300");
        fs::write(tile1.join("tombstone.json"), b"{}").unwrap();

        // Run 2 with expanded CSV
        let content2 = make_en_csv_103(&[
            ("501", "Aug 10, 2026, 06:00:00 AM", "Run 1", 300, 5000.0),
            ("502", "Aug 10, 2026, 12:00:00 PM", "Run 2", 300, 5000.0),
            ("503", "Aug 11, 2026, 08:00:00 AM", "Run 3", 300, 5000.0),
        ]);
        let csv_file2 = journal.path().join("activities2.csv");
        fs::write(&csv_file2, content2.as_bytes()).unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file2, "20260811_100000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("entries_written=1"), "{}", run2.stdout);

        // Run 1 stays deleted (tombstoned), Run 3 is created, no _301 created
        assert!(tile1.join("tombstone.json").is_file());
        assert!(
            !journal
                .path()
                .join("chronicle/20260810/import.strava/060000_301")
                .exists()
        );
        assert!(
            journal
                .path()
                .join("chronicle/20260811/import.strava/080000_300/workout.json")
                .is_file()
        );
    }

    #[test]
    fn strava_cli_connectivity_and_registration() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        fs::write(&csv_file, b"arbitrary").unwrap();

        let lookup_env = |var: &str| {
            if var == "SOL_SKIP_SUPERVISOR_CHECK" {
                Some("1".to_string())
            } else {
                None
            }
        };
        let connectivity = || false;

        let strava_args = vec![
            "--source".to_string(),
            "strava".to_string(),
            csv_file.to_str().unwrap().to_string(),
        ];
        let outcome_strava = solstone_core_import_host::cli_argv::run_cli_with(
            &strava_args,
            journal.path(),
            lookup_env,
            connectivity,
        );
        let run = match outcome_strava {
            solstone_core_import_host::cli_argv::CliOutcome::Rendered(run) => run,
            solstone_core_import_host::cli_argv::CliOutcome::Imported { run, .. } => run,
            solstone_core_import_host::cli_argv::CliOutcome::Registry(dispatch) => {
                run(dispatch, journal.path())
            }
        };
        assert_ne!(run.exit_code, 0);
        assert!(!journal.path().join("imports").exists());

        let ics_file = journal.path().join("test.ics");
        fs::write(&ics_file, b"BEGIN:VCALENDAR\nEND:VCALENDAR\n").unwrap();
        let ics_args = vec![
            "--source".to_string(),
            "ics".to_string(),
            ics_file.to_str().unwrap().to_string(),
        ];
        let outcome_ics = solstone_core_import_host::cli_argv::run_cli_with(
            &ics_args,
            journal.path(),
            lookup_env,
            connectivity,
        );
        match outcome_ics {
            solstone_core_import_host::cli_argv::CliOutcome::Registry(dispatch) => {
                assert_eq!(dispatch.source, RegistrySource::Ics);
            }
            other => panic!("expected CliOutcome::Registry with Ics, got {:?}", other),
        }

        assert_eq!(
            RegistrySource::from_name("strava"),
            Some(RegistrySource::Strava)
        );
        assert_eq!(RegistrySource::Strava.name(), "strava");
        assert!(solstone_core_format::body::is_body_stream("import.strava"));
        assert!(
            !solstone_core_import_sources::registry::claims(RegistrySource::Strava, &csv_file)
                .unwrap()
        );
        let real = journal.path().join("real-activities.csv");
        fs::write(
            &real,
            make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]).as_bytes(),
        )
        .unwrap();
        assert!(
            solstone_core_import_sources::registry::claims(RegistrySource::Strava, &real).unwrap()
        );
        assert_eq!(RegistrySource::from_name("ics"), Some(RegistrySource::Ics));
    }

    #[test]
    fn strava_keeps_every_field_of_the_owners_workout() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");

        let row: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "901"),
            ("Activity Date", 1, "\"Aug 10, 2026, 06:30:00 AM\""),
            ("Activity Name", 1, "Sentinel Run"),
            ("Activity Type", 1, "Run"),
            ("Activity Description", 1, "SENTINEL_DESC"),
            ("Activity Private Note", 1, "SENTINEL_NOTE"),
            ("Activity Gear", 1, "SENTINEL_GEAR"),
            ("Media", 1, "SENTINEL_MEDIA"),
            ("Weather Condition", 1, "SENTINEL_WEATHER"),
            ("Distance", 1, "SENTINEL_DIST"),
            ("Distance", 2, "5000"),
            ("Relative Effort", 1, "SENTINEL_EFFORT"),
            ("Elapsed Time", 2, "300"),
            ("Filename", 1, "activities/901.gpx"),
        ];
        let content = build_csv(HEADER_ENGLISH_103, &[row]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (run, id) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        let import_id = id.unwrap();

        // Every column of the owner's own row is kept, attributed to Strava.
        let tile = fs::read_to_string(
            journal
                .path()
                .join("chronicle/20260810/import.strava/063000_300/workout.json"),
        )
        .unwrap();
        for kept in [
            "SENTINEL_DESC",
            "SENTINEL_NOTE",
            "SENTINEL_GEAR",
            "SENTINEL_MEDIA",
            "SENTINEL_WEATHER",
            "SENTINEL_DIST",
            "SENTINEL_EFFORT",
        ] {
            assert!(tile.contains(kept), "{kept} missing from {tile}");
        }

        assert!(
            !journal
                .path()
                .join(format!("imports/{import_id}/activities.csv"))
                .exists()
        );
        assert!(
            !journal
                .path()
                .join(format!("imports/{import_id}/source"))
                .exists()
        );
    }

    #[test]
    fn strava_error_codes_claim_no_attempts_and_print_code() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());

        // 1. List missing
        let missing_csv = journal.path().join("nonexistent.csv");
        let (run, _) = run_bound(
            strava_dispatch(&missing_csv, "20260810_100001"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("strava_list_missing"), "{}", run.stderr);
        assert!(!journal.path().join("imports/20260810_100001").exists());

        // 2. Zip unreadable
        let bad_zip = journal.path().join("bad.zip");
        fs::write(&bad_zip, b"PK\x03\x04corrupted_zip_bytes").unwrap();
        let (run, _) = run_bound(strava_dispatch(&bad_zip, "20260810_100002"), journal.path());
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("strava_zip_unreadable"),
            "{}",
            run.stderr
        );
        assert!(!journal.path().join("imports/20260810_100002").exists());

        // 3. Language unsupported
        let unsupp_csv = journal.path().join("unsupported.csv");
        fs::write(
            &unsupp_csv,
            b"Date,Time,Activity,Steps\n2026-01-01,10:00,Walk,100\n",
        )
        .unwrap();
        let (run, _) = run_bound(
            strava_dispatch(&unsupp_csv, "20260810_100003"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("strava_language_unsupported"),
            "{}",
            run.stderr
        );
        assert!(!journal.path().join("imports/20260810_100003").exists());

        // 4. Layout unrecognised (3 ways)
        // 4a. Missing kept column (drop Filename)
        let cols_86: Vec<&str> = HEADER_ENGLISH_86.trim_end().split(',').collect();
        let dropped_cols: Vec<&str> = cols_86.into_iter().filter(|c| *c != "Filename").collect();
        let bad_hdr_csv = journal.path().join("bad_hdr.csv");
        fs::write(&bad_hdr_csv, format!("{}\n", dropped_cols.join(","))).unwrap();
        let (run, _) = run_bound(
            strava_dispatch(&bad_hdr_csv, "20260810_100004"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("strava_layout_unrecognised"),
            "{}",
            run.stderr
        );
        assert!(!journal.path().join("imports/20260810_100004").exists());

        // 4b. Single distance column
        let cols_103: Vec<&str> = HEADER_ENGLISH_103.trim_end().split(',').collect();
        let mut first_dist_dropped = false;
        let mut single_dist_cols = Vec::new();
        for c in cols_103 {
            if c == "Distance" && !first_dist_dropped {
                first_dist_dropped = true;
            } else {
                single_dist_cols.push(c);
            }
        }
        let single_dist_csv = journal.path().join("single_dist.csv");
        fs::write(
            &single_dist_csv,
            format!("{}\n", single_dist_cols.join(",")),
        )
        .unwrap();
        let (run, _) = run_bound(
            strava_dispatch(&single_dist_csv, "20260810_100005"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("strava_layout_unrecognised"),
            "{}",
            run.stderr
        );
        assert!(!journal.path().join("imports/20260810_100005").exists());

        // 4c. No valid date format across decoded rows
        let bad_dates_csv = journal.path().join("bad_dates.csv");
        let row_bad_date: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "1"),
            ("Activity Date", 1, "not-a-valid-date"),
            ("Activity Name", 1, "Run"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 2, "300"),
        ];
        fs::write(
            &bad_dates_csv,
            build_csv(HEADER_ENGLISH_103, &[row_bad_date]).as_bytes(),
        )
        .unwrap();
        let (run, _) = run_bound(
            strava_dispatch(&bad_dates_csv, "20260810_100006"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(
            run.stderr.contains("strava_layout_unrecognised"),
            "{}",
            run.stderr
        );
        assert!(!journal.path().join("imports/20260810_100006").exists());

        // 5. No workouts (header-only)
        let hdr_only_csv = journal.path().join("hdr_only.csv");
        fs::write(&hdr_only_csv, HEADER_ENGLISH_103.as_bytes()).unwrap();
        let (run, _) = run_bound(
            strava_dispatch(&hdr_only_csv, "20260810_100007"),
            journal.path(),
        );
        assert_ne!(run.exit_code, 0);
        assert!(run.stderr.contains("strava_no_workouts"), "{}", run.stderr);
        assert!(!journal.path().join("imports/20260810_100007").exists());
    }

    #[test]
    fn strava_simultaneous_start_and_timing_collisions() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");

        let content = make_en_csv_103(&[
            ("101", "Aug 10, 2026, 07:00:00 AM", "Run 1", 300, 5000.0),
            ("102", "Aug 10, 2026, 07:00:00 AM", "Run 2", 300, 5000.0),
            (
                "103",
                "Aug 10, 2026, 08:00:00 AM",
                "Long Run",
                3600,
                15000.0,
            ),
            ("104", "Aug 10, 2026, 08:05:00 AM", "Short Run", 300, 1000.0),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (run, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);

        let p101 = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_300/workout.json");
        let p102 = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_301/workout.json");
        assert!(p101.is_file());
        assert!(p102.is_file());
        let v101: serde_json::Value = serde_json::from_slice(&fs::read(p101).unwrap()).unwrap();
        let v102: serde_json::Value = serde_json::from_slice(&fs::read(p102).unwrap()).unwrap();
        assert_eq!(v101["activity_id"], 101);
        assert_eq!(v102["activity_id"], 102);

        let p104 = journal
            .path()
            .join("chronicle/20260810/import.strava/080500_301/workout.json");
        assert!(p104.is_file());
        let v104: serde_json::Value = serde_json::from_slice(&fs::read(p104).unwrap()).unwrap();
        assert_eq!(v104["activity_id"], 104);
    }

    #[test]
    fn strava_duplicate_identity_canonical_selection() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());

        let day_dir = journal.path().join("chronicle/20260810/import.strava");
        fs::create_dir_all(day_dir.join("070000_99")).unwrap();
        fs::create_dir_all(day_dir.join("070000_100")).unwrap();

        let make_tile_json = |act_id: u64, name: &str| -> String {
            serde_json::json!({
                "schema": "solstone.import.strava.tile.v1",
                "import_id": "20260810_010000",
                "activity_id": act_id,
                "zone": "UTC",
                "tile": {
                    "index": 0,
                    "count": 1,
                    "start": "2026-08-10T07:00:00+00:00",
                    "end": "2026-08-10T07:05:00+00:00",
                    "seconds": 300
                },
                "workout": {
                    "name": name,
                    "type": "Run",
                    "start": "2026-08-10T07:00:00+00:00",
                    "elapsed_seconds": 300,
                    "moving_seconds": 300,
                    "distance_m": 5000.0,
                    "elevation_gain_m": 100.0,
                    "heart_rate_avg_bpm": 140.0,
                    "heart_rate_max_bpm": 160.0,
                    "power_avg_w": 200.0,
                    "power_weighted_w": 210.0,
                    "calories_kcal": 350.0,
                    "commute": true,
                    "entered_by_hand": false
                }
            })
            .to_string()
        };

        fs::write(
            day_dir.join("070000_99/workout.json"),
            make_tile_json(101, "Old Name"),
        )
        .unwrap();
        fs::write(
            day_dir.join("070000_100/workout.json"),
            make_tile_json(101, "Duplicate Copy"),
        )
        .unwrap();

        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[(
            "101",
            "Aug 10, 2026, 07:00:00 AM",
            "Updated Name",
            300,
            5000.0,
        )]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (run, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(run.stdout.contains("updated=1"), "{}", run.stdout);

        let v99: serde_json::Value =
            serde_json::from_slice(&fs::read(day_dir.join("070000_99/workout.json")).unwrap())
                .unwrap();
        assert_eq!(v99["workout"]["name"], "Updated Name");

        let v100: serde_json::Value =
            serde_json::from_slice(&fs::read(day_dir.join("070000_100/workout.json")).unwrap())
                .unwrap();
        assert_eq!(v100["workout"]["name"], "Duplicate Copy");
    }

    #[test]
    fn strava_reimport_renamed_and_timing_changed() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");

        // Run 1: initial
        let content1 = make_en_csv_103(&[(
            "101",
            "Aug 10, 2026, 07:00:00 AM",
            "Initial Name",
            300,
            5000.0,
        )]);
        fs::write(&csv_file, content1.as_bytes()).unwrap();
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);
        assert!(run1.stdout.contains("entries_written=1"));

        // Run 2: renamed -> present_updated, empty days_affected, entries_written=0 / entry_count=0
        let content2 = make_en_csv_103(&[(
            "101",
            "Aug 10, 2026, 07:00:00 AM",
            "Renamed Workout",
            300,
            5000.0,
        )]);
        fs::write(&csv_file, content2.as_bytes()).unwrap();
        let (run2, id2) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("entries_written=0"));
        assert!(run2.stdout.contains("present_updated=1"));
        let manifest_path2 = journal
            .path()
            .join(format!("imports/{}/manifest.json", id2.as_ref().unwrap()));
        let manifest2: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path2).unwrap()).unwrap();
        assert_eq!(manifest2["entry_count"], 0);
        assert_eq!(manifest2["days_affected"], serde_json::json!([]));
        let pub_rec2 = solstone_core_import::publish::read_publication_record(
            &journal
                .path()
                .join(format!("imports/{}", id2.as_ref().unwrap())),
        )
        .unwrap()
        .unwrap();
        assert!(pub_rec2.segments.is_empty());

        // Run 3: timing changed -> present_timing_changed=1
        let content3 = make_en_csv_103(&[(
            "101",
            "Aug 10, 2026, 07:00:00 AM",
            "Renamed Workout",
            600,
            5000.0,
        )]);
        fs::write(&csv_file, content3.as_bytes()).unwrap();
        let (run3, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_120000"),
            journal.path(),
        );
        assert_eq!(run3.exit_code, 0);
        assert!(run3.stdout.contains("present_timing_changed=1"));

        // Run 4: same instant different date format -> unchanged
        let row_diff_fmt: &[(&str, usize, &str)] = &[
            ("Activity ID", 1, "101"),
            ("Activity Date", 1, "\"10 Aug 2026, 07:00:00\""),
            ("Activity Name", 1, "Renamed Workout"),
            ("Activity Type", 1, "Run"),
            ("Elapsed Time", 1, "9999"),
            ("Elapsed Time", 2, "300"),
            ("Moving Time", 1, "300"),
            ("Distance", 1, "99.9"),
            ("Distance", 2, "5000"),
            ("Elevation Gain", 1, "100"),
            ("Average Heart Rate", 1, "140"),
            ("Max Heart Rate", 1, "999"),
            ("Max Heart Rate", 2, "160"),
            ("Average Watts", 1, "200"),
            ("Weighted Average Power", 1, "210"),
            ("Calories", 1, "350"),
            ("Commute", 1, "false"),
            ("Commute", 2, "true"),
            ("Filename", 1, "activities/101.gpx"),
        ];
        let content4 = build_csv(HEADER_ENGLISH_103, &[row_diff_fmt]);
        fs::write(&csv_file, content4.as_bytes()).unwrap();
        let (run4, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_130000"),
            journal.path(),
        );
        assert_eq!(run4.exit_code, 0);
        assert!(run4.stdout.contains("present_unchanged=1"));
    }

    #[test]
    fn strava_tombstone_and_unknown_reason_preservation() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");

        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 600, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);

        let tile1 = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_300");
        let tile2 = journal
            .path()
            .join("chronicle/20260810/import.strava/070500_300");
        fs::write(
            tile1.join("tombstone.json"),
            br#"{"reason":"user_deleted"}"#,
        )
        .unwrap();
        fs::write(
            tile2.join("tombstone.json"),
            br#"{"reason":"some_alien_reason"}"#,
        )
        .unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("stayed_deleted=2"));
        assert!(run2.stdout.contains(" deleted=1 "), "{}", run2.stdout);
        assert!(run2.stdout.contains("entries_written=0"));
    }

    /// What deleting a whole import run leaves at a piece's key: only its tombstone.
    fn release_tile(tile: &std::path::Path, reason: &[u8]) {
        for entry in fs::read_dir(tile).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::write(tile.join("tombstone.json"), reason).unwrap();
    }

    #[test]
    fn strava_never_keeps_an_uploaded_download() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
        let staged_dir = journal.path().join("imports/20260810_100000");
        fs::create_dir_all(&staged_dir).unwrap();
        let staged = staged_dir.join("activities.csv");
        fs::write(&staged, content.as_bytes()).unwrap();
        let (run1, _) = run_bound(strava_dispatch(&staged, "20260810_100000"), journal.path());
        assert_eq!(run1.exit_code, 0, "{}", run1.stdout);
        assert!(!staged.exists(), "the uploaded download is not kept");
        assert!(
            journal
                .path()
                .join("chronicle/20260810/import.strava/070000_300/workout.json")
                .is_file()
        );

        // A file the owner pointed the command at outside the journal's imports stays.
        let elsewhere = journal.path().join("activities.csv");
        fs::write(&elsewhere, content.as_bytes()).unwrap();
        let (run2, _) = run_bound(
            strava_dispatch(&elsewhere, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(elsewhere.is_file());
    }

    #[test]
    fn strava_steps_past_a_released_piece_and_stops_at_every_other_deletion() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[
            ("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0),
            ("102", "Aug 10, 2026, 09:00:00 AM", "Run", 300, 4000.0),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);
        let day = journal.path().join("chronicle/20260810/import.strava");
        // 101 released with its import; 102 deleted by the owner.
        release_tile(
            &day.join("070000_300"),
            br#"{"reason":"import_run_release","cid":"x"}"#,
        );
        release_tile(
            &day.join("090000_300"),
            br#"{"reason":"owner_segment_delete","cid":"x"}"#,
        );
        let released_before = fs::read(day.join("070000_300/tombstone.json")).unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0, "{}", run2.stdout);
        // The released workout comes back one key over; the owner's delete holds.
        assert!(day.join("070000_301/workout.json").is_file());
        assert!(!day.join("090000_301").exists());
        assert_eq!(
            fs::read(day.join("070000_300/tombstone.json")).unwrap(),
            released_before
        );
        assert_eq!(
            fs::read_dir(day.join("070000_300")).unwrap().count(),
            1,
            "nothing is written into a released key"
        );
        assert!(run2.stdout.contains(" deleted=1 "), "{}", run2.stdout);
        assert!(run2.stdout.contains("entries_written=1"), "{}", run2.stdout);
    }

    #[test]
    fn strava_stops_at_a_release_look_alike_and_fails_on_an_unreadable_tombstone() {
        for reason in [
            &br#"{"reason":"import_run_release "}"#[..],
            br#"{"reason":5}"#,
            br#"{"cid":"x"}"#,
            b"not json",
        ] {
            let journal = tempfile::tempdir().unwrap();
            write_utc_zone(journal.path());
            let csv_file = journal.path().join("activities.csv");
            let content =
                make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
            fs::write(&csv_file, content.as_bytes()).unwrap();
            let (run1, _) = run_bound(
                strava_dispatch(&csv_file, "20260810_100000"),
                journal.path(),
            );
            assert_eq!(run1.exit_code, 0);
            let day = journal.path().join("chronicle/20260810/import.strava");
            release_tile(&day.join("070000_300"), reason);
            let (run2, _) = run_bound(
                strava_dispatch(&csv_file, "20260810_110000"),
                journal.path(),
            );
            assert_eq!(run2.exit_code, 0);
            assert!(
                !day.join("070000_301").exists(),
                "stopped at {:?}",
                String::from_utf8_lossy(reason)
            );
        }

        // A tombstone that exists but can't be read fails the run, writing nothing.
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);
        let tile = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_300");
        for entry in fs::read_dir(&tile).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::create_dir(tile.join("tombstone.json")).unwrap();
        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_ne!(run2.exit_code, 0);
        assert!(
            !journal
                .path()
                .join("chronicle/20260810/import.strava/070000_301")
                .exists()
        );
    }

    #[test]
    fn strava_zone_record_permutations() {
        let journal = tempfile::tempdir().unwrap();
        write_custom_zone(journal.path(), "Asia/Tokyo");
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 600, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        // Run 1: seeds zone record Asia/Tokyo
        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);
        let zone_file = journal.path().join("imports/.strava-zone.json");
        assert!(
            fs::read_to_string(&zone_file)
                .unwrap()
                .contains("Asia/Tokyo")
        );

        // Journal identity changes to Denver, but import continues using Tokyo
        let zone_bytes_before = fs::read(&zone_file).unwrap();
        write_custom_zone(journal.path(), "America/Denver");
        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("zone=Asia/Tokyo"), "{}", run2.stdout);
        assert_eq!(fs::read(&zone_file).unwrap(), zone_bytes_before);
        assert!(!journal.path().join("chronicle/20260809").exists());
        assert!(
            fs::read_to_string(&zone_file)
                .unwrap()
                .contains("Asia/Tokyo")
        );

        // Invalid zone in record -> strava_zone_unrecognised
        fs::write(&zone_file, br#"{"zone":"Mars/Olympus"}"#).unwrap();
        let (run3, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_120000"),
            journal.path(),
        );
        assert_ne!(run3.exit_code, 0);
        assert!(
            run3.stderr.contains("strava_zone_unrecognised"),
            "{}",
            run3.stderr
        );

        // Corrupted JSON -> strava_zone_unrecognised
        fs::write(&zone_file, b"not-valid-json").unwrap();
        let (run4, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_130000"),
            journal.path(),
        );
        assert_ne!(run4.exit_code, 0);
        assert!(
            run4.stderr.contains("strava_zone_unrecognised"),
            "{}",
            run4.stderr
        );

        // Delete zone record: seeds from live tiles
        fs::remove_file(&zone_file).unwrap();
        let (run5, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_140000"),
            journal.path(),
        );
        assert_eq!(run5.exit_code, 0);
        assert!(
            fs::read_to_string(&zone_file)
                .unwrap()
                .contains("Asia/Tokyo")
        );

        // Two canonical tiles for workout 101 with different zones -> zone_conflict
        let day_dir = journal.path().join("chronicle/20260810/import.strava");
        let diff_zone_json = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20260810_010000",
            "activity_id": 101,
            "zone": "America/New_York",
            "tile": {
                "index": 1,
                "count": 2,
                "start": "2026-08-10T08:05:00-04:00",
                "end": "2026-08-10T08:10:00-04:00",
                "seconds": 300
            },
            "workout": {
                "name": "Run",
                "type": "Run",
                "start": "2026-08-10T08:00:00-04:00",
                "elapsed_seconds": 600,
                "moving_seconds": 600,
                "distance_m": 5000.0,
                "elevation_gain_m": 100.0,
                "heart_rate_avg_bpm": 140.0,
                "heart_rate_max_bpm": 160.0,
                "power_avg_w": 200.0,
                "power_weighted_w": 210.0,
                "calories_kcal": 350.0,
                "commute": true,
                "entered_by_hand": false
            }
        });
        fs::write(
            day_dir.join("070500_300/workout.json"),
            diff_zone_json.to_string(),
        )
        .unwrap();
        fs::remove_file(&zone_file).unwrap();
        let (run6, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_150000"),
            journal.path(),
        );
        assert_eq!(run6.exit_code, 0);
        assert!(run6.stdout.contains("zone_conflict=1"));

        // Seeding with two live tiles (Denver lower, Tokyo higher, no record, journal Tokyo)
        let journal_seed = tempfile::tempdir().unwrap();
        write_custom_zone(journal_seed.path(), "Asia/Tokyo");
        let day_dir_seed = journal_seed.path().join("chronicle/20260810/import.strava");
        fs::create_dir_all(day_dir_seed.join("070000_300")).unwrap();
        fs::create_dir_all(day_dir_seed.join("080000_300")).unwrap();
        let tile_denver = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20260810_010000",
            "activity_id": 201,
            "zone": "America/Denver",
            "tile": { "index": 0, "count": 1, "start": "2026-08-10T07:00:00-06:00", "end": "2026-08-10T07:05:00-06:00", "seconds": 300 },
            "workout": { "name": "Run", "type": "Run", "start": "2026-08-10T07:00:00-06:00", "elapsed_seconds": 300, "moving_seconds": 300, "distance_m": 1000.0, "elevation_gain_m": 0.0, "heart_rate_avg_bpm": null, "heart_rate_max_bpm": null, "power_avg_w": null, "power_weighted_w": null, "calories_kcal": null, "commute": false, "entered_by_hand": false }
        });
        let tile_tokyo = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20260810_010000",
            "activity_id": 202,
            "zone": "Asia/Tokyo",
            "tile": { "index": 0, "count": 1, "start": "2026-08-10T08:00:00+09:00", "end": "2026-08-10T08:05:00+09:00", "seconds": 300 },
            "workout": { "name": "Run", "type": "Run", "start": "2026-08-10T08:00:00+09:00", "elapsed_seconds": 300, "moving_seconds": 300, "distance_m": 1000.0, "elevation_gain_m": 0.0, "heart_rate_avg_bpm": null, "heart_rate_max_bpm": null, "power_avg_w": null, "power_weighted_w": null, "calories_kcal": null, "commute": false, "entered_by_hand": false }
        });
        fs::write(
            day_dir_seed.join("070000_300/workout.json"),
            tile_denver.to_string(),
        )
        .unwrap();
        fs::write(
            day_dir_seed.join("080000_300/workout.json"),
            tile_tokyo.to_string(),
        )
        .unwrap();
        let csv_seed = journal_seed.path().join("activities.csv");
        let content_seed =
            make_en_csv_103(&[("301", "Aug 10, 2026, 09:00:00 AM", "Run", 300, 1000.0)]);
        fs::write(&csv_seed, content_seed.as_bytes()).unwrap();
        let (run_seed, _) = run_bound(
            strava_dispatch(&csv_seed, "20260810_160000"),
            journal_seed.path(),
        );
        assert_eq!(run_seed.exit_code, 0);
        let seeded_rec =
            fs::read_to_string(journal_seed.path().join("imports/.strava-zone.json")).unwrap();
        assert!(seeded_rec.contains("America/Denver"));

        // New workout's zone and record match
        let new_tile_str = fs::read_to_string(
            journal_seed
                .path()
                .join("chronicle/20260810/import.strava/090000_300/workout.json"),
        )
        .unwrap();
        let new_tile_json: serde_json::Value = serde_json::from_str(&new_tile_str).unwrap();
        assert_eq!(new_tile_json["zone"], "America/Denver");

        // Third journal: zone Asia/Tokyo, no zone record, two live schema-v1 tiles on 20260810.
        // 070000_300 has "zone":"Mars/Olympus". 080000_300 has "zone":"America/Denver".
        // Import one new workout. The created imports/.strava-zone.json contains America/Denver and does not contain Asia/Tokyo.
        let journal_unp = tempfile::tempdir().unwrap();
        write_custom_zone(journal_unp.path(), "Asia/Tokyo");
        let day_dir_unp = journal_unp.path().join("chronicle/20260810/import.strava");
        fs::create_dir_all(day_dir_unp.join("070000_300")).unwrap();
        fs::create_dir_all(day_dir_unp.join("080000_300")).unwrap();
        let tile_mars = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20260810_010000",
            "activity_id": 201,
            "zone": "Mars/Olympus",
            "tile": { "index": 0, "count": 1, "start": "2026-08-10T07:00:00-06:00", "end": "2026-08-10T07:05:00-06:00", "seconds": 300 },
            "workout": { "name": "Run", "type": "Run", "start": "2026-08-10T07:00:00-06:00", "elapsed_seconds": 300, "moving_seconds": 300, "distance_m": 1000.0, "elevation_gain_m": 0.0, "heart_rate_avg_bpm": null, "heart_rate_max_bpm": null, "power_avg_w": null, "power_weighted_w": null, "calories_kcal": null, "commute": false, "entered_by_hand": false }
        });
        let tile_denver2 = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "import_id": "20260810_010000",
            "activity_id": 202,
            "zone": "America/Denver",
            "tile": { "index": 0, "count": 1, "start": "2026-08-10T08:00:00-06:00", "end": "2026-08-10T08:05:00-06:00", "seconds": 300 },
            "workout": { "name": "Run", "type": "Run", "start": "2026-08-10T08:00:00-06:00", "elapsed_seconds": 300, "moving_seconds": 300, "distance_m": 1000.0, "elevation_gain_m": 0.0, "heart_rate_avg_bpm": null, "heart_rate_max_bpm": null, "power_avg_w": null, "power_weighted_w": null, "calories_kcal": null, "commute": false, "entered_by_hand": false }
        });
        fs::write(
            day_dir_unp.join("070000_300/workout.json"),
            tile_mars.to_string(),
        )
        .unwrap();
        fs::write(
            day_dir_unp.join("080000_300/workout.json"),
            tile_denver2.to_string(),
        )
        .unwrap();
        let csv_unp = journal_unp.path().join("activities.csv");
        let content_unp =
            make_en_csv_103(&[("301", "Aug 10, 2026, 09:00:00 AM", "Run", 300, 1000.0)]);
        fs::write(&csv_unp, content_unp.as_bytes()).unwrap();
        let (run_unp, _) = run_bound(
            strava_dispatch(&csv_unp, "20260810_160000"),
            journal_unp.path(),
        );
        assert_eq!(run_unp.exit_code, 0);
        let seeded_rec_unp =
            fs::read_to_string(journal_unp.path().join("imports/.strava-zone.json")).unwrap();
        assert!(seeded_rec_unp.contains("America/Denver"));
        assert!(!seeded_rec_unp.contains("Asia/Tokyo"));
    }

    #[test]
    fn strava_partial_failure_and_recovery() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let target_seg = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_300");
        let blocker_fn = |p: &Path| {
            let seg = p.join("chronicle/20260810/import.strava/070000_300");
            fs::create_dir_all(seg.join("workout.json")).unwrap();
        };
        let hook1 = StravaRunHook {
            fault: None,
            after_lock: None,
            before_apply: Some(&blocker_fn),
            lock_timeout: None,
            stop_before_finish: false,
        };
        let mut selected = None;
        let dispatch1 = strava_dispatch(&csv_file, "20260810_100000");
        let run1 = run_strava_hooked(dispatch1, journal.path(), &mut selected, Some(&hook1));
        assert_ne!(run1.exit_code, 0);
        assert!(journal.path().join("imports/.strava-zone.json").is_file());
        assert!(
            !journal
                .path()
                .join("imports/20260810_100000/manifest.json")
                .exists()
        );

        fs::remove_dir_all(&target_seg).unwrap();

        let (run2, id2) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0, "{}", run2.stderr);
        assert!(run2.stdout.contains("entries_written=1"));
        let manifest_path = journal
            .path()
            .join(format!("imports/{}/manifest.json", id2.unwrap()));
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["entry_count"], 1);
    }

    #[test]
    fn strava_stop_before_finish_and_stream_republish() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[
            ("101", "Aug 10, 2026, 07:00:00 AM", "Run 1", 300, 5000.0),
            ("102", "Aug 10, 2026, 08:00:00 AM", "Run 2", 300, 5000.0),
        ]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let hook1 = StravaRunHook {
            fault: None,
            after_lock: None,
            before_apply: None,
            lock_timeout: None,
            stop_before_finish: true,
        };
        let mut selected = None;
        let dispatch1 = strava_dispatch(&csv_file, "20260810_100000");
        let run1 = run_strava_hooked(dispatch1, journal.path(), &mut selected, Some(&hook1));
        assert_eq!(run1.exit_code, 0);

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("republished="));

        let stream_file = journal
            .path()
            .join("chronicle/20260810/import.strava/080000_300/stream.json");
        fs::remove_file(&stream_file).unwrap();
        let (run3, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_120000"),
            journal.path(),
        );
        assert_eq!(run3.exit_code, 0);
        assert!(stream_file.is_file());

        let mid_marker = journal
            .path()
            .join("chronicle/20260810/import.strava/070000_300/stream.json");
        fs::remove_file(&mid_marker).unwrap();
        let (run4, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_130000"),
            journal.path(),
        );
        assert_eq!(run4.exit_code, 0);
        assert!(run4.stdout.contains("marker_missing_in_chain=1"));
    }

    #[test]
    fn strava_dry_run_preview_tokens() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let mut dispatch_dry = strava_dispatch(&csv_file, "20260810_100000");
        dispatch_dry.dry_run = true;
        let (run_dry, id_dry) = run_bound(dispatch_dry, journal.path());
        assert_eq!(run_dry.exit_code, 0);
        assert!(id_dry.is_none());
        assert!(run_dry.stdout.contains("new=1 tiles_to_create=1"));
        assert!(!journal.path().join("chronicle").exists());
        assert!(!journal.path().join("imports").exists());

        let (run_real, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run_real.exit_code, 0);
        assert!(run_real.stdout.contains("new=1 tiles_to_create=1"));
    }

    #[test]
    fn strava_post_lock_faults_and_lock_unavailable() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0)]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (init, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_090000"),
            journal.path(),
        );
        assert_eq!(init.exit_code, 0);

        for (idx, (fault_kind, expected_code)) in [
            (strava::FaultKind::WorkoutFile, "strava_tile_unreadable"),
            (strava::FaultKind::DayList, "strava_day_unreadable"),
            (strava::FaultKind::OwnerDeleted, "strava_probe_unreadable"),
            (strava::FaultKind::StreamMarker, "strava_marker_unreadable"),
            (
                strava::FaultKind::StreamRecord,
                "strava_stream_record_unreadable",
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let fault = strava::ReadFault {
                kind: fault_kind,
                fail_on: 2,
                seen: std::cell::Cell::new(0),
            };
            let hook = StravaRunHook {
                fault: Some(&fault),
                after_lock: None,
                before_apply: None,
                lock_timeout: None,
                stop_before_finish: false,
            };
            let mut selected = None;
            let ts = format!("20260810_10000{}", idx);
            let dispatch = strava_dispatch(&csv_file, &ts);
            let run = run_strava_hooked(dispatch, journal.path(), &mut selected, Some(&hook));
            assert_ne!(run.exit_code, 0);
            assert!(
                run.stderr.contains(expected_code),
                "expected {} in {}",
                expected_code,
                run.stderr
            );
            if fault_kind == strava::FaultKind::DayList {
                assert!(
                    run.stderr.contains("20260810"),
                    "expected day in stderr: {}",
                    run.stderr
                );
            }
            assert!(!journal.path().join("imports").join(&ts).exists());
        }

        let plant_unparseable_marker = |p: &Path| {
            let m = p.join("chronicle/20260810/import.strava/070000_300/stream.json");
            fs::write(m, b"not valid json").unwrap();
        };
        let hook_marker = StravaRunHook {
            fault: None,
            after_lock: Some(&plant_unparseable_marker),
            before_apply: None,
            lock_timeout: None,
            stop_before_finish: false,
        };
        let mut selected = None;
        let run_marker = run_strava_hooked(
            strava_dispatch(&csv_file, "20260810_110000"),
            journal.path(),
            &mut selected,
            Some(&hook_marker),
        );
        assert_ne!(run_marker.exit_code, 0);
        assert!(
            run_marker.stderr.contains("strava_marker_unparseable"),
            "{}",
            run_marker.stderr
        );

        // Lock unavailable on separate journal
        let journal_lock = tempfile::tempdir().unwrap();
        write_utc_zone(journal_lock.path());
        let csv_file_lock = journal_lock.path().join("activities.csv");
        fs::write(&csv_file_lock, content.as_bytes()).unwrap();

        let hook_lock = StravaRunHook {
            fault: None,
            after_lock: None,
            before_apply: None,
            lock_timeout: Some(std::time::Duration::from_millis(0)),
            stop_before_finish: false,
        };
        let _held_lock = solstone_core_journal_io::locking::hold_lock(
            journal_lock.path().join("imports/.strava"),
            solstone_core_journal_io::locking::LockOptions::default(),
        )
        .unwrap();
        let mut selected = None;
        let run_lock = run_strava_hooked(
            strava_dispatch(&csv_file_lock, "20260810_120000"),
            journal_lock.path(),
            &mut selected,
            Some(&hook_lock),
        );
        assert_ne!(run_lock.exit_code, 0);
        assert!(
            run_lock.stderr.contains("strava_lock_unavailable"),
            "{}",
            run_lock.stderr
        );
    }

    #[test]
    fn strava_midnight_spanning_workout_completed() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities.csv");

        let content = make_en_csv_103(&[(
            "101",
            "Aug 10, 2026, 11:58:00 PM",
            "Midnight Run",
            600,
            5000.0,
        )]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let blocker = |p: &Path| {
            let seg = p.join("chronicle/20260811/import.strava/000300_300");
            fs::create_dir_all(seg.join("workout.json")).unwrap();
        };
        let hook1 = StravaRunHook {
            fault: None,
            after_lock: None,
            before_apply: Some(&blocker),
            lock_timeout: None,
            stop_before_finish: false,
        };
        let mut selected = None;
        let run1 = run_strava_hooked(
            strava_dispatch(&csv_file, "20260811_010000"),
            journal.path(),
            &mut selected,
            Some(&hook1),
        );
        assert_ne!(run1.exit_code, 0);

        let seg_dplus1 = journal
            .path()
            .join("chronicle/20260811/import.strava/000300_300");
        fs::remove_dir_all(&seg_dplus1).unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file, "20260811_020000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);
        assert!(run2.stdout.contains("complete=1"));
    }

    #[test]
    fn strava_zone_persisted_and_locked_on_first_import() {
        let journal = tempfile::tempdir().unwrap();
        write_custom_zone(journal.path(), "America/Denver");
        let csv_file = journal.path().join("activities.csv");
        let content = make_en_csv_103(&[(
            "701",
            "Aug 10, 2026, 06:30:00 AM",
            "Denver Run",
            300,
            5000.0,
        )]);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (run1, _) = run_bound(
            strava_dispatch(&csv_file, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);

        let zone_record_path = journal.path().join("imports/.strava-zone.json");
        let zone_record_str = fs::read_to_string(&zone_record_path).unwrap();
        assert!(zone_record_str.contains("America/Denver"));

        // Change journal identity timezone to America/New_York
        write_custom_zone(journal.path(), "America/New_York");

        let content2 = make_en_csv_103(&[(
            "702",
            "Aug 11, 2026, 06:30:00 AM",
            "Second Run",
            300,
            5000.0,
        )]);
        let csv_file2 = journal.path().join("activities2.csv");
        fs::write(&csv_file2, content2.as_bytes()).unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file2, "20260811_100000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);

        // Zone record remains America/Denver
        let zone_record_str2 = fs::read_to_string(&zone_record_path).unwrap();
        assert_eq!(zone_record_str, zone_record_str2);
    }

    // A 500-workout run takes most of a minute unoptimized, so it runs with the
    // full suite rather than the routine one.
    #[cfg(feature = "full-tests")]
    #[test]
    fn strava_scale_500_workouts() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file = journal.path().join("activities_scale.csv");

        let base_date = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let mut row_data = Vec::with_capacity(500);
        let mut id_strings = Vec::with_capacity(500);
        let mut date_strings = Vec::with_capacity(500);
        let mut name_strings = Vec::with_capacity(500);

        for i in 1..=500 {
            let cur_date = base_date + chrono::Duration::days((i % 100) as i64);
            let date_str = cur_date.format("%d %b %Y, 07:00:00").to_string();
            let id_str = format!("{}", 1000 + i);
            let name_str = format!("Workout {i}");
            id_strings.push(id_str);
            date_strings.push(date_str);
            name_strings.push(name_str);
        }

        for i in 0..500 {
            row_data.push((
                id_strings[i].as_str(),
                date_strings[i].as_str(),
                name_strings[i].as_str(),
                300,
                5000.0,
            ));
        }

        let content = make_en_csv_103(&row_data);
        fs::write(&csv_file, content.as_bytes()).unwrap();

        let (run, id) = run_bound(
            strava_dispatch(&csv_file, "20260501_120000"),
            journal.path(),
        );

        assert_eq!(run.exit_code, 0, "{}", run.stderr);
        assert!(id.is_some());
        assert!(run.stdout.contains("entries_written=500"));
    }

    #[test]
    fn strava_deleted_head_publication() {
        let journal = tempfile::tempdir().unwrap();
        write_utc_zone(journal.path());
        let csv_file1 = journal.path().join("activities1.csv");
        let content1 = make_en_csv_103(&[
            ("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0),
            ("102", "Aug 10, 2026, 08:00:00 AM", "Run", 300, 5000.0),
        ]);
        fs::write(&csv_file1, content1.as_bytes()).unwrap();

        let (run1, _) = run_bound(
            strava_dispatch(&csv_file1, "20260810_090000"),
            journal.path(),
        );
        assert_eq!(run1.exit_code, 0);

        let stream_head =
            solstone_core_segment::read_stream_record(journal.path(), "import.strava")
                .unwrap()
                .unwrap();
        assert_eq!(stream_head["last_segment"], "080000_300");
        assert_eq!(stream_head["seq"], 2);

        let targets = vec![
            solstone_core_retention::receipt::Target {
                day: "20260810".into(),
                stream: "import.strava".into(),
                dir: "070000_300".into(),
            },
            solstone_core_retention::receipt::Target {
                day: "20260810".into(),
                stream: "import.strava".into(),
                dir: "080000_300".into(),
            },
        ];
        let outcome = solstone_core_retention::door::remove_segments(
            journal.path(),
            &targets,
            "2026-08-10T12:00:00Z",
            solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
            "sha256:abc",
        );
        assert!(outcome.halted.is_none());

        let csv_file2 = journal.path().join("activities2.csv");
        let content2 = make_en_csv_103(&[
            ("101", "Aug 10, 2026, 07:00:00 AM", "Run", 300, 5000.0),
            ("102", "Aug 10, 2026, 08:00:00 AM", "Run", 300, 5000.0),
            ("103", "Aug 10, 2026, 09:00:00 AM", "Run", 300, 5000.0),
        ]);
        fs::write(&csv_file2, content2.as_bytes()).unwrap();

        let (run2, _) = run_bound(
            strava_dispatch(&csv_file2, "20260810_100000"),
            journal.path(),
        );
        assert_eq!(run2.exit_code, 0);

        let marker_103_path = journal
            .path()
            .join("chronicle/20260810/import.strava/090000_300/stream.json");
        let marker_103: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker_103_path).unwrap()).unwrap();
        assert_eq!(marker_103["seq"], 3);
        assert_eq!(marker_103["prev_segment"], "080000_300");
        assert_eq!(marker_103["prev_day"], "20260810");

        assert!(
            !journal
                .path()
                .join("chronicle/20260810/import.strava/070000_300/stream.json")
                .exists()
        );
        assert!(
            !journal
                .path()
                .join("chronicle/20260810/import.strava/080000_300/stream.json")
                .exists()
        );

        record_finished_import(journal.path(), "20260810_100000", 0);

        let pub_record = solstone_core_import::publish::read_publication_record(
            &journal.path().join("imports/20260810_100000"),
        )
        .unwrap()
        .unwrap();
        assert!(pub_record.day_markers.is_empty());
    }
}
