// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native append path for support-draft events and their day locator.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use chrono::{DateTime, Local, MappedLocalTime, NaiveDate, NaiveDateTime, TimeDelta, TimeZone};
use regex::Regex;
use serde_json::{Map, Value};
use solstone_core_journal_io::{
    AtomicWriteError, AtomicWriteOptions, HealthMarkerKind, atomic_replace, bump_stream_marker,
    health_marker_path, write_bytes_exclusive, write_jsonl,
};
use solstone_core_segment::{Kind, SegmentDir, StreamHints, advance_unbound_stream};
use thiserror::Error;

const SUPPORT_DRAFTS_STREAM: &str = "support-drafts";
const SUPPORT_DRAFT: &str = "support_draft";
const SEGMENT_WINDOW_MS: i64 = 300_000;

static DRAFT_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
static DRAFT_LOCK_DEPTH: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
struct TestDraftLockScope;

#[cfg(test)]
impl TestDraftLockScope {
    fn enter() -> Self {
        DRAFT_LOCK_DEPTH.fetch_add(1, AtomicOrdering::SeqCst);
        Self
    }
}

#[cfg(test)]
impl Drop for TestDraftLockScope {
    fn drop(&mut self) {
        DRAFT_LOCK_DEPTH.fetch_sub(1, AtomicOrdering::SeqCst);
    }
}

/// Failure while appending a support-draft event or its draft locator.
#[derive(Debug, Error)]
pub enum SupportDraftError {
    /// The caller-supplied journal root does not exist.
    #[error("journal root does not exist: {path}")]
    JournalRootMissing { path: PathBuf },
    /// The caller-supplied journal root is not a directory.
    #[error("journal root is not a directory: {path}")]
    JournalRootNotDirectory { path: PathBuf },
    /// Inspecting the caller-supplied journal root failed.
    #[error("inspect journal root {path}: {source}")]
    JournalRootIo { path: PathBuf, source: io::Error },
    /// A draft id is not one plain path component.
    #[error("invalid support draft id")]
    InvalidDraftId,
    /// The native path supports only the reference's support-draft event kind.
    #[error("unknown support-draft event kind: {kind}")]
    UnknownKind { kind: String },
    /// A required support-draft field was absent.
    #[error("missing required support-draft field: {field}")]
    MissingField { field: &'static str },
    /// An event timestamp was not an integer milliseconds value.
    #[error("support-draft event timestamp must be an integer")]
    InvalidTimestamp,
    /// A timestamp could not be resolved as a local wall-clock time.
    #[error("resolve local support-draft time for timestamp {timestamp}")]
    LocalTime { timestamp: i64 },
    /// A discovered support-draft segment was not a valid local timestamp key.
    #[error("invalid support-draft segment key {segment} for day {day}")]
    InvalidSegmentKey { day: String, segment: String },
    /// Reading an existing support-draft file failed.
    #[error("read support-draft file {path}: {source}")]
    DraftRead { path: PathBuf, source: io::Error },
    /// An existing nonblank support-draft line was not valid JSON.
    #[error("malformed support-draft line {line} in {path}: {source}")]
    MalformedDraftLine {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
    /// An existing nonblank support-draft line was valid JSON but not an object.
    #[error("support-draft line {line} in {path} is not an object")]
    DraftLineNotObject { path: PathBuf, line: usize },
    /// Replacing the support-draft file or draft locator failed.
    #[error("atomic write {path}: {source}")]
    AtomicWrite {
        path: PathBuf,
        source: solstone_core_journal_io::AtomicWriteError,
    },
    /// The draft write is durable, but its exact day could not be marked dirty.
    #[error(
        "support-draft content for {day} remains written, but stream marker advancement failed at {path}: {source}"
    )]
    StreamMarker {
        path: PathBuf,
        day: String,
        source: solstone_core_journal_io::AtomicWriteError,
    },
    /// Resolving the support-draft segment path failed.
    #[error("resolve support-draft segment: {source}")]
    SegmentPath {
        source: solstone_core_segment::SegmentError,
    },
    /// Advancing the unbound support-draft stream or writing its marker failed.
    #[error("advance support-draft stream: {source}")]
    StreamAdvance {
        source: solstone_core_segment::UnboundStreamAdvanceError,
    },
}

/// Append one support-draft event beneath a caller-supplied journal root.
///
/// The event must carry the support route's fields; `ts` is supplied when
/// absent before validation. No other event kind is native-owned by this crate.
pub fn append_support_draft(
    journal: &Path,
    event: Map<String, Value>,
) -> Result<Value, SupportDraftError> {
    append_draft_event(journal, SUPPORT_DRAFT, event)
}

fn append_draft_event(
    journal: &Path,
    kind: &str,
    event: Map<String, Value>,
) -> Result<Value, SupportDraftError> {
    append_draft_event_at(journal, kind, event, Local::now())
}

/// Record the day containing one support draft for later bounded resolution.
pub fn record_draft_captured(
    journal: &Path,
    draft_id: &str,
    captured_day: &str,
) -> Result<(), SupportDraftError> {
    validate_draft_id(draft_id)?;
    require_journal_root(journal)?;
    let path = support_draft_index_path(journal, draft_id);
    let captured_day = serde_json::to_string(captured_day)
        .expect("serializing a Rust string for a support-draft locator cannot fail");
    let contents = format!("{{\"captured_day\":{captured_day}}}\n");
    atomic_replace(&path, contents.as_bytes(), AtomicWriteOptions::default())
        .map_err(|source| SupportDraftError::AtomicWrite { path, source })
}

