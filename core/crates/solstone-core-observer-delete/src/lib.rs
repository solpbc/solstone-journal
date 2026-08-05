// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-authorized deletion of location-data source files.
//!
//! This crate owns destructive location-data cleanup and its durable
//! `health/observer-delete` ledger. Chronicle paths are always re-resolved from
//! discovered `(day, stream, segment)` names through `SegmentDir`.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use solstone_core_indexer_store::db::prune_chunks_by_stream;
use solstone_core_journal_io::{
    AppendError, AtomicWriteError, PathError, PathOrDay, append_jsonl, atomic_replace, day_dirs,
    iter_segments,
};
use solstone_core_segment::{
    SegmentDir, SegmentError, is_reserved_name, is_valid_device_did, write_tombstone,
};

const LOCATION_STREAM: &str = "location";
const LOCATION_ORIGINAL: &str = "location.jsonl";
const ITEM_SIDECAR: &str = "item.json";
const TOMBSTONE: &str = "tombstone.json";
const LEDGER_EVENT: &str = "location_source_delete";
const MODE_TOMBSTONE: &str = "tombstone-created";
const MODE_MIXED: &str = "mixed-location-stripped";

const SEGMENT_NOT_REMOVED_REASON: &str =
    "This segment could not be removed from disk. Try again after checking file permissions.";
const INDEX_NOT_REMOVED_REASON: &str = "The search index could not be updated. The imported files may be gone, but search results may still mention them until this is repaired.";
const STREAM_STATE_NOT_REMOVED_REASON: &str = "The stream state file could not be removed from disk. Try again after checking file permissions.";
const HISTORY_NOT_REMOVED_REASON: &str = "Observer history could not be updated. The imported files may be gone, but this source may still appear there until this is repaired.";
const ORIGINAL_NOT_REMOVED_REASON: &str =
    "This source file could not be removed from disk. Try again after checking file permissions.";
const LEDGER_NOT_REMOVED_REASON: &str =
    "The deletion ledger could not be updated. No location data was removed from this segment.";
const RETAINED_HISTORY_REASON: &str = "This source was imported together with others in one record; its history entry can't be removed on its own.";

/// Receipt returned by one location-source delete operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeleteReceipt {
    pub target: DeleteTarget,
    pub removed: RemovedCounts,
    pub not_confirmed: Vec<ReceiptEntry>,
    pub not_removed: Vec<ReceiptEntry>,
    pub backup_hosted: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeleteTarget {
    pub stream: String,
    pub journal: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RemovedCounts {
    pub originals: u64,
    pub segments: u64,
    pub mixed_segments: u64,
    /// Compatibility counter; native deletion creates no in-segment derived cleanup.
    pub in_segment_derived: u64,
    pub index_chunks: u64,
    pub stream_identity: u64,
    pub history_rows: u64,
    pub days: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReceiptEntry {
    pub what: String,
    pub plain_reason: String,
}

/// Journal-level failures that prevent a trustworthy delete receipt.
#[derive(Debug)]
pub enum DeleteError {
    Io { path: PathBuf, source: io::Error },
    Path(PathError),
    Segment(SegmentError),
}

impl fmt::Display for DeleteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Path(error) => error.fmt(formatter),
            Self::Segment(error) => error.fmt(formatter),
        }
    }
}

impl Error for DeleteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Path(error) => Some(error),
            Self::Segment(error) => Some(error),
        }
    }
}

