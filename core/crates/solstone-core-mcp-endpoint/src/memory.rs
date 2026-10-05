// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private connection-owned memory append state machine.

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Map, Value};
use solstone_core_format::agent_memory::{
    ChainPredecessor, Coordinate, MAX_METADATA_BYTES, MAX_NOTE_BYTES, OperationRecord, Origin,
    OriginKind, Readiness, ReadyDocument, SourceKey, digest, ready_document,
    validate_creation_label, validate_operation_id, validate_ready_document, validate_record,
    validate_record_header,
};
use solstone_core_journal_io::atomic::{
    DetailedAtomicOutcome, ExclusivePublication, FinalNameConfirmation, MetadataDurability,
    StageCleanup, atomic_replace_detailed, write_bytes_exclusive_detailed,
};
use solstone_core_journal_io::{
    AtomicWriteOptions, find_available_segment_with_occupied, path_lexists, sync_dir,
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
    hold_agent_memory_mutation, read_agent_memory_chain,
};

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
    pub coordinate: Coordinate,
    pub origin: Origin,
    pub created_at: DateTime<Utc>,
    pub digest: String,
    pub byte_count: usize,
}

/// Values supplied by the authenticated transport after it has verified a
/// connection. This private input cannot grant owner-read append authority.
pub(crate) struct AuthenticatedMemorySource<'a> {
    pub verified_id: &'a str,
    pub creation_label: &'a str,
}

/// Storage preparation for the wire. Its success is provisional until the
/// caller publishes the final receipt's terminal audit record.
pub(crate) struct PreparedAppend {
    pub result: AppendResult,
    pub audit: Option<AuditCoordinates>,
    pub outcome: Option<Outcome>,
}

pub(crate) fn prepare_connection_memory(
    journal: &Path,
    source: AuthenticatedMemorySource<'_>,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<PreparedAppend, AppendError> {
    let mut store = JournalStore::new(journal);
    store.defer_success = true;
    let result = append_with_source(&mut store, source, text, operation_id, now)?;
    let prepared = PreparedAppend {
        result,
        audit: store.admission.take(),
        outcome: store.pending_outcome.take(),
    };
    // All storage guards drop before the wire examines the index or asks
    // ordinary read authorities to prepare the receipt.
    drop(store);
    Ok(prepared)
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

impl AppendError {
    pub(crate) fn wire_reason(&self) -> &'static str {
        if self.0 == "operation identifier is bound to different bytes" {
            "memory_operation_conflict"
        } else {
            "memory_save_unavailable"
        }
    }
}

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
    fn read_chain(
        &mut self,
        coordinate: &Coordinate,
    ) -> Result<Option<StreamAdvance>, StoreFailure>;
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
    reason = "storage-only adapter retained for native publication and recovery proofs"
)]
pub(crate) fn append_connection_memory(
    journal: &Path,
    source: AuthenticatedMemorySource<'_>,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<AppendResult, AppendError> {
    append_with_source(
        &mut JournalStore::new(journal),
        source,
        text,
        operation_id,
        now,
    )
}

#[cfg(all(test, not(feature = "full-tests")))]
fn append_with<S: AppendStore>(
    store: &mut S,
    verified_id: &str,
    text: &str,
    operation_id: &str,
    now: DateTime<Utc>,
) -> Result<AppendResult, AppendError> {
    append_with_source(
        store,
        AuthenticatedMemorySource {
            verified_id,
            creation_label: "authenticated connection",
        },
        text,
        operation_id,
        now,
    )
}

