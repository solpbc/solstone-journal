// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only, replay-safe device-ingest listing assembly.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};
use solstone_core_callosum::{DeviceIngestEvent, read_device_ingest_events};
use solstone_core_ingest_resolve::SegmentTerminalProof;
use solstone_core_segment::{
    ContentName, SegmentDir, TerminalProofVerifier, is_safe_stream_component, list_stream_segments,
    receipt_cids_for_stream,
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
    /// Key used by both day-listing wire projections.
    pub(crate) key: String,
    /// Physical segment-directory basename.
    pub(crate) segment: String,
    /// Physical stream-directory name.
    pub(crate) stream: String,
    /// Whether this basename was repeated in the emitted response.
    pub(crate) collided: bool,
    pub(crate) observed: bool,
    pub(crate) original_key: Option<String>,
    pub(crate) files: Vec<ListingFile>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DayListing {
    pub(crate) segments: Vec<ListingSegment>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ListingError {
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
    source: &str,
) -> Result<Vec<DeviceIngestEvent>, ListingError> {
    let Some(stream) = stream else {
        return Ok(Vec::new());
    };
    let segments =
        list_stream_segments(journal_root, day, stream).map_err(|_| ListingError::JournalRead)?;
    let mut events = Vec::new();
    for segment in segments {
        let identity = segment
            .record_identity()
            .map_err(|_| ListingError::JournalRead)?;
        events.extend(segment_events(
            journal_root,
            segment.path(),
            day,
            stream,
            identity.name,
            cid,
            source,
        )?);
    }
    Ok(events)
}

/// Read one segment's custody receipts without accepting corrupt or foreign rows.
pub(crate) fn segment_events(
    journal_root: &Path,
    path: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    cid: &str,
    source: &str,
) -> Result<Vec<DeviceIngestEvent>, ListingError> {
    let receipt_cids = receipt_cids_for_stream(journal_root, stream, cid, source)
        .map_err(|_| ListingError::JournalRead)?;
    if receipt_cids.is_empty() {
        return Err(ListingError::JournalRead);
    }
    let report = read_device_ingest_events(path).map_err(|_| ListingError::JournalRead)?;
    if report.unparseable > 0
        || report.records.iter().any(|event| {
            !receipt_cids.contains(&event.cid)
                || event.source != source
                || event.stream != stream
                || event.day != day
                || event.segment != segment
        })
    {
        return Err(ListingError::JournalRead);
    }
    Ok(report.records)
}

/// Combine every native durable event per segment.
pub(crate) fn merge_day_listing(
    journal_root: &Path,
    day: &str,
    events: Vec<DeviceIngestEvent>,
) -> Result<DayListing, ListingError> {
    let mut by_segment: BTreeMap<(String, String), SegmentAccumulator> = BTreeMap::new();
    for event in events {
        let accumulator = by_segment
            .entry((event.segment.clone(), event.stream.clone()))
            .or_default();
        for file in event.files {
            let entry = project_event_file(journal_root, day, &event.stream, &event.segment, file)?;
            accumulator.insert(entry);
        }
        // DeviceIngestEvent carries no segment_original equivalent; do not infer one.
    }
    let mut physical_segments = Vec::new();
    for ((segment, stream), accumulator) in by_segment {
        let original_key = accumulator.original_key.clone();
        let files = accumulator.reduce_effective_names()?;
        if !files.is_empty() {
            physical_segments.push(ListingSegment {
                key: segment.clone(),
                segment,
                stream,
                collided: false,
                observed: false,
                original_key,
                files,
            });
        }
    }

    // `physical_segments` is already ordered by `(basename, stream)` because
    // the accumulator map uses that pair. Count only segments that survived
    // reduction and will actually be emitted.
    let mut basename_counts = BTreeMap::<String, usize>::new();
    let mut occupied = BTreeSet::new();
    for segment in &physical_segments {
        *basename_counts.entry(segment.segment.clone()).or_default() += 1;
        occupied.insert(segment.segment.clone());
    }

    let mut assigned = BTreeSet::new();
    for segment in &mut physical_segments {
        if basename_counts[&segment.segment] < 2 {
            continue;
        }
        segment.collided = true;
        let preferred = format!("{}~{}", segment.segment, segment.stream);
        let mut candidate = preferred.clone();
        let mut suffix = 2_u64;
        while occupied.contains(&candidate) || assigned.contains(&candidate) {
            candidate = format!("{preferred}~{suffix}");
            suffix += 1;
        }
        segment.key = candidate.clone();
        assigned.insert(candidate);
    }

    let mut wire_keys = BTreeSet::new();
    if physical_segments
        .iter()
        .any(|segment| !wire_keys.insert(segment.key.clone()))
    {
        return Err(ListingError::JournalRead);
    }

    Ok(DayListing {
        segments: physical_segments,
    })
}

/// Project one assembled segment to the protocol-3 item shape.
pub(crate) fn segment_item_json(segment: &ListingSegment) -> Value {
    let mut item = Map::new();
    item.insert("key".to_owned(), Value::String(segment.key.clone()));
    item.insert("observed".to_owned(), Value::Bool(segment.observed));
    item.insert("files".to_owned(), listing_files_json(&segment.files));
    if let Some(original_key) = &segment.original_key {
        item.insert(
            "original_key".to_owned(),
            Value::String(original_key.clone()),
        );
    }
    if segment.collided {
        item.insert("segment".to_owned(), Value::String(segment.segment.clone()));
        item.insert("stream".to_owned(), Value::String(segment.stream.clone()));
    }
    Value::Object(item)
}

pub(crate) fn listing_files_json(files: &[ListingFile]) -> Value {
    Value::Array(
        files
            .iter()
            .map(|file| {
                let mut value = Map::new();
                value.insert("name".to_owned(), Value::String(file.name.clone()));
                value.insert("size".to_owned(), Value::from(file.size));
                value.insert("sha256".to_owned(), Value::String(file.sha256.clone()));
                value.insert(
                    "status".to_owned(),
                    Value::String(file.status.as_str().to_owned()),
                );
                if let Some(submitted_name) = &file.submitted_name
                    && submitted_name != &file.name
                {
                    value.insert(
                        "submitted_name".to_owned(),
                        Value::String(submitted_name.clone()),
                    );
                }
                Value::Object(value)
            })
            .collect(),
    )
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
/// to a scrub.
fn resolve_file_status(
    journal_root: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    written: &str,
    size: u64,
) -> Result<FileStatus, ListingError> {
    let name = ContentName::new(written).map_err(|_| ListingError::JournalRead)?;
    if !is_safe_stream_component(segment) || !is_safe_stream_component(stream) {
        return Err(ListingError::JournalRead);
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

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::Map;
    use solstone_core_callosum::{
        DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
    };
    use solstone_core_segment::{
        Kind, PairedStreamBase, SegmentDir, StreamAllocationBase, StreamHints,
        advance_bound_stream, bind_named_stream, bind_paired_stream, list_stream_segments,
        with_takeover_stream_boundary,
    };

    use super::{
        FileStatus, ListingError, ListingFile, SegmentAccumulator, listing_files_json,
        merge_day_listing, native_events, resolve_file_status, segment_events, segment_item_json,
    };

    const DAY: &str = "20261004";
    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn descriptor(submitted: &str, written: &str, size: u64, sha256: &str) -> FileDescriptor {
        FileDescriptor {
            submitted: submitted.to_owned(),
            written: written.to_owned(),
            size,
            sha256: sha256.to_owned(),
            extra: Map::new(),
        }
    }

    fn event(stream: &str, segment: &str, files: Vec<FileDescriptor>) -> DeviceIngestEvent {
        DeviceIngestEvent {
            record_type: "device_ingest".to_owned(),
            record_version: 1,
            outcome: "accepted".to_owned(),
            protocol_version: 3,
            cid: CID_A.to_owned(),
            source: "browser".to_owned(),
            stream: stream.to_owned(),
            day: DAY.to_owned(),
            segment: segment.to_owned(),
            files,
            meta: Map::new(),
            extra: Map::new(),
        }
    }

    fn plant_file(root: &std::path::Path, stream: &str, segment: &str, name: &str, bytes: &[u8]) {
        let path = root.join("chronicle").join(DAY).join(stream).join(segment);
        fs::create_dir_all(&path).expect("segment directory");
        fs::write(path.join(name), bytes).expect("segment file");
    }

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
    fn merge_day_listing_preserves_non_collision_physical_names() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        let basename = "120000_10~browser_b";
        append_event(root, CID_A, Some("browser_b"), basename);
        let events = native_events(root, DAY, Some("browser_b"), CID_A, "audio")
            .expect("tilde directory has a readable native receipt");
        let listing = merge_day_listing(root, DAY, events).expect("listing");

        let segment = listing.segments.first().expect("one segment");
        assert_eq!(segment.key, basename);
        assert_eq!(segment.segment, basename);
        assert!(!segment.collided);
        let json = segment_item_json(segment);
        let mut keys = json
            .as_object()
            .expect("segment item object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["files", "key", "observed"]);
        assert!(json["files"][0].get("submitted_name").is_none());
        assert_eq!(json["files"][0]["name"], "capture.json");
        assert_eq!(listing_files_json(&segment.files)[0]["status"], "missing");
    }

    #[test]
    fn merge_day_listing_keeps_equal_byte_cross_stream_twins() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        let file = descriptor(
            "browser_pages.jsonl",
            "browser_pages.jsonl",
            4,
            &"a".repeat(64),
        );
        plant_file(
            root,
            "browser_a",
            "120000_10",
            "browser_pages.jsonl",
            b"same",
        );
        plant_file(
            root,
            "browser_b",
            "120000_10",
            "browser_pages.jsonl",
            b"same",
        );

        let listing = merge_day_listing(
            root,
            DAY,
            vec![
                event("browser_a", "120000_10", vec![file.clone()]),
                event("browser_b", "120000_10", vec![file]),
            ],
        )
        .expect("listing");

        assert_eq!(listing.segments.len(), 2);
        assert_eq!(listing.segments[0].key, "120000_10~browser_a");
        assert_eq!(listing.segments[1].key, "120000_10~browser_b");
        assert!(listing.segments.iter().all(|item| item.collided));
        assert_eq!(
            segment_item_json(&listing.segments[0])["files"][0]["status"],
            "present"
        );
        assert_eq!(
            segment_item_json(&listing.segments[1])["files"][0]["status"],
            "present"
        );
    }

    #[test]
    fn merge_day_listing_keeps_heal_reduction_segment_local() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        plant_file(root, "browser_a", "120000_10", "new.bin", b"new");
        plant_file(root, "browser_b", "120000_10", "other.bin", b"other");
        let shared = "browser_pages.jsonl";
        let listing = merge_day_listing(
            root,
            DAY,
            vec![
                event(
                    "browser_a",
                    "120000_10",
                    vec![
                        descriptor(shared, "old.bin", 3, &"a".repeat(64)),
                        descriptor(shared, "new.bin", 3, &"b".repeat(64)),
                    ],
                ),
                event(
                    "browser_b",
                    "120000_10",
                    vec![descriptor(shared, "other.bin", 5, &"c".repeat(64))],
                ),
            ],
        )
        .expect("separate segment reductions");

        assert_eq!(listing.segments.len(), 2);
        assert_eq!(listing.segments[0].files.len(), 1);
        assert_eq!(listing.segments[0].files[0].name, "new.bin");
        assert_eq!(listing.segments[0].files[0].status, FileStatus::Present);
        assert_eq!(listing.segments[1].files.len(), 1);
        assert_eq!(listing.segments[1].files[0].name, "other.bin");
    }

    #[test]
    fn merge_day_listing_skips_occupied_alias_candidate() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        let files = [
            ("browser_a", "120000_10"),
            ("browser_b", "120000_10"),
            ("browser_a", "120000_10~browser_b"),
            ("browser_c", "120000_100"),
        ];
        let mut events = Vec::new();
        for (index, (stream, basename)) in files.into_iter().enumerate() {
            let file_name = format!("page-{index}.jsonl");
            plant_file(root, stream, basename, &file_name, b"page");
            events.push(event(
                stream,
                basename,
                vec![descriptor(
                    &file_name,
                    &file_name,
                    4,
                    &format!("{index:x}").repeat(64),
                )],
            ));
        }

        let listing = merge_day_listing(root, DAY, events).expect("listing");
        let keys = listing
            .segments
            .iter()
            .map(|segment| {
                (
                    segment.stream.as_str(),
                    segment.segment.as_str(),
                    segment.key.as_str(),
                    segment.collided,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                ("browser_a", "120000_10", "120000_10~browser_a", true),
                ("browser_b", "120000_10", "120000_10~browser_b~2", true),
                ("browser_c", "120000_100", "120000_100", false),
                (
                    "browser_a",
                    "120000_10~browser_b",
                    "120000_10~browser_b",
                    false
                ),
            ]
        );
    }

    #[test]
    fn native_events_refuse_segment_mismatch() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        let stream = append_event(root, CID_A, None, "120000_1");
        let segment = list_stream_segments(root, DAY, &stream)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert!(
            segment_events(
                root,
                segment.path(),
                DAY,
                &stream,
                "120000_2",
                CID_A,
                "audio",
            )
            .is_err()
        );
    }

    #[test]
    fn listing_status_order_remains_missing_processed_present() {
        assert!(FileStatus::Missing < FileStatus::Processed);
        assert!(FileStatus::Processed < FileStatus::Present);
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

    #[test]
    fn continuity_a_to_b_to_c_reads_ancestor_receipts_and_rejects_unrelated() {
        let dir = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let root = dir.path();
        let stream = append_event(
            root,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            None,
            "120000_1",
        );
        publish(
            root,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        append_event(
            root,
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            Some(&stream),
            "120100_1",
        );
        publish(
            root,
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        );
        append_event(
            root,
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            Some(&stream),
            "120200_1",
        );

        let events = native_events(
            root,
            "20261004",
            Some(&stream),
            "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "audio",
        )
        .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event.cid.as_str())
                .collect::<Vec<_>>(),
            [
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            ]
        );
        assert!(
            native_events(
                root,
                "20261004",
                Some(&stream),
                "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                "audio",
            )
            .is_err()
        );
        assert!(
            native_events(
                root,
                "20261004",
                Some(&stream),
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "video",
            )
            .is_err()
        );
        assert!(
            native_events(
                root,
                "20261004",
                Some("unrelated"),
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "audio",
            )
            .unwrap()
            .is_empty()
        );

        let segment = list_stream_segments(root, "20261004", &stream)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let identity = segment.record_identity().unwrap();
        assert!(
            segment_events(
                root,
                segment.path(),
                "20261004",
                "unrelated",
                identity.name,
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "audio",
            )
            .is_err()
        );
        append_durable_event(
            segment.path(),
            &DurableEvent::DeviceIngest(DeviceIngestEvent {
                record_type: "device_ingest".to_owned(),
                record_version: 1,
                outcome: "accepted".to_owned(),
                protocol_version: 3,
                cid: "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
                    .to_owned(),
                source: "audio".to_owned(),
                stream: stream.clone(),
                day: "20261004".to_owned(),
                segment: identity.name.to_owned(),
                files: Vec::new(),
                meta: Map::new(),
                extra: Map::new(),
            }),
        )
        .unwrap();
        assert!(
            native_events(
                root,
                "20261004",
                Some(&stream),
                "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                "audio",
            )
            .is_err()
        );

        let rejected = crate::stream_identity::bind_ingest_stream(
            root,
            "20261004",
            "120300_1",
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "audio",
            &hints(),
        );
        assert!(rejected.is_err(), "a historical CID cannot upload");
    }

    fn hints() -> StreamHints {
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        }
    }

    fn append_event(
        root: &std::path::Path,
        cid: &str,
        stream: Option<&str>,
        segment: &str,
    ) -> String {
        let bound = match stream {
            Some(stream) => {
                bind_named_stream(root, "20261004", segment, stream, cid, "audio", &hints())
                    .unwrap()
            }
            None => bind_paired_stream(
                root,
                "20261004",
                segment,
                &PairedStreamBase {
                    origin: StreamAllocationBase::Device,
                    input: "phone",
                },
                cid,
                "audio",
                &hints(),
            )
            .unwrap(),
        };
        let segment_dir = SegmentDir::resolve(root, "20261004", segment, &bound.stream).unwrap();
        advance_bound_stream(
            &bound.stream,
            "20261004",
            segment,
            &segment_dir,
            hints(),
            cid,
            "audio",
        )
        .unwrap();
        let event = DeviceIngestEvent {
            record_type: "device_ingest".to_owned(),
            record_version: 1,
            outcome: "ok".to_owned(),
            protocol_version: 3,
            cid: cid.to_owned(),
            source: "audio".to_owned(),
            stream: bound.stream.clone(),
            day: "20261004".to_owned(),
            segment: segment.to_owned(),
            files: vec![FileDescriptor {
                submitted: "capture.json".to_owned(),
                written: "capture.json".to_owned(),
                size: 1,
                sha256: "a".repeat(64),
                extra: Map::new(),
            }],
            meta: Map::new(),
            extra: Map::new(),
        };
        append_durable_event(segment_dir.path(), &DurableEvent::DeviceIngest(event)).unwrap();
        bound.stream
    }

    fn publish(root: &std::path::Path, adopted: &str, retired: &str) {
        with_takeover_stream_boundary(root, adopted, retired, |guard| {
            let plan = guard.plan(adopted, retired);
            guard.publish(&plan).unwrap();
            Ok(())
        })
        .unwrap();
    }
}
