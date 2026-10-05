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
    MemoryOriginalRow, OwnMemoryDateFilters, OwnMemoryOpenError, OwnMemoryQueryMode, QueryBoundary,
    compile_own_memory_query, inspect_own_memory_index, open_own_memory_connection,
    own_memory_candidates, read_own_memory_row,
};
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_journal_io::readers::read_relative_file_bounded;
use solstone_core_journal_io::strict_segment::resolve_stream_exact;
use solstone_core_memory_original::{OriginalRead, read_original};

use crate::references::{MemoryRecallCursor, ReferenceCodec, ReferenceKind, ReferenceTarget};

const RECALL_DEADLINE: Duration = Duration::from_secs(5);
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 20;
const MAX_PAGE_BYTES: usize = 65_536;
const BATCH_SIZE: usize = 1;

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

/// Private endpoint engine. Callers must pass the already verified identity
/// and its current permission generation; this does not register an MCP tool.
pub(crate) fn recall(
    journal: &Path,
    codec: &ReferenceCodec,
    connection_identity: &str,
    verified_identity: &str,
    permission_generation: u64,
    args: RecallArgs,
    reference_date: NaiveDate,
) -> RecallPage {
    let deadline = Instant::now() + RECALL_DEADLINE;
    let source_key = SourceKey::from_verified_id(verified_identity);
    let boundary = QueryBoundary::OwnMemory {
        source_key: source_key.as_str().to_owned(),
    };
    let normalized_query = args.query.as_deref().map(normalize_query);
    let mut cursor_anchor = None;
    if let Some(token) = args.continuation.as_deref() {
        let Ok(ReferenceTarget::MemoryRecall(cursor)) = codec.resolve(
            token,
            ReferenceKind::MemoryRecall,
            connection_identity,
            permission_generation,
        ) else {
            return failure("memory_recall_cursor_invalid");
        };
        if cursor.source_key != source_key.as_str()
            || cursor.query != normalized_query
            || cursor.day != args.day
            || cursor.day_from != args.day_from
            || cursor.day_to != args.day_to
        {
            return failure("memory_recall_cursor_invalid");
        }
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
    if deadline_passed(deadline) {
        return failure("memory_recall_budget_exhausted");
    }
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
    match inspect_own_memory_index(&connection) {
        Ok(()) => {}
        Err(_) if deadline_passed(deadline) => {
            return failure("memory_recall_budget_exhausted");
        }
        Err(OwnMemoryOpenError::Pending) => return failure("memory_index_pending"),
        Err(OwnMemoryOpenError::Unavailable) => return failure("memory_index_unavailable"),
    }
    if let Some(reason) =
        coverage_failure_reason(coverage(journal, &connection, &source_key, deadline))
    {
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
    let mut inclusive = true;
    let mut continuation = None;
    let mut reason = None;
    let mut exhausted = false;

    loop {
        if deadline_passed(deadline) {
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
                continuation = mint_cursor(
                    codec,
                    connection_identity,
                    permission_generation,
                    &source_key,
                    normalized_query.clone(),
                    &args,
                    row_anchor,
                );
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
                    continuation = mint_cursor(
                        codec,
                        connection_identity,
                        permission_generation,
                        &source_key,
                        normalized_query.clone(),
                        &args,
                        row_anchor,
                    );
                    break;
                }
                page_bytes += note.bytes.len();
                notes.push(*note);
            }
            Candidate::DeletedOrUnready => {}
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
        anchor = Some(row_anchor);
        inclusive = false;
    }
    drop(watchdog);
    if exhausted && continuation.is_none() {
        // The page has no safe first-unreturned coordinate. Retrying from the
        // same request cannot skip a row; a fresh browse remains available.
    }
    let complete = reason.is_none() && continuation.is_none();
    RecallPage {
        notes,
        continuation,
        reason,
        query_reason: query.reason,
        complete,
        self_resolution: reason.map(|_| "Retry recall or start a fresh query."),
    }
}

fn failure(reason: &'static str) -> RecallPage {
    RecallPage {
        notes: Vec::new(),
        continuation: None,
        reason: Some(reason),
        query_reason: None,
        complete: false,
        self_resolution: Some("Retry recall or start a fresh query."),
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

fn mint_cursor(
    codec: &ReferenceCodec,
    connection_identity: &str,
    permission_generation: u64,
    source_key: &SourceKey,
    query: Option<String>,
    args: &RecallArgs,
    anchor: (String, String),
) -> Option<String> {
    codec
        .mint(
            connection_identity,
            permission_generation,
            ReferenceTarget::MemoryRecall(MemoryRecallCursor {
                source_key: source_key.as_str().to_owned(),
                query,
                day: args.day.clone(),
                day_from: args.day_from.clone(),
                day_to: args.day_to.clone(),
                anchor_day: anchor.0,
                anchor_path: anchor.1,
            }),
        )
        .ok()
}

enum Candidate {
    Ready(Box<RecallNote>),
    DeletedOrUnready,
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
    let outcome = read_original(journal, source_key, &coordinate);
    if deadline_passed(deadline) {
        return Candidate::Budget;
    }
    match outcome {
        OriginalRead::Absent
        | OriginalRead::Unready
        | OriginalRead::Deleted
        | OriginalRead::Staged => Candidate::DeletedOrUnready,
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

fn coverage(
    journal: &Path,
    connection: &Connection,
    source_key: &SourceKey,
    deadline: Instant,
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
                let stream_result = resolve_stream_exact(journal, &day, &stream);
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
                    match file_type {
                        Ok(_) => {}
                        Err(_) => return coverage_io_failure(deadline),
                    }
                    if segment.starts_with(".removing_") {
                        continue;
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
    if coordinates.is_empty() {
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
        let original = read_original(journal, source_key, &coordinate);
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
            OriginalRead::Absent
            | OriginalRead::Unready
            | OriginalRead::Deleted
            | OriginalRead::Staged => {}
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
    fn query_normalization_only_collapses_whitespace() {
        assert_eq!(normalize_query("  Keep\tthis  "), "Keep this");
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
            coverage(
                journal.path(),
                &actual,
                &source,
                Instant::now() + Duration::from_secs(5)
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
            9,
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
            9,
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
    fn memory_measurement_harness_explains_real_browse_and_term_statements() {
        use rusqlite::{Connection, params_from_iter};
        use solstone_core_format::agent_memory::{OriginKind, Readiness};
        use solstone_core_indexer_query::own_memory_candidate_statement;

        let journal = tempfile::Builder::new()
            .prefix("memory-recall-measurement-")
            .tempdir_in(test_scratch_root())
            .unwrap();
        let source = SourceKey::parse(format!("sha256:{}", "a".repeat(64))).unwrap();
        let stream = format!("agent-memory-{}", source.component());
        let ledger = journal
            .path()
            .join("config/agent-memory")
            .join(source.component());
        fs::create_dir_all(&ledger).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        for ordinal in 0..256 {
            let operation_id = format!("operation-{ordinal}");
            let coordinate = Coordinate {
                day: "20260901".to_owned(),
                stream: stream.clone(),
                segment: format!(
                    "{:02}{:02}{:02}_1",
                    ordinal / 3600,
                    (ordinal / 60) % 60,
                    ordinal % 60
                ),
            };
            let record = OperationRecord {
                operation_id: operation_id.clone(),
                digest: digest(b"needle"),
                byte_count: 6,
                created_at,
                origin_kind: OriginKind::AgentMemory,
                creation_label: "measurement label".to_owned(),
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

        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE memory_originals(path TEXT PRIMARY KEY, day TEXT, stream TEXT, segment TEXT, source_key TEXT, bytes BLOB, origin_json TEXT, digest TEXT, byte_count INTEGER, created_at TEXT, creation_label TEXT, chain_json TEXT);
             CREATE INDEX memory_originals_source_day_path ON memory_originals(source_key, day DESC, path DESC);
             CREATE VIRTUAL TABLE chunks USING fts5(content, path UNINDEXED, stream UNINDEXED);",
        ).unwrap();
        for ordinal in 0..512 {
            let path = format!(
                "202609{:02}/agent-memory-{}/seg-{ordinal}/note.txt",
                ordinal % 30 + 1,
                "a".repeat(64)
            );
            let day = path[..8].to_owned();
            let stream = format!("agent-memory-{}", "a".repeat(64));
            connection.execute(
                "INSERT INTO memory_originals(path,day,stream,segment,source_key,bytes,origin_json,digest,byte_count,created_at,creation_label,chain_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
                rusqlite::params![path, day, stream, format!("seg-{ordinal}"), format!("sha256:{}", "a".repeat(64)), b"needle", "{}", "digest", 6, "2026-09-01T00:00:00Z", "saved", "{}"],
            ).unwrap();
            if ordinal % 2 == 0 {
                connection
                    .execute(
                        "INSERT INTO chunks(content,path,stream) VALUES(?,?,?)",
                        rusqlite::params!["needle", path, stream],
                    )
                    .unwrap();
            }
        }
        connection
            .execute(
                "INSERT INTO chunks(content,path,stream) VALUES(?,?,?)",
                ["needle", "20260901/talents/memory.md", "ordinary"],
            )
            .unwrap();

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
