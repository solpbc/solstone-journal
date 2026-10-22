// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Publication of already-created import segments and files.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{AtomicWriteError, JsonWriteOptions, write_json};
use solstone_core_segment::{
    SegmentError, StreamAdvance, StreamHints, UnboundStreamAdvanceError, advance_unbound_stream,
    touch_stream_health_marker,
};

use crate::events::{
    EnrichmentReady, EventEmitter, ObservedSegment, emit_enrichment_ready, emit_observe_observed,
    emit_supervisor_drain,
};

pub const LEGACY_PUBLICATION_SCHEMA: &str = "solstone.import.publication.v1";
pub const PUBLICATION_SCHEMA: &str = "solstone.import.publication.v2";

#[derive(Clone, Debug)]
pub struct CreatedSegment {
    pub day: String,
    pub segment: String,
    pub stream: String,
    pub hints: StreamHints,
}

pub struct PublicationInput<'a> {
    pub journal: &'a Path,
    pub import_dir: Option<&'a Path>,
    pub import_id: &'a str,
    pub importer: &'a str,
    pub revision: Option<&'a str>,
    pub segments: &'a [CreatedSegment],
    pub files_created: &'a [PathBuf],
    /// Files queued for search alone. They bind no segment, mark no day as
    /// changed and ask for no processing: calendar note files beside an
    /// import's transcripts, and those a re-save removed.
    pub index_only: &'a [PathBuf],
    pub may_write_record: Option<&'a (dyn Fn() -> bool + 'a)>,
}

