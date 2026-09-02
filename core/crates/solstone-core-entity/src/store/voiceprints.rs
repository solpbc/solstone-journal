// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Number, Value};
use solstone_core_journal_io::AtomicWriteError;
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::LockError;
use solstone_core_journal_io::LockOptions;
use solstone_core_journal_io::PathError;
use solstone_core_journal_io::ReadError;
use solstone_core_journal_io::Removed;
use solstone_core_journal_io::atomic_replace;
use solstone_core_journal_io::hold_lock;
use solstone_core_journal_io::path_lexists;
use solstone_core_journal_io::read_bytes;
use solstone_core_journal_io::remove_file;
use solstone_core_npy::{NpyBlob, parse_npy, write_npy};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::entity_paths::entity_memory_path;
use super::lifecycle::EntityLifecycleError;
use super::reconcile::{float_to_integer, integer_value, python_optional_json_equal};

pub(crate) const EMBEDDING_WIDTH: usize = 256;
const EMBEDDINGS_MEMBER: &str = "embeddings.npy";
const METADATA_MEMBER: &str = "metadata.npy";
const ENVELOPE_MEMBER: &str = "envelope.npy";
const ENVELOPE_FORMAT: &str = "solstone-voiceprint-envelope";
const CURRENT_ENVELOPE_VERSION: u32 = 1;

/// The encoder which produced a voiceprint embedding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderIdentity {
    pub id: String,
    pub sha256: String,
    pub width: usize,
}

/// Self-describing metadata for a voiceprint archive.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintEnvelope {
    pub version: u32,
    pub encoder: Option<EncoderIdentity>,
    pub extra: serde_json::Map<String, Value>,
}

impl Default for VoiceprintEnvelope {
    fn default() -> Self {
        Self {
            version: 0,
            encoder: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintArchive {
    pub embeddings: Vec<f32>,
    pub rows: usize,
    pub metadata: Vec<String>,
    pub envelope: VoiceprintEnvelope,
    pub unrecognized_members: Vec<String>,
}

impl VoiceprintArchive {
    /// Whether this archive positively identifies the supplied running encoder.
    pub fn matches_running_encoder(&self, running_encoder: &EncoderIdentity) -> bool {
        self.envelope.encoder.as_ref() == Some(running_encoder)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceprintNpzError {
    Archive(String),
    Invalid(String),
    EmbeddingWidth { found: usize },
}

impl fmt::Display for VoiceprintNpzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(message) | Self::Invalid(message) => formatter.write_str(message),
            Self::EmbeddingWidth { found } => write!(
                formatter,
                "embeddings.npy width {found} does not match {EMBEDDING_WIDTH}"
            ),
        }
    }
}

impl Error for VoiceprintNpzError {}

/// One voiceprint row supplied to the durable batch writer.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintItem {
    pub embedding: Vec<f32>,
    pub metadata: Value,
}

/// A canonical, Python-equality-compatible metadata key field.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CanonicalKeyField {
    Absent,
    Bool(bool),
    Int(i128),
    Float(u64),
    Str(String),
}

/// The four metadata fields that identify a voiceprint row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VoiceprintKey(pub [CanonicalKeyField; 4]);

/// One requested row removal with an optional expected complete metadata value.
///
/// A present value provides the exact-match safety required by durable undo.
/// `None` removes every row with the canonical four-field key, matching the
/// authenticated direct speaker-management operation.
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintRemoval {
    pub key: Value,
    pub expected_metadata: Option<Value>,
}

/// Per-cause count for rows that were not removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceprintSkipReasons {
    pub missing: usize,
    pub metadata_mismatch: usize,
}

/// Result of applying one or more voiceprint removals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoiceprintRemovalReport {
    pub removed_count: usize,
    pub skipped_count: usize,
    pub skipped_reasons: VoiceprintSkipReasons,
    pub file_removed: bool,
}

/// Failure while reading or mutating an entity voiceprint archive.
#[derive(Debug)]
pub enum VoiceprintOperationError {
    Lifecycle(EntityLifecycleError),
    Lock(LockError),
    Read(ReadError),
    Write(AtomicWriteError),
    Path(PathError),
    Npz(VoiceprintNpzError),
    MetadataJson(String),
    MetadataNotObject,
    InvalidRemovalKey,
    UnsupportedKeyField {
        field: &'static str,
    },
    DuplicateExactMatch,
    UnrecognizedNpzMember {
        member: String,
    },
    EncoderIdentityMismatch {
        stored_encoder_id: String,
        caller_encoder_id: String,
    },
    UnsupportedEnvelopeVersion {
        found: u32,
        max_supported: u32,
    },
}

