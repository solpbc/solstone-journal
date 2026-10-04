// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private connection-owned memory append state machine.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use solstone_core_format::agent_memory::{
    ChainPredecessor, Coordinate, OperationRecord, Origin, Readiness, SourceKey, digest,
    validate_operation_id, validate_record,
};
use solstone_core_journal_io::atomic::{
    DetailedAtomicOutcome, ExclusivePublication, FinalNameConfirmation, MetadataDurability,
    StageCleanup, atomic_replace_detailed, write_bytes_exclusive_detailed,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, find_available_segment, path_lexists, sync_dir,
};
use solstone_core_mcp_audit::{
    Admission, AuditCoordinates, AuditWriteError, Outcome, ResultShape, ToolName,
    write_interaction_record, write_outcome_record,
};
use solstone_core_retention::{
    layout::segment_rel, staging::staged_name, tombstone::TOMBSTONE_NAME,
};
use solstone_core_segment::{
    SegmentDir, StreamAdvance, advance_agent_memory_stream, bind_agent_memory_stream,
    hold_source_mutation,
};

const MAX_NOTE_BYTES: usize = 32_768;
const MAX_SEGMENT_ATTEMPTS: usize = 128;
const NOTE_FILE: &str = "note.txt";
const ORIGIN_FILE: &str = "origin.json";
const READY_FILE: &str = "ready.json";
const RECORD_VALIDATION_REASON: &str = "memory record validation failed";
const RECORD_READ_REASON: &str = "memory record could not be read";

/// Result of one append attempt. Memory content is never returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AppendResult {
    Stored(AppendReceipt),
    Replayed(AppendReceipt),
    Deleted(AppendReceipt),
    UncertainRetry { operation_id: String },
}

/// Durable, body-free details returned for a completed append.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppendReceipt {
    pub origin: Origin,
    pub created_at: DateTime<Utc>,
    pub digest: String,
    pub byte_count: usize,
}

