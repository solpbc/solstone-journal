// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single search-index writer consumes the file-backed work list.

use crate::StoreError;
use crate::writer_admission::IndexAdmission;
use serde::Serialize;
pub use solstone_core_indexer_todo::{
    Entry, TodoStatus, Token, enqueue_path, enqueue_path_bound, enqueue_token, has_due, inspect,
    now_ms,
};
use solstone_core_indexer_todo::{
    entry_paths, path_absent, publish_if_absent, read_entry, rewrite, root,
};
use solstone_core_journal_io::append::append_jsonl;
use solstone_core_journal_io::locking::{LockOptions, hold_lock_checked};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

const BATCH_SIZE: usize = 32;
const RETRY_MS: u64 = 120_000;
const DELETE_RETRY_MS: u64 = 3_600_000;

#[derive(Clone, Copy, Debug)]
pub struct DrainOptions {
    /// Daily think ignores retry delays so its talents can read their inputs.
    pub force: bool,
    pub budget: Duration,
}

impl Default for DrainOptions {
    fn default() -> Self {
        Self {
            force: false,
            budget: Duration::from_secs(110 * 60),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DrainReport {
    pub batches: usize,
    pub processed: usize,
    pub failed: usize,
    pub recovered: usize,
    pub timed_out: bool,
    pub remaining: TodoStatus,
}

fn record_attempt(journal: &Path, entry: &Entry, outcome: &str, cause: Option<&str>) {
    let now = chrono::Utc::now();
    let relative = solstone_core_journal_io::durability::artifact(
        solstone_core_journal_io::durability::ArtifactId::IndexAttempts,
    )
    .path
    .replacen('*', &now.format("%Y%m%d").to_string(), 1);
    let log_path = journal.join(relative);
    let event = serde_json::json!({
        "event": "index.attempt", "ts": now.timestamp_millis(),
        "path": entry.key, "outcome": outcome, "cause": cause,
    });
    if let Err(error) = append_jsonl(&log_path, &event) {
        log::warn!(target: "solstone::indexer", "could not record index attempt for {}: {error}", entry.key);
    }
}

/// Only called with the persistent drain lock held. A producer cannot replace
/// the fresh entry while its attempts are carried forward here.
fn return_failed(journal: &Path, mut entry: Entry, claim: &Path, cause: &str) -> io::Result<()> {
    let destination = root(journal).join(entry.filename());
    if let Some(fresh) = read_entry(&destination, false)? {
        entry.attempts = entry.attempts.max(fresh.attempts);
    }
    entry.attempts = entry.attempts.saturating_add(1);
    // A crash while returning work must not recover the claim's older count.
    rewrite(claim, &entry)?;
    record_attempt(journal, &entry, "failed", Some(cause));
    let failed_deletion = entry.kind == "path" && path_absent(journal, &entry.key);
    if entry.attempts >= 3 && !failed_deletion {
        // The catch-all will retry a stale present file. A fresh save does not
        // reset the crash budget of the claim it replaced.
        if let Some(fresh) = read_entry(&destination, false)? {
            entry.attempts = entry.attempts.max(fresh.attempts);
            fs::remove_file(&destination)?;
        }
    } else if let Some(mut fresh) = read_entry(&destination, false)? {
        fresh.attempts = fresh.attempts.max(entry.attempts);
        fresh.not_before_ms = 0;
        rewrite(&destination, &fresh)?;
    } else {
        entry.not_before_ms = now_ms().saturating_add(if entry.attempts >= 3 {
            DELETE_RETRY_MS
        } else {
            RETRY_MS
        });
        publish_if_absent(&destination, &entry)?;
        // A producer can win publication after the preceding read. Carry the
        // attempts onto that entry as well; only the drain ever rewrites it.
        if let Some(mut fresh) = read_entry(&destination, false)?
            && fresh.attempts < entry.attempts
        {
            fresh.attempts = entry.attempts;
            fresh.not_before_ms = 0;
            rewrite(&destination, &fresh)?;
        }
    }
    fs::remove_file(claim)
}

/// Serialize drains across scheduled, explicit, daily-think and mobile runs.
/// The writer lease is released between bounded batches, independently of this lock.
pub fn drain(journal: &Path, options: DrainOptions) -> Result<DrainReport, StoreError> {
    drain_with_checkpoint(journal, options, &|| Ok(()))
}

pub fn drain_with_checkpoint(
    journal: &Path,
    options: DrainOptions,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<DrainReport, StoreError> {
    let started = Instant::now();
    let check = || {
        checkpoint()?;
        if started.elapsed() >= options.budget {
            Err("index work wait expired".to_owned())
        } else {
            Ok(())
        }
    };
    let _drain_lock = match hold_lock_checked(
        journal.join("indexer/drain"),
        LockOptions {
            timeout: options.budget,
            ..LockOptions::default()
        },
        &check,
    ) {
        Ok(lock) => lock,
        Err(solstone_core_journal_io::errors::LockError::Timeout(_)) => {
            return Ok(DrainReport {
                timed_out: true,
                remaining: inspect(journal)?,
                ..DrainReport::default()
            });
        }
        Err(_) if check().is_err() => {
            return Ok(DrainReport {
                timed_out: true,
                remaining: inspect(journal)?,
                ..DrainReport::default()
            });
        }
        Err(error) => return Err(StoreError::Io(io::Error::other(error))),
    };
    fs::create_dir_all(root(journal).join("claimed"))?;
    let mut report = DrainReport::default();
    for claim in entry_paths(&root(journal).join("claimed"))? {
        if let Some(entry) = read_entry(&claim, true)? {
            return_failed(
                journal,
                entry,
                &claim,
                "the previous index task stopped before finishing this entry",
            )?;
            report.recovered += 1;
        }
    }
    let mut entries = Vec::new();
    let at_ms = now_ms();
    for path in entry_paths(&root(journal))? {
        if let Some(entry) = read_entry(&path, true)?
            && (options.force || entry.not_before_ms <= at_ms)
        {
            entries.push(entry);
        }
    }
    entries.sort_by_key(|entry| (entry.rank(), entry.enqueued_ms, entry.key.clone()));
    let mut offset = 0;
    while offset < entries.len() {
        if check().is_err() {
            report.timed_out = true;
            break;
        }
        let admission = match IndexAdmission::acquire_bounded(
            journal,
            "todo",
            options
                .budget
                .saturating_sub(started.elapsed())
                .min(Duration::from_secs(10)),
            &check,
        ) {
            Ok(admission) => admission,
            Err(StoreError::WriterBusy { .. }) if check().is_ok() => continue,
            Err(StoreError::WriterBusy { .. } | StoreError::WriterAdmission { .. })
                if check().is_err() =>
            {
                report.timed_out = true;
                break;
            }
            Err(error) => return Err(error),
        };
        let end = (offset + BATCH_SIZE).min(entries.len());
        report.batches += 1;
        for entry in &entries[offset..end] {
            if check().is_err() {
                report.timed_out = true;
                break;
            }
            let path = root(journal).join(entry.filename());
            let claim = root(journal).join("claimed").join(entry.filename());
            if entry.kind == "path" && entry.attempts >= 2 {
                // Do this before claiming: if invalidation cannot commit, the
                // hint keeps its remaining attempt. A third crash can then drop
                // the hint without letting a same-second source look current.
                crate::db::invalidate_paths_admitted(journal, &[&entry.key], &admission)?;
            }
            match fs::rename(&path, &claim) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            }
            crate::writer_admission::check_test_seam("todo-claimed");
            let result = process(journal, entry, &admission, &check);
            match result {
                Ok(outcome) => {
                    record_attempt(journal, entry, outcome, None);
                    fs::remove_file(&claim)?;
                    report.processed += 1;
                }
                Err(error) => {
                    return_failed(journal, entry.clone(), &claim, &error.to_string())?;
                    report.failed += 1;
                }
            }
        }
        drop(admission);
        crate::writer_admission::check_test_seam("todo-batch-released");
        if report.timed_out {
            break;
        }
        offset = end;
    }
    report.remaining = inspect(journal)?;
    Ok(report)
}

fn process(
    journal: &Path,
    entry: &Entry,
    admission: &IndexAdmission,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<&'static str, StoreError> {
    match entry.rank() {
        Some(0) => crate::scan::reconcile_path_admitted(journal, &entry.key, admission, checkpoint),
        Some(1) => {
            let report = crate::scan::scan_journal_admitted(journal, true, admission, checkpoint)?;
            if report.failed > 0 {
                return Err(io::Error::other(format!(
                    "index scan failed for {} files",
                    report.failed
                ))
                .into());
            }
            Ok("indexed")
        }
        Some(2) => {
            let mut snapshot = || {
                let _trust = solstone_core_entity::hold_facet_trust_lock(journal)
                    .map_err(|error| StoreError::Io(io::Error::other(error)))?;
                crate::classification::FacetDeclarationSet::from_journal(journal)
            };
            let report = crate::reconcile::reconcile_stale_classifications_admitted(
                journal,
                &mut snapshot,
                admission,
            )?;
            if report.incomplete {
                return Err(io::Error::other("facet classifications kept changing").into());
            }
            Ok("indexed")
        }
        Some(3) => {
            crate::scan::update_entity_search_admitted(journal, admission)?;
            Ok("indexed")
        }
        _ => unreachable!("only known entries are claimed"),
    }
}

/// Test fixtures that save through production seams can explicitly converge.
#[cfg(any(test, feature = "test-fixtures"))]
pub fn drain_now(journal: &Path) -> Result<DrainReport, StoreError> {
    drain(
        journal,
        DrainOptions {
            force: true,
            ..DrainOptions::default()
        },
    )
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "todo_tests.rs"]
mod tests;
