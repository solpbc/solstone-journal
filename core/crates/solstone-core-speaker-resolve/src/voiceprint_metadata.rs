// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Stable voiceprint metadata records shared by speaker-resolution writers.

use serde_json::{Map, Value};
use thiserror::Error;

/// The required field order for all newly written P6 voiceprint metadata.
pub const VOICEPRINT_METADATA_KEYS: [&str; 7] = [
    "day",
    "segment_key",
    "source",
    "stream",
    "sentence_id",
    "added_at",
    "last_seen_ts",
];

/// One voiceprint provenance record.  `last_seen_ts` is absent only in legacy data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceprintMetadata {
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
}

impl VoiceprintMetadata {
    /// Construct a new-format seven-key metadata record.
    #[must_use]
    pub fn new(
        day: impl Into<String>,
        segment_key: impl Into<String>,
        source: impl Into<String>,
        stream: impl Into<String>,
        sentence_id: i64,
        added_at: i64,
        last_seen_ts: i64,
    ) -> Self {
        Self {
            day: day.into(),
            segment_key: segment_key.into(),
            source: source.into(),
            stream: stream.into(),
            sentence_id,
            added_at,
            last_seen_ts: Some(last_seen_ts),
        }
    }

    /// Serialize a new record in the pinned seven-key insertion order.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("day".to_owned(), Value::String(self.day.clone()));
        object.insert(
            "segment_key".to_owned(),
            Value::String(self.segment_key.clone()),
        );
        object.insert("source".to_owned(), Value::String(self.source.clone()));
        object.insert("stream".to_owned(), Value::String(self.stream.clone()));
        object.insert("sentence_id".to_owned(), Value::from(self.sentence_id));
        object.insert("added_at".to_owned(), Value::from(self.added_at));
        object.insert(
            "last_seen_ts".to_owned(),
            Value::from(
                self.last_seen_ts
                    .expect("new records always carry last_seen_ts"),
            ),
        );
        Value::Object(object)
    }

    /// Parse either the new seven-key record or a six-key legacy record.
    pub fn from_json(value: &Value) -> Result<Self, VoiceprintMetadataError> {
        let object = value
            .as_object()
            .ok_or(VoiceprintMetadataError::NotObject)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac21_metadata_keyset_is_the_literal_seven_key_json_shape() {
        let metadata =
            VoiceprintMetadata::new("20260808", "120000_300", "audio", "main", 7, 123, 456);
        assert_eq!(
            serde_json::to_string(&metadata.to_json()).unwrap(),
            r#"{"day":"20260808","segment_key":"120000_300","source":"audio","stream":"main","sentence_id":7,"added_at":123,"last_seen_ts":456}"#,
        );
    }

    #[test]
    fn legacy_six_key_metadata_remains_readable() {
        let value = serde_json::json!({
            "day": "20260808",
            "segment_key": "120000_300",
            "source": "audio",
            "stream": "main",
            "sentence_id": 7,
            "added_at": 123,
        });
        assert_eq!(
            VoiceprintMetadata::from_json(&value).unwrap().last_seen_ts,
            None
        );
    }
}
