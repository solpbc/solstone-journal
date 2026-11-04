// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Edges follow their inputs.
//!
//! An edge file's rows depend on its own bytes and on what it was resolved
//! against: the names it looked up, the speaker index, the owner's timezone
//! and the extractor itself. `reconcile_edges` follows the bytes by mtime.
//! This module follows the rest. A change marks only the files it reaches as
//! stale, and a bounded pass brings stale files current one file at a time,
//! newest day first, so a long journal is never rebuilt in one transaction and
//! its connections never read empty.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use solstone_core_format::paths::CHRONICLE_DIR;
use solstone_core_format::segment::day_of;
use solstone_core_indexer::edges::candidates::EdgeResolver;

use crate::StoreError;

/// The `edge_files.mtime` of a file whose rows wait to be re-extracted
/// because something it was resolved against changed. No real file has it.
pub(crate) const STALE_EDGE_MTIME: i64 = -1;

/// Raise this when extraction changes what it produces from the same inputs.
/// Every edge file is then refreshed once, progressively.
pub(crate) const EDGE_EXTRACTOR_VERSION: i64 = 1;

const EXTRACTOR_PATH: &str = "edges:extractor";
const TIMEZONE_PATH: &str = "edges:timezone";
const SPEAKERS_PATH: &str = "edges:speakers";
const CANDIDATES_PREFIX: &str = "edges:candidates:";
const SPEAKER_LABELS_SUFFIX: &str = "/talents/speaker_labels.json";

/// What one invalidation found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EdgeInvalidation {
    /// Edge files newly marked stale.
    pub marked: usize,
    /// The journal's people changed, so calendar invitees were linked again.
    pub people_changed: bool,
}

/// What one refresh pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EdgeRefresh {
    pub refreshed: usize,
    /// Stale files no longer on disk, left for the scan's removal.
    pub missing: usize,
    pub rows_inserted: usize,
    pub drops: usize,
    /// Files skipped for an invalid segment key.
    pub skipped: usize,
    pub failed: Vec<String>,
    pub warnings: Vec<String>,
    /// Stale files left for a later pass.
    pub remaining: usize,
}