impl From<PathError> for DeleteError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<SegmentError> for DeleteError {
    fn from(error: SegmentError) -> Self {
        Self::Segment(error)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Touch {
    day: String,
    stream: String,
    segment: String,
}

#[derive(Serialize)]
struct LedgerRow<'a> {
    event_kind: &'static str,
    recorded_at: &'a str,
    day: &'a str,
    stream: &'a str,
    segment: &'a str,
    mode: &'a str,
}

/// Delete the single owner-authorized location source from `journal`.
pub fn delete_location_source(journal: &Path) -> Result<DeleteReceipt, DeleteError> {
    let canonical_journal = fs::canonicalize(journal).map_err(|source| DeleteError::Io {
        path: journal.to_path_buf(),
        source,
    })?;
    let mut receipt = DeleteReceipt {
        target: DeleteTarget {
            stream: LOCATION_STREAM.to_owned(),
            journal: canonical_journal.to_string_lossy().into_owned(),
        },
        removed: RemovedCounts::default(),
        not_confirmed: Vec::new(),
        not_removed: Vec::new(),
        backup_hosted: "not confirmed".to_owned(),
    };

    let current_days = delete_current_sources(&canonical_journal, &mut receipt)?;
    receipt.removed.days = current_days.len() as u64;
    prune_outside_chronicle(&canonical_journal, &mut receipt);

    let touches = collect_delete_touches(&canonical_journal)?;
    receipt.not_confirmed = not_confirmed_entries(&canonical_journal, &touches)?;
    Ok(receipt)
}

fn delete_current_sources(
    journal: &Path,
    receipt: &mut DeleteReceipt,
) -> Result<BTreeSet<String>, DeleteError> {
    let mut days: Vec<String> = day_dirs(journal)?.into_keys().collect();
    days.sort();
    let mut completed_days = BTreeSet::new();
    for day in days {
        let mut segments = iter_segments(journal, PathOrDay::Day(&day))?;
        segments.sort_by(|left, right| {
            (left.stream.as_str(), left.key.as_str())
                .cmp(&(right.stream.as_str(), right.key.as_str()))
        });
        for discovered in segments {
            let segment = SegmentDir::resolve(journal, &day, &discovered.key, &discovered.stream)?;
            let classification = classify_segment(segment.path())?;
            let Some(mode) = classification else { continue };
            let timestamp = now_rfc3339();
            if let Err(error) = append_ledger(
                journal,
                &day,
                &discovered.stream,
                &discovered.key,
                mode.ledger_mode(),
                &timestamp,
            ) {
                receipt.not_removed.push(ReceiptEntry {
                    what: segment_label(
                        &day,
                        &discovered.stream,
                        &discovered.key,
                        "deletion ledger",
                    ),
                    plain_reason: LEDGER_NOT_REMOVED_REASON.to_owned(),
                });
                let _ = error;
                continue;
            }
            match mode {
                Classification::Mixed => {
                    if remove_location_original(segment.path()).is_err() {
                        receipt.not_removed.push(ReceiptEntry {
                            what: segment_label(
                                &day,
                                &discovered.stream,
                                &discovered.key,
                                "location data",
                            ),
                            plain_reason: ORIGINAL_NOT_REMOVED_REASON.to_owned(),
                        });
                        continue;
                    }
                    receipt.removed.originals += 1;
                    receipt.removed.mixed_segments += 1;
                    completed_days.insert(day.clone());
                }
                Classification::LocationOnly(mut names) => {
                    names.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
                    let did = load_device_did(segment.path());
                    let mut failed = None;
                    for name in names {
                        if let Err(_error) = remove_segment_file(&segment.path().join(&name)) {
                            failed = Some(name);
                            break;
                        }
                    }
                    if let Some(name) = failed {
                        receipt.not_removed.push(ReceiptEntry {
                            what: segment_label(&day, &discovered.stream, &discovered.key, &name),
                            plain_reason: SEGMENT_NOT_REMOVED_REASON.to_owned(),
                        });
                        continue;
                    }
                    if write_tombstone(&segment, &timestamp, &did).is_err() {
                        receipt.not_removed.push(ReceiptEntry {
                            what: segment_label(
                                &day,
                                &discovered.stream,
                                &discovered.key,
                                TOMBSTONE,
                            ),
                            plain_reason: SEGMENT_NOT_REMOVED_REASON.to_owned(),
                        });
                        continue;
                    }
                    receipt.removed.originals += 1;
                    receipt.removed.segments += 1;
                    completed_days.insert(day.clone());
                }
            }
        }
    }
    Ok(completed_days)
}

fn prune_outside_chronicle(journal: &Path, receipt: &mut DeleteReceipt) {
    match prune_chunks_by_stream(journal, LOCATION_STREAM) {
        Ok(counts) => receipt.removed.index_chunks = counts.chunks,
        Err(_) => receipt.not_removed.push(ReceiptEntry {
            what: "search index".to_owned(),
            plain_reason: INDEX_NOT_REMOVED_REASON.to_owned(),
        }),
    }
    let stream_state = journal.join("streams").join("location.json");
    match fs::remove_file(&stream_state) {
        Ok(()) => receipt.removed.stream_identity = 1,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => receipt.not_removed.push(ReceiptEntry {
            what: "location stream state".to_owned(),
            plain_reason: STREAM_STATE_NOT_REMOVED_REASON.to_owned(),
        }),
    }
    match prune_history_by_stream(journal, LOCATION_STREAM) {
        Ok(rows) => receipt.removed.history_rows = rows,
        Err(_) => receipt.not_removed.push(ReceiptEntry {
            what: "observer history".to_owned(),
            plain_reason: HISTORY_NOT_REMOVED_REASON.to_owned(),
        }),
    }
}

enum Classification {
    Mixed,
    LocationOnly(Vec<String>),
}

impl Classification {
    fn ledger_mode(&self) -> &'static str {
        match self {
            Self::Mixed => MODE_MIXED,
            Self::LocationOnly(_) => MODE_TOMBSTONE,
        }
    }
}