impl fmt::Display for VoiceprintOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::Lock(error) => error.fmt(formatter),
            Self::Read(error) => error.fmt(formatter),
            Self::Write(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Npz(error) => error.fmt(formatter),
            Self::MetadataJson(error) => {
                write!(formatter, "invalid voiceprint metadata JSON: {error}")
            }
            Self::MetadataNotObject => formatter.write_str("voiceprint metadata must be an object"),
            Self::InvalidRemovalKey => {
                formatter.write_str("voiceprint removal key must be an object")
            }
            Self::UnsupportedKeyField { field } => {
                write!(formatter, "voiceprint key field {field} must be a scalar")
            }
            Self::DuplicateExactMatch => {
                formatter.write_str("voiceprint removal locator matched multiple rows")
            }
            Self::UnrecognizedNpzMember { member } => {
                write!(
                    formatter,
                    "voiceprint archive has unrecognized member {member}"
                )
            }
            Self::EncoderIdentityMismatch {
                stored_encoder_id,
                caller_encoder_id,
            } => write!(
                formatter,
                "voiceprint encoder {stored_encoder_id} does not match {caller_encoder_id}"
            ),
            Self::UnsupportedEnvelopeVersion {
                found,
                max_supported,
            } => write!(
                formatter,
                "voiceprint envelope version {found} exceeds supported version {max_supported}"
            ),
        }
    }
}

impl Error for VoiceprintOperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Lifecycle(error) => Some(error),
            Self::Lock(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::Write(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Npz(error) => Some(error),
            Self::MetadataJson(_)
            | Self::MetadataNotObject
            | Self::InvalidRemovalKey
            | Self::UnsupportedKeyField { .. }
            | Self::DuplicateExactMatch
            | Self::UnrecognizedNpzMember { .. }
            | Self::EncoderIdentityMismatch { .. }
            | Self::UnsupportedEnvelopeVersion { .. } => None,
        }
    }
}

impl From<EntityLifecycleError> for VoiceprintOperationError {
    fn from(error: EntityLifecycleError) -> Self {
        Self::Lifecycle(error)
    }
}

impl From<LockError> for VoiceprintOperationError {
    fn from(error: LockError) -> Self {
        Self::Lock(error)
    }
}

impl From<ReadError> for VoiceprintOperationError {
    fn from(error: ReadError) -> Self {
        Self::Read(error)
    }
}

impl From<AtomicWriteError> for VoiceprintOperationError {
    fn from(error: AtomicWriteError) -> Self {
        Self::Write(error)
    }
}

impl From<PathError> for VoiceprintOperationError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

impl From<VoiceprintNpzError> for VoiceprintOperationError {
    fn from(error: VoiceprintNpzError) -> Self {
        Self::Npz(error)
    }
}

/// L2-normalize an embedding vector, returning `None` for a zero vector.
pub fn normalize_embedding(embedding: &[f32]) -> Option<Vec<f32>> {
    let norm = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f32>()
        .sqrt();
    (norm > 0.0).then(|| embedding.iter().map(|value| value / norm).collect())
}

/// Load a valid entity voiceprint archive, collapsing absence and read failures to `None`.
pub fn load_entity_voiceprints_file(
    journal_root: &Path,
    entity_id: &str,
) -> Option<VoiceprintArchive> {
    match try_load_entity_voiceprints_file(journal_root, entity_id) {
        Ok(archive) => archive,
        Err(error) => {
            log::warn!(
                "failed to load voiceprints for entity {}: {}",
                entity_id,
                error
            );
            None
        }
    }
}

/// Load an entity voiceprint archive while preserving read and parse failures.
pub fn try_load_entity_voiceprints_file(
    journal_root: &Path,
    entity_id: &str,
) -> Result<Option<VoiceprintArchive>, VoiceprintOperationError> {
    let (_directory, path) = resolve_voiceprint_path(journal_root, entity_id, false)?;
    load_voiceprints(&path)
}

/// Load an entity voiceprint archive from an already-resolved entity directory.
///
/// `try_load_entity_voiceprints_file` resolves `entity_id` through the journal
/// identity map, and building that map reads every `entities/*/entity.json` on
/// disk. A caller that resolves many entities in one pass holds the map itself
/// and calls this instead, so the scan happens once rather than once per lookup.
pub fn try_load_entity_voiceprints_in_dir(
    journal_root: &Path,
    entity_dir: &str,
) -> Result<Option<VoiceprintArchive>, VoiceprintOperationError> {
    let identity = super::paths::identity_path(journal_root, entity_dir)
        .map_err(EntityLifecycleError::from)?;
    let path = identity
        .parent()
        .expect("identity path always has an entity directory")
        .join("voiceprints.npz");
    load_voiceprints(&path)
}

