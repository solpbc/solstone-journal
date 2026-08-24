// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Tolerant stream-registry inspection and targeted stream-tail repair.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    AtomicWriteError, JsonWriteOptions, LockError, LockOptions, MalformedPolicy,
    bump_stream_marker, hold_lock, read_json, write_json,
};

use crate::stream_record::{registry_json_paths, stream_record_path, write_stream_record};
use crate::{SegmentError, StreamRecord, is_reserved_name, is_safe_stream_component, list_days};

/// The tail derived from validated segment markers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkerTail<'a> {
    pub last_day: &'a str,
    pub last_segment: &'a str,
    pub max_seq: u64,
}

/// Why a marker-driven repair made no change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnchangedReason {
    AlreadyCurrent,
    RecordAhead,
}

/// The result of a marker-driven stream-tail repair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairOutcome {
    Repaired,
    Unchanged(UnchangedReason),
    NoRecord,
    Malformed,
    Locked,
    WriteFailed,
}

/// A tolerant registry scan: valid object records and isolated bad files.
#[derive(Debug, Default)]
pub struct TolerantStreamRecords {
    pub records: Vec<(String, Value)>,
    pub anomalies: Vec<(PathBuf, String)>,
}

/// Read one registry record without normalizing its keys or values.
pub fn read_stream_record(journal: &Path, name: &str) -> Result<Option<Value>, SegmentError> {
    if !is_safe_stream_component(name) {
        return Ok(None);
    }
    read_stream_record_value(&stream_record_path(journal, name))
}

/// Read every registry record independently, preserving valid raw objects when
/// another record is malformed.
pub fn list_stream_records_tolerant(journal: &Path) -> Result<TolerantStreamRecords, SegmentError> {
    let mut result = TolerantStreamRecords::default();
    for path in registry_json_paths(journal)? {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        match read_stream_record_value(&path) {
            Ok(Some(value)) if value.is_object() => result.records.push((name, value)),
            Ok(Some(_)) | Ok(None) | Err(_) => {
                result.anomalies.push((path, "malformed record".to_owned()));
            }
        }
    }
    Ok(result)
}

/// Repair an existing registry record from a marker-derived tail. The initial
/// existence check intentionally precedes lock acquisition so a marker-only
/// stream never creates the registry directory or a lock sidecar.
pub fn repair_stream_tail_from_markers(
    journal: &Path,
    stream: &str,
    marker_tail: &MarkerTail<'_>,
    lock_options: LockOptions,
) -> RepairOutcome {
    if !is_safe_stream_component(stream) {
        return RepairOutcome::NoRecord;
    }
    let path = stream_record_path(journal, stream);
    if !path.exists() {
        return RepairOutcome::NoRecord;
    }
    let _lock = match hold_lock(&path, lock_options) {
        Ok(lock) => lock,
        Err(_) => return RepairOutcome::Locked,
    };
    let value = match read_stream_record_value(&path) {
        Ok(Some(value)) if value.is_object() => value,
        Ok(Some(_)) | Ok(None) | Err(_) => return RepairOutcome::Malformed,
    };
    let record: StreamRecord = match serde_json::from_value(value.clone()) {
        Ok(record) => record,
        Err(_) => return RepairOutcome::Malformed,
    };
    if record.seq > marker_tail.max_seq {
        return RepairOutcome::Unchanged(UnchangedReason::RecordAhead);
    }
    if record.seq == marker_tail.max_seq
        && record.last_day.as_deref() == Some(marker_tail.last_day)
        && record.last_segment.as_deref() == Some(marker_tail.last_segment)
    {
        return RepairOutcome::Unchanged(UnchangedReason::AlreadyCurrent);
    }
    let mut value = value;
    let object = value.as_object_mut().expect("object checked above");
    object.insert("last_day".to_owned(), json!(marker_tail.last_day));
    object.insert("last_segment".to_owned(), json!(marker_tail.last_segment));
    object.insert("seq".to_owned(), json!(marker_tail.max_seq));
    match write_stream_record(&path, &value) {
        Ok(()) => RepairOutcome::Repaired,
        Err(_) => RepairOutcome::WriteFailed,
    }
}

/// Set a registry tail after prune has already determined that its recorded
/// tail is absent. This preserves the prune path's intentionally best-effort,
/// lock-free behavior.
pub fn set_stream_tail_unconditionally(
    journal: &Path,
    stream: &str,
    last_day: Option<&str>,
    last_segment: Option<&str>,
    max_seq: u64,
) -> StreamRecord {
    let path = stream_record_path(journal, stream);
    let existing =
        read_stream_record_value(&path).ok().flatten().and_then(
            |value| match serde_json::from_value::<StreamRecord>(value.clone()) {
                Ok(record) if value.is_object() => Some((value, record)),
                Err(_) | Ok(_) => None,
            },
        );
    if let Some((mut value, record)) = existing {
        let object = value.as_object_mut().expect("object checked above");
        object.insert("last_day".to_owned(), json!(last_day));
        object.insert("last_segment".to_owned(), json!(last_segment));
        object.insert("seq".to_owned(), json!(record.seq.max(max_seq)));
        let updated: StreamRecord = serde_json::from_value(value.clone())
            .expect("in-place mutation preserves a usable stream record");
        let _ = write_stream_record(&path, &value);
        return updated;
    }

    let state = default_stream_record(stream, last_day, last_segment, max_seq);
    let _ = write_stream_record(&path, &state);
    state
}

/// Touch a chronicle day's stream health marker.
pub fn touch_stream_health_marker(
    journal: &Path,
    day: &str,
) -> Result<(), solstone_core_journal_io::AtomicWriteError> {
    bump_stream_marker(journal, day).map(|_| ())
}

fn read_stream_record_value(path: &Path) -> Result<Option<Value>, SegmentError> {
    match read_json(path, None, MalformedPolicy::Raise) {
        Ok(value) => Ok(value),
        Err(error @ solstone_core_journal_io::ReadError::Malformed(_)) => {
            Err(SegmentError::MalformedStreamRecord {
                path: path.to_path_buf(),
                source: error,
            })
        }
        Err(error) => Err(SegmentError::Read(error)),
    }
}

fn default_stream_record(
    stream: &str,
    last_day: Option<&str>,
    last_segment: Option<&str>,
    max_seq: u64,
) -> StreamRecord {
    StreamRecord {
        name: stream.to_owned(),
        kind: "unknown".to_owned(),
        host: None,
        platform: None,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
        last_day: last_day.map(ToOwned::to_owned),
        last_segment: last_segment.map(ToOwned::to_owned),
        seq: max_seq,
        did: None,
        source: None,
    }
}

// ---------------------------------------------------------------------------
// One-time stream backfill.
//
// The native form of the `settings:001_backfill_streams` migration. Every
// signal, its precedence, the chronological relinking, and the stream-state
// rebuild reproduce the Python migration this replaces.
// ---------------------------------------------------------------------------