/// A validation or pre-publication failure.
#[derive(Debug)]
pub(crate) struct AppendError(&'static str);

impl fmt::Display for AppendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for AppendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Publication {
    Confirmed,
    PublishedUnconfirmed,
    NotPublished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FilePublication {
    Confirmed,
    PublishedUnconfirmed,
    NotPublished,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SegmentObservation {
    Missing,
    Live,
    Tombstone,
    Staged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OperationAddress {
    source_component: String,
    operation_component: String,
}

impl OperationAddress {
    fn new(source: &SourceKey, operation_id: &str) -> Self {
        Self {
            source_component: source.component().to_owned(),
            operation_component: digest(operation_id.as_bytes()),
        }
    }

    fn relative(&self) -> String {
        format!(
            "config/agent-memory/{}/{}.json",
            self.source_component, self.operation_component
        )
    }
}

#[derive(Clone, Debug)]
struct AuditLocation {
    day: String,
    stream: String,
    segment: String,
}

trait AppendStore {
    fn hold_source(&mut self, source_component: &str) -> Result<(), StoreFailure>;
    fn admit(
        &mut self,
        verified_id: &str,
        source: &SourceKey,
        operation_id: &str,
        note_digest: &str,
        byte_count: usize,
        now: DateTime<Utc>,
    ) -> Result<AuditLocation, AdmissionFailure>;
    fn load_operation(
        &mut self,
        address: &OperationAddress,
    ) -> Result<Option<Vec<u8>>, StoreFailure>;
    fn choose_coordinate(
        &mut self,
        stream: &str,
        now: DateTime<Utc>,
    ) -> Result<Coordinate, StoreFailure>;
    fn save_operation(
        &mut self,
        address: &OperationAddress,
        bytes: &[u8],
        create_only: bool,
    ) -> Publication;
    fn observe_segment(
        &mut self,
        coordinate: &Coordinate,
    ) -> Result<SegmentObservation, StoreFailure>;
    fn create_segment(&mut self, coordinate: &Coordinate) -> Result<(), StoreFailure>;
    fn bind_stream(
        &mut self,
        source: &SourceKey,
        coordinate: &Coordinate,
    ) -> Result<(), StoreFailure>;
    fn read_file(
        &mut self,
        coordinate: &Coordinate,
        name: &str,
    ) -> Result<Option<Vec<u8>>, StoreFailure>;
    fn publish_file(
        &mut self,
        coordinate: &Coordinate,
        name: &str,
        bytes: &[u8],
    ) -> FilePublication;
    fn advance_stream(
        &mut self,
        source: &SourceKey,
        coordinate: &Coordinate,
    ) -> Result<StreamAdvance, StoreFailure>;
    fn write_outcome(
        &mut self,
        location: &AuditLocation,
        outcome: Outcome,
        reason: Option<&str>,
        result: Option<ResultShape>,
        now: DateTime<Utc>,
    ) -> Publication;
}

#[derive(Clone, Debug)]
struct StoreFailure(&'static str);

impl fmt::Display for StoreFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

#[derive(Clone, Debug)]
enum AdmissionFailure {
    NotPublished,
    PublishedUnconfirmed,
}

/// Production wrapper for the shared append state machine.
#[allow(
    dead_code,
    reason = "private connection-memory capability is intentionally unwired"
)]
pub(crate) fn append_connection_memory(
    journal: &Path,
    verified_id: &str,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<AppendResult, AppendError> {
    append_with(
        &mut JournalStore::new(journal),
        verified_id,
        text,
        operation_id,
        now,
    )
}

fn append_with<S: AppendStore>(
    store: &mut S,
    verified_id: &str,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<AppendResult, AppendError> {
    let note = text.as_bytes();
    if note.is_empty() || note.len() > MAX_NOTE_BYTES {
        return Err(AppendError(
            "memory text length is outside the accepted range",
        ));
    }
    validate_operation_id(operation_id)
        .map_err(|_| AppendError("invalid memory operation identifier"))?;

    let source = SourceKey::from_verified_id(verified_id);
    let address = OperationAddress::new(&source, operation_id);
    let note_digest = digest(note);
    store
        .hold_source(source.component())
        .map_err(|_| AppendError("memory source lock could not be acquired"))?;

    let audit = match store.admit(
        verified_id,
        &source,
        operation_id,
        &note_digest,
        note.len(),
        now,
    ) {
        Ok(location) => location,
        Err(AdmissionFailure::PublishedUnconfirmed) => {
            return Ok(AppendResult::UncertainRetry {
                operation_id: operation_id.to_owned(),
            });
        }
        Err(AdmissionFailure::NotPublished) => {
            return Err(AppendError("memory audit admission could not be published"));
        }
    };

    let loaded = match store.load_operation(&address) {
        Ok(value) => value,
        Err(_) => {
            return terminal_error(store, &audit, now, RECORD_READ_REASON, operation_id);
        }
    };
    let (mut record, was_ready, is_new) = match loaded {
        Some(bytes) => {
            let record: OperationRecord = match serde_json::from_slice(&bytes) {
                Ok(record) => record,
                Err(_) => {
                    return terminal_error(
                        store,
                        &audit,
                        now,
                        RECORD_VALIDATION_REASON,
                        operation_id,
                    );
                }
            };
            match validate_record(&record, &source, operation_id, note) {
                Ok(()) => {}
                Err(
                    solstone_core_format::agent_memory::FormatError::DigestMismatch
                    | solstone_core_format::agent_memory::FormatError::ByteCountMismatch,
                ) => {
                    return terminal_error(
                        store,
                        &audit,
                        now,
                        "operation identifier is bound to different bytes",
                        operation_id,
                    );
                }
                Err(_) => {
                    return terminal_error(
                        store,
                        &audit,
                        now,
                        RECORD_VALIDATION_REASON,
                        operation_id,
                    );
                }
            }
            let was_ready = record.phase == Readiness::Ready;
            (record, was_ready, false)
        }
        None => {
            let stream = format!("agent-memory-{}", source.component());
            let coordinate = match store.choose_coordinate(&stream, now) {
                Ok(coordinate) => coordinate,
                Err(_) => {
                    return terminal_error(store, &audit, now, RECORD_READ_REASON, operation_id);
                }
            };
            (
                OperationRecord {
                    operation_id: operation_id.to_owned(),
                    digest: note_digest.clone(),
                    byte_count: note.len(),
                    created_at: now,
                    coordinate,
                    phase: Readiness::Reserved,
                    chain: None,
                },
                false,
                true,
            )
        }
    };

    if is_new {
        match save_record(store, &address, &record, true) {
            Publication::Confirmed => {}
            Publication::PublishedUnconfirmed => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
            Publication::NotPublished => {
                return terminal_error(
                    store,
                    &audit,
                    now,
                    "memory reservation could not be published",
                    operation_id,
                );
            }
        }
    }

    let observation = match store.observe_segment(&record.coordinate) {
        Ok(observation) => observation,
        Err(_) => {
            return Ok(AppendResult::UncertainRetry {
                operation_id: operation_id.to_owned(),
            });
        }
    };
    match observation {
        SegmentObservation::Tombstone => {
            let receipt = receipt(&record, &source);
            let result = result_shape_for(&receipt);
            return match write_terminal(store, &audit, now, Outcome::Deleted, None, Some(result)) {
                Publication::Confirmed => Ok(AppendResult::Deleted(receipt)),
                Publication::PublishedUnconfirmed | Publication::NotPublished => {
                    Ok(AppendResult::UncertainRetry {
                        operation_id: operation_id.to_owned(),
                    })
                }
            };
        }
        SegmentObservation::Staged => {
            return Ok(AppendResult::UncertainRetry {
                operation_id: operation_id.to_owned(),
            });
        }
        SegmentObservation::Missing if record.phase != Readiness::Reserved => {
            return terminal_error(store, &audit, now, RECORD_READ_REASON, operation_id);
        }
        SegmentObservation::Missing | SegmentObservation::Live => {}
    }

    let origin = origin_for(&record, &source);
    let origin_bytes = serde_json::to_vec(&origin)
        .map_err(|_| AppendError("memory origin could not be serialized"))?;
    let ready_bytes = ready_bytes(&record)?;

    if was_ready {
        if !file_equals(store, &record.coordinate, NOTE_FILE, note)?
            || !file_equals(store, &record.coordinate, ORIGIN_FILE, &origin_bytes)?
            || !file_equals(store, &record.coordinate, READY_FILE, &ready_bytes)?
        {
            return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
        }
        let receipt = receipt(&record, &source);
        let result = result_shape_for(&receipt);
        return match write_terminal(store, &audit, now, Outcome::Replayed, None, Some(result)) {
            Publication::Confirmed => Ok(AppendResult::Replayed(receipt)),
            Publication::PublishedUnconfirmed | Publication::NotPublished => {
                Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                })
            }
        };
    }

    if observation == SegmentObservation::Missing
        && store.create_segment(&record.coordinate).is_err()
    {
        return Ok(AppendResult::UncertainRetry {
            operation_id: operation_id.to_owned(),
        });
    }
    if store.bind_stream(&source, &record.coordinate).is_err() {
        return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
    }

    if record.phase == Readiness::Reserved {
        for (name, bytes) in [(NOTE_FILE, note), (ORIGIN_FILE, origin_bytes.as_slice())] {
            match store.publish_file(&record.coordinate, name, bytes) {
                FilePublication::Confirmed => {}
                FilePublication::Conflict => {
                    return terminal_error(
                        store,
                        &audit,
                        now,
                        RECORD_VALIDATION_REASON,
                        operation_id,
                    );
                }
                FilePublication::PublishedUnconfirmed | FilePublication::NotPublished => {
                    return Ok(AppendResult::UncertainRetry {
                        operation_id: operation_id.to_owned(),
                    });
                }
            }
        }
        record.phase = Readiness::Noted;
        match save_record(store, &address, &record, false) {
            Publication::Confirmed => {}
            Publication::PublishedUnconfirmed | Publication::NotPublished => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
        }
    } else if !file_equals(store, &record.coordinate, NOTE_FILE, note)?
        || !file_equals(store, &record.coordinate, ORIGIN_FILE, &origin_bytes)?
    {
        return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
    }

    if record.phase == Readiness::Noted {
        let advance = match store.advance_stream(&source, &record.coordinate) {
            Ok(advance) => advance,
            Err(_) => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
        };
        record.chain = Some(ChainPredecessor {
            prev_day: advance.prev_day,
            prev_segment: advance.prev_segment,
            seq: advance.seq,
        });
        record.phase = Readiness::Chained;
        match save_record(store, &address, &record, false) {
            Publication::Confirmed => {}
            Publication::PublishedUnconfirmed | Publication::NotPublished => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
        }
    }

    if record.phase == Readiness::Chained {
        match store.publish_file(&record.coordinate, READY_FILE, &ready_bytes) {
            FilePublication::Confirmed => {}
            FilePublication::Conflict => {
                if !file_equals(store, &record.coordinate, READY_FILE, &ready_bytes)? {
                    return terminal_error(
                        store,
                        &audit,
                        now,
                        RECORD_VALIDATION_REASON,
                        operation_id,
                    );
                }
            }
            FilePublication::PublishedUnconfirmed | FilePublication::NotPublished => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
        }
        record.phase = Readiness::Ready;
        match save_record(store, &address, &record, false) {
            Publication::Confirmed => {}
            Publication::PublishedUnconfirmed | Publication::NotPublished => {
                return Ok(AppendResult::UncertainRetry {
                    operation_id: operation_id.to_owned(),
                });
            }
        }
    }

    let receipt = receipt(&record, &source);
    let result = result_shape_for(&receipt);
    match write_terminal(store, &audit, now, Outcome::Stored, None, Some(result)) {
        Publication::Confirmed => Ok(AppendResult::Stored(receipt)),
        Publication::PublishedUnconfirmed | Publication::NotPublished => {
            Ok(AppendResult::UncertainRetry {
                operation_id: operation_id.to_owned(),
            })
        }
    }
}

fn save_record<S: AppendStore>(
    store: &mut S,
    address: &OperationAddress,
    record: &OperationRecord,
    create_only: bool,
) -> Publication {
    match serde_json::to_vec(record) {
        Ok(bytes) => store.save_operation(address, &bytes, create_only),
        Err(_) => Publication::NotPublished,
    }
}

fn file_equals<S: AppendStore>(
    store: &mut S,
    coordinate: &Coordinate,
    name: &str,
    expected: &[u8],
) -> Result<bool, AppendError> {
    store
        .read_file(coordinate, name)
        .map(|actual| actual.as_deref() == Some(expected))
        .map_err(|_| AppendError("memory file could not be read"))
}

fn terminal_error<S: AppendStore>(
    store: &mut S,
    audit: &AuditLocation,
    now: DateTime<Utc>,
    reason: &'static str,
    operation_id: &str,
) -> Result<AppendResult, AppendError> {
    match write_terminal(store, audit, now, Outcome::Error, Some(reason), None) {
        Publication::Confirmed => Err(AppendError(reason)),
        Publication::PublishedUnconfirmed | Publication::NotPublished => {
            Ok(AppendResult::UncertainRetry {
                operation_id: operation_id.to_owned(),
            })
        }
    }
}

fn write_terminal<S: AppendStore>(
    store: &mut S,
    audit: &AuditLocation,
    now: DateTime<Utc>,
    outcome: Outcome,
    reason: Option<&str>,
    result: Option<ResultShape>,
) -> Publication {
    store.write_outcome(audit, outcome, reason, result, now)
}

fn origin_for(record: &OperationRecord, source: &SourceKey) -> Origin {
    Origin {
        source_key: source.clone(),
        created_at: record.created_at,
        stream: record.coordinate.stream.clone(),
        segment: record.coordinate.segment.clone(),
    }
}

fn receipt(record: &OperationRecord, source: &SourceKey) -> AppendReceipt {
    AppendReceipt {
        origin: origin_for(record, source),
        created_at: record.created_at,
        digest: record.digest.clone(),
        byte_count: record.byte_count,
    }
}

fn result_shape_for(receipt: &AppendReceipt) -> ResultShape {
    let mut result = solstone_core_mcp_audit::result_shape(0, Vec::new(), receipt.digest.clone());
    result.origin = serde_json::to_value(&receipt.origin).ok();
    result.created_at = Some(receipt.created_at);
    result.byte_count = Some(receipt.byte_count);
    result
}

#[derive(Serialize)]
struct ReadyDocument<'a> {
    coordinate: &'a Coordinate,
    digest: &'a str,
    created_at: DateTime<Utc>,
    byte_count: usize,
}

fn ready_bytes(record: &OperationRecord) -> Result<Vec<u8>, AppendError> {
    serde_json::to_vec(&ReadyDocument {
        coordinate: &record.coordinate,
        digest: &record.digest,
        created_at: record.created_at,
        byte_count: record.byte_count,
    })
    .map_err(|_| AppendError("memory readiness could not be serialized"))
}

struct JournalStore<'a> {
    journal: &'a Path,
    source_lock: Option<solstone_core_journal_io::FileLock>,
}

impl<'a> JournalStore<'a> {
    fn new(journal: &'a Path) -> Self {
        Self {
            journal,
            source_lock: None,
        }
    }

    fn operation_path(&self, address: &OperationAddress) -> PathBuf {
        self.journal.join(address.relative())
    }

    fn segment_dir(&self, coordinate: &Coordinate) -> Result<SegmentDir, StoreFailure> {
        let exact = solstone_core_journal_io::resolve_segment_exact(
            self.journal,
            &coordinate.day,
            &coordinate.stream,
            &coordinate.segment,
        )
        .map_err(|_| StoreFailure("memory segment lookup failed"))?
        .ok_or(StoreFailure("memory segment is absent"))?;
        let segment = SegmentDir::resolve(
            self.journal,
            &coordinate.day,
            &coordinate.segment,
            &coordinate.stream,
        )
        .map_err(|_| StoreFailure("memory segment containment failed"))?;
        if exact != segment.path() {
            return Err(StoreFailure("memory segment containment disagrees"));
        }
        Ok(segment)
    }

    fn ensure_operation_parent(&self, address: &OperationAddress) -> Result<(), StoreFailure> {
        let mut relative = PathBuf::new();
        for component in ["config", "agent-memory", address.source_component.as_str()] {
            relative.push(component);
            let path = self.journal.join(&relative);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(_) => return Err(StoreFailure("memory operation path is not a directory")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir(&path)
                        .map_err(|_| StoreFailure("memory directory create failed"))?;
                    let parent = relative.parent().unwrap_or(Path::new(""));
                    sync_parent(self.journal, parent)?;
                }
                Err(_) => return Err(StoreFailure("memory directory could not be inspected")),
            }
        }
        let operation_relative = address.relative();
        let relative = operation_relative
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or_default();
        solstone_core_journal_io::contained_path(self.journal, relative)
            .map_err(|_| StoreFailure("memory operation path escaped the journal"))?;
        Ok(())
    }

    fn operation_record_bytes(
        &self,
        address: &OperationAddress,
    ) -> Result<Option<Vec<u8>>, StoreFailure> {
        let path = self.operation_path(address);
        let relative = address.relative();
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(StoreFailure("memory operation path could not be inspected")),
            Ok(metadata) if metadata.file_type().is_file() => {
                solstone_core_journal_io::contained_path(self.journal, &relative)
                    .map_err(|_| StoreFailure("memory operation path escaped the journal"))?;
                fs::read(path)
                    .map(Some)
                    .map_err(|_| StoreFailure("memory operation record could not be read"))
            }
            Ok(_) => Err(StoreFailure(
                "memory operation record is not a regular file",
            )),
        }
    }
}