/// Return saved voiceprint identity keys for idempotency checks.
pub fn load_existing_voiceprint_keys(
    journal_root: &Path,
    entity_id: &str,
) -> HashSet<VoiceprintKey> {
    let Some(archive) = load_entity_voiceprints_file(journal_root, entity_id) else {
        return HashSet::new();
    };
    let mut keys = HashSet::with_capacity(archive.metadata.len());
    for metadata in &archive.metadata {
        match voiceprint_removal_key(metadata) {
            Ok(key) => {
                keys.insert(key);
            }
            Err(error) => {
                log::warn!(
                    "failed to read voiceprint key for entity {}: {}",
                    entity_id,
                    error
                );
                return HashSet::new();
            }
        }
    }
    keys
}

/// Append a batch of caller-normalized voiceprints in one locked write.
pub fn save_voiceprints_batch(
    journal_root: &Path,
    entity_id: &str,
    new_items: &[VoiceprintItem],
    running_encoder: &EncoderIdentity,
) -> Result<usize, VoiceprintOperationError> {
    if new_items.is_empty() {
        return Ok(0);
    }
    let (_directory, path) = resolve_voiceprint_path(journal_root, entity_id, true)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut archive = load_voiceprints(&path)?.unwrap_or_else(empty_archive);
    ensure_mutation_allowed(&archive, running_encoder)?;
    for item in new_items {
        archive.embeddings.extend_from_slice(&item.embedding);
        archive.metadata.push(serialize_metadata(&item.metadata)?);
    }
    archive.rows = archive.metadata.len();
    write_and_verify_voiceprints(&path, &archive, running_encoder)?;
    Ok(new_items.len())
}

/// Rewrite metadata in place only when the mutator reports changes.
pub fn rewrite_voiceprint_metadata<F>(
    journal_root: &Path,
    entity_id: &str,
    running_encoder: &EncoderIdentity,
    mutator: F,
) -> Result<usize, VoiceprintOperationError>
where
    F: FnOnce(&mut [Value]) -> usize,
{
    let Ok((_directory, path)) = resolve_voiceprint_path(journal_root, entity_id, false) else {
        return Ok(0);
    };
    if !path_lexists(&path)? {
        return Ok(0);
    }
    let _lock = hold_lock(&path, LockOptions::default())?;
    let Some(mut archive) = load_voiceprints(&path)? else {
        return Ok(0);
    };
    ensure_mutation_allowed(&archive, running_encoder)?;
    let mut metadata = parse_metadata_values(&archive.metadata)?;
    let updates = mutator(&mut metadata);
    if updates == 0 {
        return Ok(0);
    }
    archive.metadata = metadata
        .iter()
        .map(serialize_metadata)
        .collect::<Result<_, _>>()?;
    write_and_verify_voiceprints(&path, &archive, running_encoder)?;
    Ok(updates)
}

