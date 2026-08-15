// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native append path for support-draft chat events and their day locator.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{Map, Value};
use solstone_core_indexer_store::scan::rescan_file;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, write_jsonl};
use solstone_core_segment::{Kind, SegmentDir, StreamHints, advance_unbound_stream};
use thiserror::Error;

const CHAT_STREAM: &str = "chat";
const SUPPORT_DRAFT: &str = "support_draft";
const SEGMENT_WINDOW_MS: i64 = 300_000;

static CHAT_LOCK: Mutex<()> = Mutex::new(());

/// Failure while appending a support-draft chat event or its draft locator.
#[derive(Debug, Error)]
pub enum ChatAppendError {
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
    #[error("unknown chat event kind: {kind}")]
    UnknownKind { kind: String },
    /// A required support-draft field was absent.
    #[error("missing required support-draft field: {field}")]
    MissingField { field: &'static str },
    /// An event timestamp was not an integer milliseconds value.
    #[error("chat event timestamp must be an integer")]
    InvalidTimestamp,
    /// A timestamp could not be resolved as a local wall-clock time.
    #[error("resolve local chat time for timestamp {timestamp}")]
    LocalTime { timestamp: i64 },
    /// A discovered chat segment was not a valid local timestamp key.
    #[error("invalid chat segment key {segment} for day {day}")]
    InvalidSegmentKey { day: String, segment: String },
    /// Reading an existing chat file failed.
    #[error("read chat file {path}: {source}")]
    ChatRead { path: PathBuf, source: io::Error },
    /// An existing nonblank chat line was not valid JSON.
    #[error("malformed chat line {line} in {path}: {source}")]
    MalformedChatLine {
        path: PathBuf,
        line: usize,
        source: serde_json::Error,
    },
    /// An existing nonblank chat line was valid JSON but not an object.
    #[error("chat line {line} in {path} is not an object")]
    ChatLineNotObject { path: PathBuf, line: usize },
    /// Replacing the chat file or draft locator failed.
    #[error("atomic write {path}: {source}")]
    AtomicWrite {
        path: PathBuf,
        source: solstone_core_journal_io::AtomicWriteError,
    },
    /// Resolving the chat segment path failed.
    #[error("resolve chat segment: {source}")]
    SegmentPath {
        source: solstone_core_segment::SegmentError,
    },
    /// Advancing the unbound chat stream or writing its marker failed.
    #[error("advance chat stream: {source}")]
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
) -> Result<Value, ChatAppendError> {
    append_chat_event(journal, SUPPORT_DRAFT, event)
}

fn append_chat_event(
    journal: &Path,
    kind: &str,
    event: Map<String, Value>,
) -> Result<Value, ChatAppendError> {
    append_chat_event_at(journal, kind, event, Local::now())
}

/// Record the day containing one support draft for later bounded resolution.
pub fn record_draft_captured(
    journal: &Path,
    draft_id: &str,
    captured_day: &str,
) -> Result<(), ChatAppendError> {
    validate_draft_id(draft_id)?;
    require_journal_root(journal)?;
    let path = support_draft_index_path(journal, draft_id);
    let captured_day = serde_json::to_string(captured_day)
        .expect("serializing a Rust string for a support-draft locator cannot fail");
    let contents = format!("{{\"captured_day\":{captured_day}}}\n");
    atomic_replace(&path, contents.as_bytes(), AtomicWriteOptions::default())
        .map_err(|source| ChatAppendError::AtomicWrite { path, source })
}

/// Resolve a support draft's captured day without changing journal state.
pub fn resolve_draft_day(
    journal: &Path,
    draft_id: &str,
) -> Result<Option<String>, ChatAppendError> {
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

fn append_chat_event_at(
    journal: &Path,
    kind: &str,
    mut event: Map<String, Value>,
    now: DateTime<Local>,
) -> Result<Value, ChatAppendError> {
    if !event.contains_key("ts") {
        event.insert("ts".to_owned(), Value::from(now.timestamp_millis()));
    }
    validate_event(kind, &event)?;
    require_journal_root(journal)?;

    let timestamp = event
        .get("ts")
        .and_then(Value::as_i64)
        .ok_or(ChatAppendError::InvalidTimestamp)?;
    let (day, segment) = current_segment_key(journal, timestamp)?;
    let segment_dir = SegmentDir::resolve(journal, &day, &segment, CHAT_STREAM)
        .map_err(|source| ChatAppendError::SegmentPath { source })?;
    let chat_path = segment_dir.path().join("chat.jsonl");
    let mut stored = Map::new();
    stored.insert("kind".to_owned(), Value::String(kind.to_owned()));
    stored.extend(event);
    let stored = Value::Object(stored);

    {
        let _guard = CHAT_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let existed = chat_path.exists();
        let mut events = read_events_file(&chat_path)?;
        pause_at("chat-read-before-write");
        events.push(stored.clone());
        write_jsonl(&chat_path, events, AtomicWriteOptions::default()).map_err(|source| {
            ChatAppendError::AtomicWrite {
                path: chat_path.clone(),
                source,
            }
        })?;
        if !existed {
            advance_unbound_stream(
                journal,
                CHAT_STREAM,
                &day,
                &segment,
                StreamHints {
                    kind: Some(Kind::Chat),
                    host: None,
                    platform: None,
                },
            )
            .map_err(|source| ChatAppendError::StreamAdvance { source })?;
        }
    }

    if let Err(error) = rescan_file(journal, &chat_path) {
        log::warn!(
            "support-draft chat rescan failed for {}: {error}",
            chat_path.display()
        );
    }
    Ok(stored)
}

fn validate_event(kind: &str, event: &Map<String, Value>) -> Result<(), ChatAppendError> {
    if kind != SUPPORT_DRAFT {
        return Err(ChatAppendError::UnknownKind {
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
            return Err(ChatAppendError::MissingField { field });
        }
    }
    if !event.get("ts").is_some_and(Value::is_i64) {
        return Err(ChatAppendError::InvalidTimestamp);
    }
    Ok(())
}

fn require_journal_root(journal: &Path) -> Result<(), ChatAppendError> {
    match fs::metadata(journal) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(ChatAppendError::JournalRootNotDirectory {
            path: journal.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(ChatAppendError::JournalRootMissing {
                path: journal.to_path_buf(),
            })
        }
        Err(source) => Err(ChatAppendError::JournalRootIo {
            path: journal.to_path_buf(),
            source,
        }),
    }
}

fn validate_draft_id(draft_id: &str) -> Result<(), ChatAppendError> {
    if draft_id.is_empty() || draft_id.contains('/') || matches!(draft_id, "." | "..") {
        return Err(ChatAppendError::InvalidDraftId);
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

fn current_segment_key(
    journal: &Path,
    timestamp: i64,
) -> Result<(String, String), ChatAppendError> {
    let event_time = local_time(timestamp)?;
    let day = event_time.format("%Y%m%d").to_string();
    let mut existing = chat_segments(journal, &day)?;
    if existing.is_empty() {
        return Ok((day, segment_key_for_start(event_time.naive_local())));
    }
    existing.sort();
    let current = existing.pop().expect("checked nonempty chat segment list");
    let current_start = segment_start_timestamp(&day, &current)?;
    if i128::from(timestamp) - i128::from(current_start) >= i128::from(SEGMENT_WINDOW_MS) {
        Ok((day, segment_key_for_start(event_time.naive_local())))
    } else {
        Ok((day, current))
    }
}

fn local_time(timestamp: i64) -> Result<DateTime<Local>, ChatAppendError> {
    Local
        .timestamp_millis_opt(timestamp)
        .single()
        .ok_or(ChatAppendError::LocalTime { timestamp })
}

fn chat_segments(journal: &Path, day: &str) -> Result<Vec<String>, ChatAppendError> {
    let directory = journal.join("chronicle").join(day).join(CHAT_STREAM);
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
            return Err(ChatAppendError::ChatRead {
                path: directory,
                source,
            });
        }
    };
    let mut segments = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ChatAppendError::ChatRead {
            path: directory.clone(),
            source,
        })?;
        if !entry
            .file_type()
            .map_err(|source| ChatAppendError::ChatRead {
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
    let bytes = value.as_bytes();
    bytes.len() > 7
        && bytes[..6].iter().all(u8::is_ascii_digit)
        && bytes[6] == b'_'
        && bytes[7..].iter().all(u8::is_ascii_digit)
}

fn segment_key_for_start(start: NaiveDateTime) -> String {
    format!("{}_300", start.format("%H%M%S"))
}

fn segment_start_timestamp(day: &str, segment: &str) -> Result<i64, ChatAppendError> {
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").map_err(|_| {
        ChatAppendError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        }
    })?;
    let time = chrono::NaiveTime::parse_from_str(&segment[..6], "%H%M%S").map_err(|_| {
        ChatAppendError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        }
    })?;
    Local
        .from_local_datetime(&NaiveDateTime::new(date, time))
        .single()
        .map(|time| time.timestamp_millis())
        .ok_or_else(|| ChatAppendError::InvalidSegmentKey {
            day: day.to_owned(),
            segment: segment.to_owned(),
        })
}