/// Which signal decided a segment's stream during a backfill, in the exact
/// precedence order the classifier applies them.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StreamBackfillSignal {
    /// An existing `stream.json` already naming a stream.
    ExistingMarker,
    /// An audio header's `stream` field.
    AudioStream,
    /// An audio header's `remote` field.
    AudioRemote,
    /// An audio header's truthy `imported` field.
    AudioImported,
    /// The presence of `imported_audio.jsonl`.
    ImportedJsonl,
    /// The `imports/*/segments.json` reverse index.
    ImportIndex,
    /// The `audio.jsonl` header's `host` field.
    AudioHost,
    /// A tmux-only capture with no recognized audio.
    TmuxOnly,
    /// The fallback host, with no segment-local evidence at all.
    HostnameFallback,
}

impl StreamBackfillSignal {
    /// The reporting name this signal is counted under.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ExistingMarker => "existing_marker",
            Self::AudioStream => "audio.jsonl_stream",
            Self::AudioRemote => "audio.jsonl_remote",
            Self::AudioImported => "audio.jsonl_imported",
            Self::ImportedJsonl => "imported_audio.jsonl",
            Self::ImportIndex => "import_reverse_index",
            Self::AudioHost => "audio.jsonl_host",
            Self::TmuxOnly => "tmux_only_segment",
            Self::HostnameFallback => "hostname_fallback",
        }
    }
}

impl fmt::Display for StreamBackfillSignal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One classified segment, as the backfill decided it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamClassification {
    /// The `YYYYMMDD` chronicle day.
    pub day: String,
    /// The segment directory name.
    pub segment: String,
    /// The stream the segment was assigned to.
    pub stream: String,
    /// The signal that decided it.
    pub signal: StreamBackfillSignal,
    /// Whether the classification changed anything on disk.
    pub rewritten: bool,
}

/// What a completed stream backfill classified, wrote, and rebuilt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamBackfillReport {
    /// The host used wherever no segment-local evidence existed.
    pub fallback_host: String,
    /// How many `(day, segment)` pairs the import reverse index covered.
    pub import_index_segments: usize,
    /// How many segments were classified.
    pub classified: usize,
    /// Signal counts, most frequent first.
    pub signal_counts: Vec<(StreamBackfillSignal, usize)>,
    /// Stream counts, most frequent first.
    pub stream_counts: Vec<(String, usize)>,
    /// Markers written for a segment that had no marker or a different stream.
    pub written: usize,
    /// Markers rewritten only to repair `seq`/`prev` linkage.
    pub linkage_fixed: usize,
    /// Markers already carrying the classified stream, seq, and linkage.
    pub already_correct: usize,
    /// Stream state records rebuilt in `streams/`.
    pub rebuilt_streams: usize,
    /// Per-segment classifications, recorded only when verbose.
    pub classifications: Vec<StreamClassification>,
}

impl StreamBackfillReport {
    /// True when every marker was already correct, so nothing was rewritten and
    /// no stream state record was rebuilt.
    pub fn nothing_to_do(&self) -> bool {
        self.written == 0 && self.linkage_fixed == 0
    }
}

/// A stream backfill that could not be completed.
#[derive(Debug)]
pub enum StreamRepairError {
    /// A journal read or a stream-record write failed.
    Segment(SegmentError),
    /// A marker could not be published.
    MarkerWrite {
        path: PathBuf,
        source: AtomicWriteError,
    },
    /// A stream record's lock could not be acquired.
    Lock { path: PathBuf, source: LockError },
    /// A classified stream name would not stay inside one path component, so
    /// neither its marker nor its state record can be written.
    UnsafeStreamName { segment: PathBuf, name: String },
}

impl fmt::Display for StreamRepairError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Segment(error) => error.fmt(formatter),
            Self::MarkerWrite { path, source } => {
                write!(
                    formatter,
                    "stream marker write {}: {source}",
                    path.display()
                )
            }
            Self::Lock { path, source } => {
                write!(formatter, "stream record lock {}: {source}", path.display())
            }
            Self::UnsafeStreamName { segment, name } => write!(
                formatter,
                "unsafe stream name {name:?} classified for {}",
                segment.display()
            ),
        }
    }
}

impl Error for StreamRepairError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Segment(error) => Some(error),
            Self::MarkerWrite { source, .. } => Some(source),
            Self::Lock { source, .. } => Some(source),
            Self::UnsafeStreamName { .. } => None,
        }
    }
}

impl From<SegmentError> for StreamRepairError {
    fn from(error: SegmentError) -> Self {
        Self::Segment(error)
    }
}