/// Remove rows by exact Python-equality-compatible key and metadata match.
pub fn remove_voiceprints_by_key(
    journal_root: &Path,
    entity_id: &str,
    removals: &[VoiceprintRemoval],
    running_encoder: &EncoderIdentity,
) -> Result<VoiceprintRemovalReport, VoiceprintOperationError> {
    let mut report = VoiceprintRemovalReport::default();
    if removals.is_empty() {
        return Ok(report);
    }
    let Ok((directory, path)) = resolve_voiceprint_path(journal_root, entity_id, false) else {
        mark_all_missing(&mut report, removals.len());
        return Ok(report);
    };
    if !path_lexists(&path)? {
        mark_all_missing(&mut report, removals.len());
        return Ok(report);
    }
    let normalized_removals = removals
        .iter()
        .map(removal_key)
        .collect::<Result<Vec<_>, _>>()?;

    let _lock = hold_lock(&path, LockOptions::default())?;
    let Some(archive) = load_voiceprints(&path)? else {
        mark_all_missing(&mut report, removals.len());
        return Ok(report);
    };
    ensure_mutation_allowed(&archive, running_encoder)?;
    let metadata = parse_metadata_values(&archive.metadata)?;
    let keys = archive
        .metadata
        .iter()
        .map(|value| voiceprint_removal_key(value))
        .collect::<Result<Vec<_>, _>>()?;
    let mut remove_indexes = HashSet::new();
    for (removal, key) in removals.iter().zip(&normalized_removals) {
        let key_matches = metadata
            .iter()
            .enumerate()
            .filter_map(|(index, stored)| (keys[index] == *key).then_some((index, stored)))
            .collect::<Vec<_>>();
        if removal.expected_metadata.is_none() {
            if key_matches.is_empty() {
                report.skipped_reasons.missing += 1;
            } else {
                remove_indexes.extend(key_matches.into_iter().map(|(index, _)| index));
            }
            continue;
        }
        let exact_matches = key_matches
            .iter()
            .filter_map(|(index, stored)| {
                python_optional_json_equal(Some(*stored), removal.expected_metadata.as_ref())
                    .then_some(*index)
            })
            .collect::<Vec<_>>();
        if exact_matches.len() > 1 {
            return Err(VoiceprintOperationError::DuplicateExactMatch);
        }
        if let Some(index) = exact_matches.first() {
            remove_indexes.insert(*index);
            continue;
        }
        if !key_matches.is_empty() {
            report.skipped_reasons.metadata_mismatch += 1;
        } else {
            report.skipped_reasons.missing += 1;
        }
    }
    report.skipped_count =
        report.skipped_reasons.missing + report.skipped_reasons.metadata_mismatch;
    if remove_indexes.is_empty() {
        return Ok(report);
    }
    report.removed_count = remove_indexes.len();
    if remove_indexes.len() == archive.rows {
        let removed = remove_file(&directory, "voiceprints.npz")?;
        report.file_removed = matches!(removed, Removed::Unlinked);
        return Ok(report);
    }
    let mut kept = empty_archive();
    for (index, (embedding, metadata)) in archive
        .embeddings
        .chunks_exact(EMBEDDING_WIDTH)
        .zip(metadata.iter())
        .enumerate()
    {
        if !remove_indexes.contains(&index) {
            kept.embeddings.extend_from_slice(embedding);
            kept.metadata.push(serialize_metadata(metadata)?);
        }
    }
    kept.rows = kept.metadata.len();
    write_and_verify_voiceprints(&path, &kept, running_encoder)?;
    Ok(report)
}

/// Extract the normalized four-field key from one serialized metadata row.
pub(crate) fn voiceprint_removal_key(
    metadata: &str,
) -> Result<VoiceprintKey, VoiceprintOperationError> {
    let value = serde_json::from_str(metadata)
        .map_err(|error| VoiceprintOperationError::MetadataJson(error.to_string()))?;
    key_from_metadata_value(&value)
}

pub(crate) fn resolve_voiceprint_path(
    journal_root: &Path,
    entity_id: &str,
    create: bool,
) -> Result<(PathBuf, PathBuf), EntityLifecycleError> {
    let directory = entity_memory_path(journal_root, entity_id, create)?;
    Ok((directory.clone(), directory.join("voiceprints.npz")))
}

fn empty_archive() -> VoiceprintArchive {
    VoiceprintArchive {
        embeddings: Vec::new(),
        rows: 0,
        metadata: Vec::new(),
        envelope: VoiceprintEnvelope::default(),
        unrecognized_members: Vec::new(),
    }
}

fn load_voiceprints(path: &Path) -> Result<Option<VoiceprintArchive>, VoiceprintOperationError> {
    if !path_lexists(path)? {
        return Ok(None);
    }
    let bytes = read_bytes(path, Vec::new())?;
    read_voiceprints_npz(&bytes).map(Some).map_err(Into::into)
}

fn write_and_verify_voiceprints(
    path: &Path,
    archive: &VoiceprintArchive,
    selected_encoder: &EncoderIdentity,
) -> Result<(), VoiceprintOperationError> {
    let expected_written_archive = VoiceprintArchive {
        embeddings: archive.embeddings.clone(),
        rows: archive.rows,
        metadata: archive.metadata.clone(),
        envelope: stamped_envelope(&archive.envelope, selected_encoder),
        unrecognized_members: Vec::new(),
    };
    let bytes = write_voiceprints_npz(
        &archive.embeddings,
        &archive.metadata,
        &archive.envelope,
        selected_encoder,
    )?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default())?;
    let verified = load_voiceprints(path)?.ok_or_else(|| {
        VoiceprintOperationError::Npz(VoiceprintNpzError::Invalid(
            "voiceprint archive disappeared after write".to_owned(),
        ))
    })?;
    if verified != expected_written_archive {
        return Err(VoiceprintOperationError::Npz(VoiceprintNpzError::Invalid(
            "voiceprint archive changed after write".to_owned(),
        )));
    }
    Ok(())
}

