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
use solstone_core_removals::{Action, Event, Removal};
use solstone_core_trash::backup_erase::{self, Entry, MARKER_KEY, MARKER_NAME};

/// The runner's own files, beside the entries.
const STATUS_FILE: &str = "status.json";
const RECONCILED_FILE: &str = "reconciled";
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

fn write_status(journal: &Path, state: &str, reason: Option<&str>, pending: usize, staying: usize) {
    let status = Status {
        state: state.into(),
        reason: reason.map(str::to_owned),
        pending,
        staying,
        updated: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
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
/// generation it was offloaded in. `Err` when a witness can't be placed, or the
/// ledger can't be read in full: then the archives aren't safe to call cleared.
fn archive_paths(
    journal: &Path,
    entry: &Entry,
) -> Result<BTreeMap<String, Vec<String>>, &'static str> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in &entry.paths {
        let Some((day, stream, segment)) = segment_of(path) else {
            continue;
        };
        let catalog =
            crate::ledger::catalog_segment(journal, day, stream, segment).map_err(|_| "ledger")?;
        if catalog.skipped_records > 0 || !catalog.unreadable_ledgers.is_empty() {
            return Err("ledger");
        }
        let witnesses = catalog
            .history
            .iter()
            .flat_map(|manifest| manifest.files.iter())
            .chain(catalog.active.iter());
        for witness in witnesses {
            // A witness from before archive paths were recorded can't be placed
            // in its archive: that archive is left alone, and the entry says so.
            let Some(archive_path) = witness.archive_path.clone() else {
                return Err("legacy_archive");
            };
            let snapshot = witness.snapshot_id.clone();
            let paths = out.entry(snapshot).or_default();
            if !paths.contains(&archive_path) {
                paths.push(archive_path);
            }
        }
    }
    Ok(out)
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

/// Run once: drain every pending entry the backup can take now.
pub fn run(journal: &Path, services: &BackupServices<'_>) -> Report {
    reconcile(journal, services);
    let (entries, _unreadable) = backup_erase::pending(journal);
    if entries.is_empty() {
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

    // An entry for a destination that isn't the one set up now can't be
    // reached: retire it, with no claim.
    let current = solstone_core_backup::destination_identity(journal);
    let mut live = Vec::new();
    for entry in entries {
        if current.is_none() || entry.destination != current {
            let _ = backup_erase::retire(journal, &entry.id);
            report.unreached += 1;
        } else {
            live.push(entry);
        }
    }
    report.pending = live.len();
    let finish = |state: &str, reason: Option<&str>, report: &mut Report| {
        write_status(journal, state, reason, report.pending, report.staying);
        report.state = state.into();
        report.reason = reason.map(str::to_owned);
    };
    if live.is_empty() {
        finish("idle", None, &mut report);
        return report;
    }
    if solstone_core_backup_runtime::backup_paused(journal) {
        finish("waiting", Some("paused"), &mut report);
        return report;
    }
    let Some(helper) = erase_paths::helper(services) else {
        finish("waiting", Some("unavailable"), &mut report);
        return report;
    };
    let destination = match erase_paths::open(journal, services) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            finish("waiting", Some("unavailable"), &mut report);
            return report;
        }
        Err(_) => {
            finish("waiting", Some("offline"), &mut report);
            return report;
        }
    };

    // One request for every entry it can place.
    let mut targets = Vec::new();
    let mut snapshot_paths: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut runnable = Vec::new();
    let mut held_back: Option<&'static str> = None;
    for entry in &live {
        let archives = match archive_paths(journal, entry) {
            Ok(archives) => archives,
            Err(reason) => {
                held_back = Some(reason);
                continue;
            }
        };
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
        runnable.push(entry);
    }
    if runnable.is_empty() {
        finish("stays", held_back, &mut report);
        report.staying = live.len();
        return report;
    }
    let Some(root) = erase_paths::journal_root(journal) else {
        finish("failed", Some("error"), &mut report);
        return report;
    };
    let live_trash: Vec<String> = solstone_core_trash::list(journal)
        .map(|items| items.into_iter().map(|item| item.id).collect())
        .unwrap_or_default();
    let request = json!({
        "roots": [root],
        "targets": targets,
        "trash": {"dir": solstone_core_trash::TRASH_DIR, "files": "files", "live": live_trash,
                  "item": "item.json", "paths_key": "paths"},
        "snapshot_paths": snapshot_paths,
        "drop_empty_tags": [solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG],
    });
    let request_path = dir(journal).join(REQUEST_FILE);
    if std::fs::write(&request_path, request.to_string()).is_err() {
        finish("failed", Some("error"), &mut report);
        return report;
    }
    finish("running", None, &mut report);
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
            finish("waiting", Some("offline"), &mut report);
            return report;
        }
    };
    let result = &erased.result;
    if erased.returncode != 0 || result.get("ok") != Some(&Value::Bool(true)) {
        let (state, reason) = match result.get("reason").and_then(Value::as_str) {
            Some("lock") => ("waiting", "busy"),
            Some("stopped") => ("stopped", "stopped"),
            Some("delete_refused") => ("stays", "refused"),
            Some("residue") => ("stays", "residue"),
            Some("missed") => ("failed", "missed"),
            _ => ("failed", "error"),
        };
        if state == "stays" {
            report.staying = runnable.len();
        }
        finish(state, Some(reason), &mut report);
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
    let mut done = BTreeSet::new();
    for entry in &runnable {
        let reported = entries_report.get(&entry.id);
        let kept = reported
            .and_then(|r| r.get("in_use_kept"))
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + reported
                .and_then(|r| r.get("in_use_trash"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
        let action = match (keeps, kept) {
            (Some(_), _) => Action::BackupEraseStays,
            (None, 0) => Action::BackupErase,
            (None, _) => Action::BackupEraseShared,
        };
        Removal::begin(journal, action, entry.surface)
            .for_item(&entry.id)
            .record_event(Event::Removed, entry.unit.clone());
        match action {
            Action::BackupEraseStays => report.staying += 1,
            Action::BackupEraseShared => report.shared += 1,
            _ => report.removed += 1,
        }
        if backup_erase::retire(journal, &entry.id).is_ok() {
            done.insert(entry.id.clone());
        }
    }
    report.pending = live.len() - done.len();
    match (keeps, held_back) {
        (Some(kind), _) => finish("stays", Some(kind), &mut report),
        (None, Some(reason)) => finish("stays", Some(reason), &mut report),
        (None, None) if report.pending > 0 => finish("waiting", None, &mut report),
        (None, None) => {
            // Nothing left owes a backup half: the saved list has done its job.
            let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
            finish("idle", None, &mut report);
            report.state = "done".into();
        }
    }
    report
}

/// How long a `stays` word stays once its copies may have gone: until the
/// next erase checks again, or a week.
pub const STAYS_DAYS: i64 = KEEP_RESULTS_DAYS;