fn classify_segment(path: &Path) -> Result<Option<Classification>, DeleteError> {
    let mut location_present = false;
    let mut mixed = false;
    let mut removable = Vec::new();
    for entry in fs::read_dir(path).map_err(|source| DeleteError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| DeleteError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| DeleteError::Io {
            path: entry.path(),
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            mixed = true;
            continue;
        };
        if name == LOCATION_ORIGINAL {
            if file_type.is_dir() {
                mixed = true;
            } else {
                location_present = true;
                removable.push(name);
            }
            continue;
        }
        if name == TOMBSTONE {
            continue;
        }
        if file_type.is_dir() || !(is_reserved_name(&name) || name == ITEM_SIDECAR) {
            mixed = true;
            continue;
        }
        removable.push(name);
    }
    if !location_present {
        return Ok(None);
    }
    if mixed {
        Ok(Some(Classification::Mixed))
    } else {
        Ok(Some(Classification::LocationOnly(removable)))
    }
}

fn remove_location_original(path: &Path) -> io::Result<()> {
    remove_segment_file(&path.join(LOCATION_ORIGINAL))
}

fn remove_segment_file(path: &Path) -> io::Result<()> {
    #[cfg(test)]
    if REMOVE_FAILURE
        .with(|name| name.borrow().as_deref() == Some(path.file_name().unwrap_or_default()))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "test removal failure",
        ));
    }
    fs::remove_file(path)
}

fn append_ledger(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    mode: &str,
    recorded_at: &str,
) -> Result<(), AppendError> {
    append_jsonl(
        ledger_path(journal, day),
        &LedgerRow {
            event_kind: LEDGER_EVENT,
            recorded_at,
            day,
            stream,
            segment,
            mode,
        },
    )
}

fn ledger_path(journal: &Path, day: &str) -> PathBuf {
    journal
        .join("health")
        .join("observer-delete")
        .join(format!("{day}.jsonl"))
}

fn collect_delete_touches(journal: &Path) -> Result<BTreeSet<Touch>, DeleteError> {
    let mut touches = ledger_touches(journal)?;
    let mut days: Vec<String> = day_dirs(journal)?.into_keys().collect();
    days.sort();
    for day in days {
        for discovered in iter_segments(journal, PathOrDay::Day(&day))? {
            let segment = SegmentDir::resolve(journal, &day, &discovered.key, &discovered.stream)?;
            if segment.path().join(TOMBSTONE).is_file() {
                touches.insert(Touch {
                    day: day.clone(),
                    stream: discovered.stream,
                    segment: discovered.key,
                });
            }
        }
    }
    Ok(touches)
}