fn ensure_mutation_allowed(
    archive: &VoiceprintArchive,
    running_encoder: &EncoderIdentity,
) -> Result<(), VoiceprintOperationError> {
    if let Some(member) = archive.unrecognized_members.first() {
        return Err(VoiceprintOperationError::UnrecognizedNpzMember {
            member: member.clone(),
        });
    }
    if archive.envelope.version > CURRENT_ENVELOPE_VERSION {
        return Err(VoiceprintOperationError::UnsupportedEnvelopeVersion {
            found: archive.envelope.version,
            max_supported: CURRENT_ENVELOPE_VERSION,
        });
    }
    if let Some(stored) = &archive.envelope.encoder
        && stored != running_encoder
    {
        return Err(VoiceprintOperationError::EncoderIdentityMismatch {
            stored_encoder_id: stored.id.clone(),
            caller_encoder_id: running_encoder.id.clone(),
        });
    }
    Ok(())
}

fn parse_metadata_values(values: &[String]) -> Result<Vec<Value>, VoiceprintOperationError> {
    values
        .iter()
        .map(|value| {
            serde_json::from_str(value)
                .map_err(|error| VoiceprintOperationError::MetadataJson(error.to_string()))
        })
        .collect()
}

fn serialize_metadata(value: &Value) -> Result<String, VoiceprintOperationError> {
    serde_json::to_string(value)
        .map_err(|error| VoiceprintOperationError::MetadataJson(error.to_string()))
}

fn removal_key(removal: &VoiceprintRemoval) -> Result<VoiceprintKey, VoiceprintOperationError> {
    if !removal.key.is_object() {
        return Err(VoiceprintOperationError::InvalidRemovalKey);
    }
    key_from_metadata_value(&removal.key)
}

fn key_from_metadata_value(value: &Value) -> Result<VoiceprintKey, VoiceprintOperationError> {
    let object = value
        .as_object()
        .ok_or(VoiceprintOperationError::MetadataNotObject)?;
    Ok(VoiceprintKey([
        canonical_key_field(object.get("day"), "day")?,
        canonical_key_field(object.get("segment_key"), "segment_key")?,
        canonical_key_field(object.get("source"), "source")?,
        canonical_key_field(object.get("sentence_id"), "sentence_id")?,
    ]))
}

fn canonical_key_field(
    value: Option<&Value>,
    field: &'static str,
) -> Result<CanonicalKeyField, VoiceprintOperationError> {
    match value {
        None | Some(Value::Null) => Ok(CanonicalKeyField::Absent),
        Some(Value::Bool(value)) => Ok(CanonicalKeyField::Bool(*value)),
        Some(Value::String(value)) => Ok(CanonicalKeyField::Str(value.clone())),
        Some(Value::Number(value)) => canonical_number(value),
        Some(Value::Array(_)) | Some(Value::Object(_)) => {
            Err(VoiceprintOperationError::UnsupportedKeyField { field })
        }
    }
}

fn canonical_number(value: &Number) -> Result<CanonicalKeyField, VoiceprintOperationError> {
    if let Some(integer) = integer_value(value) {
        return Ok(CanonicalKeyField::Int(integer));
    }
    let float = value.as_f64().ok_or_else(|| {
        VoiceprintOperationError::MetadataJson(
            "voiceprint key number is not representable".to_owned(),
        )
    })?;
    if let Some(integer) = float_to_integer(float) {
        return Ok(CanonicalKeyField::Int(integer));
    }
    let bits = if float == 0.0 {
        0.0_f64.to_bits()
    } else {
        float.to_bits()
    };
    Ok(CanonicalKeyField::Float(bits))
}

fn mark_all_missing(report: &mut VoiceprintRemovalReport, count: usize) {
    report.skipped_reasons.missing = count;
    report.skipped_count = count;
}