/// Give every non-empty legacy segment a stream marker, then rebuild the
/// registry records those markers imply.
///
/// A segment's stream is decided by the first signal that yields a usable name:
/// an existing marker, an audio header's `stream`, `remote`, or `imported`
/// field, an `imported_audio.jsonl` sidecar, the `imports/*/segments.json`
/// reverse index, the `audio.jsonl` header's `host`, a tmux-only capture, and
/// finally the fallback host. Host precedence deliberately sits *after* every
/// import signal: an imported segment carrying a capture host must still be
/// filed as an import.
///
/// Segments are then grouped by stream, ordered by `(day, segment)`, and given
/// one-based sequence numbers with preceding-segment links. A marker that
/// already agrees on stream, sequence, and linkage is left untouched; one that
/// agrees only on the stream has its linkage repaired. When nothing needs
/// rewriting, no registry record is rebuilt either.
///
/// `host` overrides the fallback host. Without it the fallback is the first
/// observer record in `streams/`, and failing that the machine hostname.
pub fn backfill_stream_records(
    journal: &Path,
    host: Option<&str>,
    verbose: bool,
) -> Result<StreamBackfillReport, StreamRepairError> {
    let fallback_host = infer_fallback_host(journal, host)?;
    let import_index = build_import_reverse_index(journal);

    let mut entries: Vec<BackfillEntry> = Vec::new();
    let mut signal_counts: BTreeMap<StreamBackfillSignal, usize> = BTreeMap::new();
    let mut stream_counts: BTreeMap<String, usize> = BTreeMap::new();
    for (day, day_dir) in list_days(journal)? {
        for (name, path) in sorted_children(&day_dir) {
            if !path.is_dir() || segment_key(&name).is_none() || !has_content(&path) {
                continue;
            }
            let (stream, signal) =
                classify_segment(&path, &day, &name, &import_index, &fallback_host);
            if !is_writable_stream_name(&stream) {
                return Err(StreamRepairError::UnsafeStreamName {
                    segment: path,
                    name: stream,
                });
            }
            *signal_counts.entry(signal).or_default() += 1;
            *stream_counts.entry(stream.clone()).or_default() += 1;
            entries.push(BackfillEntry {
                day: day.clone(),
                segment: name,
                path,
                stream,
                signal,
                seq: 0,
                prev_day: None,
                prev_segment: None,
                action: BackfillAction::Write,
            });
        }
    }

    let mut grouped: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        grouped.entry(entry.stream.clone()).or_default().push(index);
    }
    for indices in grouped.values_mut() {
        indices.sort_by(|left, right| {
            let left = &entries[*left];
            let right = &entries[*right];
            (&left.day, &left.segment).cmp(&(&right.day, &right.segment))
        });
    }

    for indices in grouped.values() {
        for (position, index) in indices.iter().enumerate() {
            let (prev_day, prev_segment) = match position.checked_sub(1) {
                None => (None, None),
                Some(previous) => {
                    let previous = &entries[indices[previous]];
                    (Some(previous.day.clone()), Some(previous.segment.clone()))
                }
            };
            let entry = &mut entries[*index];
            entry.seq = position as u64 + 1;
            entry.prev_day = prev_day;
            entry.prev_segment = prev_segment;
            entry.action = decide_action(entry);
        }
    }

    let mut report = StreamBackfillReport {
        fallback_host,
        import_index_segments: import_index.len(),
        classified: entries.len(),
        signal_counts: rank_counts(signal_counts),
        stream_counts: rank_counts(stream_counts),
        written: count_action(&entries, BackfillAction::Write),
        linkage_fixed: count_action(&entries, BackfillAction::FixLinkage),
        already_correct: count_action(&entries, BackfillAction::Skip),
        rebuilt_streams: 0,
        classifications: Vec::new(),
    };
    if verbose {
        report.classifications = entries
            .iter()
            .map(|entry| StreamClassification {
                day: entry.day.clone(),
                segment: entry.segment.clone(),
                stream: entry.stream.clone(),
                signal: entry.signal,
                rewritten: entry.action != BackfillAction::Skip,
            })
            .collect();
    }
    if report.nothing_to_do() {
        return Ok(report);
    }

    for entry in &entries {
        if entry.action == BackfillAction::Skip {
            continue;
        }
        let path = entry.path.join("stream.json");
        write_json(
            &path,
            &BackfillMarker {
                stream: &entry.stream,
                prev_day: entry.prev_day.as_deref(),
                prev_segment: entry.prev_segment.as_deref(),
                seq: entry.seq,
            },
            JsonWriteOptions::default(),
        )
        .map_err(|source| StreamRepairError::MarkerWrite { path, source })?;
    }

    for (stream, indices) in &grouped {
        let last = &entries[*indices.last().expect("grouped streams are non-empty")];
        rebuild_stream_state(journal, stream, last, &report.fallback_host)?;
        report.rebuilt_streams += 1;
    }
    Ok(report)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackfillAction {
    Write,
    FixLinkage,
    Skip,
}

struct BackfillEntry {
    day: String,
    segment: String,
    path: PathBuf,
    stream: String,
    signal: StreamBackfillSignal,
    seq: u64,
    prev_day: Option<String>,
    prev_segment: Option<String>,
    action: BackfillAction,
}

#[derive(Serialize)]
struct BackfillMarker<'a> {
    stream: &'a str,
    prev_day: Option<&'a str>,
    prev_segment: Option<&'a str>,
    seq: u64,
}

/// The rebuilt registry record. Every field is deliberately a raw value: a
/// rebuild preserves whatever an existing record carried in these slots rather
/// than normalizing a shape it did not author.
#[derive(Serialize)]
struct RebuiltStreamState<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    kind: Value,
    host: Value,
    platform: Value,
    created_at: Value,
    last_day: &'a str,
    last_segment: &'a str,
    seq: u64,
    #[serde(rename = "cid")]
    did: Value,
    source: Value,
}

fn decide_action(entry: &BackfillEntry) -> BackfillAction {
    let Some(existing) = read_json_object(&entry.path.join("stream.json")) else {
        return BackfillAction::Write;
    };
    let same_stream =
        matches!(existing.get("stream"), Some(Value::String(stream)) if *stream == entry.stream);
    if !same_stream {
        return BackfillAction::Write;
    }
    let same_seq = existing
        .get("seq")
        .and_then(Value::as_f64)
        .is_some_and(|seq| seq == entry.seq as f64);
    let same_prev = matches_optional_str(existing.get("prev_day"), entry.prev_day.as_deref())
        && matches_optional_str(existing.get("prev_segment"), entry.prev_segment.as_deref());
    if same_seq && same_prev {
        BackfillAction::Skip
    } else {
        BackfillAction::FixLinkage
    }
}

