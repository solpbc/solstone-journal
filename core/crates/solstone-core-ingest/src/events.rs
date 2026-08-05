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
    let mut records = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|_| ReasonCode::IngestEventLogMalformed)?;
        if value.get("record_type").and_then(serde_json::Value::as_str) != Some("device_ingest") {
            continue;
        }
        records
            .push(serde_json::from_value(value).map_err(|_| ReasonCode::IngestEventLogMalformed)?);
    }
    Ok(records)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::read_events;

    #[test]
    fn ignores_unrelated_event_records() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("solstone-core-ingest-events-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        let ingest = json!({"record_type":"device_ingest","record_version":1,"outcome":"accepted","protocol_version":3,"did":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"","stream":"_default","day":"20260804","segment":"120000_1","files":[],"meta":{}});
        fs::write(
            root.join("events.jsonl"),
            format!("{}\n{{\"record_type\":\"other\"}}\n", ingest),
        )
        .unwrap();

        let records = read_events(&root).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record_type, "device_ingest");
        let _ = fs::remove_dir_all(root);
    }
}
