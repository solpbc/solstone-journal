// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Removing erased things from every backup: the runner.
//!
//! The trash leaves one entry for each thing an owner chose to erase from
//! their backups too ([`solstone_core_trash::backup_erase`]). This drains them
//! into one run of the journal's restic helper, which rewrites every backup
//! holding the thing (where it was, its place in the trash, earlier trash
//! copies, and the media archives the offload ledger names) without it, then
//! removes the stored pieces only it used and scans every stored pack.
//!
//! The local erase never waits for this. One runner at a time; an entry whose
//! backup destination isn't the one set up now is retired unreached, with no
//! claim. Each outcome is written to the removal history, and the page reads
//! [`status`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use solstone_core_backup_runtime::engine::BackupServices;
use solstone_core_backup_runtime::erase_paths;
use solstone_core_backup_runtime::teardown::owned::{StorageKeeps, StorageRetention};
use solstone_core_removals::{Action, Event, OperationId, Page, Removal, Surface};
use solstone_core_trash::backup_erase::{self, Entry, MARKER_KEY, MARKER_NAME};

/// The runner's own files, beside the entries.
const STATUS_FILE: &str = "status.json";
const RECONCILED_FILE: &str = "reconciled";
const RESULTS_FILE: &str = "results.jsonl";
const OFFLOAD_TAG: &str = solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG;
const PIECES_FILE: &str = "pieces.json";
const REQUEST_FILE: &str = "request.json";
/// How long a result stays on the page once nothing is pending.
const KEEP_RESULTS_DAYS: i64 = 7;

/// Where the runner stands, for the trash page and the backup page.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    /// `idle`, `running`, `waiting`, `stopped`, `failed` or `stays`.
    pub state: String,
    /// Why, in a stable word: `paused`, `offline`, `busy`, `unavailable`,
    /// `refused`, `residue`, `unknown`, `versions`, `locks`, `hidden_files`,
    /// `legacy_archive`, `ledger`, `missed`, `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Erased things still to leave the backups.
    pub pending: usize,
    /// Erased things whose copies stay in storage (`stays`).
    #[serde(default)]
    pub staying: usize,
    pub updated: String,
    /// Runs that failed in a row, and when the next may try.
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_try: Option<String>,
}

fn dir(journal: &Path) -> PathBuf {
    journal.join(backup_erase::DIR)
}

/// The runner's last word, or idle.
pub fn status(journal: &Path) -> Status {
    let pending = backup_erase::pending(journal).0.len();
    let mut status: Status = std::fs::read(dir(journal).join(STATUS_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    status.pending = pending;
    if status.state.is_empty() || (pending == 0 && status.state != "stays") {
        status.state = "idle".into();
        status.reason = None;
    }
    status
}

fn write_status_full(
    journal: &Path,
    state: &str,
    reason: Option<&str>,
    pending: usize,
    staying: usize,
    attempts: u32,
    next_try: Option<chrono::DateTime<Utc>>,
) {
    let status = Status {
        state: state.into(),
        reason: reason.map(str::to_owned),
        pending,
        staying,
        updated: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        attempts,
        next_try: next_try.map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true)),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&status) {
        let _ = solstone_core_journal_io::atomic::atomic_replace(
            dir(journal).join(STATUS_FILE),
            &bytes,
            solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
        );
    }
}

/// What one run did.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Report {
    /// `nothing`, `done`, or the state written to [`status`].
    pub state: String,
    pub reason: Option<String>,
    pub removed: usize,
    pub shared: usize,
    pub staying: usize,
    pub unreached: usize,
    pub pending: usize,
}

/// A segment's place, when an entry's path is one.
fn segment_of(path: &str) -> Option<(&str, &str, &str)> {
    let mut parts = path.split('/');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("chronicle"), Some(day), Some(stream), Some(segment), None) => {
            Some((day, stream, segment))
        }
        _ => None,
    }
}