impl AppendStore for JournalStore<'_> {
    fn hold_source(&mut self, source_component: &str) -> Result<(), StoreFailure> {
        let streams_preexisted = self.journal.join("streams").is_dir();
        self.source_lock = Some(
            hold_source_mutation(self.journal, source_component)
                .map_err(|_| StoreFailure("memory source lock failed"))?,
        );
        if !streams_preexisted {
            #[cfg(unix)]
            solstone_core_journal_io::sync_root(self.journal)
                .map_err(|_| StoreFailure("memory source parent sync failed"))?;
        } else {
            sync_dir(self.journal, "streams")
                .map_err(|_| StoreFailure("memory source parent sync failed"))?;
        }
        Ok(())
    }

    fn admit(
        &mut self,
        verified_id: &str,
        source: &SourceKey,
        operation_id: &str,
        note_digest: &str,
        byte_count: usize,
        now: DateTime<Utc>,
    ) -> Result<AuditLocation, AdmissionFailure> {
        let mut arguments = Map::new();
        arguments.insert(
            "operation_id".into(),
            Value::String(operation_id.to_owned()),
        );
        arguments.insert("digest".into(), Value::String(note_digest.to_owned()));
        arguments.insert("byte_count".into(), Value::from(byte_count));
        let admission = Admission {
            connection: verified_id,
            agent_identity: source.as_str(),
            tool_name: ToolName::SaveMemory,
            arguments,
            permission: None,
        };
        match write_interaction_record(self.journal, now, &admission) {
            Ok(coordinates) => Ok(AuditLocation {
                day: coordinates.day.format("%Y%m%d").to_string(),
                stream: coordinates.stream,
                segment: coordinates.segment,
            }),
            Err(AuditWriteError::PublishedUnconfirmed { .. }) => {
                Err(AdmissionFailure::PublishedUnconfirmed)
            }
            Err(_) => Err(AdmissionFailure::NotPublished),
        }
    }

    fn load_operation(
        &mut self,
        address: &OperationAddress,
    ) -> Result<Option<Vec<u8>>, StoreFailure> {
        self.operation_record_bytes(address)
    }

    fn choose_coordinate(
        &mut self,
        stream: &str,
        now: DateTime<Utc>,
    ) -> Result<Coordinate, StoreFailure> {
        let day = now.format("%Y%m%d").to_string();
        let candidate = format!("{}_1", now.format("%H%M%S"));
        let parent = self.journal.join("chronicle").join(&day).join(stream);
        let segment = find_available_segment(&parent, &candidate, MAX_SEGMENT_ATTEMPTS)
            .map_err(|_| StoreFailure("memory coordinate allocation failed"))?
            .ok_or(StoreFailure("memory coordinate allocation exhausted"))?;
        Ok(Coordinate {
            day,
            stream: stream.to_owned(),
            segment,
        })
    }

    fn save_operation(
        &mut self,
        address: &OperationAddress,
        bytes: &[u8],
        create_only: bool,
    ) -> Publication {
        if self.ensure_operation_parent(address).is_err() {
            return Publication::NotPublished;
        }
        let path = self.operation_path(address);
        if create_only {
            return match write_bytes_exclusive_detailed(&path, bytes, AtomicWriteOptions::default())
            {
                Ok(publication) if exclusive_confirmed(&publication) => Publication::Confirmed,
                Ok(_) => Publication::PublishedUnconfirmed,
                Err(_) => Publication::NotPublished,
            };
        }
        match atomic_replace_detailed(&path, bytes, 0o600) {
            Ok(DetailedAtomicOutcome::Published) => Publication::Confirmed,
            Ok(_) => Publication::PublishedUnconfirmed,
            Err(_) => Publication::NotPublished,
        }
    }

    fn observe_segment(
        &mut self,
        coordinate: &Coordinate,
    ) -> Result<SegmentObservation, StoreFailure> {
        let relative = segment_rel(&coordinate.day, &coordinate.stream, &coordinate.segment);
        let parent_rel = relative
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .ok_or(StoreFailure("memory segment parent is invalid"))?;
        let parent = self.journal.join(parent_rel);
        if !path_lexists(&parent)
            .map_err(|_| StoreFailure("memory segment parent lookup failed"))?
        {
            return Ok(SegmentObservation::Missing);
        }
        solstone_core_journal_io::contained_path(self.journal, parent_rel)
            .map_err(|_| StoreFailure("memory segment parent escaped the journal"))?;
        let live = self.journal.join(&relative);
        let _live_name_lock = solstone_core_journal_io::hold_lock(
            &live,
            solstone_core_journal_io::LockOptions::default(),
        )
        .map_err(|_| StoreFailure("memory live-name lock failed"))?;
        let staged = parent.join(staged_name(&coordinate.segment));
        if path_lexists(&staged).map_err(|_| StoreFailure("memory staged path lookup failed"))? {
            return Ok(SegmentObservation::Staged);
        }
        match fs::symlink_metadata(&live) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SegmentObservation::Missing)
            }
            Err(_) => Err(StoreFailure("memory segment lookup failed")),
            Ok(metadata) if metadata.file_type().is_dir() => {
                if path_lexists(&live.join(TOMBSTONE_NAME))
                    .map_err(|_| StoreFailure("memory tombstone lookup failed"))?
                {
                    Ok(SegmentObservation::Tombstone)
                } else {
                    Ok(SegmentObservation::Live)
                }
            }
            Ok(_) => Err(StoreFailure("memory segment is not a directory")),
        }
    }

    fn create_segment(&mut self, coordinate: &Coordinate) -> Result<(), StoreFailure> {
        solstone_core_journal_io::create_segment_strict(
            self.journal,
            &coordinate.day,
            &coordinate.stream,
            &coordinate.segment,
        )
        .map_err(|_| StoreFailure("memory segment create failed"))?;
        sync_chronicle_chain(self.journal, coordinate)?;
        self.segment_dir(coordinate).map(|_| ())
    }

    fn bind_stream(
        &mut self,
        source: &SourceKey,
        coordinate: &Coordinate,
    ) -> Result<(), StoreFailure> {
        bind_agent_memory_stream(
            self.journal,
            &coordinate.day,
            &coordinate.segment,
            &coordinate.stream,
            source.as_str(),
            source.component(),
        )
        .map(|_| ())
        .map_err(|_| StoreFailure("memory stream binding failed"))
    }

    fn read_file(
        &mut self,
        coordinate: &Coordinate,
        name: &str,
    ) -> Result<Option<Vec<u8>>, StoreFailure> {
        let segment = self.segment_dir(coordinate)?;
        let path = segment.path().join(name);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(StoreFailure("memory file could not be inspected")),
            Ok(metadata) if metadata.file_type().is_file() => fs::read(path)
                .map(Some)
                .map_err(|_| StoreFailure("memory file could not be read")),
            Ok(_) => Err(StoreFailure("memory file is not regular")),
        }
    }

    fn publish_file(
        &mut self,
        coordinate: &Coordinate,
        name: &str,
        bytes: &[u8],
    ) -> FilePublication {
        let segment = match self.segment_dir(coordinate) {
            Ok(segment) => segment,
            Err(_) => return FilePublication::NotPublished,
        };
        let path = segment.path().join(name);
        match write_bytes_exclusive_detailed(&path, bytes, AtomicWriteOptions::default()) {
            Ok(publication) if exclusive_confirmed(&publication) => FilePublication::Confirmed,
            Ok(_) => FilePublication::PublishedUnconfirmed,
            Err(error) if error.source.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.read_file(coordinate, name) {
                    Ok(Some(existing)) if existing == bytes => {
                        match atomic_replace_detailed(&path, bytes, 0o600) {
                            Ok(DetailedAtomicOutcome::Published) => FilePublication::Confirmed,
                            Ok(_) => FilePublication::PublishedUnconfirmed,
                            Err(_) => FilePublication::NotPublished,
                        }
                    }
                    Ok(Some(_)) => FilePublication::Conflict,
                    Ok(None) | Err(_) => FilePublication::NotPublished,
                }
            }
            Err(_) => FilePublication::NotPublished,
        }
    }

    fn advance_stream(
        &mut self,
        source: &SourceKey,
        coordinate: &Coordinate,
    ) -> Result<StreamAdvance, StoreFailure> {
        let segment = self.segment_dir(coordinate)?;
        advance_agent_memory_stream(
            &coordinate.stream,
            &coordinate.day,
            &coordinate.segment,
            &segment,
            source.as_str(),
            source.component(),
        )
        .map_err(|_| StoreFailure("memory stream advance was not confirmed"))
    }

    fn write_outcome(
        &mut self,
        location: &AuditLocation,
        outcome: Outcome,
        reason: Option<&str>,
        result: Option<ResultShape>,
        now: DateTime<Utc>,
    ) -> Publication {
        let Ok(day) = NaiveDate::parse_from_str(&location.day, "%Y%m%d") else {
            return Publication::NotPublished;
        };
        let coordinates = AuditCoordinates {
            day,
            stream: location.stream.clone(),
            segment: location.segment.clone(),
        };
        match write_outcome_record(self.journal, &coordinates, now, outcome, reason, result) {
            Ok(()) => Publication::Confirmed,
            Err(AuditWriteError::PublishedUnconfirmed { .. }) => Publication::PublishedUnconfirmed,
            Err(_) => Publication::NotPublished,
        }
    }
}

