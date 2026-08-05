// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io;
use std::path::Path;

use crate::model::{DeviceIngestEvent, ReasonCode};

pub fn read_events(segment: &Path) -> Result<Vec<DeviceIngestEvent>, ReasonCode> {
    let path = segment.join("events.jsonl");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(ReasonCode::JournalReadFailed),
    };
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).map_err(|_| ReasonCode::IngestEventLogMalformed))
        .filter_map(
            |record: Result<DeviceIngestEvent, ReasonCode>| match record {
                Ok(record) if record.record_type == "device_ingest" => Some(Ok(record)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            },
        )
        .collect()
}