fn ledger_touches(journal: &Path) -> Result<BTreeSet<Touch>, DeleteError> {
    let directory = journal.join("health").join("observer-delete");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(source) => {
            return Err(DeleteError::Io {
                path: directory,
                source,
            });
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| DeleteError::Io {
            path: directory.clone(),
            source,
        })?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(path);
        }
    }
    paths.sort();
    let mut touches = BTreeSet::new();
    for path in paths {
        let Some(day) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if !is_day(day) {
            continue;
        }
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(record) = parse_ledger_touch(&value, day) else {
                continue;
            };
            touches.insert(record);
        }
    }
    Ok(touches)
}

fn parse_ledger_touch(value: &Value, expected_day: &str) -> Option<Touch> {
    let object = value.as_object()?;
    let day = object.get("day")?.as_str()?;
    let stream = object.get("stream")?.as_str()?;
    let segment = object.get("segment")?.as_str()?;
    let mode = object.get("mode")?.as_str()?;
    if object.get("event_kind")?.as_str()? != LEDGER_EVENT
        || day != expected_day
        || !is_day(day)
        || !is_plain_component(stream)
        || !is_plain_component(segment)
        || !matches!(mode, MODE_TOMBSTONE | MODE_MIXED)
    {
        return None;
    }
    Some(Touch {
        day: day.to_owned(),
        stream: stream.to_owned(),
        segment: segment.to_owned(),
    })
}

fn not_confirmed_entries(
    journal: &Path,
    touches: &BTreeSet<Touch>,
) -> Result<Vec<ReceiptEntry>, DeleteError> {
    let days: BTreeSet<&str> = touches.iter().map(|touch| touch.day.as_str()).collect();
    let mobile_streams: BTreeSet<&str> = touches
        .iter()
        .filter(|touch| touch.stream != LOCATION_STREAM)
        .map(|touch| touch.stream.as_str())
        .collect();
    let mut entries = facet_not_confirmed_entries(journal, &days)?;
    for stream in mobile_streams {
        if has_history_for_stream(journal, stream) {
            entries.push(ReceiptEntry {
                what: format!("{stream}: import history"),
                plain_reason: RETAINED_HISTORY_REASON.to_owned(),
            });
        }
    }
    Ok(entries)
}

fn facet_not_confirmed_entries(
    journal: &Path,
    days: &BTreeSet<&str>,
) -> Result<Vec<ReceiptEntry>, DeleteError> {
    let facets = journal.join("facets");
    let entries = match fs::read_dir(&facets) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(DeleteError::Io {
                path: facets,
                source,
            });
        }
    };
    let mut facet_paths = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|source| DeleteError::Io {
            path: facets.clone(),
            source,
        })?;
        if !entry.path().is_dir() || !entry.path().join("facet.json").is_file() {
            continue;
        }
        let Ok(value) = fs::read_to_string(entry.path().join("facet.json")) else {
            continue;
        };
        if serde_json::from_str::<Value>(&value).is_ok_and(|value| value.is_object()) {
            facet_paths.insert(
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            );
        }
    }
    let mut output = Vec::new();
    for day in days {
        for (name, facet) in &facet_paths {
            let day_display = day_display(day);
            for (relative, kind, reason) in [
                (
                    format!("entities/{day}.jsonl"),
                    "people and topics",
                    "This was merged into this day's people and topics; can't remove just this source's part.",
                ),
                (
                    format!("logs/{day}.jsonl"),
                    "activity summary",
                    "This was merged into this day's activity summary; can't remove just this source's part.",
                ),
                (
                    format!("news/{day}.md"),
                    "news",
                    "This was merged into this day's news; can't remove just this source's part.",
                ),
            ] {
                if facet.join(relative).is_file() {
                    output.push(ReceiptEntry {
                        what: format!("{name} {day_display}: {kind}"),
                        plain_reason: reason.to_owned(),
                    });
                }
            }
        }
    }
    Ok(output)
}