fn existing_fingerprint(existing: &Map<String, Value>) -> Value {
    existing
        .get("cid")
        .or_else(|| existing.get("did"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn rebuild_stream_state(
    journal: &Path,
    stream: &str,
    last: &BackfillEntry,
    fallback_host: &str,
) -> Result<(), StreamRepairError> {
    let path = stream_record_path(journal, stream);
    let _lock =
        hold_lock(&path, LockOptions::default()).map_err(|source| StreamRepairError::Lock {
            path: path.clone(),
            source,
        })?;
    let existing = read_json_object(&path).unwrap_or_default();
    let kind = if stream.starts_with("import.") {
        json!("import")
    } else if stream.contains('.') && stream.ends_with(".tmux") {
        json!("observer")
    } else {
        existing
            .get("type")
            .cloned()
            .unwrap_or_else(|| json!("observer"))
    };
    let host = match existing.get("host") {
        Some(host) => host.clone(),
        None if kind == json!("observer") => json!(fallback_host),
        None => Value::Null,
    };
    write_stream_record(
        &path,
        &RebuiltStreamState {
            name: stream,
            kind,
            host,
            platform: existing.get("platform").cloned().unwrap_or(Value::Null),
            created_at: existing
                .get("created_at")
                .cloned()
                .unwrap_or_else(|| json!(0)),
            last_day: &last.day,
            last_segment: &last.segment,
            seq: last.seq,
            did: existing_fingerprint(&existing),
            source: existing.get("source").cloned().unwrap_or(Value::Null),
        },
    )?;
    Ok(())
}

/// Decide one segment's stream. Every branch mirrors one numbered signal.
fn classify_segment(
    segment_dir: &Path,
    day: &str,
    segment: &str,
    import_index: &BTreeMap<(String, String), &'static str>,
    fallback_host: &str,
) -> (String, StreamBackfillSignal) {
    if let Some(marker) = read_json_object(&segment_dir.join("stream.json"))
        && let Some(stream) = truthy_str(marker.get("stream"))
    {
        return (stream.to_owned(), StreamBackfillSignal::ExistingMarker);
    }

    let audio_path = segment_dir.join("audio.jsonl");
    if let Some(header) = read_jsonl_header(&audio_path)
        && let Some(classified) = classify_from_audio_header(&header)
    {
        return classified;
    }
    for (name, path) in sorted_children(segment_dir) {
        if name.ends_with("_audio.jsonl")
            && name != "imported_audio.jsonl"
            && let Some(header) = read_jsonl_header(&path)
            && let Some(classified) = classify_from_audio_header(&header)
        {
            return classified;
        }
    }

    let imported_path = segment_dir.join("imported_audio.jsonl");
    if imported_path.exists() {
        let header = read_jsonl_header(&imported_path);
        let source = header
            .as_ref()
            .and_then(|header| truthy_str(header.get("raw")))
            .map_or("audio", import_source_from_raw);
        if let Some(name) = import_stream_name(source) {
            return (name, StreamBackfillSignal::ImportedJsonl);
        }
    }

    let key = segment_key(segment).unwrap_or_else(|| segment.to_owned());
    if let Some(source) = import_index.get(&(day.to_owned(), key))
        && let Some(name) = import_stream_name(source)
    {
        return (name, StreamBackfillSignal::ImportIndex);
    }

    if let Some(header) = read_jsonl_header(&audio_path)
        && let Some(host) = truthy_str(header.get("host"))
        && let Some(name) = host_stream_name(host, None)
    {
        return (name, StreamBackfillSignal::AudioHost);
    }

    if has_tmux_only(segment_dir)
        && let Some(name) = host_stream_name(fallback_host, Some("tmux"))
    {
        return (name, StreamBackfillSignal::TmuxOnly);
    }

    let name =
        host_stream_name(fallback_host, None).unwrap_or_else(|| fallback_host.to_lowercase());
    (name, StreamBackfillSignal::HostnameFallback)
}

/// Signals 2 through 4, shared by `audio.jsonl` and its `*_audio.jsonl` peers.
fn classify_from_audio_header(
    header: &Map<String, Value>,
) -> Option<(String, StreamBackfillSignal)> {
    if let Some(stream) = truthy_str(header.get("stream")) {
        return Some((stream.to_owned(), StreamBackfillSignal::AudioStream));
    }
    // A `remote` header names the remote capture device, so it normalizes the
    // same way an observer name does. The Python migration spelled this
    // `stream_name(remote=...)`, a keyword that function never accepted; the
    // intended derivation is reproduced here rather than its TypeError.
    if let Some(remote) = truthy_str(header.get("remote"))
        && let Some(name) = host_stream_name(remote, None)
    {
        return Some((name, StreamBackfillSignal::AudioRemote));
    }
    if is_truthy(header.get("imported")) {
        let source = truthy_str(header.get("raw")).map_or("audio", import_source_from_raw);
        if let Some(name) = import_stream_name(source) {
            return Some((name, StreamBackfillSignal::AudioImported));
        }
    }
    None
}

/// Map `(day, segment)` to an import source using `imports/*/segments.json`.
fn build_import_reverse_index(journal: &Path) -> BTreeMap<(String, String), &'static str> {
    let mut index = BTreeMap::new();
    for (_, directory) in sorted_children(&journal.join("imports")) {
        if !directory.is_dir() {
            continue;
        }
        let Some(data) = read_json_object(&directory.join("segments.json")) else {
            continue;
        };
        let day = data.get("day").and_then(Value::as_str).unwrap_or_default();
        let segments = data.get("segments").and_then(Value::as_array);
        let Some(segments) = segments.filter(|segments| !segments.is_empty()) else {
            continue;
        };
        if day.is_empty() {
            continue;
        }
        let mut source = "audio";
        if let Some(meta) = read_json_object(&directory.join("import.json")) {
            if let Some(filename) = truthy_str(meta.get("original_filename")) {
                source = import_source_from_raw(filename);
            } else if let Some(mime) = truthy_str(meta.get("mime_type")) {
                source = import_source_from_mime(mime);
            }
        }
        for segment in segments {
            if let Some(segment) = segment.as_str() {
                index.insert((day.to_owned(), segment.to_owned()), source);
            }
        }
    }
    index
}

/// The fallback host: an explicit override, else the first observer record in
/// `streams/`, else the machine hostname.
fn infer_fallback_host(
    journal: &Path,
    override_host: Option<&str>,
) -> Result<String, StreamRepairError> {
    if let Some(host) = override_host.filter(|host| !host.is_empty()) {
        return Ok(strip_hostname(host));
    }
    for path in registry_json_paths(journal)? {
        let Some(state) = read_json_object(&path) else {
            continue;
        };
        if state.get("type").and_then(Value::as_str) == Some("observer")
            && let Some(host) = truthy_str(state.get("host"))
        {
            return Ok(strip_hostname(host));
        }
    }
    Ok(strip_hostname(&system_hostname()))
}

fn system_hostname() -> String {
    if let Ok(value) = std::env::var("HOSTNAME")
        && !value.is_empty()
    {
        return value;
    }
    fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Reduce a hostname to one dot-free label, because dots separate stream
/// qualifiers. Dotted-quad addresses keep every octet, joined by dashes.
fn strip_hostname(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = name.split('.').filter(|part| !part.is_empty()).collect();
    if parts
        .iter()
        .all(|part| part.chars().all(|character| character.is_ascii_digit()))
    {
        return parts.join("-");
    }
    name.split('.').next().unwrap_or_default().to_owned()
}

fn host_stream_name(host: &str, qualifier: Option<&str>) -> Option<String> {
    canonical_stream_name(&strip_hostname(host), qualifier)
}

fn import_stream_name(source: &str) -> Option<String> {
    canonical_stream_name(&format!("import.{source}"), None)
}

/// Lowercase, collapse separators, append any qualifier, then validate.
fn canonical_stream_name(base: &str, qualifier: Option<&str>) -> Option<String> {
    let mut name = collapse_separators(base);
    if let Some(qualifier) = qualifier {
        name = format!("{name}.{}", collapse_separators(qualifier));
    }
    if name.is_empty() || name.contains("..") {
        return None;
    }
    let mut characters = name.chars();
    let first = characters.next()?;
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return None;
    }
    characters
        .all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
        .then_some(name)
}

fn collapse_separators(value: &str) -> String {
    let lowered = value.to_lowercase();
    let mut output = String::new();
    let mut pending = false;
    for character in lowered.trim().chars() {
        if character.is_whitespace() || matches!(character, '/' | '\\') {
            pending = true;
            continue;
        }
        if pending {
            output.push('-');
            pending = false;
        }
        output.push(character);
    }
    output
}

fn import_source_from_raw(raw: &str) -> &'static str {
    match extension(raw).to_lowercase().as_str() {
        ".m4a" => "apple",
        ".txt" | ".md" | ".pdf" => "text",
        _ => "audio",
    }
}

fn import_source_from_mime(mime: &str) -> &'static str {
    if mime.contains("m4a") || mime.contains("mp4") {
        "apple"
    } else if mime.starts_with("text/") {
        "text"
    } else {
        "audio"
    }
}