fn read_events_file(path: &Path) -> Result<Vec<Value>, ChatAppendError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ChatAppendError::ChatRead {
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
            serde_json::from_str(line).map_err(|source| ChatAppendError::MalformedChatLine {
                path: path.to_path_buf(),
                line: offset + 1,
                source,
            })?;
        if !value.is_object() {
            return Err(ChatAppendError::ChatLineNotObject {
                path: path.to_path_buf(),
                line: offset + 1,
            });
        }
        events.push(value);
    }
    Ok(events)
}

#[cfg(any(test, feature = "test-hooks"))]
type PauseHook = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

#[cfg(any(test, feature = "test-hooks"))]
static PAUSE_HOOK: std::sync::OnceLock<Mutex<Option<PauseHook>>> = std::sync::OnceLock::new();

#[cfg(any(test, feature = "test-hooks"))]
fn pause_at(point: &str) {
    let hook = PAUSE_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if let Some(hook) = hook {
        hook(point);
    }
}

#[cfg(not(any(test, feature = "test-hooks")))]
fn pause_at(_: &str) {}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone, Utc};
    use serde_json::{Map, Value, json};

    use super::{
        CHAT_STREAM, ChatAppendError, PAUSE_HOOK, append_chat_event_at, append_support_draft,
        record_draft_captured, resolve_draft_day, support_draft_index_path,
    };

    static NEXT_JOURNAL: AtomicU64 = AtomicU64::new(0);

    struct TestJournal {
        path: PathBuf,
    }

    impl TestJournal {
        fn new() -> Self {
            let sequence = NEXT_JOURNAL.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/var/tmp").join(format!(
                "solstone-core-chat-append-{}-{sequence}",
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

    fn chat_path(journal: &Path, day: &str, segment: &str) -> PathBuf {
        journal
            .join("chronicle")
            .join(day)
            .join(CHAT_STREAM)
            .join(segment)
            .join("chat.jsonl")
    }

    fn write_chat(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().expect("chat parent")).expect("create chat parent");
        fs::write(path, contents).expect("write chat");
    }

    #[test]
    fn ac1_support_draft_preserves_route_order_and_extra_fields() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let mut event = route_event(now.timestamp_millis());
        event.insert("future_field".to_owned(), json!({"kept": true}));

        let stored =
            append_chat_event_at(&journal.path, "support_draft", event, now).expect("append");
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
            append_chat_event_at(&journal.path, "support_draft", event, now).expect("append");
        assert_eq!(stored["ts"], now.timestamp_millis());
        assert_eq!(stored["payload"], Value::Null);
    }

    #[test]
    fn ac1a_event_jsonl_keeps_non_ascii_as_utf8() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let mut event = route_event(now.timestamp_millis());
        event.insert("payload".to_owned(), json!({"body": "café"}));
        append_chat_event_at(&journal.path, "support_draft", event, now).expect("append");
        let bytes =
            fs::read(chat_path(&journal.path, "20260815", "100347_300")).expect("read chat");
        assert!(
            bytes
                .windows("café".len())
                .any(|window| window == "café".as_bytes())
        );
        assert!(
            !String::from_utf8(bytes)
                .expect("utf8")
                .contains("\\\\u00e9")
        );
    }

    #[test]
    fn ac2_segment_uses_raw_time_and_signed_reuse_window() {
        let now = local_at(2026, 8, 15, 10, 3, 47);

        let raw = TestJournal::new();
        append_chat_event_at(
            &raw.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("raw append");
        assert!(chat_path(&raw.path, "20260815", "100347_300").is_file());
        assert!(!chat_path(&raw.path, "20260815", "100000_300").exists());

        let within = TestJournal::new();
        write_chat(&chat_path(&within.path, "20260815", "095848_300"), "{}\n");
        append_chat_event_at(
            &within.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("299ms-window append");
        assert!(!chat_path(&within.path, "20260815", "100347_300").exists());

        let boundary = TestJournal::new();
        write_chat(&chat_path(&boundary.path, "20260815", "095847_300"), "{}\n");
        append_chat_event_at(
            &boundary.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("300ms-window append");
        assert!(chat_path(&boundary.path, "20260815", "100347_300").is_file());

        let older = TestJournal::new();
        write_chat(&chat_path(&older.path, "20260815", "100500_300"), "{}\n");
        append_chat_event_at(
            &older.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("older event append");
        assert!(!chat_path(&older.path, "20260815", "100347_300").exists());
        assert_eq!(
            fs::read_to_string(chat_path(&older.path, "20260815", "100500_300"))
                .expect("read reused chat")
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
    }

    #[test]
    fn ac4_existing_events_survive_and_blank_lines_are_dropped() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let path = chat_path(&journal.path, "20260815", "100347_300");
        write_chat(&path, "{\"prior\":1}\n\n{\"prior\":2}\n");
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append");
        let contents = fs::read_to_string(path).expect("read chat");
        assert_eq!(contents.lines().count(), 3);
        assert!(!contents.contains("\n\n"));
    }

    #[test]
    fn ac5_malformed_line_refuses_append_without_replacing_file() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let path = chat_path(&journal.path, "20260815", "100347_300");
        write_chat(&path, "not-json\n");
        let before = fs::read(&path).expect("read original");
        assert!(matches!(
            append_chat_event_at(
                &journal.path,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(ChatAppendError::MalformedChatLine { .. })
        ));
        assert_eq!(fs::read(&path).expect("read unchanged"), before);
        write_chat(&path, "{\"prior\":true}\n");
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append after repair");
    }

    #[test]
    fn ac6_first_append_advances_unbound_stream_only_once() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("first append");
        let state = journal.path.join("streams/chat.json");
        let marker = journal
            .path
            .join("chronicle/20260815/chat/100347_300/stream.json");
        let first_state = fs::read(&state).expect("stream state");
        let first_marker = fs::read(&marker).expect("stream marker");
        let value: Value = serde_json::from_slice(&first_state).expect("stream json");
        assert_eq!(value["kind"], "chat");
        assert!(value.get("did").is_none());
        assert!(value.get("source").is_none());
        append_chat_event_at(
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
    fn ac7_index_failure_is_logged_and_does_not_fail_append() {
        let journal = TestJournal::new();
        fs::write(journal.path.join("indexer"), "not a directory").expect("block indexer");
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append despite index failure");
        assert!(chat_path(&journal.path, "20260815", "100347_300").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn ac8_chat_and_draft_locator_are_private_files() {
        use std::os::unix::fs::PermissionsExt;

        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append");
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("record draft");
        let chat_mode = fs::metadata(chat_path(&journal.path, "20260815", "100347_300"))
            .expect("chat metadata")
            .permissions()
            .mode()
            & 0o777;
        let index_mode = fs::metadata(support_draft_index_path(&journal.path, "a1b2c3d4"))
            .expect("locator metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(chat_mode, 0o600);
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
                Err(ChatAppendError::InvalidDraftId)
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
        let unreadable = support_draft_index_path(&journal.path, "unreadable");
        fs::create_dir_all(&unreadable).expect("unreadable shape");
        assert_eq!(
            resolve_draft_day(&journal.path, "unreadable").expect("unreadable"),
            None
        );
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
            append_chat_event_at(
                &missing,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(ChatAppendError::JournalRootMissing { .. })
        ));
        let file = journal.path.join("not-a-directory");
        fs::write(&file, "file").expect("write file root");
        assert!(matches!(
            append_chat_event_at(
                &file,
                "support_draft",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(ChatAppendError::JournalRootNotDirectory { .. })
        ));
    }

    #[test]
    fn unknown_kind_and_noninteger_timestamp_fail_before_write() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        assert!(matches!(
            append_chat_event_at(
                &journal.path,
                "other",
                route_event(now.timestamp_millis()),
                now
            ),
            Err(ChatAppendError::UnknownKind { .. })
        ));
        let mut event = route_event(now.timestamp_millis());
        event.insert("ts".to_owned(), json!("not-an-integer"));
        assert!(matches!(
            append_chat_event_at(&journal.path, "support_draft", event, now),
            Err(ChatAppendError::InvalidTimestamp)
        ));
        assert!(!journal.path.join("chronicle").exists());
    }

    #[test]
    fn ac12_static_lock_keeps_two_concurrent_appends() {
        let journal = Arc::new(TestJournal::new());
        let now = local_at(2026, 8, 15, 10, 3, 47);
        let entered = Arc::new(Barrier::new(2));
        let released = Arc::new(Barrier::new(2));
        let paused = Arc::new(AtomicBool::new(false));
        let hook_entered = Arc::clone(&entered);
        let hook_released = Arc::clone(&released);
        let hook_paused = Arc::clone(&paused);
        let hooks = PAUSE_HOOK.get_or_init(|| Mutex::new(None));
        *hooks.lock().expect("hook lock") = Some(Arc::new(move |point| {
            if point == "chat-read-before-write" && !hook_paused.swap(true, Ordering::SeqCst) {
                hook_entered.wait();
                hook_released.wait();
            }
        }));

        let first_journal = Arc::clone(&journal);
        let first = thread::spawn(move || {
            append_chat_event_at(
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
        let contents = fs::read_to_string(chat_path(&journal.path, "20260815", "100347_300"))
            .expect("read chat");
        assert_eq!(contents.lines().count(), 2);
    }

    #[test]
    fn ac13_caller_records_draft_before_it_appends_chat_event() {
        let journal = TestJournal::new();
        let now = local_at(2026, 8, 15, 10, 3, 47);
        record_draft_captured(&journal.path, "a1b2c3d4", "20260815").expect("record first");
        assert!(support_draft_index_path(&journal.path, "a1b2c3d4").is_file());
        assert!(!journal.path.join("chronicle/20260815/chat").exists());
        append_chat_event_at(
            &journal.path,
            "support_draft",
            route_event(now.timestamp_millis()),
            now,
        )
        .expect("append second");
    }
}
