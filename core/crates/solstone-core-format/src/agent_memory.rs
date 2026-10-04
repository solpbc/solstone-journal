// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared, credential-free format for private connection-owned agent memory.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SOURCE_DOMAIN: &str = "solstone-agent-memory-source-v1\n";
pub const MAX_NOTE_BYTES: usize = 32_768;
pub const MAX_METADATA_BYTES: usize = 16_384;

/// Stable, non-reversible identifier for one verified connection identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SourceKey(String);

impl SourceKey {
    /// Derive a source key from the exact verified identity bytes.
    #[must_use]
    pub fn from_verified_id(verified_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(SOURCE_DOMAIN.as_bytes());
        hasher.update(verified_id.as_bytes());
        Self(format!("sha256:{:x}", hasher.finalize()))
    }

    /// Validate the canonical `sha256:` plus lowercase hexadecimal spelling.
    pub fn parse(value: impl Into<String>) -> Result<Self, FormatError> {
        let value = value.into();
        let hex = value
            .strip_prefix("sha256:")
            .ok_or(FormatError::InvalidSourceKey)?;
        if !is_lower_hex_64(hex) {
            return Err(FormatError::InvalidSourceKey);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The safe 64-character component used in paths and stream bindings.
    #[must_use]
    pub fn component(&self) -> &str {
        // Construction and deserialization validation are separate: callers
        // should call `validate` before using an untrusted deserialized value.
        self.0.strip_prefix("sha256:").unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), FormatError> {
        Self::parse(self.0.clone()).map(|_| ())
    }
}

/// Exact location of one memory note in the chronicle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Coordinate {
    pub day: String,
    pub stream: String,
    pub segment: String,
}

/// Credential-free origin stored beside a memory note.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Origin {
    /// The server-owned kind of this immutable origin.
    pub kind: OriginKind,
    pub source_key: SourceKey,
    /// Trusted label captured at creation; later connection renames do not amend it.
    pub creation_label: String,
    pub created_at: DateTime<Utc>,
    pub stream: String,
    pub segment: String,
}

/// The origin kind for a private connection-owned original.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    AgentMemory,
}

impl Origin {
    #[must_use]
    pub fn coordinate(&self, day: impl Into<String>) -> Coordinate {
        Coordinate {
            day: day.into(),
            stream: self.stream.clone(),
            segment: self.segment.clone(),
        }
    }
}

/// State recorded as work moves from reservation through publication.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Reserved,
    Noted,
    Chained,
    Ready,
}

/// The predecessor pair and sequence committed by one stream advancement.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChainPredecessor {
    pub prev_day: Option<String>,
    pub prev_segment: Option<String>,
    pub seq: u64,
}

/// Durable idempotency record for a note append.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OperationRecord {
    pub operation_id: String,
    pub digest: String,
    pub byte_count: usize,
    pub created_at: DateTime<Utc>,
    /// Server-authored origin kind, retained with recovery state.
    pub origin_kind: OriginKind,
    /// Trusted creation label captured when this operation was first reserved.
    pub creation_label: String,
    pub coordinate: Coordinate,
    pub phase: Readiness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainPredecessor>,
}

/// Immutable readiness marker for one complete memory original.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReadyDocument {
    pub source_key: SourceKey,
    pub origin: Origin,
    pub coordinate: Coordinate,
    pub created_at: DateTime<Utc>,
    pub digest: String,
    pub byte_count: usize,
    pub chain: ChainPredecessor,
}

/// Format validation failure. The wording deliberately avoids audit outcome tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FormatError {
    InvalidSourceKey,
    InvalidOperationId,
    InvalidDigest,
    DigestMismatch,
    ByteCountMismatch,
    InvalidCoordinate,
    InvalidChain,
    BindingMismatch,
    InvalidCreationLabel,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSourceKey => "invalid memory source key",
            Self::InvalidOperationId => "invalid memory operation identifier",
            Self::InvalidDigest => "invalid memory digest",
            Self::DigestMismatch => "memory digest does not match bytes",
            Self::ByteCountMismatch => "memory byte count does not match bytes",
            Self::InvalidCoordinate => "invalid memory coordinate",
            Self::InvalidChain => "invalid memory chain position",
            Self::BindingMismatch => "memory record binding does not match",
            Self::InvalidCreationLabel => "invalid memory creation label",
        })
    }
}

