// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only, replay-safe observer listing assembly.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use solstone_core_callosum::{DeviceIngestEvent, read_device_ingest_events};
use solstone_core_ingest_resolve::SegmentTerminalProof;
use solstone_core_segment::{
    ContentName, SegmentDir, TerminalProofVerifier, is_safe_stream_component, list_segments,
};

use crate::observer_evidence::{
    HistoryEvidence, ObserverEvidenceError, ResolvedObserver, fallback_stream,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum FileStatus {
    Missing,
    Processed,
    Present,
}

impl FileStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Processed => "processed",
            Self::Present => "present",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ListingFile {
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) submitted_name: Option<String>,
    pub(crate) status: FileStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct ListingSegment {
    pub(crate) key: String,
    pub(crate) observed: bool,
    pub(crate) original_key: Option<String>,
    pub(crate) files: Vec<ListingFile>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DayListing {
    pub(crate) segments: Vec<ListingSegment>,
}

#[derive(Debug)]
pub(crate) enum ListingError {
    Malformed,
    AmbiguousName,
    JournalRead,
}

/// List every native durable event for the authenticated device in its bound
/// native stream. This deliberately retains all events for a segment: a heal
/// appends a second event and keep-first would hide its new file forever.
pub(crate) fn native_events(
    journal_root: &Path,
    day: &str,
    stream: Option<&str>,
    cid: &str,
) -> Result<Vec<DeviceIngestEvent>, ListingError> {
    let Some(stream) = stream else {
        return Ok(Vec::new());
    };
    let segments = list_segments(journal_root, day).map_err(|_| ListingError::JournalRead)?;
    let mut events = Vec::new();
    for segment in segments
        .into_iter()
        .filter(|segment| segment.stream().matches(stream))
    {
        let report =
            read_device_ingest_events(segment.path()).map_err(|_| ListingError::JournalRead)?;
        events.extend(report.records.into_iter().filter(|event| event.cid == cid));
    }
    Ok(events)
}

/// Combine Python sync history with every native durable event per segment.
pub(crate) fn merge_day_listing(
    journal_root: &Path,
    day: &str,
    observer: Option<&ResolvedObserver>,
    history: HistoryEvidence,
    events: Vec<DeviceIngestEvent>,
) -> Result<DayListing, ListingError> {
    let fallback = observer
        .map(fallback_stream)
        .transpose()
        .map_err(map_evidence_error)?;
    let mut by_segment: BTreeMap<String, SegmentAccumulator> = BTreeMap::new();
    for record in history.records {
        let object = record.as_object().ok_or(ListingError::Malformed)?;
        if object.get("type").and_then(Value::as_str) == Some("observed") {
            continue;
        }
        let segment = required_string(object.get("segment"))?;
        if history.pruned_segments.contains(segment) {
            continue;
        }
        let stream = match object.get("stream") {
            Some(value) => required_string(Some(value))?.to_owned(),
            None => fallback.clone().ok_or(ListingError::Malformed)?,
        };
        let original_key = object
            .get("segment_original")
            .map(|value| required_string(Some(value)).map(str::to_owned))
            .transpose()?;
        let files: &[Value] = match object.get("files") {
            Some(files) => files.as_array().ok_or(ListingError::Malformed)?,
            None => &[],
        };
        let accumulator = by_segment.entry(segment.to_owned()).or_default();
        if accumulator.original_key.is_none() {
            accumulator.original_key = original_key;
        }
        for file in files {
            let object = file.as_object().ok_or(ListingError::Malformed)?;
            if object.get("disposition").and_then(Value::as_str) == Some("received_not_written") {
                continue;
            }
            let entry = project_history_file(journal_root, day, &stream, segment, object)?;
            accumulator.insert(entry);
        }
    }
    for event in events {
        if event.day != day {
            continue;
        }
        let accumulator = by_segment.entry(event.segment.clone()).or_default();
        for file in event.files {
            let entry = project_event_file(journal_root, day, &event.stream, &event.segment, file)?;
            accumulator.insert(entry);
        }
        // DeviceIngestEvent carries no segment_original equivalent; do not infer one.
    }
    let mut segments = Vec::new();
    for (key, accumulator) in by_segment {
        // Native-era segments list observed:false because the reference computes
        // it from history observed rows and the native writer emits none. Apply
        // a historical marker after the two evidence stores have been unioned.
        let observed = history.observed_segments.contains(&key);
        let original_key = accumulator.original_key.clone();
        let files = accumulator.reduce_effective_names()?;
        if !files.is_empty() {
            segments.push(ListingSegment {
                key,
                observed,
                original_key,
                files,
            });
        }
    }
    Ok(DayListing { segments })
}

fn project_history_file(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<ListingFile, ListingError> {
    let written = required_string(object.get("written"))?;
    let submitted = required_string(object.get("submitted"))?;
    let size = required_u64(object.get("size"))?;
    let sha256 = required_string(object.get("sha256"))?.to_owned();
    Ok(ListingFile {
        name: written.to_owned(),
        size,
        sha256,
        submitted_name: (submitted != written).then(|| submitted.to_owned()),
        status: resolve_file_status(journal_root, day, stream, segment, written, size)?,
    })
}

fn project_event_file(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    file: solstone_core_callosum::FileDescriptor,
) -> Result<ListingFile, ListingError> {
    let status = resolve_file_status(journal_root, day, stream, segment, &file.written, file.size)?;
    Ok(ListingFile {
        submitted_name: (file.submitted != file.written).then_some(file.submitted),
        name: file.written,
        size: file.size,
        sha256: file.sha256,
        status,
    })
}

/// The reference-compatible three-arm status check. `present` is a stat, not a
/// read-and-hash. This loses the only server-side detection of bytes drifted
/// from their attestation: such a file formerly read missing and triggered an
/// automatic re-upload repair; now it reads present forever and nothing repairs
/// it. That loss is accepted because clients compare their own attestations,
/// and a per-request full-journal read is an outage, not a check; repair belongs
/// to a scrub. SegmentDir::resolve's DEFAULT_STREAM special case diverges from
/// Python's always-interposed stream, but is unreachable today because history
/// rows carry a real stream name.
fn resolve_file_status(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    written: &str,
    size: u64,
) -> Result<FileStatus, ListingError> {
    let name = ContentName::new(written).map_err(|_| ListingError::Malformed)?;
    if !is_safe_stream_component(segment) || !is_safe_stream_component(stream) {
        return Err(ListingError::Malformed);
    }
    let segment = SegmentDir::resolve(journal_root, day, segment, stream)
        .map_err(|_| ListingError::JournalRead)?;
    let path = segment.path().join(written);
    match fs::metadata(&path) {
        Ok(_) => return Ok(FileStatus::Present),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ListingError::JournalRead),
    }
    if SegmentTerminalProof::new(&segment).has_terminal_proof(&name, size) {
        Ok(FileStatus::Processed)
    } else {
        Ok(FileStatus::Missing)
    }
}

#[derive(Default)]
struct SegmentAccumulator {
    original_key: Option<String>,
    // Held-wins is a safer generalization of the Python reference's
    // last-write-wins behavior, not an equivalent behavior across streams.
    by_identity: BTreeMap<(String, String), ListingFile>,
}

impl SegmentAccumulator {
    fn insert(&mut self, entry: ListingFile) {
        let key = (entry.name.clone(), entry.sha256.clone());
        match self.by_identity.get(&key) {
            Some(existing) if existing.status >= entry.status => {
                // Same identity with distinct submitted names is keep-first,
                // matching the reference's insertion behavior.
            }
            _ => {
                self.by_identity.insert(key, entry);
            }
        }
    }

    fn reduce_effective_names(self) -> Result<Vec<ListingFile>, ListingError> {
        let mut groups: BTreeMap<String, Vec<ListingFile>> = BTreeMap::new();
        for entry in self.by_identity.into_values() {
            let effective = entry
                .submitted_name
                .as_deref()
                .unwrap_or(&entry.name)
                .to_owned();
            groups.entry(effective).or_default().push(entry);
        }
        let mut output = Vec::new();
        for entries in groups.into_values() {
            if entries.len() == 1 {
                output.extend(entries);
                continue;
            }
            let held = entries
                .into_iter()
                .filter(|entry| entry.status != FileStatus::Missing)
                .collect::<Vec<_>>();
            if held.len() == 1 {
                output.extend(held);
            } else {
                return Err(ListingError::AmbiguousName);
            }
        }
        Ok(output)
    }
}

fn required_string(value: Option<&Value>) -> Result<&str, ListingError> {
    value.and_then(Value::as_str).ok_or(ListingError::Malformed)
}

fn required_u64(value: Option<&Value>) -> Result<u64, ListingError> {
    value.and_then(Value::as_u64).ok_or(ListingError::Malformed)
}

fn map_evidence_error(error: ObserverEvidenceError) -> ListingError {
    match error {
        ObserverEvidenceError::Malformed => ListingError::Malformed,
        _ => ListingError::JournalRead,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{
        FileStatus, HistoryEvidence, ListingError, ListingFile, SegmentAccumulator,
        merge_day_listing, resolve_file_status,
    };

    fn entry(
        name: &str,
        sha256: &str,
        submitted_name: Option<&str>,
        status: FileStatus,
    ) -> ListingFile {
        ListingFile {
            name: name.to_owned(),
            size: 1,
            sha256: sha256.to_owned(),
            submitted_name: submitted_name.map(str::to_owned),
            status,
        }
    }

    #[test]
    fn same_identity_keeps_the_first_submitted_name() {
        let mut accumulator = SegmentAccumulator::default();
        accumulator.insert(entry(
            "written.flac",
            "a",
            Some("first.flac"),
            FileStatus::Missing,
        ));
        accumulator.insert(entry(
            "written.flac",
            "a",
            Some("second.flac"),
            FileStatus::Missing,
        ));
        let files = accumulator.reduce_effective_names().expect("one identity");
        assert_eq!(files[0].submitted_name.as_deref(), Some("first.flac"));
    }

    #[test]
    fn effective_name_reduction_requires_one_held_survivor() {
        let mut names = SegmentAccumulator::default();
        names.insert(entry("left.flac", "a", None, FileStatus::Missing));
        names.insert(entry("right.flac", "a", None, FileStatus::Missing));
        assert_eq!(
            names
                .reduce_effective_names()
                .expect("distinct names")
                .len(),
            2
        );

        let mut ambiguous = SegmentAccumulator::default();
        ambiguous.insert(entry("same.flac", "a", None, FileStatus::Missing));
        ambiguous.insert(entry("same.flac", "b", None, FileStatus::Missing));
        assert!(matches!(
            ambiguous.reduce_effective_names(),
            Err(ListingError::AmbiguousName)
        ));

        let mut held = SegmentAccumulator::default();
        held.insert(entry("same.flac", "a", None, FileStatus::Missing));
        held.insert(entry("same.flac", "b", None, FileStatus::Present));
        let files = held.reduce_effective_names().expect("one held twin");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].status, FileStatus::Present);
    }

    #[test]
    fn received_not_written_is_dropped_before_same_name_different_sha_reduction() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let segment = root.join("chronicle/20260804/laptop/120000_60");
        fs::create_dir_all(&segment).expect("segment directory");
        fs::write(segment.join("audio.flac"), b"held").expect("held media");
        let history = HistoryEvidence {
            records: vec![json!({
                "segment": "120000_60",
                "stream": "laptop",
                "files": [
                    {"submitted":"audio.flac", "written":"audio.flac", "size":4, "sha256":"held", "disposition":"written"},
                    {"submitted":"audio.flac", "written":"audio.flac", "size":4, "sha256":"audit", "disposition":"received_not_written"}
                ]
            })],
            ..HistoryEvidence::default()
        };
        let listing = merge_day_listing(root, "20260804", None, history, Vec::new())
            .expect("audit-only row must not make the name ambiguous");
        assert_eq!(listing.segments[0].files.len(), 1);
        assert_eq!(listing.segments[0].files[0].sha256, "held");
    }

    #[test]
    fn on_disk_image_is_present_with_or_without_a_depict_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let segment = root.join("chronicle/20260804/laptop/120000_60");
        fs::create_dir_all(&segment).expect("segment directory");
        fs::write(segment.join("photo.png"), b"image").expect("image");

        assert_eq!(
            resolve_file_status(root, "20260804", "laptop", "120000_60", "photo.png", 5)
                .expect("status without sidecar"),
            FileStatus::Present
        );

        fs::write(
            segment.join("photo.jsonl"),
            concat!(
                r#"{"_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","reason_code":"ok","handler":"depict","attempted_at":"2026-08-05T00:00:00Z","input_size":5}}"#,
                "\n",
                r#"{"text":"caption"}"#,
                "\n",
            ),
        )
        .expect("sidecar");

        assert_eq!(
            resolve_file_status(root, "20260804", "laptop", "120000_60", "photo.png", 5)
                .expect("status with depict record"),
            FileStatus::Present
        );
    }
}
