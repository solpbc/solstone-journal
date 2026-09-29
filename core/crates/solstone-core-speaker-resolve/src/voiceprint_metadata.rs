// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable voiceprint metadata records shared by speaker-resolution writers.

use std::path::Path;

use chrono::{NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde_json::{Map, Value};
use thiserror::Error;

use solstone_core_journal_config::{ConfigLoadError, read_journal_config};
use solstone_core_journal_io::paths::SegmentLayout;

/// The required field order for all newly written voiceprint metadata.
pub const VOICEPRINT_METADATA_KEYS: [&str; 9] = [
    "schema_version",
    "stream_layout",
    "day",
    "segment_key",
    "source",
    "stream",
    "sentence_id",
    "added_at",
    "last_seen_ts",
];

/// One voiceprint provenance record. `schema_version` and `stream_layout` are present
/// in current writes; unversioned legacy records omit them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceprintMetadata {
    pub schema_version: Option<i64>,
    pub stream_layout: Option<SegmentLayout>,
    pub day: String,
    pub segment_key: String,
    pub source: String,
    pub stream: String,
    pub sentence_id: i64,
    pub added_at: i64,
    pub last_seen_ts: Option<i64>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VoiceprintMetadataError {
    #[error("voiceprint metadata must be an object")]
    NotObject,
    #[error("voiceprint metadata is missing or has invalid {field}")]
    InvalidField { field: &'static str },
    #[error("unsupported voiceprint metadata schema version {version}")]
    UnsupportedSchemaVersion { version: i64 },
}

impl VoiceprintMetadata {
    /// Construct a new-format nine-key metadata record.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        day: impl Into<String>,
        stream_layout: SegmentLayout,
        segment_key: impl Into<String>,
        source: impl Into<String>,
        stream: impl Into<String>,
        sentence_id: i64,
        added_at: i64,
        last_seen_ts: i64,
    ) -> Self {
        Self {
            schema_version: Some(1),
            stream_layout: Some(stream_layout),
            day: day.into(),
            segment_key: segment_key.into(),
            source: source.into(),
            stream: stream.into(),
            sentence_id,
            added_at,
            last_seen_ts: Some(last_seen_ts),
        }
    }

    /// Serialize a record in the pinned nine-key insertion order for new records.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        if let Some(version) = self.schema_version {
            object.insert("schema_version".to_owned(), Value::from(version));
        }
        if let Some(layout) = self.stream_layout {
            object.insert(
                "stream_layout".to_owned(),
                Value::String(layout.as_str().to_owned()),
            );
        }
        object.insert("day".to_owned(), Value::String(self.day.clone()));
        object.insert(
            "segment_key".to_owned(),
            Value::String(self.segment_key.clone()),
        );
        object.insert("source".to_owned(), Value::String(self.source.clone()));
        object.insert("stream".to_owned(), Value::String(self.stream.clone()));
        object.insert("sentence_id".to_owned(), Value::from(self.sentence_id));
        object.insert("added_at".to_owned(), Value::from(self.added_at));
        if let Some(last_seen) = self.last_seen_ts {
            object.insert("last_seen_ts".to_owned(), Value::from(last_seen));
        }
        Value::Object(object)
    }

    /// Parse either the new nine-key record or a legacy unversioned record.
    pub fn from_json(value: &Value) -> Result<Self, VoiceprintMetadataError> {
        let object = value
            .as_object()
            .ok_or(VoiceprintMetadataError::NotObject)?;
        let schema_version = match object.get("schema_version") {
            None => None,
            Some(v) => {
                let ver = v.as_i64().ok_or(VoiceprintMetadataError::InvalidField {
                    field: "schema_version",
                })?;
                if ver != 1 {
                    return Err(VoiceprintMetadataError::UnsupportedSchemaVersion { version: ver });
                }
                Some(ver)
            }
        };
        let stream_layout = match object.get("stream_layout") {
            None => {
                if schema_version.is_some() {
                    return Err(VoiceprintMetadataError::InvalidField {
                        field: "stream_layout",
                    });
                }
                None
            }
            Some(Value::String(s)) => match s.as_str() {
                "direct" => Some(SegmentLayout::Direct),
                "named" => Some(SegmentLayout::Named),
                _ => {
                    return Err(VoiceprintMetadataError::InvalidField {
                        field: "stream_layout",
                    });
                }
            },
            Some(_) => {
                return Err(VoiceprintMetadataError::InvalidField {
                    field: "stream_layout",
                });
            }
        };
        let string = |field| {
            object
                .get(field)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or(VoiceprintMetadataError::InvalidField { field })
        };
        let integer = |field| {
            object
                .get(field)
                .and_then(Value::as_i64)
                .ok_or(VoiceprintMetadataError::InvalidField { field })
        };
        let last_seen_ts =
            match object.get("last_seen_ts") {
                None => None,
                Some(value) => Some(value.as_i64().ok_or(
                    VoiceprintMetadataError::InvalidField {
                        field: "last_seen_ts",
                    },
                )?),
            };
        Ok(Self {
            schema_version,
            stream_layout,
            day: string("day")?,
            segment_key: string("segment_key")?,
            source: string("source")?,
            stream: string("stream")?,
            sentence_id: integer("sentence_id")?,
            added_at: integer("added_at")?,
            last_seen_ts,
        })
    }
}