impl std::error::Error for FormatError {}

#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn validate_operation_id(value: &str) -> Result<(), FormatError> {
    let bytes = value.as_bytes();
    if !(1..=128).contains(&bytes.len())
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(FormatError::InvalidOperationId);
    }
    Ok(())
}

/// Creation labels are server-authored descriptive values, never credentials.
pub fn validate_creation_label(value: &str) -> Result<(), FormatError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(FormatError::InvalidCreationLabel);
    }
    Ok(())
}

pub fn validate_record(
    record: &OperationRecord,
    source_key: &SourceKey,
    operation_id: &str,
    bytes: &[u8],
) -> Result<(), FormatError> {
    validate_record_header(record, source_key)?;
    validate_operation_id(operation_id)?;
    if record.operation_id != operation_id {
        return Err(FormatError::BindingMismatch);
    }
    if record.digest != digest(bytes) {
        return Err(FormatError::DigestMismatch);
    }
    if record.byte_count != bytes.len() {
        return Err(FormatError::ByteCountMismatch);
    }
    Ok(())
}

/// Construct the one immutable readiness marker after stream advancement.
pub fn ready_document(
    record: &OperationRecord,
    source_key: &SourceKey,
) -> Result<ReadyDocument, FormatError> {
    validate_record_header(record, source_key)?;
    let chain = record.chain.clone().ok_or(FormatError::InvalidChain)?;
    if record.phase != Readiness::Chained && record.phase != Readiness::Ready {
        return Err(FormatError::InvalidChain);
    }
    let origin = Origin {
        kind: record.origin_kind,
        source_key: source_key.clone(),
        creation_label: record.creation_label.clone(),
        created_at: record.created_at,
        stream: record.coordinate.stream.clone(),
        segment: record.coordinate.segment.clone(),
    };
    Ok(ReadyDocument {
        source_key: source_key.clone(),
        origin,
        coordinate: record.coordinate.clone(),
        created_at: record.created_at,
        digest: record.digest.clone(),
        byte_count: record.byte_count,
        chain,
    })
}

/// Independently validate an immutable readiness marker against the original bytes.
pub fn validate_ready_document(
    document: &ReadyDocument,
    source_key: &SourceKey,
    bytes: &[u8],
) -> Result<(), FormatError> {
    source_key.validate()?;
    if bytes.is_empty() || bytes.len() > MAX_NOTE_BYTES {
        return Err(FormatError::ByteCountMismatch);
    }
    if &document.source_key != source_key
        || document.digest != digest(bytes)
        || document.byte_count != bytes.len()
    {
        return Err(FormatError::BindingMismatch);
    }
    validate_coordinate(&document.coordinate, source_key)?;
    validate_origin(&document.origin, source_key)?;
    validate_chain(&document.chain, &document.coordinate)?;
    if document.origin.kind != OriginKind::AgentMemory
        || document.origin.created_at != document.created_at
        || document.origin.coordinate(&document.coordinate.day) != document.coordinate
    {
        return Err(FormatError::BindingMismatch);
    }
    Ok(())
}

/// Bind readiness to the independently read source coordinate, origin and marker.
/// An internally consistent readiness document alone does not prove these bindings.
pub fn validate_complete_original(
    document: &ReadyDocument,
    source_key: &SourceKey,
    coordinate: &Coordinate,
    origin: &Origin,
    chain: &ChainPredecessor,
    bytes: &[u8],
) -> Result<(), FormatError> {
    validate_ready_document(document, source_key, bytes)?;
    if &document.coordinate != coordinate || &document.origin != origin || &document.chain != chain
    {
        return Err(FormatError::BindingMismatch);
    }
    Ok(())
}

