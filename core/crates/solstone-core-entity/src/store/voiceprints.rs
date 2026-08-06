// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeSet, HashSet};
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
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::entity_paths::entity_memory_path;
use super::lifecycle::EntityLifecycleError;
use super::reconcile::{float_to_integer, integer_value, python_optional_json_equal};

const EMBEDDING_WIDTH: usize = 256;
const EMBEDDINGS_MEMBER: &str = "embeddings.npy";
const METADATA_MEMBER: &str = "metadata.npy";

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceprintArchive {
    pub embeddings: Vec<f32>,
    pub rows: usize,
    pub metadata: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceprintNpzError {
    Archive(String),
    Invalid(String),
}

impl fmt::Display for VoiceprintNpzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(message) | Self::Invalid(message) => formatter.write_str(message),
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

/// One requested row removal with its expected complete metadata value.
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
    UnsupportedKeyField { field: &'static str },
    DuplicateExactMatch,
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
            | Self::DuplicateExactMatch => None,
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
    let Ok((_directory, path)) = resolve_voiceprint_path(journal_root, entity_id, false) else {
        return None;
    };
    match load_voiceprints(&path) {
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
) -> Result<usize, VoiceprintOperationError> {
    if new_items.is_empty() {
        return Ok(0);
    }
    let (_directory, path) = resolve_voiceprint_path(journal_root, entity_id, true)?;
    let _lock = hold_lock(&path, LockOptions::default())?;
    let mut archive = load_voiceprints(&path)?.unwrap_or_else(empty_archive);
    for item in new_items {
        archive.embeddings.extend_from_slice(&item.embedding);
        archive.metadata.push(serialize_metadata(&item.metadata)?);
    }
    archive.rows = archive.metadata.len();
    write_and_verify_voiceprints(&path, &archive)?;
    Ok(new_items.len())
}

/// Rewrite metadata in place only when the mutator reports changes.
pub fn rewrite_voiceprint_metadata<F>(
    journal_root: &Path,
    entity_id: &str,
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
    let mut metadata = parse_metadata_values(&archive.metadata)?;
    let updates = mutator(&mut metadata);
    if updates == 0 {
        return Ok(0);
    }
    archive.metadata = metadata
        .iter()
        .map(serialize_metadata)
        .collect::<Result<_, _>>()?;
    write_and_verify_voiceprints(&path, &archive)?;
    Ok(updates)
}

/// Remove rows by exact Python-equality-compatible key and metadata match.
pub fn remove_voiceprints_by_key(
    journal_root: &Path,
    entity_id: &str,
    removals: &[VoiceprintRemoval],
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
    let metadata = parse_metadata_values(&archive.metadata)?;
    let keys = archive
        .metadata
        .iter()
        .map(|value| voiceprint_removal_key(value))
        .collect::<Result<Vec<_>, _>>()?;
    let mut remove_indexes = HashSet::new();
    for (removal, key) in removals.iter().zip(&normalized_removals) {
        let exact_matches = metadata
            .iter()
            .enumerate()
            .filter_map(|(index, stored)| {
                (keys[index] == *key
                    && python_optional_json_equal(Some(stored), removal.expected_metadata.as_ref()))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        if exact_matches.len() > 1 {
            return Err(VoiceprintOperationError::DuplicateExactMatch);
        }
        if let Some(index) = exact_matches.first() {
            remove_indexes.insert(*index);
            continue;
        }
        if keys.iter().any(|stored_key| stored_key == key) {
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
    write_and_verify_voiceprints(&path, &kept)?;
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

fn resolve_voiceprint_path(
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
) -> Result<(), VoiceprintOperationError> {
    let bytes = write_voiceprints_npz(&archive.embeddings, &archive.metadata)?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default())?;
    let verified = load_voiceprints(path)?.ok_or_else(|| {
        VoiceprintOperationError::Npz(VoiceprintNpzError::Invalid(
            "voiceprint archive disappeared after write".to_owned(),
        ))
    })?;
    if verified != *archive {
        return Err(VoiceprintOperationError::Npz(VoiceprintNpzError::Invalid(
            "voiceprint archive changed after write".to_owned(),
        )));
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
    let names = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|file| file.name().to_owned())
                .map_err(|error| {
                    VoiceprintNpzError::Archive(format!("invalid voiceprint archive: {error}"))
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected = BTreeSet::from([EMBEDDINGS_MEMBER.to_owned(), METADATA_MEMBER.to_owned()]);
    if names != expected || archive.len() != expected.len() {
        return Err(VoiceprintNpzError::Invalid(
            "voiceprint archive must contain exactly embeddings.npy and metadata.npy".to_owned(),
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
    Ok(VoiceprintArchive {
        embeddings: values,
        rows: embedding_rows,
        metadata,
    })
}

pub(crate) fn write_voiceprints_npz(
    embeddings: &[f32],
    metadata: &[String],
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
    let (header, payload) = parse_npy(bytes)?;
    if header.descr != "<f4"
        || header.fortran_order
        || header.shape.len() != 2
        || header.shape[1] != EMBEDDING_WIDTH
    {
        return Err(VoiceprintNpzError::Invalid(
            "embeddings.npy must be a little-endian float32 C-order (N, 256) array".to_owned(),
        ));
    }
    let rows = header.shape[0];
    let expected = rows
        .checked_mul(EMBEDDING_WIDTH)
        .and_then(|count| count.checked_mul(4))
        .ok_or_else(|| {
            VoiceprintNpzError::Invalid("embeddings.npy shape is too large".to_owned())
        })?;
    if payload.len() != expected {
        return Err(VoiceprintNpzError::Invalid(
            "embeddings.npy payload length does not match its shape".to_owned(),
        ));
    }
    let values = payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
        .collect();
    Ok((rows, values))
}

fn parse_metadata(bytes: &[u8]) -> Result<(usize, Vec<String>), VoiceprintNpzError> {
    let (header, payload) = parse_npy(bytes)?;
    if header.fortran_order || header.shape.len() != 1 {
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy must be a C-order one-dimensional unicode array".to_owned(),
        ));
    }
    let width = header
        .descr
        .strip_prefix("<U")
        .ok_or_else(|| {
            VoiceprintNpzError::Invalid(
                "metadata.npy must use a little-endian unicode dtype, never pickle or object values"
                    .to_owned(),
            )
        })?
        .parse::<usize>()
        .map_err(|_| {
            VoiceprintNpzError::Invalid("metadata.npy has an invalid unicode dtype".to_owned())
        })?;
    let rows = header.shape[0];
    if width == 0 {
        if rows == 0 && payload.is_empty() {
            return Ok((0, Vec::new()));
        }
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy has an invalid zero-width unicode dtype".to_owned(),
        ));
    }
    let row_width = width.checked_mul(4).ok_or_else(|| {
        VoiceprintNpzError::Invalid("metadata.npy unicode dtype is too large".to_owned())
    })?;
    let expected = rows
        .checked_mul(row_width)
        .ok_or_else(|| VoiceprintNpzError::Invalid("metadata.npy shape is too large".to_owned()))?;
    if payload.len() != expected {
        return Err(VoiceprintNpzError::Invalid(
            "metadata.npy payload length does not match its shape".to_owned(),
        ));
    }
    let mut values = Vec::with_capacity(rows);
    for row in payload.chunks_exact(row_width) {
        let value = row
            .chunks_exact(4)
            .map(|bytes| u32::from_le_bytes(bytes.try_into().expect("exact chunk length")))
            .take_while(|codepoint| *codepoint != 0)
            .map(|codepoint| {
                char::from_u32(codepoint).ok_or_else(|| {
                    VoiceprintNpzError::Invalid(
                        "metadata.npy contains an invalid unicode code point".to_owned(),
                    )
                })
            })
            .collect::<Result<String, _>>()?;
        values.push(value);
    }
    validate_metadata(&values)?;
    Ok((rows, values))
}

fn write_embeddings_npy(values: &[f32], rows: usize) -> Vec<u8> {
    let mut payload = Vec::with_capacity(values.len() * 4);
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    write_npy("<f4", &format!("({rows}, {EMBEDDING_WIDTH})"), &payload)
}

fn write_metadata_npy(values: &[String]) -> Result<Vec<u8>, VoiceprintNpzError> {
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
    Ok(write_npy(
        &format!("<U{width}"),
        &format!("({},)", values.len()),
        &payload,
    ))
}

fn write_npy(descr: &str, shape: &str, payload: &[u8]) -> Vec<u8> {
    let mut header = format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
    let padding = (64 - ((10 + header.len() + 1) % 64)) % 64;
    header.push_str(&" ".repeat(padding));
    header.push('\n');
    let mut bytes = Vec::with_capacity(10 + header.len() + payload.len());
    bytes.extend_from_slice(b"\x93NUMPY");
    bytes.extend_from_slice(&[1, 0]);
    bytes.extend_from_slice(&(header.len() as u16).to_le_bytes());
    bytes.extend_from_slice(header.as_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

struct NpyHeader {
    descr: String,
    fortran_order: bool,
    shape: Vec<usize>,
}

fn parse_npy(bytes: &[u8]) -> Result<(NpyHeader, &[u8]), VoiceprintNpzError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(VoiceprintNpzError::Invalid(
            "invalid NPY magic bytes".to_owned(),
        ));
    }
    let version = (bytes[6], bytes[7]);
    let (header_start, header_len): (usize, usize) = match version {
        (1, 0) => (10, u16::from_le_bytes([bytes[8], bytes[9]]) as usize),
        (2, 0) | (3, 0) if bytes.len() >= 12 => (
            12,
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
        ),
        _ => {
            return Err(VoiceprintNpzError::Invalid(
                "unsupported NPY version".to_owned(),
            ));
        }
    };
    let header_end = header_start
        .checked_add(header_len)
        .ok_or_else(|| VoiceprintNpzError::Invalid("NPY header is too large".to_owned()))?;
    let header = bytes
        .get(header_start..header_end)
        .ok_or_else(|| VoiceprintNpzError::Invalid("truncated NPY header".to_owned()))?;
    let header = std::str::from_utf8(header)
        .map_err(|_| VoiceprintNpzError::Invalid("NPY header is not UTF-8".to_owned()))?;
    let descr = header_string_value(header, "descr")?;
    let fortran_order = match header_value(header, "fortran_order")? {
        "False" => false,
        "True" => true,
        _ => {
            return Err(VoiceprintNpzError::Invalid(
                "NPY header has an invalid fortran_order value".to_owned(),
            ));
        }
    };
    let shape_text = header_value(header, "shape")?;
    let shape = shape_text
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| VoiceprintNpzError::Invalid("NPY header has an invalid shape".to_owned()))?
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            (!part.is_empty()).then_some(part)
        })
        .map(|part| {
            part.parse::<usize>().map_err(|_| {
                VoiceprintNpzError::Invalid("NPY header has an invalid shape dimension".to_owned())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if shape.is_empty() {
        return Err(VoiceprintNpzError::Invalid(
            "NPY header must describe an array".to_owned(),
        ));
    }
    Ok((
        NpyHeader {
            descr,
            fortran_order,
            shape,
        },
        &bytes[header_end..],
    ))
}

fn header_string_value(header: &str, key: &str) -> Result<String, VoiceprintNpzError> {
    let value = header_value(header, key)?;
    let value = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}")))?;
    Ok(value.to_owned())
}

fn header_value<'a>(header: &'a str, key: &str) -> Result<&'a str, VoiceprintNpzError> {
    let prefix = format!("'{key}':");
    let value = header
        .split(&prefix)
        .nth(1)
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header is missing {key}")))?
        .trim_start();
    if value.starts_with('(') {
        let end = value.find(')').ok_or_else(|| {
            VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}"))
        })?;
        return Ok(&value[..=end]);
    }
    Ok(value
        .split(',')
        .next()
        .ok_or_else(|| VoiceprintNpzError::Invalid(format!("NPY header has an invalid {key}")))?
        .trim())
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

    #[test]
    fn round_trips_embeddings_and_unicode_json_metadata() {
        let embeddings = (0..EMBEDDING_WIDTH * 2)
            .map(|index| index as f32 / 10.0)
            .collect::<Vec<_>>();
        let metadata = vec![
            r#"{"day":"20260101","source":"screen"}"#.to_owned(),
            r#"{"day":"20260102","label":"José"}"#.to_owned(),
        ];

        let bytes = write_voiceprints_npz(&embeddings, &metadata).unwrap();
        let actual = read_voiceprints_npz(&bytes).unwrap();

        assert_eq!(actual.embeddings, embeddings);
        assert_eq!(actual.rows, 2);
        assert_eq!(actual.metadata, metadata);
    }

    #[test]
    fn rejects_object_metadata_dtype() {
        let bytes = write_npy("|O", "(1,)", &[0; 8]);
        assert!(matches!(
            parse_metadata(&bytes),
            Err(VoiceprintNpzError::Invalid(message)) if message.contains("never pickle")
        ));
    }

    #[test]
    fn round_trips_empty_voiceprints() {
        let bytes = write_voiceprints_npz(&[], &[]).unwrap();
        assert_eq!(
            read_voiceprints_npz(&bytes).unwrap(),
            VoiceprintArchive {
                embeddings: Vec::new(),
                rows: 0,
                metadata: Vec::new(),
            }
        );
    }
}