fn observer_prefixes(journal: &Path) -> Vec<String> {
    let directory = journal.join("apps").join("observer").join("observers");
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut prefixes = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(key) = value.get("key").and_then(Value::as_str) else {
            continue;
        };
        if key.len() >= 8 {
            prefixes.insert(key[..8].to_owned());
        }
    }
    prefixes.into_iter().collect()
}

fn has_history_for_stream(journal: &Path, stream: &str) -> bool {
    for prefix in observer_prefixes(journal) {
        let hist = journal
            .join("apps")
            .join("observer")
            .join("observers")
            .join(prefix)
            .join("hist");
        let Ok(entries) = fs::read_dir(hist) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            if text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|row| row.get("stream").and_then(Value::as_str) == Some(stream))
            {
                return true;
            }
        }
    }
    false
}

fn prune_history_by_stream(journal: &Path, stream: &str) -> Result<u64, AtomicWriteError> {
    let mut total = 0;
    for prefix in observer_prefixes(journal) {
        let hist = journal
            .join("apps")
            .join("observer")
            .join("observers")
            .join(prefix)
            .join("hist");
        let Ok(entries) = fs::read_dir(hist) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let rows: Vec<Value> = match text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(serde_json::from_str)
                .collect()
            {
                Ok(rows) => rows,
                Err(_) => continue,
            };
            let kept: Vec<Value> = rows
                .iter()
                .filter(|row| row.get("stream").and_then(Value::as_str) != Some(stream))
                .cloned()
                .collect();
            let removed = rows.len() - kept.len();
            if removed == 0 {
                continue;
            }
            let mut content = String::new();
            for row in kept {
                content.push_str(&serde_json::to_string(&row).expect("JSON value serializes"));
                content.push('\n');
            }
            atomic_replace(&path, content.as_bytes(), Default::default())?;
            total += removed as u64;
        }
    }
    Ok(total)
}