/// Validate recovery metadata before its note bytes are available.
pub fn validate_record_header(
    record: &OperationRecord,
    source_key: &SourceKey,
) -> Result<(), FormatError> {
    source_key.validate()?;
    validate_operation_id(&record.operation_id)?;
    if !is_lower_hex_64(&record.digest) {
        return Err(FormatError::InvalidDigest);
    }
    if record.byte_count == 0 || record.byte_count > MAX_NOTE_BYTES {
        return Err(FormatError::ByteCountMismatch);
    }
    validate_creation_label(&record.creation_label)?;
    validate_coordinate(&record.coordinate, source_key)?;
    if let Some(chain) = &record.chain {
        validate_chain(chain, &record.coordinate)?;
    }
    if (record.phase == Readiness::Chained || record.phase == Readiness::Ready)
        != record.chain.is_some()
    {
        return Err(FormatError::InvalidChain);
    }
    Ok(())
}

pub fn validate_coordinate(
    coordinate: &Coordinate,
    source_key: &SourceKey,
) -> Result<(), FormatError> {
    source_key.validate()?;
    let stream = format!("agent-memory-{}", source_key.component());
    if coordinate.stream != stream
        || !valid_day(&coordinate.day)
        || !valid_segment(&coordinate.segment)
    {
        return Err(FormatError::InvalidCoordinate);
    }
    Ok(())
}

pub fn validate_origin(origin: &Origin, source_key: &SourceKey) -> Result<(), FormatError> {
    source_key.validate()?;
    origin.source_key.validate()?;
    validate_creation_label(&origin.creation_label)?;
    if &origin.source_key != source_key
        || origin.kind != OriginKind::AgentMemory
        || origin.stream != format!("agent-memory-{}", source_key.component())
        || !valid_segment(&origin.segment)
    {
        return Err(FormatError::BindingMismatch);
    }
    Ok(())
}

fn validate_chain(chain: &ChainPredecessor, coordinate: &Coordinate) -> Result<(), FormatError> {
    if chain.seq == 0
        || (chain.seq == 1) != chain.prev_day.is_none()
        || chain.prev_day.is_some() != chain.prev_segment.is_some()
        || chain.prev_day.as_deref().is_some_and(|day| !valid_day(day))
        || chain
            .prev_segment
            .as_deref()
            .is_some_and(|segment| !valid_segment(segment))
        || (chain.prev_day.as_deref() == Some(coordinate.day.as_str())
            && chain.prev_segment.as_deref() == Some(coordinate.segment.as_str()))
    {
        return Err(FormatError::InvalidChain);
    }
    Ok(())
}

fn valid_segment(value: &str) -> bool {
    let Some((time, ordinal)) = value.split_once('_') else {
        return false;
    };
    time.len() == 6
        && NaiveTime::parse_from_str(time, "%H%M%S").is_ok()
        && ordinal.bytes().all(|byte| byte.is_ascii_digit())
        && ordinal.parse::<u64>().is_ok_and(|duration| duration > 0)
        && safe_component(value)
}