fn exclusive_confirmed(publication: &ExclusivePublication) -> bool {
    let name = matches!(
        publication.final_name,
        FinalNameConfirmation::Confirmed { .. }
    );
    let cleanup = matches!(publication.cleanup, StageCleanup::Removed);
    #[cfg(unix)]
    let durability = matches!(publication.durability, MetadataDurability::Confirmed);
    #[cfg(windows)]
    let durability = matches!(
        publication.durability,
        MetadataDurability::Unproven { source: None }
    );
    name && cleanup && durability
}

fn sync_parent(journal: &Path, parent: &Path) -> Result<(), StoreFailure> {
    if parent.as_os_str().is_empty() {
        #[cfg(unix)]
        solstone_core_journal_io::sync_root(journal)
            .map_err(|_| StoreFailure("memory parent sync failed"))?;
        return Ok(());
    }
    let relative = parent.to_string_lossy().replace('\\', "/");
    sync_dir(journal, &relative).map_err(|_| StoreFailure("memory parent sync failed"))
}

fn sync_chronicle_chain(journal: &Path, coordinate: &Coordinate) -> Result<(), StoreFailure> {
    #[cfg(unix)]
    solstone_core_journal_io::sync_root(journal)
        .map_err(|_| StoreFailure("memory parent sync failed"))?;
    for relative in [
        "chronicle".to_owned(),
        format!("chronicle/{}", coordinate.day),
        format!("chronicle/{}/{}", coordinate.day, coordinate.stream),
        segment_rel(&coordinate.day, &coordinate.stream, &coordinate.segment),
    ] {
        sync_dir(journal, &relative).map_err(|_| StoreFailure("memory parent sync failed"))?;
    }
    Ok(())
}

