// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Historical-day scanning and dispatch for `journal sense --day`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, CallosumSocketConnection};
use solstone_core_journal_io::{
    DEFAULT_STREAM, PathOrDay, Segment, SegmentIdentityError, bump_stream_marker,
    check_record_identities, iter_segments, sync_dir,
};
use solstone_core_processing_record::{
    MediaKind, media_kind, read_processing_record_header, should_reenter_analysis_output,
};
use thiserror::Error;

use crate::config::{read_config, resolve_concurrency};
use crate::dispatch::{BatchMarkerPolicy, Outbound, SenseDispatcher};

/// Existing output classes that can be deleted before a batch reprocess.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReprocessKind {
    Screen,
    Audio,
    All,
}

impl ReprocessKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Audio => "audio",
            Self::All => "all",
        }
    }
}

/// Parsed owner-facing inputs for a finite historical-day run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchRequest {
    pub day: String,
    pub jobs: i64,
    pub reprocess: Option<ReprocessKind>,
    pub segment: Option<String>,
    pub stream: Option<String>,
    pub dry_run: bool,
    pub verbose: bool,
    pub debug: bool,
}

/// A source file found by a batch scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchWork {
    pub path: PathBuf,
    pub handler: &'static str,
    pub stream: String,
    pub segment: String,
}

#[derive(Debug, Error)]
pub enum BatchError {
    #[error(transparent)]
    Paths(#[from] solstone_core_journal_io::PathError),
    #[error("could not scan {path}: {source}")]
    Scan {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not delete {path}: {source}")]
    Delete {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("reprocess removed {path}, but could not durably mark day {day} dirty: {detail}")]
    PostDelete {
        path: PathBuf,
        day: String,
        detail: String,
    },
    #[error("batch callosum runtime unavailable")]
    Runtime,
    #[error("sense batch timed out after {timeout:?}")]
    TimedOut { timeout: Duration },
    #[error("{failed} failed of {ran} ran")]
    Failed { failed: usize, ran: usize },
    #[error("sense batch refused: {reason}")]
    Unrepresentable { reason: String },
}

impl From<SegmentIdentityError> for BatchError {
    fn from(error: SegmentIdentityError) -> Self {
        Self::Unrepresentable {
            reason: error.to_string(),
        }
    }
}

fn selected_segments<'a>(
    segments: &'a [Segment],
    segment_filter: Option<&str>,
    stream_filter: Option<&str>,
) -> Vec<&'a Segment> {
    segments
        .iter()
        .filter(|segment| stream_filter.is_none_or(|value| segment.stream().matches(value)))
        .filter(|segment| segment_filter.is_none_or(|value| segment.key() == value))
        .collect()
}

/// Run one finite historical-day batch.
pub fn run_batch(journal: &Path, request: &BatchRequest) -> Result<(), BatchError> {
    run_batch_with_environment(journal, request, &BTreeMap::new())
}

/// Run one finite historical-day batch with scoped native-child environment.
pub fn run_batch_with_environment(
    journal: &Path,
    request: &BatchRequest,
    child_environment: &BTreeMap<OsString, OsString>,
) -> Result<(), BatchError> {
    run_batch_with_environment_and_timeout(journal, request, child_environment, None)
}

/// Run one finite historical-day batch with scoped native-child environment
/// and an optional aggregate deadline. A timeout stops the dispatcher and
/// reaps its managed child processes before returning [`BatchError::TimedOut`].
pub fn run_batch_with_environment_and_timeout(
    journal: &Path,
    request: &BatchRequest,
    child_environment: &BTreeMap<OsString, OsString>,
    timeout: Option<Duration>,
) -> Result<(), BatchError> {
    run_batch_with_environment_and_timeout_with_marker_policy(
        journal,
        request,
        child_environment,
        timeout,
        BatchMarkerPolicy::AdvanceStream,
    )
}

/// Run Sense as one phase enclosed by a whole-day lifecycle. The enclosing
/// finalizer owns the single stream/daily marker transition for that attempt.
pub fn run_batch_for_whole_day_with_environment_and_timeout(
    journal: &Path,
    request: &BatchRequest,
    child_environment: &BTreeMap<OsString, OsString>,
    timeout: Option<Duration>,
) -> Result<(), BatchError> {
    run_batch_with_environment_and_timeout_with_marker_policy(
        journal,
        request,
        child_environment,
        timeout,
        BatchMarkerPolicy::EnclosedWholeDay,
    )
}

