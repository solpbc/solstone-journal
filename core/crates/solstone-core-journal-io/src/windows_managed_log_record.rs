// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict, path-free pointer records for Windows managed operational logs.

#![allow(
    dead_code,
    reason = "the managed-log substrate is intentionally inactive"
)]

use std::error::Error;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::name_admission::{NameAdmissionReason, check_portable_component};
use crate::paths::is_day_key;
use crate::windows_identity::WindowsFileIdentity;

pub(crate) const MANAGED_LOG_RECORD_VERSION: u32 = 1;
pub(crate) const MAX_MANAGED_LOG_RECORD_BYTES: usize = 4096;

/// A versioned alias payload that identifies a canonical file without naming a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedLogRecord {
    version: u32,
    generation: u64,
    day: String,
    reference: String,
    name: String,
    canonical_identity: WindowsFileIdentity,
}

impl ManagedLogRecord {
    pub(crate) fn new(
        generation: u64,
        day: String,
        reference: String,
        name: String,
        canonical_identity: WindowsFileIdentity,
    ) -> Result<Self, ManagedLogRecordError> {
        let record = Self {
            version: MANAGED_LOG_RECORD_VERSION,
            generation,
            day,
            reference,
            name,
            canonical_identity,
        };
        record.validate()?;
        Ok(record)
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, ManagedLogRecordError> {
        if bytes.len() > MAX_MANAGED_LOG_RECORD_BYTES {
            return Err(ManagedLogRecordError::TooLarge { size: bytes.len() });
        }
        let record: Self = serde_json::from_slice(bytes).map_err(ManagedLogRecordError::Json)?;
        record.validate()?;
        Ok(record)
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, ManagedLogRecordError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(ManagedLogRecordError::Json)?;
        if bytes.len() > MAX_MANAGED_LOG_RECORD_BYTES {
            return Err(ManagedLogRecordError::TooLarge { size: bytes.len() });
        }
        Ok(bytes)
    }

    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn day(&self) -> &str {
        &self.day
    }

    pub(crate) fn reference(&self) -> &str {
        &self.reference
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) const fn canonical_identity(&self) -> WindowsFileIdentity {
        self.canonical_identity
    }

    fn validate(&self) -> Result<(), ManagedLogRecordError> {
        if self.version != MANAGED_LOG_RECORD_VERSION {
            return Err(ManagedLogRecordError::UnsupportedVersion {
                version: self.version,
            });
        }
        if self.generation == 0 {
            return Err(ManagedLogRecordError::ZeroGeneration);
        }
        if !is_day_key(&self.day) {
            return Err(ManagedLogRecordError::InvalidDay);
        }
        check_portable_component(&self.reference).map_err(|reason| {
            ManagedLogRecordError::InvalidLogicalComponent {
                field: "reference",
                reason,
            }
        })?;
        check_portable_component(&self.name).map_err(|reason| {
            ManagedLogRecordError::InvalidLogicalComponent {
                field: "name",
                reason,
            }
        })?;
        Ok(())
    }
}

/// Admit a candidate record only at the next generation observed while holding
/// its persistent alias lock. The caller performs the subsequent atomic publish.
pub(crate) fn admit_next_generation(
    current: Option<&ManagedLogRecord>,
    candidate: &ManagedLogRecord,
) -> Result<(), ManagedLogGenerationError> {
    candidate
        .validate()
        .map_err(ManagedLogGenerationError::Record)?;
    let expected = match current {
        None => 1,
        Some(current) => current
            .generation()
            .checked_add(1)
            .ok_or(ManagedLogGenerationError::Exhausted)?,
    };
    if candidate.generation() != expected {
        return Err(ManagedLogGenerationError::Superseded {
            observed: current.map_or(0, ManagedLogRecord::generation),
        });
    }
    Ok(())
}

impl Serialize for ManagedLogRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ManagedLogRecord", 6)?;
        state.serialize_field("version", &self.version)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("day", &self.day)?;
        state.serialize_field("reference", &self.reference)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("canonical_identity", &IdentityWire(self.canonical_identity))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ManagedLogRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ManagedLogRecordVisitor)
    }
}

