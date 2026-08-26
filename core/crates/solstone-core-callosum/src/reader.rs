// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{CallosumEnvelope, DeviceIngestEvent, DurableEvent};

const EVENTS_FILE: &str = "events.jsonl";

/// Parsed durable rows and row-level recovery counts.
#[derive(Clone, Debug, Default)]
pub struct DurableEventsReport {
    pub records: Vec<DurableEvent>,
    pub unparseable: usize,
    pub unrecognized: usize,
}

/// Device-ingest rows from a mixed durable event log.
#[derive(Clone, Debug, Default)]
pub struct DeviceIngestReport {
    pub records: Vec<DeviceIngestEvent>,
    pub wrong_family: usize,
    pub unparseable: usize,
    pub unrecognized: usize,
}

/// A durable event-log filesystem read failure.
#[derive(Debug)]
pub enum CallosumReadError {
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for CallosumReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl Error for CallosumReadError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Read a segment's mixed durable event log with per-row recovery.
///
/// The file is read as bytes so an invalid UTF-8 tail from an interrupted write
/// only skips that line rather than aborting valid surrounding records.
pub fn read_durable_events(segment_path: &Path) -> Result<DurableEventsReport, CallosumReadError> {
    let path = segment_path.join(EVENTS_FILE);
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DurableEventsReport::default());
        }
        Err(source) => return Err(CallosumReadError::Io { path, source }),
    };

    let mut report = DurableEventsReport::default();
    for line in contents.split(|byte| *byte == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match classify(line) {
            Ok(Some(record)) => report.records.push(record),
            Ok(None) => report.unrecognized += 1,
            Err(()) => report.unparseable += 1,
        }
    }
    Ok(report)
}

/// Read only device-ingest rows while reporting skipped bus-envelope rows.
pub fn read_device_ingest_events(
    segment_path: &Path,
) -> Result<DeviceIngestReport, CallosumReadError> {
    let durable = read_durable_events(segment_path)?;
    let mut report = DeviceIngestReport {
        unparseable: durable.unparseable,
        unrecognized: durable.unrecognized,
        ..DeviceIngestReport::default()
    };
    for record in durable.records {
        match record {
            DurableEvent::DeviceIngest(record) => report.records.push(record),
            DurableEvent::Callosum(_) => report.wrong_family += 1,
        }
    }
    Ok(report)
}