pub(crate) fn read_voiceprints_npz(bytes: &[u8]) -> Result<VoiceprintArchive, VoiceprintNpzError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        VoiceprintNpzError::Archive(format!("invalid voiceprint archive: {error}"))
    })?;
    let mut names = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|file| file.name().to_owned())
                .map_err(|error| {
                    VoiceprintNpzError::Archive(format!("invalid voiceprint archive: {error}"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    if names
        .iter()
        .filter(|name| name.as_str() == EMBEDDINGS_MEMBER)
        .count()
        != 1
        || names
            .iter()
            .filter(|name| name.as_str() == METADATA_MEMBER)
            .count()
            != 1
    {
        return Err(VoiceprintNpzError::Invalid(
            "voiceprint archive must contain exactly one embeddings.npy and metadata.npy"
                .to_owned(),
        ));
    }
    let embeddings = read_member(&mut archive, EMBEDDINGS_MEMBER)?;
    let metadata = read_member(&mut archive, METADATA_MEMBER)?;
    let (embedding_rows, values) = parse_embeddings(&embeddings)?;
    let (metadata_rows, metadata) = parse_metadata(&metadata)?;
    if embedding_rows != metadata_rows {
        return Err(VoiceprintNpzError::Invalid(
            "voiceprint embedding and metadata row counts differ".to_owned(),
        ));
    }
    let envelope_count = names
        .iter()
        .filter(|name| name.as_str() == ENVELOPE_MEMBER)
        .count();
    let envelope = if envelope_count == 1 {
        read_member(&mut archive, ENVELOPE_MEMBER)
            .ok()
            .map_or_else(VoiceprintEnvelope::default, |bytes| parse_envelope(&bytes))
    } else {
        VoiceprintEnvelope::default()
    };
    let unrecognized_members = names
        .into_iter()
        .filter(|name| {
            name != EMBEDDINGS_MEMBER && name != METADATA_MEMBER && name != ENVELOPE_MEMBER
        })
        .collect();
    Ok(VoiceprintArchive {
        embeddings: values,
        rows: embedding_rows,
        metadata,
        envelope,
        unrecognized_members,
    })
}

pub(crate) fn write_voiceprints_npz(
    embeddings: &[f32],
    metadata: &[String],
    prior_envelope: &VoiceprintEnvelope,
    selected_encoder: &EncoderIdentity,
) -> Result<Vec<u8>, VoiceprintNpzError> {
    let expected_embeddings = metadata.len().checked_mul(EMBEDDING_WIDTH).ok_or_else(|| {
        VoiceprintNpzError::Invalid("voiceprint row count is too large".to_owned())
    })?;
    if embeddings.len() != expected_embeddings {
        return Err(VoiceprintNpzError::Invalid(format!(
            "voiceprint embeddings length {} does not match {} rows",
            embeddings.len(),
            metadata.len()
        )));
    }
    validate_metadata(metadata)?;
    let embeddings = write_embeddings_npy(embeddings, metadata.len());
    let metadata = write_metadata_npy(metadata)?;
    let envelope = write_envelope_npy(&stamped_envelope(prior_envelope, selected_encoder))?;
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    writer
        .start_file(EMBEDDINGS_MEMBER, options)
        .map_err(zip_error)?;
    writer.write_all(&embeddings).map_err(io_error)?;
    writer
        .start_file(METADATA_MEMBER, options)
        .map_err(zip_error)?;
    writer.write_all(&metadata).map_err(io_error)?;
    writer
        .start_file(ENVELOPE_MEMBER, options)
        .map_err(zip_error)?;
    writer.write_all(&envelope).map_err(io_error)?;
    writer
        .finish()
        .map_err(zip_error)
        .map(|cursor| cursor.into_inner())
}

fn read_member(
    archive: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Vec<u8>, VoiceprintNpzError> {
    let mut member = archive.by_name(name).map_err(zip_error)?;
    let mut bytes = Vec::new();
    member.read_to_end(&mut bytes).map_err(io_error)?;
    Ok(bytes)
}

fn parse_embeddings(bytes: &[u8]) -> Result<(usize, Vec<f32>), VoiceprintNpzError> {
    let NpyBlob {
        descr,
        fortran_order,
        shape,
        payload,
    } = parse_npy(bytes).map_err(|error| VoiceprintNpzError::Invalid(error.to_string()))?;
    if shape.len() == 2 && shape[1] != EMBEDDING_WIDTH {
        return Err(VoiceprintNpzError::EmbeddingWidth { found: shape[1] });
    }
    if descr != "<f4" || fortran_order || shape.len() != 2 {
        return Err(VoiceprintNpzError::Invalid(
            "embeddings.npy must be a little-endian float32 C-order (N, 256) array".to_owned(),
        ));
    }
    let rows = shape[0];
    let values = payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
        .collect();
    Ok((rows, values))
}

fn parse_metadata(bytes: &[u8]) -> Result<(usize, Vec<String>), VoiceprintNpzError> {
    let values = parse_unicode_npy(bytes, METADATA_MEMBER)?;
    validate_metadata(&values)?;
    Ok((values.len(), values))
}

fn write_embeddings_npy(values: &[f32], rows: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(values.len() * 4);
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    write_npy("<f4", &format!("({rows}, {EMBEDDING_WIDTH})"), &payload)
}

fn write_metadata_npy(values: &[String]) -> Result<Vec<u8>, VoiceprintNpzError> {
    Ok(write_unicode_npy(values))
}

fn write_unicode_npy(values: &[String]) -> Vec<u8> {
    let width = values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0);
    let mut payload = Vec::new();
    for value in values {
        for character in value.chars() {
            payload.extend_from_slice(&(character as u32).to_le_bytes());
        }
        for _ in value.chars().count()..width {
            payload.extend_from_slice(&0_u32.to_le_bytes());
        }
    }
    write_npy(
        &format!("<U{width}"),
        &format!("({},)", values.len()),
        &payload,
    )
}

fn parse_unicode_npy(bytes: &[u8], name: &str) -> Result<Vec<String>, VoiceprintNpzError> {
    let NpyBlob {
        descr,
        fortran_order,
        shape,
        payload,
    } = parse_npy(bytes).map_err(|error| VoiceprintNpzError::Invalid(error.to_string()))?;
    if fortran_order || shape.len() != 1 {
        return Err(VoiceprintNpzError::Invalid(format!(
            "{name} must be a C-order one-dimensional unicode array"
        )));
    }
    let width = descr
        .strip_prefix("<U")
        .ok_or_else(|| {
            VoiceprintNpzError::Invalid(format!(
                "{name} must use a little-endian unicode dtype, never pickle or object values"
            ))
        })?
        .parse::<usize>()
        .map_err(|_| VoiceprintNpzError::Invalid(format!("{name} has an invalid unicode dtype")))?;
    let rows = shape[0];
    if width == 0 {
        if rows == 0 && payload.is_empty() {
            return Ok(Vec::new());
        }
        return Err(VoiceprintNpzError::Invalid(format!(
            "{name} has an invalid zero-width unicode dtype"
        )));
    }
    let row_width = width
        .checked_mul(4)
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("{name} unicode dtype is too large")))?;
    payload
        .chunks_exact(row_width)
        .map(|row| {
            row.chunks_exact(4)
                .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
                .take_while(|codepoint| *codepoint != 0)
                .map(|codepoint| {
                    char::from_u32(codepoint).ok_or_else(|| {
                        VoiceprintNpzError::Invalid(format!(
                            "{name} contains an invalid unicode code point"
                        ))
                    })
                })
                .collect::<Result<String, _>>()
        })
        .collect()
}

fn stamped_envelope(
    prior_envelope: &VoiceprintEnvelope,
    selected_encoder: &EncoderIdentity,
) -> VoiceprintEnvelope {
    VoiceprintEnvelope {
        version: CURRENT_ENVELOPE_VERSION,
        encoder: Some(selected_encoder.clone()),
        extra: prior_envelope.extra.clone(),
    }
}

fn write_envelope_npy(envelope: &VoiceprintEnvelope) -> Result<Vec<u8>, VoiceprintNpzError> {
    let encoder = envelope.encoder.as_ref().ok_or_else(|| {
        VoiceprintNpzError::Invalid("voiceprint envelope writer requires an encoder".to_owned())
    })?;
    let mut value = envelope.extra.clone();
    value.insert(
        "format".to_owned(),
        Value::String(ENVELOPE_FORMAT.to_owned()),
    );
    value.insert("version".to_owned(), Value::from(envelope.version));
    value.insert("encoder".to_owned(), Value::String(encoder.id.clone()));
    value.insert(
        "encoder_sha256".to_owned(),
        Value::String(encoder.sha256.clone()),
    );
    value.insert("width".to_owned(), Value::from(encoder.width));
    let serialized = serde_json::to_string(&Value::Object(value)).map_err(|error| {
        VoiceprintNpzError::Invalid(format!("voiceprint envelope cannot be serialized: {error}"))
    })?;
    Ok(write_unicode_npy(&[serialized]))
}

fn parse_envelope(bytes: &[u8]) -> VoiceprintEnvelope {
    let Ok(values) = parse_unicode_npy(bytes, ENVELOPE_MEMBER) else {
        return VoiceprintEnvelope::default();
    };
    let [serialized] = values.as_slice() else {
        return VoiceprintEnvelope::default();
    };
    let Ok(Value::Object(mut value)) = serde_json::from_str(serialized) else {
        return VoiceprintEnvelope::default();
    };
    let Some(Value::String(format)) = value.remove("format") else {
        return VoiceprintEnvelope::default();
    };
    if format != ENVELOPE_FORMAT {
        return VoiceprintEnvelope::default();
    }
    let Some(version) = value
        .remove("version")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
    else {
        return VoiceprintEnvelope::default();
    };
    let Some(Value::String(id)) = value.remove("encoder") else {
        return VoiceprintEnvelope::default();
    };
    let Some(Value::String(sha256)) = value.remove("encoder_sha256") else {
        return VoiceprintEnvelope::default();
    };
    let Some(width) = value
        .remove("width")
        .and_then(|value| value.as_u64())
        .and_then(|value| usize::try_from(value).ok())
    else {
        return VoiceprintEnvelope::default();
    };
    if id.is_empty() || sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return VoiceprintEnvelope::default();
    }
    VoiceprintEnvelope {
        version,
        encoder: Some(EncoderIdentity { id, sha256, width }),
        extra: value,
    }
}