struct ManagedLogRecordVisitor;

impl<'de> Visitor<'de> for ManagedLogRecordVisitor {
    type Value = ManagedLogRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a strict managed-log pointer record")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut version = None;
        let mut generation = None;
        let mut day = None;
        let mut reference = None;
        let mut name = None;
        let mut canonical_identity = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "version" => set_once(&mut version, map.next_value()?, "version")?,
                "generation" => set_once(&mut generation, map.next_value()?, "generation")?,
                "day" => set_once(&mut day, map.next_value()?, "day")?,
                "reference" => set_once(&mut reference, map.next_value()?, "reference")?,
                "name" => set_once(&mut name, map.next_value()?, "name")?,
                "canonical_identity" => set_once(
                    &mut canonical_identity,
                    map.next_value::<IdentityWire>()?,
                    "canonical_identity",
                )?,
                _ => return Err(de::Error::unknown_field(&field, RECORD_FIELDS)),
            }
        }
        let version = required(version, "version")?;
        let generation = required(generation, "generation")?;
        let day = required(day, "day")?;
        let reference = required(reference, "reference")?;
        let name = required(name, "name")?;
        let canonical_identity = required(canonical_identity, "canonical_identity")?.0;
        ManagedLogRecord::new(generation, day, reference, name, canonical_identity)
            .map_err(de::Error::custom)
            .and_then(|record| {
                if record.version == version {
                    Ok(record)
                } else {
                    Err(de::Error::custom(
                        ManagedLogRecordError::UnsupportedVersion { version },
                    ))
                }
            })
    }
}

const RECORD_FIELDS: &[&str] = &[
    "version",
    "generation",
    "day",
    "reference",
    "name",
    "canonical_identity",
];

fn set_once<T, E>(slot: &mut Option<T>, value: T, field: &'static str) -> Result<(), E>
where
    E: de::Error,
{
    if slot.replace(value).is_some() {
        return Err(E::duplicate_field(field));
    }
    Ok(())
}

fn required<T, E>(value: Option<T>, field: &'static str) -> Result<T, E>
where
    E: de::Error,
{
    value.ok_or_else(|| E::missing_field(field))
}

struct IdentityWire(WindowsFileIdentity);

impl Serialize for IdentityWire {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("CanonicalIdentity", 2)?;
        state.serialize_field("volume_serial", &format!("{:016x}", self.0.volume_serial()))?;
        state.serialize_field("file_id", &hex_bytes(&self.0.file_id()))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for IdentityWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(IdentityVisitor)
    }
}

struct IdentityVisitor;

impl<'de> Visitor<'de> for IdentityVisitor {
    type Value = IdentityWire;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical Windows file identity")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut volume_serial: Option<String> = None;
        let mut file_id: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "volume_serial" => {
                    set_once(&mut volume_serial, map.next_value()?, "volume_serial")?
                }
                "file_id" => set_once(&mut file_id, map.next_value()?, "file_id")?,
                _ => return Err(de::Error::unknown_field(&field, IDENTITY_FIELDS)),
            }
        }
        let volume_serial = decode_hex::<8, A::Error>(&required(volume_serial, "volume_serial")?)?;
        let file_id = decode_hex::<16, A::Error>(&required(file_id, "file_id")?)?;
        Ok(IdentityWire(WindowsFileIdentity::from_parts(
            u64::from_be_bytes(volume_serial),
            file_id,
        )))
    }
}

const IDENTITY_FIELDS: &[&str] = &["volume_serial", "file_id"];

fn hex_bytes(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn decode_hex<const N: usize, E: de::Error>(value: &str) -> Result<[u8; N], E> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(E::custom(
            "identity must be fixed-width lowercase hexadecimal",
        ));
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let position = index * 2;
        let high = hex_nibble(value.as_bytes()[position]);
        let low = hex_nibble(value.as_bytes()[position + 1]);
        *byte = (high << 4) | low;
    }
    Ok(bytes)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("decode_hex validates every byte first"),
    }
}