#[derive(Debug)]
pub enum PublishError {
    Thinking(String),
    RecordWrite(AtomicWriteError),
    RecordRead {
        path: PathBuf,
        source: std::io::Error,
    },
    RecordDecode {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thinking(error) => formatter.write_str(error),
            Self::RecordWrite(error) => error.fmt(formatter),
            Self::RecordRead { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::RecordDecode { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for PublishError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Thinking(_) => None,
            Self::RecordWrite(error) => Some(error),
            Self::RecordRead { source, .. } => Some(source),
            Self::RecordDecode { source, .. } => Some(source),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationStatus {
    Success,
    Failure,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SegmentBindingOutcome {
    Bound {
        prev_day: Option<String>,
        prev_segment: Option<String>,
        seq: u64,
    },
    OwnerDeleted,
    FailedAtAdvance {
        error: String,
    },
    FailedAtMarkerWrite {
        error: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SegmentPublicationOutcome {
    pub day: String,
    pub segment: String,
    pub stream: String,
    pub outcome: SegmentBindingOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QueuedFile {
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IndexedFileError {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct IndexPublicationOutcomes {
    #[serde(default, alias = "published")]
    pub queued: Vec<QueuedFile>,
    pub declined: Vec<PathBuf>,
    pub errored: Vec<IndexedFileError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DayMarkerStatus {
    Touched,
    Failed { error: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DayMarkerOutcome {
    pub day: String,
    pub outcome: DayMarkerStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationRecord {
    pub schema: String,
    pub status: PublicationStatus,
    pub segments: Vec<SegmentPublicationOutcome>,
    pub indexing: IndexPublicationOutcomes,
    pub day_markers: Vec<DayMarkerOutcome>,
}

fn is_path_under_segment(path: &Path, day: &str, stream: &str, segment: &str) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    components
        .windows(4)
        .any(|w| w == ["chronicle", day, stream, segment])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueFileOutcome {
    Queued,
    Declined,
}

/// Injectable publication side effects. Real and fake implementations share one seam.
pub trait PublicationOperations {
    fn advance_stream(
        &self,
        journal: &Path,
        segment: &CreatedSegment,
    ) -> Result<StreamAdvance, UnboundStreamAdvanceError>;
    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<QueueFileOutcome, String>;
    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String>;
    fn emit_observed(
        &self,
        journal: &Path,
        revision: Option<&str>,
        day: &str,
        segment: &str,
        stream: &str,
    );
    fn emit_enrichment_ready(
        &self,
        journal: &Path,
        revision: Option<&str>,
        import_id: &str,
        importer: &str,
        days: &[String],
        entries_written: u64,
    );
    fn emit_drain(&self, journal: &Path, revision: Option<&str>, day: &str);
}

pub struct NativePublicationOperations;

impl PublicationOperations for NativePublicationOperations {
    fn advance_stream(
        &self,
        journal: &Path,
        segment: &CreatedSegment,
    ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
        advance_unbound_stream(
            journal,
            &segment.stream,
            &segment.day,
            &segment.segment,
            segment.hints.clone(),
        )
    }

    fn enqueue_path(&self, journal: &Path, path: &Path) -> Result<QueueFileOutcome, String> {
        if !solstone_core_indexer_store::scan::publication_path_is_indexable(journal, path) {
            return Ok(QueueFileOutcome::Declined);
        }
        solstone_core_indexer_store::todo::enqueue_path(journal, path)
            .map(|()| QueueFileOutcome::Queued)
            .map_err(|error| error.to_string())
    }

    fn touch_stream_health_marker(&self, journal: &Path, day: &str) -> Result<(), String> {
        touch_stream_health_marker(journal, day).map_err(|error| error.to_string())
    }

    fn emit_observed(
        &self,
        journal: &Path,
        revision: Option<&str>,
        day: &str,
        segment: &str,
        stream: &str,
    ) {
        let emitter = EventEmitter::new(journal, revision);
        emit_observe_observed(
            &emitter,
            &ObservedSegment {
                day: day.to_owned(),
                segment: segment.to_owned(),
                stream: stream.to_owned(),
            },
        );
    }

    fn emit_enrichment_ready(
        &self,
        journal: &Path,
        revision: Option<&str>,
        import_id: &str,
        importer: &str,
        days: &[String],
        entries_written: u64,
    ) {
        let emitter = EventEmitter::new(journal, revision);
        emit_enrichment_ready(
            &emitter,
            &EnrichmentReady {
                import_id: import_id.to_owned(),
                importer: importer.to_owned(),
                days: days.to_vec(),
                entries_written,
            },
        );
    }

    fn emit_drain(&self, journal: &Path, revision: Option<&str>, day: &str) {
        emit_supervisor_drain(&EventEmitter::new(journal, revision), day);
    }
}

pub fn publish(input: PublicationInput<'_>) -> Result<PublicationRecord, PublishError> {
    publish_with_operations(input, &NativePublicationOperations)
}

pub fn publish_with_operations(
    input: PublicationInput<'_>,
    operations: &dyn PublicationOperations,
) -> Result<PublicationRecord, PublishError> {
    let mut segments = Vec::with_capacity(input.segments.len());
    let mut failed_streams = BTreeSet::new();
    for segment in input.segments {
        let outcome = if failed_streams.contains(&segment.stream) {
            SegmentBindingOutcome::FailedAtAdvance {
                error: "not attempted after an earlier topology failure in this stream".to_owned(),
            }
        } else {
            match operations.advance_stream(input.journal, segment) {
                Ok(advance) => SegmentBindingOutcome::Bound {
                    prev_day: advance.prev_day,
                    prev_segment: advance.prev_segment,
                    seq: advance.seq,
                },
                Err(UnboundStreamAdvanceError::Advance(SegmentError::Tombstoned { .. })) => {
                    SegmentBindingOutcome::OwnerDeleted
                }
                Err(UnboundStreamAdvanceError::Advance(error)) => {
                    failed_streams.insert(segment.stream.clone());
                    SegmentBindingOutcome::FailedAtAdvance {
                        error: error.to_string(),
                    }
                }
                Err(UnboundStreamAdvanceError::MarkerWrite { path, source }) => {
                    failed_streams.insert(segment.stream.clone());
                    SegmentBindingOutcome::FailedAtMarkerWrite {
                        error: format!("{}: {source}", path.display()),
                    }
                }
            }
        };
        segments.push(SegmentPublicationOutcome {
            day: segment.day.clone(),
            segment: segment.segment.clone(),
            stream: segment.stream.clone(),
            outcome,
        });
    }

    let mut indexing = IndexPublicationOutcomes::default();
    for path in input.files_created.iter().chain(input.index_only) {
        let under_deleted = segments.iter().any(|s| {
            matches!(s.outcome, SegmentBindingOutcome::OwnerDeleted)
                && is_path_under_segment(path, &s.day, &s.stream, &s.segment)
        });
        if under_deleted {
            continue;
        }
        match operations.enqueue_path(input.journal, path) {
            Ok(QueueFileOutcome::Queued) => {
                indexing.queued.push(QueuedFile {
                    path: path.clone(),
                    warnings: Vec::new(),
                });
            }
            Ok(QueueFileOutcome::Declined) => indexing.declined.push(path.clone()),
            Err(error) => indexing.errored.push(IndexedFileError {
                path: path.clone(),
                error,
            }),
        }
    }

    let days = input
        .segments
        .iter()
        .zip(&segments)
        .filter(|(_, s_out)| matches!(s_out.outcome, SegmentBindingOutcome::Bound { .. }))
        .map(|(s_in, _)| s_in.day.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut day_markers = Vec::with_capacity(days.len());
    for day in &days {
        let outcome = match operations.touch_stream_health_marker(input.journal, day) {
            Ok(()) => DayMarkerStatus::Touched,
            Err(error) => DayMarkerStatus::Failed { error },
        };
        day_markers.push(DayMarkerOutcome {
            day: day.clone(),
            outcome,
        });
    }
    let thinking_file_count = input
        .files_created
        .iter()
        .filter(|path| {
            !input.segments.iter().zip(&segments).any(|(s_in, s_out)| {
                (!matches!(s_out.outcome, SegmentBindingOutcome::Bound { .. }))
                    && is_path_under_segment(path, &s_in.day, &s_in.stream, &s_in.segment)
            })
        })
        .count();

    let failed = segments.iter().any(|segment| {
        matches!(
            segment.outcome,
            SegmentBindingOutcome::FailedAtAdvance { .. }
                | SegmentBindingOutcome::FailedAtMarkerWrite { .. }
        )
    }) || !indexing.errored.is_empty()
        || day_markers
            .iter()
            .any(|marker| matches!(marker.outcome, DayMarkerStatus::Failed { .. }));
    let mut record = PublicationRecord {
        schema: PUBLICATION_SCHEMA.to_owned(),
        status: if failed {
            PublicationStatus::Failure
        } else {
            PublicationStatus::Success
        },
        segments,
        indexing,
        day_markers,
    };
    let should_write = input.may_write_record.is_none_or(|predicate| predicate());
    if should_write {
        record = write_publication_record(input.import_dir, &record)?;
        // The durable coordinate scope is visible before any admission wakeup.
        for segment in &record.segments {
            if matches!(segment.outcome, SegmentBindingOutcome::Bound { .. }) {
                operations.emit_observed(
                    input.journal,
                    input.revision,
                    &segment.day,
                    &segment.segment,
                    &segment.stream,
                );
            }
        }
        if let Some(day) = days.last() {
            operations.emit_drain(input.journal, input.revision, day);
        }
        if thinking_file_count > 0 && !days.is_empty() {
            operations.emit_enrichment_ready(
                input.journal,
                input.revision,
                input.import_id,
                input.importer,
                &days,
                u64::try_from(thinking_file_count).expect("file count fits u64"),
            );
        }
    }
    Ok(record)
}

pub fn write_publication_record(
    import_dir: Option<&Path>,
    record: &PublicationRecord,
) -> Result<PublicationRecord, PublishError> {
    let Some(import_dir) = import_dir else {
        return Ok(record.clone());
    };
    let _lock = solstone_core_journal_io::hold_lock(
        import_dir.join("thinking.lock"),
        solstone_core_journal_io::LockOptions::default(),
    )
    .map_err(|error| PublishError::Thinking(error.to_string()))?;
    let mut record = record.clone();
    if read_publication_record(import_dir)?
        .is_some_and(|old| old.schema == LEGACY_PUBLICATION_SCHEMA)
    {
        // A pre-ladder import can finish a retry without acquiring fresh
        // spending consent. Its files remain visible to ordinary discovery.
        record.schema = LEGACY_PUBLICATION_SCHEMA.into();
    } else {
        crate::thinking::prepare(import_dir).map_err(PublishError::Thinking)?;
    }
    write_json(
        import_dir.join("imported.json"),
        &record,
        JsonWriteOptions::default(),
    )
    .map_err(PublishError::RecordWrite)?;
    Ok(record)
}

pub fn read_publication_record(
    import_dir: &Path,
) -> Result<Option<PublicationRecord>, PublishError> {
    let path = import_dir.join("imported.json");
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PublishError::RecordRead { path, source }),
    };
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(|source| PublishError::RecordDecode { path, source })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use solstone_core_indexer_store::merge::fingerprint_edge_rows;
    use solstone_core_segment::{SegmentError, UnboundStreamAdvanceError};

    use super::*;

    #[derive(Default)]
    struct FakeOperations {
        durable_scope: Option<PathBuf>,
        advances: RefCell<Vec<Result<StreamAdvance, UnboundStreamAdvanceError>>>,
        enqueues: RefCell<Vec<Result<QueueFileOutcome, String>>>,
        enqueues_seen: RefCell<Vec<PathBuf>>,
        markers: RefCell<Vec<String>>,
        observed: RefCell<Vec<(String, String, String)>>,
        enrichment: RefCell<Vec<(Vec<String>, u64)>>,
        drains: RefCell<Vec<String>>,
    }

    impl FakeOperations {
        fn with_advances(advances: Vec<Result<StreamAdvance, UnboundStreamAdvanceError>>) -> Self {
            Self {
                advances: RefCell::new(advances),
                ..Self::default()
            }
        }
    }

    impl PublicationOperations for FakeOperations {
        fn advance_stream(
            &self,
            _: &Path,
            _: &CreatedSegment,
        ) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
            self.advances.borrow_mut().remove(0)
        }

        fn enqueue_path(&self, _: &Path, path: &Path) -> Result<QueueFileOutcome, String> {
            self.enqueues_seen.borrow_mut().push(path.to_path_buf());
            self.enqueues.borrow_mut().remove(0)
        }

        fn touch_stream_health_marker(&self, _: &Path, day: &str) -> Result<(), String> {
            self.markers.borrow_mut().push(day.to_owned());
            Ok(())
        }

        fn emit_observed(&self, _: &Path, _: Option<&str>, day: &str, segment: &str, stream: &str) {
            if let Some(directory) = &self.durable_scope {
                let record = read_publication_record(directory)
                    .unwrap()
                    .expect("published before notification");
                assert!(record.segments.iter().any(|row| row.day == day
                    && row.segment == segment
                    && row.stream == stream
                    && matches!(row.outcome, SegmentBindingOutcome::Bound { .. })));
            }
            self.observed.borrow_mut().push((
                day.to_owned(),
                segment.to_owned(),
                stream.to_owned(),
            ));
        }

        fn emit_enrichment_ready(
            &self,
            _: &Path,
            _: Option<&str>,
            _: &str,
            _: &str,
            days: &[String],
            entries_written: u64,
        ) {
            self.enrichment
                .borrow_mut()
                .push((days.to_vec(), entries_written));
        }

        fn emit_drain(&self, _: &Path, _: Option<&str>, day: &str) {
            self.drains.borrow_mut().push(day.to_owned());
        }
    }

    fn segment(day: &str, key: &str) -> CreatedSegment {
        CreatedSegment {
            day: day.to_owned(),
            segment: key.to_owned(),
            stream: "import.apple".to_owned(),
            hints: StreamHints::default(),
        }
    }

    fn advance(sequence: u64) -> Result<StreamAdvance, UnboundStreamAdvanceError> {
        Ok(StreamAdvance {
            prev_day: None,
            prev_segment: None,
            seq: sequence,
        })
    }

    fn input<'a>(
        journal: &'a Path,
        import_dir: Option<&'a Path>,
        segments: &'a [CreatedSegment],
        files: &'a [PathBuf],
    ) -> PublicationInput<'a> {
        PublicationInput {
            journal,
            import_dir,
            import_id: "import-1",
            importer: "ics",
            revision: Some("test"),
            segments,
            files_created: files,
            index_only: &[],
            may_write_record: None,
        }
    }

    #[test]
    fn marker_failure_stops_later_advances_in_that_stream() {
        let temporary = tempfile::TempDir::new().unwrap();
        let mut other_stream = segment("20260801", "120300_60");
        other_stream.stream = "import.google".to_owned();
        let segments = vec![
            segment("20260801", "120000_60"),
            segment("20260801", "120100_60"),
            segment("20260801", "120200_60"),
            other_stream,
        ];
        let fake = FakeOperations::with_advances(vec![
            advance(1),
            Err(UnboundStreamAdvanceError::MarkerWrite {
                path: PathBuf::from("marker"),
                source: AtomicWriteError::Io {
                    path: PathBuf::from("marker"),
                    source: std::io::Error::other("blocked"),
                },
            }),
            advance(3),
        ]);
        let record =
            publish_with_operations(input(temporary.path(), None, &segments, &[]), &fake).unwrap();
        assert!(matches!(record.status, PublicationStatus::Failure));
        assert!(matches!(
            record.segments[1].outcome,
            SegmentBindingOutcome::FailedAtMarkerWrite { .. }
        ));
        assert!(matches!(
            &record.segments[2].outcome,
            SegmentBindingOutcome::FailedAtAdvance { error }
                if error.contains("not attempted")
        ));
        assert!(matches!(
            record.segments[3].outcome,
            SegmentBindingOutcome::Bound { .. }
        ));
        assert_eq!(fake.observed.borrow().len(), 2);
        assert_eq!(record.segments.len(), 4);
    }

    #[test]
    fn separates_index_published_declined_and_errored_outcomes() {
        let temporary = tempfile::TempDir::new().unwrap();
        let segments = vec![segment("20260801", "120000_60")];
        let files = vec![
            PathBuf::from("first.jsonl"),
            PathBuf::from("declined.raw"),
            PathBuf::from("failed.jsonl"),
            PathBuf::from("last.jsonl"),
        ];
        let fake = FakeOperations {
            advances: RefCell::new(vec![advance(1)]),
            enqueues: RefCell::new(vec![
                Ok(QueueFileOutcome::Queued),
                Ok(QueueFileOutcome::Declined),
                Err("index broke".to_owned()),
                Ok(QueueFileOutcome::Queued),
            ]),
            ..FakeOperations::default()
        };
        let record =
            publish_with_operations(input(temporary.path(), None, &segments, &files), &fake)
                .unwrap();
        assert_eq!(record.indexing.queued.len(), 2);
        assert_eq!(
            record.indexing.declined,
            vec![PathBuf::from("declined.raw")]
        );
        assert_eq!(
            record.indexing.errored[0].path,
            PathBuf::from("failed.jsonl")
        );
        assert_eq!(fake.observed.borrow().len(), 1);
        assert!(matches!(record.status, PublicationStatus::Failure));
    }

    #[test]
    fn markers_drains_and_enrichment_use_sorted_distinct_segment_days() {
        let temporary = tempfile::TempDir::new().unwrap();
        let segments = vec![
            segment("20260803", "120000_60"),
            segment("20260801", "120100_60"),
            segment("20260802", "120200_60"),
            segment("20260801", "120300_60"),
        ];
        let files = vec![PathBuf::from("stream-name-two-levels-up/file.jsonl")];
        let fake = FakeOperations {
            advances: RefCell::new(vec![advance(1), advance(2), advance(3), advance(4)]),
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued)]),
            ..FakeOperations::default()
        };
        publish_with_operations(input(temporary.path(), None, &segments, &files), &fake).unwrap();
        let expected = vec![
            "20260801".to_owned(),
            "20260802".to_owned(),
            "20260803".to_owned(),
        ];
        assert_eq!(*fake.markers.borrow(), expected);
        assert_eq!(*fake.drains.borrow(), vec!["20260803".to_owned()]);
        assert_eq!(*fake.enrichment.borrow(), vec![(expected, 1)]);
        assert_eq!(fake.observed.borrow().len(), 4);
    }

    #[test]
    fn index_only_files_are_queued_for_search_and_ask_for_no_processing() {
        let temporary = tempfile::TempDir::new().unwrap();
        let notes = vec![
            PathBuf::from("chronicle/20260801/import.ics/120000_60/calendar_a.jsonl"),
            PathBuf::from("chronicle/20260802/import.ics/090000_60/calendar_b.jsonl"),
        ];
        let fake = FakeOperations {
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued); 2]),
            ..FakeOperations::default()
        };
        let mut alone = input(temporary.path(), None, &[], &[]);
        alone.index_only = &notes;
        let record = publish_with_operations(alone, &fake).unwrap();
        assert_eq!(*fake.enqueues_seen.borrow(), notes);
        assert_eq!(record.indexing.queued.len(), 2);
        assert!(record.day_markers.is_empty());
        assert!(fake.markers.borrow().is_empty());
        assert!(fake.drains.borrow().is_empty());
        assert!(fake.enrichment.borrow().is_empty());

        // The twin: the transcript beside them still marks its day and asks
        // for processing, counted without the calendar files.
        let segments = vec![segment("20260801", "120000_60")];
        let transcript = vec![PathBuf::from(
            "chronicle/20260801/import.ics/120000_60/event_transcript.md",
        )];
        let fake = FakeOperations {
            advances: RefCell::new(vec![advance(1)]),
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued); 3]),
            ..FakeOperations::default()
        };
        let mut both = input(temporary.path(), None, &segments, &transcript);
        both.index_only = &notes;
        publish_with_operations(both, &fake).unwrap();
        assert_eq!(fake.enqueues_seen.borrow().len(), 3);
        assert_eq!(*fake.markers.borrow(), vec!["20260801".to_owned()]);
        assert_eq!(
            *fake.enrichment.borrow(),
            vec![(vec!["20260801".to_owned()], 1)]
        );
    }

    #[test]
    fn empty_archive_shaped_publication_emits_no_day_side_effects() {
        let temporary = tempfile::TempDir::new().unwrap();
        let fake = FakeOperations::default();
        let record =
            publish_with_operations(input(temporary.path(), None, &[], &[]), &fake).unwrap();
        assert!(record.day_markers.is_empty());
        assert!(fake.markers.borrow().is_empty());
        assert!(fake.drains.borrow().is_empty());
        assert!(fake.enrichment.borrow().is_empty());
    }

    #[test]
    fn persisted_record_round_trips_separate_index_buckets() {
        let temporary = tempfile::TempDir::new().unwrap();
        let import_dir = temporary.path().join("imports/import-1");
        fs::create_dir_all(&import_dir).unwrap();
        let segments = vec![segment("20260801", "120000_60")];
        let files = vec![PathBuf::from("declined.raw"), PathBuf::from("failed.jsonl")];
        let fake = FakeOperations {
            durable_scope: Some(import_dir.clone()),
            advances: RefCell::new(vec![advance(1)]),
            enqueues: RefCell::new(vec![
                Ok(QueueFileOutcome::Declined),
                Err("bad index".to_owned()),
            ]),
            ..FakeOperations::default()
        };
        publish_with_operations(
            input(temporary.path(), Some(&import_dir), &segments, &files),
            &fake,
        )
        .unwrap();
        let saved = read_publication_record(&import_dir).unwrap().unwrap();
        assert_eq!(saved.indexing.declined, vec![PathBuf::from("declined.raw")]);
        assert_eq!(
            saved.indexing.errored[0].path,
            PathBuf::from("failed.jsonl")
        );
    }

    #[test]
    fn failed_scope_publication_emits_no_admission_notifications() {
        let temporary = tempfile::tempdir().unwrap();
        let import_dir = temporary.path().join("imports/broken");
        fs::create_dir_all(&import_dir).unwrap();
        fs::create_dir(import_dir.join("imported.json")).unwrap();
        let segments = vec![segment("20260801", "120000_60")];
        let fake = FakeOperations::with_advances(vec![advance(1)]);
        assert!(
            publish_with_operations(
                input(temporary.path(), Some(&import_dir), &segments, &[]),
                &fake
            )
            .is_err()
        );
        assert!(fake.observed.borrow().is_empty());
        assert!(fake.drains.borrow().is_empty());
        assert!(fake.enrichment.borrow().is_empty());
    }

    #[test]
    fn no_import_directory_never_mutates_owner_source() {
        let temporary = tempfile::TempDir::new().unwrap();
        let owner = temporary.path().join("owner");
        fs::create_dir_all(&owner).unwrap();
        fs::write(owner.join("source.txt"), "source").unwrap();
        // A generic text/audio path would be trivial: it has already copied media into
        // imports before its try block. This exercises the pre-setup None contract.
        let report = crate::observe_source_immutability(&owner, |_| {
            write_publication_record(
                None,
                &PublicationRecord {
                    schema: PUBLICATION_SCHEMA.to_owned(),
                    status: PublicationStatus::Success,
                    segments: vec![],
                    indexing: IndexPublicationOutcomes::default(),
                    day_markers: vec![],
                },
            )
            .unwrap();
        })
        .unwrap();
        assert!(!report.violated());
    }

    #[test]
    fn clean_indexing_keeps_empty_declined_and_errored_vectors() {
        let temporary = tempfile::TempDir::new().unwrap();
        let segments = vec![segment("20260801", "120000_60")];
        let files = vec![PathBuf::from("one.jsonl")];
        let fake = FakeOperations {
            advances: RefCell::new(vec![advance(1)]),
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued)]),
            ..FakeOperations::default()
        };
        let record =
            publish_with_operations(input(temporary.path(), None, &segments, &files), &fake)
                .unwrap();
        assert_eq!(record.indexing.queued.len(), 1);
        assert!(record.indexing.declined.is_empty());
        assert!(record.indexing.errored.is_empty());
        assert_eq!(record.status, PublicationStatus::Success);
    }

    #[test]
    fn real_publish_indexes_edge_source_and_unchanged_republish_is_stable() {
        let temporary = tempfile::TempDir::new().unwrap();
        let root = temporary.path();
        for (entity, name) in [("alice", "Alice Edge"), ("bob", "Bob Edge")] {
            let entity_path = root.join(format!("entities/{entity}/entity.json"));
            fs::create_dir_all(entity_path.parent().unwrap()).unwrap();
            fs::write(
                entity_path,
                format!(r#"{{"name":"{name}","type":"Person"}}"#),
            )
            .unwrap();
            let facet_path = root.join(format!("facets/work/entities/{entity}/entity.json"));
            fs::create_dir_all(facet_path.parent().unwrap()).unwrap();
            fs::write(facet_path, "{}").unwrap();
        }
        let source = root.join("facets/work/entities/20260304.jsonl");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(
            &source,
            "{\"name\":\"Alice Edge\",\"segments\":[\"s1\"]}\n{\"name\":\"Bob Edge\",\"segments\":[\"s1\"]}\n",
        )
        .unwrap();
        solstone_core_indexer_store::db::reset_index(root).unwrap();
        let before = fingerprint_edge_rows(root).unwrap();
        let files = vec![source];

        let lease = solstone_core_journal_io::hold_lock(
            solstone_core_indexer_store::db::db_path(root),
            Default::default(),
        )
        .unwrap();
        let first = publish(input(root, None, &[], &files)).unwrap();
        assert_eq!(first.indexing.queued.len(), 1);
        assert_eq!(
            solstone_core_indexer_store::todo::inspect(root)
                .unwrap()
                .keys,
            ["facets/work/entities/20260304.jsonl"]
        );
        drop(lease);
        solstone_core_indexer_store::todo::drain(root, Default::default()).unwrap();
        let after_first = fingerprint_edge_rows(root).unwrap();
        assert_ne!(after_first, before);

        let second = publish(input(root, None, &[], &files)).unwrap();
        assert_eq!(second.indexing.queued.len(), 1);
        solstone_core_indexer_store::todo::drain(root, Default::default()).unwrap();
        assert_eq!(fingerprint_edge_rows(root).unwrap(), after_first);
        let check = solstone_core_indexer_store::scan::scan_journal(root, false).unwrap();
        assert_eq!((check.indexed, check.removed), (0, 0));
    }

    #[test]
    fn advance_error_stops_only_that_stream() {
        let temporary = tempfile::TempDir::new().unwrap();
        let mut other_stream = segment("20260801", "120200_60");
        other_stream.stream = "import.google".to_owned();
        let segments = vec![
            segment("20260801", "120000_60"),
            segment("20260801", "120100_60"),
            other_stream,
        ];
        let fake = FakeOperations::with_advances(vec![
            Err(UnboundStreamAdvanceError::Advance(
                SegmentError::StreamInput("forced"),
            )),
            advance(2),
        ]);
        let record =
            publish_with_operations(input(temporary.path(), None, &segments, &[]), &fake).unwrap();
        assert!(matches!(
            record.segments[0].outcome,
            SegmentBindingOutcome::FailedAtAdvance { .. }
        ));
        assert!(matches!(
            record.segments[1].outcome,
            SegmentBindingOutcome::FailedAtAdvance { .. }
        ));
        assert!(matches!(
            record.segments[2].outcome,
            SegmentBindingOutcome::Bound { .. }
        ));
    }

    #[test]
    fn a_recent_body_day_starts_thinking_like_any_other_import() {
        let temporary = tempfile::TempDir::new().unwrap();
        for stream in ["import.apple_health", "import.oura", "import.strava"] {
            let mut s = segment("20260729", "120000_60");
            s.stream = stream.to_string();
            let file = temporary.path().join(format!(
                "chronicle/20260729/{stream}/120000_60/workout.json"
            ));
            let fake = FakeOperations {
                advances: RefCell::new(vec![advance(1)]),
                enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued)]),
                ..FakeOperations::default()
            };
            let record = publish_with_operations(
                input(temporary.path(), None, &[s], std::slice::from_ref(&file)),
                &fake,
            )
            .unwrap();
            assert_eq!(record.status, PublicationStatus::Success);
            assert_eq!(fake.observed.borrow().len(), 1);
            assert_eq!(*fake.markers.borrow(), vec!["20260729".to_owned()]);
            assert_eq!(*fake.drains.borrow(), vec!["20260729".to_owned()]);
            assert_eq!(
                *fake.enrichment.borrow(),
                vec![(vec!["20260729".to_owned()], 1)]
            );
        }
    }

    #[test]
    fn an_older_body_day_enters_the_same_paced_discovery_as_other_imports() {
        let temporary = tempfile::TempDir::new().unwrap();
        let mut old = segment("20260728", "120000_60");
        old.stream = "import.strava".to_string();
        let file = temporary
            .path()
            .join("chronicle/20260728/import.strava/120000_60/workout.json");
        let fake = FakeOperations {
            advances: RefCell::new(vec![advance(1)]),
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued)]),
            ..FakeOperations::default()
        };
        let record = publish_with_operations(
            input(temporary.path(), None, &[old], std::slice::from_ref(&file)),
            &fake,
        )
        .unwrap();
        assert_eq!(record.status, PublicationStatus::Success);
        assert!(matches!(
            record.segments[0].outcome,
            SegmentBindingOutcome::Bound { .. }
        ));
        assert_eq!(*fake.enqueues_seen.borrow(), vec![file]);
        assert_eq!(fake.observed.borrow().len(), 1);
        assert_eq!(record.day_markers.len(), 1);
        assert_eq!(*fake.drains.borrow(), vec!["20260728".to_owned()]);
        assert_eq!(fake.enrichment.borrow().len(), 1);

        // Source age does not remove a published import from discovery.
        let fake = FakeOperations::with_advances(vec![advance(1)]);
        publish_with_operations(
            input(
                temporary.path(),
                None,
                &[segment("20200101", "120000_60")],
                &[],
            ),
            &fake,
        )
        .unwrap();
        assert_eq!(*fake.drains.borrow(), vec!["20200101".to_owned()]);
    }

    #[test]
    fn real_publish_of_an_old_body_day_retains_declined_index_status_and_durable_scope() {
        let temporary = tempfile::TempDir::new().unwrap();
        let file_path = temporary
            .path()
            .join("chronicle/20260801/import.strava/120000_60/body.csv");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(&file_path, "header\n1,2,3\n").unwrap();
        let mut s = segment("20260801", "120000_60");
        s.stream = "import.strava".to_string();
        let files = vec![file_path.clone()];

        let record = publish(input(temporary.path(), None, &[s], &files)).unwrap();
        assert_eq!(record.status, PublicationStatus::Success);
        assert_eq!(record.indexing.declined, vec![file_path]);
        assert!(record.indexing.queued.is_empty());
        assert_eq!(record.day_markers.len(), 1);
    }

    #[test]
    fn publish_with_tombstoned_segment_binds_owner_deleted_and_skips_rescan() {
        let temporary = tempfile::TempDir::new().unwrap();
        let seg1 = segment("20260801", "120000_60");
        let seg2 = segment("20260801", "120100_60");
        let file1 = temporary
            .path()
            .join("chronicle/20260801/import.apple/120000_60/file1.jsonl");
        let file2 = temporary
            .path()
            .join("chronicle/20260801/import.apple/120100_60/file2.jsonl");
        let files = vec![file1, file2.clone()];
        let fake = FakeOperations {
            advances: RefCell::new(vec![
                Err(UnboundStreamAdvanceError::Advance(
                    SegmentError::Tombstoned {
                        path: PathBuf::from("deleted"),
                    },
                )),
                advance(2),
            ]),
            enqueues: RefCell::new(vec![Ok(QueueFileOutcome::Queued)]),
            ..FakeOperations::default()
        };
        let record =
            publish_with_operations(input(temporary.path(), None, &[seg1, seg2], &files), &fake)
                .unwrap();
        assert_eq!(record.status, PublicationStatus::Success);
        assert_eq!(
            record.segments[0].outcome,
            SegmentBindingOutcome::OwnerDeleted
        );
        assert!(matches!(
            record.segments[1].outcome,
            SegmentBindingOutcome::Bound { .. }
        ));
        assert_eq!(*fake.enqueues_seen.borrow(), vec![file2]);
        assert_eq!(fake.observed.borrow().len(), 1);
        assert_eq!(
            *fake.enrichment.borrow(),
            vec![(vec!["20260801".to_owned()], 1)]
        );
    }
}
