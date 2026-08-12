// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

/// A validated observer document that retains every field owned by other
/// producers.  Mutations deliberately change only observer CLI fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ObserverRecord {
    map: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordError {
    MissingKey,
    LegacyFingerprint,
    InvalidDeviceBinding,
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKey => f.write_str("missing observer key"),
            Self::LegacyFingerprint => f.write_str("fingerprint-keyed observer record"),
            Self::InvalidDeviceBinding => f.write_str("invalid observer device binding"),
        }
    }
}

impl std::error::Error for RecordError {}

impl ObserverRecord {
    pub fn validate(mut map: Map<String, Value>) -> Result<Self, RecordError> {
        if !map
            .get("key")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty())
        {
            return Err(RecordError::MissingKey);
        }
        if map
            .get("fingerprint")
            .and_then(Value::as_str)
            .is_some_and(|fingerprint| !fingerprint.is_empty())
        {
            return Err(RecordError::LegacyFingerprint);
        }
        if let Some(binding) = map.get("device_binding")
            && !binding.is_null()
        {
            let binding = normalize_device_binding(binding)?;
            map.insert("device_binding".to_owned(), Value::Object(binding));
        }
        Ok(Self { map })
    }

    pub fn from_value(value: Value) -> Result<Self, RecordError> {
        match value {
            Value::Object(map) => Self::validate(map),
            _ => Err(RecordError::MissingKey),
        }
    }

    pub fn value(&self) -> &Map<String, Value> {
        &self.map
    }

    pub fn into_value(self) -> Map<String, Value> {
        self.map
    }

    pub fn key(&self) -> &str {
        self.map
            .get("key")
            .and_then(Value::as_str)
            .expect("validated observer record has a key")
    }

    pub fn prefix(&self) -> String {
        self.key().chars().take(8).collect()
    }

    pub fn name(&self) -> Option<&str> {
        self.string("name")
    }
    pub fn created_at(&self) -> Option<i64> {
        self.integer("created_at")
    }
    pub fn last_seen(&self) -> Option<i64> {
        self.integer("last_seen")
    }
    pub fn last_segment(&self) -> Option<&str> {
        self.string("last_segment")
    }
    pub fn last_segment_received_at(&self) -> Option<i64> {
        self.integer("last_segment_received_at")
    }
    pub fn last_segment_day(&self) -> Option<&str> {
        self.string("last_segment_day")
    }
    pub fn enabled(&self) -> Option<bool> {
        self.map.get("enabled").and_then(Value::as_bool)
    }
    pub fn revoked(&self) -> bool {
        self.map
            .get("revoked")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }
    pub fn revoked_at(&self) -> Option<i64> {
        self.integer("revoked_at")
    }
    pub fn stats(&self) -> Option<&Map<String, Value>> {
        self.map.get("stats").and_then(Value::as_object)
    }
    /// The active durable ingest rejection, if the observer recorded one.
    pub fn ingest_rejection(&self) -> Option<&Map<String, Value>> {
        self.map
            .get("health")
            .and_then(Value::as_object)
            .and_then(|health| health.get("ingest_rejection"))
            .and_then(Value::as_object)
    }

    /// The durable observer health beacon, if present.
    pub fn health_beacon(&self) -> Option<&Map<String, Value>> {
        self.map
            .get("health")
            .and_then(Value::as_object)
            .and_then(|health| health.get("beacon"))
            .and_then(Value::as_object)
    }

    pub fn device_binding_device(&self) -> Option<&str> {
        self.device_binding()
            .and_then(|binding| binding.get("device"))
            .and_then(Value::as_str)
    }

    pub fn device_binding_kind(&self) -> Option<&str> {
        self.device_binding()
            .and_then(|binding| binding.get("kind"))
            .and_then(Value::as_str)
    }

    pub fn set_name(&mut self, name: String) {
        self.map.insert("name".to_owned(), Value::String(name));
    }
    pub fn set_revoked(&mut self, revoked: bool) {
        self.map.insert("revoked".to_owned(), Value::Bool(revoked));
    }
    pub fn set_revoked_at(&mut self, value: i64) {
        self.map.insert("revoked_at".to_owned(), Value::from(value));
    }
    pub fn set_stats(&mut self, stats: Map<String, Value>) {
        self.map.insert("stats".to_owned(), Value::Object(stats));
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.map.get(key).and_then(Value::as_str)
    }
    fn integer(&self, key: &str) -> Option<i64> {
        self.map.get(key).and_then(Value::as_i64)
    }
    fn device_binding(&self) -> Option<&Map<String, Value>> {
        self.map.get("device_binding").and_then(Value::as_object)
    }
}

fn normalize_device_binding(value: &Value) -> Result<Map<String, Value>, RecordError> {
    let binding = value.as_object().ok_or(RecordError::InvalidDeviceBinding)?;
    let device = binding
        .get("device")
        .and_then(Value::as_str)
        .ok_or(RecordError::InvalidDeviceBinding)?;
    let kind = binding
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty())
        .ok_or(RecordError::InvalidDeviceBinding)?;
    let digest = device
        .strip_prefix("sha256:")
        .ok_or(RecordError::InvalidDeviceBinding)?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(RecordError::InvalidDeviceBinding);
    }
    let mut normalized = Map::new();
    normalized.insert("device".to_owned(), Value::String(device.to_owned()));
    normalized.insert("kind".to_owned(), Value::String(kind.to_owned()));
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record() -> ObserverRecord {
        ObserverRecord::from_value(json!({
            "key": "abcdefgh123", "name": "office", "device_binding": {
                "device": format!("sha256:{}", "a".repeat(64)), "kind": "cert", "extra": "discarded-by-python"
            }, "created_at": 1, "last_seen": 2, "last_segment": "s", "last_segment_received_at": 3,
            "last_segment_day": "20260101", "enabled": true, "revoked": false, "revoked_at": null,
            "stats": {"segments_received": 1, "bytes_received": 2, "note": "keep", "nested": {"x": 1}},
            "platform": "linux", "stream": {"retained": true}
        })).expect("record")
    }

    #[test]
    fn mutations_retain_every_unowned_field() {
        let mut record = record();
        record.set_name("renamed".to_owned());
        record.set_revoked(true);
        record.set_revoked_at(4);
        assert_eq!(record.value()["platform"], "linux");
        assert_eq!(record.value()["stream"]["retained"], true);
        assert_eq!(record.value()["name"], "renamed");
        assert_eq!(record.value()["revoked_at"], 4);
    }

    #[test]
    fn arbitrary_stats_map_survives_record_round_trip() {
        let record = record();
        assert_eq!(record.stats().expect("stats")["note"], "keep");
        assert_eq!(record.stats().expect("stats")["nested"]["x"], 1);
    }
}