#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn append_connection_memory_test_hook(
    journal: &Path,
    verified_id: &str,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    append_connection_memory(journal, verified_id, text, operation_id, now)
        .map(|result| matches!(result, AppendResult::Stored(_) | AppendResult::Replayed(_)))
        .map_err(|error| error.to_string())
}

#[cfg(all(test, not(feature = "full-tests")))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
mod tests {
    use std::collections::{HashMap, VecDeque};

    use chrono::{DateTime, TimeZone, Utc};
    use solstone_core_format::agent_memory::{
        Coordinate, OperationRecord, Readiness, SourceKey, digest,
    };
    use solstone_core_mcp_audit::{Outcome, ResultShape};

    use super::{
        AdmissionFailure, AppendResult, AppendStore, AuditLocation, FilePublication,
        OperationAddress, Publication, SegmentObservation, StoreFailure, append_with,
    };

    #[derive(Default)]
    struct MemoryStore {
        records: HashMap<String, Vec<u8>>,
        files: HashMap<(String, String), Vec<u8>>,
        observations: HashMap<String, SegmentObservation>,
        events: Vec<String>,
        admissions: Vec<(String, String, usize)>,
        admission: Option<Publication>,
        record_publications: VecDeque<Publication>,
        file_publications: VecDeque<FilePublication>,
        outcome_publications: VecDeque<Publication>,
        outcomes: Vec<Outcome>,
        coordinate_number: usize,
        sequences: HashMap<String, u64>,
    }

    impl MemoryStore {
        fn status(queue: &mut VecDeque<Publication>) -> Publication {
            queue.pop_front().unwrap_or(Publication::Confirmed)
        }