fn run_batch_with_environment_and_timeout_with_marker_policy(
    journal: &Path,
    request: &BatchRequest,
    child_environment: &BTreeMap<OsString, OsString>,
    timeout: Option<Duration>,
    marker_policy: BatchMarkerPolicy,
) -> Result<(), BatchError> {
    let deadline = timeout.map(BatchDeadline::new);
    check_deadline(deadline)?;
    let day_dir = journal.join("chronicle").join(&request.day);
    if !day_dir.exists() {
        println!("Day directory not found: {}", day_dir.display());
        return Ok(());
    }

    let listed = iter_segments(journal, PathOrDay::Directory(&day_dir))?;
    check_record_identities(selected_segments(
        &listed,
        request.segment.as_deref(),
        request.stream.as_deref(),
    ))?;
    check_deadline(deadline)?;

    if let Some(reprocess) = request.reprocess {
        let deleted = delete_outputs(
            journal,
            &day_dir,
            reprocess,
            request.segment.as_deref(),
            request.stream.as_deref(),
            request.dry_run,
        )?;
        check_deadline(deadline)?;
        if request.dry_run {
            if deleted.is_empty() {
                println!("No files to delete");
            } else {
                println!("Would delete {} output file(s):", deleted.len());
                for path in deleted {
                    println!("  {}", journal_relative_path(journal, &path));
                }
            }
            return Ok(());
        }
        println!("Deleted {} output file(s)", deleted.len());
    }

    let modality = match request.reprocess {
        Some(ReprocessKind::Audio) => Some(ReprocessKind::Audio),
        Some(ReprocessKind::Screen) => Some(ReprocessKind::Screen),
        Some(ReprocessKind::All) | None => None,
    };
    if request.dry_run {
        let work = scan_unprocessed(
            journal,
            &day_dir,
            request.segment.as_deref(),
            request.stream.as_deref(),
            modality,
        )?;
        check_deadline(deadline)?;
        print_dry_run(journal, &work);
        return Ok(());
    }

    let segment_message = request
        .segment
        .as_deref()
        .map(|segment| format!(" (segment: {segment})"))
        .unwrap_or_default();
    println!(
        "Processing files from day {}{} with {} concurrent jobs",
        request.day, segment_message, request.jobs
    );
    process_day_with_environment(
        journal,
        request,
        modality,
        child_environment,
        deadline,
        marker_policy,
    )
}

/// Install the finite-run signal path without changing the event service's
/// graceful shutdown handler. SIGTERM follows the shell-visible 128+15 rule.
pub fn install_batch_signal_handlers() {
    #[cfg(unix)]
    {
        let _ = std::thread::Builder::new()
            .name("sense-batch-signals".into())
            .spawn(|| {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async {
                    let Ok(mut terminate) =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    else {
                        return;
                    };
                    tokio::select! {
                        _ = terminate.recv() => std::process::exit(143),
                        _ = tokio::signal::ctrl_c() => std::process::exit(130),
                    }
                });
            });
    }
}