fn classify(line: &[u8]) -> Result<Option<DurableEvent>, ()> {
    let value: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_) => return Err(()),
    };
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    if object.get("record_type").and_then(Value::as_str) == Some("device_ingest") {
        return serde_json::from_value(value)
            .map(DurableEvent::DeviceIngest)
            .map(Some)
            .map_err(|_| ());
    }
    if object.get("tract").and_then(Value::as_str).is_some()
        && object.get("event").and_then(Value::as_str).is_some()
    {
        return serde_json::from_value::<CallosumEnvelope>(value)
            .map(DurableEvent::Callosum)
            .map(Some)
            .map_err(|_| ());
    }
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use super::*;

    static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

    fn segment_path(name: &str) -> PathBuf {
        let suffix = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("solstone-core-callosum-{name}-{suffix}"))
    }

    fn write_events(segment: &Path, contents: &[u8]) {
        fs::create_dir_all(segment).unwrap();
        fs::write(segment.join(EVENTS_FILE), contents).unwrap();
    }

    fn device_event(extra: Value) -> Value {
        let mut event = json!({
            "record_type":"device_ingest",
            "record_version":1,
            "outcome":"accepted",
            "protocol_version":3,
            "did":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "source":"",
            "stream":"device",
            "day":"20260804",
            "segment":"120000_1",
            "files":[],
            "meta":{}
        });
        event
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        event
    }

    #[test]
    fn torn_multibyte_tail_does_not_abort_valid_records() {
        let segment = segment_path("torn-multibyte");
        write_events(
            &segment,
            b"{\"tract\":\"observe\",\"event\":\"status\"}\n{\"record_type\":\"other\"}\n{\"tail\":\"\xe2",
        );

        let report = read_durable_events(&segment).unwrap();
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.unrecognized, 1);
        assert_eq!(report.unparseable, 1);
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn invalid_utf8_and_bad_json_preserve_valid_rows_and_counts() {
        let segment = segment_path("invalid-rows");
        write_events(
            &segment,
            b"{\"tract\":\"observe\",\"event\":\"status\"}\n{bad json}\n{\"record_type\":\"device_ingest\",\"record_version\":1,\"outcome\":\"accepted\",\"protocol_version\":3,\"did\":\"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"source\":\"\",\"stream\":\"device\",\"day\":\"20260804\",\"segment\":\"120000_1\",\"files\":[],\"meta\":{}}\n{\"tail\":\"\xe2",
        );

        let report = read_durable_events(&segment).unwrap();
        assert_eq!(report.records.len(), 2);
        assert_eq!(report.unparseable, 2);
        assert_eq!(report.unrecognized, 0);
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn envelope_round_trip_preserves_unknown_values_and_number_kinds() {
        let original = json!({
            "tract":"unregistered",
            "event":"future",
            "ts":123,
            "integer":3,
            "float":3.0,
            "nested":{"future":true}
        });
        let envelope: CallosumEnvelope = serde_json::from_value(original.clone()).unwrap();
        let round_trip = serde_json::to_value(envelope).unwrap();

        assert_eq!(round_trip, original);
        assert!(round_trip["integer"].as_number().unwrap().is_i64());
        assert!(round_trip["float"].as_number().unwrap().is_f64());
    }

    #[test]
    fn matching_envelopes_keep_independent_extra_keys() {
        let segment = segment_path("independent-extra");
        write_events(
            &segment,
            b"{\"tract\":\"future\",\"event\":\"same\",\"one\":1}\n{\"tract\":\"future\",\"event\":\"same\",\"two\":2}\n",
        );

        let report = read_durable_events(&segment).unwrap();
        let DurableEvent::Callosum(first) = &report.records[0] else {
            panic!("expected envelope");
        };
        let DurableEvent::Callosum(second) = &report.records[1] else {
            panic!("expected envelope");
        };
        assert_eq!(first.extra["one"], json!(1));
        assert!(first.extra.get("two").is_none());
        assert_eq!(second.extra["two"], json!(2));
        assert!(second.extra.get("one").is_none());
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn mixed_families_are_attributed_and_device_reader_counts_bus_rows() {
        let segment = segment_path("mixed-families");
        let device = device_event(json!({"future_device_key":true}));
        write_events(
            &segment,
            format!("{{\"tract\":\"future\",\"event\":\"open\",\"unknown\":true}}\n{device}\n")
                .as_bytes(),
        );

        let durable = read_durable_events(&segment).unwrap();
        assert!(matches!(durable.records[0], DurableEvent::Callosum(_)));
        let DurableEvent::DeviceIngest(event) = &durable.records[1] else {
            panic!("expected device ingest");
        };
        assert_eq!(event.extra["future_device_key"], json!(true));

        let device_only = read_device_ingest_events(&segment).unwrap();
        assert_eq!(device_only.records.len(), 1);
        assert_eq!(device_only.wrong_family, 1);
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn device_reader_reports_disjoint_skip_reasons() {
        let segment = segment_path("skip-reasons");
        write_events(
            &segment,
            b"{\"tract\":\"observe\",\"event\":\"status\"}\n{\"record_type\":\"device_ingest\"}\n{\"record_type\":\"other\"}\n",
        );

        let report = read_device_ingest_events(&segment).unwrap();
        assert!(report.records.is_empty());
        assert_eq!(report.wrong_family, 1);
        assert_eq!(report.unparseable, 1);
        assert_eq!(report.unrecognized, 1);
        let _ = fs::remove_dir_all(segment);
    }

    #[test]
    fn legacy_did_key_is_accepted_as_device_ingest() {
        let segment = segment_path("legacy-did");
        let event = device_event(json!({}));
        write_events(&segment, format!("{event}\n").as_bytes());

        let report = read_device_ingest_events(&segment).unwrap();
        assert_eq!(report.records.len(), 1);
        assert_eq!(
            report.records[0].cid,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(report.records[0].extra.get("did").is_none());
        assert_eq!(report.unparseable, 0);
        assert_eq!(report.unrecognized, 0);
        assert_eq!(report.wrong_family, 0);
        let _ = fs::remove_dir_all(segment);
    }
}