fn validate_metadata(values: &[String]) -> Result<(), VoiceprintNpzError> {
    for value in values {
        serde_json::from_str::<serde_json::Value>(value).map_err(|error| {
            VoiceprintNpzError::Invalid(format!("voiceprint metadata must be JSON: {error}"))
        })?;
    }
    Ok(())
}

fn zip_error(error: zip::result::ZipError) -> VoiceprintNpzError {
    VoiceprintNpzError::Archive(format!("voiceprint archive error: {error}"))
}

fn io_error(error: std::io::Error) -> VoiceprintNpzError {
    VoiceprintNpzError::Archive(format!("voiceprint archive I/O error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_encoder() -> EncoderIdentity {
        EncoderIdentity {
            id: "test-encoder".to_owned(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            width: EMBEDDING_WIDTH,
        }
    }

    #[test]
    fn round_trips_embeddings_and_unicode_json_metadata() {
        let embeddings = (0..EMBEDDING_WIDTH * 2)
            .map(|index| index as f32 / 10.0)
            .collect::<Vec<_>>();
        let metadata = vec![
            r#"{"day":"20260101","source":"screen"}"#.to_owned(),
            r#"{"day":"20260102","label":"José"}"#.to_owned(),
        ];

        let bytes = write_voiceprints_npz(
            &embeddings,
            &metadata,
            &VoiceprintEnvelope::default(),
            &test_encoder(),
        )
        .unwrap();
        let actual = read_voiceprints_npz(&bytes).unwrap();

        assert_eq!(
            actual
                .embeddings
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            embeddings
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(actual.rows, 2);
        assert_eq!(actual.metadata, metadata);
    }

    #[test]
    fn metadata_npy_bytes_match_the_literal_legacy_fixture() {
        const FIXTURE_HEX: &str = "934e554d5059010076007b276465736372273a20273c5532272c2027666f727472616e5f6f72646572273a2046616c73652c20277368617065273a2028312c292c207d2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020200a7b0000007d000000";
        let expected = (0..FIXTURE_HEX.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&FIXTURE_HEX[index..index + 2], 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(write_metadata_npy(&["{}".to_owned()]).unwrap(), expected);
    }

    #[test]
    fn rejects_object_metadata_dtype() {
        let bytes = write_npy("|O", "(1,)", &[0; 8]);
        assert!(matches!(
            parse_metadata(&bytes),
            Err(VoiceprintNpzError::Invalid(_))
        ));
    }

    #[test]
    fn voiceprint_embeddings_reject_exact_length_mismatches() {
        let mut payload = vec![0_u8; EMBEDDING_WIDTH * 4];
        payload.push(0);
        let overlong = write_npy("<f4", "(1, 256)", &payload);
        assert!(parse_embeddings(&overlong).is_err());

        let short = write_npy("<f4", "(1, 256)", &payload[..payload.len() - 2]);
        assert!(parse_embeddings(&short).is_err());
    }

    #[test]
    fn voiceprint_embeddings_reject_scalar_shape_at_the_domain_layer() {
        let scalar = write_npy("<f4", "()", &0_f32.to_le_bytes());
        assert!(matches!(
            parse_embeddings(&scalar),
            Err(VoiceprintNpzError::Invalid(message)) if message.contains("(N, 256)")
        ));
    }

    #[test]
    fn round_trips_empty_voiceprints() {
        let bytes =
            write_voiceprints_npz(&[], &[], &VoiceprintEnvelope::default(), &test_encoder())
                .unwrap();
        assert_eq!(
            read_voiceprints_npz(&bytes).unwrap(),
            VoiceprintArchive {
                embeddings: Vec::new(),
                rows: 0,
                metadata: Vec::new(),
                envelope: stamped_envelope(&VoiceprintEnvelope::default(), &test_encoder()),
                unrecognized_members: Vec::new(),
            }
        );
    }
}