/// Record parsing and serialization failures are deliberately strict.
#[derive(Debug)]
pub(crate) enum ManagedLogRecordError {
    TooLarge {
        size: usize,
    },
    Json(serde_json::Error),
    UnsupportedVersion {
        version: u32,
    },
    ZeroGeneration,
    InvalidDay,
    InvalidLogicalComponent {
        field: &'static str,
        reason: NameAdmissionReason,
    },
}

/// Generation admission never rewrites a record or chooses a publication authority.
#[derive(Debug)]
pub(crate) enum ManagedLogGenerationError {
    Record(ManagedLogRecordError),
    Superseded { observed: u64 },
    Exhausted,
}

impl fmt::Display for ManagedLogGenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Record(error) => error.fmt(formatter),
            Self::Superseded { observed } => {
                write!(
                    formatter,
                    "managed-log generation was superseded at {observed}"
                )
            }
            Self::Exhausted => formatter.write_str("managed-log generation is exhausted"),
        }
    }
}

impl Error for ManagedLogGenerationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Record(error) => Some(error),
            Self::Superseded { .. } | Self::Exhausted => None,
        }
    }
}

impl fmt::Display for ManagedLogRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size } => write!(
                formatter,
                "managed-log record exceeds 4096-byte limit ({size})"
            ),
            Self::Json(error) => write!(formatter, "malformed managed-log record: {error}"),
            Self::UnsupportedVersion { version } => write!(
                formatter,
                "unsupported managed-log record version {version}"
            ),
            Self::ZeroGeneration => formatter.write_str("managed-log generation must be nonzero"),
            Self::InvalidDay => formatter.write_str("managed-log day must be eight ASCII digits"),
            Self::InvalidLogicalComponent { field, reason } => {
                write!(formatter, "invalid managed-log {field}: {reason}")
            }
        }
    }
}

impl Error for ManagedLogRecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(bytes: [u8; 16]) -> WindowsFileIdentity {
        WindowsFileIdentity::from_parts(7, bytes)
    }

    #[test]
    fn round_trip_preserves_the_full_file_id_not_its_fold() {
        let left = identity([1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        let right = identity([3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(left.folded_file_id(), right.folded_file_id());
        assert_ne!(left, right);
        let record =
            ManagedLogRecord::new(1, "20260829".into(), "writer".into(), "stream".into(), left)
                .unwrap();
        assert_eq!(
            ManagedLogRecord::parse(&record.to_bytes().unwrap())
                .unwrap()
                .canonical_identity(),
            left
        );
    }

    #[test]
    fn strict_parser_rejects_duplicate_unknown_missing_and_torn_records() {
        let base = r#"{"version":1,"generation":1,"day":"20260829","reference":"writer","name":"stream","canonical_identity":{"volume_serial":"0000000000000007","file_id":"00000000000000000000000000000001"}}"#;
        assert!(ManagedLogRecord::parse(base.as_bytes()).is_ok());
        for invalid in [
            r#"{"version":1,"version":1}"#,
            r#"{"version":1}"#,
            r#"{"version":1,"generation":1,"day":"20260829","reference":"writer","name":"stream","canonical_identity":{"volume_serial":"0000000000000007","file_id":"00000000000000000000000000000001"},"extra":1}"#,
            r#"{"version":1"#,
        ] {
            assert!(ManagedLogRecord::parse(invalid.as_bytes()).is_err());
        }
    }

    #[test]
    fn generation_admission_requires_exact_next_record() {
        let first = ManagedLogRecord::new(
            1,
            "20260829".into(),
            "writer".into(),
            "stream".into(),
            identity([1; 16]),
        )
        .unwrap();
        let second = ManagedLogRecord::new(
            2,
            "20260829".into(),
            "writer".into(),
            "stream".into(),
            identity([2; 16]),
        )
        .unwrap();
        assert!(admit_next_generation(None, &first).is_ok());
        assert!(admit_next_generation(Some(&first), &second).is_ok());
        assert!(admit_next_generation(Some(&first), &first).is_err());
    }
}