/// The trailing extension of a path's final component, dot included. A leading
/// dot names a hidden file rather than an extension.
fn extension(path: &str) -> &str {
    let basename = path.rsplit('/').next().unwrap_or_default();
    let leading = basename.len() - basename.trim_start_matches('.').len();
    let remainder = &basename[leading..];
    match remainder.rfind('.') {
        Some(index) => &remainder[index..],
        None => "",
    }
}

/// A segment has content when it holds a file that is not a journal sidecar.
fn has_content(segment_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(segment_dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.path().is_file() && !is_reserved_name(&entry.file_name().to_string_lossy())
    })
}

/// A tmux screen capture with no recognized audio beside it.
fn has_tmux_only(segment_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(segment_dir) else {
        return false;
    };
    let mut tmux = false;
    let mut audio = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("tmux_") && name.ends_with("_screen.jsonl") {
            tmux = true;
        }
        if name.ends_with(".flac")
            || name.ends_with(".m4a")
            || name.ends_with(".ogg")
            || name.ends_with(".opus")
            || name == "audio.jsonl"
            || name.ends_with("_audio.jsonl")
        {
            audio = true;
        }
    }
    tmux && !audio
}

/// Directory children as `(name, path)` pairs, ordered by name. Unreadable
/// directories enumerate as empty, exactly as the classifier's other reads
/// tolerate absence.
fn sorted_children(directory: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    children.sort();
    children
}

/// The first line of a JSONL file, when it is a JSON object.
fn read_jsonl_header(path: &Path) -> Option<Map<String, Value>> {
    let file = fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    match serde_json::from_str(line.trim()) {
        Ok(Value::Object(header)) => Some(header),
        Ok(_) | Err(_) => None,
    }
}

/// A whole JSON file, when it is an object. Missing and malformed files read
/// alike: a one-time backfill classifies what it can rather than refusing a
/// journal because one legacy sidecar is unreadable.
fn read_json_object(path: &Path) -> Option<Map<String, Value>> {
    match serde_json::from_slice(&fs::read(path).ok()?) {
        Ok(Value::Object(object)) => Some(object),
        Ok(_) | Err(_) => None,
    }
}

/// Python's truthiness, so header fields decide the same way they did there.
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64() != Some(0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(value)) => !value.is_empty(),
        Some(Value::Object(value)) => !value.is_empty(),
    }
}

fn truthy_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn matches_optional_str(value: Option<&Value>, expected: Option<&str>) -> bool {
    match (value, expected) {
        (None | Some(Value::Null), None) => true,
        (Some(Value::String(value)), Some(expected)) => value == expected,
        _ => false,
    }
}

/// A classified name must stay one path component: it names both the segment's
/// marker content and a file directly under `streams/`.
fn is_writable_stream_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && !name.starts_with('.')
        && !name.contains('\0')
}