        fn coordinate_key(coordinate: &Coordinate) -> String {
            format!(
                "{}/{}/{}",
                coordinate.day, coordinate.stream, coordinate.segment
            )
        }

        fn file_status(&mut self) -> FilePublication {
            self.file_publications
                .pop_front()
                .unwrap_or(FilePublication::Confirmed)
        }
    }

    impl AppendStore for MemoryStore {
        fn hold_source(&mut self, _: &str) -> Result<(), StoreFailure> {
            self.events.push("lock".into());
            Ok(())
        }

        fn admit(
            &mut self,
            verified_id: &str,
            _: &SourceKey,
            operation_id: &str,
            note_digest: &str,
            byte_count: usize,
            _: DateTime<Utc>,
        ) -> Result<AuditLocation, AdmissionFailure> {
            self.events.push("admission".into());
            self.admissions
                .push((verified_id.into(), operation_id.into(), byte_count));
            match self.admission.take().unwrap_or(Publication::Confirmed) {
                Publication::Confirmed => {
                    assert_eq!(note_digest.len(), 64);
                    Ok(AuditLocation {
                        day: "20260102".into(),
                        stream: "mcp.agent".into(),
                        segment: format!("120000_{}", self.admissions.len()),
                    })
                }
                Publication::PublishedUnconfirmed => Err(AdmissionFailure::PublishedUnconfirmed),
                Publication::NotPublished => Err(AdmissionFailure::NotPublished),
            }
        }

        fn load_operation(
            &mut self,
            address: &OperationAddress,
        ) -> Result<Option<Vec<u8>>, StoreFailure> {
            self.events.push("load".into());
            Ok(self.records.get(&address.relative()).cloned())
        }

        fn choose_coordinate(
            &mut self,
            stream: &str,
            now: DateTime<Utc>,
        ) -> Result<Coordinate, StoreFailure> {
            self.coordinate_number += 1;
            Ok(Coordinate {
                day: now.format("%Y%m%d").to_string(),
                stream: stream.into(),
                segment: format!("{}_{}", now.format("%H%M%S"), self.coordinate_number),
            })
        }

        fn save_operation(
            &mut self,
            address: &OperationAddress,
            bytes: &[u8],
            _: bool,
        ) -> Publication {
            self.events.push("operation".into());
            let status = Self::status(&mut self.record_publications);
            if status != Publication::NotPublished {
                self.records.insert(address.relative(), bytes.to_vec());
            }
            status
        }

        fn observe_segment(
            &mut self,
            coordinate: &Coordinate,
        ) -> Result<SegmentObservation, StoreFailure> {
            self.events.push("observe".into());
            Ok(self
                .observations
                .get(&Self::coordinate_key(coordinate))
                .copied()
                .unwrap_or(SegmentObservation::Missing))
        }

        fn create_segment(&mut self, coordinate: &Coordinate) -> Result<(), StoreFailure> {
            self.events.push("segment".into());
            self.observations
                .insert(Self::coordinate_key(coordinate), SegmentObservation::Live);
            Ok(())
        }

        fn bind_stream(&mut self, _: &SourceKey, _: &Coordinate) -> Result<(), StoreFailure> {
            self.events.push("bind".into());
            Ok(())
        }

        fn read_file(
            &mut self,
            coordinate: &Coordinate,
            name: &str,
        ) -> Result<Option<Vec<u8>>, StoreFailure> {
            Ok(self
                .files
                .get(&(Self::coordinate_key(coordinate), name.to_owned()))
                .cloned())
        }

        fn publish_file(
            &mut self,
            coordinate: &Coordinate,
            name: &str,
            bytes: &[u8],
        ) -> FilePublication {
            self.events.push(name.into());
            let status = self.file_status();
            if status == FilePublication::Confirmed
                || status == FilePublication::PublishedUnconfirmed
            {
                self.files.insert(
                    (Self::coordinate_key(coordinate), name.into()),
                    bytes.to_vec(),
                );
            }
            status
        }

        fn advance_stream(
            &mut self,
            _: &SourceKey,
            coordinate: &Coordinate,
        ) -> Result<solstone_core_segment::StreamAdvance, StoreFailure> {
            self.events.push("advance".into());
            let key = coordinate.stream.clone();
            let sequence = self.sequences.entry(key).or_default();
            *sequence += 1;
            Ok(solstone_core_segment::StreamAdvance {
                prev_day: None,
                prev_segment: None,
                seq: *sequence,
            })
        }

        fn write_outcome(
            &mut self,
            _: &AuditLocation,
            outcome: Outcome,
            reason: Option<&str>,
            _: Option<ResultShape>,
            _: DateTime<Utc>,
        ) -> Publication {
            self.events.push("outcome".into());
            if let Some(reason) = reason {
                for forbidden in [
                    "served",
                    "empty",
                    "refused",
                    "error",
                    "uncertain",
                    "stored",
                    "replayed",
                    "deleted",
                ] {
                    assert!(!reason.contains(forbidden));
                }
            }
            self.outcomes.push(outcome);
            Self::status(&mut self.outcome_publications)
        }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap()
    }

    #[test]
    fn rejects_empty_and_oversize_before_any_store_action_and_accepts_limit() {
        for text in ["", &"x".repeat(32_769)] {
            let mut store = MemoryStore::default();
            assert!(append_with(&mut store, "bearer:x", text, "op", now()).is_err());
            assert!(store.events.is_empty());
        }
        let mut store = MemoryStore::default();
        assert!(matches!(
            append_with(&mut store, "bearer:x", &"x".repeat(32_768), "op", now()),
            Ok(AppendResult::Stored(_))
        ));
    }

    #[test]
    fn exact_text_bytes_round_trip_and_operation_ids_are_case_sensitive_and_bounded() {
        for text in ["a\r\nb", "é🪨", " note ", "   "] {
            let mut store = MemoryStore::default();
            assert!(matches!(
                append_with(&mut store, "bearer:x", text, "A:z-._", now()),
                Ok(AppendResult::Stored(_))
            ));
            assert!(store.files.values().any(|bytes| bytes == text.as_bytes()));
        }
        for id in ["A", &"x".repeat(128)] {
            let mut store = MemoryStore::default();
            assert!(append_with(&mut store, "bearer:x", "n", id, now()).is_ok());
        }
        for id in ["", &"x".repeat(129), "a b", "a/b"] {
            let mut store = MemoryStore::default();
            assert!(append_with(&mut store, "bearer:x", "n", id, now()).is_err());
            assert!(store.events.is_empty());
        }
        let mut case_sensitive = MemoryStore::default();
        assert!(matches!(
            append_with(&mut case_sensitive, "bearer:x", "n", "A", now()),
            Ok(AppendResult::Stored(_))
        ));
        assert!(matches!(
            append_with(&mut case_sensitive, "bearer:x", "n", "a", now()),
            Ok(AppendResult::Stored(_))
        ));
    }

