// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only media-offload status projection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::ledger::catalog_journal;
use crate::measurement::{RawMediaPopulation, measure_local_media};
use serde_json::{Value, json};
use solstone_core_backup::get_backup_config;
use solstone_core_retention::archive_catalog::{ArchiveWitness, JournalCatalog};

/// Advisory metadata presence, never hash equality or restore/release authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogLocalPresence {
    Present { local_bytes: u64 },
    Absent,
    Unknown { known_local_bytes: Option<u64> },
    Unbound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMediaObservation {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub witness: ArchiveWitness,
    pub presence: CatalogLocalPresence,
}

/// Coverage is retained even when no active witness could be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMediaStatus {
    pub files: Vec<CatalogMediaObservation>,
    pub local_complete: bool,
    pub catalog_degraded: bool,
}

/// Join active catalog witnesses to checked local metadata by exact locator/name.
///
/// Historical unstored locators remain unbound. An absent observation requires complete
/// coverage; none of these observations certify backup safety or proposed pending relief.
pub fn observe_catalog_media(
    catalog: &JournalCatalog,
    local: &RawMediaPopulation,
) -> CatalogMediaStatus {
    let files = local
        .files
        .iter()
        .map(|file| {
            (
                (
                    file.key.day.as_str(),
                    file.key.stream.as_str(),
                    file.key.segment_dir.as_str(),
                    file.key.layout,
                    file.key.name.as_str(),
                ),
                file.bytes,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut observations = Vec::new();
    for day in &catalog.days {
        for segment in &day.segments {
            for witness in &segment.active {
                let presence = match (witness.layout, witness.segment_dir.as_ref()) {
                    (Some(layout), Some(segment_dir)) => {
                        let key = (
                            segment.day.as_str(),
                            segment.stream.as_str(),
                            segment_dir.as_str(),
                            layout,
                            witness.file.name.as_str(),
                        );
                        let known = files.get(&key).copied();
                        if catalog.degraded()
                            || day.degraded()
                            || segment.degraded()
                            || !local.complete()
                        {
                            CatalogLocalPresence::Unknown {
                                known_local_bytes: known,
                            }
                        } else if let Some(local_bytes) = known {
                            CatalogLocalPresence::Present { local_bytes }
                        } else {
                            CatalogLocalPresence::Absent
                        }
                    }
                    _ => CatalogLocalPresence::Unbound,
                };
                observations.push(CatalogMediaObservation {
                    day: segment.day.clone(),
                    stream: segment.stream.clone(),
                    segment: segment.segment.clone(),
                    witness: witness.clone(),
                    presence,
                });
            }
        }
    }
    CatalogMediaStatus {
        files: observations,
        local_complete: local.complete(),
        catalog_degraded: catalog.degraded()
            || catalog
                .days
                .iter()
                .any(|day| day.degraded() || day.segments.iter().any(|segment| segment.degraded())),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OffloadStatus {
    pub value: Value,
}

/// Checked metadata projection. Pending credit awaits an owning current verdict.
/// Device geometry is supplied separately by the consumer.
pub fn build_checked_offload_status(journal: &Path) -> Result<OffloadStatus, String> {
    let local = measure_local_media(journal);
    let catalog = catalog_journal(journal);
    let config = get_backup_config(journal).map_err(|error| error.to_string())?;
    let mut value = checked_media_value(&catalog, &local)?;
    for key in [
        "offload",
        "last_offload",
        "last_verification",
        "last_restore",
    ] {
        value[key] = config.get(key).cloned().unwrap_or(Value::Null);
    }
    value["device"] = json!({"free_bytes":null,"total_bytes":null});
    value["suggested_defaults"] = Value::Null;
    Ok(OffloadStatus { value })
}

#[derive(Default)]
struct CheckedDay {
    local_bytes: u64,
    local_files: u64,
    backup_bytes: u64,
    backup_files: u64,
    backup_segments: BTreeSet<(
        String,
        solstone_core_journal_io::paths::SegmentLayout,
        String,
    )>,
    unbound_files: u64,
    unknown_files: u64,
    present_files: u64,
    skipped_records: u64,
    unreadable_ledgers: Vec<String>,
}

fn add_count(total: &mut u64, value: u64) -> Result<(), String> {
    *total = total
        .checked_add(value)
        .ok_or("media status total overflow")?;
    Ok(())
}

fn checked_media_value(
    catalog: &JournalCatalog,
    local: &RawMediaPopulation,
) -> Result<Value, String> {
    let usage = local.known_usage()?;
    let observed = observe_catalog_media(catalog, local);
    let mut days = BTreeMap::<String, CheckedDay>::new();
    for day in &usage.per_day {
        let row = days.entry(day.day.clone()).or_default();
        row.local_bytes = day.bytes;
        row.local_files = day.files;
    }
    for day in &catalog.days {
        let row = days.entry(day.day.clone()).or_default();
        row.skipped_records = day.skipped_records;
        row.unreadable_ledgers = day.unreadable_ledgers.clone();
    }
    for observation in observed.files {
        let row = days.entry(observation.day).or_default();
        match observation.presence {
            CatalogLocalPresence::Absent => {
                add_count(&mut row.backup_bytes, observation.witness.file.bytes)?;
                add_count(&mut row.backup_files, 1)?;
                if let (Some(layout), Some(dir)) =
                    (observation.witness.layout, observation.witness.segment_dir)
                {
                    row.backup_segments
                        .insert((observation.stream, layout, dir));
                }
            }
            CatalogLocalPresence::Present { .. } => add_count(&mut row.present_files, 1)?,
            CatalogLocalPresence::Unknown { .. } => add_count(&mut row.unknown_files, 1)?,
            CatalogLocalPresence::Unbound => add_count(&mut row.unbound_files, 1)?,
        }
    }
    let mut backup_bytes = 0;
    let mut backup_files = 0;
    let mut backup_segments = 0;
    let mut backup_days = 0;
    let mut unbound_files = 0;
    let mut unknown_files = 0;
    let mut present_files = 0;
    let mut payload_days = Vec::new();
    for (day, row) in days {
        let segments = row.backup_segments.len() as u64;
        add_count(&mut backup_bytes, row.backup_bytes)?;
        add_count(&mut backup_files, row.backup_files)?;
        add_count(&mut backup_segments, segments)?;
        add_count(&mut backup_days, u64::from(row.backup_files > 0))?;
        add_count(&mut unbound_files, row.unbound_files)?;
        add_count(&mut unknown_files, row.unknown_files)?;
        add_count(&mut present_files, row.present_files)?;
        let complete = observed.local_complete
            && !observed.catalog_degraded
            && row.unbound_files == 0
            && row.unknown_files == 0;
        payload_days.push(json!({
            "day":day,
            "raw_media_bytes":observed.local_complete.then_some(row.local_bytes),
            "raw_media_files":observed.local_complete.then_some(row.local_files),
            "raw_media_known_bytes":row.local_bytes,"raw_media_known_files":row.local_files,
            "raw_media_complete":observed.local_complete,
            "backup_only_bytes":complete.then_some(row.backup_bytes),
            "backup_only_files":complete.then_some(row.backup_files),
            "backup_only_segments":complete.then_some(segments),
            "backup_only_known_bytes":row.backup_bytes,"backup_only_known_files":row.backup_files,
            "backup_only_known_segments":segments,"backup_only_complete":complete,
            "unbound_files":row.unbound_files,"unknown_files":row.unknown_files,
            "present_files":row.present_files,"degraded":observed.catalog_degraded,
            "skipped_records":row.skipped_records,"unreadable_ledgers":row.unreadable_ledgers,
            "pending_release_bytes":null,"pending_release_files":null,"pending_release_segments":null
        }));
    }
    let complete = observed.local_complete
        && !observed.catalog_degraded
        && unbound_files == 0
        && unknown_files == 0;
    let unknown_paths = local
        .unknown_paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    Ok(json!({
        "raw_media":{
            "total_bytes":observed.local_complete.then_some(usage.total_bytes),
            "total_files":observed.local_complete.then_some(usage.total_files),
            "known_bytes":usage.total_bytes,"known_files":usage.total_files,
            "complete":observed.local_complete,"unknown_paths":unknown_paths
        },
        "backup_only":{
            "total_bytes":complete.then_some(backup_bytes),"total_files":complete.then_some(backup_files),
            "total_segments":complete.then_some(backup_segments),"total_days":complete.then_some(backup_days),
            "known_bytes":backup_bytes,"known_files":backup_files,
            "known_segments":backup_segments,"known_days":backup_days,
            "complete":complete,"degraded":observed.catalog_degraded,
            "skipped_records":catalog.skipped_records,"unreadable_ledgers":catalog.unreadable_ledgers,
            "unbound_files":unbound_files,"unknown_files":unknown_files,"present_files":present_files
        },
        "proposed_release":{
            "structural_bytes":null,"eligible_bytes":null,"projected_local_bytes":null,"available":false
        },
        "pending_release":{
            "total_bytes":null,"total_files":null,"total_segments":null,"total_days":null
        },
        "days":payload_days
    }))
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use crate::measurement::{LocalMediaFile, LocalMediaKey};
    use solstone_core_journal_io::paths::SegmentLayout;
    use solstone_core_retention::archive_catalog::{DayCatalog, OffloadFile, SegmentCatalog};

    fn fixture() -> (JournalCatalog, RawMediaPopulation) {
        let witness = ArchiveWitness {
            manifest_id: "manifest".into(),
            witness_id: "witness".into(),
            snapshot_id: "a".repeat(64),
            repository_id: Some("c".repeat(64)),
            archive_path: Some("/exact/archive/raw.flac".into()),
            journal_id: Some("native-root-v1:unix:0000000000000001:0000000000000002".into()),
            layout: Some(SegmentLayout::Direct),
            segment_dir: Some("010000_001_full".into()),
            processing: Some(
                solstone_core_retention::archive_proof::AcceptedProcessingObservation {
                    record: None,
                    has_analysis_row: true,
                },
            ),
            file: OffloadFile {
                name: "raw.flac".into(),
                bytes: 17,
                sha256: "b".repeat(64),
            },
        };
        let catalog = JournalCatalog {
            days: vec![DayCatalog {
                day: "20260101".into(),
                segments: vec![SegmentCatalog {
                    day: "20260101".into(),
                    stream: "_default".into(),
                    segment: "010000_001".into(),
                    active: vec![witness],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let local = RawMediaPopulation {
            files: vec![LocalMediaFile {
                key: LocalMediaKey {
                    day: "20260101".into(),
                    stream: "_default".into(),
                    segment: "010000_001".into(),
                    segment_dir: "010000_001_full".into(),
                    layout: SegmentLayout::Direct,
                    name: "raw.flac".into(),
                },
                bytes: 7,
            }],
            unknown_paths: Vec::new(),
        };
        (catalog, local)
    }

    #[test]
    fn exact_presence_is_metadata_only_and_distinguishes_layout_and_full_basename() {
        let (mut catalog, local) = fixture();
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Present { local_bytes: 7 }
        );
        // Archived size differs: local presence grants no content identity or AlreadyPresent authority.
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0]
                .witness
                .file
                .bytes,
            17
        );
        catalog.days[0].segments[0].segment = "grouping-only".into();
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Present { local_bytes: 7 }
        );
        catalog.days[0].segments[0].active[0].layout = Some(SegmentLayout::Named);
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Absent
        );
        catalog.days[0].segments[0].active[0].layout = Some(SegmentLayout::Direct);
        catalog.days[0].segments[0].active[0].segment_dir = Some("010000_001_other".into());
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Absent
        );
    }

    #[test]
    fn incomplete_and_degraded_coverage_stays_unknown_with_known_bytes_advisory() {
        let (mut catalog, mut local) = fixture();
        local.unknown_paths.push("unreadable".into());
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Unknown {
                known_local_bytes: Some(7)
            }
        );
        local.files.clear();
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Unknown {
                known_local_bytes: None
            }
        );
        local.unknown_paths.clear();
        catalog.days[0].skipped_records = 1;
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Unknown {
                known_local_bytes: None
            }
        );
        catalog.days[0].skipped_records = 0;
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Absent
        );
    }

    #[test]
    fn legacy_locator_is_unbound_and_inactive_history_is_not_observed() {
        let (mut catalog, local) = fixture();
        catalog.days[0].segments[0].active[0].layout = None;
        catalog.days[0].segments[0].active[0].segment_dir = None;
        assert_eq!(
            observe_catalog_media(&catalog, &local).files[0].presence,
            CatalogLocalPresence::Unbound
        );
        catalog.days[0].segments[0].active.clear();
        assert!(observe_catalog_media(&catalog, &local).files.is_empty());
    }

    #[test]
    fn empty_observations_preserve_unknown_coverage() {
        let catalog = JournalCatalog {
            unreadable_ledgers: vec!["unreadable-ledger".into()],
            ..Default::default()
        };
        let local = RawMediaPopulation {
            files: Vec::new(),
            unknown_paths: vec!["unreadable-media".into()],
        };
        let status = observe_catalog_media(&catalog, &local);
        assert!(status.files.is_empty());
        assert!(!status.local_complete);
        assert!(status.catalog_degraded);
    }

    #[test]
    fn checked_totals_keep_actual_local_bytes_and_pending_unavailable() {
        let (catalog, mut local) = fixture();
        let mut ordinary = local.files[0].clone();
        ordinary.key.name = "ordinary.webm".into();
        ordinary.bytes = 11;
        let mut kept = ordinary.clone();
        kept.key.name = "kept.flac".into();
        kept.bytes = 5;
        local.files.extend([ordinary, kept]);
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["raw_media"]["total_bytes"], 23);
        assert_eq!(value["raw_media"]["total_files"], 3);
        assert_eq!(value["backup_only"]["present_files"], 1);
        assert_eq!(value["backup_only"]["total_bytes"], 0);
        assert!(value["pending_release"]["total_bytes"].is_null());
        assert!(value["proposed_release"]["structural_bytes"].is_null());
        assert!(value["proposed_release"]["eligible_bytes"].is_null());
        assert!(value["proposed_release"]["projected_local_bytes"].is_null());
        assert_eq!(value["proposed_release"]["available"], false);
    }

    #[test]
    fn checked_backup_totals_group_exact_active_absent_targets_only() {
        let (mut catalog, _) = fixture();
        let first = catalog.days[0].segments[0].active[0].clone();
        let mut named = first.clone();
        named.witness_id = "named".into();
        named.layout = Some(SegmentLayout::Named);
        named.snapshot_id = "d".repeat(64);
        named.file.bytes = 3;
        let mut other_name = first.clone();
        other_name.witness_id = "other-name".into();
        other_name.file.name = "other.flac".into();
        other_name.file.bytes = 5;
        let segment = &mut catalog.days[0].segments[0];
        segment.active.extend([named, other_name]);
        segment
            .history
            .push(solstone_core_retention::archive_catalog::ArchiveManifest {
                manifest_id: "inactive".into(),
                files: vec![first],
            });
        let local = RawMediaPopulation::default();
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["backup_only"]["total_bytes"], 25);
        assert_eq!(value["backup_only"]["total_files"], 3);
        assert_eq!(value["backup_only"]["total_segments"], 2);
        assert_eq!(value["backup_only"]["total_days"], 1);
        assert_eq!(value["days"][0]["backup_only_bytes"], 25);
        assert_eq!(value["backup_only"]["complete"], true);
        catalog.days[0].segments[0].active[0].file.bytes = u64::MAX;
        assert!(checked_media_value(&catalog, &local).is_err());
    }

    #[test]
    fn checked_local_and_catalog_uncertainty_remain_independent() {
        let (mut catalog, mut local) = fixture();
        catalog.unreadable_ledgers.push("blocked".into());
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["raw_media"]["complete"], true);
        assert_eq!(value["raw_media"]["total_bytes"], 7);
        assert_eq!(value["backup_only"]["degraded"], true);
        assert!(value["backup_only"]["total_bytes"].is_null());
        assert_eq!(value["backup_only"]["unknown_files"], 1);
        local.unknown_paths.push("unreadable-media".into());
        let value = checked_media_value(&catalog, &local).unwrap();
        assert!(value["raw_media"]["total_bytes"].is_null());
        assert_eq!(value["raw_media"]["known_bytes"], 7);
        assert_eq!(value["days"][0]["raw_media_complete"], false);
        catalog.unreadable_ledgers.clear();
        local.unknown_paths.clear();
        catalog.days[0].segments[0].active[0].layout = None;
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["backup_only"]["unbound_files"], 1);
        assert_eq!(value["backup_only"]["unknown_files"], 0);
        assert!(value["backup_only"]["total_bytes"].is_null());
        local.files[0].bytes = u64::MAX;
        let mut extra = local.files[0].clone();
        extra.bytes = 1;
        extra.key.name = "other.flac".into();
        local.files.push(extra);
        assert!(checked_media_value(&catalog, &local).is_err());
    }

    #[test]
    fn checked_empty_totals_distinguish_complete_zero_from_unknown() {
        let mut catalog = JournalCatalog::default();
        let mut local = RawMediaPopulation::default();
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["raw_media"]["total_bytes"], 0);
        assert_eq!(value["backup_only"]["total_bytes"], 0);
        assert_eq!(value["backup_only"]["complete"], true);
        assert_eq!(value["days"], json!([]));
        catalog.skipped_records = 1;
        let value = checked_media_value(&catalog, &local).unwrap();
        assert_eq!(value["raw_media"]["total_bytes"], 0);
        assert!(value["backup_only"]["total_bytes"].is_null());
        assert_eq!(value["backup_only"]["degraded"], true);
        local.unknown_paths.push("unknown-root".into());
        let value = checked_media_value(&catalog, &local).unwrap();
        assert!(value["raw_media"]["total_bytes"].is_null());
        assert_eq!(value["raw_media"]["known_bytes"], 0);
        assert_eq!(value["raw_media"]["complete"], false);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::ledger::append_offload_event;
    use solstone_core_retention::archive_catalog::OffloadFile;
    use std::fs;

    #[test]
    fn checked_builder_uses_plain_media_without_ledger_subtraction() {
        let journal = tempfile::tempdir().unwrap();
        let segment = journal.path().join("chronicle/20260101/010000_001");
        fs::create_dir_all(&segment).unwrap();
        for (name, bytes) in [
            ("ordinary.webm", 11),
            ("pending.flac", 7),
            ("kept.flac", 5),
            ("metadata.txt", 100),
        ] {
            fs::write(segment.join(name), vec![0; bytes]).unwrap();
        }
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "snapshot",
            &[OffloadFile {
                name: "pending.flac".into(),
                bytes: 7,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        let value = build_checked_offload_status(journal.path()).unwrap().value;
        assert_eq!(value["raw_media"]["total_bytes"], 23);
        assert_eq!(value["raw_media"]["total_files"], 3);
        assert_eq!(value["backup_only"]["unbound_files"], 1);
        assert!(value["pending_release"]["total_bytes"].is_null());
        assert!(value["device"]["total_bytes"].is_null());
        let ledger = crate::ledger::ledger_path_for_day(journal.path(), "20260101").unwrap();
        fs::write(&ledger, b"{broken}\n").unwrap();
        let value = build_checked_offload_status(journal.path()).unwrap().value;
        assert_eq!(value["raw_media"]["total_bytes"], 23);
        assert_eq!(value["raw_media"]["complete"], true);
        assert_eq!(value["backup_only"]["degraded"], true);
        assert!(value["backup_only"]["total_bytes"].is_null());
        assert!(value["proposed_release"]["eligible_bytes"].is_null());
        assert_eq!(fs::read(ledger).unwrap(), b"{broken}\n");
    }

    #[test]
    fn checked_status_keeps_multiday_local_media_and_legacy_witnesses_unbound() {
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
        let value = build_checked_offload_status(journal.path()).unwrap().value;
        assert!(value["offload"]["enabled"].is_boolean());
        assert_eq!(value["raw_media"]["total_bytes"], 33);
        assert_eq!(value["raw_media"]["total_files"], 3);
        assert_eq!(value["backup_only"]["unbound_files"], 2);
        assert!(value["backup_only"]["total_bytes"].is_null());
        assert!(value["pending_release"]["total_bytes"].is_null());
        assert_eq!(value["proposed_release"]["available"], false);
        assert!(value["device"]["free_bytes"].is_null());
        assert!(value["device"]["total_bytes"].is_null());
        let days = value["days"].as_array().unwrap();
        assert_eq!(days.len(), 2);
        for (day, bytes, files) in [("20260101", 13, 2), ("20260102", 20, 1)] {
            let row = days.iter().find(|row| row["day"] == day).unwrap();
            assert_eq!(row["raw_media_bytes"], bytes);
            assert_eq!(row["raw_media_files"], files);
            assert_eq!(row["raw_media_complete"], true);
            assert_eq!(row["unbound_files"], 1);
            assert_eq!(row["backup_only_complete"], false);
            assert!(row["backup_only_bytes"].is_null());
            assert!(row["pending_release_bytes"].is_null());
        }
    }

    #[test]
    fn checked_status_leaves_legacy_witness_unbound_across_exact_basename_ambiguity() {
        let journal = tempfile::tempdir().unwrap();
        let first = journal.path().join("chronicle/20260101/other/010000_001_a");
        let second = journal.path().join("chronicle/20260101/other/010000_001_b");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        append_offload_event(
            journal.path(),
            "20260101",
            "other",
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
        let value = build_checked_offload_status(journal.path()).unwrap().value;
        assert_eq!(value["backup_only"]["unbound_files"], 1);
        assert_eq!(value["backup_only"]["unknown_files"], 0);
        assert_eq!(value["backup_only"]["present_files"], 0);
        assert_eq!(value["backup_only"]["complete"], false);
        assert!(value["backup_only"]["total_bytes"].is_null());
        assert!(value["pending_release"]["total_bytes"].is_null());
        let days = value["days"].as_array().unwrap();
        let day = days.iter().find(|row| row["day"] == "20260101").unwrap();
        assert_eq!(day["raw_media_bytes"], 0);
        assert_eq!(day["unbound_files"], 1);
        assert!(day["backup_only_segments"].is_null());
        assert!(day["pending_release_segments"].is_null());
    }
}