/// The `HHMMSS_LEN` key inside a name, matching Python's word-boundary search.
fn segment_key(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 8 <= bytes.len() {
        if (index > 0 && is_word_byte(bytes[index - 1]))
            || !bytes[index..index + 6].iter().all(u8::is_ascii_digit)
            || bytes[index + 6] != b'_'
        {
            index += 1;
            continue;
        }
        let mut end = index + 7;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > index + 7
            && (end == bytes.len() || !is_word_byte(bytes[end]) || bytes[end] == b'_')
        {
            return Some(value[index..end].to_owned());
        }
        index += 1;
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

fn count_action(entries: &[BackfillEntry], action: BackfillAction) -> usize {
    entries
        .iter()
        .filter(|entry| entry.action == action)
        .count()
}

/// Counts ordered most frequent first, ties broken by key for determinism.
fn rank_counts<K: Ord>(counts: BTreeMap<K, usize>) -> Vec<(K, usize)> {
    let mut ranked: Vec<(K, usize)> = counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use solstone_core_journal_io::{LockOptions, hold_lock};

    use crate::test_support::TempDir;

    use super::*;

    fn record_path(root: &Path, name: &str) -> PathBuf {
        root.join("streams").join(format!("{name}.json"))
    }

    #[test]
    fn tolerant_list_keeps_valid_records_and_reports_bad_paths() {
        let temporary = TempDir::new();
        let valid = record_path(temporary.path(), "valid");
        fs::create_dir_all(valid.parent().unwrap()).unwrap();
        fs::write(&valid, br#"{"name":"valid","type":"observer"}"#).unwrap();
        let broken = record_path(temporary.path(), "broken");
        fs::write(&broken, b"{not json").unwrap();

        let listed = list_stream_records_tolerant(temporary.path()).unwrap();
        assert_eq!(listed.records.len(), 1);
        assert_eq!(listed.records[0].0, "valid");
        assert_eq!(listed.records[0].1["type"], "observer");
        assert_eq!(
            listed.anomalies,
            vec![(broken, "malformed record".to_owned())]
        );
    }

    #[test]
    fn marker_repair_preserves_legacy_and_unknown_fields() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":"20260101","last_segment":"090000_300","seq":1,"legacy":"kept"}"#,
        )
        .unwrap();

        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260102",
                    last_segment: "100000_300",
                    max_seq: 2,
                },
                LockOptions::default(),
            ),
            RepairOutcome::Repaired
        );
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["type"], "observer");
        assert_eq!(value["legacy"], "kept");
        assert_eq!(value["last_day"], "20260102");
        assert_eq!(value["seq"], 2);
    }

    #[test]
    fn marker_repair_missing_record_creates_no_directory_or_lock() {
        let temporary = TempDir::new();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "missing",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 1,
                },
                LockOptions::default(),
            ),
            RepairOutcome::NoRecord
        );
        assert!(!temporary.path().join("streams").exists());
    }

    #[test]
    fn unsafe_stream_names_never_escape_the_registry_directory() {
        let temporary = TempDir::new();
        let escaped_name = format!(
            "../../{}-escaped",
            temporary.path().file_name().unwrap().to_str().unwrap()
        );
        let escaped = temporary.path().parent().unwrap().join(format!(
            "{}-escaped.json",
            temporary.path().file_name().unwrap().to_str().unwrap()
        ));
        let outside = TempDir::new();
        let absolute_name = outside.path().join("absolute-record");
        let absolute_record = outside.path().join("absolute-record.json");
        let marker_tail = MarkerTail {
            last_day: "20260101",
            last_segment: "090000_300",
            max_seq: 1,
        };

        for name in [escaped_name.as_str(), absolute_name.to_str().unwrap()] {
            assert_eq!(read_stream_record(temporary.path(), name).unwrap(), None);
            assert_eq!(
                repair_stream_tail_from_markers(
                    temporary.path(),
                    name,
                    &marker_tail,
                    LockOptions::default(),
                ),
                RepairOutcome::NoRecord
            );
        }

        assert!(!temporary.path().join("streams").exists());
        assert!(!escaped.exists());
        assert!(!absolute_record.exists());
    }

    #[test]
    fn marker_repair_does_not_touch_an_already_current_record() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":"20260101","last_segment":"090000_300","seq":2}"#;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 2,
                },
                LockOptions::default(),
            ),
            RepairOutcome::Unchanged(UnchangedReason::AlreadyCurrent)
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn marker_repair_reports_a_held_lock() {
        let temporary = TempDir::new();
        let path = record_path(temporary.path(), "workstation");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"name":"workstation","type":"observer","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":0}"#).unwrap();
        let _held = hold_lock(&path, LockOptions::default()).unwrap();
        assert_eq!(
            repair_stream_tail_from_markers(
                temporary.path(),
                "workstation",
                &MarkerTail {
                    last_day: "20260101",
                    last_segment: "090000_300",
                    max_seq: 1,
                },
                LockOptions {
                    timeout: Duration::from_millis(10),
                    ..LockOptions::default()
                },
            ),
            RepairOutcome::Locked
        );
    }

    #[test]
    fn health_marker_creates_its_parent_and_surfaces_failure() {
        let temporary = TempDir::new();
        touch_stream_health_marker(temporary.path(), "20260101").unwrap();
        assert!(matches!(
            solstone_core_journal_io::read_health_marker(
                temporary.path(),
                "20260101",
                solstone_core_journal_io::HealthMarkerKind::Stream,
            )
            .unwrap(),
            solstone_core_journal_io::HealthMarkerState::Versioned {
                marker: solstone_core_journal_io::HealthMarker { generation: 1, .. },
                ..
            }
        ));

        let blocked = TempDir::new();
        let health = blocked.path().join("chronicle/20260102/health");
        fs::create_dir_all(health.parent().unwrap()).unwrap();
        fs::write(&health, b"not a directory").unwrap();
        assert!(touch_stream_health_marker(blocked.path(), "20260102").is_err());
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod backfill_tests {
    use std::collections::BTreeMap;
    use std::fs;

    use serde_json::json;

    use crate::test_support::TempDir;

    use super::*;

    const DAY: &str = "20260401";

    /// One precedence case: the fixture files a segment holds, and the stream
    /// and signal they must produce.
    struct Case {
        segment: &'static str,
        files: Vec<(&'static str, &'static str)>,
        stream: &'static str,
        signal: StreamBackfillSignal,
    }

    fn segment(root: &Path, day: &str, key: &str, files: &[(&str, &str)]) -> PathBuf {
        let path = root.join("chronicle").join(day).join(key);
        fs::create_dir_all(&path).expect("segment directory");
        for (name, contents) in files {
            fs::write(path.join(name), contents).expect("segment file");
        }
        path
    }

    fn import(root: &Path, timestamp: &str, day: &str, segments: &[&str], meta: Value) {
        let directory = root.join("imports").join(timestamp);
        fs::create_dir_all(&directory).expect("import directory");
        fs::write(
            directory.join("segments.json"),
            json!({"segments": segments, "day": day}).to_string(),
        )
        .expect("segments.json");
        fs::write(directory.join("import.json"), meta.to_string()).expect("import.json");
    }

    fn stream_state(root: &Path, name: &str, state: Value) {
        let directory = root.join("streams");
        fs::create_dir_all(&directory).expect("streams directory");
        fs::write(directory.join(format!("{name}.json")), state.to_string()).expect("state");
    }

    fn marker(segment_dir: &Path) -> Value {
        serde_json::from_slice(&fs::read(segment_dir.join("stream.json")).expect("marker read"))
            .expect("marker parses")
    }

    fn record(root: &Path, name: &str) -> Value {
        serde_json::from_slice(
            &fs::read(root.join("streams").join(format!("{name}.json"))).expect("record read"),
        )
        .expect("record parses")
    }

    fn decided(report: &StreamBackfillReport) -> BTreeMap<String, (String, StreamBackfillSignal)> {
        report
            .classifications
            .iter()
            .map(|entry| (entry.segment.clone(), (entry.stream.clone(), entry.signal)))
            .collect()
    }

    #[test]
    fn every_signal_decides_its_segment_in_precedence_order() {
        let temporary = TempDir::new();
        let root = temporary.path();
        // Each case is the shallowest fixture that reaches exactly one signal:
        // every earlier signal is absent, so the decision names the rule.
        let cases: [Case; 10] = [
            Case {
                segment: "010000_60",
                files: vec![
                    ("stream.json", r#"{"stream": "legacy", "seq": 1}"#),
                    ("notes.txt", "kept"),
                ],
                stream: "legacy",
                signal: StreamBackfillSignal::ExistingMarker,
            },
            Case {
                segment: "020000_60",
                files: vec![(
                    "audio.jsonl",
                    "{\"stream\": \"headered\"}\n{\"text\": \"x\"}\n",
                )],
                stream: "headered",
                signal: StreamBackfillSignal::AudioStream,
            },
            Case {
                segment: "030000_60",
                files: vec![("mic_audio.jsonl", "{\"stream\": \"sidecar\"}\n")],
                stream: "sidecar",
                signal: StreamBackfillSignal::AudioStream,
            },
            Case {
                segment: "040000_60",
                files: vec![("audio.jsonl", "{\"remote\": \"Laptop.local\"}\n")],
                stream: "laptop",
                signal: StreamBackfillSignal::AudioRemote,
            },
            Case {
                segment: "050000_60",
                files: vec![(
                    "audio.jsonl",
                    "{\"imported\": true, \"raw\": \"memo.M4A\"}\n",
                )],
                stream: "import.apple",
                signal: StreamBackfillSignal::AudioImported,
            },
            Case {
                segment: "060000_60",
                files: vec![("imported_audio.jsonl", "{\"raw\": \"notes.pdf\"}\n")],
                stream: "import.text",
                signal: StreamBackfillSignal::ImportedJsonl,
            },
            Case {
                segment: "070000_60",
                files: vec![("screen.jsonl", "{}\n")],
                stream: "import.text",
                signal: StreamBackfillSignal::ImportIndex,
            },
            Case {
                segment: "080000_60",
                files: vec![("audio.jsonl", "{\"host\": \"Workstation.local\"}\n")],
                stream: "workstation",
                signal: StreamBackfillSignal::AudioHost,
            },
            Case {
                segment: "090000_60",
                files: vec![("tmux_0_screen.jsonl", "{}\n")],
                stream: "desk.tmux",
                signal: StreamBackfillSignal::TmuxOnly,
            },
            Case {
                segment: "100000_60",
                files: vec![("screen.jsonl", "{}\n")],
                stream: "desk",
                signal: StreamBackfillSignal::HostnameFallback,
            },
        ];
        for case in &cases {
            segment(root, DAY, case.segment, &case.files);
        }
        import(
            root,
            "20260401_070000",
            DAY,
            &["070000_60"],
            json!({"mime_type": "text/plain"}),
        );

        let report = backfill_stream_records(root, Some("Desk.local"), true).expect("backfill");

        // The fallback host keeps its recorded case; only the derived stream
        // name is canonicalized.
        assert_eq!(report.fallback_host, "Desk");
        assert_eq!(report.classified, cases.len());
        let decided = decided(&report);
        for case in &cases {
            assert_eq!(
                decided.get(case.segment),
                Some(&(case.stream.to_owned(), case.signal)),
                "{}",
                case.segment
            );
        }
        assert_eq!(
            report.signal_counts.first(),
            Some(&(StreamBackfillSignal::AudioStream, 2))
        );
    }

    #[test]
    fn import_evidence_outranks_a_capture_host_on_the_same_segment() {
        let temporary = TempDir::new();
        let root = temporary.path();
        // Host is signal seven precisely so an imported segment that also
        // carries a capture host is still filed as an import.
        segment(
            root,
            DAY,
            "110000_60",
            &[("audio.jsonl", "{\"host\": \"workstation\"}\n")],
        );
        import(
            root,
            "20260401_110000",
            DAY,
            &["110000_60"],
            json!({"original_filename": "voice memo.m4a", "mime_type": "text/plain"}),
        );

        let report = backfill_stream_records(root, Some("desk"), true).expect("backfill");

        assert_eq!(
            decided(&report).get("110000_60"),
            Some(&("import.apple".to_owned(), StreamBackfillSignal::ImportIndex))
        );
    }

    #[test]
    fn an_unusable_header_signal_falls_through_to_the_next_rule() {
        let temporary = TempDir::new();
        let root = temporary.path();
        // A remote name that cannot canonicalize is not a decision, so the
        // header's own import evidence still gets its turn.
        segment(
            root,
            DAY,
            "120000_60",
            &[(
                "audio.jsonl",
                "{\"remote\": \"...\", \"imported\": 1, \"raw\": \"clip.wav\"}\n",
            )],
        );
        segment(
            root,
            DAY,
            "130000_60",
            &[("audio.jsonl", "not json at all\n")],
        );

        let report = backfill_stream_records(root, Some("desk"), true).expect("backfill");

        let decided = decided(&report);
        assert_eq!(
            decided.get("120000_60"),
            Some(&(
                "import.audio".to_owned(),
                StreamBackfillSignal::AudioImported
            ))
        );
        assert_eq!(
            decided.get("130000_60"),
            Some(&("desk".to_owned(), StreamBackfillSignal::HostnameFallback))
        );
    }

    #[test]
    fn segments_are_relinked_chronologically_within_each_stream() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let first = segment(root, "20260401", "090000_60", &[("screen.jsonl", "{}\n")]);
        let second = segment(root, "20260401", "100000_60", &[("screen.jsonl", "{}\n")]);
        let third = segment(root, "20260402", "080000_60", &[("screen.jsonl", "{}\n")]);
        let other = segment(
            root,
            "20260401",
            "093000_60",
            &[("imported_audio.jsonl", "{\"raw\": \"memo.m4a\"}\n")],
        );

        let report = backfill_stream_records(root, Some("desk"), false).expect("backfill");

        assert_eq!(report.written, 4);
        assert_eq!(report.linkage_fixed, 0);
        assert_eq!(report.rebuilt_streams, 2);
        assert_eq!(
            marker(&first),
            json!({"stream": "desk", "prev_day": null, "prev_segment": null, "seq": 1})
        );
        assert_eq!(
            marker(&second),
            json!({"stream": "desk", "prev_day": "20260401", "prev_segment": "090000_60",
                "seq": 2})
        );
        assert_eq!(
            marker(&third),
            json!({"stream": "desk", "prev_day": "20260401", "prev_segment": "100000_60",
                "seq": 3})
        );
        assert_eq!(
            marker(&other),
            json!({"stream": "import.apple", "prev_day": null, "prev_segment": null, "seq": 1})
        );
        assert_eq!(
            record(root, "desk"),
            json!({"name": "desk", "type": "observer", "host": "desk", "platform": null,
                "created_at": 0, "last_day": "20260402", "last_segment": "080000_60", "seq": 3,
                "cid": null, "source": null})
        );
        assert_eq!(
            record(root, "import.apple"),
            json!({"name": "import.apple", "type": "import", "host": null, "platform": null,
                "created_at": 0, "last_day": "20260401", "last_segment": "093000_60", "seq": 1,
                "cid": null, "source": null})
        );
    }

    #[test]
    fn correct_markers_are_skipped_and_only_stale_linkage_is_repaired() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let first = segment(
            root,
            DAY,
            "090000_60",
            &[
                ("screen.jsonl", "{}\n"),
                (
                    "stream.json",
                    r#"{"stream": "desk", "prev_day": null, "prev_segment": null, "seq": 1}"#,
                ),
            ],
        );
        let second = segment(
            root,
            DAY,
            "100000_60",
            &[
                ("screen.jsonl", "{}\n"),
                (
                    "stream.json",
                    r#"{"stream": "desk", "prev_day": null, "prev_segment": null, "seq": 9}"#,
                ),
            ],
        );
        let untouched = fs::read(first.join("stream.json")).expect("marker bytes");

        let report = backfill_stream_records(root, Some("desk"), false).expect("backfill");

        assert_eq!(report.already_correct, 1);
        assert_eq!(report.linkage_fixed, 1);
        assert_eq!(report.written, 0);
        assert_eq!(report.rebuilt_streams, 1);
        assert_eq!(fs::read(first.join("stream.json")).unwrap(), untouched);
        assert_eq!(
            marker(&second),
            json!({"stream": "desk", "prev_day": "20260401", "prev_segment": "090000_60",
                "seq": 2})
        );
    }

    #[test]
    fn an_entirely_correct_journal_rewrites_nothing_and_rebuilds_no_record() {
        let temporary = TempDir::new();
        let root = temporary.path();
        // A marker missing prev_day still reads as an absent link, so a legacy
        // shape that already agrees must not be rewritten.
        segment(
            root,
            DAY,
            "090000_60",
            &[
                ("screen.jsonl", "{}\n"),
                ("stream.json", r#"{"stream": "desk", "seq": 1}"#),
            ],
        );

        let report = backfill_stream_records(root, Some("desk"), true).expect("backfill");

        assert!(report.nothing_to_do());
        assert_eq!(report.already_correct, 1);
        assert_eq!(report.rebuilt_streams, 0);
        assert_eq!(report.classifications.len(), 1);
        assert!(!report.classifications[0].rewritten);
        assert!(!root.join("streams").exists());
    }

    #[test]
    fn a_rebuilt_record_keeps_the_identity_fields_it_did_not_author() {
        let temporary = TempDir::new();
        let root = temporary.path();
        segment(root, DAY, "090000_60", &[("tmux_0_screen.jsonl", "{}\n")]);
        segment(root, DAY, "100000_60", &[("screen.jsonl", "{}\n")]);
        stream_state(
            root,
            "desk",
            json!({"name": "desk", "type": "capture", "host": "elsewhere",
                "platform": "linux", "created_at": 7, "did": "did:plc:desk",
                "source": "iphone", "last_day": "19990101",
                "last_segment": "000000_1", "seq": 42}),
        );
        stream_state(
            root,
            "desk.tmux",
            json!({"name": "desk.tmux", "host": null}),
        );

        let report = backfill_stream_records(root, Some("desk"), false).expect("backfill");

        assert_eq!(report.rebuilt_streams, 2);
        assert_eq!(
            record(root, "desk"),
            json!({"name": "desk", "type": "capture", "host": "elsewhere",
                "platform": "linux", "created_at": 7, "last_day": "20260401",
                "last_segment": "100000_60", "seq": 1, "cid": "did:plc:desk",
                "source": "iphone"})
        );
        // A `.tmux` name is always an observer, and an explicit null host is a
        // recorded value rather than a missing one. Missing cid/source become
        // null, matching host/platform — they must not disappear.
        assert_eq!(
            record(root, "desk.tmux"),
            json!({"name": "desk.tmux", "type": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": "20260401", "last_segment": "090000_60", "seq": 1,
                "cid": null, "source": null})
        );
    }

    #[test]
    fn only_non_empty_segment_directories_are_classified() {
        let temporary = TempDir::new();
        let root = temporary.path();
        segment(root, DAY, "090000_60", &[("stream.json", "{}")]);
        segment(root, DAY, "100000_60", &[]);
        segment(root, DAY, "workstation", &[("screen.jsonl", "{}\n")]);
        segment(root, DAY, "110000_60", &[("screen.jsonl", "{}\n")]);
        fs::write(root.join("chronicle").join(DAY).join("timeline.json"), "{}").expect("day file");
        fs::create_dir_all(root.join("chronicle").join("notaday")).expect("stray directory");

        let report = backfill_stream_records(root, Some("desk"), true).expect("backfill");

        assert_eq!(report.classified, 1);
        assert_eq!(report.classifications[0].segment, "110000_60");
        assert_eq!(report.stream_counts, vec![("desk".to_owned(), 1)]);
    }

    #[test]
    fn the_fallback_host_prefers_an_override_then_an_observer_record() {
        let temporary = TempDir::new();
        let root = temporary.path();
        segment(root, DAY, "090000_60", &[("screen.jsonl", "{}\n")]);
        stream_state(
            root,
            "aaa-import",
            json!({"type": "import", "host": "ignored"}),
        );
        stream_state(
            root,
            "bbb-desk",
            json!({"type": "observer", "host": "Recorded.local"}),
        );

        assert_eq!(
            backfill_stream_records(root, Some("Override.local"), false)
                .expect("backfill")
                .fallback_host,
            "Override"
        );
        assert_eq!(
            backfill_stream_records(root, None, false)
                .expect("backfill")
                .fallback_host,
            "Recorded"
        );
    }

    #[test]
    fn a_name_that_would_leave_the_registry_refuses_before_anything_is_written() {
        let temporary = TempDir::new();
        let root = temporary.path();
        let escaping = segment(
            root,
            DAY,
            "090000_60",
            &[
                ("screen.jsonl", "{}\n"),
                ("audio.jsonl", r#"{"stream": "../../escaped"}"#),
            ],
        );
        segment(root, DAY, "100000_60", &[("screen.jsonl", "{}\n")]);

        let error = backfill_stream_records(root, Some("desk"), false).expect_err("refusal");

        assert!(matches!(
            &error,
            StreamRepairError::UnsafeStreamName { segment, name }
                if segment == &escaping && name == "../../escaped"
        ));
        assert!(!root.join("streams").exists());
        assert!(
            !root
                .join("chronicle")
                .join(DAY)
                .join("100000_60")
                .join("stream.json")
                .exists()
        );
    }

    #[test]
    fn hostnames_reduce_to_one_dot_free_label() {
        assert_eq!(strip_hostname("ja1r.local"), "ja1r");
        assert_eq!(strip_hostname("192.168.1.1"), "192-168-1-1");
        assert_eq!(strip_hostname("  archon  "), "archon");
        assert_eq!(strip_hostname("my.host.example.com"), "my");
        assert_eq!(strip_hostname("..."), "");
        assert_eq!(strip_hostname(""), "");
    }

    #[test]
    fn canonical_names_lowercase_collapse_and_validate() {
        assert_eq!(
            host_stream_name("My Host", None).as_deref(),
            Some("my-host")
        );
        assert_eq!(
            host_stream_name("desk", Some("TMUX")).as_deref(),
            Some("desk.tmux")
        );
        assert_eq!(import_stream_name("apple").as_deref(), Some("import.apple"));
        for rejected in ["", "...", "-leading", "/", "Владимир"] {
            assert_eq!(host_stream_name(rejected, None), None, "{rejected}");
        }
    }

    #[test]
    fn import_sources_come_from_the_raw_extension_then_the_mime_type() {
        assert_eq!(import_source_from_raw("/tmp/Memo.M4A"), "apple");
        for text in ["a.txt", "b.MD", "c.pdf"] {
            assert_eq!(import_source_from_raw(text), "text", "{text}");
        }
        assert_eq!(import_source_from_raw("clip.wav"), "audio");
        assert_eq!(import_source_from_raw("noextension"), "audio");
        assert_eq!(import_source_from_raw(".hidden"), "audio");
        assert_eq!(import_source_from_mime("audio/mp4"), "apple");
        assert_eq!(import_source_from_mime("text/markdown"), "text");
        assert_eq!(import_source_from_mime("application/zip"), "audio");
    }

    #[test]
    fn segment_keys_match_python_word_boundary_semantics() {
        assert_eq!(segment_key("143022_300").as_deref(), Some("143022_300"));
        assert_eq!(
            segment_key("143022_300_summary.txt").as_deref(),
            Some("143022_300")
        );
        assert_eq!(
            segment_key("/journal/20250109/143022_300/audio.jsonl").as_deref(),
            Some("143022_300")
        );
        assert_eq!(segment_key("1234567_300"), None);
        assert_eq!(segment_key("143022_300abc"), None);
        assert_eq!(segment_key("invalid"), None);
        assert_eq!(segment_key("workstation"), None);
    }
}
