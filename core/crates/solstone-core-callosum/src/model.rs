// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A Callosum wire message with all extension keys preserved.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CallosumEnvelope {
    pub tract: String,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A file attributed to a device-ingest record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FileDescriptor {
    pub submitted: String,
    pub written: String,
    pub size: u64,
    pub sha256: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Durable attribution for a linked-device ingest.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DeviceIngestEvent {
    pub record_type: String,
    pub record_version: u8,
    pub outcome: String,
    pub protocol_version: u8,
    #[serde(rename = "cid", alias = "did")]
    pub cid: String,
    pub source: String,
    pub stream: String,
    pub day: String,
    pub segment: String,
    pub files: Vec<FileDescriptor>,
    pub meta: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The time zone a linked device reported for the wall clock it keyed a
/// segment's `day` and `HHMMSS` in. The device reports it; the journal never
/// assigns one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportedZone {
    /// A zone identifier shaped like an IANA name. Callers that need a real
    /// zone still parse it.
    pub tz: Option<String>,
    /// East-positive UTC offset in seconds at the segment's wall-clock start.
    pub utc_offset_seconds: Option<i32>,
}

/// UTC offsets outside +/-18 hours are not real zones.
const MAX_OFFSET_SECONDS: i64 = 18 * 3600;

impl DeviceIngestEvent {
    /// The zone this upload's `meta` reported, or `None` when it reported
    /// nothing usable. A malformed value is ignored, never trusted: the
    /// segment itself was accepted either way.
    pub fn reported_zone(&self) -> Option<ReportedZone> {
        let tz = self
            .meta
            .get("tz")
            .and_then(Value::as_str)
            .filter(|name| zone_name_shaped(name))
            .map(str::to_owned);
        let utc_offset_seconds = self
            .meta
            .get("utc_offset_seconds")
            .and_then(Value::as_i64)
            .filter(|offset| offset.abs() <= MAX_OFFSET_SECONDS)
            .and_then(|offset| i32::try_from(offset).ok());
        (tz.is_some() || utc_offset_seconds.is_some()).then_some(ReportedZone {
            tz,
            utc_offset_seconds,
        })
    }
}

fn zone_name_shaped(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.contains("//")
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'+' | b'-'))
}

/// One recognized durable event-log row.
/// Its `day` and `segment` values may be restamped after a segment move.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DurableEvent {
    Callosum(CallosumEnvelope),
    DeviceIngest(DeviceIngestEvent),
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{DeviceIngestEvent, ReportedZone};

    fn event(meta: Value) -> DeviceIngestEvent {
        serde_json::from_value(json!({
            "record_type": "device_ingest",
            "record_version": 1,
            "outcome": "accepted",
            "protocol_version": 3,
            "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source": "",
            "stream": "phone",
            "day": "20260929",
            "segment": "211400_300",
            "files": [],
            "meta": meta,
        }))
        .unwrap()
    }

    #[test]
    fn reported_zone_reads_both_fields() {
        assert_eq!(
            event(json!({"tz": "Asia/Tokyo", "utc_offset_seconds": 32400})).reported_zone(),
            Some(ReportedZone {
                tz: Some("Asia/Tokyo".to_owned()),
                utc_offset_seconds: Some(32400),
            })
        );
    }

    #[test]
    fn reported_zone_keeps_either_field_alone() {
        assert_eq!(
            event(json!({"utc_offset_seconds": -21600})).reported_zone(),
            Some(ReportedZone {
                tz: None,
                utc_offset_seconds: Some(-21600),
            })
        );
        assert_eq!(
            event(json!({"tz": "America/Argentina/Buenos_Aires"})).reported_zone(),
            Some(ReportedZone {
                tz: Some("America/Argentina/Buenos_Aires".to_owned()),
                utc_offset_seconds: None,
            })
        );
    }

    #[test]
    fn reported_zone_ignores_malformed_values() {
        for meta in [
            json!({}),
            json!({"tz": ""}),
            json!({"tz": "../etc/passwd"}),
            json!({"tz": "Mountain Standard Time"}),
            json!({"tz": 7}),
            json!({"utc_offset_seconds": "32400"}),
            json!({"utc_offset_seconds": 32400.5}),
            json!({"utc_offset_seconds": 64801}),
            json!({"started_at": "2026-09-29T12:14:00Z"}),
        ] {
            assert_eq!(event(meta.clone()).reported_zone(), None, "{meta}");
        }
    }
}