/// Resolve a support draft's captured day without changing journal state.
pub fn resolve_draft_day(
    journal: &Path,
    draft_id: &str,
) -> Result<Option<String>, SupportDraftError> {
    if validate_draft_id(draft_id).is_err() {
        return Ok(None);
    }
    let path = support_draft_index_path(journal, draft_id);
    if !path.is_file() {
        return Ok(None);
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return Ok(None),
    };
    let payload: Value = match serde_json::from_str(&contents) {
        Ok(payload) => payload,
        Err(_) => return Ok(None),
    };
    Ok(payload
        .as_object()
        .and_then(|object| object.get("captured_day"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

/// Load the captured support-draft event for `draft_id`, if the locator and event exist.
///
/// Resolution is bounded to the locator's captured day. Invalid ids, a missing
/// locator, and a day with no matching event all return `Ok(None)`.
pub fn load_draft_event(
    journal: &Path,
    draft_id: &str,
) -> Result<Option<Value>, SupportDraftError> {
    let Some(day) = resolve_draft_day(journal, draft_id)? else {
        return Ok(None);
    };
    for segment in draft_segments(journal, &day)? {
        let path = journal
            .join("chronicle")
            .join(&day)
            .join(SUPPORT_DRAFTS_STREAM)
            .join(segment)
            .join("support-drafts.jsonl");
        for event in read_events_file(&path)? {
            if event.get("kind").and_then(Value::as_str) == Some(SUPPORT_DRAFT)
                && event.get("draft_id").and_then(Value::as_str) == Some(draft_id)
            {
                return Ok(Some(event));
            }
        }
    }
    Ok(None)
}

/// Read the terminal mark for one support draft, if present.
///
/// Soft-fails like [`resolve_draft_day`]: an invalid id, missing file, or
/// malformed payload returns `Ok(None)` rather than an error.
pub fn resolve_draft_outcome(
    journal: &Path,
    draft_id: &str,
) -> Result<Option<String>, SupportDraftError> {
    if validate_draft_id(draft_id).is_err() {
        return Ok(None);
    }
    let path = support_draft_outcome_path(journal, draft_id);
    if !path.is_file() {
        return Ok(None);
    }
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(_) => return Ok(None),
    };
    let payload: Value = match serde_json::from_str(&contents) {
        Ok(payload) => payload,
        Err(_) => return Ok(None),
    };
    Ok(payload
        .as_object()
        .and_then(|object| object.get("outcome"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

/// Record that a support draft was submitted.
///
/// Create-only. If the mark file already exists — this verb or the other —
/// this returns `Ok(())` without rewriting it. Callers that need to know which
/// outcome actually won must re-read [`resolve_draft_outcome`] after this returns.
pub fn mark_draft_submitted(journal: &Path, draft_id: &str) -> Result<(), SupportDraftError> {
    mark_draft_outcome(journal, draft_id, "submitted")
}

/// Record that a support draft was cancelled.
///
/// Create-only. If the mark file already exists — this verb or the other —
/// this returns `Ok(())` without rewriting it. Callers that need to know which
/// outcome actually won must re-read [`resolve_draft_outcome`] after this returns.
pub fn mark_draft_cancelled(journal: &Path, draft_id: &str) -> Result<(), SupportDraftError> {
    mark_draft_outcome(journal, draft_id, "cancelled")
}

fn mark_draft_outcome(
    journal: &Path,
    draft_id: &str,
    outcome: &str,
) -> Result<(), SupportDraftError> {
    validate_draft_id(draft_id)?;
    require_journal_root(journal)?;
    let path = support_draft_outcome_path(journal, draft_id);
    let contents = format!("{{\"outcome\":\"{outcome}\"}}\n");
    match write_bytes_exclusive(&path, contents.as_bytes(), AtomicWriteOptions::default()) {
        Ok(()) => Ok(()),
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(source) => Err(SupportDraftError::AtomicWrite { path, source }),
    }
}

fn append_draft_event_at(
    journal: &Path,
    kind: &str,
    mut event: Map<String, Value>,
    now: DateTime<Local>,
) -> Result<Value, SupportDraftError> {
    if !event.contains_key("ts") {
        event.insert("ts".to_owned(), Value::from(now.timestamp_millis()));
    }
    validate_event(kind, &event)?;
    require_journal_root(journal)?;

    let timestamp = event
        .get("ts")
        .and_then(Value::as_i64)
        .ok_or(SupportDraftError::InvalidTimestamp)?;
    let event_time = local_time(timestamp)?;
    append_validated_draft_event_at_local_time(
        journal,
        kind,
        event,
        timestamp,
        event_time.naive_local(),
    )
}

fn append_validated_draft_event_at_local_time(
    journal: &Path,
    kind: &str,
    event: Map<String, Value>,
    timestamp: i64,
    event_time: NaiveDateTime,
) -> Result<Value, SupportDraftError> {
    let mut stored = Map::new();
    stored.insert("kind".to_owned(), Value::String(kind.to_owned()));
    stored.extend(event);
    let stored = Value::Object(stored);

    {
        pause_at(journal, "draft-before-lock");
        let _guard = DRAFT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        #[cfg(test)]
        let _test_lock_scope = TestDraftLockScope::enter();
        let (day, segment) = current_segment_key(journal, timestamp, event_time)?;
        let segment_dir = SegmentDir::resolve(journal, &day, &segment, SUPPORT_DRAFTS_STREAM)
            .map_err(|source| SupportDraftError::SegmentPath { source })?;
        let draft_path = segment_dir.path().join("support-drafts.jsonl");
        let mut events = read_events_file(&draft_path)?;
        pause_at(journal, "draft-read-before-write");
        events.push(stored.clone());
        write_jsonl(&draft_path, events, AtomicWriteOptions::default()).map_err(|source| {
            SupportDraftError::AtomicWrite {
                path: draft_path.clone(),
                source,
            }
        })?;
        bump_stream_marker(journal, &day).map_err(|source| SupportDraftError::StreamMarker {
            path: health_marker_path(journal, &day, HealthMarkerKind::Stream),
            day: day.clone(),
            source,
        })?;
        if !segment_dir.path().join("stream.json").is_file() {
            advance_unbound_stream(
                journal,
                SUPPORT_DRAFTS_STREAM,
                &day,
                &segment,
                StreamHints {
                    kind: Some(Kind::Unknown),
                    host: None,
                    platform: None,
                },
            )
            .map_err(|source| SupportDraftError::StreamAdvance { source })?;
        }
    }

    Ok(stored)
}

fn validate_event(kind: &str, event: &Map<String, Value>) -> Result<(), SupportDraftError> {
    if kind != SUPPORT_DRAFT {
        return Err(SupportDraftError::UnknownKind {
            kind: kind.to_owned(),
        });
    }
    for field in [
        "draft_id",
        "captured_day",
        "verb",
        "payload",
        "diagnostics_snapshot",
    ] {
        if !event.contains_key(field) {
            return Err(SupportDraftError::MissingField { field });
        }
    }
    if !event.get("ts").is_some_and(Value::is_i64) {
        return Err(SupportDraftError::InvalidTimestamp);
    }
    Ok(())
}

fn require_journal_root(journal: &Path) -> Result<(), SupportDraftError> {
    match fs::metadata(journal) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(SupportDraftError::JournalRootNotDirectory {
            path: journal.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(SupportDraftError::JournalRootMissing {
                path: journal.to_path_buf(),
            })
        }
        Err(source) => Err(SupportDraftError::JournalRootIo {
            path: journal.to_path_buf(),
            source,
        }),
    }
}

fn validate_draft_id(draft_id: &str) -> Result<(), SupportDraftError> {
    if draft_id.is_empty() || draft_id.contains('/') || matches!(draft_id, "." | "..") {
        return Err(SupportDraftError::InvalidDraftId);
    }
    Ok(())
}

fn support_draft_index_path(journal: &Path, draft_id: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join("health")
        .join("support-drafts")
        .join(format!("{draft_id}.json"))
}

fn support_draft_outcome_path(journal: &Path, draft_id: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join("health")
        .join("support-drafts")
        .join(format!("{draft_id}.outcome.json"))
}

fn current_segment_key(
    journal: &Path,
    timestamp: i64,
    event_time: NaiveDateTime,
) -> Result<(String, String), SupportDraftError> {
    pause_at(journal, "draft-before-segment-selection");
    let day = event_time.format("%Y%m%d").to_string();
    let mut existing = draft_segments(journal, &day)?;
    if existing.is_empty() {
        return Ok((day, segment_key_for_start(event_time)));
    }
    existing.sort();
    let current = existing.pop().expect("checked nonempty draft segment list");
    let current_start = segment_start_timestamp(&day, &current)?;
    if i128::from(timestamp) - i128::from(current_start) >= i128::from(SEGMENT_WINDOW_MS) {
        Ok((day, segment_key_for_start(event_time)))
    } else {
        Ok((day, current))
    }
}

fn local_time(timestamp: i64) -> Result<DateTime<Local>, SupportDraftError> {
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .ok_or(SupportDraftError::LocalTime { timestamp })
}

fn draft_segments(journal: &Path, day: &str) -> Result<Vec<String>, SupportDraftError> {
    let directory = journal
        .join("chronicle")
        .join(day)
        .join(SUPPORT_DRAFTS_STREAM);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(SupportDraftError::DraftRead {
                path: directory,
                source,
            });
        }
    };
    let mut segments = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| SupportDraftError::DraftRead {
            path: directory.clone(),
            source,
        })?;
        if !entry
            .file_type()
            .map_err(|source| SupportDraftError::DraftRead {
                path: entry.path(),
                source,
            })?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_segment_key(&name) {
            segments.push(name);
        }
    }
    Ok(segments)
}

fn is_segment_key(value: &str) -> bool {
    static SEGMENT_KEY: OnceLock<Regex> = OnceLock::new();
    // Adaptation: Rust `\\b` treats Marks, Connector_Punctuation, and Join_Control as word
    // characters (for example, `\\u{0301}100000_300`), unlike Python. Later parsing requires
    // ASCII digits, unlike Python `.isdigit()`/`int()` (for example, `١٢٣٤٥٦_٣٠٠`). Neither
    // form can be system-created because draft writers emit ASCII `%H%M%S_300` keys.
    SEGMENT_KEY
        .get_or_init(|| {
            Regex::new(r"\b(\d{6})_(\d+)(?:_|\b)")
                .expect("the Python-compatible segment-key pattern is valid")
        })
        .is_match(value)
}

fn segment_key_for_start(start: NaiveDateTime) -> String {
    format!("{}_300", start.format("%H%M%S"))
}

fn segment_start_timestamp(day: &str, segment: &str) -> Result<i64, SupportDraftError> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| {
        SupportDraftError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        }
    })?;
    let (time_text, duration_text) =
        segment
            .split_once('_')
            .ok_or_else(|| SupportDraftError::InvalidSegmentKey {
                day: day.to_owned(),
                segment: segment.to_owned(),
            })?;
    if time_text.len() != 6
        || !time_text.bytes().all(|byte| byte.is_ascii_digit())
        || duration_text.is_empty()
        || !duration_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(SupportDraftError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        });
    }
    let time = chrono::NaiveTime::parse_from_str(time_text, "%H%M%S").map_err(|_| {
        SupportDraftError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        }
    })?;
    resolve_local_datetime(NaiveDateTime::new(date, time))
        .map(|time| time.timestamp_millis())
        .ok_or_else(|| SupportDraftError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        })
}