    #[test]
    fn replay_preserves_coordinate_created_time_digest_and_does_not_write_another_note() {
        let mut store = MemoryStore::default();
        let AppendResult::Stored(first) =
            append_with(&mut store, "bearer:opaque", "same", "op", now()).unwrap()
        else {
            panic!("first append");
        };
        let AppendResult::Replayed(second) = append_with(
            &mut store,
            "bearer:opaque",
            "same",
            "op",
            now() + chrono::Duration::days(1),
        )
        .unwrap() else {
            panic!("retry append");
        };
        assert_eq!(first, second);
        assert_eq!(first.digest, digest(b"same"));
        assert!(!format!("{:?}", first.origin).contains("bearer:"));
        assert_eq!(
            store
                .files
                .values()
                .filter(|bytes| *bytes == b"same")
                .count(),
            1
        );
    }

    #[test]
    fn removed_operation_record_is_not_reported_as_replayed_or_deleted() {
        let mut store = MemoryStore::default();
        let AppendResult::Stored(first) =
            append_with(&mut store, "bearer:rollback", "note", "op", now()).unwrap()
        else {
            panic!("first append");
        };

        let source = SourceKey::from_verified_id("bearer:rollback");
        let address = OperationAddress::new(&source, "op");
        assert!(store.records.remove(&address.relative()).is_some());

        let AppendResult::Stored(second) =
            append_with(&mut store, "bearer:rollback", "note", "op", now()).unwrap()
        else {
            panic!("append after rollback");
        };
        assert_ne!(first.origin.segment, second.origin.segment);
        assert_eq!(store.coordinate_number, 2);
    }

    #[test]
    fn admission_precedes_reservation_and_note_publication() {
        let mut store = MemoryStore::default();
        assert!(matches!(
            append_with(&mut store, "bearer:x", "n", "op", now()),
            Ok(AppendResult::Stored(_))
        ));
        let admission = store
            .events
            .iter()
            .position(|item| item == "admission")
            .unwrap();
        let operation = store
            .events
            .iter()
            .position(|item| item == "operation")
            .unwrap();
        let note = store
            .events
            .iter()
            .position(|item| item == "note.txt")
            .unwrap();
        assert!(admission < operation && operation < note);
    }

    #[test]
    fn different_bytes_conflict_and_another_verified_identity_is_independent() {
        let mut store = MemoryStore::default();
        assert!(matches!(
            append_with(&mut store, "bearer:one", "a", "op", now()),
            Ok(AppendResult::Stored(_))
        ));
        assert!(append_with(&mut store, "bearer:one", "b", "op", now()).is_err());
        assert_eq!(store.coordinate_number, 1);
        assert_eq!(store.outcomes.last(), Some(&Outcome::Error));
        assert!(matches!(
            append_with(&mut store, "bearer:two", "b", "op", now()),
            Ok(AppendResult::Stored(_))
        ));
        assert_eq!(store.admissions[0].0, "bearer:one");
        assert_eq!(store.admissions[2].0, "bearer:two");
    }

    #[test]
    fn source_key_tracks_verified_id_and_origin_keeps_only_the_key() {
        let bearer = SourceKey::from_verified_id("bearer:durable");
        assert_eq!(bearer, SourceKey::from_verified_id("bearer:durable"));
        assert_ne!(bearer, SourceKey::from_verified_id("bearer:new"));
        assert_ne!(bearer, SourceKey::from_verified_id("oauth:new-grant"));
        let mut store = MemoryStore::default();
        let AppendResult::Stored(receipt) =
            append_with(&mut store, "oauth:grant-id", "note", "op", now()).unwrap()
        else {
            panic!("append");
        };
        let origin = serde_json::to_string(&receipt.origin).unwrap();
        assert!(!origin.contains("bearer:"));
        assert!(!origin.contains("oauth:"));
        let operation = store.records.values().next().unwrap();
        let operation = String::from_utf8_lossy(operation);
        assert!(!operation.contains("bearer:"));
        assert!(!operation.contains("oauth:"));
    }

    #[test]
    fn admission_and_reservation_publication_failures_do_not_claim_storage() {
        let mut admission_missing = MemoryStore {
            admission: Some(Publication::NotPublished),
            ..MemoryStore::default()
        };
        assert!(append_with(&mut admission_missing, "bearer:x", "n", "op", now()).is_err());
        assert!(admission_missing.records.is_empty());

        let mut admission_unclear = MemoryStore {
            admission: Some(Publication::PublishedUnconfirmed),
            ..MemoryStore::default()
        };
        assert!(matches!(
            append_with(&mut admission_unclear, "bearer:x", "n", "op", now()),
            Ok(AppendResult::UncertainRetry { .. })
        ));
        assert!(admission_unclear.records.is_empty());

        let mut reservation_unclear = MemoryStore {
            record_publications: VecDeque::from([Publication::PublishedUnconfirmed]),
            ..MemoryStore::default()
        };
        assert!(matches!(
            append_with(&mut reservation_unclear, "bearer:x", "n", "op", now()),
            Ok(AppendResult::UncertainRetry { .. })
        ));
        assert_eq!(reservation_unclear.records.len(), 1);

        let mut reservation_absent = MemoryStore {
            record_publications: VecDeque::from([Publication::NotPublished]),
            ..MemoryStore::default()
        };
        assert!(append_with(&mut reservation_absent, "bearer:x", "n", "op", now()).is_err());
        assert!(reservation_absent.records.is_empty());
    }

    #[test]
    fn uncertain_file_and_terminal_publications_never_return_stored() {
        let mut note_unclear = MemoryStore {
            file_publications: VecDeque::from([FilePublication::PublishedUnconfirmed]),
            ..MemoryStore::default()
        };
        assert!(matches!(
            append_with(&mut note_unclear, "bearer:x", "n", "op", now()),
            Ok(AppendResult::UncertainRetry { .. })
        ));
        assert!(note_unclear.files.values().any(|bytes| bytes == b"n"));

        let mut outcome_absent = MemoryStore {
            outcome_publications: VecDeque::from([Publication::NotPublished]),
            ..MemoryStore::default()
        };
        assert!(matches!(
            append_with(&mut outcome_absent, "bearer:x", "n", "op", now()),
            Ok(AppendResult::UncertainRetry { .. })
        ));
    }

