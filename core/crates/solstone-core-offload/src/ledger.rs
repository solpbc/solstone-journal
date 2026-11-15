// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Durable JSONL pre-mark witness for media offload.
//!
//! This is JSONL rather than SQLite because backups exclude `*.sqlite*` but
//! include `health/`. Appends are fsynced by journal-io: this record exists
//! before a pending-release mark is minted. Folding is append order, never
//! timestamp order; malformed reads degrade rather than creating a trusted zero.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::append_jsonl;
use solstone_core_retention::archive_catalog::{
    DayCatalog, JournalCatalog, OffloadFile, SegmentCatalog,
    codec::{component, fold_lines, valid_day, valid_file},
};

// Additive recording leaves stay uncalled until orchestration is qualified.
#[allow(dead_code)]
pub(crate) mod writer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SegmentOffloadSummary {
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub currently_offloaded: bool,
    pub snapshot_id: Option<String>,
    pub files: Vec<OffloadFile>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayOffloadSummary {
    pub day: String,
    pub segments: Vec<SegmentOffloadSummary>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub offloaded_segments: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalOffloadSummary {
    pub days: Vec<DayOffloadSummary>,
    pub offloaded_bytes: u64,
    pub offloaded_file_count: u64,
    pub offloaded_segments: u64,
    pub offloaded_days: u64,
    pub skipped_records: u64,
    pub unreadable_ledgers: Vec<String>,
}
impl SegmentOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl DayOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}
impl JournalOffloadSummary {
    pub fn degraded(&self) -> bool {
        self.skipped_records > 0 || !self.unreadable_ledgers.is_empty()
    }
}

/// Media archives a backup erase rewrote: each archive's first id, as the
/// ledger's witnesses name it, to the id it has now. Beside the day ledgers and
/// backed up with them; a `.json`, so the ledger listing passes over it.
const MOVES_FILE: &str = "health/offload/archive-moves.json";

/// The archive moves recorded so far.
pub fn archive_moves(journal: &Path) -> std::collections::BTreeMap<String, String> {
    fs::read(journal.join(MOVES_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value.get("moves").cloned())
        .and_then(|moves| serde_json::from_value(moves).ok())
        .unwrap_or_default()
}

/// Record that archives were rewritten: `original` is now `current`. Later
/// records win. Ids are full restic snapshot ids.
pub fn record_archive_moves(journal: &Path, moves: &[(String, String)]) -> Result<(), String> {
    if moves.is_empty() {
        return Ok(());
    }
    let hex = |id: &str| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    if moves.iter().any(|(from, to)| !hex(from) || !hex(to)) {
        return Err("invalid archive move".into());
    }
    let mut all = archive_moves(journal);
    for (original, current) in moves {
        all.insert(original.clone(), current.clone());
    }
    fs::create_dir_all(journal.join("health/offload")).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({"schema": 1, "moves": all}))
        .map_err(|error| error.to_string())?;
    solstone_core_journal_io::atomic::atomic_replace(
        journal.join(MOVES_FILE),
        &bytes,
        solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
    )
    .map_err(|error| error.to_string())
}

/// A witness's archive as it is now: the id a backup erase moved it to, found
/// by the original id, or by a prefix of it for a witness that names a short
/// one.
fn moved(moves: &std::collections::BTreeMap<String, String>, id: &str) -> Option<String> {
    if id.len() < 8 {
        return None;
    }
    moves.get(id).cloned().or_else(|| {
        let mut found = moves
            .iter()
            .filter(|(original, _)| original.starts_with(id));
        match (found.next(), found.next()) {
            (Some((_, current)), None) => Some(current.clone()),
            _ => None,
        }
    })
}

fn apply_moves(journal: &Path, mut catalog: DayCatalog) -> DayCatalog {
    let moves = archive_moves(journal);
    if moves.is_empty() {
        return catalog;
    }
    for segment in &mut catalog.segments {
        let witnesses = segment
            .history
            .iter_mut()
            .flat_map(|manifest| manifest.files.iter_mut())
            .chain(segment.active.iter_mut());
        for witness in witnesses {
            if let Some(current) = moved(&moves, &witness.snapshot_id) {
                witness.snapshot_id = current;
            }
        }
        // A legacy restore binding's revision covers its snapshot id, so it is
        // never moved; the erase leaves the archives such bindings name alone.
    }
    catalog
}

fn path(journal: &Path, day: &str) -> PathBuf {
    journal.join("health/offload").join(format!("{day}.jsonl"))
}
pub fn ledger_path_for_day(journal: &Path, day: &str) -> Result<PathBuf, String> {
    if !valid_day(day) {
        return Err("day must be in YYYYMMDD format".into());
    }
    Ok(path(journal, day))
}
pub fn append_offload_event(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    snapshot_id: &str,
    files: &[OffloadFile],
    time: u64,
) -> Result<(), String> {
    if !valid_day(day)
        || !component(stream)
        || !component(segment)
        || snapshot_id.is_empty()
        || files.is_empty()
        || files.iter().any(|file| !valid_file(file))
        || files.iter().map(|f| &f.name).collect::<BTreeSet<_>>().len() != files.len()
    {
        return Err("invalid offload ledger event".into());
    }
    let record = serde_json::json!({"event_kind":"offload","time":time,"day":day,"stream":stream,"segment":segment,"snapshot_id":snapshot_id,"files":files});
    append_jsonl(path(journal, day), &record).map_err(|error| error.to_string())
}
pub fn append_restore_event(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    time: u64,
) -> Result<(), String> {
    if !valid_day(day) || !component(stream) || !component(segment) {
        return Err("invalid restore ledger event".into());
    }
    append_jsonl(path(journal,day),&serde_json::json!({"event_kind":"restore","time":time,"day":day,"stream":stream,"segment":segment})).map_err(|error|error.to_string())
}

// A missing optional store is empty; a non-directory, symlink or IO failure is unknown.
fn ledger_directory(journal: &Path) -> Result<bool, PathBuf> {
    for dir in [
        journal.to_path_buf(),
        journal.join("health"),
        journal.join("health/offload"),
    ] {
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && dir != journal => {
                return Ok(false);
            }
            _ => return Err(dir),
        }
    }
    Ok(true)
}
pub fn catalog_day(journal: &Path, day: &str) -> Result<DayCatalog, String> {
    if !valid_day(day) {
        return Err("day must be in YYYYMMDD format".into());
    }
    let mut result = DayCatalog {
        day: day.into(),
        ..Default::default()
    };
    match ledger_directory(journal) {
        Ok(false) => return Ok(result),
        Err(dir) => {
            result.unreadable_ledgers.push(dir.display().to_string());
            return Ok(result);
        }
        Ok(true) => {}
    }
    let file = path(journal, day);
    match fs::symlink_metadata(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(result),
        Ok(meta) if meta.is_file() => {}
        _ => {
            result.unreadable_ledgers.push(file.display().to_string());
            return Ok(result);
        }
    }
    match fs::read(&file) {
        Ok(bytes) => Ok(apply_moves(journal, fold_lines(day, &bytes))),
        Err(_) => {
            result.unreadable_ledgers.push(file.display().to_string());
            Ok(result)
        }
    }
}
pub fn catalog_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<SegmentCatalog, String> {
    let catalog = catalog_day(journal, day)?;
    Ok(catalog
        .segments
        .into_iter()
        .find(|s| s.stream == stream && s.segment == segment)
        .unwrap_or(SegmentCatalog {
            day: day.into(),
            stream: stream.into(),
            segment: segment.into(),
            skipped_records: catalog.skipped_records,
            unreadable_ledgers: catalog.unreadable_ledgers,
            ..Default::default()
        }))
}
pub fn catalog_journal(journal: &Path) -> JournalCatalog {
    let mut result = JournalCatalog::default();
    match ledger_directory(journal) {
        Ok(false) => return result,
        Err(dir) => {
            result.unreadable_ledgers.push(dir.display().to_string());
            return result;
        }
        Ok(true) => {}
    }
    let directory = journal.join("health/offload");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(_) => {
            result
                .unreadable_ledgers
                .push(directory.display().to_string());
            return result;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                result
                    .unreadable_ledgers
                    .push(directory.display().to_string());
                continue;
            }
        };
        let file = entry.path();
        if file.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let Some(day) = file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|day| valid_day(day))
        else {
            result.unreadable_ledgers.push(file.display().to_string());
            continue;
        };
        match catalog_day(journal, day) {
            Ok(day) => {
                result.skipped_records += day.skipped_records;
                result
                    .unreadable_ledgers
                    .extend(day.unreadable_ledgers.clone());
                result.days.push(day);
            }
            Err(_) => result.unreadable_ledgers.push(file.display().to_string()),
        }
    }
    result.days.sort_by(|a, b| a.day.cmp(&b.day));
    result
}