/// Map a naive local wall time the way the reference does: earlier offset on a
/// fall-back overlap, and skip forward through a spring-forward gap.
fn resolve_local_datetime(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    let mut candidate = naive;
    for _ in 0..180 {
        match Local.from_local_datetime(&candidate) {
            MappedLocalTime::Single(time) => return Some(time),
            MappedLocalTime::Ambiguous(earliest, _) => return Some(earliest),
            MappedLocalTime::None => candidate += TimeDelta::minutes(1),
        }
    }
    None
}

fn read_events_file(path: &Path) -> Result<Vec<Value>, SupportDraftError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(SupportDraftError::DraftRead {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut events = Vec::new();
    for (offset, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|source| SupportDraftError::MalformedDraftLine {
                path: path.to_path_buf(),
                line: offset + 1,
                source,
            })?;
        if !value.is_object() {
            return Err(SupportDraftError::DraftLineNotObject {
                path: path.to_path_buf(),
                line: offset + 1,
            });
        }
        events.push(value);
    }
    Ok(events)
}

#[cfg(any(test, feature = "test-hooks"))]
type PauseHook = std::sync::Arc<dyn Fn(&Path, &str) + Send + Sync>;

#[cfg(any(test, feature = "test-hooks"))]
static PAUSE_HOOK: std::sync::OnceLock<Mutex<Option<PauseHook>>> = std::sync::OnceLock::new();