    #[test]
    fn tombstone_is_consumed_and_staging_returns_uncertain_retry() {
        let mut deleted = MemoryStore::default();
        let AppendResult::Stored(receipt) =
            append_with(&mut deleted, "bearer:x", "n", "op", now()).unwrap()
        else {
            panic!("first append");
        };
        let source = SourceKey::from_verified_id("bearer:x");
        let address = OperationAddress::new(&source, "op");
        let mut record: OperationRecord =
            serde_json::from_slice(deleted.records.get(&address.relative()).unwrap()).unwrap();
        let key = MemoryStore::coordinate_key(&record.coordinate);
        deleted
            .observations
            .insert(key.clone(), SegmentObservation::Tombstone);
        assert!(matches!(
            append_with(&mut deleted, "bearer:x", "n", "op", now()).unwrap(),
            AppendResult::Deleted(_)
        ));
        assert_eq!(deleted.outcomes.last(), Some(&Outcome::Deleted));
        assert_eq!(
            deleted
                .files
                .values()
                .filter(|bytes| *bytes == b"n")
                .count(),
            1
        );
        assert_eq!(receipt.digest, digest(b"n"));

        record.phase = Readiness::Ready;
        let mut staged = MemoryStore::default();
        staged
            .records
            .insert(address.relative(), serde_json::to_vec(&record).unwrap());
        staged.observations.insert(key, SegmentObservation::Staged);
        assert!(matches!(
            append_with(&mut staged, "bearer:x", "n", "op", now()).unwrap(),
            AppendResult::UncertainRetry { .. }
        ));
        assert!(staged.outcomes.is_empty());
    }

    #[test]
    fn reserved_a_keeps_its_coordinate_until_its_later_chain_advance() {
        let mut store = MemoryStore {
            file_publications: VecDeque::from([FilePublication::PublishedUnconfirmed]),
            ..MemoryStore::default()
        };
        assert!(matches!(
            append_with(&mut store, "bearer:x", "a", "A", now()),
            Ok(AppendResult::UncertainRetry { .. })
        ));
        let source = SourceKey::from_verified_id("bearer:x");
        let key_a = OperationAddress::new(&source, "A").relative();
        let first_a: OperationRecord =
            serde_json::from_slice(store.records.get(&key_a).unwrap()).unwrap();
        assert_eq!(first_a.phase, Readiness::Reserved);
        assert!(first_a.chain.is_none());

        assert!(matches!(
            append_with(&mut store, "bearer:x", "b", "B", now()),
            Ok(AppendResult::Stored(_))
        ));
        let AppendResult::Stored(a_final) =
            append_with(&mut store, "bearer:x", "a", "A", now()).unwrap()
        else {
            panic!("A completes");
        };
        let final_a: OperationRecord =
            serde_json::from_slice(store.records.get(&key_a).unwrap()).unwrap();
        assert_eq!(first_a.coordinate, final_a.coordinate);
        assert_eq!(first_a.created_at, final_a.created_at);
        assert!(final_a.chain.unwrap().seq > 1);
        assert_eq!(a_final.origin.segment, first_a.coordinate.segment);
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod full_tests {
    use std::fs;
    use std::process::Command;

    use chrono::{TimeZone, Utc};
    use solstone_core_format::agent_memory::{
        Coordinate, OperationRecord, Readiness, SourceKey, digest,
    };

    use super::{AppendResult, OperationAddress, append_connection_memory};

    fn fixture() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-memory-full-")
            .tempdir_in("/var/tmp")
            .expect("journal fixture")
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap()
    }

    #[test]
    fn corrupt_coordinate_does_not_publish_outside_its_source() {
        let journal = fixture();
        let outside = fixture();
        let source = SourceKey::from_verified_id("bearer:corrupt-coordinate");
        let address = OperationAddress::new(&source, "op");
        let relative = address.relative();
        let operation_path = journal.path().join(&relative);
        fs::create_dir_all(operation_path.parent().unwrap()).unwrap();
        let record = OperationRecord {
            operation_id: "op".into(),
            digest: digest(b"note"),
            byte_count: 4,
            created_at: now(),
            coordinate: Coordinate {
                day: "../escape".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "120000_1".into(),
            },
            phase: Readiness::Reserved,
            chain: None,
        };
        fs::write(&operation_path, serde_json::to_vec(&record).unwrap()).unwrap();

        assert!(
            append_connection_memory(
                journal.path(),
                "bearer:corrupt-coordinate",
                "note",
                "op",
                now()
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        let coordinate_path = journal
            .path()
            .join("chronicle")
            .join("../escape")
            .join(format!("agent-memory-{}", source.component()))
            .join("120000_1");
        assert!(!coordinate_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn source_directory_symlink_cannot_publish_outside_the_journal() {
        use std::os::unix::fs::symlink;

        let journal = fixture();
        let outside = fixture();
        let source = SourceKey::from_verified_id("bearer:symlink");
        fs::create_dir_all(journal.path().join("config/agent-memory")).unwrap();
        symlink(
            outside.path(),
            journal
                .path()
                .join("config/agent-memory")
                .join(source.component()),
        )
        .unwrap();

        assert!(
            append_connection_memory(journal.path(), "bearer:symlink", "note", "op", now())
                .is_err()
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn same_operation_is_serialized_across_processes() {
        let root = std::env::var_os("SOLSTONE_MEMORY_CHILD_ROOT");
        if let Some(root) = root {
            let root = std::path::PathBuf::from(root);
            let result =
                append_connection_memory(&root, "bearer:multi-process", "note", "same-op", now());
            assert!(matches!(
                result,
                Ok(AppendResult::Stored(_) | AppendResult::Replayed(_))
            ));
            return;
        }

        let journal = fixture();
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for _ in 0..2 {
            children.push(
                Command::new(&executable)
                    .args([
                        "--exact",
                        "memory::full_tests::same_operation_is_serialized_across_processes",
                    ])
                    .env("SOLSTONE_MEMORY_CHILD_ROOT", journal.path())
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }

        let source = SourceKey::from_verified_id("bearer:multi-process");
        let record_path = journal
            .path()
            .join(OperationAddress::new(&source, "same-op").relative());
        let record: OperationRecord =
            serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
        let note = journal
            .path()
            .join("chronicle")
            .join(record.coordinate.day)
            .join(record.coordinate.stream)
            .join(record.coordinate.segment)
            .join("note.txt");
        assert_eq!(fs::read(note).unwrap(), b"note");
    }
}
