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
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileDescriptor {
    pub submitted: String,
    pub written: String,
    pub size: u64,
    pub sha256: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Durable attribution for a linked-device ingest.
#[derive(Clone, Debug, Deserialize, Serialize)]
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

/// One recognized durable event-log row.
/// Its `day` and `segment` values may be restamped after a segment move.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DurableEvent {
    Callosum(CallosumEnvelope),
    DeviceIngest(DeviceIngestEvent),
}