/// The exact archive paths the ledger names for an erased segment, in every
/// generation it was offloaded in, each keyed by the archive id the ledger
/// recorded: the id a rewritten archive keeps as its `original`. The flag says
/// a witness from before archive paths were recorded can't be placed: its
/// archive is left alone, and the entry says so. `Err` when the ledger can't
/// be read in full.
fn archive_paths(
    journal: &Path,
    entry: &Entry,
) -> Result<(BTreeMap<String, Vec<String>>, bool), &'static str> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut legacy = false;
    for path in &entry.paths {
        let Some((day, stream, segment)) = segment_of(path) else {
            continue;
        };
        let catalog = crate::ledger::catalog_segment_as_recorded(journal, day, stream, segment)
            .map_err(|_| "ledger")?;
        if catalog.skipped_records > 0 || !catalog.unreadable_ledgers.is_empty() {
            return Err("ledger");
        }
        let witnesses = catalog
            .history
            .iter()
            .flat_map(|manifest| manifest.files.iter())
            .chain(catalog.active.iter());
        for witness in witnesses {
            let Some(archive_path) = witness.archive_path.clone() else {
                legacy = true;
                continue;
            };
            let paths = out.entry(witness.snapshot_id.clone()).or_default();
            if !paths.contains(&archive_path) {
                paths.push(archive_path);
            }
        }
    }
    Ok((out, legacy))
}

/// Read back, from the destination, which media archives a backup erase
/// rewrote, and record any move the ledger is missing: after a whole-journal
/// restore its moves are only as new as the backup restored. Once a day, while
/// the journal has offloaded media and the helper to rewrite archives with.
fn reconcile(journal: &Path, services: &BackupServices<'_>) {
    let marker = dir(journal).join(RECONCILED_FILE);
    let fresh = std::fs::metadata(&marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < std::time::Duration::from_secs(24 * 60 * 60));
    if fresh || erase_paths::helper(services).is_none() {
        return;
    }
    let has_ledger = std::fs::read_dir(journal.join("health/offload")).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
    });
    if !has_ledger {
        return;
    }
    let moves = crate::ledger::archive_moves(journal);
    let Ok(Some(destination)) = erase_paths::open(journal, services) else {
        return;
    };
    let Ok(listing) = destination.snapshots_tagged(services, OFFLOAD_TAG) else {
        return;
    };
    let found: Vec<(String, String)> = listing
        .iter()
        .filter_map(|row| {
            Some((
                row.get("original")?.as_str()?.to_owned(),
                row.get("id")?.as_str()?.to_owned(),
            ))
        })
        .filter(|(original, current)| moves.get(original) != Some(current))
        .collect();
    if crate::ledger::record_archive_moves(journal, &found).is_ok() {
        let _ = std::fs::create_dir_all(dir(journal));
        let _ = std::fs::write(&marker, b"");
    }
}

/// When a run that failed may try again: an hour, doubling, at most a day.
fn next_try(attempts: u32) -> chrono::DateTime<Utc> {
    let hours = 1_i64 << attempts.min(5).saturating_sub(1);
    Utc::now() + chrono::Duration::hours(hours.min(24))
}

/// One line in the engine's own record of each outcome, with its counts.
fn record_result(journal: &Path, line: Value) {
    let _ = solstone_core_journal_io::append_jsonl(dir(journal).join(RESULTS_FILE), &line);
}

