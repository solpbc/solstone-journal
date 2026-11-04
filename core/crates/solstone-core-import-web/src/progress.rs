// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Current thinking progress over every published recording, independent of
//! pending spending permission or a past completion event.

use serde_json::{Value, json};
use solstone_core_import::{
    publish::{SegmentBindingOutcome, read_publication_record},
    size::Recording,
};
use solstone_core_system::work_retry::{self, Unit};
use solstone_core_system_health as health;
use std::{collections::BTreeMap, path::Path};

pub(crate) fn read(root: &Path, import_id: &str) -> Value {
    match read_counts(root, import_id) {
        Ok(Some(counts)) => counts,
        Ok(None) => json!({"status": "not_published"}),
        Err(_) => json!({"status": "unavailable"}),
    }
}

fn read_counts(root: &Path, import_id: &str) -> Result<Option<Value>, String> {
    let Some(record) = read_publication_record(&root.join("imports").join(import_id))
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    if !matches!(
        record.schema.as_str(),
        solstone_core_import::publish::PUBLICATION_SCHEMA
            | solstone_core_import::publish::LEGACY_PUBLICATION_SCHEMA
    ) {
        return Err("unrecognized import publication".to_owned());
    }
    let mut days: BTreeMap<String, std::collections::BTreeSet<Recording>> = BTreeMap::new();
    for segment in record.segments {
        if !matches!(segment.outcome, SegmentBindingOutcome::Bound { .. }) {
            continue;
        }
        Unit::Segment {
            day: segment.day.clone(),
            stream: segment.stream.clone(),
            segment: segment.segment.clone(),
            talent: "sense".to_owned(),
            direct: false,
        }
        .validate()?;
        days.entry(segment.day.clone())
            .or_default()
            .insert(Recording {
                day: segment.day,
                stream: segment.stream,
                segment: segment.segment,
            });
    }
    let total: usize = days.values().map(|items| items.len()).sum();
    let (holds, hold_errors) = work_retry::list(root);
    let mut ready = 0;
    let mut held = 0;
    let mut waiting = 0;
    let mut unreadable = 0;
    for (day, recordings) in days {
        let scan = health::scan_day(
            &health::FilesystemSegmentSource,
            root,
            &day,
            chrono::Utc::now(),
        )
        .map_err(|error| error.to_string());
        let progress =
            health::read_segment_progress(&health::FilesystemHealthLogSource::new(root), &day)
                .map_err(|error| error.to_string());
        let (Ok((_, _, segments)), Ok(progress)) = (scan, progress) else {
            unreadable += recordings.len();
            continue;
        };
        if progress.malformed_line_count != 0 {
            unreadable += recordings.len();
            continue;
        }
        for recording in recordings {
            let Some(segment) = segments.iter().find(|segment| {
                segment.stream == recording.stream && segment.key == recording.segment
            }) else {
                unreadable += 1;
                continue;
            };
            let has_hold = holds.iter().any(|hold| {
                hold.held
                    && match &hold.unit {
                        Unit::Raw {
                            day: hold_day,
                            stream,
                            segment: key,
                            ..
                        }
                        | Unit::Segment {
                            day: hold_day,
                            stream,
                            segment: key,
                            ..
                        } => {
                            hold_day == &day
                                && stream == &recording.stream
                                && key == &recording.segment
                        }
                        _ => false,
                    }
            });
            let capped = health::lookup_segment_progress(
                &progress.value,
                &recording.stream,
                &recording.segment,
            )
            .is_some_and(|value| !value.capped_by_skip.is_empty());
            if has_hold
                || capped
                || segment
                    .data_state
                    .0
                    .values()
                    .any(|state| state == "failed_final")
            {
                held += 1;
                continue;
            }
            match health::segment_work_ready(root, &day, segment, &progress.value) {
                Ok(true) => {
                    ready += 1;
                    continue;
                }
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
                Ok(false) => {}
            }
            if !hold_errors.is_empty() {
                unreadable += 1;
            } else {
                waiting += 1;
            }
        }
    }
    Ok(Some(
        json!({ "status": "current", "recordings": total, "ready": ready, "held": held, "waiting": waiting, "unreadable": unreadable }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_import::publish::*;
    use std::fs;

    const ID: &str = "20261011_120000";
    const DAY: &str = "20261010";
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("imports").join(ID);
        fs::create_dir_all(&directory).unwrap();
        write_publication_record(
            Some(&directory),
            &PublicationRecord {
                schema: PUBLICATION_SCHEMA.to_owned(),
                status: PublicationStatus::Success,
                segments: vec![SegmentPublicationOutcome {
                    day: DAY.to_owned(),
                    stream: "mcp.agent".to_owned(),
                    segment: "120000_60".to_owned(),
                    outcome: SegmentBindingOutcome::Bound {
                        prev_day: None,
                        prev_segment: None,
                        seq: 1,
                    },
                    body_only: false,
                }],
                indexing: IndexPublicationOutcomes::default(),
                day_markers: vec![],
            },
        )
        .unwrap();
        root
    }
    #[test]
    fn retired_consent_does_not_remove_recordings_or_claim_readiness() {
        let root = fixture();
        let inventory = solstone_core_import::thinking::read(root.path()).unwrap();
        let import = &inventory.imports[0];
        solstone_core_import::thinking::fulfill(
            root.path(),
            import,
            import.segments.clone(),
            import.days.clone(),
        )
        .unwrap();
        let before =
            solstone_core_import::snapshot::fingerprint(&root.path().join("imports")).unwrap();
        let counts = read(root.path(), ID);
        assert_eq!(counts["recordings"], 1);
        assert_eq!(counts["ready"], 0);
        assert_eq!(counts["unreadable"], 1);
        assert_eq!(
            solstone_core_import::snapshot::fingerprint(&root.path().join("imports")).unwrap(),
            before,
            "reading progress cannot renew consent"
        );
    }
    #[test]
    fn current_reads_notice_readiness_holds_and_removed_outputs() {
        let root = fixture();
        let directory = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("mcp.agent/120000_60");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("imported.md"), "a saved interaction").unwrap();
        assert_eq!(read(root.path(), ID)["ready"], 1);
        work_retry::observe_held(
            root.path(),
            &Unit::Segment {
                day: DAY.to_owned(),
                stream: "mcp.agent".to_owned(),
                segment: "120000_60".to_owned(),
                talent: "sense".to_owned(),
                direct: false,
            },
            "test_failure",
            "input",
        )
        .unwrap();
        let held = read(root.path(), ID);
        assert_eq!(held["held"], 1);
        assert_eq!(held["ready"], 0);
        fs::remove_dir_all(directory).unwrap();
        assert_eq!(read(root.path(), ID)["unreadable"], 1);
    }
    #[test]
    fn malformed_publication_is_unavailable_instead_of_zero_complete() {
        let root = fixture();
        let path = root.path().join("imports").join(ID).join("imported.json");
        fs::write(&path, b"not json").unwrap();
        assert_eq!(read(root.path(), ID)["status"], "unavailable");
        fs::remove_file(path).unwrap();
        assert_eq!(read(root.path(), ID)["status"], "not_published");
    }
}