fn append_with_source<S: AppendStore>(
    store: &mut S,
    source_input: AuthenticatedMemorySource<'_>,
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
    validate_creation_label(source_input.creation_label)
        .map_err(|_| AppendError("invalid memory creation label"))?;

    let source = SourceKey::from_verified_id(source_input.verified_id);
    let address = OperationAddress::new(&source, operation_id);
    let note_digest = digest(note);
    store
        .hold_source(source.component())
        .map_err(|_| AppendError("memory source lock could not be acquired"))?;

    let audit = match store.admit(
        source_input.verified_id,
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
                    origin_kind: OriginKind::AgentMemory,
                    creation_label: source_input.creation_label.to_owned(),
                    coordinate,
                    phase: Readiness::Reserved,
                    chain: None,
                },
                false,
                true,
            )
        }
    };

    // Reconfirm a recovered reservation and its ancestry before source mutation.
    // Visibility after an interrupted publication is not durability proof.
    {
        match save_record(store, &address, &record, is_new) {
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

    if was_ready {
        let complete = file_equals(store, &record.coordinate, NOTE_FILE, note)
            .and_then(|note_matches| {
                Ok(note_matches
                    && file_equals(store, &record.coordinate, ORIGIN_FILE, &origin_bytes)?)
            })
            .and_then(|source_matches| {
                Ok(source_matches
                    && ready_matches(store, &record.coordinate, &record, &source, note)?)
            });
        if !matches!(complete, Ok(true)) {
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
    } else if !matches!(
        file_equals(store, &record.coordinate, NOTE_FILE, note),
        Ok(true)
    ) || !matches!(
        file_equals(store, &record.coordinate, ORIGIN_FILE, &origin_bytes),
        Ok(true)
    ) {
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
        if validate_record(&record, &source, operation_id, note).is_err() {
            return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
        }
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
        let actual_chain = store.read_chain(&record.coordinate);
        let chain_matches = matches!((actual_chain, record.chain.as_ref()), (Ok(Some(actual)), Some(expected))
            if actual.seq == expected.seq && actual.prev_day == expected.prev_day && actual.prev_segment == expected.prev_segment);
        if !chain_matches {
            return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
        }
        let ready_bytes = ready_bytes(&record, &source)?;
        match store.publish_file(&record.coordinate, READY_FILE, &ready_bytes) {
            FilePublication::Confirmed => {}
            FilePublication::Conflict => {
                return terminal_error(store, &audit, now, RECORD_VALIDATION_REASON, operation_id);
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
        kind: record.origin_kind,
        source_key: source.clone(),
        creation_label: record.creation_label.clone(),
        created_at: record.created_at,
        stream: record.coordinate.stream.clone(),
        segment: record.coordinate.segment.clone(),
    }
}

fn receipt(record: &OperationRecord, source: &SourceKey) -> AppendReceipt {
    AppendReceipt {
        coordinate: record.coordinate.clone(),
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

fn ready_bytes(record: &OperationRecord, source: &SourceKey) -> Result<Vec<u8>, AppendError> {
    let document = ready_document(record, source)
        .map_err(|_| AppendError("memory readiness could not be serialized"))?;
    serde_json::to_vec(&document)
        .map_err(|_| AppendError("memory readiness could not be serialized"))
}

fn ready_matches<S: AppendStore>(
    store: &mut S,
    coordinate: &Coordinate,
    record: &OperationRecord,
    source: &SourceKey,
    note: &[u8],
) -> Result<bool, AppendError> {
    let Some(bytes) = store
        .read_file(coordinate, READY_FILE)
        .map_err(|_| AppendError("memory file could not be read"))?
    else {
        return Ok(false);
    };
    let actual: ReadyDocument = serde_json::from_slice(&bytes)
        .map_err(|_| AppendError("memory readiness could not be read"))?;
    validate_ready_document(&actual, source, note)
        .map_err(|_| AppendError("memory readiness could not be validated"))?;
    let expected = ready_document(record, source)
        .map_err(|_| AppendError("memory readiness could not be serialized"))?;
    let chain = store
        .read_chain(coordinate)
        .map_err(|_| AppendError("memory chain could not be read"))?;
    Ok(actual == expected
        && chain.is_some_and(|chain| {
            chain.seq == actual.chain.seq
                && chain.prev_day == actual.chain.prev_day
                && chain.prev_segment == actual.chain.prev_segment
        }))
}

struct JournalStore<'a> {
    journal: &'a Path,
    source_lock: Option<solstone_core_journal_io::BoundParentLock>,
    live_name_lock: Option<solstone_core_journal_io::FileLock>,
    defer_success: bool,
    admission: Option<AuditCoordinates>,
    pending_outcome: Option<Outcome>,
}

impl<'a> JournalStore<'a> {
    fn new(journal: &'a Path) -> Self {
        Self {
            journal,
            source_lock: None,
            live_name_lock: None,
            defer_success: false,
            admission: None,
            pending_outcome: None,
        }
    }

    fn operation_path(&self, address: &OperationAddress) -> PathBuf {
        self.journal.join(address.relative())
    }

    fn read_bounded(
        &self,
        relative: &Path,
        maximum: usize,
    ) -> Result<Option<Vec<u8>>, StoreFailure> {
        let root = solstone_core_journal_io::journal_root::JournalRoot::open(self.journal)
            .map_err(|_| StoreFailure("memory journal could not be admitted"))?;
        solstone_core_journal_io::read_relative_file_bounded(&root, relative, maximum)
            .map(|observed| observed.map(|observed| observed.bytes))
            .map_err(|_| StoreFailure("memory file could not be read safely"))
    }

    fn segment_dir(&self, coordinate: &Coordinate) -> Result<SegmentDir, StoreFailure> {
        // Root aliases are admitted by JournalRoot (for example /var on
        // macOS). Compare both segment owners using its verified spelling;
        // exact lookup still refuses every linked descendant below the root.
        let root = solstone_core_journal_io::journal_root::JournalRoot::open(self.journal)
            .map_err(|_| StoreFailure("memory journal could not be admitted"))?;
        let namespace = segment_namespace_root(&root)?;
        let exact = solstone_core_journal_io::resolve_segment_exact(
            &namespace,
            &coordinate.day,
            &coordinate.stream,
            &coordinate.segment,
        )
        .map_err(|_| StoreFailure("memory segment lookup failed"))?
        .ok_or(StoreFailure("memory segment is absent"))?;
        let segment = SegmentDir::resolve(
            root.canonical_path(),
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
                }
                Err(_) => return Err(StoreFailure("memory directory could not be inspected")),
            }
            let parent = relative.parent().unwrap_or(Path::new(""));
            sync_parent(self.journal, parent)?;
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
        let relative = address.relative();
        self.read_bounded(Path::new(&relative), MAX_METADATA_BYTES)
    }

    fn occupied_coordinates(
        &self,
        stream: &str,
        day: &str,
    ) -> Result<HashSet<String>, StoreFailure> {
        let source_component = stream
            .strip_prefix("agent-memory-")
            .filter(|component| component.len() == 64)
            .ok_or(StoreFailure("memory stream is invalid"))?;
        let source = SourceKey::parse(format!("sha256:{source_component}"))
            .map_err(|_| StoreFailure("memory source is invalid"))?;
        let relative = format!("config/agent-memory/{source_component}");
        let directory = self.journal.join(&relative);
        match fs::symlink_metadata(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
            Err(_) => {
                return Err(StoreFailure(
                    "memory reservation directory could not be inspected",
                ));
            }
            Ok(metadata) if !metadata.file_type().is_dir() => {
                return Err(StoreFailure("memory reservation path is not a directory"));
            }
            Ok(_) => {}
        }
        solstone_core_journal_io::contained_path(self.journal, &relative)
            .map_err(|_| StoreFailure("memory reservation path escaped the journal"))?;
        let mut occupied = HashSet::new();
        for entry in fs::read_dir(directory)
            .map_err(|_| StoreFailure("memory reservation directory could not be read"))?
        {
            let entry =
                entry.map_err(|_| StoreFailure("memory reservation entry could not be read"))?;
            if solstone_core_journal_io::atomic::is_publication_candidate_name(&entry.file_name()) {
                continue;
            }
            let metadata = entry
                .file_type()
                .map_err(|_| StoreFailure("memory reservation entry could not be inspected"))?;
            if !metadata.is_file() {
                return Err(StoreFailure(
                    "memory reservation entry is not a regular file",
                ));
            }
            let bytes = self
                .read_bounded(
                    &Path::new(&relative).join(entry.file_name()),
                    MAX_METADATA_BYTES,
                )?
                .ok_or(StoreFailure("memory reservation record disappeared"))?;
            let record: OperationRecord = serde_json::from_slice(&bytes)
                .map_err(|_| StoreFailure("memory reservation record is invalid"))?;
            validate_record_header(&record, &source)
                .map_err(|_| StoreFailure("memory reservation record is invalid"))?;
            if entry.file_name()
                != std::ffi::OsStr::new(&format!("{}.json", digest(record.operation_id.as_bytes())))
            {
                return Err(StoreFailure(
                    "memory reservation filename does not match its operation",
                ));
            }
            if record.coordinate.day == day && record.coordinate.stream == stream {
                occupied.insert(record.coordinate.segment);
            }
        }
        Ok(occupied)
    }
}

impl AppendStore for JournalStore<'_> {
    fn hold_source(&mut self, source_component: &str) -> Result<(), StoreFailure> {
        self.source_lock = Some(
            hold_agent_memory_mutation(self.journal, source_component)
                .map_err(|_| StoreFailure("memory source lock failed"))?,
        );
        #[cfg(unix)]
        solstone_core_journal_io::sync_root(self.journal)
            .map_err(|_| StoreFailure("memory source parent sync failed"))?;
        sync_dir(self.journal, "streams")
            .map_err(|_| StoreFailure("memory source parent sync failed"))?;
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
        arguments.insert("authority".into(), Value::String("own_memory".into()));
        let admission = Admission {
            connection: verified_id,
            agent_identity: source.as_str(),
            tool_name: ToolName::SaveMemory,
            arguments,
            permission: None,
        };
        let zone = solstone_core_journal_config::owner_zone(self.journal);
        match write_interaction_record(self.journal, now.with_timezone(&zone), &admission) {
            Ok(coordinates) => {
                self.admission = Some(coordinates.clone());
                Ok(AuditLocation {
                    day: coordinates.day.format("%Y%m%d").to_string(),
                    stream: coordinates.stream,
                    segment: coordinates.segment,
                })
            }
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
        let zone = solstone_core_journal_config::owner_zone(self.journal);
        let owner_now = now.with_timezone(&zone);
        let day = owner_now.format("%Y%m%d").to_string();
        let candidate = format!("{}_1", owner_now.format("%H%M%S"));
        let parent = self.journal.join("chronicle").join(&day).join(stream);
        let occupied = self.occupied_coordinates(stream, &day)?;
        let segment = find_available_segment_with_occupied(
            &parent,
            &candidate,
            MAX_SEGMENT_ATTEMPTS,
            &occupied,
        )
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
        let root = solstone_core_journal_io::journal_root::JournalRoot::open(self.journal)
            .map_err(|_| StoreFailure("memory journal could not be admitted"))?;
        let namespace = segment_namespace_root(&root)?;
        let journal = namespace.as_path();
        let relative = segment_rel(&coordinate.day, &coordinate.stream, &coordinate.segment);
        let parent_rel = relative
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .ok_or(StoreFailure("memory segment parent is invalid"))?;
        let parent = journal.join(parent_rel);
        // Exact lookup refuses linked ancestry even when its target remains
        // inside this journal. A containment-only check could adopt another
        // stream's tombstone or create a lock in that other stream.
        if solstone_core_journal_io::resolve_stream_exact(
            journal,
            &coordinate.day,
            &coordinate.stream,
        )
        .map_err(|_| StoreFailure("memory segment parent lookup failed"))?
        .is_none()
        {
            solstone_core_journal_io::create_segment_parent_strict(
                journal,
                &coordinate.day,
                &coordinate.stream,
                &coordinate.segment,
            )
            .map_err(|_| StoreFailure("memory segment parent create failed"))?;
        }
        // This is also the spelling used by SegmentDir, so the stream owner
        // can verify the exact live-name guard even through a root alias.
        let live = journal.join(&relative);
        let live_name_lock = solstone_core_journal_io::hold_lock(
            &live,
            solstone_core_journal_io::LockOptions::default(),
        )
        .map_err(|_| StoreFailure("memory live-name lock failed"))?;
        self.live_name_lock = Some(live_name_lock);
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
        if self.live_name_lock.is_none() {
            return Err(StoreFailure("memory current live-name lock is missing"));
        }
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
        self.segment_dir(coordinate)?;
        // Keep the retained-root read relative to the validated coordinate;
        // a canonical segment path cannot be stripped against a root alias.
        let relative = PathBuf::from(segment_rel(
            &coordinate.day,
            &coordinate.stream,
            &coordinate.segment,
        ))
        .join(name);
        self.read_bounded(
            &relative,
            if name == NOTE_FILE {
                MAX_NOTE_BYTES
            } else {
                MAX_METADATA_BYTES
            },
        )
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
                    // Create-exclusive publication flushed the original inode
                    // before exposing its name. Reconfirm the containing names
                    // on retry without replacing the immutable source sibling.
                    Ok(Some(existing)) if existing == bytes => {
                        match sync_chronicle_chain(self.journal, coordinate) {
                            Ok(()) => FilePublication::Confirmed,
                            Err(_) => FilePublication::PublishedUnconfirmed,
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
        let live_name_lock = self
            .live_name_lock
            .as_ref()
            .ok_or(StoreFailure("memory current live-name lock is missing"))?;
        advance_agent_memory_stream(
            &coordinate.stream,
            &coordinate.day,
            &coordinate.segment,
            &segment,
            source.as_str(),
            source.component(),
            live_name_lock,
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
        if self.defer_success
            && matches!(
                outcome,
                Outcome::Stored | Outcome::Replayed | Outcome::Deleted
            )
        {
            self.pending_outcome = Some(outcome);
            return Publication::Confirmed;
        }
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

    fn read_chain(
        &mut self,
        coordinate: &Coordinate,
    ) -> Result<Option<StreamAdvance>, StoreFailure> {
        read_agent_memory_chain(&self.segment_dir(coordinate)?)
            .map_err(|_| StoreFailure("memory chain could not be read"))
    }
}

fn segment_namespace_root(
    root: &solstone_core_journal_io::journal_root::JournalRoot,
) -> Result<PathBuf, StoreFailure> {
    #[cfg(windows)]
    {
        // Windows admission deliberately retains a plain drive path, while
        // SegmentDir uses Rust's verbatim canonical spelling. Normalize only
        // the admitted root, then revalidate its binding; descendant lookup
        // still goes through the strict no-follow resolver. SegmentDir keeps
        // the admitted plain root for its later retained-root reads.
        let namespace = fs::canonicalize(root.canonical_path())
            .map_err(|_| StoreFailure("memory root namespace could not be resolved"))?;
        root.revalidate_canonical_binding()
            .map_err(|_| StoreFailure("memory root namespace binding changed"))?;
        Ok(namespace)
    }
    #[cfg(not(windows))]
    {
        Ok(root.canonical_path().to_path_buf())
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
    append_connection_memory(
        journal,
        AuthenticatedMemorySource {
            verified_id,
            creation_label: "authenticated connection",
        },
        text,
        operation_id,
        now,
    )
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
        AdmissionFailure, AppendResult, AppendStore, AuditLocation, AuthenticatedMemorySource,
        FilePublication, OperationAddress, Publication, SegmentObservation, StoreFailure,
        append_with, append_with_source,
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
        chains: HashMap<String, solstone_core_segment::StreamAdvance>,
        last_coordinates: HashMap<String, Coordinate>,
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
            if let Some(chain) = self.chains.get(&Self::coordinate_key(coordinate)) {
                return Ok(chain.clone());
            }
            let key = coordinate.stream.clone();
            let sequence = self.sequences.entry(key.clone()).or_default();
            *sequence += 1;
            let previous = self.last_coordinates.insert(key, coordinate.clone());
            let chain = solstone_core_segment::StreamAdvance {
                prev_day: previous.as_ref().map(|coordinate| coordinate.day.clone()),
                prev_segment: previous.map(|coordinate| coordinate.segment),
                seq: *sequence,
            };
            self.chains
                .insert(Self::coordinate_key(coordinate), chain.clone());
            Ok(chain)
        }

        fn read_chain(
            &mut self,
            coordinate: &Coordinate,
        ) -> Result<Option<solstone_core_segment::StreamAdvance>, StoreFailure> {
            Ok(self.chains.get(&Self::coordinate_key(coordinate)).cloned())
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
    fn trusted_creation_label_is_captured_once_and_not_rewritten_by_rename() {
        let mut store = MemoryStore::default();
        let first = append_with_source(
            &mut store,
            AuthenticatedMemorySource {
                verified_id: "oauth:grant",
                creation_label: "original connection",
            },
            "note",
            "op",
            now(),
        )
        .unwrap();
        let replay = append_with_source(
            &mut store,
            AuthenticatedMemorySource {
                verified_id: "oauth:grant",
                creation_label: "renamed connection",
            },
            "note",
            "op",
            now() + chrono::Duration::days(1),
        )
        .unwrap();
        let (AppendResult::Stored(first), AppendResult::Replayed(replay)) = (first, replay) else {
            panic!("expected stored then replayed memory");
        };
        assert_eq!(first.origin.creation_label, "original connection");
        assert_eq!(replay.origin, first.origin);
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
        Coordinate, OperationRecord, OriginKind, Readiness, SourceKey, digest,
    };

    use super::{
        AppendResult, AuthenticatedMemorySource, OperationAddress, append_connection_memory,
    };

    fn fixture() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("solstone-memory-full-")
            .tempdir()
            .expect("journal fixture")
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap()
    }

    fn authenticated(verified_id: &str) -> AuthenticatedMemorySource<'_> {
        AuthenticatedMemorySource {
            verified_id,
            creation_label: "authenticated connection",
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn journal_root_namespace_preserves_append_chain_and_operation_replay() {
        let outer = fixture();
        let canonical = outer.path().join("real").join("journal");
        fs::create_dir_all(&canonical).unwrap();
        #[cfg(unix)]
        let alias = {
            std::os::unix::fs::symlink(outer.path().join("real"), outer.path().join("alias"))
                .unwrap();
            outer.path().join("alias").join("journal")
        };
        #[cfg(windows)]
        let alias = canonical.clone();
        solstone_core_journal_io::JournalRoot::open(&alias)
            .expect("fixture root uses an admitted native path spelling");
        assert_ne!(alias, fs::canonicalize(&alias).unwrap());
        let AppendResult::Stored(first) = append_connection_memory(
            &alias,
            authenticated("bearer:root-alias"),
            "first",
            "A",
            now(),
        )
        .unwrap() else {
            panic!("append through an admitted root alias should complete");
        };
        let AppendResult::Stored(second) = append_connection_memory(
            &canonical,
            authenticated("bearer:root-alias"),
            "second",
            "B",
            now() + chrono::Duration::seconds(1),
        )
        .unwrap() else {
            panic!("later append through the same root should complete");
        };
        assert_ne!(first.origin.segment, second.origin.segment);
        assert_eq!(
            append_connection_memory(
                &alias,
                authenticated("bearer:root-alias"),
                "first",
                "A",
                now() + chrono::Duration::minutes(1),
            )
            .unwrap(),
            AppendResult::Replayed(first.clone())
        );
        let second_marker = canonical
            .join("chronicle/20260102")
            .join(&second.origin.stream)
            .join(&second.origin.segment)
            .join("stream.json");
        let marker: serde_json::Value =
            serde_json::from_slice(&fs::read(second_marker).unwrap()).unwrap();
        assert_eq!(marker["prev_segment"], first.origin.segment);
        assert_eq!(marker["prev_day"], "20260102");
        assert_eq!(marker["seq"], 2);
    }

    #[cfg(unix)]
    #[test]
    fn linked_chronicle_stream_cannot_adopt_another_sources_tombstone_or_write_its_lock() {
        let journal = fixture();
        let other = journal.path().join("chronicle/20260102/other-source");
        fs::create_dir_all(other.join("120000_1")).unwrap();
        fs::write(other.join("120000_1/tombstone.json"), b"{}").unwrap();
        let source = SourceKey::from_verified_id("bearer:linked-stream");
        let reservation = OperationRecord {
            operation_id: "op".into(),
            digest: digest(b"note"),
            byte_count: 4,
            created_at: now(),
            origin_kind: OriginKind::AgentMemory,
            creation_label: "authenticated connection".into(),
            coordinate: Coordinate {
                day: "20260102".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "120000_1".into(),
            },
            phase: Readiness::Reserved,
            chain: None,
        };
        let recovery = journal
            .path()
            .join(OperationAddress::new(&source, "op").relative());
        fs::create_dir_all(recovery.parent().unwrap()).unwrap();
        fs::write(recovery, serde_json::to_vec(&reservation).unwrap()).unwrap();
        std::os::unix::fs::symlink(
            &other,
            journal
                .path()
                .join("chronicle/20260102")
                .join(format!("agent-memory-{}", source.component())),
        )
        .unwrap();
        assert!(matches!(
            append_connection_memory(
                journal.path(),
                authenticated("bearer:linked-stream"),
                "note",
                "op",
                now(),
            ),
            Ok(AppendResult::UncertainRetry { .. }) | Err(_)
        ));
        assert!(!other.join("120000_1.lock").exists());
        assert_eq!(fs::read_dir(&other).unwrap().count(), 1);
        assert!(!other.join("120000_1/note.txt").exists());
        assert_eq!(
            fs::read(other.join("120000_1/tombstone.json")).unwrap(),
            b"{}"
        );
    }

    #[cfg(all(unix, feature = "test-hooks"))]
    #[test]
    fn immutable_publication_retry_confirms_the_original_inode_without_replacement() {
        use super::{AppendStore, FilePublication, JournalStore};
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_fault,
        };
        use std::os::unix::fs::MetadataExt;
        let journal = fixture();
        let source = SourceKey::from_verified_id("bearer:immutable-publication");
        let coordinate = Coordinate {
            day: "20260102".into(),
            stream: format!("agent-memory-{}", source.component()),
            segment: "120000_1".into(),
        };
        let mut store = JournalStore::new(journal.path());
        store.hold_source(source.component()).unwrap();
        store.observe_segment(&coordinate).unwrap();
        store.create_segment(&coordinate).unwrap();
        let (outcome, consumed) =
            run_with_bound_publication_fault(BoundPublicationPrimitive::ParentSync, 1, 5, || {
                store.publish_file(&coordinate, "note.txt", b"original")
            });
        assert!(consumed);
        assert_eq!(outcome, FilePublication::PublishedUnconfirmed);
        let path = store
            .segment_dir(&coordinate)
            .unwrap()
            .path()
            .join("note.txt");
        let original = fs::metadata(&path).unwrap();
        assert_eq!(
            store.publish_file(&coordinate, "note.txt", b"original"),
            FilePublication::Confirmed
        );
        let retried = fs::metadata(&path).unwrap();
        assert_eq!(retried.ino(), original.ino());
        assert_eq!(retried.modified().unwrap(), original.modified().unwrap());
        assert_eq!(
            store.publish_file(&coordinate, "note.txt", b"different"),
            FilePublication::Conflict
        );
        assert_eq!(fs::read(path).unwrap(), b"original");
    }

    #[test]
    fn malformed_marker_replay_reports_failure_without_quarantining_or_repairing_it() {
        let journal = fixture();
        let AppendResult::Stored(first) = append_connection_memory(
            journal.path(),
            authenticated("bearer:corrupt-marker"),
            "note",
            "op",
            now(),
        )
        .unwrap() else {
            panic!("original append should complete");
        };
        let marker = journal
            .path()
            .join("chronicle/20260102")
            .join(first.origin.stream)
            .join(first.origin.segment)
            .join("stream.json");
        fs::write(&marker, b"{").unwrap();
        assert!(
            append_connection_memory(
                journal.path(),
                authenticated("bearer:corrupt-marker"),
                "note",
                "op",
                now(),
            )
            .is_err()
        );
        assert_eq!(fs::read(&marker).unwrap(), b"{");
        let error_outcomes = fs::read_dir(journal.path().join("chronicle/20260102/mcp.agent"))
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path().join("outcome.json");
                fs::read(path).ok().map(|bytes| {
                    serde_json::from_slice::<solstone_core_mcp_audit::OutcomeRecord>(&bytes)
                        .unwrap()
                })
            })
            .filter(|outcome| outcome.outcome == solstone_core_mcp_audit::Outcome::Error)
            .count();
        assert_eq!(error_outcomes, 1);
    }

    #[test]
    fn owner_retention_consumes_the_original_operation_and_later_appends_skip_its_tail() {
        let journal = fixture();
        let AppendResult::Stored(first) = append_connection_memory(
            journal.path(),
            authenticated("bearer:owner-delete"),
            "original",
            "op",
            now(),
        )
        .unwrap() else {
            panic!("original append should complete");
        };
        let target = solstone_core_retention::receipt::Target {
            day: "20260102".into(),
            stream: first.origin.stream.clone(),
            dir: first.origin.segment.clone(),
        };
        let removed = solstone_core_retention::door::remove_segments(
            journal.path(),
            std::slice::from_ref(&target),
            &now().to_rfc3339(),
            solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
            "journal-owner",
        );
        assert!(removed.removed_paths().next().is_some());
        assert!(matches!(
            append_connection_memory(
                journal.path(),
                authenticated("bearer:owner-delete"),
                "original",
                "op",
                now(),
            ),
            Ok(AppendResult::Deleted(_))
        ));
        let AppendResult::Stored(second) = append_connection_memory(
            journal.path(),
            authenticated("bearer:owner-delete"),
            "later",
            "new-op",
            now(),
        )
        .unwrap() else {
            panic!("later append should complete");
        };
        assert_ne!(first.origin.segment, second.origin.segment);
        let original = journal
            .path()
            .join("chronicle")
            .join(target.day)
            .join(target.stream)
            .join(target.dir);
        assert!(!original.join("note.txt").exists());
        assert!(!original.join("stream.json").exists());
        assert!(original.join("tombstone.json").is_file());
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
            origin_kind: OriginKind::AgentMemory,
            creation_label: "authenticated connection".into(),
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
                AuthenticatedMemorySource {
                    verified_id: "bearer:corrupt-coordinate",
                    creation_label: "authenticated connection",
                },
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
            append_connection_memory(
                journal.path(),
                AuthenticatedMemorySource {
                    verified_id: "bearer:symlink",
                    creation_label: "authenticated connection",
                },
                "note",
                "op",
                now(),
            )
            .is_err()
        );
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[test]
    fn same_operation_is_serialized_across_processes() {
        let root = std::env::var_os("SOLSTONE_MEMORY_CHILD_ROOT");
        if let Some(root) = root {
            let root = std::path::PathBuf::from(root);
            let result = append_connection_memory(
                &root,
                AuthenticatedMemorySource {
                    verified_id: "bearer:multi-process",
                    creation_label: "authenticated connection",
                },
                "note",
                "same-op",
                now(),
            );
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

    #[test]
    fn durable_reservation_occupies_its_coordinate_before_segment_creation() {
        let journal = fixture();
        let source = SourceKey::from_verified_id("bearer:reservation");
        let address = OperationAddress::new(&source, "A");
        let coordinate = Coordinate {
            day: "20260102".into(),
            stream: format!("agent-memory-{}", source.component()),
            segment: "120000_1".into(),
        };
        let reservation = OperationRecord {
            operation_id: "A".into(),
            digest: digest(b"a"),
            byte_count: 1,
            created_at: now(),
            origin_kind: OriginKind::AgentMemory,
            creation_label: "authenticated connection".into(),
            coordinate: coordinate.clone(),
            phase: Readiness::Reserved,
            chain: None,
        };
        let operation_path = journal.path().join(address.relative());
        fs::create_dir_all(operation_path.parent().unwrap()).unwrap();
        fs::write(operation_path, serde_json::to_vec(&reservation).unwrap()).unwrap();

        let AppendResult::Stored(second) = append_connection_memory(
            journal.path(),
            authenticated("bearer:reservation"),
            "b",
            "B",
            now(),
        )
        .unwrap() else {
            panic!("second operation should complete");
        };
        let AppendResult::Stored(first) = append_connection_memory(
            journal.path(),
            authenticated("bearer:reservation"),
            "a",
            "A",
            now() + chrono::Duration::minutes(5),
        )
        .unwrap() else {
            panic!("reserved operation should complete");
        };
        assert_eq!(first.origin.coordinate("20260102"), coordinate);
        assert_eq!(first.created_at, now());
        assert_ne!(second.origin.segment, first.origin.segment);
    }

    #[test]
    fn recovered_marker_keeps_its_original_chain_after_a_later_head_commits() {
        let journal = fixture();
        let source = SourceKey::from_verified_id("bearer:marker-recovery");
        let AppendResult::Stored(first) = append_connection_memory(
            journal.path(),
            authenticated("bearer:marker-recovery"),
            "a",
            "A",
            now(),
        )
        .unwrap() else {
            panic!("first operation should complete");
        };
        let address = OperationAddress::new(&source, "A");
        let record_path = journal.path().join(address.relative());
        let mut record: OperationRecord =
            serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        let marker_path = journal
            .path()
            .join("chronicle")
            .join("20260102")
            .join(&record.coordinate.stream)
            .join(&record.coordinate.segment)
            .join("stream.json");
        let original_marker = fs::read(&marker_path).unwrap();
        record.phase = Readiness::Noted;
        record.chain = None;
        fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();

        let AppendResult::Stored(second) = append_connection_memory(
            journal.path(),
            authenticated("bearer:marker-recovery"),
            "b",
            "B",
            now(),
        )
        .unwrap() else {
            panic!("later operation should complete");
        };
        assert_ne!(first.origin.segment, second.origin.segment);
        assert!(matches!(
            append_connection_memory(
                journal.path(),
                authenticated("bearer:marker-recovery"),
                "a",
                "A",
                now() + chrono::Duration::minutes(1),
            ),
            Ok(AppendResult::Stored(_))
        ));
        let recovered: OperationRecord =
            serde_json::from_slice(&fs::read(record_path).unwrap()).unwrap();
        assert_eq!(recovered.chain.as_ref().map(|chain| chain.seq), Some(1));
        assert_eq!(fs::read(&marker_path).unwrap(), original_marker);
        let stream_state: serde_json::Value = serde_json::from_slice(
            &fs::read(
                journal
                    .path()
                    .join("streams")
                    .join(format!("{}.json", recovered.coordinate.stream)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(stream_state["seq"], 2);
        assert_eq!(stream_state["last_segment"], second.origin.segment);
    }
}
