// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable voiceprint metadata records shared by speaker-resolution writers.

use std::path::Path;

use jiff::SignedDuration;
use jiff::civil::{DateTime, Time};
use jiff::tz::TimeZone;
use serde_json::{Map, Value};
use thiserror::Error;

use solstone_core_entity::{VoiceprintKey, voiceprint_key_from_metadata};
use solstone_core_journal_config::{ConfigLoadError, read_journal_config};
use solstone_core_journal_io::paths::SegmentLayout;
use solstone_core_time::from_local;

/// Required scalar fields for speaker-resolution archive rows. The entity
/// store also accepts other scalar shapes, which these consumers must skip.
pub(crate) fn metadata_key(value: &Value) -> Option<VoiceprintKey> {
    value.get("day")?.as_str()?;
    value.get("segment_key")?.as_str()?;
    value.get("source")?.as_str()?;
    value.get("sentence_id")?.as_i64()?;
    voiceprint_key_from_metadata(value).ok()
}

/// Bootstrap and accumulation require a valid string when layout is present,
/// including rejecting null. Missing layout still uses canonical inference.
pub(crate) fn strict_metadata_key(value: &Value) -> Option<VoiceprintKey> {
    if let Some(layout) = value.get("stream_layout") {
        match layout.as_str()? {
            "direct" | "named" => {}
            _ => return None,
        }
    }
    metadata_key(value)
}

pub(crate) fn provenance_key(
    day: &str,
    layout: SegmentLayout,
    stream: &str,
    segment_key: &str,
    source: &str,
    sentence_id: i64,
) -> VoiceprintKey {
    voiceprint_key_from_metadata(&serde_json::json!({
        "day": day,
        "stream_layout": layout.as_str(),
        "stream": stream,
        "segment_key": segment_key,
        "source": source,
        "sentence_id": sentence_id,
    }))
    .expect("typed voiceprint provenance has supported key fields")
}

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

/// The journal's owner zone, which segment keys are read in. A config that
/// exists but does not parse is an error; a missing or unknown zone falls back
/// to this computer's zone.
pub fn owner_timezone(journal_root: &Path) -> Result<TimeZone, ConfigLoadError> {
    read_journal_config(journal_root)?;
    Ok(solstone_core_journal_config::owner_zone(journal_root))
}

/// Epoch milliseconds at which a segment started: the `last_seen_ts` of a
/// voiceprint taken from it. `None` for a malformed day or segment key.
#[must_use]
pub fn segment_start_ts_ms(timezone: &TimeZone, day: &str, segment_key: &str) -> Option<i64> {
    let date = solstone_core_time::parse_day(day)?;
    // A segment key starts with its wall-clock start as six ASCII digits.
    let digits = segment_key.as_bytes().get(..6)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let field = |at: usize| (digits[at] - b'0') as i8 * 10 + (digits[at + 1] - b'0') as i8;
    let time = Time::new(field(0), field(2), field(4), 0).ok()?;
    wall_time_ms(timezone, date.to_datetime(time))
}

/// The instant a zone gives a wall time. The repeated hour at the end of
/// daylight time takes its first instant, and a wall time the clocks skip when
/// they spring forward reads at the offset in effect just before the jump.
fn wall_time_ms(timezone: &TimeZone, local: DateTime) -> Option<i64> {
    if let Some(value) = from_local(timezone, local).earliest() {
        return Some(value.timestamp().as_millisecond());
    }
    let before = (1..=3).find_map(|hours| {
        let earlier = local.checked_sub(SignedDuration::from_hours(hours)).ok()?;
        from_local(timezone, earlier).earliest()
    })?;
    let ts = before.offset().to_timestamp(local).ok()?;
    Some(ts.as_millisecond())
}

#[cfg(test)]
mod key_tests {
    use super::segment_start_ts_ms;
    use solstone_core_journal_config::TimeZone;

    #[test]
    fn a_segment_key_must_start_with_six_digits() {
        let utc = TimeZone::UTC;
        assert_eq!(
            segment_start_ts_ms(&utc, "19700101", "010000_300"),
            Some(3_600_000)
        );
        for key in [
            "+10000_300",
            "1\u{e9}0000_300",
            "01000",
            "0100x0_300",
            " 10000_300",
        ] {
            assert_eq!(segment_start_ts_ms(&utc, "19700101", key), None, "{key}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_keys_normalize_streams_without_widening_row_admission() {
        let direct = provenance_key(
            "20260808",
            SegmentLayout::Direct,
            "_default",
            "120000_300",
            "audio",
            7,
        );
        let base = serde_json::json!({"day":"20260808","segment_key":"120000_300","source":"audio","sentence_id":7});
        for stream in [
            None,
            Some(Value::Null),
            Some(Value::Bool(false)),
            Some(Value::from("")),
            Some(Value::from("_default")),
        ] {
            let mut value = base.clone();
            if let Some(stream) = stream {
                value["stream"] = stream;
            }
            assert_eq!(strict_metadata_key(&value), Some(direct.clone()));
        }
        let mut value = base.clone();
        value["stream"] = Value::from("_default");
        value["stream_layout"] = Value::from("named");
        let named = provenance_key(
            "20260808",
            SegmentLayout::Named,
            "_default",
            "120000_300",
            "audio",
            7,
        );
        assert_eq!(strict_metadata_key(&value), Some(named.clone()));
        assert_ne!(named, direct);
        value["stream"] = Value::from("mic");
        value.as_object_mut().unwrap().remove("stream_layout");
        assert_eq!(
            strict_metadata_key(&value),
            Some(provenance_key(
                "20260808",
                SegmentLayout::Named,
                "mic",
                "120000_300",
                "audio",
                7
            ))
        );
        for layout in [
            Value::Null,
            Value::from(1),
            Value::from(""),
            Value::from("other"),
        ] {
            value["stream_layout"] = layout;
            assert_eq!(strict_metadata_key(&value), None);
        }
        for field in ["day", "segment_key", "source", "sentence_id"] {
            let mut value = base.clone();
            value.as_object_mut().unwrap().remove(field);
            assert_eq!(strict_metadata_key(&value), None);
            value[field] = if field == "sentence_id" {
                Value::from("7")
            } else {
                Value::from(7)
            };
            assert_eq!(strict_metadata_key(&value), None);
        }
    }

    #[test]
    fn segment_start_is_owner_local_wall_time() {
        let denver = solstone_core_time::zone("America/Denver").unwrap();
        // 2026-08-08 12:00:00 MDT is 18:00:00 UTC.
        assert_eq!(
            segment_start_ts_ms(&denver, "20260808", "120000_300"),
            Some(1_786_212_000_000)
        );
        assert_eq!(
            segment_start_ts_ms(&TimeZone::UTC, "20260808", "120000_300"),
            Some(1_786_190_400_000)
        );
        // The repeated hour at the end of daylight time takes its first instant.
        assert_eq!(
            segment_start_ts_ms(&denver, "20261101", "013000_300"),
            Some(1_793_518_200_000)
        );
        // A wall time skipped at the start of daylight time reads at the
        // offset before the jump: 02:30 MST is 09:30 UTC.
        assert_eq!(
            segment_start_ts_ms(&denver, "20260308", "023000_300"),
            Some(1_772_962_200_000)
        );
        for (day, key) in [
            ("2026088", "120000_300"),
            ("20260808", "12000"),
            ("20260808", "250000_1"),
        ] {
            assert_eq!(
                segment_start_ts_ms(&TimeZone::UTC, day, key),
                None,
                "{day} {key}"
            );
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
