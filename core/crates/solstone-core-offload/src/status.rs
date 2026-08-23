// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only media-offload status projection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_backup::get_backup_config;
use solstone_core_journal_io::paths::{PathOrDay, iter_segments, segment_path};

use crate::ledger::summarize_journal;
use crate::measurement::{
    RawMediaUsage, SuggestedOffloadDefaults, device_free_bytes, device_total_bytes,
    measure_raw_media_usage, suggest_offload_defaults,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffloadStatusMeasurement {
    pub usage: RawMediaUsage,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub suggested_defaults: SuggestedOffloadDefaults,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OffloadStatus {
    pub value: Value,
}

pub fn measure_offload_status(journal: &Path) -> Result<OffloadStatusMeasurement, String> {
    let total_bytes = device_total_bytes(journal)?;
    Ok(OffloadStatusMeasurement {
        usage: measure_raw_media_usage(journal),
        free_bytes: device_free_bytes(journal)?,
        total_bytes,
        suggested_defaults: suggest_offload_defaults(total_bytes)?,
    })
}

pub fn build_offload_status(journal: &Path) -> Result<OffloadStatus, String> {
    build_offload_status_with(journal, measure_offload_status(journal)?)
}

fn build_offload_status_with(
    journal: &Path,
    measurement: OffloadStatusMeasurement,
) -> Result<OffloadStatus, String> {
    let config = get_backup_config(journal).map_err(|error| error.to_string())?;
    let ledger = summarize_journal(journal);
    let mut raw_by_day = measurement
        .usage
        .per_day
        .iter()
        .map(|day| (day.day.clone(), (day.bytes, day.files)))
        .collect::<BTreeMap<_, _>>();
    let mut backup_only = BTreeMap::<String, (u64, u64, u64, bool, u64, Vec<String>)>::new();
    let mut pending = BTreeMap::<String, (u64, u64, u64)>::new();
    let mut ambiguous = BTreeMap::<String, Vec<String>>::new();
    for day in &ledger.days {
        let mut backup = (0, 0, 0);
        let mut pending_counts = (0, 0, 0);
        for segment in &day.segments {
            if !segment.currently_offloaded {
                continue;
            }
            let listed = iter_segments(journal, PathOrDay::Day(&segment.day))
                .map_err(|error| error.to_string())?;
            let matches: Vec<_> = listed
                .iter()
                .filter(|found| {
                    found.record_identity().is_some_and(|identity| {
                        identity.stream == segment.stream && identity.key == segment.segment
                    })
                })
                .collect();
            if matches.len() > 1 {
                ambiguous
                    .entry(day.day.clone())
                    .or_default()
                    .push(format!("{}/{}", segment.stream, segment.segment));
                continue;
            }
            let directory = match matches.as_slice() {
                [found] => found.path().to_path_buf(),
                _ => segment_path(
                    journal,
                    &segment.day,
                    &segment.segment,
                    &segment.stream,
                    false,
                )
                .map_err(|error| error.to_string())?,
            };
            let (mut has_backup, mut has_pending) = (false, false);
            for file in &segment.files {
                if directory.join(&file.name).is_file() {
                    pending_counts.0 += file.bytes;
                    pending_counts.1 += 1;
                    has_pending = true;
                } else {
                    backup.0 += file.bytes;
                    backup.1 += 1;
                    has_backup = true;
                }
            }
            backup.2 += u64::from(has_backup);
            pending_counts.2 += u64::from(has_pending);
        }
        backup_only.insert(
            day.day.clone(),
            (
                backup.0,
                backup.1,
                backup.2,
                day.degraded(),
                day.skipped_records,
                day.unreadable_ledgers.clone(),
            ),
        );
        pending.insert(day.day.clone(), pending_counts);
    }
    let days = raw_by_day
        .keys()
        .chain(backup_only.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let pending_bytes: u64 = pending.values().map(|value| value.0).sum();
    let pending_files: u64 = pending.values().map(|value| value.1).sum();
    let backup_bytes: u64 = backup_only.values().map(|value| value.0).sum();
    let backup_files: u64 = backup_only.values().map(|value| value.1).sum();
    let backup_segments: u64 = backup_only.values().map(|value| value.2).sum();
    let payload_days = days.into_iter().map(|day| { let raw = raw_by_day.remove(&day).unwrap_or((0, 0)); let backup = backup_only.remove(&day).unwrap_or((0, 0, 0, false, 0, vec![])); let pending = pending.remove(&day).unwrap_or((0, 0, 0)); let ambiguous_matches = ambiguous.remove(&day).unwrap_or_default(); json!({"day":day,"raw_media_bytes":raw.0.saturating_sub(pending.0),"raw_media_files":raw.1.saturating_sub(pending.1),"backup_only_bytes":backup.0,"backup_only_files":backup.1,"backup_only_segments":backup.2,"degraded":backup.3,"skipped_records":backup.4,"unreadable_ledgers":backup.5,"pending_release_bytes":pending.0,"pending_release_files":pending.1,"pending_release_segments":pending.2,"ambiguous_ledger_matches":ambiguous_matches}) }).collect::<Vec<_>>();
    Ok(OffloadStatus {
        value: json!({
            "offload":config.get("offload").cloned().unwrap_or(Value::Null),
            "last_offload":config.get("last_offload").cloned().unwrap_or(Value::Null),
            "last_verification":config.get("last_verification").cloned().unwrap_or(Value::Null),
            "last_restore":config.get("last_restore").cloned().unwrap_or(Value::Null),
            "device":{"free_bytes":measurement.free_bytes,"total_bytes":measurement.total_bytes},
            "suggested_defaults":{"budget_bytes":measurement.suggested_defaults.budget_bytes,"floor_bytes":measurement.suggested_defaults.floor_bytes},
            "raw_media":{"total_bytes":measurement.usage.total_bytes.saturating_sub(pending_bytes),"total_files":measurement.usage.total_files.saturating_sub(pending_files)},
            "backup_only":{"total_bytes":backup_bytes,"total_files":backup_files,"total_segments":backup_segments,"total_days":ledger.days.iter().filter(|day|day.offloaded_segments>0).count(),"degraded":ledger.degraded(),"skipped_records":ledger.skipped_records,"unreadable_ledgers":ledger.unreadable_ledgers},
            "pending_release":{"total_bytes":pending_bytes,"total_files":pending_files,"total_segments":payload_days.iter().map(|value|value["pending_release_segments"].as_u64().unwrap_or(0)).sum::<u64>(),"total_days":payload_days.iter().filter(|value|value["pending_release_segments"].as_u64().unwrap_or(0)>0).count()},
            "has_ambiguous_ledger_matches": payload_days.iter().any(|value| value["ambiguous_ledger_matches"].as_array().is_some_and(|items| !items.is_empty())),
            "days":payload_days
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{OffloadFile, append_offload_event};
    use std::fs;

    #[test]
    fn status_folds_distinct_multi_day_pending_and_backup_only_media() {
        let journal = tempfile::tempdir().unwrap();
        let first = journal.path().join("chronicle/20260101/010000_001");
        let second = journal.path().join("chronicle/20260102/020000_001");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("pending.webm"), b"abc").unwrap();
        fs::write(first.join("other.webm"), b"0123456789").unwrap();
        fs::write(second.join("raw.webm"), b"01234567890123456789").unwrap();
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot-a",
            &[OffloadFile {
                name: "pending.webm".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "snapshot-b",
            &[OffloadFile {
                name: "backup.webm".into(),
                bytes: 7,
                sha256: "b".repeat(64),
            }],
            2,
        )
        .unwrap();
        let value = build_offload_status(journal.path()).unwrap().value;
        assert!(value["offload"]["enabled"].is_boolean());
        let numbers = [
            value["raw_media"]["total_bytes"].as_u64().unwrap(),
            value["pending_release"]["total_bytes"].as_u64().unwrap(),
            value["backup_only"]["total_bytes"].as_u64().unwrap(),
            value["device"]["total_bytes"].as_u64().unwrap(),
        ];
        assert_eq!(numbers[..3], [30, 3, 7]);
        assert_ne!(numbers[0], numbers[1]);
        assert_ne!(numbers[1], numbers[2]);
        assert_ne!(numbers[2], numbers[3]);
    }

    #[test]
    fn status_surfaces_ambiguous_ledger_matches_apart_from_not_offloaded() {
        let journal = tempfile::tempdir().unwrap();
        let direct = journal.path().join("chronicle/20260101/010000_001");
        let named = journal
            .path()
            .join("chronicle/20260101/_default/010000_001");
        fs::create_dir_all(&direct).unwrap();
        fs::create_dir_all(&named).unwrap();
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot-a",
            &[OffloadFile {
                name: "pending.webm".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        let value = build_offload_status(journal.path()).unwrap().value;
        assert_eq!(value["has_ambiguous_ledger_matches"], true);
        let days = value["days"].as_array().unwrap();
        let day = days.iter().find(|row| row["day"] == "20260101").unwrap();
        assert_eq!(
            day["ambiguous_ledger_matches"],
            serde_json::json!(["_default/010000_001"])
        );
        assert_eq!(day["backup_only_segments"].as_u64().unwrap(), 0);
        assert_eq!(day["pending_release_segments"].as_u64().unwrap(), 0);
    }
}