/// Run once: drain every pending entry the backup can take now.
pub fn run(journal: &Path, services: &BackupServices<'_>) -> Report {
    reconcile(journal, services);
    let (entries, _unreadable) = backup_erase::pending(journal);
    if entries.is_empty() {
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
        return Report {
            state: "nothing".into(),
            ..Report::default()
        };
    }
    // One runner at a time; a second finds it busy at once and leaves it.
    let Ok(_lock) = solstone_core_journal_io::hold_lock(
        dir(journal),
        solstone_core_journal_io::LockOptions {
            timeout: std::time::Duration::from_secs(1),
            ..solstone_core_journal_io::LockOptions::default()
        },
    ) else {
        return Report {
            state: "running".into(),
            pending: entries.len(),
            ..Report::default()
        };
    };
    let mut report = Report::default();
    let previous = status(journal);
    let finish = |state: &str, reason: Option<&str>, report: &mut Report, retry: bool| {
        let attempts = if retry {
            previous.attempts.saturating_add(1)
        } else {
            0
        };
        write_status_full(
            journal,
            state,
            reason,
            report.pending,
            report.staying,
            attempts,
            retry.then(|| next_try(attempts)),
        );
        report.state = state.into();
        report.reason = reason.map(str::to_owned);
    };
    // A run that failed waits its turn, unless the owner erased more since.
    let newest = entries
        .iter()
        .map(|entry| entry.erased_at.as_str())
        .max()
        .unwrap_or_default();
    if let Some(at) = previous.next_try.as_deref()
        && at
            > Utc::now()
                .to_rfc3339_opts(SecondsFormat::Millis, true)
                .as_str()
        && newest <= previous.updated.as_str()
    {
        report.pending = entries.len();
        report.state = previous.state.clone();
        report.reason = previous.reason.clone();
        return report;
    }

    // The backup set up now. A configuration that can't be read decides
    // nothing: everything waits.
    if solstone_core_backup::get_backup_config(journal).is_err() {
        report.pending = entries.len();
        finish("failed", Some("error"), &mut report, true);
        return report;
    }
    // An entry for a destination that isn't the one set up now can't be
    // reached: it is retired with no claim, and the engine's record says why.
    let current = solstone_core_backup::destination_identity(journal);
    let mut live = Vec::new();
    for entry in entries {
        if current.is_none() || entry.destination != current {
            record_result(
                journal,
                json!({"id": entry.id, "at": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true), "outcome": "not_reached"}),
            );
            let _ = backup_erase::retire(journal, &entry.id);
            report.unreached += 1;
        } else {
            live.push(entry);
        }
    }
    report.pending = live.len();
    if live.is_empty() {
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
        finish("idle", None, &mut report, false);
        return report;
    }
    if solstone_core_backup_runtime::backup_paused(journal) {
        finish("waiting", Some("paused"), &mut report, false);
        return report;
    }
    let Some(helper) = erase_paths::helper(services) else {
        finish("waiting", Some("unavailable"), &mut report, false);
        return report;
    };
    let destination = match erase_paths::open(journal, services) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            finish("waiting", Some("unavailable"), &mut report, false);
            return report;
        }
        Err(_) => {
            finish("waiting", Some("offline"), &mut report, false);
            return report;
        }
    };
    // Every item still in the trash, whatever it's doing, keeps its copies.
    let Ok(in_trash) = solstone_core_trash::ids(journal) else {
        finish("failed", Some("error"), &mut report, true);
        return report;
    };
    let Some(root) = erase_paths::journal_root(journal) else {
        finish("failed", Some("roots"), &mut report, true);
        return report;
    };

    // One request for every entry.
    let mut targets = Vec::new();
    let mut snapshot_paths: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut legacy = BTreeSet::new();
    for entry in &live {
        if entry
            .paths
            .iter()
            .chain([&entry.trash_path])
            .any(|path| path.contains('\\'))
        {
            finish("failed", Some("error"), &mut report, true);
            return report;
        }
        let (archives, legacy_archive) = match archive_paths(journal, entry) {
            Ok(found) => found,
            Err(reason) => {
                finish("failed", Some(reason), &mut report, true);
                return report;
            }
        };
        if legacy_archive {
            legacy.insert(entry.id.clone());
        }
        for path in &entry.paths {
            targets.push(json!({
                "entry": entry.id, "path": path, "settled": entry.settled(), "trash_copies": true,
                "marker": {"name": MARKER_NAME, "key": MARKER_KEY, "value": entry.id},
            }));
        }
        targets.push(json!({"entry": entry.id, "path": entry.trash_path}));
        for (snapshot, paths) in archives {
            snapshot_paths.entry(snapshot).or_default().extend(
                paths
                    .into_iter()
                    .map(|path| json!({"entry": entry.id, "path": path})),
            );
        }
    }
    let ours: BTreeSet<&str> = live.iter().map(|entry| entry.id.as_str()).collect();
    let live_trash: Vec<String> = in_trash
        .into_iter()
        .filter(|id| !ours.contains(id.as_str()))
        .collect();
    let request = json!({
        "roots": [root],
        "snapshot_roots": true,
        "targets": targets,
        "trash": {"dir": solstone_core_trash::TRASH_DIR, "files": "files", "live": live_trash,
                  "item": "item.json", "paths_key": "paths"},
        "snapshot_paths": snapshot_paths,
        "drop_empty_tags": [OFFLOAD_TAG],
    });
    let request_path = dir(journal).join(REQUEST_FILE);
    if std::fs::write(&request_path, request.to_string()).is_err() {
        finish("failed", Some("error"), &mut report, true);
        return report;
    }
    finish("running", None, &mut report, false);
    let erased = destination.erase(
        services,
        &helper,
        &request_path,
        &dir(journal).join(PIECES_FILE),
    );
    let _ = std::fs::remove_file(&request_path);
    let erased = match erased {
        Ok(erased) => erased,
        Err(_) => {
            finish("waiting", Some("offline"), &mut report, false);
            return report;
        }
    };
    let result = &erased.result;
    if erased.returncode != 0 || result.get("ok") != Some(&Value::Bool(true)) {
        let (state, reason, retry) = match result.get("reason").and_then(Value::as_str) {
            Some("lock") => ("waiting", "busy", false),
            Some("stopped") => ("stopped", "stopped", false),
            Some("delete_refused") => ("stays", "refused", true),
            Some("residue") => ("stays", "residue", true),
            Some("missed") => ("failed", "missed", true),
            Some("roots") => ("failed", "roots", true),
            _ => ("failed", "error", true),
        };
        if state == "stays" {
            report.staying = live.len();
        }
        finish(state, Some(reason), &mut report, retry);
        return report;
    }

    // Media archives keep their place in the ledger by their first id.
    let moves: Vec<(String, String)> = result
        .get("snapshots_rewritten")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|rewritten| {
            rewritten
                .get("tags")
                .and_then(Value::as_array)
                .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some(OFFLOAD_TAG)))
        })
        .filter_map(|rewritten| {
            Some((
                rewritten.get("original")?.as_str()?.to_owned(),
                rewritten.get("to")?.as_str()?.to_owned(),
            ))
        })
        .collect();
    // Not recorded here, the daily reconcile reads them back from the
    // destination.
    let _ = crate::ledger::record_archive_moves(journal, &moves);

    // Every listed piece no backup uses is gone. What the storage keeps of a
    // deleted file is the storage's own answer.
    let storage = destination.storage(services);
    let keeps = match storage {
        StorageRetention::Removes => None,
        StorageRetention::Keeps(StorageKeeps::Versions) => Some("versions"),
        StorageRetention::Keeps(StorageKeeps::ObjectLock | StorageKeeps::FileLock) => Some("locks"),
        StorageRetention::Keeps(StorageKeeps::HiddenFiles) => Some("hidden_files"),
        StorageRetention::Unknown => Some("unknown"),
    };
    let entries_report = result.get("entries").cloned().unwrap_or(Value::Null);
    let count = |entry: &Entry, key: &str| {
        entries_report
            .get(&entry.id)
            .and_then(|r| r.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    for entry in &live {
        let shared = count(entry, "in_use_kept") + count(entry, "in_use_trash");
        let left_alone = legacy.contains(&entry.id);
        let action = match (keeps, left_alone, shared) {
            (Some(_), _, _) | (None, true, _) => Action::BackupEraseStays,
            (None, false, 0) => Action::BackupErase,
            (None, false, _) => Action::BackupEraseShared,
        };
        // Durable: a run that stops before the entry is retired finds the
        // same history row again rather than writing a second.
        let at = chrono::DateTime::parse_from_rfc3339(&entry.erased_at)
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Removal::with_id(
            journal,
            OperationId::durable(at, &format!("{}backups", entry.id)),
            action,
            // The backup half has the backup's place word in the history.
            Surface::Web { page: Page::Backup },
        )
        .for_item(&entry.id)
        .record_event(Event::Removed, entry.unit.clone());
        record_result(
            journal,
            json!({
                "id": entry.id, "at": now, "outcome": format!("{action:?}"),
                "snapshots": count(entry, "snapshots"), "files": count(entry, "files"),
                "in_use_kept": count(entry, "in_use_kept"), "in_use_trash": count(entry, "in_use_trash"),
                "storage": keeps.unwrap_or("removes"), "legacy_archive": left_alone,
            }),
        );
        match action {
            Action::BackupEraseStays => report.staying += 1,
            Action::BackupEraseShared => report.shared += 1,
            _ => report.removed += 1,
        }
        let _ = backup_erase::retire(journal, &entry.id);
    }
    report.pending = backup_erase::pending(journal).0.len();
    if report.pending == 0 {
        // Nothing left owes a backup half: the saved list has done its job.
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
    }
    match (keeps, legacy.is_empty()) {
        (Some(kind), _) => finish("stays", Some(kind), &mut report, false),
        (None, false) => finish("stays", Some("legacy_archive"), &mut report, false),
        (None, true) if report.pending > 0 => finish("waiting", None, &mut report, false),
        (None, true) => {
            finish("idle", None, &mut report, false);
            report.state = "done".into();
        }
    }
    report
}

/// How long a `stays` word stays once its copies may have gone: until the
/// next erase checks again, or a week.
pub const STAYS_DAYS: i64 = KEEP_RESULTS_DAYS;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test code")]
mod tests {
    use super::*;
    use solstone_core_retention::archive_catalog::OffloadFile;

    fn entry(paths: &[&str]) -> Entry {
        serde_json::from_value(json!({
            "schema": 1, "id": "t1", "unit": {"kind": "segment", "day": "20261101", "stream": "macbook", "segment": "090000_300"},
            "surface": "cli", "paths": paths, "trash_path": ".trash/t1",
            "trashed_at": "2026-11-15T10:00:00Z", "erased_at": "2026-11-15T11:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn only_a_segment_path_names_a_segment() {
        assert_eq!(
            segment_of("chronicle/20261101/macbook/090000_300"),
            Some(("20261101", "macbook", "090000_300"))
        );
        assert_eq!(segment_of("chronicle/20261101/macbook"), None);
        assert_eq!(segment_of("imports/20261101_090000"), None);
        assert_eq!(
            segment_of("chronicle/20261101/macbook/090000_300/audio.m4a"),
            None
        );
    }

    #[test]
    fn a_witness_without_its_archive_path_leaves_its_archive_alone() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path();
        crate::ledger::append_offload_event(
            journal,
            "20261101",
            "macbook",
            "090000_300",
            &"a".repeat(64),
            &[OffloadFile {
                name: "audio.flac".into(),
                bytes: 3,
                sha256: "b".repeat(64),
            }],
            1,
        )
        .unwrap();
        let (placed, legacy) =
            archive_paths(journal, &entry(&["chronicle/20261101/macbook/090000_300"])).unwrap();
        assert!(
            legacy && placed.is_empty(),
            "its archive is left alone, and the entry says so"
        );
        // Nothing offloaded: nothing to place.
        let (placed, legacy) =
            archive_paths(journal, &entry(&["chronicle/20261102/macbook/090000_300"])).unwrap();
        assert!(!legacy && placed.is_empty());
    }

    #[test]
    fn idle_until_something_is_pending() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(status(temp.path()).state, "idle");
        assert_eq!(status(temp.path()).pending, 0);
    }
}