fn summary(catalog: SegmentCatalog) -> SegmentOffloadSummary {
    let manifests = catalog
        .active
        .iter()
        .map(|w| &w.manifest_id)
        .collect::<BTreeSet<_>>();
    // The old consumer understands neither per-file snapshots nor repository bindings.
    let representable =
        manifests.len() <= 1 && catalog.active.iter().all(|w| w.repository_id.is_none());
    let selectable = representable && !catalog.degraded();
    SegmentOffloadSummary {
        day: catalog.day,
        stream: catalog.stream,
        segment: catalog.segment,
        currently_offloaded: !catalog.active.is_empty(),
        snapshot_id: selectable
            .then(|| catalog.active.first().map(|w| w.snapshot_id.clone()))
            .flatten(),
        files: if selectable {
            catalog.active.iter().map(|w| w.file.clone()).collect()
        } else {
            vec![]
        },
        offloaded_bytes: catalog
            .active
            .iter()
            .fold(0_u64, |sum, w| sum.saturating_add(w.file.bytes)),
        offloaded_file_count: catalog.active.len() as u64,
        skipped_records: catalog.skipped_records + u64::from(!representable),
        unreadable_ledgers: catalog.unreadable_ledgers,
    }
}
pub fn summarize_segment(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
) -> Result<SegmentOffloadSummary, String> {
    catalog_segment(journal, day, stream, segment).map(summary)
}
fn day_summary(catalog: DayCatalog) -> DayOffloadSummary {
    let segments = catalog
        .segments
        .into_iter()
        .filter(|s| !s.active.is_empty())
        .map(summary)
        .collect::<Vec<_>>();
    DayOffloadSummary {
        day: catalog.day,
        offloaded_bytes: segments
            .iter()
            .fold(0_u64, |n, s| n.saturating_add(s.offloaded_bytes)),
        offloaded_file_count: segments.iter().map(|s| s.offloaded_file_count).sum(),
        offloaded_segments: segments.len() as u64,
        skipped_records: catalog.skipped_records
            + segments
                .iter()
                .filter(|s| s.skipped_records > catalog.skipped_records)
                .count() as u64,
        unreadable_ledgers: catalog.unreadable_ledgers,
        segments,
    }
}
pub fn summarize_day(journal: &Path, day: &str) -> Result<DayOffloadSummary, String> {
    catalog_day(journal, day).map(day_summary)
}
pub fn summarize_journal(journal: &Path) -> JournalOffloadSummary {
    let catalog = catalog_journal(journal);
    let days = catalog
        .days
        .into_iter()
        .map(day_summary)
        .collect::<Vec<_>>();
    JournalOffloadSummary {
        offloaded_bytes: days
            .iter()
            .fold(0_u64, |n, d| n.saturating_add(d.offloaded_bytes)),
        offloaded_file_count: days.iter().map(|d| d.offloaded_file_count).sum(),
        offloaded_segments: days.iter().map(|d| d.offloaded_segments).sum(),
        offloaded_days: days.iter().filter(|d| d.offloaded_segments > 0).count() as u64,
        skipped_records: days.iter().map(|d| d.skipped_records).sum(),
        unreadable_ledgers: catalog.unreadable_ledgers,
        days,
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use serde_json::{Value, json};
    use solstone_core_retention::archive_catalog::{
        ArchiveWitness, RestoreDisposition, RestoreFileV2, RestoreOperation,
    };

    fn file(name: &str) -> OffloadFile {
        OffloadFile {
            name: name.into(),
            bytes: 7,
            sha256: "a".repeat(64),
        }
    }
    pub(super) fn archive(snapshot: &str, names: &[&str]) -> Value {
        json!({"event_kind":"offload", "time":1, "day":"20260101", "stream":"_default", "segment":"010000_001",
            "snapshot_id":snapshot, "files":names.iter().map(|name| file(name)).collect::<Vec<_>>()})
    }
    fn restore() -> Value {
        json!({"event_kind":"restore", "time":0, "day":"20260101", "stream":"_default", "segment":"010000_001"})
    }
    pub(super) fn archive_v2(id: &str, name: &str) -> Value {
        json!({"event_kind":"offload_archive_v2", "schema_version":2, "time":2, "day":"20260101", "stream":"_default", "segment":"010000_001",
            "segment_dir":"010000_001_full", "layout":"direct", "journal_id":"native-root-v1:unix:0000000000000001:0000000000000002", "repository_id":"c".repeat(64),
            "snapshot_id":"b".repeat(64), "manifest_id":id,
            "files":[{"witness_id":format!("w-{id}"), "name":name, "bytes":7, "sha256":"a".repeat(64), "archive_path":format!("/stage/{name}"),
                "processing":{"record":null,"has_analysis_row":true}}]})
    }
    fn folded(records: &[Value]) -> DayCatalog {
        let lines = records.iter().map(|r| format!("{r}\n")).collect::<String>();
        fold_lines("20260101", lines.as_bytes())
    }
    #[test]
    fn recovery_witness_values_roundtrip_losslessly_and_refuse_unknown_fields() {
        for record in [
            archive("a", &["raw.flac"]),
            archive_v2("manifest", "raw.flac"),
        ] {
            let day = folded(&[record]);
            let witness = &day.segments[0].active[0];
            let mut value = serde_json::to_value(witness).unwrap();
            assert_eq!(
                serde_json::from_value::<ArchiveWitness>(value.clone()).unwrap(),
                *witness
            );
            value["unknown_binding"] = json!(true);
            assert!(serde_json::from_value::<ArchiveWitness>(value).is_err());
        }
        let operation = RestoreOperation {
            operation_id: "operation".into(),
            files: vec![RestoreFileV2 {
                witness_id: "w-manifest".into(),
                name: "raw.flac".into(),
                disposition: RestoreDisposition::Published,
            }],
        };
        let mut value = serde_json::to_value(&operation).unwrap();
        assert_eq!(
            serde_json::from_value::<RestoreOperation>(value.clone()).unwrap(),
            operation
        );
        value["files"][0]["unknown_effect"] = json!(true);
        assert!(serde_json::from_value::<RestoreOperation>(value).is_err());
    }
    fn processing_record(state: &str, bytes: u64) -> Value {
        json!({"schema":"solstone.processing.v1", "handler":"transcribe", "state":state,
            "input_size":bytes, "reason_code":if state == "analyzed" { "ok" } else { "no_decodable_audio" },
            "attempted_at":"2026-01-01T00:00:00Z", "source":"backfill",
            "extra":{"preserved":[null,true,"detail"]}})
    }
    #[test]
    fn accepted_processing_observations_remain_lossless_historical_evidence() {
        for (record, has_analysis_row) in [
            (Value::Null, true),
            (processing_record("empty", 7), false),
            (processing_record("analyzed", 7), true),
        ] {
            let mut event = archive_v2("accepted", "audio.flac");
            let processing = json!({"record":record,"has_analysis_row":has_analysis_row});
            event["files"][0]["processing"] = processing.clone();
            let day = folded(&[event]);
            assert!(!day.degraded());
            let segment = &day.segments[0];
            let witness = &segment.active[0];
            assert_eq!(segment.history[0].files[0], *witness);
            assert_eq!(
                serde_json::to_value(&witness.processing).unwrap(),
                processing
            );
            assert_eq!(
                serde_json::from_value::<ArchiveWitness>(serde_json::to_value(witness).unwrap())
                    .unwrap(),
                *witness
            );
        }
        let legacy = folded(&[archive("legacy", &["audio.flac"])]);
        assert!(legacy.segments[0].active[0].processing.is_none());
    }
    #[test]
    fn invalid_processing_or_binding_retains_preceding_generation() {
        let preceding = archive_v2("previous", "audio.flac");
        let valid = archive_v2("later", "audio.flac");
        let mut cases = Vec::new();
        let mut missing = valid.clone();
        missing["files"][0]
            .as_object_mut()
            .unwrap()
            .remove("processing");
        cases.push(missing);
        for observation in [
            json!({"record":null,"has_analysis_row":true,"unknown":true}),
            json!({"record":null,"has_analysis_row":"true"}),
            json!({"record":null,"has_analysis_row":false}),
            json!({"record":processing_record("analyzed",7),"has_analysis_row":false}),
            json!({"record":processing_record("empty",8),"has_analysis_row":true}),
        ] {
            let mut event = valid.clone();
            event["files"][0]["processing"] = observation;
            cases.push(event);
        }
        for repository in ["repo", "A".repeat(64).as_str()] {
            let mut event = valid.clone();
            event["repository_id"] = json!(repository);
            cases.push(event);
        }
        for path in [
            "/stage/other.flac",
            "/stage//audio.flac",
            "/stage/../audio.flac",
        ] {
            let mut event = valid.clone();
            event["files"][0]["archive_path"] = json!(path);
            cases.push(event);
        }
        let mut overflow = valid.clone();
        overflow["files"][0]["bytes"] = json!(u64::MAX);
        let mut second = overflow["files"][0].clone();
        second["name"] = json!("z.flac");
        second["witness_id"] = json!("second");
        second["archive_path"] = json!("/stage/z.flac");
        overflow["files"].as_array_mut().unwrap().push(second);
        cases.push(overflow);
        for invalid in cases {
            let day = folded(&[preceding.clone(), invalid]);
            assert_eq!(day.skipped_records, 1);
            assert!(day.degraded());
            assert_eq!(day.segments[0].history.len(), 1);
            assert_eq!(day.segments[0].active[0].witness_id, "w-previous");
        }
    }
    #[test]
    fn archive_names_remain_representable_by_exact_restore_events() {
        assert!(folded(&[archive_v2("colon", "audio:part.flac")]).degraded());
        for disposition in ["published", "already_present"] {
            let restore = json!({"event_kind":"offload_restore_v2", "schema_version":2,
                "time":3, "day":"20260101", "stream":"_default", "segment":"010000_001",
                "operation_id":"restore", "files":[{"witness_id":"w-valid",
                    "name":"audio.flac", "disposition":disposition}]});
            let day = folded(&[archive_v2("valid", "audio.flac"), restore]);
            assert!(!day.degraded());
            assert!(day.segments[0].active.is_empty());
            assert_eq!(day.segments[0].history.len(), 1);
        }
    }
    #[test]
    fn disjoint_generations_survive_legacy_restore_of_last_manifest() {
        let day = folded(&[
            archive("a", &["a.webm"]),
            archive("b", &["b.webm"]),
            restore(),
            restore(),
        ]);
        assert!(!day.degraded());
        assert_eq!(day.segments[0].history.len(), 2);
        assert_eq!(
            day.segments[0]
                .active
                .iter()
                .map(|w| w.file.name.as_str())
                .collect::<Vec<_>>(),
            ["a.webm"]
        );
        assert_eq!(day.segments[0].active[0].snapshot_id, "a");
    }
    #[test]
    fn superseded_generation_never_returns_after_restoring_newest() {
        let day = folded(&[
            archive("a", &["a.webm"]),
            archive("b", &["a.webm"]),
            restore(),
        ]);
        assert!(day.segments[0].active.is_empty());
        assert_eq!(day.segments[0].history.len(), 2);
    }
    #[test]
    fn multi_manifest_old_summary_refuses_selection_and_preserves_totals() {
        let day = folded(&[archive("a", &["a.webm"]), archive("b", &["b.webm"])]);
        let old_summary = summary(day.segments[0].clone());
        assert!(old_summary.degraded());
        assert!(old_summary.snapshot_id.is_none());
        assert!(old_summary.files.is_empty());
        assert_eq!(old_summary.offloaded_file_count, 2);
        assert_eq!(old_summary.offloaded_bytes, 14);
        let single = summary_for_test(&[archive("a", &["a.webm", "b.webm"])]);
        assert!(!single.degraded());
        assert_eq!(single.files.len(), 2);
        assert_eq!(single.snapshot_id.as_deref(), Some("a"));
    }
    fn summary_for_test(records: &[Value]) -> SegmentOffloadSummary {
        summary(folded(records).segments.remove(0))
    }
    #[test]
    fn v2_exact_dispositions_do_not_retire_newer_or_unpublished_witnesses() {
        for disposition in [
            "published",
            "already_present",
            "conflict",
            "unverified",
            "unattempted",
        ] {
            let event = json!({"event_kind":"offload_restore_v2", "schema_version":2, "time":3, "day":"20260101", "stream":"_default", "segment":"010000_001",
                "operation_id":"operation", "files":[{"witness_id":"w-a", "name":"a.webm", "disposition":disposition}]});
            let old = folded(&[archive_v2("a", "a.webm"), event.clone()]);
            assert!(!old.degraded());
            assert_eq!(
                old.segments[0].active.is_empty(),
                matches!(disposition, "published" | "already_present")
            );
            let newer = folded(&[archive_v2("a", "a.webm"), archive_v2("b", "a.webm"), event]);
            assert!(!newer.degraded());
            assert_eq!(newer.segments[0].active[0].witness_id, "w-b");
        }
    }
    #[test]
    fn malformed_unknown_duplicate_and_torn_evidence_degrades_read_only_fold() {
        let valid = archive("a", &["a.webm"]);
        let cases = [
            json!({"event_kind":"future"}),
            archive("a", &["a.webm", "a.webm"]),
            json!({"event_kind":"restore", "time":"bad", "day":"20260101", "stream":"_default", "segment":"010000_001"}),
        ];
        for bad in cases {
            let day = folded(&[valid.clone(), bad]);
            assert_eq!(day.skipped_records, 1);
            assert!(day.segments[0].degraded());
        }
        for tail in [format!("{valid}"), "{torn".into()] {
            let day = fold_lines("20260101", format!("{valid}\n{tail}").as_bytes());
            assert_eq!(day.skipped_records, 1);
        }
        assert!(folded(&[valid.clone(), valid]).degraded());
        let duplicate = folded(&[archive_v2("a", "a.webm"), archive_v2("a", "b.webm")]);
        assert_eq!(duplicate.skipped_records, 1);
    }
    #[test]
    fn v2_bindings_are_required_and_legacy_bindings_remain_unknown() {
        let valid = archive_v2("a", "a.webm");
        let day = folded(std::slice::from_ref(&valid));
        assert!(!day.degraded());
        assert_eq!(
            day.segments[0].active[0].segment_dir.as_deref(),
            Some("010000_001_full")
        );
        for key in ["journal_id", "layout", "segment_dir", "repository_id"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(folded(&[missing]).degraded());
            let mut malformed = valid.clone();
            malformed[key] = json!(false);
            assert!(folded(&[malformed]).degraded());
        }
        let legacy = folded(&[archive("a", &["a.webm"])]);
        let witness = &legacy.segments[0].active[0];
        assert!(witness.repository_id.is_none());
        assert!(witness.layout.is_none());
        assert!(witness.segment_dir.is_none());
        assert_eq!(witness.witness_id, "v1:20260101:1:a.webm");
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn damaged_ledger_bytes_are_preserved_and_directory_failures_are_unknown() {
        let journal = tempfile::tempdir().unwrap();
        assert!(!catalog_journal(journal.path()).degraded());
        fs::write(journal.path().join("health"), b"blocked").unwrap();
        assert!(catalog_journal(journal.path()).degraded());
        fs::remove_file(journal.path().join("health")).unwrap();
        let ledger = path(journal.path(), "20260101");
        fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        fs::write(&ledger, b"{broken}\n{torn").unwrap();
        assert_eq!(
            catalog_day(journal.path(), "20260101")
                .unwrap()
                .skipped_records,
            2
        );
        assert_eq!(fs::read(&ledger).unwrap(), b"{broken}\n{torn");
        assert_eq!(fs::read_dir(ledger.parent().unwrap()).unwrap().count(), 1);
        fs::remove_file(&ledger).unwrap();
        fs::create_dir(&ledger).unwrap();
        assert!(catalog_journal(journal.path()).degraded());
    }

    #[test]
    fn append_order_beats_timestamp_and_read_is_read_only() {
        let journal = tempfile::tempdir().unwrap();
        let file = OffloadFile {
            name: "raw.webm".into(),
            bytes: 3,
            sha256: "a".repeat(64),
        };
        append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000",
            "snapshot",
            &[file],
            20,
        )
        .unwrap();
        append_restore_event(journal.path(), "20260101", "_default", "010000", 10).unwrap();
        let before = fs::read(path(journal.path(), "20260101")).unwrap();
        assert!(
            !summarize_segment(journal.path(), "20260101", "_default", "010000")
                .unwrap()
                .currently_offloaded
        );
        assert_eq!(before, fs::read(path(journal.path(), "20260101")).unwrap());
    }

    #[test]
    fn duplicate_and_reordered_events_fold_in_append_order() {
        let journal = tempfile::tempdir().unwrap();
        let file = OffloadFile {
            name: "raw.webm".into(),
            bytes: 7,
            sha256: "b".repeat(64),
        };
        append_restore_event(journal.path(), "20260102", "_default", "020000_001", 99).unwrap();
        append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "first",
            std::slice::from_ref(&file),
            1,
        )
        .unwrap();
        append_offload_event(
            journal.path(),
            "20260102",
            "_default",
            "020000_001",
            "last",
            &[file],
            2,
        )
        .unwrap();

        let summary =
            summarize_segment(journal.path(), "20260102", "_default", "020000_001").unwrap();
        assert!(summary.currently_offloaded);
        assert_eq!(summary.snapshot_id.as_deref(), Some("last"));
    }

    #[test]
    fn malformed_and_wrong_identity_records_degrade_without_summary_writes() {
        let journal = tempfile::tempdir().unwrap();
        let ledger = path(journal.path(), "20260103");
        fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        fs::write(
            &ledger,
            concat!(
                "{\"event_kind\":\"offload\",\"time\":1,\"day\":\"20260103\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[{\"name\":\"raw.webm\",\"bytes\":3,\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}]}\n",
                "{\"event_kind\":\"offload\",\"time\":2,\"day\":\"wrong\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[]}\n",
                "{\"event_kind\":\"offload\",\"time\":3,\"day\":\"20260103\",\"stream\":\"_default\",\"segment\":\"030000_001\",\"snapshot_id\":\"snapshot\",\"files\":[{\"name\":\"../wrong.webm\",\"bytes\":3,\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}]}\n",
                "{\"event_kind\":\"offload\"\n"
            ),
        )
        .unwrap();

        let summary =
            summarize_segment(journal.path(), "20260103", "_default", "030000_001").unwrap();
        assert!(summary.currently_offloaded);
        assert_eq!(summary.skipped_records, 3);
        assert!(summary.degraded());
        assert!(summary.files.is_empty());
    }
}