fn load_device_did(segment: &Path) -> String {
    let Ok(text) = fs::read_to_string(segment.join("device.json")) else {
        return "unknown".to_owned();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return "unknown".to_owned();
    };
    let Some(did) = value.get("did").and_then(Value::as_str) else {
        return "unknown".to_owned();
    };
    if is_valid_device_did(did) {
        did.to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn segment_label(day: &str, stream: &str, segment: &str, target: &str) -> String {
    format!("{stream} {} {segment}: {target}", day_display(day))
}

fn day_display(day: &str) -> String {
    format!("{}-{}-{}", &day[..4], &day[4..6], &day[6..8])
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_plain_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['/', '\\'])
        && !matches!(value, "." | "..")
        && !value.starts_with('.')
}

#[cfg(test)]
thread_local! {
    static REMOVE_FAILURE: std::cell::RefCell<Option<std::ffi::OsString>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    use solstone_core_indexer_store::db::open_index;

    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-core-observer-delete-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn journal(&self) -> PathBuf {
            let journal = self.path.join("journal");
            fs::create_dir(&journal).unwrap();
            journal
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn segment(journal: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
        let path = journal.join("chronicle").join(day).join(stream).join(key);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn location_only(journal: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
        let path = segment(journal, day, stream, key);
        fs::write(path.join(LOCATION_ORIGINAL), b"location").unwrap();
        fs::write(path.join("stream.json"), b"{}").unwrap();
        fs::write(
            path.join("device.json"),
            format!(r#"{{"did":"{}","jid":"wrong"}}"#, did()),
        )
        .unwrap();
        path
    }

    fn mixed(journal: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
        let path = location_only(journal, day, stream, key);
        fs::write(path.join("audio.m4a"), b"audio").unwrap();
        path
    }

    fn did() -> &'static str {
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }

    fn add_facet_artifacts(journal: &Path, facet: &str, day: &str) {
        let facet_dir = journal.join("facets").join(facet);
        fs::create_dir_all(facet_dir.join("entities")).unwrap();
        fs::create_dir_all(facet_dir.join("logs")).unwrap();
        fs::create_dir_all(facet_dir.join("news")).unwrap();
        fs::write(facet_dir.join("facet.json"), "{}").unwrap();
        fs::write(
            facet_dir.join("entities").join(format!("{day}.jsonl")),
            "{}",
        )
        .unwrap();
        fs::write(facet_dir.join("logs").join(format!("{day}.jsonl")), "{}").unwrap();
        fs::write(facet_dir.join("news").join(format!("{day}.md")), "news").unwrap();
    }

    fn add_history(journal: &Path, stream: &str) {
        let prefix = "abcd1234";
        let observers = journal.join("apps/observer/observers");
        fs::create_dir_all(observers.join(prefix).join("hist")).unwrap();
        fs::write(
            observers.join(format!("{prefix}.json")),
            r#"{"key":"abcd1234ffffffffffffffffffffffffffffffffffffffffffffffffffffffff"}"#,
        )
        .unwrap();
        fs::write(
            observers.join(prefix).join("hist/20260804.jsonl"),
            format!("{{\"stream\":\"{stream}\",\"segment\":\"120000_60\"}}\n"),
        )
        .unwrap();
    }

    #[test]
    fn location_only_ends_with_only_tombstone_and_real_did() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        let path = location_only(&journal, "20260804", "location", "120000_60");

        let receipt = delete_location_source(&journal).unwrap();

        let names: BTreeSet<_> = fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, BTreeSet::from([std::ffi::OsString::from(TOMBSTONE)]));
        let tombstone: Value =
            serde_json::from_slice(&fs::read(path.join(TOMBSTONE)).unwrap()).unwrap();
        assert_eq!(tombstone.as_object().unwrap().len(), 3);
        assert_eq!(tombstone["did"], did());
        assert_eq!(receipt.removed.originals, 1);
        assert_eq!(receipt.removed.segments, 1);
        assert_eq!(receipt.removed.days, 1);

        let rerun = delete_location_source(&journal).unwrap();
        assert!(path.join(TOMBSTONE).is_file());
        assert_eq!(rerun.removed.originals, 0);
        assert_eq!(rerun.removed.segments, 0);
    }

    #[test]
    fn mixed_segment_loses_only_location_and_gets_no_tombstone() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        let path = mixed(&journal, "20260804", "pixel", "120000_60");

        let receipt = delete_location_source(&journal).unwrap();

        assert!(!path.join(LOCATION_ORIGINAL).exists());
        assert_eq!(fs::read(path.join("audio.m4a")).unwrap(), b"audio");
        assert!(path.join("stream.json").is_file());
        assert!(!path.join(TOMBSTONE).exists());
        assert_eq!(receipt.removed.mixed_segments, 1);
        assert_eq!(receipt.removed.segments, 0);
    }

    #[test]
    fn partial_third_sorted_delete_has_no_tombstone_and_names_actual_target() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        let path = location_only(&journal, "20260804", "location", "120000_60");
        fs::write(path.join("events.jsonl"), "{}").unwrap();
        fs::write(path.join("ingest.json"), "{}").unwrap();
        let third = ["device.json", "events.jsonl", "ingest.json"];
        REMOVE_FAILURE.with(|value| *value.borrow_mut() = Some(third[2].into()));

        let receipt = delete_location_source(&journal).unwrap();

        REMOVE_FAILURE.with(|value| *value.borrow_mut() = None);
        assert!(!path.join("device.json").exists());
        assert!(!path.join("events.jsonl").exists());
        assert!(path.join("ingest.json").exists());
        assert!(path.join(LOCATION_ORIGINAL).exists());
        assert!(!path.join(TOMBSTONE).exists());
        assert_eq!(
            receipt.not_removed[0].what,
            "location 2026-08-04 120000_60: ingest.json"
        );
    }

    #[test]
    fn durable_touches_keep_exact_facet_and_mobile_history_disclosures_on_rerun() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        location_only(&journal, "20260804", "location", "120000_60");
        mixed(&journal, "20260805", "pixel", "120000_60");
        for day in ["20260804", "20260805"] {
            for facet in ["personal", "work"] {
                add_facet_artifacts(&journal, facet, day);
            }
        }
        add_history(&journal, "pixel");

        delete_location_source(&journal).unwrap();
        let second = delete_location_source(&journal).unwrap();

        let expected: BTreeSet<String> = [
            "personal 2026-08-04: activity summary",
            "personal 2026-08-04: news",
            "personal 2026-08-04: people and topics",
            "work 2026-08-04: activity summary",
            "work 2026-08-04: news",
            "work 2026-08-04: people and topics",
            "personal 2026-08-05: activity summary",
            "personal 2026-08-05: news",
            "personal 2026-08-05: people and topics",
            "work 2026-08-05: activity summary",
            "work 2026-08-05: news",
            "work 2026-08-05: people and topics",
            "pixel: import history",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let actual: BTreeSet<String> = second
            .not_confirmed
            .iter()
            .map(|entry| entry.what.clone())
            .collect();
        assert_eq!(actual, expected);
        assert_eq!(second.removed.days, 0);
    }

    #[test]
    fn outside_counters_prune_index_stream_state_and_location_history() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        location_only(&journal, "20260804", "location", "120000_60");
        let conn = open_index(&journal).unwrap();
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('location', 'a.md', '', '', 'test', 'location', 0, '')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO files(path, mtime) VALUES ('a.md', 1)", [])
            .unwrap();
        drop(conn);
        fs::create_dir_all(journal.join("streams")).unwrap();
        fs::write(journal.join("streams/location.json"), "{}").unwrap();
        add_history(&journal, LOCATION_STREAM);

        let receipt = delete_location_source(&journal).unwrap();

        assert_eq!(receipt.removed.index_chunks, 1);
        assert_eq!(receipt.removed.stream_identity, 1);
        assert_eq!(receipt.removed.history_rows, 1);
        assert!(!journal.join("streams/location.json").exists());
        assert!(!has_history_for_stream(&journal, LOCATION_STREAM));
    }

    #[test]
    fn unknown_device_provenance_is_literal_unknown() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        let path = segment(&journal, "20260804", "location", "120000_60");
        fs::write(path.join(LOCATION_ORIGINAL), "location").unwrap();
        fs::write(path.join("device.json"), r#"{"jid":"wrong"}"#).unwrap();

        delete_location_source(&journal).unwrap();

        let value: Value =
            serde_json::from_slice(&fs::read(path.join(TOMBSTONE)).unwrap()).unwrap();
        assert_eq!(value["did"], "unknown");
    }

    #[test]
    fn ledger_prior_evidence_survives_a_later_failed_removal() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        location_only(&journal, "20260804", "location", "120000_60");
        add_facet_artifacts(&journal, "work", "20260804");
        delete_location_source(&journal).unwrap();
        let retry = location_only(&journal, "20260805", "location", "120000_60");
        REMOVE_FAILURE.with(|value| *value.borrow_mut() = Some("device.json".into()));

        let receipt = delete_location_source(&journal).unwrap();

        REMOVE_FAILURE.with(|value| *value.borrow_mut() = None);
        assert!(retry.join(LOCATION_ORIGINAL).exists());
        assert!(
            receipt
                .not_removed
                .iter()
                .any(|entry| entry.what.ends_with("device.json"))
        );
        assert!(
            receipt
                .not_confirmed
                .iter()
                .any(|entry| entry.what == "work 2026-08-04: people and topics")
        );
    }

    #[test]
    #[cfg(unix)]
    fn segment_symlink_escape_is_rejected_before_delete() {
        let temporary = TempDir::new();
        let journal = temporary.journal();
        let day = journal.join("chronicle/20260804/location");
        let outside = temporary.path.join("outside");
        fs::create_dir_all(&day).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, day.join("120000_60")).unwrap();
        assert!(delete_location_source(&journal).is_err());
    }
}
