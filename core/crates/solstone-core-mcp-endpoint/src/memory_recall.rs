// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private recall over the caller's own validated memory originals.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use rusqlite::{Connection, InterruptHandle};
use solstone_core_format::agent_memory::{
    ChainPredecessor, Coordinate, MAX_METADATA_BYTES, OperationRecord, Origin, ReadyDocument,
    SourceKey, digest, validate_record_header,
};
use solstone_core_indexer_query::{
    MemoryOriginalRow, OwnMemoryDateFilters, OwnMemoryOpenError, OwnMemoryQuery,
    OwnMemoryQueryMode, QueryBoundary, compile_own_memory_query, inspect_own_memory_index,
    open_own_memory_connection, own_memory_candidates, read_own_memory_row,
};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::readers::read_relative_file_bounded;
use solstone_core_journal_io::strict_segment::resolve_stream_exact;
use solstone_core_memory_original::{OriginalRead, read_original_until, read_source_guard};

use crate::references::{
    MemoryRecallCursor, ReferenceCodec, ReferenceError, ReferenceKind, ReferenceTarget,
};

pub(crate) const RECALL_DEADLINE: Duration = Duration::from_secs(5);
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;
const MAX_PAGE_BYTES: usize = 65_536;
const BATCH_SIZE: usize = 1;
const MAX_COVERAGE_COORDINATES: usize = 10_000;
const MAX_EXAMINED_ROWS: usize = 1_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecallNote {
    pub day: String,
    pub path: String,
    pub bytes: Vec<u8>,
    pub origin: Origin,
    pub ready: ReadyDocument,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecallPage {
    pub notes: Vec<RecallNote>,
    pub continuation: Option<String>,
    pub reason: Option<&'static str>,
    pub query_reason: Option<&'static str>,
    pub complete: bool,
    pub self_resolution: Option<&'static str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecallArgs {
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
    pub continuation: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Coverage {
    Complete,
    Pending,
    SourceUnavailable,
    Budget,
}

struct Watchdog {
    stop: mpsc::Sender<()>,
    join: Option<thread::JoinHandle<()>>,
}

impl Watchdog {
    fn start(handle: InterruptHandle, deadline: Instant) -> Self {
        let (stop, receive) = mpsc::channel();
        let join = thread::spawn(move || {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if receive.recv_timeout(remaining).is_err() {
                handle.interrupt();
            }
        });
        Self {
            stop,
            join: Some(join),
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Private endpoint engine, bound to verified identity and credential lineage.
/// Own cursors use this process-local signing generation independently of the
/// ordinary read-grant ledger. This does not register an MCP tool.
#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn recall(
    journal: &Path,
    codec: &ReferenceCodec,
    connection_identity: &str,
    verified_identity: &str,
    credential_lineage: &str,
    args: RecallArgs,
    reference_date: NaiveDate,
) -> RecallPage {
    recall_until(
        journal,
        codec,
        connection_identity,
        verified_identity,
        credential_lineage,
        args,
        reference_date,
        Instant::now() + RECALL_DEADLINE,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "transport supplies one deadline for selection and final live validation"
)]
pub(crate) fn recall_until(
    journal: &Path,
    codec: &ReferenceCodec,
    connection_identity: &str,
    verified_identity: &str,
    credential_lineage: &str,
    args: RecallArgs,
    mut reference_date: NaiveDate,
    deadline: Instant,
) -> RecallPage {
    let source_key = SourceKey::from_verified_id(verified_identity);
    let boundary = QueryBoundary::OwnMemory {
        source_key: source_key.as_str().to_owned(),
    };
    let normalized_query = args.query.as_deref().map(normalize_query);
    let mut cursor_anchor = None;
    let mut cursor_inclusive = true;
    let mut cursor_predicate = None;
    if let Some(token) = args.continuation.as_deref() {
        let Ok(ReferenceTarget::MemoryRecall(cursor)) = codec.resolve(
            token,
            ReferenceKind::MemoryRecall,
            connection_identity,
            codec.signing_generation(),
        ) else {
            return failure("memory_recall_cursor_invalid");
        };
        if cursor.source_key != source_key.as_str()
            || cursor.credential_lineage != credential_lineage
            || cursor.query != normalized_query
        {
            return failure("memory_recall_cursor_invalid");
        }
        reference_date = cursor.reference_date;
        cursor_inclusive = cursor.anchor_inclusive;
        cursor_predicate = Some((cursor.compiled_mode, cursor.effective_date));
        cursor_anchor = Some((cursor.anchor_day, cursor.anchor_path));
    }

    let query = compile_own_memory_query(
        args.query.as_deref(),
        OwnMemoryDateFilters {
            day: args.day.clone(),
            day_from: args.day_from.clone(),
            day_to: args.day_to.clone(),
        },
        reference_date,
    );
    if cursor_predicate
        .is_some_and(|(mode, date)| mode != query.mode || date != query.predicate.effective_date)
    {
        return failure("memory_recall_cursor_invalid");
    }
    if deadline_passed(deadline) {
        return failure("memory_recall_budget_exhausted");
    }
    let source_guard = match read_source_guard(journal, &source_key, deadline) {
        Ok(guard) => guard,
        Err(_) if deadline_passed(deadline) => return failure("memory_recall_budget_exhausted"),
        Err(_) => return failure("memory_source_unavailable"),
    };
    let busy = remaining(deadline);
    let connection = match open_own_memory_connection(journal, busy) {
        Ok(connection) => connection,
        Err(_) if deadline_passed(deadline) => {
            return failure("memory_recall_budget_exhausted");
        }
        Err(OwnMemoryOpenError::Pending) => return failure("memory_index_pending"),
        Err(OwnMemoryOpenError::Unavailable) => return failure("memory_index_unavailable"),
    };
    if deadline_passed(deadline) {
        return failure("memory_recall_budget_exhausted");
    }
    let watchdog = Watchdog::start(connection.get_interrupt_handle(), deadline);
    // This read transaction begins before schema/coverage observations and
    // remains the same view throughout note selection.
    if connection.execute_batch("BEGIN").is_err() {
        return failure(if deadline_passed(deadline) {
            "memory_recall_budget_exhausted"
        } else {
            "memory_index_unavailable"
        });
    }
    match inspect_own_memory_index(&connection) {
        Ok(()) => {}
        Err(_) if deadline_passed(deadline) => {
            return failure("memory_recall_budget_exhausted");
        }
        Err(OwnMemoryOpenError::Pending) => return failure("memory_index_pending"),
        Err(OwnMemoryOpenError::Unavailable) => return failure("memory_index_unavailable"),
    }
    if let Some(reason) = coverage_failure_reason(coverage_in_view(
        journal,
        &connection,
        &source_key,
        deadline,
        source_guard.is_some(),
    )) {
        return failure(reason);
    }
    if deadline_passed(deadline) {
        return failure("memory_recall_budget_exhausted");
    }
    if matches!(query.mode, OwnMemoryQueryMode::NoSearchTerms) {
        return RecallPage {
            notes: Vec::new(),
            continuation: None,
            reason: None,
            query_reason: query.reason,
            complete: true,
            self_resolution: None,
        };
    }

    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut notes = Vec::new();
    let mut page_bytes = 0usize;
    let mut anchor = cursor_anchor;
    let mut inclusive = cursor_inclusive;
    let mut examined = 0;
    let mut continuation = None;
    let mut continuation_mint_failed = false;
    let mut reason = None;
    let mut exhausted = false;

    loop {
        if deadline_passed(deadline) || examined >= MAX_EXAMINED_ROWS {
            reason = Some("memory_recall_budget_exhausted");
            exhausted = true;
            break;
        }
        if let Err(error) = connection.busy_timeout(remaining(deadline)) {
            let _ = error;
            reason = Some("memory_recall_budget_exhausted");
            exhausted = true;
            break;
        }
        let candidates = own_memory_candidates(
            &connection,
            &boundary,
            &query,
            anchor
                .as_ref()
                .map(|(day, path)| (day.as_str(), path.as_str())),
            inclusive,
            BATCH_SIZE,
        );
        let row = match candidates {
            Ok(mut candidates) => candidates.pop(),
            Err(error) if is_interrupt(&error) => {
                reason = Some("memory_recall_budget_exhausted");
                exhausted = true;
                break;
            }
            Err(_) => {
                reason = Some("memory_index_unavailable");
                break;
            }
        };
        let Some(row) = row else {
            break;
        };
        let row_anchor = (row.day.clone(), row.path.clone());
        if deadline_passed(deadline) {
            reason = Some("memory_recall_budget_exhausted");
            exhausted = true;
            break;
        }
        let candidate = validate_candidate(journal, &source_key, &row, deadline);
        if deadline_passed(deadline) {
            if matches!(candidate, Candidate::Ready(_)) {
                continuation = match mint_cursor(
                    codec,
                    connection_identity,
                    credential_lineage,
                    &source_key,
                    normalized_query.clone(),
                    &query,
                    reference_date,
                    row_anchor,
                    true,
                ) {
                    Ok(token) => Some(token),
                    Err(_) => {
                        continuation_mint_failed = true;
                        None
                    }
                };
            }
            reason = Some("memory_recall_budget_exhausted");
            exhausted = true;
            break;
        }
        match candidate {
            Candidate::Ready(note) => {
                if notes.len() >= limit
                    || page_bytes.saturating_add(note.bytes.len()) > MAX_PAGE_BYTES
                {
                    continuation = match mint_cursor(
                        codec,
                        connection_identity,
                        credential_lineage,
                        &source_key,
                        normalized_query.clone(),
                        &query,
                        reference_date,
                        row_anchor,
                        true,
                    ) {
                        Ok(token) => Some(token),
                        Err(_) => {
                            continuation_mint_failed = true;
                            None
                        }
                    };
                    break;
                }
                page_bytes += note.bytes.len();
                notes.push(*note);
            }
            Candidate::Deleted => {}
            Candidate::Pending => {
                reason = Some("memory_index_pending");
                break;
            }
            Candidate::Incomplete => {
                reason = Some("memory_source_unavailable");
                break;
            }
            Candidate::Budget => {
                reason = Some("memory_recall_budget_exhausted");
                exhausted = true;
                break;
            }
        }
        examined += 1;
        anchor = Some(row_anchor);
        inclusive = false;
    }
    drop(watchdog);
    if exhausted
        && continuation.is_none()
        && let Some(anchor) = anchor
    {
        continuation = mint_cursor(
            codec,
            connection_identity,
            credential_lineage,
            &source_key,
            normalized_query,
            &query,
            reference_date,
            anchor,
            inclusive,
        )
        .ok();
    }
    let complete = page_is_complete(reason, continuation.as_deref(), continuation_mint_failed);
    RecallPage {
        notes,
        continuation,
        reason,
        query_reason: query.reason,
        complete,
        self_resolution: (reason.is_some() || continuation_mint_failed)
            .then_some("retry recall or start a fresh query."),
    }
}

fn failure(reason: &'static str) -> RecallPage {
    RecallPage {
        notes: Vec::new(),
        continuation: None,
        reason: Some(reason),
        query_reason: None,
        complete: false,
        self_resolution: Some("retry recall or start a fresh query."),
    }
}

fn normalize_query(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn remaining(deadline: Instant) -> Duration {
    remaining_at(deadline, Instant::now())
}

fn remaining_at(deadline: Instant, now: Instant) -> Duration {
    deadline.saturating_duration_since(now)
}

fn deadline_passed(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

fn coverage_failure_reason(coverage: Coverage) -> Option<&'static str> {
    match coverage {
        Coverage::Complete => None,
        Coverage::Pending => Some("memory_index_pending"),
        Coverage::SourceUnavailable => Some("memory_source_unavailable"),
        Coverage::Budget => Some("memory_recall_budget_exhausted"),
    }
}

fn is_interrupt(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(code, _) if code.code == rusqlite::ErrorCode::OperationInterrupted)
}

#[allow(
    clippy::too_many_arguments,
    reason = "one token binds identity, frozen query and continuation coordinate"
)]
fn mint_cursor(
    codec: &ReferenceCodec,
    connection_identity: &str,
    credential_lineage: &str,
    source_key: &SourceKey,
    query: Option<String>,
    compiled: &OwnMemoryQuery,
    reference_date: NaiveDate,
    anchor: (String, String),
    anchor_inclusive: bool,
) -> Result<String, ReferenceError> {
    codec.mint(
        connection_identity,
        codec.signing_generation(),
        ReferenceTarget::MemoryRecall(MemoryRecallCursor {
            source_key: source_key.as_str().to_owned(),
            credential_lineage: credential_lineage.to_owned(),
            query,
            reference_date,
            compiled_mode: compiled.mode.clone(),
            effective_date: compiled.predicate.effective_date.clone(),
            anchor_day: anchor.0,
            anchor_path: anchor.1,
            anchor_inclusive,
        }),
    )
}

fn page_is_complete(
    reason: Option<&'static str>,
    continuation: Option<&str>,
    continuation_mint_failed: bool,
) -> bool {
    reason.is_none() && continuation.is_none() && !continuation_mint_failed
}

enum Candidate {
    Ready(Box<RecallNote>),
    Deleted,
    Pending,
    Incomplete,
    Budget,
}

fn validate_candidate(
    journal: &Path,
    source_key: &SourceKey,
    row: &MemoryOriginalRow,
    deadline: Instant,
) -> Candidate {
    if deadline_passed(deadline) {
        return Candidate::Budget;
    }
    let coordinate = Coordinate {
        day: row.day.clone(),
        stream: row.stream.clone(),
        segment: row.segment.clone(),
    };
    let outcome = read_original_until(journal, source_key, &coordinate, deadline);
    if deadline_passed(deadline) {
        return Candidate::Budget;
    }
    match outcome {
        OriginalRead::Absent | OriginalRead::Deleted => Candidate::Deleted,
        OriginalRead::Unready | OriginalRead::Staged => Candidate::Pending,
        OriginalRead::Corrupt | OriginalRead::Unavailable { .. } => Candidate::Incomplete,
        OriginalRead::Ready {
            bytes,
            origin,
            ready,
        } => {
            if bytes != row.bytes || !row_matches(row, &coordinate, &origin, &ready) {
                return Candidate::Incomplete;
            }
            Candidate::Ready(Box::new(RecallNote {
                day: row.day.clone(),
                path: row.path.clone(),
                bytes,
                origin,
                ready,
            }))
        }
    }
}

fn row_matches(
    row: &MemoryOriginalRow,
    coordinate: &Coordinate,
    origin: &Origin,
    ready: &ReadyDocument,
) -> bool {
    let Ok(cached_origin) = serde_json::from_str::<Origin>(&row.origin_json) else {
        return false;
    };
    let Ok(cached_chain) = serde_json::from_str::<ChainPredecessor>(&row.chain_json) else {
        return false;
    };
    let path = format!(
        "{}/{}/{}/note.txt",
        coordinate.day, coordinate.stream, coordinate.segment
    );
    row.path == path
        && row.day == coordinate.day
        && row.stream == coordinate.stream
        && row.segment == coordinate.segment
        && row.source_key == origin.source_key.as_str()
        && row.bytes.len() == row.byte_count
        && row.digest == digest(&row.bytes)
        && cached_origin == *origin
        && cached_chain == ready.chain
        && row.creation_label == origin.creation_label
        && row.created_at == origin.created_at.to_rfc3339()
}

fn coverage_in_view(
    journal: &Path,
    connection: &Connection,
    source_key: &SourceKey,
    deadline: Instant,
    source_is_coordinated: bool,
) -> Coverage {
    if deadline_passed(deadline) {
        return Coverage::Budget;
    }
    let operation_rel = format!("config/agent-memory/{}", source_key.component());
    let operation_dir = journal.join(&operation_rel);
    let operation_status = safe_directory_status(journal, &operation_rel, deadline);
    let operation_exists = match operation_status {
        DirectoryStatus::Absent => false,
        DirectoryStatus::Directory => true,
        DirectoryStatus::Unavailable => return Coverage::SourceUnavailable,
        DirectoryStatus::Budget => return Coverage::Budget,
    };

    let root_result = JournalRoot::open(journal);
    if deadline_passed(deadline) {
        return Coverage::Budget;
    }
    let root = match root_result {
        Ok(root) => root,
        Err(_) => return coverage_io_failure(deadline),
    };
    let namespace = match fs::canonicalize(root.canonical_path()) {
        Ok(path) => path,
        Err(_) => return coverage_io_failure(deadline),
    };
    let mut coordinates = Vec::new();
    if operation_exists {
        let entries_result = fs::read_dir(&operation_dir);
        if deadline_passed(deadline) {
            return Coverage::Budget;
        }
        let entries = match entries_result {
            Ok(entries) => entries,
            Err(_) => return coverage_io_failure(deadline),
        };
        let mut entries = entries;
        loop {
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let next = entries.next();
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let Some(item) = next else { break };
            let item = match item {
                Ok(item) => item,
                Err(_) => return coverage_io_failure(deadline),
            };
            let file_type_result = item.file_type();
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let file_type = match file_type_result {
                Ok(file_type) => file_type,
                Err(_) => return coverage_io_failure(deadline),
            };
            if !file_type.is_file()
                || item.path().extension().and_then(|ext| ext.to_str()) != Some("json")
            {
                return Coverage::SourceUnavailable;
            }
            let Some(name) = item.file_name().to_str().map(str::to_owned) else {
                return Coverage::SourceUnavailable;
            };
            let relative = PathBuf::from(&operation_rel).join(&name);
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let observation_result =
                read_relative_file_bounded(&root, &relative, MAX_METADATA_BYTES);
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let observation = match observation_result {
                Ok(Some(observation)) => observation,
                Ok(None) | Err(_) => return coverage_io_failure(deadline),
            };
            let record: OperationRecord = match serde_json::from_slice(&observation.bytes) {
                Ok(record) => record,
                Err(_) => return Coverage::SourceUnavailable,
            };
            if digest(record.operation_id.as_bytes()) != name.trim_end_matches(".json")
                || validate_record_header(&record, source_key).is_err()
            {
                return Coverage::SourceUnavailable;
            }
            if coordinates.len() >= MAX_COVERAGE_COORDINATES {
                return Coverage::Budget;
            }
            coordinates.push(record.coordinate);
        }
        if coordinates.is_empty() {
            return Coverage::SourceUnavailable;
        }
    }

    let stream = format!("agent-memory-{}", source_key.component());
    let chronicle = journal.join("chronicle");
    let chronicle_status = safe_directory_status(journal, "chronicle", deadline);
    let mut found_stream = false;
    match chronicle_status {
        DirectoryStatus::Unavailable => return Coverage::SourceUnavailable,
        DirectoryStatus::Budget => return Coverage::Budget,
        DirectoryStatus::Absent => {}
        DirectoryStatus::Directory => {
            let days_result = fs::read_dir(&chronicle);
            if deadline_passed(deadline) {
                return Coverage::Budget;
            }
            let days = match days_result {
                Ok(days) => days,
                Err(_) => return coverage_io_failure(deadline),
            };
            let mut days = days;
            loop {
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let next = days.next();
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let Some(day_entry) = next else { break };
                let day_entry = match day_entry {
                    Ok(day_entry) => day_entry,
                    Err(_) => return coverage_io_failure(deadline),
                };
                let day = match day_entry.file_name().into_string() {
                    Ok(day) if day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()) => {
                        day
                    }
                    _ => continue,
                };
                let metadata_result = fs::symlink_metadata(day_entry.path());
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let metadata = match metadata_result {
                    Ok(metadata) => metadata,
                    Err(_) => return coverage_io_failure(deadline),
                };
                if !metadata.file_type().is_dir() {
                    return Coverage::SourceUnavailable;
                }
                let stream_result = resolve_stream_exact(&namespace, &day, &stream);
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let stream_path = match stream_result {
                    Ok(Some(path)) => path,
                    Ok(None) => continue,
                    Err(_) => return coverage_io_failure(deadline),
                };
                found_stream = true;
                let segments_result = fs::read_dir(&stream_path);
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let segments = match segments_result {
                    Ok(segments) => segments,
                    Err(_) => return coverage_io_failure(deadline),
                };
                let mut segments = segments;
                loop {
                    if deadline_passed(deadline) {
                        return Coverage::Budget;
                    };
                    let next = segments.next();
                    if deadline_passed(deadline) {
                        return Coverage::Budget;
                    }
                    let Some(segment_entry) = next else { break };
                    let segment_entry = match segment_entry {
                        Ok(segment_entry) => segment_entry,
                        Err(_) => return coverage_io_failure(deadline),
                    };
                    let segment = match segment_entry.file_name().into_string() {
                        Ok(segment) => segment,
                        Err(_) => return Coverage::SourceUnavailable,
                    };
                    let file_type = segment_entry.file_type();
                    if deadline_passed(deadline) {
                        return Coverage::Budget;
                    }
                    let file_type = match file_type {
                        Ok(kind) => kind,
                        Err(_) => return coverage_io_failure(deadline),
                    };
                    if segment.starts_with(".removing_") {
                        continue;
                    }
                    if let Some(live_name) = segment.strip_suffix(".lock") {
                        let coordinate = Coordinate {
                            day: day.clone(),
                            stream: stream.clone(),
                            segment: live_name.to_owned(),
                        };
                        if file_type.is_file()
                            && solstone_core_format::agent_memory::validate_coordinate(
                                &coordinate,
                                source_key,
                            )
                            .is_ok()
                        {
                            continue;
                        }
                        return Coverage::SourceUnavailable;
                    }
                    if coordinates.len() >= MAX_COVERAGE_COORDINATES {
                        return Coverage::Budget;
                    }
                    coordinates.push(Coordinate {
                        day: day.clone(),
                        stream: stream.clone(),
                        segment,
                    });
                }
            }
        }
    }

    if !operation_exists && !found_stream {
        return Coverage::Complete;
    }
    if !source_is_coordinated || coordinates.is_empty() {
        return Coverage::SourceUnavailable;
    }
    let mut seen = std::collections::BTreeSet::new();
    for coordinate in coordinates {
        if deadline_passed(deadline) {
            return Coverage::Budget;
        }
        if !seen.insert((coordinate.day.clone(), coordinate.segment.clone())) {
            continue;
        }
        let original = read_original_until(journal, source_key, &coordinate, deadline);
        if deadline_passed(deadline) {
            return Coverage::Budget;
        }
        match original {
            OriginalRead::Ready {
                bytes,
                origin,
                ready,
            } => {
                let path = format!(
                    "{}/{}/{}/note.txt",
                    coordinate.day, coordinate.stream, coordinate.segment
                );
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let row_result = read_own_memory_row(connection, source_key.as_str(), &path);
                if deadline_passed(deadline) {
                    return Coverage::Budget;
                }
                let row = match row_result {
                    Ok(Some(row)) => row,
                    Ok(None) => return Coverage::Pending,
                    Err(_) => return coverage_io_failure(deadline),
                };
                if bytes != row.bytes || !row_matches(&row, &coordinate, &origin, &ready) {
                    return Coverage::SourceUnavailable;
                }
            }
            OriginalRead::Corrupt | OriginalRead::Unavailable { .. } => {
                return Coverage::SourceUnavailable;
            }
            OriginalRead::Absent | OriginalRead::Deleted => {}
            OriginalRead::Unready | OriginalRead::Staged => return Coverage::Pending,
        }
        if deadline_passed(deadline) {
            return Coverage::Budget;
        }
    }
    Coverage::Complete
}

enum DirectoryStatus {
    Absent,
    Directory,
    Unavailable,
    Budget,
}

fn safe_directory_status(root: &Path, relative: &str, deadline: Instant) -> DirectoryStatus {
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        if deadline_passed(deadline) {
            return DirectoryStatus::Budget;
        }
        let std::path::Component::Normal(component) = component else {
            return DirectoryStatus::Unavailable;
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current);
        if deadline_passed(deadline) {
            return DirectoryStatus::Budget;
        }
        match metadata {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return DirectoryStatus::Unavailable,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return DirectoryStatus::Absent;
            }
            Err(_) => return DirectoryStatus::Unavailable,
        }
    }
    DirectoryStatus::Directory
}