#[cfg(any(test, feature = "test-hooks"))]
fn pause_at(journal: &Path, point: &str) {
    let hook = PAUSE_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(hook) = hook {
        hook(journal, point);
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn pause_at(_: &Path, _: &str) {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_journal_io::{HealthMarkerKind, HealthMarkerState, read_health_marker};

    use super::{
        DRAFT_LOCK_DEPTH, PAUSE_HOOK, SUPPORT_DRAFTS_STREAM, SupportDraftError,
        append_draft_event_at, append_support_draft, append_validated_draft_event_at_local_time,
        load_draft_event, mark_draft_cancelled, mark_draft_submitted, record_draft_captured,
        resolve_draft_day, resolve_draft_outcome, resolve_local_datetime, support_draft_index_path,
        support_draft_outcome_path,
    };

    static NEXT_JOURNAL: AtomicU64 = AtomicU64::new(0);
    static PAUSE_TEST_GUARD: Mutex<()> = Mutex::new(());

    struct TestJournal {
        path: PathBuf,
    }

    impl TestJournal {
        fn new() -> Self {
            let sequence = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/var/tmp").join(format!(
                "solstone-core-support-drafts-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test journal");
            Self { path }
        }
    }

    impl Drop for TestJournal {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).expect("remove test journal");
        }
    }

    #[test]
    fn spring_forward_gap_resolves_instead_of_erroring() {
        use chrono::MappedLocalTime;
        let naive = NaiveDate::from_ymd_opt(2026, 3, 8)
            .expect("date")
            .and_hms_opt(2, 30, 0)
            .expect("time");
        let resolved = resolve_local_datetime(naive).expect("gap or ordinary local time resolves");
        assert!(
            !matches!(
                Local.from_local_datetime(&resolved.naive_local()),
                MappedLocalTime::None
            ),
            "resolved time must be a valid local wall time"
        );
    }

    fn local_at(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<Local> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(year, month, day)
                    .expect("date")
                    .and_hms_opt(hour, minute, second)
                    .expect("time"),
            )
            .single()
            .expect("unambiguous local time")
    }

    fn route_event(timestamp: i64) -> Map<String, Value> {
        let mut event = Map::new();
        event.insert("ts".to_owned(), Value::from(timestamp));
        event.insert("draft_id".to_owned(), json!("a1b2c3d4"));
        event.insert("captured_day".to_owned(), json!("20260815"));
        event.insert("verb".to_owned(), json!("create"));
        event.insert("payload".to_owned(), json!({"body": "hello"}));
        event.insert("diagnostics_snapshot".to_owned(), json!({"state": "ok"}));
        event
    }

    fn draft_path(journal: &Path, day: &str, segment: &str) -> PathBuf {
        journal
            .join("chronicle")
            .join(day)
            .join(SUPPORT_DRAFTS_STREAM)
            .join(segment)
            .join("support-drafts.jsonl")
    }

    fn write_draft(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("draft parent")).expect("create draft parent");
        fs::write(path, contents).expect("write draft");
    }

    #[test]
    fn ac1_support_draft_preserves_route_order_and_extra_fields() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let mut event = route_event(now.timestamp_millis());
        event.insert("future_field".to_owned(), json!({"kept": true}));

        let stored =
            append_draft_event_at(&journal.path, "support_draft", event, now).expect("append");
        let object = stored.as_object().expect("stored object");
        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            [
                "kind",
                "ts",
                "draft_id",
                "captured_day",
                "verb",
                "payload",
                "diagnostics_snapshot",
                "future_field",
            ]
        );
        assert_eq!(
            stored,
            json!({
                "kind": "support_draft",
                "ts": now.timestamp_millis(),
                "draft_id": "a1b2c3d4",
                "captured_day": "20260815",
                "verb": "create",
                "payload": {"body": "hello"},
                "diagnostics_snapshot": {"state": "ok"},
                "future_field": {"kept": true},
            })
        );
        let landed: Value = serde_json::from_str(
            &fs::read_to_string(draft_path(&journal.path, "20260815", "100347_300"))
                .expect("read landed event"),
        )
        .expect("landed JSON");
        let landed_object = landed.as_object().expect("landed object");
        assert_eq!(landed, stored);
        assert_eq!(
            landed_object.keys().collect::<Vec<_>>(),
            [
                "kind",
                "ts",
                "draft_id",
                "captured_day",
                "verb",
                "payload",
                "diagnostics_snapshot",
                "future_field",
            ]
        );
    }

    #[test]
    fn timestamp_is_inserted_before_presence_only_validation() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let mut event = route_event(now.timestamp_millis());
        event.remove("ts");
        for field in [
            "draft_id",
            "captured_day",
            "verb",
            "payload",
            "diagnostics_snapshot",
        ] {
            event.insert(field.to_owned(), Value::Null);
        }
        let stored =
            append_draft_event_at(&journal.path, "support_draft", event, now).expect("append");
        assert_eq!(stored["ts"], now.timestamp_millis());
        assert_eq!(stored["payload"], Value::Null);
    }

    #[test]
    fn ac1a_event_jsonl_keeps_non_ascii_as_utf8() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let mut event = route_event(now.timestamp_millis());
        event.insert("payload".to_owned(), json!({"body": "café"}));
        append_draft_event_at(&journal.path, "support_draft", event, now).expect("append");
        let bytes =
            fs::read(draft_path(&journal.path, "20260815", "100347_300")).expect("read draft");
        assert!(
            bytes
                .windows("café".len())
                .any(|window| window == "café".as_bytes())
        );
        assert!(!String::from_utf8(bytes).expect("utf8").contains("\\u00e9"));
    }

    #[test]
    fn ac2_segment_uses_raw_time_and_signed_reuse_window() {
        let now = local_at(2026, 8, 15, 10, 3, 47);

        let raw = TestJournal::new();
        append_draft_event_at(
            &raw.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("raw append");
        assert!(draft_path(&raw.path, "20260815", "100347_300").is_file());
        assert!(!draft_path(&raw.path, "20260815", "100000_300").exists());

        let within = TestJournal::new();
        write_draft(&draft_path(&within.path, "20260815", "095848_300"), "{}\n");
        append_draft_event_at(
            &within.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("299-second-window append");
        assert!(!draft_path(&within.path, "20260815", "100347_300").exists());

        let boundary = TestJournal::new();
        write_draft(
            &draft_path(&boundary.path, "20260815", "095847_300"),
            "{}\n",
        );
        append_draft_event_at(
            &boundary.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("300-second-window append");
        assert!(draft_path(&boundary.path, "20260815", "100347_300").is_file());

        let older = TestJournal::new();
        write_draft(&draft_path(&older.path, "20260815", "100500_300"), "{}\n");
        append_draft_event_at(
            &older.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("older event append");
        assert!(!draft_path(&older.path, "20260815", "100347_300").exists());
        assert_eq!(
            fs::read_to_string(draft_path(&older.path, "20260815", "100500_300"))
                .expect("read reused draft")
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn ac3_local_wall_clock_bucket_differs_from_utc_for_non_utc_offset() {
        let offset = FixedOffset::west_opt(7 * 3600).expect("offset");
        let instant = offset
            .with_ymd_and_hms(2026, 8, 15, 23, 30, 0)
            .single()
            .expect("fixed datetime");
        assert_ne!(instant.naive_local().date(), instant.naive_utc().date());
        assert_eq!(
            instant.naive_local().format("%Y%m%d").to_string(),
            "20260815"
        );
        assert_eq!(
            instant.with_timezone(&Utc).format("%Y%m%d").to_string(),
            "20260816"
        );
        let journal = TestJournal::new();
        append_validated_draft_event_at_local_time(
            &journal.path,
            "support_draft",
            route_event(instant.timestamp_millis()),
            instant.timestamp_millis(),
            instant.naive_local(),
        )
        .expect("append at injected local wall clock");
        assert!(draft_path(&journal.path, "20260815", "233000_300").is_file());
        assert!(!draft_path(&journal.path, "20260816", "063000_300").exists());
    }

    #[test]
    fn ac4_existing_events_survive_and_blank_lines_are_dropped() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let path = draft_path(&journal.path, "20260815", "100347_300");
        write_draft(&path, "{\"prior\":1}\n\n{\"prior\":2}\n");
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append");
        let contents = fs::read_to_string(path).expect("read draft");
        assert_eq!(contents.lines().count(), 3);
        assert!(!contents.contains("\n\n"));
    }

    #[test]
    fn ac5_malformed_line_refuses_append_without_replacing_file() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let path = draft_path(&journal.path, "20260815", "100347_300");
        write_draft(&path, "not-json\n");
        let before = fs::read(&path).expect("read original");
        assert!(matches!(
            append_draft_event_at(
                &journal.path,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(SupportDraftError::MalformedDraftLine { .. })
        ));
        assert_eq!(fs::read(&path).expect("read unchanged"), before);
        write_draft(&path, "{\"prior\":true}\n");
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append after repair");
    }

    #[test]
    fn malformed_segment_name_accepted_by_reference_predicate_refuses_append() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let path = draft_path(&journal.path, "20260815", "100000_300_extra");
        write_draft(&path, "{}\n");
        assert!(matches!(
            append_draft_event_at(
                &journal.path,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(SupportDraftError::InvalidSegmentKey { .. })
        ));
        assert_eq!(
            fs::read_to_string(path).expect("read unchanged draft"),
            "{}\n"
        );
    }

    #[test]
    fn ac6_first_append_advances_unbound_stream_only_once() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("first append");
        let state = journal.path.join("streams/support-drafts.json");
        let marker = journal
            .path
            .join("chronicle/20260815/support-drafts/100347_300/stream.json");
        let first_state = fs::read(&state).expect("stream state");
        let first_marker = fs::read(&marker).expect("stream marker");
        let value: Value = serde_json::from_slice(&first_state).expect("stream json");
        assert_eq!(value["kind"], "unknown");
        assert!(value.get("cid").is_none());
        assert!(value.get("did").is_none());
        assert!(value.get("source").is_none());
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis() + 1),
            now,
        )
        .expect("second append");
        assert_eq!(fs::read(state).expect("state unchanged"), first_state);
        assert_eq!(fs::read(marker).expect("marker unchanged"), first_marker);
    }

    #[test]
    fn existing_segment_append_advances_the_exact_day_marker() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("first append");
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis() + 1),
            now,
        )
        .expect("existing segment append");

        assert!(matches!(
            read_health_marker(&journal.path, "20260815", HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 2
        ));
        assert!(
            !journal
                .path
                .join("chronicle/20260816/health/stream.updated")
                .exists()
        );
        assert_eq!(
            fs::read_to_string(draft_path(&journal.path, "20260815", "100347_300"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn new_segment_retries_topology_after_state_advance_was_blocked() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let state = journal.path.join("streams/support-drafts.json");
        fs::create_dir_all(&state).expect("block stream state");

        let error = append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .unwrap_err();

        assert!(matches!(error, SupportDraftError::StreamAdvance { .. }));
        assert!(draft_path(&journal.path, "20260815", "100347_300").is_file());
        assert!(matches!(
            read_health_marker(&journal.path, "20260815", HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
        ));

        fs::remove_dir(&state).expect("unblock stream state");
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis() + 1),
            now,
        )
        .expect("retry completes topology");

        let state: Value = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
        assert_eq!(state["seq"], 1);
        assert_eq!(state["last_day"], "20260815");
        assert_eq!(state["last_segment"], "100347_300");
        let topology: Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path
                    .join("chronicle/20260815/support-drafts/100347_300/stream.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(topology["seq"], 1);
        assert_eq!(topology["prev_day"], Value::Null);
        assert_eq!(topology["prev_segment"], Value::Null);
        assert_eq!(
            fs::read_to_string(draft_path(&journal.path, "20260815", "100347_300"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn new_segment_retries_missing_topology_marker_without_seq_or_self_link_drift() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let topology = journal
            .path
            .join("chronicle/20260815/support-drafts/100347_300/stream.json");
        fs::create_dir_all(&topology).expect("block topology marker");

        let error = append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .unwrap_err();
        assert!(matches!(error, SupportDraftError::StreamAdvance { .. }));
        let partial_state: Value = serde_json::from_slice(
            &fs::read(journal.path.join("streams/support-drafts.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(partial_state["seq"], 1);
        assert_eq!(partial_state["last_segment"], "100347_300");

        fs::remove_dir(&topology).expect("unblock topology marker");
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis() + 1),
            now,
        )
        .expect("retry completes the partial advance");

        let final_state: Value = serde_json::from_slice(
            &fs::read(journal.path.join("streams/support-drafts.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(final_state["seq"], 1);
        let marker: Value = serde_json::from_slice(&fs::read(topology).unwrap()).unwrap();
        assert_eq!(marker["seq"], 1);
        assert_eq!(marker["prev_day"], Value::Null);
        assert_eq!(marker["prev_segment"], Value::Null);
    }

    #[test]
    fn marker_failure_is_typed_terminal_and_retains_the_draft() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let marker = journal
            .path
            .join("chronicle/20260815/health/stream.updated");
        fs::create_dir_all(&marker).expect("block stream marker");

        let error = append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .unwrap_err();

        assert!(matches!(
            &error,
            SupportDraftError::StreamMarker { path, day, .. }
                if path == &marker && day == "20260815"
        ));
        assert!(error.to_string().contains("remains written"));
        assert!(draft_path(&journal.path, "20260815", "100347_300").is_file());
        assert!(!journal.path.join("streams/support-drafts.json").exists());
    }

    #[test]
    fn ac7_index_failure_is_logged_and_does_not_fail_append() {
        let journal = TestJournal::new();
        fs::write(journal.path.join("indexer"), "not a directory").expect("block indexer");
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append despite index failure");
        assert!(draft_path(&journal.path, "20260815", "100347_300").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn ac8_draft_event_and_locator_are_private_files() {
        use std::os::unix::fs::PermissionsExt;

        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append");
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("record draft");
        let draft_mode = fs::metadata(draft_path(&journal.path, "20260815", "100347_300"))
            .expect("draft metadata")
            .permissions()
            .mode()
            & 0o777;
        let index_mode = fs::metadata(support_draft_index_path(&journal.path, "a1b2c3d4"))
            .expect("locator metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(draft_mode, 0o600);
        assert_eq!(index_mode, 0o600);
    }

    #[test]
    fn ac9_record_and_resolve_draft_day_round_trip_exact_bytes() {
        let journal = TestJournal::new();
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("record draft");
        assert_eq!(
            fs::read(support_draft_index_path(&journal.path, "a1b2c3d4")).expect("read locator"),
            b"{\"captured_day\":\"20260815\"}\n"
        );
        assert_eq!(
            resolve_draft_day(&journal.path, "a1b2c3d4").expect("resolve"),
            Some("20260815".to_owned())
        );
    }

    #[test]
    fn ac10_draft_id_refusal_precedes_filesystem_writes() {
        let journal = TestJournal::new();
        for draft_id in ["../x", "a/b", "", "."] {
            assert!(matches!(
                record_draft_captured(&journal.path, draft_id, "20260815"),
                Err(SupportDraftError::InvalidDraftId)
            ));
            assert_eq!(
                resolve_draft_day(&journal.path, draft_id).expect("resolve"),
                None
            );
            assert!(!journal.path.join("chronicle").exists());
        }
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("ordinary id accepted");
    }

    #[test]
    fn ac11_resolve_draft_day_returns_none_for_all_absent_or_invalid_forms() {
        let journal = TestJournal::new();
        assert_eq!(
            resolve_draft_day(&journal.path, "../x").expect("invalid id"),
            None
        );
        assert_eq!(
            resolve_draft_day(&journal.path, "missing").expect("missing"),
            None
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let unreadable = support_draft_index_path(&journal.path, "unreadable");
            fs::create_dir_all(unreadable.parent().expect("locator parent"))
                .expect("create locator parent");
            fs::write(&unreadable, "{\"captured_day\":\"20260815\"}")
                .expect("write unreadable locator");
            fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000))
                .expect("remove locator permissions");
            if fs::File::open(&unreadable).is_err() {
                assert_eq!(
                    resolve_draft_day(&journal.path, "unreadable").expect("unreadable"),
                    None
                );
            }
        }
        for (id, contents) in [
            ("bad-json", "{"),
            ("array", "[]"),
            ("number", "{\"captured_day\":1}"),
        ] {
            let path = support_draft_index_path(&journal.path, id);
            fs::create_dir_all(path.parent().expect("locator parent"))
                .expect("create locator parent");
            fs::write(path, contents).expect("write locator");
            assert_eq!(resolve_draft_day(&journal.path, id).expect("resolve"), None);
        }
    }

    #[test]
    fn append_refuses_missing_or_nondirectory_journal_roots() {
        let journal = TestJournal::new();
        let missing = journal.path.join("missing-root");
        let now = local_at(2026, 8, 15, 10, 3, 47);
        assert!(matches!(
            append_draft_event_at(
                &missing,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(SupportDraftError::JournalRootMissing { .. })
        ));
        let file = journal.path.join("not-a-directory");
        fs::write(&file, "file").expect("write file root");
        assert!(matches!(
            append_draft_event_at(
                &file,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(SupportDraftError::JournalRootNotDirectory { .. })
        ));
    }

    #[test]
    fn unknown_kind_and_noninteger_timestamp_fail_before_write() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        assert!(matches!(
            append_draft_event_at(
                &journal.path,
                "other",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(SupportDraftError::UnknownKind { .. })
        ));
        let mut event = route_event(now.timestamp_millis());
        event.insert("ts".to_owned(), json!("not-an-integer"));
        assert!(matches!(
            append_draft_event_at(&journal.path, "support_draft", event, now),
            Err(SupportDraftError::InvalidTimestamp)
        ));
        assert!(!journal.path.join("chronicle").exists());
    }

    #[test]
    fn ac12_static_lock_keeps_two_concurrent_appends() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        let journal = Arc::new(TestJournal::new());
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let entered = Arc::new(Barrier::new(2));
        let released = Arc::new(Barrier::new(2));
        let paused = Arc::new(AtomicBool::new(false));
        let hook_entered = Arc::clone(&entered);
        let hook_released = Arc::clone(&released);
        let hook_paused = Arc::clone(&paused);
        let hook_journal = journal.path.clone();
        let hooks = PAUSE_HOOK.get_or_init(|| Mutex::new(None));
        *hooks.lock().expect("hook lock") = Some(Arc::new(move |candidate, point| {
            if candidate == hook_journal
                && point == "draft-read-before-write"
                && !hook_paused.swap(true, Ordering::SeqCst)
            {
                hook_entered.wait();
                hook_released.wait();
            }
        }));

        let first_journal = Arc::clone(&journal);
        let first = thread::spawn(move || {
            append_draft_event_at(
                &first_journal.path,
                "support_draft",
                route_event(now.timestamp_millis()),
                now,
            )
        });
        entered.wait();
        let second_journal = Arc::clone(&journal);
        let second = thread::spawn(move || {
            append_support_draft(
                &second_journal.path,
                route_event(now.timestamp_millis() + 1),
            )
        });
        released.wait();
        first.join().expect("first thread").expect("first append");
        *hooks.lock().expect("hook lock") = None;
        second
            .join()
            .expect("second thread")
            .expect("second append");
        let contents = fs::read_to_string(draft_path(&journal.path, "20260815", "100347_300"))
            .expect("read draft");
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn ac12_static_lock_serializes_segment_selection() {
        let _pause_guard = PAUSE_TEST_GUARD.lock().expect("pause test guard");
        #[derive(Debug)]
        enum Event {
            FirstPaused,
            SecondAwaitingSelection,
            SelectionOutsideLock,
        }

        let journal = Arc::new(TestJournal::new());
        let first = local_at(2026, 8, 15, 10, 5, 0);
        let second = local_at(2026, 8, 15, 10, 7, 0);
        write_draft(&draft_path(&journal.path, "20260815", "100000_300"), "{}\n");
        let released = Arc::new(Barrier::new(2));
        let paused = Arc::new(AtomicBool::new(false));
        let hook_released = Arc::clone(&released);
        let hook_paused = Arc::clone(&paused);
        let lock_attempts = Arc::new(AtomicU64::new(0));
        let hook_lock_attempts = Arc::clone(&lock_attempts);
        let selection_outside_lock = Arc::new(AtomicBool::new(false));
        let hook_selection_outside_lock = Arc::clone(&selection_outside_lock);
        let (event_tx, event_rx) = mpsc::channel();
        let hook_journal = journal.path.clone();
        let hooks = PAUSE_HOOK.get_or_init(|| Mutex::new(None));
        *hooks.lock().expect("hook lock") = Some(Arc::new(move |candidate, point| {
            if candidate != hook_journal {
                return;
            }
            match point {
                "draft-before-lock" => {
                    if hook_lock_attempts.fetch_add(1, Ordering::SeqCst) == 1 {
                        event_tx
                            .send(Event::SecondAwaitingSelection)
                            .expect("test receiver");
                    }
                }
                "draft-before-segment-selection" => {
                    if DRAFT_LOCK_DEPTH.load(Ordering::SeqCst) != 1 {
                        hook_selection_outside_lock.store(true, Ordering::SeqCst);
                        event_tx
                            .send(Event::SelectionOutsideLock)
                            .expect("test receiver");
                    }
                }
                "draft-read-before-write"
                    if !hook_selection_outside_lock.load(Ordering::SeqCst)
                        && !hook_paused.swap(true, Ordering::SeqCst) =>
                {
                    event_tx.send(Event::FirstPaused).expect("test receiver");
                    hook_released.wait();
                }
                _ => {}
            }
        }));

        let first_journal = Arc::clone(&journal);
        let first_append = thread::spawn(move || {
            append_draft_event_at(
                &first_journal.path,
                "support_draft",
                route_event(first.timestamp_millis()),
                first,
            )
        });
        match event_rx.recv().expect("first append event") {
            Event::FirstPaused => {}
            Event::SelectionOutsideLock => {
                *hooks.lock().expect("hook lock") = None;
                first_append
                    .join()
                    .expect("first thread")
                    .expect("first append");
                panic!("segment selection started outside DRAFT_LOCK");
            }
            Event::SecondAwaitingSelection => {
                panic!("second append reached the lock before the first append paused");
            }
        }
        let second_journal = Arc::clone(&journal);
        let second_append = thread::spawn(move || {
            append_draft_event_at(
                &second_journal.path,
                "support_draft",
                route_event(second.timestamp_millis()),
                second,
            )
        });
        match event_rx.recv().expect("second append event") {
            Event::SecondAwaitingSelection => {}
            Event::SelectionOutsideLock => {
                *hooks.lock().expect("hook lock") = None;
                released.wait();
                first_append
                    .join()
                    .expect("first thread")
                    .expect("first append");
                second_append
                    .join()
                    .expect("second thread")
                    .expect("second append");
                panic!("segment selection started outside DRAFT_LOCK");
            }
            Event::FirstPaused => panic!("only the first append may pause before the write"),
        }
        released.wait();
        first_append
            .join()
            .expect("first thread")
            .expect("first append");
        *hooks.lock().expect("hook lock") = None;
        second_append
            .join()
            .expect("second thread")
            .expect("second append");
        assert!(
            !selection_outside_lock.load(Ordering::SeqCst),
            "segment selection started outside DRAFT_LOCK"
        );

        let selected = draft_path(&journal.path, "20260815", "100500_300");
        assert_eq!(
            fs::read_to_string(selected)
                .expect("read serialized segment")
                .lines()
                .count(),
            2
        );
        assert!(!draft_path(&journal.path, "20260815", "100700_300").exists());
    }

    #[test]
    fn ac13_caller_records_draft_before_it_appends_draft_event() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("record first");
        assert!(support_draft_index_path(&journal.path, "a1b2c3d4").is_file());
        assert!(!journal.path.join("chronicle/20260815/chat").exists());
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append second");
    }

    fn write_draft_event(journal: &Path, day: &str, segment: &str, draft_id: &str, extra: Value) {
        let path = draft_path(journal, day, segment);
        let mut event = route_event(1);
        event.insert("draft_id".to_owned(), json!(draft_id));
        event.insert("captured_day".to_owned(), json!(day));
        if let Value::Object(fields) = extra {
            event.extend(fields);
        }
        let mut stored = Map::new();
        stored.insert("kind".to_owned(), json!("support_draft"));
        stored.extend(event);
        write_draft(&path, &format!("{}\n", Value::Object(stored)));
    }

    #[test]
    fn ac14_load_draft_event_finds_event_in_only_segment() {
        let journal = TestJournal::new();
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        write_draft_event(
            &journal.path,
            "20260815",
            "100347_300",
            "a1b2c3d4",
            json!({}),
        );
        let loaded = load_draft_event(&journal.path, "a1b2c3d4")
            .expect("load")
            .expect("found");
        assert_eq!(loaded["kind"], "support_draft");
        assert_eq!(loaded["draft_id"], "a1b2c3d4");
        assert_eq!(loaded["captured_day"], "20260815");
    }

    #[test]
    fn ac15_load_draft_event_finds_event_in_later_segment() {
        let journal = TestJournal::new();
        record_draft_captured(&journal.path, "later-id", "20260815").expect("locator");
        write_draft_event(
            &journal.path,
            "20260815",
            "100000_300",
            "other-id",
            json!({"payload": {"body": "other"}}),
        );
        write_draft_event(
            &journal.path,
            "20260815",
            "100347_300",
            "later-id",
            json!({"payload": {"body": "later"}}),
        );
        let loaded = load_draft_event(&journal.path, "later-id")
            .expect("load")
            .expect("found");
        assert_eq!(loaded["draft_id"], "later-id");
        assert_eq!(loaded["payload"]["body"], "later");
    }

    #[test]
    fn ac16_load_draft_event_returns_none_without_locator_or_match() {
        let journal = TestJournal::new();
        assert_eq!(
            load_draft_event(&journal.path, "missing").expect("no locator"),
            None
        );
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        write_draft_event(
            &journal.path,
            "20260815",
            "100347_300",
            "other-id",
            json!({}),
        );
        write_draft_event(
            &journal.path,
            "20260816",
            "100347_300",
            "a1b2c3d4",
            json!({}),
        );
        assert_eq!(
            load_draft_event(&journal.path, "a1b2c3d4").expect("wrong day only"),
            None
        );
    }

    #[test]
    fn ac17_draft_outcome_soft_fails_and_round_trips_marks() {
        let journal = TestJournal::new();
        assert_eq!(
            resolve_draft_outcome(&journal.path, "a1b2c3d4").expect("absent"),
            None
        );
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        assert_eq!(
            resolve_draft_outcome(&journal.path, "a1b2c3d4").expect("unmarked"),
            None
        );
        mark_draft_submitted(&journal.path, "a1b2c3d4").expect("submit");
        assert_eq!(
            resolve_draft_outcome(&journal.path, "a1b2c3d4").expect("submitted"),
            Some("submitted".to_owned())
        );
        let cancelled = TestJournal::new();
        mark_draft_cancelled(&cancelled.path, "a1b2c3d4").expect("cancel");
        assert_eq!(
            resolve_draft_outcome(&cancelled.path, "a1b2c3d4").expect("cancelled"),
            Some("cancelled".to_owned())
        );
        let malformed = support_draft_outcome_path(&journal.path, "bad-json");
        fs::create_dir_all(malformed.parent().expect("outcome parent")).expect("parent");
        fs::write(&malformed, "{").expect("malformed mark");
        assert_eq!(
            resolve_draft_outcome(&journal.path, "bad-json").expect("malformed"),
            None
        );
    }

    #[test]
    fn ac18_mark_is_create_only_and_leaves_locator_untouched() {
        let journal = TestJournal::new();
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        let locator = support_draft_index_path(&journal.path, "a1b2c3d4");
        let before = fs::read(&locator).expect("read locator");
        mark_draft_submitted(&journal.path, "a1b2c3d4").expect("first mark");
        let mark = support_draft_outcome_path(&journal.path, "a1b2c3d4");
        let submitted = fs::read(&mark).expect("read submitted mark");
        assert_eq!(submitted, b"{\"outcome\":\"submitted\"}\n");
        mark_draft_submitted(&journal.path, "a1b2c3d4").expect("repeat same verb");
        assert_eq!(fs::read(&mark).expect("unchanged after repeat"), submitted);
        mark_draft_cancelled(&journal.path, "a1b2c3d4").expect("other verb after win");
        assert_eq!(
            fs::read(&mark).expect("unchanged after other verb"),
            submitted
        );
        assert_eq!(
            resolve_draft_outcome(&journal.path, "a1b2c3d4").expect("winner"),
            Some("submitted".to_owned())
        );
        assert_eq!(fs::read(locator).expect("locator untouched"), before);
    }

    #[test]
    fn support_draft_round_trip_writes_support_drafts_stream() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_draft_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append");
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        let loaded = load_draft_event(&journal.path, "a1b2c3d4")
            .expect("load")
            .expect("present");
        assert_eq!(loaded["draft_id"], "a1b2c3d4");
        assert!(draft_path(&journal.path, "20260815", "100347_300").is_file());
        assert!(
            journal
                .path
                .join("chronicle/20260815/support-drafts")
                .is_dir()
        );
        assert!(journal.path.join("streams/support-drafts.json").is_file());
        assert!(!journal.path.join("streams/chat.json").exists());
        assert!(
            !journal
                .path
                .join("chronicle/20260815/chat/100347_300/chat.jsonl")
                .exists()
        );
    }

    #[test]
    fn load_draft_event_ignores_legacy_chat_jsonl() {
        let journal = TestJournal::new();
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("locator");
        let mut event = route_event(1);
        event.insert("kind".to_owned(), json!("support_draft"));
        let old = journal
            .path
            .join("chronicle/20260815/chat/100000_300/chat.jsonl");
        fs::create_dir_all(old.parent().expect("legacy parent")).expect("legacy parent");
        fs::write(&old, format!("{}\n", Value::Object(event))).expect("plant legacy chat");
        assert_eq!(
            load_draft_event(&journal.path, "a1b2c3d4").expect("load"),
            None
        );
        assert!(
            !journal
                .path
                .join("chronicle/20260815/support-drafts")
                .exists()
        );
    }
}