/// The owner's configured timezone, which segment keys are local wall time in.
///
/// A missing or unknown zone reads as UTC.
pub fn owner_timezone(journal_root: &Path) -> Result<Tz, ConfigLoadError> {
    let config = read_journal_config(journal_root)?;
    Ok(config
        .config
        .as_ref()
        .and_then(|value| value.get("identity"))
        .and_then(Value::as_object)
        .and_then(|identity| identity.get("timezone"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC))
}

/// Epoch milliseconds at which a segment started: the `last_seen_ts` of a
/// voiceprint taken from it. `None` for a malformed day or segment key.
#[must_use]
pub fn segment_start_ts_ms(timezone: Tz, day: &str, segment_key: &str) -> Option<i64> {
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let time = segment_key.get(0..6)?;
    let date = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let local = date.and_hms_opt(
        time[0..2].parse().ok()?,
        time[2..4].parse().ok()?,
        time[4..6].parse().ok()?,
    )?;
    timezone
        .from_local_datetime(&local)
        .single()
        .or_else(|| timezone.from_local_datetime(&local).earliest())
        .map(|value| value.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_start_is_owner_local_wall_time() {
        let denver: Tz = "America/Denver".parse().unwrap();
        // 2026-08-08 12:00:00 MDT is 18:00:00 UTC.
        assert_eq!(
            segment_start_ts_ms(denver, "20260808", "120000_300"),
            Some(1_786_212_000_000)
        );
        assert_eq!(
            segment_start_ts_ms(Tz::UTC, "20260808", "120000_300"),
            Some(1_786_190_400_000)
        );
        // The repeated hour at the end of daylight time takes its first instant.
        assert_eq!(
            segment_start_ts_ms(denver, "20261101", "013000_300"),
            Some(1_793_518_200_000)
        );
        for (day, key) in [
            ("2026088", "120000_300"),
            ("20260808", "12000"),
            ("20260808", "250000_1"),
        ] {
            assert_eq!(segment_start_ts_ms(Tz::UTC, day, key), None, "{day} {key}");
        }
    }

    #[test]
    fn ac21_metadata_keyset_is_the_literal_nine_key_json_shape() {
        let metadata = VoiceprintMetadata::new(
            "20260808",
            SegmentLayout::Named,
            "120000_300",
            "audio",
            "main",
            7,
            123,
            456,
        );
        assert_eq!(
            serde_json::to_string(&metadata.to_json()).unwrap(),
            r#"{"schema_version":1,"stream_layout":"named","day":"20260808","segment_key":"120000_300","source":"audio","stream":"main","sentence_id":7,"added_at":123,"last_seen_ts":456}"#,
        );
    }

    #[test]
    fn legacy_six_and_seven_key_metadata_remains_readable() {
        let value7 = serde_json::json!({
            "day": "20260808",
            "segment_key": "120000_300",
            "source": "audio",
            "stream": "main",
            "sentence_id": 7,
            "added_at": 123,
            "last_seen_ts": 456,
        });
        let parsed7 = VoiceprintMetadata::from_json(&value7).unwrap();
        assert_eq!(parsed7.schema_version, None);
        assert_eq!(parsed7.stream_layout, None);
        assert_eq!(parsed7.last_seen_ts, Some(456));

        let value6 = serde_json::json!({
            "day": "20260808",
            "segment_key": "120000_300",
            "source": "audio",
            "stream": "main",
            "sentence_id": 7,
            "added_at": 123,
        });
        let parsed6 = VoiceprintMetadata::from_json(&value6).unwrap();
        assert_eq!(parsed6.schema_version, None);
        assert_eq!(parsed6.stream_layout, None);
        assert_eq!(parsed6.last_seen_ts, None);
    }
}
