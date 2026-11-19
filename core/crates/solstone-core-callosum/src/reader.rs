// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{CallosumEnvelope, DeviceIngestEvent, DurableEvent, ReportedZone};

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

    Ok(parse_durable_events(&contents))
}

fn parse_durable_events(contents: &[u8]) -> DurableEventsReport {
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
    report
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

/// The zone reported for this segment, either from its latest device ingest
/// receipt or from an imported photo's `image_meta.json`. A segment without
/// receipts or photo zone metadata, or whose metadata cannot be read, has no
/// reported zone.
pub fn read_reported_zone(journal_root: &Path, segment_path: &Path) -> Option<ReportedZone> {
    // Zone metadata is optional. Refuse symbolic links, nonregular files and oversized
    // files without affecting the segment's readable transcript.
    const MAX_REPORTED_ZONE_BYTES: usize = 1024 * 1024;
    const MAX_IMAGE_META_BYTES: usize = 64 * 1024;
    let absolute_root = std::path::absolute(journal_root).ok()?;
    let root = solstone_core_journal_io::JournalRoot::open(&absolute_root).ok()?;
    let segment_relative = segment_path.strip_prefix(journal_root).ok()?;
    let events_relative = segment_relative.join(EVENTS_FILE);
    if let Ok(Some(observed)) = solstone_core_journal_io::read_relative_file_bounded(
        &root,
        &events_relative,
        MAX_REPORTED_ZONE_BYTES,
    ) && let Some(zone) = parse_durable_events(&observed.bytes)
        .records
        .iter()
        .rev()
        .find_map(|record| match record {
            DurableEvent::DeviceIngest(event) => event.reported_zone(),
            DurableEvent::Callosum(_) => None,
        })
    {
        return Some(zone);
    }

    let meta_relative = segment_relative.join("image_meta.json");
    let observed = solstone_core_journal_io::read_relative_file_bounded(
        &root,
        &meta_relative,
        MAX_IMAGE_META_BYTES,
    )
    .ok()??;
    parse_image_meta_zone(&observed.bytes)
}

fn parse_image_meta_zone(bytes: &[u8]) -> Option<ReportedZone> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object()?;
    let tz = object
        .get("tz")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let utc_offset_seconds = object
        .get("utc_offset_seconds")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| (-86_399..=86_399).contains(value));
    if tz.is_none() && utc_offset_seconds.is_none() {
        return None;
    }
    Some(ReportedZone {
        tz,
        utc_offset_seconds,
    })
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

    #[test]
    fn reported_zone_comes_from_the_latest_receipt_that_carries_one() {
        let segment = segment_path("reported-zone");
        let lines = [
            device_event(json!({"meta":{"tz":"America/Denver","utc_offset_seconds":-21600}})),
            device_event(json!({"meta":{"tz":"Asia/Tokyo","utc_offset_seconds":32400}})),
            device_event(json!({"meta":{}})),
        ]
        .iter()
        .map(|event| format!("{event}\n"))
        .collect::<String>();
        write_events(&segment, lines.as_bytes());

        assert_eq!(
            read_reported_zone(segment.parent().unwrap(), &segment),
            Some(ReportedZone {
                tz: Some("Asia/Tokyo".to_owned()),
                utc_offset_seconds: Some(32400),
            })
        );
        assert_eq!(
            read_reported_zone(
                segment.parent().unwrap(),
                &segment_path("reported-zone-absent")
            ),
            None
        );
        let _ = fs::remove_dir_all(segment);
    }
    #[cfg(feature = "full-tests")]
    #[test]
    fn reported_zone_only_reads_bounded_regular_receipts() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20260309/device/120000_1");
        let row = device_event(json!({"meta":{"tz":"Asia/Tokyo","utc_offset_seconds":32400}}));
        let line = format!("{row}\n");
        let mut exact = line.into_bytes();
        exact.resize(1024 * 1024, b' ');
        write_events(&segment, &exact);
        assert_eq!(
            read_reported_zone(journal.path(), &segment)
                .unwrap()
                .utc_offset_seconds,
            Some(32400)
        );
        exact.push(b' ');
        write_events(&segment, &exact);
        assert_eq!(read_reported_zone(journal.path(), &segment), None);
        fs::remove_file(segment.join(EVENTS_FILE)).unwrap();
        fs::create_dir(segment.join(EVENTS_FILE)).unwrap();
        assert_eq!(read_reported_zone(journal.path(), &segment), None);
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn reported_zone_accepts_relative_journal_paths() {
        const CHILD: &str = "SOLSTONE_TEST_RELATIVE_REPORTED_ZONE";
        if std::env::var_os(CHILD).is_none() {
            let directory = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "reader::tests::reported_zone_accepts_relative_journal_paths",
                    "--nocapture",
                ])
                .current_dir(directory.path())
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "relative-path child failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            assert_eq!(
                fs::read(directory.path().join("relative-zone-verified")).unwrap(),
                b"passed"
            );
            return;
        }
        let relative_root = std::path::Path::new("journal");
        let relative_segment = relative_root.join("chronicle/20260309/device/120000_1");
        let row = device_event(json!({"meta":{"tz":"Asia/Tokyo","utc_offset_seconds":32400}}));
        write_events(&relative_segment, format!("{row}\n").as_bytes());
        assert_eq!(
            read_reported_zone(relative_root, &relative_segment)
                .unwrap()
                .utc_offset_seconds,
            Some(32400)
        );
        fs::write("relative-zone-verified", b"passed").unwrap();
    }

    #[test]
    fn reported_zone_falls_back_to_image_meta_and_refuses_symlinks() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal
            .path()
            .join("chronicle/20261112/import.image/211400_0");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("image_meta.json"),
            br#"{"schema":1,"time_source":"capture","utc_offset_seconds":32400}"#,
        )
        .unwrap();
        assert_eq!(
            read_reported_zone(journal.path(), &segment),
            Some(ReportedZone {
                tz: None,
                utc_offset_seconds: Some(32400),
            })
        );

        fs::write(
            segment.join("image_meta.json"),
            br#"{"schema":1,"time_source":"upload_fallback"}"#,
        )
        .unwrap();
        assert_eq!(read_reported_zone(journal.path(), &segment), None);

        #[cfg(unix)]
        {
            let target = journal.path().join("outside.json");
            fs::write(&target, br#"{"utc_offset_seconds":32400}"#).unwrap();
            fs::remove_file(segment.join("image_meta.json")).unwrap();
            std::os::unix::fs::symlink(&target, segment.join("image_meta.json")).unwrap();
            assert_eq!(read_reported_zone(journal.path(), &segment), None);
        }
    }
}
