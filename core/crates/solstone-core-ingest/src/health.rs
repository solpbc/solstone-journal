// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only device-day listing health, for `journal doctor`.
//!
//! A segment-day read refuses an unreadable day. This check runs that same
//! per-day listing for every bound device stream and names each refused day.

use std::path::Path;

use solstone_core_segment::{list_days, list_stream_bindings, read_continuity};

use crate::listing::{ListingError, merge_day_listing, native_events};
use crate::model::ReasonCode;

/// One day the device manifest would refuse for one bound device stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDayFault {
    pub day: String,
    pub stream: String,
    pub cid: String,
    /// The manifest's own reason token for the refusal.
    pub reason_code: &'static str,
}

/// Run the device manifest's per-day listing for every chain-advanced device
/// stream over the newest `recent_days` chronicle days.
///
/// `Ok(None)` means no device stream is bound, so there is nothing to judge.
pub fn device_day_listing_faults(
    journal_root: &Path,
    recent_days: usize,
) -> Result<Option<Vec<DeviceDayFault>>, String> {
    let bindings = list_stream_bindings(journal_root)
        .map_err(|error| format!("stream registry unreadable: {error}"))?
        .into_iter()
        .filter(|binding| binding.seq > 0)
        .collect::<Vec<_>>();
    let continuity = read_continuity(journal_root)
        .map_err(|error| format!("stream continuity unreadable: {error}"))?;
    if bindings.is_empty() {
        return Ok(None);
    }
    let mut days = list_days(journal_root)
        .map_err(|error| format!("chronicle unreadable: {error}"))?
        .into_iter()
        .map(|(day, _)| day)
        .collect::<Vec<_>>();
    days.sort();
    let mut faults = Vec::new();
    for day in days.into_iter().rev().take(recent_days) {
        for binding in &bindings {
            let cid = continuity
                .streams
                .get(&binding.name)
                .filter(|record| record.source == binding.source)
                .and_then(|record| record.writers.last())
                .unwrap_or(&binding.cid);
            let listing = native_events(
                journal_root,
                &day,
                Some(&binding.name),
                cid,
                &binding.source,
            )
            .and_then(|events| merge_day_listing(journal_root, &day, events));
            if let Err(error) = listing {
                faults.push(DeviceDayFault {
                    day: day.clone(),
                    stream: binding.name.clone(),
                    cid: binding.cid.clone(),
                    reason_code: day_read_reason(error).as_str(),
                });
            }
        }
    }
    Ok(Some(faults))
}

/// The manifest's reason token for a listing failure; shared with the routes.
pub(crate) fn day_read_reason(error: ListingError) -> ReasonCode {
    match error {
        ListingError::AmbiguousName => ReasonCode::AmbiguousSegmentFileName,
        ListingError::JournalRead => ReasonCode::JournalReadFailed,
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;

    use serde_json::Map;
    use solstone_core_callosum::{
        DeviceIngestEvent, DurableEvent, FileDescriptor, append_durable_event,
    };
    use solstone_core_segment::{
        Kind, SegmentDir, StreamHints, advance_bound_stream, bind_named_stream,
    };

    use super::device_day_listing_faults;

    const DAY: &str = "20261004";
    const SOURCE: &str = "browser";
    const CID_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const CID_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn hints() -> StreamHints {
        StreamHints {
            kind: Some(Kind::Observed),
            host: None,
            platform: None,
        }
    }

    fn append_event(
        root: &std::path::Path,
        cid: &str,
        stream: &str,
        segment: &str,
        files: Vec<FileDescriptor>,
    ) {
        let path = root.join("chronicle").join(DAY).join(stream).join(segment);
        fs::create_dir_all(&path).expect("segment directory");
        let bound = bind_named_stream(root, DAY, segment, stream, cid, SOURCE, &hints())
            .expect("named stream binds");
        let segment_dir = SegmentDir::resolve(root, DAY, segment, stream).expect("segment dir");
        advance_bound_stream(
            &bound.stream,
            DAY,
            segment,
            &segment_dir,
            hints(),
            cid,
            SOURCE,
        )
        .expect("stream advances");
        append_durable_event(
            segment_dir.path(),
            &DurableEvent::DeviceIngest(DeviceIngestEvent {
                record_type: "device_ingest".to_owned(),
                record_version: 1,
                outcome: "accepted".to_owned(),
                protocol_version: 3,
                cid: cid.to_owned(),
                source: SOURCE.to_owned(),
                stream: stream.to_owned(),
                day: DAY.to_owned(),
                segment: segment.to_owned(),
                files,
                meta: Map::new(),
                extra: Map::new(),
            }),
        )
        .expect("durable event appends");
    }

    fn file(submitted: &str, written: &str, digest: &str) -> FileDescriptor {
        FileDescriptor {
            submitted: submitted.to_owned(),
            written: written.to_owned(),
            size: 1,
            sha256: digest.to_owned(),
            extra: Map::new(),
        }
    }

    #[test]
    fn device_day_listing_ignores_cross_stream_shared_basename() {
        let journal = tempfile::TempDir::new().unwrap();
        for (cid, stream) in [(CID_A, "browser_a"), (CID_B, "browser_b")] {
            let path = journal
                .path()
                .join("chronicle")
                .join(DAY)
                .join(stream)
                .join("120000_10")
                .join("browser_pages.jsonl");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, stream.as_bytes()).unwrap();
            append_event(
                journal.path(),
                cid,
                stream,
                "120000_10",
                vec![file(
                    "browser_pages.jsonl",
                    "browser_pages.jsonl",
                    &"a".repeat(64),
                )],
            );
        }

        let faults = device_day_listing_faults(journal.path(), 1)
            .expect("health scan")
            .expect("bound streams");
        assert!(faults.is_empty());
    }

    #[test]
    fn device_day_listing_reports_within_stream_ambiguity() {
        let journal = tempfile::TempDir::new().unwrap();
        let segment = journal
            .path()
            .join("chronicle")
            .join(DAY)
            .join("browser_a")
            .join("120000_10");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("left.bin"), b"a").unwrap();
        fs::write(segment.join("right.bin"), b"b").unwrap();
        append_event(
            journal.path(),
            CID_A,
            "browser_a",
            "120000_10",
            vec![
                file("browser_pages.jsonl", "left.bin", &"a".repeat(64)),
                file("browser_pages.jsonl", "right.bin", &"b".repeat(64)),
            ],
        );

        let faults = device_day_listing_faults(journal.path(), 1)
            .expect("health scan")
            .expect("bound stream");
        assert_eq!(faults.len(), 1);
        assert_eq!(faults[0].reason_code, "ambiguous_segment_file_name");
    }
}