fn valid_day(value: &str) -> bool {
    value.len() == 8
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(value, "%Y%m%d").is_ok()
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !matches!(value, "." | "..")
        && !value.contains('/')
        && !value.contains('\\')
        && !value
            .chars()
            .any(|character| character.is_ascii_uppercase())
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        ChainPredecessor, Coordinate, OperationRecord, Origin, OriginKind, Readiness, SourceKey,
        digest, ready_document, validate_complete_original, validate_origin,
        validate_ready_document, validate_record,
    };

    fn record(source: &SourceKey, bytes: &[u8]) -> OperationRecord {
        OperationRecord {
            operation_id: "op:1".into(),
            digest: digest(bytes),
            byte_count: bytes.len(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            origin_kind: OriginKind::AgentMemory,
            creation_label: "authenticated connection".into(),
            coordinate: Coordinate {
                day: "20260102".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "030405_1".into(),
            },
            phase: Readiness::Reserved,
            chain: None,
        }
    }

    #[test]
    fn validates_source_operation_and_exact_note_digest() {
        let source = SourceKey::from_verified_id("bearer:opaque");
        let mut record = record(&source, b"note");
        validate_record(&record, &source, "op:1", b"note").unwrap();
        assert!(validate_record(&record, &source, "op:1", b"other").is_err());
        record.digest = "0".repeat(64);
        assert!(validate_record(&record, &source, "op:1", b"note").is_err());
        record.digest = digest(b"note");
        record.byte_count += 1;
        assert!(validate_record(&record, &source, "op:1", b"note").is_err());
        let invalid_source: SourceKey = serde_json::from_str("\"sha256:ABC\"").unwrap();
        assert!(validate_record(&record, &invalid_source, "op:1", b"note").is_err());
        assert!(SourceKey::parse("sha256:ABC").is_err());
    }

    #[test]
    fn rejects_chain_position_that_is_not_bound_to_a_chained_record() {
        let source = SourceKey::from_verified_id("bearer:opaque");
        let mut record = record(&source, b"note");
        record.phase = Readiness::Chained;
        record.chain = Some(ChainPredecessor {
            prev_day: Some("20260101".into()),
            prev_segment: Some("030405_1".into()),
            seq: 2,
        });
        validate_record(&record, &source, "op:1", b"note").unwrap();
        record.chain.as_mut().unwrap().prev_day = Some(record.coordinate.day.clone());
        record.chain.as_mut().unwrap().prev_segment = Some(record.coordinate.segment.clone());
        assert!(validate_record(&record, &source, "op:1", b"note").is_err());
        record.chain.as_mut().unwrap().prev_day = Some("20260101".into());
        record.chain.as_mut().unwrap().prev_segment = Some("../escape".into());
        assert!(validate_record(&record, &source, "op:1", b"note").is_err());

        record.phase = Readiness::Ready;
        record.chain = Some(ChainPredecessor {
            prev_day: Some("20260101".into()),
            prev_segment: Some("030405_1".into()),
            seq: 2,
        });
        validate_record(&record, &source, "op:1", b"note").unwrap();
    }

    #[test]
    fn writer_shaped_origin_and_readiness_round_trip_and_validate() {
        let source = SourceKey::from_verified_id("oauth:grant");
        let origin = Origin {
            kind: OriginKind::AgentMemory,
            source_key: source.clone(),
            creation_label: "authenticated connection".into(),
            created_at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
            stream: format!("agent-memory-{}", source.component()),
            segment: "030405_1".into(),
        };
        validate_origin(&origin, &source).unwrap();
        let encoded = serde_json::to_vec(&origin).unwrap();
        let decoded: Origin = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, origin);
        let mut record = record(&source, b"note");
        record.phase = Readiness::Chained;
        record.chain = Some(ChainPredecessor {
            prev_day: None,
            prev_segment: None,
            seq: 1,
        });
        let ready = ready_document(&record, &source).unwrap();
        let encoded = serde_json::to_vec(&ready).unwrap();
        let decoded = serde_json::from_slice(&encoded).unwrap();
        validate_ready_document(&decoded, &source, b"note").unwrap();
        validate_complete_original(
            &decoded,
            &source,
            &record.coordinate,
            &origin,
            record.chain.as_ref().unwrap(),
            b"note",
        )
        .unwrap();
        let mut corrupted = decoded.clone();
        corrupted.chain.seq = 2;
        corrupted.chain.prev_day = Some("20260101".into());
        corrupted.chain.prev_segment = Some("030405_1".into());
        // The altered document is self-consistent but disagrees with the real marker.
        validate_ready_document(&corrupted, &source, b"note").unwrap();
        assert!(
            validate_complete_original(
                &corrupted,
                &source,
                &record.coordinate,
                &origin,
                record.chain.as_ref().unwrap(),
                b"note"
            )
            .is_err()
        );
        let mut corrupted = decoded;
        corrupted.origin.creation_label = "different label".into();
        assert!(
            validate_complete_original(
                &corrupted,
                &source,
                &record.coordinate,
                &origin,
                record.chain.as_ref().unwrap(),
                b"note"
            )
            .is_err()
        );
        assert!(validate_origin(&origin, &SourceKey::from_verified_id("oauth:other")).is_err());
    }
}