/// Scan a day for matching source files whose sidecar does not prove completion.
pub fn scan_unprocessed(
    journal: &Path,
    day_dir: &Path,
    segment_filter: Option<&str>,
    stream_filter: Option<&str>,
    modality_filter: Option<ReprocessKind>,
) -> Result<Vec<BatchWork>, BatchError> {
    let mut work = Vec::new();
    for segment in iter_segments(journal, PathOrDay::Directory(day_dir))? {
        if stream_filter.is_some_and(|value| !segment.stream().matches(value)) {
            continue;
        }
        if segment_filter.is_some_and(|value| value != segment.key()) {
            continue;
        }
        let identity = segment.record_identity()?;
        let entries = fs::read_dir(segment.path()).map_err(|source| BatchError::Scan {
            path: segment.path().to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| BatchError::Scan {
                path: segment.path().to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| BatchError::Scan {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                continue;
            }
            let Some(handler) = handler_for_path(&path) else {
                continue;
            };
            if !matches_modality(&path, modality_filter) {
                continue;
            }
            let output = path.with_extension("jsonl");
            if output.exists()
                && !should_reenter_analysis_output(
                    read_processing_record_header(&output).as_ref(),
                    &output,
                    handler,
                )
            {
                continue;
            }
            if handler == "depict" && identity.stream.starts_with("import.") {
                continue;
            }
            work.push(BatchWork {
                path,
                handler,
                stream: identity.stream.to_owned(),
                segment: identity.key.to_owned(),
            });
        }
    }
    work.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(work)
}

/// Delete selected JSONL outputs, or list them when `dry_run` is true.
pub fn delete_outputs(
    journal: &Path,
    day_dir: &Path,
    reprocess: ReprocessKind,
    segment_filter: Option<&str>,
    stream_filter: Option<&str>,
    dry_run: bool,
) -> Result<Vec<PathBuf>, BatchError> {
    if !day_dir.exists() {
        return Ok(Vec::new());
    }
    let listed = iter_segments(journal, PathOrDay::Directory(day_dir))?;
    check_record_identities(selected_segments(&listed, segment_filter, stream_filter))?;
    let mut deleted = Vec::new();
    for segment in listed {
        if stream_filter.is_some_and(|value| !segment.stream().matches(value)) {
            continue;
        }
        if segment_filter.is_some_and(|value| value != segment.key()) {
            continue;
        }
        for entry in fs::read_dir(segment.path()).map_err(|source| BatchError::Scan {
            path: segment.path().to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| BatchError::Scan {
                path: segment.path().to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if !entry
                .file_type()
                .map_err(|source| BatchError::Scan {
                    path: path.clone(),
                    source,
                })?
                .is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("jsonl")
            {
                continue;
            }
            let selected = match reprocess {
                // This branch deliberately does not lowercase the output stem or
                // source extension. It proves source correspondence by exact name.
                ReprocessKind::All => has_exact_media_source(segment.path(), &path),
                // These branches deliberately lowercase only the JSONL stem.
                ReprocessKind::Screen => screen_output_name(&path),
                ReprocessKind::Audio => audio_output_name(&path),
            };
            if !selected {
                continue;
            }
            deleted.push(path.clone());
            if !dry_run {
                let parent = path
                    .parent()
                    .and_then(|parent| parent.strip_prefix(journal).ok())
                    .and_then(Path::to_str)
                    .ok_or_else(|| BatchError::Unrepresentable {
                        reason: format!(
                            "reprocess output parent is not journal-relative UTF-8: {}",
                            path.display()
                        ),
                    })?;
                let day = day_dir
                    .file_name()
                    .and_then(|value| value.to_str())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| BatchError::Unrepresentable {
                        reason: format!(
                            "reprocess day directory has no UTF-8 day key: {}",
                            day_dir.display()
                        ),
                    })?
                    .to_owned();
                fs::remove_file(&path).map_err(|source| BatchError::Delete {
                    path: path.clone(),
                    source,
                })?;
                // The unlink and its dirty transition precede every later
                // fallible scan/deadline/dispatcher step. A failed marker is
                // terminal, but cannot pretend the already-removed output was
                // restored.
                let sync = sync_dir(journal, parent).map_err(|error| error.to_string());
                let marker = bump_stream_marker(journal, &day).map_err(|error| error.to_string());
                if sync.is_err() || marker.is_err() {
                    let mut details = Vec::new();
                    if let Err(error) = sync {
                        details.push(format!("output directory sync failed: {error}"));
                    }
                    if let Err(error) = marker {
                        details.push(format!("stream marker write failed: {error}"));
                    }
                    return Err(BatchError::PostDelete {
                        path,
                        day,
                        detail: details.join("; "),
                    });
                }
                println!("Deleted: {}", path.display());
            }
        }
    }
    deleted.sort();
    Ok(deleted)
}

/// Dispatch a scanned day through the native handler worker/process path.
pub fn process_day(
    journal: &Path,
    request: &BatchRequest,
    modality_filter: Option<ReprocessKind>,
) -> Result<(), BatchError> {
    process_day_with_environment(
        journal,
        request,
        modality_filter,
        &BTreeMap::new(),
        None,
        BatchMarkerPolicy::AdvanceStream,
    )
}

fn process_day_with_environment(
    journal: &Path,
    request: &BatchRequest,
    modality_filter: Option<ReprocessKind>,
    child_environment: &BTreeMap<OsString, OsString>,
    deadline: Option<BatchDeadline>,
    marker_policy: BatchMarkerPolicy,
) -> Result<(), BatchError> {
    process_day_with_dispatcher(
        journal,
        request,
        modality_filter,
        |outbound, describe_workers| {
            SenseDispatcher::new_batch_with_environment(
                journal.to_path_buf(),
                request.verbose,
                request.debug,
                outbound,
                describe_workers,
                child_environment.clone(),
                marker_policy,
            )
        },
        deadline,
    )
}

#[cfg(feature = "test-stubs")]
/// Dispatch a batch through the built fixture handler while preserving the
/// production callosum lifecycle.
pub fn process_day_with_fixture_program(
    journal: &Path,
    request: &BatchRequest,
    modality_filter: Option<ReprocessKind>,
    program: PathBuf,
) -> Result<(), BatchError> {
    process_day_with_fixture_program_and_timeout(journal, request, modality_filter, program, None)
}

#[cfg(feature = "test-stubs")]
/// Dispatch a fixture-backed batch with an optional aggregate deadline.
pub fn process_day_with_fixture_program_and_timeout(
    journal: &Path,
    request: &BatchRequest,
    modality_filter: Option<ReprocessKind>,
    program: PathBuf,
    timeout: Option<Duration>,
) -> Result<(), BatchError> {
    let deadline = timeout.map(BatchDeadline::new);
    process_day_with_dispatcher(
        journal,
        request,
        modality_filter,
        |outbound, describe_workers| {
            SenseDispatcher::new_batch_with_fixture_program(
                journal.to_path_buf(),
                request.verbose,
                request.debug,
                outbound,
                describe_workers,
                program,
                BatchMarkerPolicy::AdvanceStream,
            )
        },
        deadline,
    )
}

fn process_day_with_dispatcher<F>(
    journal: &Path,
    request: &BatchRequest,
    modality_filter: Option<ReprocessKind>,
    make_dispatcher: F,
    deadline: Option<BatchDeadline>,
) -> Result<(), BatchError>
where
    F: FnOnce(mpsc::Sender<Outbound>, usize) -> SenseDispatcher,
{
    check_deadline(deadline)?;
    let day_dir = journal.join("chronicle").join(&request.day);
    let work = scan_unprocessed(
        journal,
        &day_dir,
        request.segment.as_deref(),
        request.stream.as_deref(),
        modality_filter,
    )?;
    check_deadline(deadline)?;
    if work.is_empty() {
        println!("No unprocessed files found in {}", day_dir.display());
        return Ok(());
    }
    print_processing_summary(&work);

    let config = read_config(journal);
    let describe_workers = batch_describe_workers(&config, request.jobs);

    let (outbound, receiver) = mpsc::channel::<Outbound>();
    let dispatcher = Arc::new(make_dispatcher(outbound, describe_workers));
    let messages = group_by_segment(&work)
        .into_iter()
        .map(|(stream, segment, files)| {
            let mut extra = Map::from_iter([
                ("day".into(), Value::String(request.day.clone())),
                ("segment".into(), Value::String(segment)),
                (
                    "files".into(),
                    Value::Array(files.into_iter().map(Value::String).collect()),
                ),
                ("batch".into(), Value::Bool(true)),
            ]);
            if let Some(stream) = stream {
                extra.insert("stream".into(), Value::String(stream));
            }
            CallosumEnvelope {
                tract: "observe".into(),
                event: "observing".into(),
                ts: None,
                extra,
            }
        })
        .collect::<Vec<_>>();
    run_batch_dispatcher(
        journal,
        Arc::clone(&dispatcher),
        receiver,
        messages,
        deadline,
    )?;
    let (failed, ran) = dispatcher.tally.snapshot();
    if failed > 0 {
        return Err(BatchError::Failed { failed, ran });
    }
    println!("Batch processing complete");
    Ok(())
}

fn run_batch_dispatcher(
    journal: &Path,
    dispatcher: Arc<SenseDispatcher>,
    receiver: mpsc::Receiver<Outbound>,
    messages: Vec<CallosumEnvelope>,
    deadline: Option<BatchDeadline>,
) -> Result<(), BatchError> {
    let connection =
        CallosumSocketConnection::new(journal.join("health/callosum.sock"), Map::new());
    let dispatching = Arc::clone(&dispatcher);
    let timeout_limit = deadline.map(|value| value.limit);
    let timed_out = Arc::new(AtomicBool::new(false));
    let timeout_result = Arc::clone(&timed_out);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("sense-batch")
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            dispatcher.stop_and_wait();
            return Err(BatchError::Runtime);
        }
    };
    runtime.block_on(crate::service::run_until(
        connection,
        dispatcher,
        receiver,
        async move {
            let remaining = deadline.map(BatchDeadline::remaining);
            let wait_until_idle = async move {
                for message in messages {
                    dispatching.handle(&message);
                    tokio::task::yield_now().await;
                }
                while !dispatching.is_idle() {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            };
            if let Some(remaining) = remaining {
                if remaining.is_zero()
                    || tokio::time::timeout(remaining, wait_until_idle)
                        .await
                        .is_err()
                {
                    timeout_result.store(true, Ordering::SeqCst);
                }
            } else {
                wait_until_idle.await;
            }
        },
    ));
    if timed_out.load(Ordering::SeqCst) {
        return Err(BatchError::TimedOut {
            timeout: timeout_limit.expect("timed out batch has a limit"),
        });
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct BatchDeadline {
    limit: Duration,
    started: Instant,
}

impl BatchDeadline {
    fn new(limit: Duration) -> Self {
        Self {
            limit,
            started: Instant::now(),
        }
    }

    fn remaining(self) -> Duration {
        self.limit.saturating_sub(self.started.elapsed())
    }
}

fn check_deadline(deadline: Option<BatchDeadline>) -> Result<(), BatchError> {
    if let Some(deadline) = deadline
        && deadline.started.elapsed() >= deadline.limit
    {
        return Err(BatchError::TimedOut {
            timeout: deadline.limit,
        });
    }
    Ok(())
}

fn group_by_segment(work: &[BatchWork]) -> Vec<(Option<String>, String, Vec<String>)> {
    let mut groups = BTreeMap::<(Option<String>, String), Vec<String>>::new();
    for item in work {
        let Some(name) = item.path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        groups
            .entry((
                (item.stream != DEFAULT_STREAM).then(|| item.stream.clone()),
                item.segment.clone(),
            ))
            .or_default()
            .push(name.to_owned());
    }
    groups
        .into_iter()
        .map(|((stream, segment), files)| (stream, segment, files))
        .collect()
}

fn print_dry_run(journal: &Path, work: &[BatchWork]) {
    if work.is_empty() {
        println!("No unprocessed files found");
        return;
    }
    let breakdown = extension_breakdown(work);
    println!("Would process {} file(s) ({breakdown}):", work.len());
    for item in work {
        println!("  {}", journal_relative_path(journal, &item.path));
    }
}

fn print_processing_summary(work: &[BatchWork]) {
    println!(
        "Found {} unprocessed files to process ({})",
        work.len(),
        extension_breakdown(work)
    );
}

fn extension_breakdown(work: &[BatchWork]) -> String {
    let mut counts = BTreeMap::<String, usize>::new();
    for item in work {
        let extension = item
            .path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{}", value.to_ascii_lowercase()))
            .unwrap_or_default();
        *counts.entry(extension).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(extension, count)| format!("{count} {extension}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn journal_relative_path(journal: &Path, path: &Path) -> String {
    path.strip_prefix(journal)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn matches_modality(path: &Path, modality: Option<ReprocessKind>) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    match modality {
        Some(ReprocessKind::Audio) => media_kind(extension) == Some(MediaKind::Audio),
        Some(ReprocessKind::Screen) => media_kind(extension) == Some(MediaKind::Video),
        Some(ReprocessKind::All) | None => true,
    }
}

fn handler_for_path(path: &Path) -> Option<&'static str> {
    match media_kind(path.extension()?.to_str()?)? {
        MediaKind::Audio => Some("transcribe"),
        MediaKind::Video => Some("describe"),
        MediaKind::Image => Some("depict"),
    }
}

fn has_exact_media_source(segment: &Path, output: &Path) -> bool {
    let Some(stem) = output.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    fs::read_dir(segment).ok().is_some_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry.path().file_stem().and_then(|value| value.to_str()) == Some(stem)
                && matches!(
                    entry
                        .path()
                        .extension()
                        .and_then(|value| value.to_str())
                        .and_then(media_kind),
                    Some(MediaKind::Audio | MediaKind::Video)
                )
        })
    })
}

fn screen_output_name(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    stem.ends_with("_screen") || stem == "screen"
}

fn audio_output_name(path: &Path) -> bool {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    stem.ends_with("_audio") || stem == "audio"
}

fn batch_describe_workers(config: &Map<String, Value>, jobs: i64) -> usize {
    let configured = resolve_concurrency(config, "describe");
    usize::try_from(jobs).unwrap_or(1).max(1).max(configured)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

    use super::*;

    fn segment(root: &Path) -> PathBuf {
        let path = root.join("chronicle/20260812/capture/120000_1");
        fs::create_dir_all(&path).expect("segment");
        path
    }

    #[test]
    fn all_reprocess_uses_exact_source_name_while_screen_lowercases_only_its_stem() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        fs::write(path.join("Screen.jsonl"), "{}\n").expect("sidecar");
        fs::write(path.join("screen.WEBM"), "video").expect("source");

        let all = delete_outputs(
            temp.path(),
            &temp.path().join("chronicle/20260812"),
            ReprocessKind::All,
            None,
            None,
            true,
        )
        .expect("all scan");
        assert!(
            all.is_empty(),
            "all must not lowercase source correspondence"
        );
        let screen = delete_outputs(
            temp.path(),
            &temp.path().join("chronicle/20260812"),
            ReprocessKind::Screen,
            None,
            None,
            true,
        )
        .expect("screen scan");
        assert_eq!(screen, vec![path.join("Screen.jsonl")]);
    }

    #[test]
    fn suffix_reprocess_selects_orphaned_outputs_but_all_does_not() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        let output = path.join("orphan_screen.jsonl");
        fs::write(&output, "{}\n").expect("sidecar");
        let day = temp.path().join("chronicle/20260812");

        assert!(
            delete_outputs(temp.path(), &day, ReprocessKind::All, None, None, true)
                .expect("all scan")
                .is_empty()
        );
        assert_eq!(
            delete_outputs(temp.path(), &day, ReprocessKind::Screen, None, None, true)
                .expect("screen scan"),
            vec![output]
        );
    }

    #[test]
    fn all_reprocess_selects_nonconforming_source_pairs_that_suffix_modes_skip() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        let output = path.join("meeting.jsonl");
        fs::write(&output, "{}\n").expect("sidecar");
        fs::write(path.join("meeting.mp4"), "video").expect("source");
        let day = temp.path().join("chronicle/20260812");

        assert_eq!(
            delete_outputs(temp.path(), &day, ReprocessKind::All, None, None, true)
                .expect("all scan"),
            vec![output.clone()]
        );
        assert!(
            delete_outputs(temp.path(), &day, ReprocessKind::Screen, None, None, true)
                .expect("screen scan")
                .is_empty()
        );
        assert!(
            delete_outputs(temp.path(), &day, ReprocessKind::Audio, None, None, true)
                .expect("audio scan")
                .is_empty()
        );
    }

    #[test]
    fn dry_run_reprocess_leaves_selected_sidecar_in_place() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        let output = path.join("audio.jsonl");
        fs::write(&output, "{}\n").expect("sidecar");
        let request = BatchRequest {
            day: "20260812".into(),
            jobs: 1,
            reprocess: Some(ReprocessKind::Audio),
            segment: None,
            stream: None,
            dry_run: true,
            verbose: false,
            debug: false,
        };
        run_batch(temp.path(), &request).expect("dry run");
        assert!(output.exists());
    }

    #[test]
    fn orphan_reprocess_deletion_dirties_day_without_a_worker_completion() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        let output = path.join("orphan_screen.jsonl");
        fs::write(&output, "{}\n").expect("sidecar");
        let request = BatchRequest {
            day: "20260812".into(),
            jobs: 1,
            reprocess: Some(ReprocessKind::Screen),
            segment: None,
            stream: None,
            dry_run: false,
            verbose: false,
            debug: false,
        };

        run_batch(temp.path(), &request).expect("orphan deletion completes");

        assert!(!output.exists());
        assert!(matches!(
            read_health_marker(temp.path(), "20260812", HealthMarkerKind::Stream)
                .expect("stream marker"),
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
        ));
    }

    #[test]
    fn marker_failure_after_reprocess_deletion_is_terminal_and_retains_the_deletion() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        let output = path.join("orphan_screen.jsonl");
        fs::write(&output, "{}\n").expect("sidecar");
        fs::create_dir_all(temp.path().join("chronicle/20260812/health/stream.updated"))
            .expect("block stream marker");

        let result = delete_outputs(
            temp.path(),
            &temp.path().join("chronicle/20260812"),
            ReprocessKind::Screen,
            None,
            None,
            false,
        );

        assert!(matches!(
            result,
            Err(BatchError::PostDelete { path, day, detail })
                if path == output
                    && day == "20260812"
                    && detail.contains("stream marker write failed")
        ));
        assert!(
            !output.exists(),
            "terminal marker failure must not claim the deletion rolled back"
        );
    }

    #[test]
    fn filters_and_modality_limit_the_scan_independently() {
        let temp = tempfile::tempdir().expect("journal");
        let first = segment(temp.path());
        fs::write(first.join("audio.flac"), "audio").expect("audio");
        fs::write(first.join("screen.webm"), "video").expect("video");
        let second = temp.path().join("chronicle/20260812/other/120001_1");
        fs::create_dir_all(&second).expect("second segment");
        fs::write(second.join("other.flac"), "audio").expect("other audio");
        let day = temp.path().join("chronicle/20260812");

        let audio = scan_unprocessed(
            temp.path(),
            &day,
            Some("120000_1"),
            Some("capture"),
            Some(ReprocessKind::Audio),
        )
        .expect("scan");
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].handler, "transcribe");
        assert!(
            scan_unprocessed(temp.path(), &day, Some("missing"), None, None)
                .expect("no match")
                .is_empty()
        );
    }

    #[test]
    fn existing_describe_failure_reenters_but_timestamp_evidence_does_not() {
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        fs::write(path.join("screen.webm"), "video").expect("video");
        fs::write(
            path.join("screen.jsonl"),
            "{\"_solstone_processing\":{\"state\":\"failed\",\"handler\":\"describe\",\"attempts\":2}}\n",
        )
        .expect("retryable sidecar");
        let day = temp.path().join("chronicle/20260812");
        assert_eq!(
            scan_unprocessed(temp.path(), &day, None, None, None)
                .expect("retry scan")
                .len(),
            1
        );
        fs::write(
            path.join("screen.jsonl"),
            "{\"frame_id\":1,\"timestamp\":0.0}\n",
        )
        .expect("evidence sidecar");
        assert!(
            scan_unprocessed(temp.path(), &day, None, None, None)
                .expect("evidence scan")
                .is_empty()
        );
    }

    #[test]
    fn depict_dispatch_survives_for_a_non_import_image() {
        assert_eq!(
            handler_for_path(std::path::Path::new("photo.png")),
            Some("depict")
        );
        let temp = tempfile::tempdir().expect("journal");
        let path = segment(temp.path());
        fs::write(path.join("photo.png"), b"image").expect("image");
        let work = scan_unprocessed(
            temp.path(),
            &temp.path().join("chronicle/20260812"),
            None,
            None,
            None,
        )
        .expect("scan");
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].handler, "depict");
    }

    #[test]
    fn jobs_elevates_only_batch_describe_concurrency() {
        let config = serde_json::json!({
            "describe": {"max_concurrent": 2},
            "transcribe": {"max_concurrent": 7},
            "depict": {"max_concurrent": 5},
        });
        let config = config.as_object().expect("config");
        assert_eq!(batch_describe_workers(config, 9), 9);
        assert_eq!(batch_describe_workers(config, -1), 2);
        assert_eq!(resolve_concurrency(config, "transcribe"), 7);
        assert_eq!(resolve_concurrency(config, "depict"), 5);
    }
}