fn coverage_io_failure(deadline: Instant) -> Coverage {
    if deadline_passed(deadline) {
        Coverage::Budget
    } else {
        Coverage::SourceUnavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "full-tests")]
    use chrono::{TimeZone, Utc};
    #[cfg(feature = "full-tests")]
    use solstone_core_format::agent_memory::{OriginKind, Readiness, digest as note_digest};

    #[test]
    fn deadline_remaining_never_resets_the_budget() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(5);
        assert_eq!(remaining_at(deadline, now), Duration::from_secs(5));
        assert_eq!(
            remaining_at(deadline, now + Duration::from_secs(2)),
            Duration::from_secs(3)
        );
        assert_eq!(
            remaining_at(deadline, now + Duration::from_secs(6)),
            Duration::ZERO
        );
    }

    #[test]
    fn required_continuation_without_token_keeps_page_incomplete() {
        assert!(!page_is_complete(None, None, true));
        assert!(page_is_complete(None, None, false));
    }

    #[test]
    fn query_normalization_only_collapses_whitespace() {
        assert_eq!(normalize_query("  Keep\tthis  "), "Keep this");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn own_recall_cursor_binds_credential_lineage_and_signing_generation() {
        let outer = tempfile::Builder::new()
            .prefix("own-memory-cursor-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        let journal = outer.path().join("unused-journal");
        assert!(!journal.exists());

        let codec = ReferenceCodec::new().unwrap();
        let connection_identity = "own-memory-cursor-connection";
        let verified_identity = "own-memory-cursor-identity";
        let source_key = SourceKey::from_verified_id(verified_identity);
        let query = Some("private memory".to_owned());
        let day = Some("20260914".to_owned());
        let args = RecallArgs {
            query: query.clone(),
            day: day.clone(),
            ..RecallArgs::default()
        };
        let mint = |codec: &ReferenceCodec, credential_lineage: &str| {
            codec
                .mint(
                    connection_identity,
                    codec.signing_generation(),
                    ReferenceTarget::MemoryRecall(MemoryRecallCursor {
                        source_key: source_key.as_str().to_owned(),
                        credential_lineage: credential_lineage.to_owned(),
                        query: query.as_deref().map(normalize_query),
                        reference_date: NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
                        compiled_mode: compile_own_memory_query(
                            args.query.as_deref(),
                            OwnMemoryDateFilters {
                                day: day.clone(),
                                ..OwnMemoryDateFilters::default()
                            },
                            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
                        )
                        .mode,
                        effective_date: solstone_core_indexer_query::EffectiveDateConstraint::Exact(
                            "20260914".into(),
                        ),
                        anchor_day: "20260914".to_owned(),
                        anchor_path: "20260914/agent-memory-source/120000_1/note.txt".to_owned(),
                        anchor_inclusive: true,
                    }),
                )
                .unwrap()
        };
        let token = mint(&codec, "credential-lineage-a");
        let signing_generation = codec.signing_generation();
        assert!(matches!(
            codec.resolve(
                &token,
                ReferenceKind::MemoryRecall,
                connection_identity,
                signing_generation
            ),
            Ok(ReferenceTarget::MemoryRecall(_))
        ));
        assert!(!journal.exists());
        assert_eq!(
            codec.resolve(
                &token,
                ReferenceKind::MemoryRecall,
                connection_identity,
                signing_generation.wrapping_add(1)
            ),
            Err(ReferenceError::NotFound)
        );
        assert!(!journal.exists());
        if signing_generation != 9 {
            assert_eq!(
                codec.resolve(&token, ReferenceKind::MemoryRecall, connection_identity, 9),
                Err(ReferenceError::NotFound)
            );
        }
        assert!(!journal.exists());
        assert_eq!(
            codec.resolve(
                &token,
                ReferenceKind::MemoryRecall,
                "another-connection",
                signing_generation
            ),
            Err(ReferenceError::NotFound)
        );
        assert!(!journal.exists());

        let recall_with = |codec: &ReferenceCodec, credential_lineage: &str, token: String| {
            recall(
                &journal,
                codec,
                connection_identity,
                verified_identity,
                credential_lineage,
                RecallArgs {
                    continuation: Some(token),
                    ..args.clone()
                },
                NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
            )
        };
        let matching = recall_with(&codec, "credential-lineage-a", token.clone());
        assert_eq!(matching.reason, Some("memory_index_unavailable"));
        assert!(!journal.exists());
        let mismatched = recall_with(&codec, "credential-lineage-b", token.clone());
        assert_eq!(mismatched.reason, Some("memory_recall_cursor_invalid"));
        assert!(!journal.exists());

        let empty_lineage_token = mint(&codec, "");
        let empty_lineage = recall_with(&codec, "", empty_lineage_token.clone());
        assert_eq!(empty_lineage.reason, Some("memory_index_unavailable"));
        assert!(!journal.exists());
        let nonempty_lineage = recall_with(&codec, "credential-lineage-a", empty_lineage_token);
        assert_eq!(
            nonempty_lineage.reason,
            Some("memory_recall_cursor_invalid")
        );
        assert!(!journal.exists());

        let other_codec = ReferenceCodec::new().unwrap();
        assert_eq!(
            other_codec.resolve(
                &token,
                ReferenceKind::MemoryRecall,
                connection_identity,
                other_codec.signing_generation()
            ),
            Err(ReferenceError::NotFound)
        );
        assert!(!journal.exists());
        let restarted = recall_with(&other_codec, "credential-lineage-a", token.clone());
        assert_eq!(restarted.reason, Some("memory_recall_cursor_invalid"));
        assert!(!journal.exists());

        let mut tampered = token.into_bytes();
        let last = tampered.len() - 1;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).unwrap();
        let tampered_result = recall_with(&codec, "credential-lineage-a", tampered);
        assert_eq!(tampered_result.reason, Some("memory_recall_cursor_invalid"));
        assert!(!journal.exists());
    }

    #[test]
    fn coverage_outcomes_keep_pending_source_and_deadline_distinct() {
        assert_eq!(coverage_failure_reason(Coverage::Complete), None);
        assert_eq!(
            coverage_failure_reason(Coverage::Pending),
            Some("memory_index_pending")
        );
        assert_eq!(
            coverage_failure_reason(Coverage::SourceUnavailable),
            Some("memory_source_unavailable")
        );
        assert_eq!(
            coverage_failure_reason(Coverage::Budget),
            Some("memory_recall_budget_exhausted")
        );
    }

    #[cfg(feature = "full-tests")]
    fn test_scratch_root() -> std::path::PathBuf {
        if cfg!(windows) {
            std::env::temp_dir()
        } else {
            std::path::PathBuf::from("/var/tmp")
        }
    }

    #[cfg(feature = "full-tests")]
    fn recall_fixture(note_sizes: &[usize]) -> (tempfile::TempDir, SourceKey, Vec<Coordinate>) {
        use solstone_core_format::agent_memory::{ChainPredecessor, OperationRecord};
        use solstone_core_indexer_store::db::db_path;

        let journal = tempfile::Builder::new()
            .prefix("memory-recall-fixture-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        let source = SourceKey::from_verified_id("recall-fixture-source");
        drop(
            solstone_core_segment::hold_agent_memory_mutation(journal.path(), source.component())
                .unwrap(),
        );
        let stream = format!("agent-memory-{}", source.component());
        let day = "20260102";
        let created_at = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let operation_dir = journal
            .path()
            .join("config/agent-memory")
            .join(source.component());
        fs::create_dir_all(&operation_dir).unwrap();
        let mut coordinates = Vec::new();
        let mut originals = Vec::new();

        for (ordinal, size) in note_sizes.iter().copied().enumerate() {
            let segment_name = format!("{:02}0000_1", 9 - ordinal);
            let coordinate = Coordinate {
                day: day.to_owned(),
                stream: stream.clone(),
                segment: segment_name,
            };
            let bytes = vec![b'x'; size];
            let chain = ChainPredecessor {
                prev_day: None,
                prev_segment: None,
                seq: 1,
            };
            let origin = Origin {
                kind: OriginKind::AgentMemory,
                source_key: source.clone(),
                creation_label: "fixture label".to_owned(),
                created_at,
                stream: stream.clone(),
                segment: coordinate.segment.clone(),
            };
            let ready = ReadyDocument {
                source_key: source.clone(),
                origin: origin.clone(),
                coordinate: coordinate.clone(),
                created_at,
                digest: note_digest(&bytes),
                byte_count: bytes.len(),
                chain: chain.clone(),
            };
            let segment_path = journal
                .path()
                .join("chronicle")
                .join(day)
                .join(&stream)
                .join(&coordinate.segment);
            fs::create_dir_all(&segment_path).unwrap();
            let _live_guard = solstone_core_journal_io::hold_lock(
                &segment_path,
                solstone_core_journal_io::LockOptions::default(),
            )
            .unwrap();
            fs::write(segment_path.join("note.txt"), &bytes).unwrap();
            fs::write(
                segment_path.join("origin.json"),
                serde_json::to_vec(&origin).unwrap(),
            )
            .unwrap();
            fs::write(
                segment_path.join("ready.json"),
                serde_json::to_vec(&ready).unwrap(),
            )
            .unwrap();
            fs::write(
                segment_path.join("stream.json"),
                serde_json::json!({
                    "stream": stream,
                    "prev_day": null,
                    "prev_segment": null,
                    "seq": 1
                })
                .to_string(),
            )
            .unwrap();

            let operation_id = format!("operation-{ordinal}");
            let operation = OperationRecord {
                operation_id: operation_id.clone(),
                digest: ready.digest.clone(),
                byte_count: bytes.len(),
                created_at,
                origin_kind: OriginKind::AgentMemory,
                creation_label: origin.creation_label.clone(),
                coordinate: coordinate.clone(),
                phase: Readiness::Reserved,
                chain: None,
            };
            fs::write(
                operation_dir.join(format!("{}.json", digest(operation_id.as_bytes()))),
                serde_json::to_vec(&operation).unwrap(),
            )
            .unwrap();
            drop(_live_guard);
            match solstone_core_memory_original::read_original(journal.path(), &source, &coordinate)
            {
                OriginalRead::Ready { .. } => {}
                OriginalRead::Unavailable { detail } => {
                    panic!("fixture original unavailable: {detail}")
                }
                other => panic!("fixture original state: {other:?}"),
            }
            originals.push((coordinate.clone(), bytes, origin, ready));
            coordinates.push(coordinate);
        }

        let db = db_path(journal.path());
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let connection = Connection::open(db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE index_build_state(id INTEGER PRIMARY KEY, schema_version INTEGER, state TEXT, files_count INTEGER, chunks_count INTEGER);
                 INSERT INTO index_build_state VALUES (1, 1, 'complete', 0, 0);
                 CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER);
                 CREATE VIRTUAL TABLE chunks USING fts5(content, path UNINDEXED, stream UNINDEXED);
                 CREATE TABLE memory_originals(path TEXT PRIMARY KEY, day TEXT, stream TEXT, segment TEXT, source_key TEXT, bytes BLOB, origin_json TEXT, digest TEXT, byte_count INTEGER, created_at TEXT, creation_label TEXT, chain_json TEXT);
                 CREATE INDEX memory_originals_source_day_path ON memory_originals(source_key, day DESC, path DESC);"
            )
            .unwrap();
        for (coordinate, bytes, origin, ready) in originals {
            let path = format!(
                "{}/{}/{}/note.txt",
                coordinate.day, coordinate.stream, coordinate.segment
            );
            connection.execute(
                "INSERT INTO memory_originals(path,day,stream,segment,source_key,bytes,origin_json,digest,byte_count,created_at,creation_label,chain_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![
                    path, coordinate.day, coordinate.stream, coordinate.segment,
                    source.as_str(), bytes, serde_json::to_string(&origin).unwrap(),
                    ready.digest, ready.byte_count as i64, created_at.to_rfc3339(),
                    origin.creation_label, serde_json::to_string(&ready.chain).unwrap()
                ],
            ).unwrap();
        }
        (journal, source, coordinates)
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn ready_marker_counts_as_coverage_before_operation_phase_update() {
        let (journal, source, _) = recall_fixture(&[16]);
        let actual =
            Connection::open(solstone_core_indexer_store::db::db_path(journal.path())).unwrap();
        assert_eq!(
            coverage_in_view(
                journal.path(),
                &actual,
                &source,
                Instant::now() + Duration::from_secs(5),
                true,
            ),
            Coverage::Complete
        );
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn recall_byte_budget_continues_inclusively_after_deleted_anchor() {
        let (journal, _source, coordinates) = recall_fixture(&[32_768, 32_768, 2, 2]);
        let codec = ReferenceCodec::new().unwrap();
        let first = recall(
            journal.path(),
            &codec,
            "connection",
            "recall-fixture-source",
            "recall-fixture-credential",
            RecallArgs::default(),
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        );
        assert_eq!(first.notes.len(), 2);
        assert_eq!(
            first
                .notes
                .iter()
                .map(|note| note.bytes.len())
                .sum::<usize>(),
            MAX_PAGE_BYTES
        );
        let continuation = first.continuation.expect("withheld note anchor");
        let deleted = coordinates[2].clone();
        fs::remove_dir_all(
            journal
                .path()
                .join("chronicle")
                .join(&deleted.day)
                .join(&deleted.stream)
                .join(&deleted.segment),
        )
        .unwrap();
        let next = recall(
            journal.path(),
            &codec,
            "connection",
            "recall-fixture-source",
            "recall-fixture-credential",
            RecallArgs {
                continuation: Some(continuation),
                ..RecallArgs::default()
            },
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        );
        assert_eq!(next.notes.len(), 1);
        assert_eq!(
            next.notes[0].path,
            format!(
                "{}/{}/{}/note.txt",
                coordinates[3].day, coordinates[3].stream, coordinates[3].segment
            )
        );
        assert_eq!(next.reason, None);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn recall_continuation_freezes_relative_date_across_midnight() {
        let (journal, _, _) = recall_fixture(&[4, 4]);
        let codec = ReferenceCodec::new().unwrap();
        let first = recall(
            journal.path(),
            &codec,
            "connection",
            "recall-fixture-source",
            "credential",
            RecallArgs {
                query: Some("today".into()),
                limit: Some(1),
                ..RecallArgs::default()
            },
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        );
        assert_eq!(first.notes.len(), 1);
        let next = recall(
            journal.path(),
            &codec,
            "connection",
            "recall-fixture-source",
            "credential",
            RecallArgs {
                query: Some("today".into()),
                limit: Some(1),
                continuation: first.continuation,
                ..RecallArgs::default()
            },
            NaiveDate::from_ymd_opt(2026, 1, 3).unwrap(),
        );
        assert_eq!(next.notes.len(), 1);
        assert_ne!(first.notes[0].path, next.notes[0].path);
        assert!(next.complete);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn recall_watchdog_interrupts_executing_sqlite_statement() {
        let connection = Connection::open_in_memory().unwrap();
        let started = Instant::now();
        let watchdog = Watchdog::start(
            connection.get_interrupt_handle(),
            started + Duration::from_millis(50),
        );
        let result = connection.query_row(
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",
            [], |row| row.get::<_, i64>(0),
        );
        assert!(is_interrupt(&result.unwrap_err()));
        drop(watchdog);
        assert!(started.elapsed() < RECALL_DEADLINE);
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn writer_original_scan_rescan_and_unused_recall_use_actual_paths() {
        use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
        use solstone_core_indexer_store::scan::{rescan_file, scan_journal};
        let outer = tempfile::Builder::new()
            .prefix("memory-scan-ready-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        let canonical = outer.path().join("journal");
        fs::create_dir(&canonical).unwrap();
        #[cfg(unix)]
        let journal = {
            let alias = outer.path().join("alias");
            std::os::unix::fs::symlink(&canonical, &alias).unwrap();
            alias
        };
        #[cfg(not(unix))]
        let journal = canonical.clone();
        scan_journal(&journal, true).unwrap();
        let codec = ReferenceCodec::new().unwrap();
        let recall_now = || {
            recall(
                &journal,
                &codec,
                "scan-connection",
                "scan-source",
                "scan-credential",
                RecallArgs::default(),
                NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            )
        };
        let unused = recall_now();
        assert!(unused.complete, "{:?}", unused.reason);
        assert!(unused.notes.is_empty());
        assert!(!journal.join("streams").exists());
        assert!(!journal.join("config/agent-memory").exists());
        let AppendResult::Stored(receipt) = append_connection_memory(
            &journal,
            AuthenticatedMemorySource {
                verified_id: "scan-source",
                creation_label: "scan fixture",
            },
            "needle exact original",
            "scan-operation",
            Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
        )
        .unwrap() else {
            panic!("writer fixture must store");
        };
        let segment = journal
            .join("chronicle/20260102")
            .join(receipt.origin.stream)
            .join(receipt.origin.segment);
        let note = segment.join("note.txt");
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&note)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(2_000_000_000)),
            )
            .unwrap();
        scan_journal(&journal, false).unwrap();
        assert_eq!(recall_now().notes.len(), 1);
        let ready = fs::read(segment.join("ready.json")).unwrap();
        fs::remove_file(segment.join("ready.json")).unwrap();
        scan_journal(&journal, false).unwrap();
        let unready = recall_now();
        assert!(!unready.complete, "{:?}", unready.reason);
        assert_eq!(unready.reason, Some("memory_index_pending"));
        assert!(unready.notes.is_empty());
        let connection =
            Connection::open(solstone_core_indexer_store::db::db_path(&journal)).unwrap();
        let cached: i64 = connection
            .query_row("SELECT count(*) FROM memory_originals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(cached, 0);
        drop(connection);
        fs::write(segment.join("ready.json"), ready).unwrap();
        rescan_file(&journal, &note).unwrap();
        assert_eq!(recall_now().notes[0].bytes, b"needle exact original");
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn source_and_live_guards_share_deadline_without_creating_entries() {
        let (journal, source, coordinates) = recall_fixture(&[16]);
        let source_guard =
            solstone_core_segment::hold_agent_memory_mutation(journal.path(), source.component())
                .unwrap();
        let started = Instant::now();
        assert!(
            read_source_guard(journal.path(), &source, started + Duration::from_millis(50))
                .is_err()
        );
        assert!(started.elapsed() < RECALL_DEADLINE);
        drop(source_guard);
        let coordinate = &coordinates[0];
        let live = journal
            .path()
            .join("chronicle")
            .join(&coordinate.day)
            .join(&coordinate.stream)
            .join(&coordinate.segment);
        let live_guard = solstone_core_journal_io::hold_lock(
            &live,
            solstone_core_journal_io::LockOptions::default(),
        )
        .unwrap();
        let before = fs::read(live.join("ready.json")).unwrap();
        let started = Instant::now();
        assert!(matches!(
            read_original_until(
                journal.path(),
                &source,
                coordinate,
                started + Duration::from_millis(50)
            ),
            OriginalRead::Unavailable { .. }
        ));
        assert!(started.elapsed() < RECALL_DEADLINE);
        assert_eq!(fs::read(live.join("ready.json")).unwrap(), before);
        drop(live_guard);
        assert!(matches!(
            read_original_until(
                journal.path(),
                &source,
                coordinate,
                Instant::now() + RECALL_DEADLINE
            ),
            OriginalRead::Ready { .. }
        ));
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn memory_measurement_harness_explains_real_browse_and_term_statements() {
        use rusqlite::{Connection, params_from_iter};
        use solstone_core_format::agent_memory::{OriginKind, Readiness};
        use solstone_core_indexer_query::own_memory_candidate_statement;

        let journal = tempfile::Builder::new()
            .prefix("memory-recall-measurement-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        use crate::memory::{AppendResult, AuthenticatedMemorySource, append_connection_memory};
        let source = SourceKey::from_verified_id("measurement-source");
        let stream = format!("agent-memory-{}", source.component());
        let created_at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        for ordinal in 0..128 {
            let AppendResult::Stored(receipt) = append_connection_memory(
                journal.path(),
                AuthenticatedMemorySource {
                    verified_id: "measurement-source",
                    creation_label: "measurement label",
                },
                "needle exact memory",
                &format!("saved-{ordinal}"),
                created_at + chrono::Duration::seconds(ordinal),
            )
            .unwrap() else {
                panic!("measurement writer must store");
            };
            if ordinal % 2 == 0 {
                fs::remove_dir_all(
                    journal
                        .path()
                        .join("chronicle")
                        .join(&receipt.coordinate.day)
                        .join(receipt.origin.stream)
                        .join(receipt.origin.segment),
                )
                .unwrap();
            }
        }
        for ordinal in 0..8 {
            append_connection_memory(
                journal.path(),
                AuthenticatedMemorySource {
                    verified_id: "other-measurement-source",
                    creation_label: "other source",
                },
                "needle other source",
                &format!("other-{ordinal}"),
                created_at + chrono::Duration::seconds(ordinal),
            )
            .unwrap();
        }
        let ledger = journal
            .path()
            .join("config/agent-memory")
            .join(source.component());
        for ordinal in 0..128 {
            let operation_id = format!("abandoned-{ordinal}");
            let coordinate = Coordinate {
                day: "20260901".into(),
                stream: stream.clone(),
                segment: format!("20{:02}{:02}_1", ordinal / 60, ordinal % 60),
            };
            let record = OperationRecord {
                operation_id: operation_id.clone(),
                digest: digest(b"needle"),
                byte_count: 6,
                created_at,
                origin_kind: OriginKind::AgentMemory,
                creation_label: "measurement label".into(),
                coordinate,
                phase: Readiness::Reserved,
                chain: None,
            };
            fs::write(
                ledger.join(format!("{}.json", digest(operation_id.as_bytes()))),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
        }
        fs::create_dir_all(journal.path().join("talents")).unwrap();
        fs::write(
            journal.path().join("talents/memory.md"),
            "needle ordinary talent",
        )
        .unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
        let codec = ReferenceCodec::new().unwrap();
        for query in [None, Some("needle")] {
            let started = Instant::now();
            let page = recall(
                journal.path(),
                &codec,
                "measurement-connection",
                "measurement-source",
                "measurement-credential",
                RecallArgs {
                    query: query.map(str::to_owned),
                    ..RecallArgs::default()
                },
                NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
            );
            assert_eq!(page.reason, None);
            assert_eq!(page.notes.len(), DEFAULT_LIMIT);
            assert!(page.continuation.is_some());
            assert!(page.notes.iter().all(
                |note| note.origin.source_key == source && note.bytes == b"needle exact memory"
            ));
            assert!(started.elapsed() < RECALL_DEADLINE);
        }
        let connection =
            Connection::open(solstone_core_indexer_store::db::db_path(journal.path())).unwrap();
        let originals: i64 = connection
            .query_row("SELECT count(*) FROM memory_originals", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(originals, 72);

        let boundary = QueryBoundary::OwnMemory {
            source_key: source.as_str().to_owned(),
        };
        let browse = compile_own_memory_query(
            None,
            OwnMemoryDateFilters::default(),
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        );
        let terms = compile_own_memory_query(
            Some("needle"),
            OwnMemoryDateFilters::default(),
            NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        );
        let plans = [
            own_memory_candidate_statement(&boundary, &browse, None, true, 5).unwrap(),
            own_memory_candidate_statement(&boundary, &terms, None, true, 5).unwrap(),
        ]
        .into_iter()
        .map(|(sql, values)| {
            let mut statement = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            statement
                .query_map(params_from_iter(values.iter()), |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .join("\n")
        })
        .collect::<Vec<_>>();
        assert!(plans[0].contains("memory_originals_source_day_path"));
        assert!(plans[1].contains("VIRTUAL TABLE INDEX"));
    }
}
