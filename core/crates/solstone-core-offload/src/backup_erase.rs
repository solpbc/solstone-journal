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
//! Four rules hold throughout:
//! - **A read that fails decides nothing.** Settings, the destination, the
//!   trash or the ledger that can't be read leave every entry waiting; only a
//!   destination that is known to be gone or changed retires an entry, and
//!   then unreached, with no claim.
//! - **Each entry stands alone.** One that can't be placed, or that the helper
//!   reports missed, waits on its own and never holds the others back.
//! - **Every way the helper ends has a state and a pace:** see [`outcome`].
//! - **Media archives keep the id the ledger recorded.** Requests name them by
//!   it; the helper matches it as a snapshot's id or its `original`.
//!
//! The local erase never waits for this. One runner at a time. Each outcome is
//! written to the removal history, and the page reads [`status`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
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
const PIECES_FILE: &str = "pieces.json";
const REQUEST_FILE: &str = "request.json";
const OFFLOAD_TAG: &str = solstone_core_backup::archive_readback::OFFLOAD_ARCHIVE_TAG;

/// Where the runner stands, for the trash page and the backup page.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Status {
    /// `idle`, `running`, `waiting`, `stopped`, `failed` or `stays`.
    pub state: String,
    /// Why, in a stable word: `paused`, `offline`, `busy`, `unavailable`,
    /// `stopped`, `refused`, `residue`, `unknown`, `versions`, `locks`,
    /// `hidden_files`, `legacy_archive`, `elsewhere`, `ledger`, `missed`,
    /// `error`.
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
    /// Entries waiting on their own, by id: why, and when each may try again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub held: BTreeMap<String, Held>,
}

/// One entry waiting on its own.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Held {
    pub reason: String,
    pub attempts: u32,
    pub next_try: String,
}