fn markers(conn: &Connection) -> Result<std::collections::BTreeMap<String, i64>, StoreError> {
    let mut statement =
        conn.prepare("SELECT path, mtime FROM edge_files WHERE path LIKE 'edges:%'")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn set_marker(conn: &Connection, path: &str, value: i64) -> Result<(), StoreError> {
    conn.execute(
        "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
        params![path, value],
    )?;
    Ok(())
}

fn mark_where(
    conn: &Connection,
    predicate: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<usize, StoreError> {
    Ok(conn.execute(
        &format!(
            "UPDATE edge_files SET mtime = {STALE_EDGE_MTIME} \
             WHERE path NOT LIKE 'edges:%' AND mtime != {STALE_EDGE_MTIME} AND ({predicate})"
        ),
        args,
    )?)
}

/// Compare what edge files were resolved against with what they would be
/// resolved against now, and mark the files a difference reaches as stale.
/// Resolution runs outside any write transaction; each mark commits on its own.
pub(crate) fn invalidate_edges(
    conn: &mut Connection,
    journal: &Path,
    resolver: &mut EdgeResolver,
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<EdgeInvalidation, StoreError> {
    let mut report = EdgeInvalidation::default();
    let known = markers(conn)?;
    let timezone = resolver.timezone_digest().map_err(edge_error)?;

    let everything = known.get(EXTRACTOR_PATH) != Some(&EDGE_EXTRACTOR_VERSION)
        || known.get(TIMEZONE_PATH) != Some(&timezone);
    if everything {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        report.marked += mark_where(&tx, "1", &[])?;
        set_marker(&tx, EXTRACTOR_PATH, EDGE_EXTRACTOR_VERSION)?;
        set_marker(&tx, TIMEZONE_PATH, timezone)?;
        crate::db::admit_scan_journal(checkpoint)?;
        tx.commit()?;
    }

    let speakers = resolver.speakers_digest().map_err(edge_error)?;
    if known.get(SPEAKERS_PATH) != Some(&speakers) {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        report.marked += mark_where(&tx, "path LIKE ?", &[&format!("%{SPEAKER_LABELS_SUFFIX}")])?;
        set_marker(&tx, SPEAKERS_PATH, speakers)?;
        crate::db::admit_scan_journal(checkpoint)?;
        tx.commit()?;
        report.people_changed = true;
    }

    // Each facet resolves against its own entities. A facet whose entities
    // are unchanged answers every name as before; for one that changed, ask
    // again only the names its files looked up.
    let mut facets = {
        let mut statement = conn.prepare("SELECT DISTINCT facet FROM edge_lookups")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if !facets.iter().any(String::is_empty) {
        facets.push(String::new());
    }
    for facet in facets {
        crate::db::admit_scan_journal(checkpoint)?;
        let marker = format!("{CANDIDATES_PREFIX}{facet}");
        let digest = resolver.candidates_digest(&facet).map_err(edge_error)?;
        if known.get(&marker) == Some(&digest) {
            continue;
        }
        let asked = {
            let mut statement = conn.prepare(
                "SELECT DISTINCT confident, name, entity_id FROM edge_lookups WHERE facet = ?",
            )?;
            let rows = statement.query_map([&facet], |row| {
                Ok((
                    row.get::<_, bool>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut changed = Vec::new();
        for (confident, name, answered) in asked {
            let answer = resolver
                .answer(&facet, confident, &name)
                .map_err(edge_error)?;
            if answer != answered {
                changed.push((confident, name, answered));
            }
        }
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (confident, name, answered) in &changed {
            report.marked += mark_where(
                &tx,
                "path IN (SELECT path FROM edge_lookups \
                 WHERE facet = ? AND confident = ? AND name = ? AND entity_id IS ?)",
                &[&facet, confident, name, answered],
            )?;
        }
        set_marker(&tx, &marker, digest)?;
        crate::db::admit_scan_journal(checkpoint)?;
        tx.commit()?;
        if facet.is_empty() {
            report.people_changed = true;
        }
    }

    if report.people_changed {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        crate::calendar_schedule::relink_all_people(&tx, journal)?;
        crate::db::admit_scan_journal(checkpoint)?;
        tx.commit()?;
    }
    Ok(report)
}

fn edge_error(error: solstone_core_indexer::edges::EdgeError) -> StoreError {
    StoreError::Io(std::io::Error::other(error.to_string()))
}

/// The stale edge files, newest day first; files without a day come last.
fn stale_paths(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT path FROM edge_files WHERE mtime = {STALE_EDGE_MTIME} AND path NOT LIKE 'edges:%'"
    ))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut paths = rows.collect::<Result<Vec<_>, _>>()?;
    paths.sort_by(|a, b| {
        (Reverse(day_of(a).map(str::to_owned)), a).cmp(&(Reverse(day_of(b).map(str::to_owned)), b))
    });
    Ok(paths)
}

/// Where an edge file key lives: structural sources under the journal,
/// day-rooted ones under the chronicle when the journal has one.
fn edge_source_path(journal: &Path, rel: &str) -> PathBuf {
    if rel.starts_with("facets/") {
        return journal.join(rel);
    }
    let chronicle = journal.join(CHRONICLE_DIR);
    if chronicle.is_dir() {
        chronicle.join(rel)
    } else {
        journal.join(rel)
    }
}

/// Bring stale edge files current, one file per transaction, newest day
/// first, until none are left or `budget` is spent. A file that fails or is
/// missing keeps its rows and goes back to the scan, which retries a failed
/// file every pass and removes a missing one under its own rules; so one bad
/// file never holds up the files behind it.
pub(crate) fn refresh_stale_edges(
    conn: &mut Connection,
    journal: &Path,
    resolver: &mut EdgeResolver,
    checkpoint: &dyn Fn() -> Result<(), String>,
    budget: Option<Duration>,
) -> Result<EdgeRefresh, StoreError> {
    let mut report = EdgeRefresh::default();
    let paths = stale_paths(conn)?;
    // After the listing, so a long list can't use up the slice unworked.
    let started = Instant::now();
    let mut handled = 0;
    for rel in &paths {
        if budget.is_some_and(|budget| started.elapsed() >= budget) {
            break;
        }
        crate::db::admit_scan_journal(checkpoint)?;
        handled += 1;
        let path = edge_source_path(journal, rel);
        let mtime = match crate::scan::file_mtime_secs(&path) {
            Ok(mtime) => mtime,
            // A missing file is the scan's to remove, under its rules for
            // days whose files are all missing; its rows stay until then.
            Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                hand_to_scan(conn, rel)?;
                report.missing += 1;
                continue;
            }
            Err(error) => {
                hand_to_scan(conn, rel)?;
                report.failed.push(rel.clone());
                report
                    .warnings
                    .push(format!("Skipping edge refresh for {rel}: {error}"));
                continue;
            }
        };
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result =
            crate::scan::replace_edge_file_edges(&tx, journal, rel, &path, mtime, resolver)?;
        if result.failed {
            tx.rollback()?;
            hand_to_scan(conn, rel)?;
            report.failed.push(rel.clone());
            report.warnings.extend(result.warnings);
            continue;
        }
        tx.commit()?;
        report.refreshed += 1;
        report.rows_inserted += result.rows_inserted;
        report.drops += result.drops;
        report.skipped += usize::from(result.invalid_segment);
        report.warnings.extend(result.warnings);
    }
    report.remaining = paths.len() - handled;
    Ok(report)
}

/// How many edge files wait to be refreshed.
pub(crate) fn stale_edge_count(conn: &Connection) -> Result<usize, StoreError> {
    let count: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT count(*) FROM edge_files WHERE mtime = {STALE_EDGE_MTIME} AND path NOT LIKE 'edges:%'"
            ),
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(count.unwrap_or(0) as usize)
}

/// Leave a file to the scan's changed-file pass: an mtime no file has that
/// is not the stale mark, so the scan retries it and nothing here does.
fn hand_to_scan(conn: &Connection, rel: &str) -> Result<(), StoreError> {
    conn.execute("UPDATE edge_files SET mtime = 0 WHERE path = ?", [rel])?;
    Ok(())
}