fn dir(journal: &Path) -> PathBuf {
    journal.join(backup_erase::DIR)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The runner's last word, or idle.
pub fn status(journal: &Path) -> Status {
    let (readable, unreadable) = backup_erase::pending(journal);
    let pending = readable.len() + unreadable.len();
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

fn write_status(journal: &Path, status: &Status) {
    if let Ok(bytes) = serde_json::to_vec_pretty(status) {
        let _ = std::fs::create_dir_all(dir(journal));
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
    pub held: usize,
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
/// be read in full for this segment.
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

/// Whether an entry's paths are ones the helper can name exactly: clean,
/// relative and slash-separated.
fn placeable(entry: &Entry) -> bool {
    let clean = |rel: &str| {
        !rel.is_empty()
            && !rel.starts_with('/')
            && !rel.contains('\\')
            && rel
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..")
    };
    entry.paths.iter().all(|path| clean(path)) && clean(&entry.trash_path)
}

/// Read back, from the destination, which media archives a backup erase
/// rewrote, and record any move the ledger's moves are missing: after a
/// whole-journal restore they are only as new as the backup restored. Once a
/// day while the journal has offloaded media, and `forced` after every run of
/// the helper, however it ended: a rewrite it did and couldn't report, or a
/// stale replacement a resumed run forgot, is read back from the archives.
fn reconcile(journal: &Path, services: &BackupServices<'_>, forced: bool) {
    let marker = dir(journal).join(RECONCILED_FILE);
    let fresh = !forced
        && std::fs::metadata(&marker)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(24 * 60 * 60));
    if fresh {
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

/// When a failed try may come again: an hour, doubling, at most a day.
fn next_try(attempts: u32) -> DateTime<Utc> {
    let hours = 1_i64 << attempts.clamp(1, 6).saturating_sub(1);
    Utc::now() + chrono::Duration::hours(hours.min(24))
}

fn due(at: &str) -> bool {
    DateTime::parse_from_rfc3339(at).map_or(true, |at| at.with_timezone(&Utc) <= Utc::now())
}

/// One line in the engine's own record of each outcome, with its counts.
fn record_result(journal: &Path, line: Value) {
    let _ = std::fs::create_dir_all(dir(journal));
    let _ = solstone_core_journal_io::append_jsonl(dir(journal).join(RESULTS_FILE), &line);
}

/// How one run of the helper ended, for the whole run: a state, its reason,
/// and whether the next try waits its turn. Per-entry outcomes come from an
/// ok run's report.
#[must_use]
pub fn outcome(
    ok: bool,
    reason: Option<&str>,
    answered: bool,
) -> (&'static str, &'static str, bool) {
    match (answered, ok, reason) {
        // No answer at all: killed, timed out, or stopped part way. The saved
        // list and the helper's own recovery take it up again.
        (false, _, _) => ("stopped", "stopped", true),
        (true, true, _) => ("done", "", false),
        // Only another holder of the lock is a wait in turn: a running backup.
        (true, false, Some("lock")) => ("waiting", "busy", false),
        (true, false, Some("open")) => ("waiting", "offline", true),
        (true, false, Some("stopped")) => ("stopped", "stopped", true),
        (true, false, Some("delete_refused")) => ("stays", "refused", true),
        (true, false, Some("residue")) => ("stays", "residue", true),
        (true, false, _) => ("failed", "error", true),
    }
}

/// Run once: drain every pending entry the backup can take now.
pub fn run(journal: &Path, services: &BackupServices<'_>) -> Report {
    // One runner at a time; a second finds it busy at once and leaves it.
    let _ = std::fs::create_dir_all(dir(journal));
    let Ok(_lock) = solstone_core_journal_io::hold_lock(
        dir(journal),
        solstone_core_journal_io::LockOptions {
            timeout: std::time::Duration::from_secs(1),
            ..solstone_core_journal_io::LockOptions::default()
        },
    ) else {
        return Report {
            state: "running".into(),
            pending: backup_erase::pending(journal).0.len(),
            ..Report::default()
        };
    };
    // Read under the lock, so no other runner's work slips between.
    let (entries, unreadable) = backup_erase::pending(journal);
    // Under the lock: the moves file is read, changed and written whole.
    reconcile(journal, services, false);
    if entries.is_empty() && unreadable.is_empty() {
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
        return Report {
            state: "nothing".into(),
            ..Report::default()
        };
    }
    let previous = status(journal);
    let mut status = Status {
        updated: now(),
        held: previous
            .held
            .clone()
            .into_iter()
            .filter(|(id, _)| entries.iter().any(|entry| &entry.id == id))
            .collect(),
        ..Status::default()
    };
    let mut report = Report::default();
    let finish = |journal: &Path,
                  mut status: Status,
                  state: &str,
                  reason: Option<&str>,
                  retry: bool,
                  report: &mut Report| {
        status.state = state.into();
        status.reason = reason.map(str::to_owned);
        status.attempts = if retry {
            previous.attempts.saturating_add(1)
        } else {
            0
        };
        status.next_try =
            retry.then(|| next_try(status.attempts).to_rfc3339_opts(SecondsFormat::Millis, true));
        let (left, torn) = backup_erase::pending(journal);
        status.pending = left.len() + torn.len();
        status.updated = now();
        report.state = state.into();
        report.reason = reason.map(str::to_owned);
        report.pending = status.pending;
        report.held = status.held.len();
        write_status(journal, &status);
    };
    // A run that failed waits its turn, unless the owner erased more since.
    let newest = entries
        .iter()
        .map(|entry| entry.erased_at.as_str())
        .max()
        .unwrap_or_default();
    if let Some(at) = previous.next_try.as_deref()
        && !due(at)
        && newest <= previous.updated.as_str()
    {
        report.state = previous.state.clone();
        report.reason = previous.reason.clone();
        report.pending = entries.len();
        return report;
    }

    // Rule one: a read that fails decides nothing.
    let current = match solstone_core_backup::destination_identity(journal) {
        Ok(current) => current,
        Err(_) => {
            finish(journal, status, "waiting", Some("error"), true, &mut report);
            return report;
        }
    };
    // A destination known to be gone, or changed, can't be reached: its
    // entries are retired unreached, with no claim.
    let mut live = Vec::new();
    for entry in entries {
        if current.is_none() || entry.destination != current {
            record_result(
                journal,
                json!({"id": entry.id, "at": now(), "outcome": "not_reached"}),
            );
            let _ = backup_erase::retire(journal, &entry.id);
            status.held.remove(&entry.id);
            report.unreached += 1;
        } else {
            live.push(entry);
        }
    }
    if live.is_empty() {
        if !unreadable.is_empty() {
            finish(
                journal,
                status,
                "failed",
                Some("unreadable"),
                false,
                &mut report,
            );
            return report;
        }
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
        finish(journal, status, "idle", None, false, &mut report);
        return report;
    }
    if solstone_core_backup_runtime::backup_paused(journal) {
        finish(
            journal,
            status,
            "waiting",
            Some("paused"),
            false,
            &mut report,
        );
        return report;
    }
    let Some(helper) = erase_paths::helper(services) else {
        finish(
            journal,
            status,
            "waiting",
            Some("unavailable"),
            false,
            &mut report,
        );
        return report;
    };
    let Ok(in_trash) = solstone_core_trash::ids(journal) else {
        finish(journal, status, "waiting", Some("error"), true, &mut report);
        return report;
    };
    let Some(root) = erase_paths::journal_root(journal) else {
        finish(journal, status, "waiting", Some("error"), true, &mut report);
        return report;
    };
    let destination = match erase_paths::open(journal, services) {
        Ok(Some(destination)) => destination,
        Ok(None) => {
            finish(
                journal,
                status,
                "waiting",
                Some("unavailable"),
                false,
                &mut report,
            );
            return report;
        }
        Err(_) => {
            finish(
                journal,
                status,
                "waiting",
                Some("offline"),
                true,
                &mut report,
            );
            return report;
        }
    };

    // Rule two: each entry stands alone. One held, and not due, sits out; one
    // that can't be placed is held on its own.
    let hold = |status: &mut Status, id: &str, reason: &str| {
        let attempts = status
            .held
            .get(id)
            .map_or(0, |held| held.attempts)
            .saturating_add(1);
        status.held.insert(
            id.to_owned(),
            Held {
                reason: reason.into(),
                attempts,
                next_try: next_try(attempts).to_rfc3339_opts(SecondsFormat::Millis, true),
            },
        );
    };
    let mut targets = Vec::new();
    let mut snapshot_paths: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut legacy = BTreeSet::new();
    let mut runnable = Vec::new();
    for entry in &live {
        if status
            .held
            .get(&entry.id)
            .is_some_and(|held| !due(&held.next_try))
        {
            continue;
        }
        if !placeable(entry) {
            hold(&mut status, &entry.id, "error");
            continue;
        }
        let (archives, legacy_archive) = match archive_paths(journal, entry) {
            Ok(found) => found,
            Err(reason) => {
                hold(&mut status, &entry.id, reason);
                continue;
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
        runnable.push(entry);
    }
    if runnable.is_empty() {
        let reason = status.held.values().next().map(|held| held.reason.clone());
        finish(
            journal,
            status,
            "failed",
            reason.as_deref(),
            false,
            &mut report,
        );
        return report;
    }
    let ours: BTreeSet<&str> = live.iter().map(|entry| entry.id.as_str()).collect();
    let live_trash: Vec<String> = in_trash
        .into_iter()
        .filter(|id| !ours.contains(id.as_str()))
        .collect();
    let request = json!({
        "roots": [root],
        "targets": targets,
        "trash": {"dir": solstone_core_trash::TRASH_DIR, "files": "files", "live": live_trash,
                  "item": "item.json", "paths_key": "paths"},
        "snapshot_paths": snapshot_paths,
        "drop_empty_tags": [OFFLOAD_TAG],
    });
    let request_path = dir(journal).join(REQUEST_FILE);
    if std::fs::write(&request_path, request.to_string()).is_err() {
        finish(journal, status, "failed", Some("error"), true, &mut report);
        return report;
    }
    {
        let mut running = status.clone();
        running.state = "running".into();
        running.updated = now();
        write_status(journal, &running);
    }
    let erased = destination.erase(
        services,
        &helper,
        &request_path,
        &dir(journal).join(PIECES_FILE),
    );
    let _ = std::fs::remove_file(&request_path);
    // Whatever the helper reported, read back what it rewrote.
    reconcile(journal, services, true);

    // Rule three: every way the helper ends has a state and a pace.
    let (answered, ok, reason, result) = match &erased {
        Ok(erased) => (
            true,
            erased.returncode == 0 && erased.result.get("ok") == Some(&Value::Bool(true)),
            erased.result.get("reason").and_then(Value::as_str),
            Some(&erased.result),
        ),
        Err(_) => (false, false, None, None),
    };
    let (state, word, retry) = outcome(ok, reason, answered);
    let Some(result) = result.filter(|_| ok) else {
        if state == "stays" {
            report.staying = runnable.len();
            status.staying = runnable.len();
        }
        finish(journal, status, state, Some(word), retry, &mut report);
        return report;
    };

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
    // Not recorded here, the next run's reconcile reads them back: a restore
    // of kept media in a rewritten archive needs its current id.
    if crate::ledger::record_archive_moves(journal, &moves).is_err() {
        let _ = std::fs::remove_file(dir(journal).join(RECONCILED_FILE));
    }
    if let Some(keys) = result.get("unmatched_keys").and_then(Value::as_array)
        && !keys.is_empty()
    {
        record_result(
            journal,
            json!({"at": now(), "outcome": "unmatched_archives", "keys": keys}),
        );
    }

    // Every listed piece no backup uses is gone. What the storage keeps of a
    // deleted file is the storage's own answer.
    let keeps = match destination.storage(services) {
        StorageRetention::Removes => None,
        StorageRetention::Keeps(StorageKeeps::Versions) => Some("versions"),
        StorageRetention::Keeps(StorageKeeps::ObjectLock | StorageKeeps::FileLock) => Some("locks"),
        StorageRetention::Keeps(StorageKeeps::HiddenFiles) => Some("hidden_files"),
        StorageRetention::Unknown => Some("unknown"),
    };
    let entries_report = result.get("entries").cloned().unwrap_or(Value::Null);
    let field = |entry: &Entry, key: &str| {
        entries_report
            .get(&entry.id)
            .and_then(|r| r.get(key))
            .cloned()
    };
    let count = |entry: &Entry, key: &str| field(entry, key).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut stays_reason: Option<&'static str> = keeps;
    for entry in runnable {
        // Missed: something that holds it wasn't reached. It waits on its own,
        // with no claim, and the rest go on.
        if field(entry, "missed").and_then(|v| v.as_bool()) == Some(true) {
            hold(&mut status, &entry.id, "missed");
            record_result(
                journal,
                json!({"id": entry.id, "at": now(), "outcome": "missed",
                "missed_at": field(entry, "missed_at")}),
            );
            continue;
        }
        let shared = count(entry, "in_use_kept") + count(entry, "in_use_trash");
        let elsewhere = count(entry, "elsewhere") > 0;
        let left_alone = legacy.contains(&entry.id);
        // Some backups still able to restore it is the larger truth, ahead of
        // what the storage keeps of deleted files.
        let action = if left_alone || elsewhere {
            Action::BackupErasePartly
        } else if keeps.is_some() {
            Action::BackupEraseStays
        } else if shared > 0 {
            Action::BackupEraseShared
        } else {
            Action::BackupErase
        };
        // The run's reason: backups that still hold it outrank storage that
        // keeps deleted files.
        if elsewhere {
            stays_reason = Some("elsewhere");
        } else if left_alone && stays_reason != Some("elsewhere") {
            stays_reason = Some("legacy_archive");
        }
        // Durable: a run that stops before the entry is retired finds the
        // same history row again rather than writing a second. A row that
        // can't be written keeps the entry for the next run.
        let at = DateTime::parse_from_rfc3339(&entry.erased_at)
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let written = Removal::with_id(
            journal,
            OperationId::durable(at, &format!("{}backups", entry.id)),
            action,
            // The backup half has the backup's place word in the history.
            Surface::Web { page: Page::Backup },
        )
        .for_item(&entry.id)
        .try_record_event(Event::Removed, entry.unit.clone());
        if written.is_err() {
            hold(&mut status, &entry.id, "error");
            continue;
        }
        record_result(
            journal,
            json!({
                "id": entry.id, "at": now(), "outcome": format!("{action:?}"),
                "snapshots": count(entry, "snapshots"), "files": count(entry, "files"),
                "in_use_kept": count(entry, "in_use_kept"), "in_use_trash": count(entry, "in_use_trash"),
                "elsewhere": count(entry, "elsewhere"),
                "storage": keeps.unwrap_or("removes"), "legacy_archive": left_alone,
            }),
        );
        match action {
            Action::BackupEraseStays | Action::BackupErasePartly => report.staying += 1,
            Action::BackupEraseShared => report.shared += 1,
            _ => report.removed += 1,
        }
        if backup_erase::retire(journal, &entry.id).is_ok() {
            status.held.remove(&entry.id);
        }
    }
    status.staying = report.staying;
    let (left, torn) = backup_erase::pending(journal);
    if !torn.is_empty() {
        // An entry that can't be read still owes its backup half: it isn't
        // dropped, and the saved list stays for it.
        let any_held = !status.held.is_empty() || stays_reason.is_some();
        if !any_held {
            finish(
                journal,
                status,
                "failed",
                Some("unreadable"),
                false,
                &mut report,
            );
            return report;
        }
    }
    if left.is_empty() && torn.is_empty() {
        // Nothing left owes a backup half: the saved list has done its job.
        let _ = std::fs::remove_file(dir(journal).join(PIECES_FILE));
    }
    let any_held = !status.held.is_empty();
    match (stays_reason, any_held) {
        (Some(kind), _) => finish(journal, status, "stays", Some(kind), false, &mut report),
        (None, true) => {
            let reason = status.held.values().next().map(|held| held.reason.clone());
            finish(
                journal,
                status,
                "failed",
                reason.as_deref(),
                false,
                &mut report,
            );
        }
        (None, false) => {
            let left = left.len();
            finish(
                journal,
                status,
                if left > 0 { "waiting" } else { "idle" },
                None,
                false,
                &mut report,
            );
            if left == 0 {
                report.state = "done".into();
            }
        }
    }
    report
}

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
    fn every_way_the_helper_ends_has_a_state_and_a_pace() {
        assert_eq!(outcome(false, None, false), ("stopped", "stopped", true));
        assert_eq!(outcome(true, None, true), ("done", "", false));
        assert_eq!(
            outcome(false, Some("lock"), true),
            ("waiting", "busy", false)
        );
        assert_eq!(
            outcome(false, Some("open"), true),
            ("waiting", "offline", true)
        );
        assert_eq!(
            outcome(false, Some("stopped"), true),
            ("stopped", "stopped", true)
        );
        assert_eq!(
            outcome(false, Some("delete_refused"), true),
            ("stays", "refused", true)
        );
        assert_eq!(
            outcome(false, Some("residue"), true),
            ("stays", "residue", true)
        );
        for other in [
            None,
            Some("list"),
            Some("find"),
            Some("rewrite"),
            Some("index"),
            Some("new"),
        ] {
            assert_eq!(
                outcome(false, other, true),
                ("failed", "error", true),
                "{other:?}"
            );
        }
    }

    #[test]
    fn only_clean_relative_paths_are_placed() {
        assert!(placeable(&entry(&[
            "chronicle/20261101/macbook/090000_300"
        ])));
        for bad in ["/abs", "a/../b", "a//b", "a\\b", "./a", ""] {
            assert!(!placeable(&entry(&[bad])), "{bad}");
        }
    }

    #[test]
    fn a_failed_try_waits_an_hour_doubling_to_a_day() {
        let hours = |attempts| (next_try(attempts) - Utc::now()).num_minutes() as f64 / 60.0;
        assert!((hours(1) - 1.0).abs() < 0.05);
        assert!((hours(3) - 4.0).abs() < 0.05);
        assert!((hours(40) - 24.0).abs() < 0.05);
    }

    #[test]
    fn idle_until_something_is_pending() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(status(temp.path()).state, "idle");
        assert_eq!(status(temp.path()).pending, 0);
    }
}
