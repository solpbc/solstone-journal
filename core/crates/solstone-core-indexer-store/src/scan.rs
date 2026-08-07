// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, params};
use solstone_core_format::content::{
    ChatLabels, ContentResolution, Family, classify, produce_chunks,
};
use solstone_core_format::paths::{relative_to_journal, resolve_journal_path};
use solstone_core_format::segment::{day_of, segment_key, time_bucket};
use solstone_core_indexer::discovery::discover_indexable_files;
use solstone_core_indexer::edges::candidates::EdgeResolver;
use solstone_core_indexer::edges::discovery::discover_edge_files;
use solstone_core_indexer::edges::registry::edge_source_for_rel;
use solstone_core_indexer::edges::{EdgeValue, NormalizedEdge, extract_file_edges};
use solstone_core_indexer::entity_search::{
    ENTITY_SEARCH_WATERMARK_COUNT_PATH, ENTITY_SEARCH_WATERMARK_MTIME_PATH, EntitySearchRow,
    build_entity_search,
};
use solstone_core_indexer::metadata::extract_path_metadata;
use solstone_core_indexer::segment_aggregate::build_segment_aggregate;
use solstone_core_indexer::stream::extract_stream;
use solstone_core_journal_config::{ConfigLoadError, plain_defaults, read_journal_config};

use crate::StoreError;
use crate::db::{
    EDGES_SCHEMA_PATH, EDGES_SCHEMA_VERSION, mark_index_build_complete, open_index,
    read_entity_search_watermark, write_entity_search_watermark,
};

const MERGE_STEP: i64 = 32;
const MERGE_BUDGET: usize = 2;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScanReport {
    pub indexed: usize,
    pub removed: usize,
    pub skipped: usize,
    pub edges_indexed: usize,
    pub edges_removed: usize,
    pub edge_rows_inserted: usize,
    pub merge_steps_spent: usize,
    pub failed: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EdgeRebuildReport {
    pub files: usize,
    pub rows: usize,
    pub drops: usize,
    pub failed: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EdgeScanReport {
    indexed: usize,
    removed: usize,
    rows_inserted: usize,
    failed: usize,
    warnings: Vec<String>,
}

fn discovered_days(files: &BTreeMap<String, PathBuf>) -> HashSet<&str> {
    files.keys().filter_map(|rel| day_of(rel)).collect()
}

fn removal_candidates(
    stored: &BTreeMap<String, i64>,
    files: &BTreeMap<String, PathBuf>,
    full: bool,
) -> (Vec<String>, BTreeMap<String, usize>) {
    let discovered = files.keys().collect::<BTreeSet<_>>();
    let days = discovered_days(files);
    let mut removed = Vec::new();
    let mut retained_days = BTreeMap::new();
    for rel in stored.keys() {
        if discovered.contains(rel) {
            continue;
        }
        if full || day_of(rel).is_none_or(|day| days.contains(day)) {
            removed.push(rel.clone());
        } else if let Some(day) = day_of(rel) {
            *retained_days.entry(day.to_string()).or_default() += 1;
        }
    }
    (removed, retained_days)
}

fn retained_day_warning(day: &str, count: usize) -> String {
    format!(
        "light scan retained {count} stale row(s) for day {day}: discovery produced no files for that day; rerun with --rescan-full to remove them"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RescanFileStatus {
    Indexed { warnings: Vec<String> },
    Declined,
}

struct ResolvedChatLabels {
    labels: ChatLabels,
    warning: Option<String>,
}

pub fn scan_journal(journal: &Path, full: bool) -> Result<ScanReport, StoreError> {
    let mut conn = open_index(journal)?;
    let mut report = ScanReport::default();
    let chat_labels = resolve_chat_labels(journal);
    let default_chat_labels = ChatLabels::default();
    let mut chat_config_error_reported = false;
    let files = discover_indexable_files(journal)?;

    let db_mtimes = load_file_mtimes(&conn)?;
    let mut to_index = Vec::new();
    for (rel, path) in &files {
        match file_mtime_secs(path) {
            Ok(mtime) => {
                if db_mtimes.get(rel) != Some(&mtime) {
                    to_index.push((rel.clone(), path.clone(), mtime));
                }
            }
            Err(error) => {
                report.skipped += 1;
                report
                    .warnings
                    .push(format!("mtime read failed for {rel}: {error}"));
            }
        }
    }

    let mut rebuilt_segments = HashSet::new();
    for (rel, path, mtime) in &to_index {
        let family = match classify(rel) {
            ContentResolution::Indexed(family) => family,
            ContentResolution::Unrecognized => {
                report.skipped += 1;
                report
                    .warnings
                    .push(format!("unclassified discovered file skipped: {rel}"));
                continue;
            }
            ContentResolution::Unindexed(_) | ContentResolution::IndexedElsewhere => continue,
        };
        if family == Family::Chat
            && let Err(error) = &chat_labels
        {
            report.failed += 1;
            if !chat_config_error_reported {
                report.warnings.push(error.to_string());
                chat_config_error_reported = true;
            }
            continue;
        }
        let (chat_labels, chat_config_warning) = match &chat_labels {
            Ok(resolved) => (&resolved.labels, resolved.warning.as_deref()),
            Err(_error) => (&default_chat_labels, None),
        };
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM chunks WHERE path=?", [rel])?;
        let mut warnings = match index_file(
            &tx,
            journal,
            rel,
            path,
            family,
            chat_labels,
            chat_config_warning,
        ) {
            Ok(warnings) => warnings,
            Err(warning) => {
                report.skipped += 1;
                report.warnings.push(warning);
                continue;
            }
        };
        let segment_to_mark =
            match index_segment_aggregate_for_rel(&tx, journal, rel, &rebuilt_segments) {
                Ok((aggregate_warnings, segment_to_mark)) => {
                    warnings.extend(aggregate_warnings);
                    segment_to_mark
                }
                Err(StoreError::AggregateIncomplete {
                    warnings: aggregate_warnings,
                    ..
                }) => {
                    report.skipped += 1;
                    report.warnings.extend(aggregate_warnings);
                    continue;
                }
                Err(error) => return Err(error),
            };
        tx.execute(
            "REPLACE INTO files(path, mtime) VALUES (?, ?)",
            params![rel, mtime],
        )?;
        tx.commit()?;
        if let Some(rel_segment) = segment_to_mark {
            rebuilt_segments.insert(rel_segment);
        }
        report.warnings.extend(warnings);
        report.indexed += 1;
    }

    let (removed, retained_days) = removal_candidates(&db_mtimes, &files, full);
    report.warnings.extend(
        retained_days
            .into_iter()
            .map(|(day, count)| retained_day_warning(&day, count)),
    );
    let mut removed_count = 0;
    for rel in &removed {
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM chunks WHERE path=?", [rel])?;
        tx.execute("DELETE FROM files WHERE path=?", [rel])?;
        let segment_to_mark =
            match index_segment_aggregate_for_rel(&tx, journal, rel, &rebuilt_segments) {
                Ok((aggregate_warnings, segment_to_mark)) => {
                    report.warnings.extend(aggregate_warnings);
                    segment_to_mark
                }
                Err(StoreError::AggregateIncomplete {
                    warnings: aggregate_warnings,
                    ..
                }) => {
                    report.skipped += 1;
                    report.warnings.extend(aggregate_warnings);
                    continue;
                }
                Err(error) => return Err(error),
            };
        tx.commit()?;
        if let Some(rel_segment) = segment_to_mark {
            rebuilt_segments.insert(rel_segment);
        }
        removed_count += 1;
    }
    report.removed = removed_count;

    let tx = conn.transaction()?;
    index_entity_search(&tx, journal, full)?;
    tx.commit()?;

    let tx = conn.transaction()?;
    let edge_report = reconcile_edges(&tx, journal, full)?;
    tx.commit()?;
    report.edges_indexed = edge_report.indexed;
    report.edges_removed = edge_report.removed;
    report.edge_rows_inserted = edge_report.rows_inserted;
    report.failed += edge_report.failed;
    report.warnings.extend(edge_report.warnings);
    if full && report.failed == 0 && report.warnings.is_empty() {
        let tx = conn.transaction()?;
        let files_count = tx.query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
        let chunks_count = tx.query_row("SELECT count(*) FROM chunks", [], |row| row.get(0))?;
        mark_index_build_complete(&tx, files_count, chunks_count)?;
        tx.commit()?;
    }
    let (merge_steps_spent, merge_warning) = run_bounded_merge(&mut conn);
    report.merge_steps_spent = merge_steps_spent;
    if let Some(warning) = merge_warning {
        report.warnings.push(warning);
    }
    Ok(report)
}

fn run_bounded_merge(conn: &mut Connection) -> (usize, Option<String>) {
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(error) => {
            return (
                0,
                Some(format!(
                    "index is correct and merely uncompacted: could not start FTS5 merge transaction: {error}"
                )),
            );
        }
    };
    let mut steps_spent = 0;
    for _ in 0..MERGE_BUDGET {
        let before = tx.total_changes();
        if let Err(error) = tx.execute(
            "INSERT INTO chunks(chunks, rank) VALUES('merge', ?)",
            [MERGE_STEP],
        ) {
            return (
                steps_spent,
                Some(format!(
                    "index is correct and merely uncompacted: FTS5 merge failed: {error}"
                )),
            );
        }
        steps_spent += 1;
        if tx.total_changes() - before < 2 {
            break;
        }
    }
    match tx.commit() {
        Ok(()) => (steps_spent, None),
        Err(error) => (
            steps_spent,
            Some(format!(
                "index is correct and merely uncompacted: could not commit FTS5 merge: {error}"
            )),
        ),
    }
}

pub fn rescan_file(journal: &Path, input: &Path) -> Result<RescanFileStatus, StoreError> {
    let (rel, path) = resolve_rescan_target(journal, input)?;
    let resolution = classify(&rel);
    let edge_source = edge_source_for_rel(&rel)?;
    let family = match resolution {
        ContentResolution::Indexed(family) => Some(family),
        ContentResolution::Unindexed(_)
        | ContentResolution::IndexedElsewhere
        | ContentResolution::Unrecognized => None,
    };
    if family.is_none() && edge_source.is_none() {
        return Ok(RescanFileStatus::Declined);
    }
    if !path.is_file() {
        return Err(StoreError::MissingFile(path));
    }
    let chat_labels = if matches!(family, Some(Family::Chat)) {
        Some(resolve_chat_labels(journal)?)
    } else {
        None
    };
    let default_chat_labels = ChatLabels::default();
    let mut edge_resolver = EdgeResolver::new(journal);
    if edge_source.is_some() {
        edge_resolver.preflight_owner_timezone()?;
    }
    let mtime = file_mtime_secs(&path)?;
    let mut conn = open_index(journal)?;
    let tx = conn.transaction()?;
    let mut warnings = Vec::new();
    if let Some(family) = family {
        let (chat_labels, chat_config_warning) = match &chat_labels {
            Some(resolved) => (&resolved.labels, resolved.warning.as_deref()),
            None => (&default_chat_labels, None),
        };
        tx.execute("DELETE FROM chunks WHERE path=?", [&rel])?;
        match index_file(
            &tx,
            journal,
            &rel,
            &path,
            family,
            chat_labels,
            chat_config_warning,
        ) {
            Ok(content_warnings) => {
                warnings.extend(content_warnings);
                if let Some(rel_segment) = segment_rel_for_file(&rel) {
                    let mut affected_segments = BTreeSet::new();
                    affected_segments.insert(rel_segment);
                    warnings.extend(index_segment_aggregates(&tx, journal, &affected_segments)?);
                }
                tx.execute(
                    "REPLACE INTO files(path, mtime) VALUES (?, ?)",
                    params![rel, mtime],
                )?;
            }
            Err(warning) => {
                return Err(StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    warning,
                )));
            }
        }
    }
    if edge_source.is_some() {
        delete_edges_for_path(&tx, &rel)?;
        let result = process_edge_file(&tx, journal, &rel, &path, &mut edge_resolver)?;
        warnings.extend(result.warnings);
        if result.failed {
            return Err(StoreError::EdgeFileFailed(warnings.join("; ")));
        }
        replace_edge_file_mtime(&tx, &rel, mtime)?;
    }
    tx.commit()?;
    Ok(RescanFileStatus::Indexed { warnings })
}

pub fn rebuild_edges(journal: &Path) -> Result<EdgeRebuildReport, StoreError> {
    let mut conn = open_index(journal)?;
    let mut resolver = EdgeResolver::new(journal);
    let mut report = EdgeRebuildReport::default();
    let tx = conn.transaction()?;
    if let Err(error) = resolver.preflight_owner_timezone() {
        report.failed = 1;
        report.warnings.push(error.to_string());
        tx.rollback()?;
        return Ok(report);
    }
    tx.execute("DELETE FROM edges", [])?;
    tx.execute("DELETE FROM edge_files", [])?;
    tx.execute(
        "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
        params![EDGES_SCHEMA_PATH, EDGES_SCHEMA_VERSION],
    )?;

    let files = discover_edge_files(journal)?;
    for (rel, path) in files {
        let mtime = match file_mtime_secs(&path) {
            Ok(mtime) => mtime,
            Err(error) => {
                report.failed += 1;
                report
                    .warnings
                    .push(format!("mtime read failed for {rel}: {error}"));
                continue;
            }
        };
        let result = process_edge_file(&tx, journal, &rel, &path, &mut resolver)?;
        report.files += 1;
        report.rows += result.rows_inserted;
        report.drops += result.drops;
        report.failed += usize::from(result.failed);
        report.skipped += usize::from(result.invalid_segment);
        report.warnings.extend(result.warnings);
        if !result.failed {
            replace_edge_file_mtime(&tx, &rel, mtime)?;
        }
    }
    if report.failed > 0 {
        tx.rollback()?;
        return Ok(report);
    }
    tx.commit()?;
    Ok(report)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EdgeProcessResult {
    rows_inserted: usize,
    drops: usize,
    failed: bool,
    invalid_segment: bool,
    warnings: Vec<String>,
}

fn reconcile_edges(
    conn: &Connection,
    journal: &Path,
    full: bool,
) -> Result<EdgeScanReport, StoreError> {
    let files = discover_edge_files(journal)?;

    let db_mtimes = edge_file_mtimes(conn)?;
    let mut to_index = Vec::new();
    for (rel, path) in &files {
        let Ok(mtime) = file_mtime_secs(path) else {
            continue;
        };
        if db_mtimes.get(rel) != Some(&mtime) {
            to_index.push((rel.clone(), path.clone(), mtime));
        }
    }

    let mut resolver = EdgeResolver::new(journal);
    let mut report = EdgeScanReport::default();
    if let Err(error) = resolver.preflight_owner_timezone() {
        report.failed = 1;
        report.warnings.push(error.to_string());
        return Ok(report);
    }
    let (removed, retained_days) = removal_candidates(&db_mtimes, &files, full);
    report.warnings.extend(
        retained_days
            .into_iter()
            .map(|(day, count)| retained_day_warning(&day, count)),
    );
    for (rel, path, mtime) in &to_index {
        begin_edge_file_savepoint(conn)?;
        let result = match replace_edge_file_edges(conn, journal, rel, path, *mtime, &mut resolver)
        {
            Ok(result) => result,
            Err(error) => {
                rollback_edge_file_savepoint(conn)?;
                report.indexed += 1;
                report.failed += 1;
                report
                    .warnings
                    .push(format!("Skipping edge extraction for {rel}: {error}"));
                continue;
            }
        };
        report.indexed += 1;
        if result.failed {
            rollback_edge_file_savepoint(conn)?;
            report.failed += 1;
            report.warnings.extend(result.warnings);
            continue;
        }
        release_edge_file_savepoint(conn)?;
        report.rows_inserted += result.rows_inserted;
        report.warnings.extend(result.warnings);
    }
    for rel in &removed {
        delete_edges_for_path(conn, rel)?;
    }
    report.removed = removed.len();
    Ok(report)
}

fn replace_edge_file_edges(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    path: &Path,
    mtime: i64,
    resolver: &mut EdgeResolver,
) -> Result<EdgeProcessResult, StoreError> {
    delete_edges_for_path(conn, rel)?;
    let result = process_edge_file(conn, journal, rel, path, resolver)?;
    if !result.failed {
        replace_edge_file_mtime(conn, rel, mtime)?;
    }
    Ok(result)
}

fn begin_edge_file_savepoint(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("SAVEPOINT edge_file_replacement", [])?;
    Ok(())
}

fn release_edge_file_savepoint(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("RELEASE SAVEPOINT edge_file_replacement", [])?;
    Ok(())
}

fn rollback_edge_file_savepoint(conn: &Connection) -> Result<(), StoreError> {
    let rollback = conn.execute("ROLLBACK TO SAVEPOINT edge_file_replacement", []);
    let release = conn.execute("RELEASE SAVEPOINT edge_file_replacement", []);
    match (rollback, release) {
        (Ok(_), Ok(_)) => Ok(()),
        (Err(error), _) | (_, Err(error)) => Err(StoreError::Sql(error)),
    }
}

fn process_edge_file(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    path: &Path,
    resolver: &mut EdgeResolver,
) -> Result<EdgeProcessResult, StoreError> {
    resolver.begin_file();
    let extracted = extract_file_edges(journal, rel, path, resolver);
    let drops = resolver.drops();
    match extracted {
        Ok(extracted) => {
            if let Some(segment) = extracted.invalid_segment {
                return Ok(EdgeProcessResult {
                    drops,
                    invalid_segment: true,
                    warnings: vec![format!(
                        "Skipping edge extraction for {rel}: invalid segment key {segment}"
                    )],
                    ..EdgeProcessResult::default()
                });
            }
            match insert_normalized_edges(conn, &extracted.rows) {
                Ok(rows_inserted) => Ok(EdgeProcessResult {
                    rows_inserted,
                    drops,
                    warnings: extracted.warnings,
                    ..EdgeProcessResult::default()
                }),
                Err(error) => Ok(EdgeProcessResult {
                    drops,
                    failed: true,
                    warnings: vec![format!("Skipping edge extraction for {rel}: {error}")],
                    ..EdgeProcessResult::default()
                }),
            }
        }
        Err(error) => Ok(EdgeProcessResult {
            drops,
            failed: true,
            warnings: vec![format!("Skipping edge extraction for {rel}: {error}")],
            ..EdgeProcessResult::default()
        }),
    }
}

fn edge_file_mtimes(conn: &Connection) -> Result<BTreeMap<String, i64>, StoreError> {
    let mut statement =
        conn.prepare("SELECT path, mtime FROM edge_files WHERE path != ? ORDER BY path")?;
    let rows = statement.query_map([EDGES_SCHEMA_PATH], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut mtimes = BTreeMap::new();
    for row in rows {
        let (path, mtime) = row?;
        mtimes.insert(path, mtime);
    }
    Ok(mtimes)
}

fn delete_edges_for_path(conn: &Connection, path: &str) -> Result<usize, StoreError> {
    let deleted = conn.execute("DELETE FROM edges WHERE path=?", [path])?;
    if path != EDGES_SCHEMA_PATH {
        conn.execute("DELETE FROM edge_files WHERE path=?", [path])?;
    }
    Ok(deleted)
}

fn replace_edge_file_mtime(conn: &Connection, rel: &str, mtime: i64) -> Result<(), StoreError> {
    conn.execute(
        "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
        params![rel, mtime],
    )?;
    Ok(())
}

fn insert_normalized_edges(
    conn: &Connection,
    rows: &[NormalizedEdge],
) -> Result<usize, StoreError> {
    if rows.is_empty() {
        return Ok(0);
    }
    insert_normalized_edges_inner(conn, rows)?;
    Ok(rows.len())
}

fn insert_normalized_edges_inner(
    conn: &Connection,
    rows: &[NormalizedEdge],
) -> Result<(), StoreError> {
    for row in rows {
        let src_name = edge_value_to_sql(&row.src_name);
        let dst_name = edge_value_to_sql(&row.dst_name);
        let label = edge_value_to_sql(&row.label);
        let ts = edge_value_to_sql(&row.ts);
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, src_name, dst_name, day, facet, source, path, anchor, label, ts, weight) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                row.src.as_str(),
                row.dst.as_str(),
                row.kind.as_str(),
                row.directed,
                src_name,
                dst_name,
                row.day.as_deref(),
                row.facet.as_deref(),
                row.source.as_str(),
                row.path.as_str(),
                row.anchor.as_deref(),
                label,
                ts,
                row.weight,
            ],
        )?;
    }
    Ok(())
}

fn edge_value_to_sql(value: &EdgeValue) -> SqlValue {
    match value {
        EdgeValue::Null => SqlValue::Null,
        EdgeValue::Text(value) => SqlValue::Text(value.clone()),
        EdgeValue::Int(value) => SqlValue::Integer(*value),
        EdgeValue::Float(value) => SqlValue::Real(*value),
    }
}

fn load_file_mtimes(conn: &Connection) -> Result<BTreeMap<String, i64>, StoreError> {
    let mut statement =
        conn.prepare("SELECT path, mtime FROM files WHERE path NOT LIKE 'entity_search:%'")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut mtimes = BTreeMap::new();
    for row in rows {
        let (path, mtime) = row?;
        mtimes.insert(path, mtime);
    }
    Ok(mtimes)
}

/// Segment-directory rel for a file rel that lives inside a segment, mirroring
/// `_index_segment_chunks`'s gate in solstone/think/indexer/journal.py.
fn segment_rel_for_file(rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() >= 4 && segment_key(parts[2]).is_some() {
        Some(parts[..3].join("/"))
    } else {
        None
    }
}

fn index_segment_aggregate_for_rel(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    rebuilt_segments: &HashSet<String>,
) -> Result<(Vec<String>, Option<String>), StoreError> {
    let Some(rel_segment) = segment_rel_for_file(rel) else {
        return Ok((Vec::new(), None));
    };
    if rebuilt_segments.contains(&rel_segment) {
        return Ok((Vec::new(), None));
    }
    let mut affected_segments = BTreeSet::new();
    affected_segments.insert(rel_segment.clone());
    let warnings = index_segment_aggregates(conn, journal, &affected_segments)?;
    Ok((warnings, Some(rel_segment)))
}

fn index_segment_aggregates(
    conn: &Connection,
    journal: &Path,
    segments: &BTreeSet<String>,
) -> Result<Vec<String>, StoreError> {
    let mut warnings = Vec::new();
    for rel_segment in segments {
        let aggregate = build_segment_aggregate(journal, rel_segment);
        if !aggregate.complete {
            return Err(StoreError::AggregateIncomplete {
                segment: rel_segment.clone(),
                warnings: aggregate.warnings,
            });
        }
        conn.execute("DELETE FROM chunks WHERE path = ?", [rel_segment])?;
        for row in &aggregate.rows {
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    row.content,
                    row.path,
                    row.day,
                    row.facet,
                    row.agent,
                    row.stream.as_deref(),
                    row.idx,
                    row.time_bucket,
                ],
            )?;
        }
        warnings.extend(aggregate.warnings);
    }
    Ok(warnings)
}

fn index_entity_search(conn: &Connection, journal: &Path, force: bool) -> Result<(), StoreError> {
    let build = build_entity_search(journal)?;
    let watermark = read_entity_search_watermark(conn)?;
    let (stored_mtime, stored_count, migrating) = match watermark {
        Some(watermark) => (watermark.mtime, watermark.count, false),
        None => (
            stored_entity_search_mtime(conn)?,
            stored_entity_search_count(conn)?,
            true,
        ),
    };
    let built_rows = group_entity_search_rows(build.rows.iter().cloned());
    let stored_rows = stored_entity_search_rows(conn)?;
    let complete = built_rows.keys().eq(stored_rows.keys());

    let entity_changed = force
        || build.watermark_mtime_secs > stored_mtime
        || build.count != stored_count
        || (build.count > 0 && !complete);
    if entity_changed {
        let paths = built_rows
            .keys()
            .chain(stored_rows.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for path in paths {
            let built = built_rows.get(&path);
            let stored = stored_rows.get(&path);
            if built == stored {
                continue;
            }
            conn.execute(
                "DELETE FROM chunks WHERE agent='entity' AND path=?",
                [&path],
            )?;
            if let Some(rows) = built {
                insert_entity_search_rows(conn, rows)?;
            }
        }
        conn.execute(
            "DELETE FROM chunks WHERE path LIKE 'entities/%/entity.json'",
            [],
        )?;
        write_entity_search_watermark(conn, build.watermark_mtime_secs, build.count)?;
    } else if migrating {
        write_entity_search_watermark(conn, stored_mtime, stored_count)?;
    }
    if migrating {
        conn.execute(
            "DELETE FROM files WHERE path IN (?, ?)",
            [
                ENTITY_SEARCH_WATERMARK_MTIME_PATH,
                ENTITY_SEARCH_WATERMARK_COUNT_PATH,
            ],
        )?;
    }
    Ok(())
}

fn group_entity_search_rows(
    rows: impl IntoIterator<Item = EntitySearchRow>,
) -> BTreeMap<String, Vec<EntitySearchRow>> {
    let mut grouped = BTreeMap::new();
    for row in rows {
        grouped
            .entry(row.path.clone())
            .or_insert_with(Vec::new)
            .push(row);
    }
    grouped
}

fn stored_entity_search_rows(
    conn: &Connection,
) -> Result<BTreeMap<String, Vec<EntitySearchRow>>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT path, content, day, facet, agent, stream, idx, time_bucket FROM chunks WHERE agent='entity' ORDER BY path, idx",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(EntitySearchRow {
            path: row.get(0)?,
            content: row.get(1)?,
            day: row.get(2)?,
            facet: row.get(3)?,
            agent: row.get(4)?,
            stream: row.get(5)?,
            idx: row.get(6)?,
            time_bucket: row.get(7)?,
        })
    })?;
    let mut grouped = BTreeMap::new();
    for row in rows {
        let row = row?;
        grouped
            .entry(row.path.clone())
            .or_insert_with(Vec::new)
            .push(row);
    }
    Ok(grouped)
}

fn insert_entity_search_rows(
    conn: &Connection,
    rows: &[EntitySearchRow],
) -> Result<(), StoreError> {
    for row in rows {
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                row.content,
                row.path,
                row.day,
                row.facet,
                row.agent,
                row.stream,
                row.idx,
                row.time_bucket,
            ],
        )?;
    }
    Ok(())
}

fn stored_entity_search_mtime(conn: &Connection) -> Result<i64, StoreError> {
    let stored = conn
        .query_row(
            "SELECT mtime FROM files WHERE path=?",
            [ENTITY_SEARCH_WATERMARK_MTIME_PATH],
            |row| row.get::<_, f64>(0),
        )
        .optional()?;
    Ok(stored.map(|value| value.trunc() as i64).unwrap_or(0))
}

fn stored_entity_search_count(conn: &Connection) -> Result<i64, StoreError> {
    let stored = conn
        .query_row(
            "SELECT mtime FROM files WHERE path=?",
            [ENTITY_SEARCH_WATERMARK_COUNT_PATH],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(stored.unwrap_or(0))
}

fn index_file(
    conn: &Connection,
    journal: &Path,
    rel: &str,
    path: &Path,
    family: Family,
    chat_labels: &ChatLabels,
    chat_config_warning: Option<&str>,
) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("content read failed for {rel}: {error}"))?;
    let mut produced = produce_chunks(family, rel, &text, chat_labels);
    if family == Family::Chat
        && let Some(warning) = chat_config_warning
    {
        produced.warnings.push(warning.to_string());
    }
    let metadata = extract_path_metadata(rel);
    let facet = metadata.facet.to_lowercase();
    let agent = produced
        .agent_override
        .clone()
        .unwrap_or_else(|| metadata.agent.clone())
        .to_lowercase();
    let stream_lookup = extract_stream(journal, rel);
    let stream = stream_lookup.stream;
    let bucket = time_bucket(rel);
    let mut warnings = produced.warnings;
    warnings.extend(stream_lookup.warning);

    for (idx, chunk) in produced.chunks.iter().enumerate() {
        let content = chunk.content.trim();
        if content.is_empty() {
            continue;
        }
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                content,
                rel,
                metadata.day,
                facet,
                agent,
                stream.as_deref(),
                idx as i64,
                bucket,
            ],
        )
        .map_err(|error| format!("chunk insert failed for {rel}: {error}"))?;
    }
    Ok(warnings)
}

/// Resolve chat labels from a strict journal configuration read.
fn resolve_chat_labels(journal: &Path) -> Result<ResolvedChatLabels, ConfigLoadError> {
    let read = read_journal_config(journal)?;
    let missing = !read.present;
    let config = read.config.unwrap_or_else(plain_defaults);
    let identity = config
        .get("identity")
        .and_then(serde_json::Value::as_object);
    let owner = identity
        .and_then(|identity| identity.get("preferred"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            let identity = config
                .get("identity")
                .and_then(serde_json::Value::as_object);
            identity
                .and_then(|identity| identity.get("name"))
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("Owner")
        .trim();
    let agent = config
        .get("agent")
        .and_then(serde_json::Value::as_object)
        .and_then(|agent| agent.get("name"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Sol")
        .trim();
    Ok(ResolvedChatLabels {
        labels: if missing {
            ChatLabels::default()
        } else {
            ChatLabels::new(owner, agent)
        },
        warning: missing.then(|| {
            "chat labels unavailable from journal config; using fallback labels Owner/Sol"
                .to_string()
        }),
    })
}

fn resolve_rescan_target(journal: &Path, input: &Path) -> Result<(String, PathBuf), StoreError> {
    if input.is_absolute() {
        let journal_abs = fs::canonicalize(journal)?;
        let abs = fs::canonicalize(input)
            .map_err(|_error| StoreError::MissingFile(input.to_path_buf()))?;
        let rel = relative_to_journal(&journal_abs, &abs)
            .ok_or_else(|| StoreError::OutsideJournal(abs.clone()))?;
        Ok((rel, abs))
    } else {
        let rel = input
            .to_str()
            .ok_or_else(|| StoreError::NonUtf8Path(input.to_path_buf()))?;
        let path = resolve_journal_path(journal, rel)?;
        Ok((rel.to_string(), path))
    }
}

fn file_mtime_secs(path: &Path) -> Result<i64, StoreError> {
    let modified = fs::metadata(path)?.modified()?;
    let duration = modified.duration_since(UNIX_EPOCH).map_err(|error| {
        StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    Ok(duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        EntitySearchWatermark, IndexBuildLifecycle, IndexBuildState, db_path,
        read_entity_search_watermark, read_index_build_state, reset_index,
        write_entity_search_watermark,
    };
    use rusqlite::{Connection, params};
    use solstone_core_format::content::RawPerceptFamily;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        std::env::temp_dir().join(format!("solstone-core-indexer-store-scan-{name}-{stamp}"))
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("test path should have parent"))
            .expect("create parent");
        fs::write(path, text).expect("write test file");
    }

    fn write_stream(root: &Path, day: &str, stream: &str, segment: &str) {
        write_stream_marker(root, day, stream, segment, stream);
    }

    fn write_stream_marker(
        root: &Path,
        day: &str,
        stream_dir: &str,
        segment: &str,
        marker_stream: &str,
    ) {
        let dir = root
            .join("chronicle")
            .join(day)
            .join(stream_dir)
            .join(segment);
        fs::create_dir_all(&dir).expect("create stream dir");
        fs::write(
            dir.join("stream.json"),
            format!(r#"{{"stream":"{marker_stream}"}}"#),
        )
        .expect("write stream marker");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("count")
    }

    fn chunk_row(
        conn: &Connection,
        path: &str,
    ) -> (String, String, String, Option<String>, String, String) {
        conn.query_row(
            "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path=? ORDER BY idx LIMIT 1",
            [path],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("chunk metadata row")
    }

    fn segment_aggregate_content(conn: &Connection, path: &str) -> String {
        conn.prepare("SELECT content FROM chunks WHERE path=? AND agent='segment' ORDER BY idx")
            .expect("prepare segment aggregate content")
            .query_map([path], |row| row.get::<_, String>(0))
            .expect("query segment aggregate content")
            .map(|row| row.expect("segment aggregate content row"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn chunk_content(conn: &Connection, path: &str) -> String {
        conn.prepare("SELECT content FROM chunks WHERE path=? ORDER BY idx")
            .expect("prepare chunk content")
            .query_map([path], |row| row.get::<_, String>(0))
            .expect("query chunk content")
            .map(|row| row.expect("chunk content row"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn file_mtime(conn: &Connection, path: &str) -> i64 {
        conn.query_row("SELECT mtime FROM files WHERE path=?", [path], |row| {
            row.get(0)
        })
        .expect("file mtime row")
    }

    fn entity_watermark(conn: &Connection) -> EntitySearchWatermark {
        read_entity_search_watermark(conn)
            .expect("read entity watermark")
            .expect("entity watermark row")
    }

    fn entity_rowids(conn: &Connection) -> BTreeMap<String, Vec<i64>> {
        let mut statement = conn
            .prepare("SELECT path, rowid FROM chunks WHERE agent='entity' ORDER BY path, idx")
            .expect("prepare entity rowids");
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .expect("query entity rowids");
        let mut result = BTreeMap::new();
        for row in rows {
            let (path, rowid) = row.expect("entity rowid");
            result.entry(path).or_insert_with(Vec::new).push(rowid);
        }
        result
    }

    fn fts_segment_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(DISTINCT segid) FROM chunks_idx", [], |row| {
            row.get(0)
        })
        .expect("count FTS segments")
    }

    fn seed_fragmented_chunks(conn: &mut Connection) {
        conn.execute(
            "INSERT INTO chunks(chunks, rank) VALUES('automerge', 0)",
            [],
        )
        .expect("disable FTS automatic merge");
        for rewrite in 0..20 {
            let tx = conn
                .unchecked_transaction()
                .expect("start fragmented rewrite");
            for path_index in 0..50 {
                let path = format!("fragment:{path_index}");
                let content = (0..300)
                    .map(|term| format!("rewrite{rewrite}_path{path_index}_term{term}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                tx.execute("DELETE FROM chunks WHERE path=?", [&path])
                    .expect("delete fragmented path");
                tx.execute(
                    "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, '', '', 'fragment', '', 0, '')",
                    params![content, path],
                )
                .expect("insert fragmented path");
            }
            tx.commit().expect("commit fragmented rewrite");
        }
    }

    fn edge_file_mtime(conn: &Connection, path: &str) -> Option<i64> {
        conn.query_row("SELECT mtime FROM edge_files WHERE path=?", [path], |row| {
            row.get(0)
        })
        .optional()
        .expect("edge file mtime query")
    }

    fn create_abort_trigger(
        conn: &Connection,
        name: &str,
        timing: &str,
        event: &str,
        table: &str,
        when_clause: Option<&str>,
    ) {
        let when_sql = when_clause
            .map(|clause| format!(" WHEN {clause}"))
            .unwrap_or_default();
        conn.execute(
            &format!(
                "CREATE TRIGGER {name} {timing} {event} ON {table}{when_sql} BEGIN SELECT RAISE(ABORT, '{name}'); END"
            ),
            [],
        )
        .expect("create abort trigger");
    }

    fn drop_trigger(conn: &Connection, name: &str) {
        conn.execute(&format!("DROP TRIGGER IF EXISTS {name}"), [])
            .expect("drop trigger");
    }

    fn assert_sqlite_and_fts_integrity(conn: &Connection) {
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity check");
        assert_eq!(integrity, "ok");
        conn.execute("INSERT INTO chunks(chunks) VALUES('integrity-check')", [])
            .expect("fts integrity check");
    }

    fn seed_edge_entity(root: &Path, entity_id: &str, name: &str) {
        write(
            root,
            &format!("entities/{entity_id}/entity.json"),
            &format!(r#"{{"name":"{name}","type":"Person"}}"#),
        );
        write(
            root,
            &format!("facets/work/entities/{entity_id}/entity.json"),
            "{}",
        );
    }

    fn edge_rows(conn: &Connection) -> Vec<(String, String, i64, String, String)> {
        conn.prepare(
            "SELECT src, dst, weight, anchor, path FROM edges ORDER BY path, src, dst, weight",
        )
        .expect("prepare edge rows")
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .expect("query edge rows")
        .map(|row| row.expect("edge row"))
        .collect()
    }

    fn edge_file_paths(conn: &Connection) -> Vec<String> {
        conn.prepare("SELECT path FROM edge_files ORDER BY path")
            .expect("prepare edge files")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query edge files")
            .map(|row| row.expect("edge file row"))
            .collect()
    }

    type EdgeValueStorageRow = (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    );

    fn normalized_edge(
        path: &str,
        src_name: EdgeValue,
        dst_name: EdgeValue,
        label: EdgeValue,
        ts: EdgeValue,
    ) -> NormalizedEdge {
        NormalizedEdge {
            src: "alice".to_string(),
            dst: "bob".to_string(),
            kind: "works-with".to_string(),
            directed: 0,
            src_name,
            dst_name,
            day: Some("20260430".to_string()),
            facet: Some("work".to_string()),
            source: "observation".to_string(),
            path: path.to_string(),
            anchor: Some("anchor".to_string()),
            label,
            ts,
            weight: 1,
        }
    }

    #[test]
    fn clean_full_scan_marks_build_state_complete_with_live_counts() {
        let root = temp_root("complete-build-state");
        let rel = "20260717/talents/flow.md";
        write(
            &root,
            &format!("chronicle/{rel}"),
            "# Flow\n\ncomplete state",
        );
        reset_index(&root).expect("reset index");

        let report = scan_journal(&root, true).expect("clean full scan");
        assert_eq!(report.failed, 0);
        assert!(report.warnings.is_empty());
        let conn = open_index(&root).expect("open completed index");
        assert_eq!(
            read_index_build_state(&conn).expect("read build state"),
            Some(IndexBuildState {
                schema_version: 1,
                state: IndexBuildLifecycle::Complete,
                files_count: count(&conn, "SELECT count(*) FROM files"),
                chunks_count: count(&conn, "SELECT count(*) FROM chunks"),
            })
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup complete build state root");
    }

    #[test]
    fn failed_full_scan_leaves_build_state_unchanged() {
        let root = temp_root("failed-build-state");
        let rel = "20260717/talents/flow.md";
        write(
            &root,
            &format!("chronicle/{rel}"),
            "# Flow\n\ninterrupted state",
        );
        reset_index(&root).expect("reset index");
        let conn = open_index(&root).expect("open reset index");
        create_abort_trigger(
            &conn,
            "abort_build_state_completion",
            "BEFORE",
            "INSERT",
            "files",
            Some("NEW.path='20260717/talents/flow.md'"),
        );
        drop(conn);

        scan_journal(&root, true).expect_err("trigger aborts full scan");
        let conn = open_index(&root).expect("open interrupted index");
        assert_eq!(
            read_index_build_state(&conn).expect("read build state"),
            Some(IndexBuildState {
                schema_version: 1,
                state: IndexBuildLifecycle::Building,
                files_count: 0,
                chunks_count: 0,
            })
        );
        drop_trigger(&conn, "abort_build_state_completion");
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup failed build state root");
    }

    #[test]
    fn light_scan_leaves_complete_build_state_unchanged() {
        let root = temp_root("light-build-state");
        write(
            &root,
            "chronicle/20260717/talents/flow.md",
            "# Flow\n\ncomplete state",
        );
        reset_index(&root).expect("reset index");
        scan_journal(&root, true).expect("clean full scan");
        let conn = open_index(&root).expect("open completed index");
        let before = read_index_build_state(&conn)
            .expect("read completed state")
            .expect("completed state row");
        drop(conn);

        scan_journal(&root, false).expect("light scan");
        let conn = open_index(&root).expect("open light-scanned index");
        assert_eq!(
            read_index_build_state(&conn).expect("read state after light scan"),
            Some(before)
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup light build state root");
    }

    #[test]
    fn light_scan_does_not_create_a_build_state_row() {
        let root = temp_root("light-no-build-state");
        scan_journal(&root, false).expect("light scan");
        let conn = open_index(&root).expect("open light-scanned index");
        assert_eq!(
            read_index_build_state(&conn).expect("read build state"),
            None
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup light no-state root");
    }

    #[test]
    fn insert_normalized_edges_binds_edge_value_variants() {
        let root = temp_root("edge-value-bind");
        let conn = open_index(&root).expect("open index");
        let rows = vec![
            normalized_edge(
                "row-null-text-int-float",
                EdgeValue::Null,
                EdgeValue::Text("target".to_string()),
                EdgeValue::Int(12),
                EdgeValue::Float(1.5),
            ),
            normalized_edge(
                "row-int-float-text-text",
                EdgeValue::Int(7),
                EdgeValue::Float(2.5),
                EdgeValue::Text("label".to_string()),
                EdgeValue::Text("not-a-time".to_string()),
            ),
            normalized_edge(
                "row-text-null-null-int",
                EdgeValue::Text("source".to_string()),
                EdgeValue::Null,
                EdgeValue::Null,
                EdgeValue::Int(42),
            ),
        ];

        assert_eq!(
            insert_normalized_edges(&conn, &rows).expect("insert normalized edges"),
            3
        );
        let stored: Vec<EdgeValueStorageRow> = conn
            .prepare(
                "SELECT path, typeof(src_name), quote(src_name), typeof(dst_name), quote(dst_name), typeof(label), quote(label), typeof(ts), quote(ts) FROM edges ORDER BY path",
            )
            .expect("prepare edge value query")
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .expect("query edge values")
            .map(|row| row.expect("edge value row"))
            .collect();
        assert_eq!(
            stored,
            vec![
                (
                    "row-int-float-text-text".to_string(),
                    "text".to_string(),
                    "'7'".to_string(),
                    "text".to_string(),
                    "'2.5'".to_string(),
                    "text".to_string(),
                    "'label'".to_string(),
                    "text".to_string(),
                    "'not-a-time'".to_string(),
                ),
                (
                    "row-null-text-int-float".to_string(),
                    "null".to_string(),
                    "NULL".to_string(),
                    "text".to_string(),
                    "'target'".to_string(),
                    "text".to_string(),
                    "'12'".to_string(),
                    "real".to_string(),
                    "1.5".to_string(),
                ),
                (
                    "row-text-null-null-int".to_string(),
                    "text".to_string(),
                    "'source'".to_string(),
                    "null".to_string(),
                    "NULL".to_string(),
                    "null".to_string(),
                    "NULL".to_string(),
                    "integer".to_string(),
                    "42".to_string(),
                ),
            ]
        );
        fs::remove_dir_all(root).expect("cleanup edge value bind root");
    }

    #[test]
    fn scan_observation_container_passthrough_fails_before_partial_insert() {
        let root = temp_root("edge-observation-container-failure");
        let rel = "facets/work/entities/source/observations.jsonl";
        let expected_warning = "Skipping edge extraction for facets/work/entities/source/observations.jsonl: edge field label does not support object";
        write(
            &root,
            rel,
            r#"{"observed_at":1777556000000,"source_day":"20260430","relation":{"kind":"works-with","target_entity_id":"target","target_name":"Target","note":"valid"}}
{"observed_at":1777556100000,"source_day":"20260430","relation":{"kind":"works-with","target_entity_id":"other","target_name":"Other","note":{"bad":true}}}
"#,
        );

        let report = scan_journal(&root, true).expect("scan observation failure");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 0);
        assert_eq!(report.failed, 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| { warning == expected_warning })
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/source/observations.jsonl'"
            ),
            0
        );
        assert_sqlite_and_fts_integrity(&conn);
        drop(conn);

        let retry = scan_journal(&root, true).expect("retry observation failure");
        assert_eq!(retry.edges_indexed, 1);
        assert_eq!(retry.edge_rows_inserted, 0);
        assert_eq!(retry.failed, 1);
        assert!(
            retry
                .warnings
                .iter()
                .any(|warning| warning == expected_warning)
        );
        let conn = Connection::open(db_path(&root)).expect("open db after retry");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/source/observations.jsonl'",
            ),
            0
        );
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup observation failure root");
    }

    #[test]
    fn scan_indexes_copresence_edges_and_second_scan_is_zero_delta() {
        let root = temp_root("edge-copresence");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        seed_edge_entity(&root, "cora", "Cora Edge");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["20260304/default/090000_300","20260304/default/090500_300"]}
{"name":"Bob Edge","segments":["20260304/default/090000_300","20260304/default/090500_300"]}
{"name":"Cora Edge","segments":["20260304/default/090500_300"]}
"#,
        );

        let report = scan_journal(&root, true).expect("scan edge copresence");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 3);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            edge_rows(&conn),
            vec![
                (
                    "alice".to_string(),
                    "bob".to_string(),
                    2,
                    "20260304/default/090000_300".to_string(),
                    "facets/work/entities/20260304.jsonl".to_string(),
                ),
                (
                    "alice".to_string(),
                    "cora".to_string(),
                    1,
                    "20260304/default/090500_300".to_string(),
                    "facets/work/entities/20260304.jsonl".to_string(),
                ),
                (
                    "bob".to_string(),
                    "cora".to_string(),
                    1,
                    "20260304/default/090500_300".to_string(),
                    "facets/work/entities/20260304.jsonl".to_string(),
                ),
            ]
        );
        drop(conn);

        let second = scan_journal(&root, true).expect("second edge scan");
        assert_eq!(second.edges_indexed, 0);
        assert_eq!(second.edge_rows_inserted, 0);
        let conn = Connection::open(db_path(&root)).expect("open db after second scan");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 3);
        fs::remove_dir_all(root).expect("cleanup edge copresence root");
    }

    #[test]
    fn scan_edge_failure_preserves_prior_rows_mtime_and_keeps_sibling() {
        let root = temp_root("edge-failure");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        write(
            &root,
            "facets/work/entities/20260230.jsonl",
            r#"{"name":"Alice Edge","segments":["s2"]}
{"name":"Bob Edge","segments":["s2"]}
"#,
        );
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale",
                "edge",
                "co-present",
                0_i64,
                "co-presence",
                "facets/work/entities/20260230.jsonl",
                1_i64,
            ],
        )
        .expect("seed stale edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params!["facets/work/entities/20260230.jsonl", 0_i64],
        )
        .expect("seed stale edge mtime");
        drop(conn);

        let report = scan_journal(&root, true).expect("scan edge failure");
        assert_eq!(report.edges_indexed, 2);
        assert_eq!(report.edge_rows_inserted, 1);
        assert_eq!(report.failed, 1);
        assert!(report.warnings.iter().any(|warning| {
            warning.starts_with("Skipping edge extraction for facets/work/entities/20260230.jsonl")
        }));
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260230.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/20260230.jsonl'"
            ),
            1
        );
        assert_eq!(
            edge_file_mtime(&conn, "facets/work/entities/20260230.jsonl"),
            Some(0)
        );
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup edge failure root");
    }

    #[test]
    fn scan_candidate_load_failure_preserves_stale_rows_and_retries() {
        let root = temp_root("edge-candidate-load-failure");
        write(&root, "entities", "not a directory");
        write(&root, "facets/work/entities/alice/entity.json", "{}");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale",
                "edge",
                "co-present",
                0_i64,
                "co-presence",
                "facets/work/entities/20260304.jsonl",
                1_i64,
            ],
        )
        .expect("seed stale edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params!["facets/work/entities/20260304.jsonl", 0_i64],
        )
        .expect("seed stale edge mtime");
        drop(conn);

        let report = scan_journal(&root, true).expect("scan candidate load failure");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 0);
        assert_eq!(report.failed, 1);
        assert!(report.warnings.iter().any(|warning| {
            warning.starts_with("Skipping edge extraction for facets/work/entities/20260304.jsonl")
                && warning.contains("candidate load failed")
        }));
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_eq!(
            edge_file_mtime(&conn, "facets/work/entities/20260304.jsonl"),
            Some(0)
        );
        assert_sqlite_and_fts_integrity(&conn);
        drop(conn);

        fs::remove_file(root.join("entities")).expect("remove blocking entities file");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let retry = scan_journal(&root, true).expect("retry candidate load");
        assert_eq!(retry.edges_indexed, 1);
        assert_eq!(retry.edge_rows_inserted, 1);
        assert_eq!(retry.failed, 0);
        let conn = Connection::open(db_path(&root)).expect("open db after retry");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert!(edge_file_mtime(&conn, "facets/work/entities/20260304.jsonl").unwrap() > 0);
        fs::remove_dir_all(root).expect("cleanup candidate load failure root");
    }

    #[test]
    fn scan_invalid_segment_edge_file_skips_and_advances_mtime() {
        let root = temp_root("edge-invalid-segment");
        write(
            &root,
            "facets/999999_300/entities/20260304.jsonl",
            r#"{"name":"Nobody","segments":["s1"]}"#,
        );
        let report = scan_journal(&root, true).expect("scan invalid segment");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 0);
        assert!(report.warnings.iter().any(|warning| warning.starts_with(
            "Skipping edge extraction for facets/999999_300/entities/20260304.jsonl"
        )));
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/999999_300/entities/20260304.jsonl'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup invalid segment edge root");
    }

    #[test]
    fn rebuild_edges_reports_invalid_segments_as_skipped_and_json_errors_as_failed() {
        let root = temp_root("edge-rebuild-counters");
        write(
            &root,
            "chronicle/20260430/default/999999_300/screen.jsonl",
            r#"{"content":{}}"#,
        );
        write(
            &root,
            "chronicle/20260430/default/090000_300/talents/documents.json",
            "{not json",
        );

        let report = rebuild_edges(&root).expect("rebuild invalid and failed edges");
        assert_eq!(report.files, 2);
        assert_eq!(report.rows, 0);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.failed, 1);
        assert!(report.warnings.iter().any(|warning| {
            warning.starts_with(
                "Skipping edge extraction for 20260430/default/999999_300/screen.jsonl",
            ) && warning.contains("invalid segment key 999999_300")
        }));
        assert!(report.warnings.iter().any(|warning| {
            warning.starts_with(
                "Skipping edge extraction for 20260430/default/090000_300/talents/documents.json",
            ) && warning.contains("edge source JSON parse failed")
        }));
        fs::remove_dir_all(root).expect("cleanup edge rebuild counters root");
    }

    #[test]
    fn scan_removed_edge_file_deletes_rows_and_mtime() {
        let root = temp_root("edge-removed-file");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let rel = "facets/work/entities/20260304.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        let report = scan_journal(&root, true).expect("initial edge scan");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        drop(conn);

        fs::remove_file(root.join(rel)).expect("remove edge source");
        let report = scan_journal(&root, true).expect("scan removed edge source");
        assert_eq!(report.edges_removed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after remove");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup removed edge file root");
    }

    #[test]
    fn scan_removed_edge_trigger_failure_preserves_rows_and_mtime() {
        let root = temp_root("edge-removal-trigger-rollback");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let rel = "facets/work/entities/20260304.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        scan_journal(&root, true).expect("initial edge scan");
        let conn = open_index(&root).expect("open index");
        let prior_mtime = edge_file_mtime(&conn, rel).expect("prior edge mtime");
        create_abort_trigger(
            &conn,
            "abort_edge_file_delete",
            "BEFORE",
            "DELETE",
            "edge_files",
            Some("OLD.path='facets/work/entities/20260304.jsonl'"),
        );
        drop(conn);
        fs::remove_file(root.join(rel)).expect("remove edge source");

        let error = scan_journal(&root, true).expect_err("trigger aborts edge removal");
        assert!(error.to_string().contains("abort_edge_file_delete"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed edge removal");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_eq!(edge_file_mtime(&conn, rel), Some(prior_mtime));
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup edge removal trigger root");
    }

    #[test]
    fn scan_changed_edge_file_replaces_rows() {
        let root = temp_root("edge-changed-file");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        seed_edge_entity(&root, "cora", "Cora Edge");
        let rel = "facets/work/entities/20260304.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        scan_journal(&root, true).expect("initial edge scan");
        let conn = open_index(&root).expect("open index");
        assert_eq!(
            edge_rows(&conn),
            vec![(
                "alice".to_string(),
                "bob".to_string(),
                1,
                "s1".to_string(),
                "facets/work/entities/20260304.jsonl".to_string(),
            )]
        );
        conn.execute("UPDATE edge_files SET mtime=0 WHERE path=?", [rel])
            .expect("force edge reextract");
        drop(conn);
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s2"]}
{"name":"Cora Edge","segments":["s2"]}
"#,
        );

        let report = scan_journal(&root, true).expect("changed edge scan");
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after changed scan");
        assert_eq!(
            edge_rows(&conn),
            vec![(
                "alice".to_string(),
                "cora".to_string(),
                1,
                "s2".to_string(),
                "facets/work/entities/20260304.jsonl".to_string(),
            )]
        );
        fs::remove_dir_all(root).expect("cleanup changed edge file root");
    }

    #[test]
    fn scan_edge_trigger_failure_rolls_back_one_file_and_keeps_sibling() {
        let root = temp_root("edge-trigger-rollback");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        seed_edge_entity(&root, "cora", "Cora Edge");
        let failed_rel = "facets/work/entities/20260304.jsonl";
        let sibling_rel = "facets/work/entities/20260305.jsonl";
        write(
            &root,
            failed_rel,
            r#"{"name":"Alice Edge","segments":["old-failed"]}
{"name":"Bob Edge","segments":["old-failed"]}
"#,
        );
        write(
            &root,
            sibling_rel,
            r#"{"name":"Alice Edge","segments":["old-sibling"]}
{"name":"Cora Edge","segments":["old-sibling"]}
"#,
        );
        scan_journal(&root, true).expect("initial edge scan");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "UPDATE edge_files SET mtime=0 WHERE path IN (?, ?)",
            params![failed_rel, sibling_rel],
        )
        .expect("force edge reindex");
        create_abort_trigger(
            &conn,
            "abort_failed_edge_mtime",
            "BEFORE",
            "INSERT",
            "edge_files",
            Some("NEW.path='facets/work/entities/20260304.jsonl'"),
        );
        drop(conn);
        write(
            &root,
            failed_rel,
            r#"{"name":"Alice Edge","segments":["new-failed"]}
{"name":"Cora Edge","segments":["new-failed"]}
"#,
        );
        write(
            &root,
            sibling_rel,
            r#"{"name":"Bob Edge","segments":["new-sibling"]}
{"name":"Cora Edge","segments":["new-sibling"]}
"#,
        );

        let report = scan_journal(&root, true).expect("edge trigger scan");
        assert_eq!(report.edges_indexed, 2);
        assert_eq!(report.edge_rows_inserted, 1);
        assert_eq!(report.failed, 1);
        assert!(report.warnings.iter().any(|warning| {
            warning.contains("facets/work/entities/20260304.jsonl")
                && warning.contains("abort_failed_edge_mtime")
        }));
        let conn = Connection::open(db_path(&root)).expect("open db after failed edge scan");
        let rows = edge_rows(&conn);
        assert!(rows.iter().any(|row| {
            row.0 == "alice" && row.1 == "bob" && row.3 == "old-failed" && row.4 == failed_rel
        }));
        assert!(rows.iter().any(|row| {
            row.0 == "bob" && row.1 == "cora" && row.3 == "new-sibling" && row.4 == sibling_rel
        }));
        assert_eq!(edge_file_mtime(&conn, failed_rel), Some(0));
        assert!(edge_file_mtime(&conn, sibling_rel).unwrap() > 0);
        assert_sqlite_and_fts_integrity(&conn);
        drop_trigger(&conn, "abort_failed_edge_mtime");
        drop(conn);

        let retry = scan_journal(&root, true).expect("edge trigger retry");
        assert_eq!(retry.edges_indexed, 1);
        assert_eq!(retry.edge_rows_inserted, 1);
        assert_eq!(retry.failed, 0);
        let conn = Connection::open(db_path(&root)).expect("open db after edge retry");
        let rows = edge_rows(&conn);
        assert!(rows.iter().any(|row| {
            row.0 == "alice" && row.1 == "cora" && row.3 == "new-failed" && row.4 == failed_rel
        }));
        assert!(rows.iter().any(|row| {
            row.0 == "bob" && row.1 == "cora" && row.3 == "new-sibling" && row.4 == sibling_rel
        }));
        fs::remove_dir_all(root).expect("cleanup edge trigger root");
    }

    #[test]
    fn scan_edge_interruption_rolls_back_target_savepoint_and_retries() {
        let root = temp_root("edge-interruption");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        seed_edge_entity(&root, "cora", "Cora Edge");
        let first = "facets/work/entities/20260304.jsonl";
        let target = "facets/work/entities/20260305.jsonl";
        let last = "facets/work/entities/20260306.jsonl";
        write(
            &root,
            first,
            r#"{"name":"Alice Edge","segments":["first"]}
{"name":"Bob Edge","segments":["first"]}
"#,
        );
        write(
            &root,
            target,
            r#"{"name":"Alice Edge","segments":["target"]}
{"name":"Cora Edge","segments":["target"]}
"#,
        );
        write(
            &root,
            last,
            r#"{"name":"Bob Edge","segments":["last"]}
{"name":"Cora Edge","segments":["last"]}
"#,
        );
        let conn = open_index(&root).expect("open index");
        create_abort_trigger(
            &conn,
            "abort_middle_edge_file",
            "BEFORE",
            "INSERT",
            "edge_files",
            Some("NEW.path='facets/work/entities/20260305.jsonl'"),
        );
        drop(conn);

        let report = scan_journal(&root, true).expect("edge scan survives savepoint rollback");
        assert_eq!(report.edges_indexed, 3);
        assert_eq!(report.edge_rows_inserted, 2);
        assert_eq!(report.failed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after interruption");
        assert!(edge_file_mtime(&conn, first).is_some());
        assert_eq!(edge_file_mtime(&conn, target), None);
        assert!(edge_file_mtime(&conn, last).is_some());
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260305.jsonl'"
            ),
            0
        );
        drop_trigger(&conn, "abort_middle_edge_file");
        drop(conn);

        let retry = scan_journal(&root, true).expect("retry target edge file");
        assert_eq!(retry.edges_indexed, 1);
        assert_eq!(retry.edge_rows_inserted, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after retry");
        assert!(edge_file_mtime(&conn, target).is_some());
        fs::remove_dir_all(root).expect("cleanup edge interruption root");
    }

    #[test]
    fn rebuild_edges_is_idempotent_and_preserves_content_tables_and_sentinel() {
        let root = temp_root("edge-rebuild");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        write(
            &root,
            "chronicle/20260717/talents/flow.md",
            "# Flow\n\ncontent",
        );
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        scan_journal(&root, true).expect("scan before rebuild");
        let conn = Connection::open(db_path(&root)).expect("open db before rebuild");
        let chunk_count = count(&conn, "SELECT count(*) FROM chunks");
        let files_count = count(&conn, "SELECT count(*) FROM files");
        let before_rows = edge_rows(&conn);
        drop(conn);

        let first = rebuild_edges(&root).expect("first rebuild edges");
        assert_eq!(first.files, 1);
        assert_eq!(first.rows, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after rebuild");
        assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), chunk_count);
        assert_eq!(count(&conn, "SELECT count(*) FROM files"), files_count);
        assert_eq!(edge_rows(&conn), before_rows);
        let sentinel: i64 = conn
            .query_row(
                "SELECT mtime FROM edge_files WHERE path='edges:__schema__'",
                [],
                |row| row.get(0),
            )
            .expect("edge schema sentinel");
        assert_eq!(sentinel, 1);
        drop(conn);

        let second = rebuild_edges(&root).expect("second rebuild edges");
        assert_eq!(second.rows, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after second rebuild");
        assert_eq!(edge_rows(&conn), before_rows);
        fs::remove_dir_all(root).expect("cleanup edge rebuild root");
    }

    #[test]
    fn rebuild_edges_trigger_failure_rolls_back_full_rebuild() {
        let root = temp_root("edge-rebuild-trigger-rollback");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let rel = "facets/work/entities/20260304.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["fresh"]}
{"name":"Bob Edge","segments":["fresh"]}
"#,
        );
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, anchor, weight) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale",
                "edge",
                "co-present",
                0_i64,
                "co-presence",
                rel,
                "stale-anchor",
                1_i64,
            ],
        )
        .expect("seed stale edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params![rel, 0_i64],
        )
        .expect("seed stale edge mtime");
        create_abort_trigger(
            &conn,
            "abort_rebuild_edge_insert",
            "BEFORE",
            "INSERT",
            "edges",
            None,
        );
        drop(conn);

        let report = rebuild_edges(&root).expect("rebuild reports trigger failure");
        assert_eq!(report.failed, 1);
        assert!(report.warnings.iter().any(|warning| {
            warning.contains("facets/work/entities/20260304.jsonl")
                && warning.contains("abort_rebuild_edge_insert")
        }));
        let conn = Connection::open(db_path(&root)).expect("open db after failed rebuild");
        assert_eq!(
            edge_rows(&conn),
            vec![(
                "stale".to_string(),
                "edge".to_string(),
                1,
                "stale-anchor".to_string(),
                rel.to_string(),
            )]
        );
        assert_eq!(edge_file_mtime(&conn, rel), Some(0));
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup edge rebuild trigger root");
    }

    #[test]
    fn rebuild_edges_corrupt_config_reports_once_and_preserves_existing_rows() {
        let root = temp_root("edge-rebuild-corrupt-config");
        let rel = "facets/work/entities/20260304.jsonl";
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, anchor, weight) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale",
                "edge",
                "co-present",
                0_i64,
                "co-presence",
                rel,
                "stale-anchor",
                1_i64,
            ],
        )
        .expect("seed stale edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params![rel, 0_i64],
        )
        .expect("seed stale edge mtime");
        let before_rows = edge_rows(&conn);
        let before_mtime = edge_file_mtime(&conn, rel);
        drop(conn);
        write(
            &root,
            "config/journal.json",
            r#"{"identity":{"timezone":NaN}}"#,
        );

        let report = rebuild_edges(&root).expect("corrupt config is reported in rebuild report");
        assert_eq!(report.rows, 0);
        assert!(report.failed >= 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].starts_with("I couldn't read your settings file"));
        assert!(!report.warnings[0].starts_with("Skipping edge extraction for"));
        let conn = Connection::open(db_path(&root)).expect("open index after corrupt rebuild");
        assert_eq!(edge_rows(&conn), before_rows);
        assert_eq!(edge_file_mtime(&conn, rel), before_mtime);
        fs::remove_dir_all(root).expect("cleanup corrupt rebuild root");
    }

    #[test]
    fn scan_corrupt_timezone_reports_once_without_per_file_warning() {
        let root = temp_root("edge-scan-corrupt-config");
        for rel in [
            "chronicle/20260430/default/090000_300/screen.jsonl",
            "chronicle/20260430/default/090000_300/talents/documents.json",
            "chronicle/20260430/default/090000_300/talents/speakers.json",
        ] {
            write(&root, rel, "{}");
        }
        write(
            &root,
            "config/journal.json",
            r#"{"identity":{"timezone":NaN}}"#,
        );

        let report = scan_journal(&root, true).expect("corrupt timezone scan is reported");
        assert_eq!(report.failed, 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].starts_with("I couldn't read your settings file"));
        assert!(!report.warnings[0].contains("Skipping edge extraction for"));
        fs::remove_dir_all(root).expect("cleanup corrupt scan root");
    }

    #[test]
    fn rescan_file_indexes_edge_rows_for_facet_entity_file() {
        let root = temp_root("edge-rescan-file");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );

        assert_eq!(
            rescan_file(&root, Path::new("facets/work/entities/20260304.jsonl"))
                .expect("rescan edge file"),
            RescanFileStatus::Indexed {
                warnings: Vec::new()
            }
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup edge rescan file root");
    }

    #[test]
    fn rescan_edge_with_corrupt_config_preserves_prior_rows() {
        let root = temp_root("edge-rescan-corrupt-config");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let rel = "facets/work/entities/20260304.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        scan_journal(&root, true).expect("initial edge scan");
        let conn = Connection::open(db_path(&root)).expect("open index before corrupt rescan");
        let before_rows = edge_rows(&conn);
        let before_mtime = edge_file_mtime(&conn, rel);
        drop(conn);
        write(
            &root,
            "config/journal.json",
            r#"{"identity":{"timezone":NaN}}"#,
        );

        assert!(matches!(
            rescan_file(&root, Path::new(rel)),
            Err(StoreError::Edge(
                solstone_core_indexer::edges::EdgeError::JournalConfigCorrupt { .. }
            ))
        ));
        let conn = Connection::open(db_path(&root)).expect("open index after corrupt rescan");
        assert_eq!(edge_rows(&conn), before_rows);
        assert_eq!(edge_file_mtime(&conn, rel), before_mtime);
        fs::remove_dir_all(root).expect("cleanup corrupt edge rescan root");
    }

    #[test]
    fn rescan_file_edge_failure_returns_error_and_preserves_prior_rows() {
        let root = temp_root("edge-rescan-file-failure");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        let rel = "facets/work/entities/20260230.jsonl";
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, anchor, weight) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale",
                "edge",
                "co-present",
                0_i64,
                "co-presence",
                rel,
                "stale-anchor",
                1_i64,
            ],
        )
        .expect("seed stale edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params![rel, 0_i64],
        )
        .expect("seed stale edge mtime");
        drop(conn);
        write(
            &root,
            rel,
            r#"{"name":"Alice Edge","segments":["s2"]}
{"name":"Bob Edge","segments":["s2"]}
"#,
        );

        let error = rescan_file(&root, Path::new(rel)).expect_err("rescan edge failure");
        assert!(error.to_string().contains("Skipping edge extraction"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed edge rescan");
        assert_eq!(
            edge_rows(&conn),
            vec![(
                "stale".to_string(),
                "edge".to_string(),
                1,
                "stale-anchor".to_string(),
                rel.to_string(),
            )]
        );
        assert_eq!(edge_file_mtime(&conn, rel), Some(0));
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup edge rescan failure root");
    }

    #[test]
    fn scan_skips_reindexes_and_deletes_missing() {
        let root = temp_root("mtime");
        write(&root, "chronicle/20260717/talents/flow.md", "# Flow\n\none");
        let report = scan_journal(&root, true).expect("first scan");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after first scan");
        let stream: Option<String> = conn
            .query_row(
                "SELECT stream FROM chunks WHERE path='20260717/talents/flow.md'",
                [],
                |row| row.get(0),
            )
            .expect("stream value");
        assert_eq!(stream, None);
        drop(conn);
        let report = scan_journal(&root, true).expect("second scan");
        assert_eq!(report.indexed, 0);

        let conn = open_index(&root).expect("open index");
        conn.execute(
            "UPDATE files SET mtime=0 WHERE path='20260717/talents/flow.md'",
            [],
        )
        .expect("force reindex");
        drop(conn);
        write(&root, "chronicle/20260717/talents/flow.md", "# Flow\n\ntwo");
        let report = scan_journal(&root, true).expect("third scan");
        assert_eq!(report.indexed, 1);

        fs::remove_file(root.join("chronicle/20260717/talents/flow.md")).expect("remove file");
        let report = scan_journal(&root, true).expect("remove scan");
        assert_eq!(report.removed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path NOT LIKE 'entity_search:%'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup mtime root");
    }

    #[test]
    fn scan_propagates_markdown_sanitize_warnings() {
        let root = temp_root("markdown-sanitize-warning");
        let rel = "20260717/talents/flow.md";
        write(
            &root,
            &format!("chronicle/{rel}"),
            &format!("# Flow\n\n{}\n\nkept alpha", "z".repeat(2049)),
        );

        let report = scan_journal(&root, true).expect("scan markdown warning");
        assert_eq!(report.indexed, 1);
        assert_eq!(
            report.warnings,
            vec!["Dropped 1 line(s) exceeding 2048 chars during markdown sanitization"]
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(chunk_content(&conn, rel), "# Flow\n\nkept alpha");
        fs::remove_dir_all(root).expect("cleanup markdown warning root");
    }

    #[test]
    fn scan_content_trigger_failure_rolls_back_chunks_and_mtime_then_retries() {
        let root = temp_root("content-trigger-rollback");
        let rel = "20260717/talents/flow.md";
        write(&root, &format!("chronicle/{rel}"), "# Flow\n\nold content");
        scan_journal(&root, true).expect("initial scan");
        let conn = open_index(&root).expect("open index");
        conn.execute("UPDATE files SET mtime=0 WHERE path=?", [rel])
            .expect("force reindex");
        create_abort_trigger(
            &conn,
            "abort_content_file_mtime",
            "BEFORE",
            "INSERT",
            "files",
            Some("NEW.path='20260717/talents/flow.md'"),
        );
        drop(conn);
        write(&root, &format!("chronicle/{rel}"), "# Flow\n\nnew content");

        let error = scan_journal(&root, true).expect_err("trigger aborts scan");
        assert!(error.to_string().contains("abort_content_file_mtime"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed scan");
        assert_eq!(chunk_content(&conn, rel), "# Flow\n\nold content");
        assert_eq!(file_mtime(&conn, rel), 0);
        assert_sqlite_and_fts_integrity(&conn);
        drop_trigger(&conn, "abort_content_file_mtime");
        drop(conn);

        let retry = scan_journal(&root, true).expect("retry content scan");
        assert_eq!(retry.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after retry");
        assert_eq!(chunk_content(&conn, rel), "# Flow\n\nnew content");
        assert!(file_mtime(&conn, rel) > 0);
        fs::remove_dir_all(root).expect("cleanup content trigger root");
    }

    #[test]
    fn scan_content_interruption_keeps_prior_file_and_retries_target() {
        let root = temp_root("content-interruption");
        let first = "20260717/talents/a.md";
        let target = "20260717/talents/b.md";
        write(&root, &format!("chronicle/{first}"), "# A\n\nfirst");
        write(&root, &format!("chronicle/{target}"), "# B\n\ntarget");
        let conn = open_index(&root).expect("open index");
        create_abort_trigger(
            &conn,
            "abort_second_content_file",
            "BEFORE",
            "INSERT",
            "files",
            Some("NEW.path='20260717/talents/b.md'"),
        );
        drop(conn);

        let error = scan_journal(&root, false).expect_err("trigger aborts target file");
        assert!(error.to_string().contains("abort_second_content_file"));
        let conn = Connection::open(db_path(&root)).expect("open db after interruption");
        assert!(file_mtime(&conn, first) > 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260717/talents/b.md'"
            ),
            0
        );
        drop_trigger(&conn, "abort_second_content_file");
        drop(conn);

        let retry = scan_journal(&root, false).expect("retry target file");
        assert_eq!(retry.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after retry");
        assert!(file_mtime(&conn, target) > 0);
        fs::remove_dir_all(root).expect("cleanup content interruption root");
    }

    #[test]
    fn rescan_file_trigger_failure_rolls_back_chunks_and_mtime() {
        let root = temp_root("rescan-trigger-rollback");
        let rel = "20260717/talents/flow.md";
        write(&root, &format!("chronicle/{rel}"), "# Flow\n\nold content");
        scan_journal(&root, true).expect("initial scan");
        let conn = open_index(&root).expect("open index");
        conn.execute("UPDATE files SET mtime=0 WHERE path=?", [rel])
            .expect("force reindex");
        create_abort_trigger(
            &conn,
            "abort_rescan_file_mtime",
            "BEFORE",
            "INSERT",
            "files",
            Some("NEW.path='20260717/talents/flow.md'"),
        );
        drop(conn);
        write(&root, &format!("chronicle/{rel}"), "# Flow\n\nnew content");

        let error = rescan_file(&root, Path::new(rel)).expect_err("trigger aborts rescan file");
        assert!(error.to_string().contains("abort_rescan_file_mtime"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed rescan");
        assert_eq!(chunk_content(&conn, rel), "# Flow\n\nold content");
        assert_eq!(file_mtime(&conn, rel), 0);
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup rescan trigger root");
    }

    #[test]
    fn scan_removed_content_trigger_failure_preserves_chunks_and_mtime() {
        let root = temp_root("content-removal-trigger-rollback");
        let rel = "20260717/talents/flow.md";
        write(&root, &format!("chronicle/{rel}"), "# Flow\n\nremove me");
        scan_journal(&root, true).expect("initial scan");
        let conn = open_index(&root).expect("open index");
        create_abort_trigger(
            &conn,
            "abort_content_file_delete",
            "BEFORE",
            "DELETE",
            "files",
            Some("OLD.path='20260717/talents/flow.md'"),
        );
        drop(conn);
        fs::remove_file(root.join(format!("chronicle/{rel}"))).expect("remove source");

        let error = scan_journal(&root, true).expect_err("trigger aborts removal");
        assert!(error.to_string().contains("abort_content_file_delete"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed removal");
        assert_eq!(chunk_content(&conn, rel), "# Flow\n\nremove me");
        assert!(file_mtime(&conn, rel) > 0);
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup content removal trigger root");
    }

    #[test]
    fn light_mode_indexes_historical_talent_and_rebuilds_segment_aggregate() {
        let root = temp_root("light-historical-talent");
        let segment = "20240101/default/090000_300";
        let rel = "20240101/default/090000_300/talents/flow.md";
        write_stream(&root, "20240101", "default", "090000_300");
        write(
            &root,
            &format!("chronicle/{rel}"),
            "# Flow\n\nhistorical text",
        );
        let report = scan_journal(&root, false).expect("light scan");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert!(file_mtime(&conn, rel) > 0);
        assert!(segment_aggregate_content(&conn, segment).contains("historical text"));
        fs::remove_dir_all(root).expect("cleanup historical talent root");
    }

    #[test]
    fn light_mode_retains_edge_rows_when_day_has_no_discovered_files() {
        let root = temp_root("edge-light-removed");
        let rel = "20240101/default/090000_300/talents/speaker_labels.json";
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                "alice",
                "bob",
                "spoke-with",
                0_i64,
                "speaker-labels",
                rel,
                1_i64,
            ],
        )
        .expect("seed historical edge");
        conn.execute(
            "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
            params![rel, 1_i64],
        )
        .expect("seed historical edge mtime");
        drop(conn);

        let light = scan_journal(&root, false).expect("light scan");
        assert_eq!(light.edges_removed, 0);
        assert_eq!(
            light.warnings,
            vec![
                "light scan retained 1 stale row(s) for day 20240101: discovery produced no files for that day; rerun with --rescan-full to remove them"
            ]
        );
        let conn = Connection::open(db_path(&root)).expect("open db after light scan");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='20240101/default/090000_300/talents/speaker_labels.json'"
            ),
            1
        );
        drop(conn);

        let full = scan_journal(&root, true).expect("full scan");
        assert_eq!(full.edges_removed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after full scan");
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edge_files WHERE path='20240101/default/090000_300/talents/speaker_labels.json'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup edge light removed root");
    }

    #[test]
    fn light_mode_indexes_historical_edge_sources() {
        let root = temp_root("light-historical-edges");
        let first = "20240101/default/090000_300/screen.jsonl";
        let second = "20240101/default/090000_300/left_screen.jsonl";
        write(&root, &format!("chronicle/{first}"), r#"{"content":{}}"#);
        write(&root, &format!("chronicle/{second}"), r#"{"content":{}}"#);

        let report = scan_journal(&root, false).expect("light scan");
        assert_eq!(report.edges_indexed, 2);
        fs::remove_dir_all(root).expect("cleanup historical edges root");
    }

    #[test]
    fn light_mode_removes_historical_edge_rows_with_a_sibling() {
        let root = temp_root("light-historical-edge-removal");
        let removed = "20240101/default/090000_300/talents/speaker_labels.json";
        let sibling = "20240101/default/091000_300/talents/speaker_labels.json";
        write(&root, &format!("chronicle/{removed}"), "{}");
        write(&root, &format!("chronicle/{sibling}"), "{}");

        let conn = open_index(&root).expect("open index");
        for (rel, src, dst) in [(removed, "alice", "bob"), (sibling, "cora", "dan")] {
            conn.execute(
                "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![src, dst, "spoke-with", 0_i64, "speaker-labels", rel, 1_i64],
            )
            .expect("seed historical edge");
            conn.execute(
                "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
                params![
                    rel,
                    file_mtime_secs(&root.join(format!("chronicle/{rel}")))
                        .expect("edge source mtime")
                ],
            )
            .expect("seed historical edge mtime");
        }
        drop(conn);
        fs::remove_file(root.join(format!("chronicle/{removed}"))).expect("remove edge source");

        let report = scan_journal(&root, false).expect("light removal scan");
        assert_eq!(report.edges_removed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(edge_file_mtime(&conn, removed), None);
        assert!(edge_file_mtime(&conn, sibling).is_some());
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='20240101/default/090000_300/talents/speaker_labels.json'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='20240101/default/091000_300/talents/speaker_labels.json'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup historical edge removal root");
    }

    #[test]
    fn light_mode_removes_missing_historical_content_with_a_sibling() {
        let root = temp_root("light-historical-content");
        let removed = "20240101/talents/old.md";
        let sibling = "20240101/talents/keep.md";
        write(&root, &format!("chronicle/{removed}"), "# Old\n\nold");
        write(&root, &format!("chronicle/{sibling}"), "# Keep\n\nkeep");
        scan_journal(&root, true).expect("full populate");
        fs::remove_file(root.join(format!("chronicle/{removed}"))).expect("remove content source");

        let report = scan_journal(&root, false).expect("light removal scan");
        assert_eq!(report.removed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20240101/talents/old.md'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20240101/talents/keep.md'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20240101/talents/old.md'"
            ),
            0
        );
        assert!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20240101/talents/keep.md'"
            ) > 0
        );
        fs::remove_dir_all(root).expect("cleanup historical content root");
    }

    #[test]
    fn light_mode_retains_undiscoverable_day_and_full_scan_removes_it() {
        let root = temp_root("light-undiscoverable-day");
        let rel = "20260717/talents/today.md";
        write(&root, &format!("chronicle/{rel}"), "# Today\n\ntoday");
        scan_journal(&root, true).expect("full populate");
        fs::remove_dir_all(root.join("chronicle/20260717")).expect("remove day");

        let light = scan_journal(&root, false).expect("light scan");
        assert_eq!(light.removed, 0);
        assert_eq!(
            light.warnings,
            vec![
                "light scan retained 1 stale row(s) for day 20260717: discovery produced no files for that day; rerun with --rescan-full to remove them"
            ]
        );
        let full = scan_journal(&root, true).expect("full removal scan");
        assert_eq!(full.removed, 1);
        fs::remove_dir_all(root).expect("cleanup undiscoverable day root");
    }

    #[test]
    fn invalid_markdown_isolated_during_scan() {
        let root = temp_root("invalid");
        write(&root, "chronicle/20260717/talents/flow.md", "# Flow\n\nok");
        let invalid = root.join("chronicle/20260717/talents/bad.md");
        fs::create_dir_all(invalid.parent().expect("invalid parent")).expect("create parent");
        fs::write(invalid, [0xff]).expect("write invalid utf8");
        let report = scan_journal(&root, true).expect("scan with invalid");
        assert_eq!(report.indexed, 1);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.warnings.len(), 1);
        fs::remove_dir_all(root).expect("cleanup invalid root");
    }

    #[test]
    fn rescan_file_indexes_classified_families_and_declines_other_paths() {
        let root = temp_root("rescan-file");
        write(
            &root,
            "chronicle/20260717/default/234567_300/talents/audio.md",
            "# Audio\n\nbad time",
        );
        write_stream(&root, "20260717", "default", "234567_300");
        assert_eq!(
            rescan_file(
                &root,
                Path::new("20260717/default/234567_300/talents/audio.md")
            )
            .expect("rescan markdown"),
            RescanFileStatus::Indexed {
                warnings: Vec::new()
            }
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        let row: (String, String) = conn
            .query_row(
                "SELECT stream, time_bucket FROM chunks WHERE path='20260717/default/234567_300/talents/audio.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("metadata row");
        assert_eq!(row, ("default".to_string(), String::new()));
        drop(conn);

        write(
            &root,
            "facets/work/events/20240101.jsonl",
            r#"{"type":"meeting","title":"Standup"}
"#,
        );
        assert_eq!(
            rescan_file(&root, Path::new("facets/work/events/20240101.jsonl"))
                .expect("rescan event jsonl"),
            RescanFileStatus::Indexed {
                warnings: Vec::new()
            }
        );
        let conn = Connection::open(db_path(&root)).expect("open db after event rescan");
        let event_agent: String = conn
            .query_row(
                "SELECT agent FROM chunks WHERE path='facets/work/events/20240101.jsonl'",
                [],
                |row| row.get(0),
            )
            .expect("event agent row");
        assert_eq!(event_agent, "event");
        drop(conn);

        write(&root, "notes/foo.txt", "unsupported");
        assert_eq!(
            rescan_file(&root, Path::new("notes/foo.txt")).expect("decline unsupported"),
            RescanFileStatus::Declined
        );
        fs::remove_dir_all(root).expect("cleanup rescan root");
    }

    #[test]
    fn raw_percept_patterns_remain_unindexed_without_content_chunks() {
        for (suffix, rel, family, edge_source) in [
            (
                "audio",
                "20260717/default/090000_300/audio.jsonl",
                RawPerceptFamily::Audio,
                false,
            ),
            (
                "split-audio",
                "20260717/default/090000_300/left_audio.jsonl",
                RawPerceptFamily::Audio,
                false,
            ),
            (
                "transcript",
                "20260717/default/090000_300/session_transcript.jsonl",
                RawPerceptFamily::Audio,
                false,
            ),
            (
                "screen",
                "20260717/default/090000_300/screen.jsonl",
                RawPerceptFamily::RawScreen,
                true,
            ),
            (
                "split-screen",
                "20260717/default/090000_300/monitor_1_screen.jsonl",
                RawPerceptFamily::RawScreen,
                true,
            ),
        ] {
            let root = temp_root(&format!("raw-percept-{suffix}"));
            write(&root, &format!("chronicle/{rel}"), "{}\n");
            assert_eq!(classify(rel), ContentResolution::Unindexed(family), "{rel}");

            let report = scan_journal(&root, true).expect("scan raw percept");
            assert_eq!(report.indexed, 0, "{rel}");
            assert_eq!(report.skipped, 0, "{rel}");
            assert!(report.warnings.is_empty(), "{rel}");
            let conn = Connection::open(db_path(&root)).expect("open db after raw scan");
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM chunks WHERE path='{rel}'")
                ),
                0,
                "{rel}: scan inserted content chunks"
            );
            drop(conn);

            let status = rescan_file(&root, Path::new(rel)).expect("rescan raw percept");
            if edge_source {
                assert!(
                    matches!(status, RescanFileStatus::Indexed { warnings } if warnings.is_empty()),
                    "{rel}: screen edge source should rescan without content indexing"
                );
            } else {
                assert_eq!(status, RescanFileStatus::Declined, "{rel}");
            }
            let conn = Connection::open(db_path(&root)).expect("open db after raw rescan");
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT COUNT(*) FROM chunks WHERE path='{rel}'")
                ),
                0,
                "{rel}: rescan inserted content chunks"
            );
            drop(conn);
            fs::remove_dir_all(root).expect("cleanup raw percept root");
        }
    }

    #[test]
    fn scan_indexes_segment_aggregate_with_marker_stream_and_no_file_row() {
        let root = temp_root("segment-aggregate");
        let segment = "20260717/stream-dir/090000_300";
        write_stream_marker(
            &root,
            "20260717",
            "stream-dir",
            "090000_300",
            "marker-stream",
        );
        write(
            &root,
            "chronicle/20260717/stream-dir/090000_300/talents/audio.md",
            "# Audio\n\nMarker-derived stream text",
        );

        let report = scan_journal(&root, true).expect("scan segment aggregate");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        let row = chunk_row(&conn, segment);
        assert_eq!(row.0, "20260717");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "segment");
        assert_eq!(row.3, Some("marker-stream".to_string()));
        assert_eq!(row.4, "morning");
        assert!(segment_aggregate_content(&conn, segment).contains("Marker-derived stream text"));
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260717/stream-dir/090000_300'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup segment aggregate root");
    }

    #[test]
    fn scan_writes_no_segment_aggregate_without_talent_markdown() {
        let root = temp_root("segment-no-markdown");
        let segment = "20260717/default/101000_300";
        write_stream(&root, "20260717", "default", "101000_300");
        write(
            &root,
            "chronicle/20260717/default/101000_300/browser_example-com.jsonl",
            r#"{"t":"segment_start","ts":1,"title":"Example","blocks":[{"type":"text","text":"Browser only"}]}
"#,
        );

        scan_journal(&root, true).expect("scan browser-only segment");
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM chunks WHERE path=? AND agent='segment'",
                [segment],
                |row| row.get::<_, i64>(0),
            )
            .expect("segment aggregate count"),
            0
        );
        fs::remove_dir_all(root).expect("cleanup no-markdown root");
    }

    #[test]
    fn scan_removes_segment_aggregate_when_last_talent_markdown_is_deleted() {
        let root = temp_root("segment-removed-to-zero");
        let segment = "20260717/default/102000_300";
        let talent = "chronicle/20260717/default/102000_300/talents/audio.md";
        write_stream(&root, "20260717", "default", "102000_300");
        write(&root, talent, "# Audio\n\nInitial aggregate");
        scan_journal(&root, true).expect("initial scan");
        let conn = Connection::open(db_path(&root)).expect("open db after initial scan");
        assert!(!segment_aggregate_content(&conn, segment).is_empty());
        drop(conn);

        fs::remove_file(root.join(talent)).expect("remove segment talent");
        scan_journal(&root, true).expect("rescan after talent removal");
        let conn = Connection::open(db_path(&root)).expect("open db after removal scan");
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM chunks WHERE path=? AND agent='segment'",
                [segment],
                |row| row.get::<_, i64>(0),
            )
            .expect("segment aggregate count"),
            0
        );
        fs::remove_dir_all(root).expect("cleanup removed-to-zero root");
    }

    #[test]
    fn rescan_file_regenerates_segment_aggregate() {
        let root = temp_root("segment-rescan-file");
        let segment = "20260717/default/103000_300";
        write_stream(&root, "20260717", "default", "103000_300");
        write(
            &root,
            "chronicle/20260717/default/103000_300/talents/audio.md",
            "# Audio\n\nRescan aggregate phrase",
        );

        assert_eq!(
            rescan_file(
                &root,
                Path::new("20260717/default/103000_300/talents/audio.md")
            )
            .expect("rescan segment talent"),
            RescanFileStatus::Indexed {
                warnings: Vec::new()
            }
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        let content = segment_aggregate_content(&conn, segment);
        assert!(content.contains("Rescan aggregate phrase"));
        fs::remove_dir_all(root).expect("cleanup rescan segment aggregate root");
    }

    #[test]
    fn scan_segment_aggregate_incomplete_preserves_prior_rows_and_retries() {
        let root = temp_root("segment-unreadable-talent");
        let segment = "20260717/default/104000_300";
        let talent_rel = "20260717/default/104000_300/talents/audio.md";
        write_stream(&root, "20260717", "default", "104000_300");
        write(
            &root,
            &format!("chronicle/{talent_rel}"),
            "# Audio\n\nInitial aggregate phrase",
        );
        scan_journal(&root, true).expect("initial segment scan");
        let conn = open_index(&root).expect("open index");
        assert!(segment_aggregate_content(&conn, segment).contains("Initial aggregate phrase"));
        conn.execute("UPDATE files SET mtime=0 WHERE path=?", [talent_rel])
            .expect("force source reindex");
        drop(conn);

        write(
            &root,
            &format!("chronicle/{talent_rel}"),
            "# Audio\n\nFresh aggregate phrase",
        );
        let bad_rel = "20260717/default/104000_300/talents/bad.md";
        let invalid = root.join(format!("chronicle/{bad_rel}"));
        fs::write(&invalid, [0xff]).expect("write invalid segment markdown");

        let report = scan_journal(&root, true).expect("scan unreadable segment talent");
        assert_eq!(report.indexed, 0);
        assert_eq!(report.skipped, 2);
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("segment aggregate read failed")
                    && warning.contains("bad.md"))
        );
        let conn = Connection::open(db_path(&root)).expect("open db");
        let content = segment_aggregate_content(&conn, segment);
        assert!(content.contains("Initial aggregate phrase"));
        assert!(!content.contains("Fresh aggregate phrase"));
        assert_eq!(file_mtime(&conn, talent_rel), 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260717/default/104000_300/talents/bad.md'"
            ),
            0
        );
        drop(conn);

        fs::write(&invalid, "# Bad\n\nRepaired aggregate phrase").expect("repair segment markdown");
        let retry = scan_journal(&root, true).expect("retry segment aggregate");
        assert_eq!(retry.indexed, 2);
        assert_eq!(retry.skipped, 0);
        let conn = Connection::open(db_path(&root)).expect("open db after aggregate retry");
        let content = segment_aggregate_content(&conn, segment);
        assert!(content.contains("Fresh aggregate phrase"));
        assert!(content.contains("Repaired aggregate phrase"));
        assert!(file_mtime(&conn, talent_rel) > 0);
        assert!(file_mtime(&conn, bad_rel) > 0);
        fs::remove_dir_all(root).expect("cleanup unreadable segment root");
    }

    #[test]
    fn scan_segment_aggregate_spans_multiple_talent_files() {
        let root = temp_root("segment-multiple-talents");
        let segment = "20260717/default/105000_300";
        write_stream(&root, "20260717", "default", "105000_300");
        write(
            &root,
            "chronicle/20260717/default/105000_300/talents/audio.md",
            "# Audio\n\nFirst distinctive phrase",
        );
        write(
            &root,
            "chronicle/20260717/default/105000_300/talents/work/brief.md",
            "# Brief\n\nSecond distinctive phrase",
        );

        scan_journal(&root, true).expect("scan multiple talent segment");
        let conn = Connection::open(db_path(&root)).expect("open db");
        let content = segment_aggregate_content(&conn, segment);
        assert!(content.contains("First distinctive phrase"));
        assert!(content.contains("Second distinctive phrase"));
        fs::remove_dir_all(root).expect("cleanup multiple talents root");
    }

    #[test]
    fn scan_segment_aggregate_rebuild_deletes_stale_rows() {
        let root = temp_root("segment-stale-rebuild");
        let segment = "20260717/default/110000_300";
        let talent = "20260717/default/110000_300/talents/audio.md";
        write_stream(&root, "20260717", "default", "110000_300");
        write(
            &root,
            "chronicle/20260717/default/110000_300/talents/audio.md",
            "# Audio\n\nOriginal aggregate phrase",
        );
        scan_journal(&root, true).expect("initial scan");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "stale aggregate phrase",
                segment,
                "20260717",
                "",
                "segment",
                "default",
                99_i64,
                "morning",
            ],
        )
        .expect("seed stale aggregate row");
        conn.execute("UPDATE files SET mtime=0 WHERE path=?", [talent])
            .expect("force segment talent reindex");
        drop(conn);
        write(
            &root,
            "chronicle/20260717/default/110000_300/talents/audio.md",
            "# Audio\n\nFresh aggregate phrase",
        );

        scan_journal(&root, true).expect("rebuild aggregate scan");
        let conn = Connection::open(db_path(&root)).expect("open db after rebuild");
        let content = segment_aggregate_content(&conn, segment);
        assert!(content.contains("Fresh aggregate phrase"));
        assert!(!content.contains("stale aggregate phrase"));
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM chunks WHERE path=? AND content='stale aggregate phrase'",
                [segment],
                |row| row.get::<_, i64>(0),
            )
            .expect("stale aggregate count"),
            0
        );
        fs::remove_dir_all(root).expect("cleanup stale rebuild root");
    }

    #[test]
    fn scan_indexes_jsonl_families_with_path_metadata() {
        let root = temp_root("jsonl-families");
        write(
            &root,
            "config/actions/20240101.jsonl",
            r#"{"action":"identity_update","actor":"settings","source":"app","timestamp":"2025-12-16T07:33:05.135587+00:00","params":{"name":"Alice"}}
"#,
        );
        write(
            &root,
            "facets/Work/events/20240101.jsonl",
            r#"{"type":"meeting","title":"Standup","start":"09:00:00","end":"09:30:00","participants":["Alice","Bob"],"summary":"Daily sync"}
"#,
        );
        write(
            &root,
            "facets/work/activities/20240101.jsonl",
            r#"{}
{"id":"coding_090000_300","segments":["090000_300"]}
"#,
        );
        write(
            &root,
            "facets/work/logs/20240101.jsonl",
            r#"{"action":"activity_update","actor":"activities","source":"app","params":{"id":"coding"}}
"#,
        );

        let report = scan_journal(&root, true).expect("scan jsonl families");
        assert_eq!(report.indexed, 4);
        let conn = Connection::open(db_path(&root)).expect("open db");
        let config_row: (String, String, String, Option<String>, String, String) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='config/actions/20240101.jsonl'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("config action row");
        assert_eq!(
            config_row,
            (
                "20240101".to_string(),
                String::new(),
                "action".to_string(),
                None,
                String::new(),
                "### Identity Update by settings\n\n**Source:** app | **Time:** 07:33:05\n\n**Parameters:**\n- name: Alice".to_string(),
            )
        );

        let event_row: (String, String, String, String) = conn
            .query_row(
                "SELECT day, facet, agent, content FROM chunks WHERE path='facets/Work/events/20240101.jsonl'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("event row");
        assert_eq!(event_row.0, "20240101");
        assert_eq!(event_row.1, "work");
        assert_eq!(event_row.2, "event");
        assert!(event_row.3.contains("### Meeting: Standup"));
        assert!(event_row.3.contains("**Participants:** Alice, Bob"));

        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/activities/20240101.jsonl' AND agent='activity'"
            ),
            2
        );
        let activity_content: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE path='facets/work/activities/20240101.jsonl' AND idx=1",
                [],
                |row| row.get(0),
            )
            .expect("activity content");
        assert!(activity_content.contains("### Coding 090000 300"));
        assert!(activity_content.contains("- Time: 09:00-09:05"));

        let action_log_agent: String = conn
            .query_row(
                "SELECT agent FROM chunks WHERE path='facets/work/logs/20240101.jsonl'",
                [],
                |row| row.get(0),
            )
            .expect("facet log row");
        assert_eq!(action_log_agent, "action");
        fs::remove_dir_all(root).expect("cleanup jsonl families root");
    }

    #[test]
    fn scan_indexes_facet_entities_and_observations() {
        let root = temp_root("facet-entities-observations");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"type":"Person","name":"Romeo Montague","description":"Met Juliet at Denver Tech Summit","tags":["summit"],"aka":["Romeo"],"role":"Engineer"}
"#,
        );
        write(
            &root,
            "facets/work/entities/123.jsonl",
            r#"{"type":"Person","name":"Short Stem","description":"Short digit stem"}
"#,
        );
        write(
            &root,
            "facets/work/entities/99999999.jsonl",
            r#"{"type":"Person","name":"Invalid Day","description":"Invalid calendar day"}
"#,
        );
        write(
            &root,
            "facets/work/entities/some-slug.jsonl",
            r#"{"type":"Project","name":"Attached Shape","description":"Slug-shaped jsonl"}
"#,
        );
        write(
            &root,
            "facets/work/entities/romeo_montague/observations.jsonl",
            r#"{"content":"Prefers morning product reviews","observed_at":1772658000000,"source_day":"20260304"}
"#,
        );
        write(&root, "facets/work/entities/empty.jsonl", "");
        write(
            &root,
            "facets/work/entities/empty_person/observations.jsonl",
            "",
        );

        let report = scan_journal(&root, true).expect("scan facet entities");
        assert_eq!(report.indexed, 7);
        let conn = Connection::open(db_path(&root)).expect("open db");

        for (path, expected_agent, expected_day) in [
            (
                "facets/work/entities/20260304.jsonl",
                "entity:detected",
                "20260304",
            ),
            // This fails if the agent predicate is implemented via is_date_key.
            ("facets/work/entities/123.jsonl", "entity:detected", ""),
            (
                "facets/work/entities/99999999.jsonl",
                "entity:detected",
                "99999999",
            ),
            (
                "facets/work/entities/some-slug.jsonl",
                "entity:attached",
                "",
            ),
        ] {
            let row = chunk_row(&conn, path);
            assert_eq!(row.0, expected_day, "{path}");
            assert_eq!(row.1, "work", "{path}");
            assert_eq!(row.2, expected_agent, "{path}");
            assert_eq!(row.3, None, "{path}");
            assert_eq!(row.4, "", "{path}");
        }

        let entity_row = chunk_row(&conn, "facets/work/entities/20260304.jsonl");
        assert!(entity_row.5.contains("### Person: Romeo Montague"));
        assert!(entity_row.5.contains("Met Juliet at Denver Tech Summit"));
        assert!(entity_row.5.contains("**Tags:** summit"));
        assert!(entity_row.5.contains("**Role:** Engineer"));

        let observation_row = chunk_row(
            &conn,
            "facets/work/entities/romeo_montague/observations.jsonl",
        );
        assert_eq!(observation_row.0, "");
        assert_eq!(observation_row.1, "work");
        assert_eq!(observation_row.2, "observation");
        assert_eq!(observation_row.3, None);
        assert_eq!(observation_row.4, "");
        assert!(
            observation_row
                .5
                .contains("- Prefers morning product reviews (observed: 20260304)")
        );

        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/entities/empty.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='facets/work/entities/empty.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/entities/empty_person/observations.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='facets/work/entities/empty_person/observations.jsonl'"
            ),
            1
        );

        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        assert_eq!(report.edges_indexed, 7);
        assert_eq!(report.edge_rows_inserted, 0);
        assert_eq!(
            edge_file_paths(&conn),
            vec![
                "edges:__schema__".to_string(),
                "facets/work/entities/123.jsonl".to_string(),
                "facets/work/entities/20260304.jsonl".to_string(),
                "facets/work/entities/99999999.jsonl".to_string(),
                "facets/work/entities/empty.jsonl".to_string(),
                "facets/work/entities/empty_person/observations.jsonl".to_string(),
                "facets/work/entities/romeo_montague/observations.jsonl".to_string(),
                "facets/work/entities/some-slug.jsonl".to_string(),
            ]
        );
        fs::remove_dir_all(root).expect("cleanup facet entities root");
    }

    #[test]
    fn scan_indexes_entity_search_rows_and_watermarks() {
        let root = temp_root("entity-search");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person","aka":["AJ"],"created_at":1767249000000}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Works on native search","tags":["rust"],"last_seen":"20260102"}"#,
        );

        let report = scan_journal(&root, true).expect("scan entity search");
        assert_eq!(report.indexed, 0);
        assert_eq!(report.removed, 0);

        let conn = Connection::open(db_path(&root)).expect("open db");
        let row: (String, String, String, String, String, String, i64, String) = conn
            .query_row(
                "SELECT content, path, day, facet, agent, stream, idx, time_bucket FROM chunks WHERE agent='entity'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .expect("entity search row");
        assert_eq!(
            row,
            (
                "Alice Johnson (Person)\nAlso known as: AJ\nWorks on native search\nTags: rust"
                    .to_string(),
                "entity_search:alice".to_string(),
                "20260102".to_string(),
                "work".to_string(),
                "entity".to_string(),
                String::new(),
                0,
                String::new(),
            )
        );
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read entity watermark"),
            Some(EntitySearchWatermark {
                mtime: build_entity_search(&root)
                    .expect("rebuild entity search")
                    .watermark_mtime_secs,
                count: 2,
            })
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path LIKE 'entity_search:%'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup entity search root");
    }

    #[test]
    fn entity_search_incremental_with_zero_sources_writes_no_watermarks() {
        let root = temp_root("entity-search-empty");
        let report = scan_journal(&root, false).expect("scan empty journal");
        assert_eq!(report.indexed, 0);
        assert_eq!(report.removed, 0);

        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read empty watermark"),
            Some(EntitySearchWatermark { mtime: 0, count: 0 })
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM chunks WHERE agent='entity'"),
            0
        );
        fs::remove_dir_all(root).expect("cleanup empty entity search root");
    }

    #[test]
    fn entity_search_trigger_failure_rolls_back_rows_and_watermarks_then_retries() {
        let root = temp_root("entity-search-trigger-rollback");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Old","type":"Person"}"#,
        );
        scan_journal(&root, true).expect("initial entity search scan");
        let conn = open_index(&root).expect("open index");
        assert_eq!(
            chunk_content(&conn, "entity_search:alice"),
            "Alice Old (Person)"
        );
        let initial_watermark = entity_watermark(&conn);
        create_abort_trigger(
            &conn,
            "abort_entity_search_watermark",
            "BEFORE",
            "INSERT",
            "entity_search_watermark",
            None,
        );
        drop(conn);
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice New","type":"Person"}"#,
        );

        let error = scan_journal(&root, true).expect_err("entity trigger aborts");
        assert!(error.to_string().contains("abort_entity_search_watermark"));
        let conn = Connection::open(db_path(&root)).expect("open db after failed entity search");
        assert_eq!(
            chunk_content(&conn, "entity_search:alice"),
            "Alice Old (Person)"
        );
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read preserved watermark"),
            Some(initial_watermark)
        );
        assert_sqlite_and_fts_integrity(&conn);
        drop_trigger(&conn, "abort_entity_search_watermark");
        drop(conn);

        let retry = scan_journal(&root, true).expect("retry entity search");
        assert_eq!(retry.failed, 0);
        let conn = Connection::open(db_path(&root)).expect("open db after entity retry");
        assert_eq!(
            chunk_content(&conn, "entity_search:alice"),
            "Alice New (Person)"
        );
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read retry watermark"),
            Some(EntitySearchWatermark {
                mtime: build_entity_search(&root)
                    .expect("read retry entity build")
                    .watermark_mtime_secs,
                count: 1,
            })
        );
        fs::remove_dir_all(root).expect("cleanup entity trigger root");
    }

    #[test]
    fn entity_search_excludes_python_real_watermark_from_file_mtimes() {
        let root = temp_root("entity-search-python-real");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES ('entity_search:__mtime__', 1784360332.93201)",
            [],
        )
        .expect("seed python mtime watermark");
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES ('entity_search:__count__', 73.0)",
            [],
        )
        .expect("seed python count watermark");
        drop(conn);

        let report = scan_journal(&root, false).expect("scan with real watermark");
        assert_eq!(report.indexed, 0);
        assert_eq!(report.removed, 0);

        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path IN ('entity_search:__mtime__', 'entity_search:__count__')"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path LIKE 'entity_search:%' AND typeof(mtime)='integer'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup python real root");
    }

    #[test]
    fn entity_search_incremental_skips_and_full_forces_rebuild() {
        let root = temp_root("entity-search-full-force");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person"}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Fresh content"}"#,
        );

        scan_journal(&root, true).expect("initial full scan");
        let conn = Connection::open(db_path(&root)).expect("open db");
        conn.execute(
            "UPDATE chunks SET content='stale content' WHERE agent='entity'",
            [],
        )
        .expect("make entity chunk stale");
        drop(conn);

        scan_journal(&root, false).expect("incremental scan");
        let conn = Connection::open(db_path(&root)).expect("open db after incremental");
        let content: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE agent='entity'",
                [],
                |row| row.get(0),
            )
            .expect("entity content after incremental");
        assert_eq!(content, "stale content");
        drop(conn);

        scan_journal(&root, true).expect("forced full scan");
        let conn = Connection::open(db_path(&root)).expect("open db after full");
        let content: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE agent='entity'",
                [],
                |row| row.get(0),
            )
            .expect("entity content after full");
        assert_eq!(content, "Alice Johnson (Person)\nFresh content");
        fs::remove_dir_all(root).expect("cleanup full force root");
    }

    #[test]
    fn entity_search_entity_data_mtime_change_rebuilds_incrementally() {
        let root = temp_root("entity-search-data-change");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person"}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Original description"}"#,
        );

        scan_journal(&root, true).expect("initial full scan");
        let conn = Connection::open(db_path(&root)).expect("open db");
        let initial = read_entity_search_watermark(&conn)
            .expect("read initial entity watermark")
            .expect("initial entity watermark");
        assert_eq!(initial.count, 2);
        write_entity_search_watermark(&conn, 0, initial.count)
            .expect("age stored entity search mtime");
        drop(conn);

        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Updated","type":"Person"}"#,
        );
        let report = scan_journal(&root, false).expect("incremental rebuild");
        assert_eq!(report.indexed, 0);
        assert_eq!(report.removed, 0);

        let conn = Connection::open(db_path(&root)).expect("open db after rebuild");
        let row = chunk_row(&conn, "entity_search:alice");
        assert_eq!(row.5, "Alice Updated (Person)\nOriginal description");
        let watermark = read_entity_search_watermark(&conn)
            .expect("read rebuilt watermark")
            .expect("rebuilt watermark");
        assert!(watermark.mtime > 0);
        assert_eq!(watermark.count, initial.count);
        assert!(initial.mtime > 0);
        fs::remove_dir_all(root).expect("cleanup data change root");
    }

    #[test]
    fn entity_search_removing_all_sources_rebuilds_to_zero_watermarks() {
        let root = temp_root("entity-search-remove-all");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person"}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Will be removed"}"#,
        );

        scan_journal(&root, true).expect("initial full scan");
        fs::remove_file(root.join("entities/alice/entity.json")).expect("remove identity");
        fs::remove_file(root.join("facets/work/entities/alice/entity.json"))
            .expect("remove relationship");

        scan_journal(&root, false).expect("incremental remove all");
        let conn = Connection::open(db_path(&root)).expect("open db after remove all");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path LIKE 'entity_search:%' OR agent='entity'"
            ),
            0
        );
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read zero watermark"),
            Some(EntitySearchWatermark { mtime: 0, count: 0 })
        );
        fs::remove_dir_all(root).expect("cleanup remove all root");
    }

    #[test]
    fn entity_search_blocked_only_stops_rebuilding_after_first_incremental_scan() {
        let root = temp_root("entity-search-blocked-only");
        write(
            &root,
            "entities/blocked/entity.json",
            r#"{"name":"Blocked Person","type":"Person","blocked":true}"#,
        );

        scan_journal(&root, false).expect("first blocked-only scan");
        let conn = Connection::open(db_path(&root)).expect("open db after first blocked scan");
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read blocked watermark"),
            Some(EntitySearchWatermark {
                mtime: build_entity_search(&root)
                    .expect("build blocked entity search")
                    .watermark_mtime_secs,
                count: 1,
            })
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM chunks WHERE agent='entity'"),
            0
        );
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('stale blocked rebuild probe', 'entity_search:stale', '', '', 'legacy', '', 0, '')",
            [],
        )
        .expect("seed stale non-entity chunk");
        drop(conn);

        scan_journal(&root, false).expect("second blocked-only scan");
        let conn = Connection::open(db_path(&root)).expect("open db after second blocked scan");
        assert!(
            read_entity_search_watermark(&conn)
                .expect("read stable blocked watermark")
                .is_some()
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM chunks WHERE agent='entity'"),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE content='stale blocked rebuild probe'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup blocked-only root");
    }

    #[test]
    fn entity_search_clean_rebuild_clears_stale_entity_rows() {
        let root = temp_root("entity-search-clean-rebuild");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Johnson","type":"Person"}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Fresh row"}"#,
        );
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('stale entity-search row', 'entity_search:stale', '', '', 'entity', '', 0, '')",
            [],
        )
        .expect("seed stale entity-search row");
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('stale legacy entity row', 'entities/stale/entity.json', '', '', 'legacy', '', 0, '')",
            [],
        )
        .expect("seed stale legacy entity row");
        drop(conn);

        scan_journal(&root, false).expect("clean rebuild scan");
        let conn = Connection::open(db_path(&root)).expect("open db after clean rebuild");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE content IN ('stale entity-search row', 'stale legacy entity row')"
            ),
            0
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 1);
        let row = chunk_row(&conn, "entity_search:alice");
        assert_eq!(row.2, "entity");
        assert_eq!(row.5, "Alice Johnson (Person)\nFresh row");
        fs::remove_dir_all(root).expect("cleanup clean rebuild root");
    }

    #[test]
    fn entity_search_rewrites_only_changed_entity_rows() {
        let root = temp_root("entity-search-per-entity-diff");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice Old","type":"Person"}"#,
        );
        write(
            &root,
            "entities/bob/entity.json",
            r#"{"name":"Bob","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        let conn = open_index(&root).expect("open initial index");
        let initial_rowids = entity_rowids(&conn);
        let watermark = entity_watermark(&conn);
        write_entity_search_watermark(&conn, 0, watermark.count).expect("age watermark");
        drop(conn);

        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice New","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("incremental entity diff");
        let conn = Connection::open(db_path(&root)).expect("open updated index");
        let updated_rowids = entity_rowids(&conn);
        assert_ne!(
            updated_rowids["entity_search:alice"],
            initial_rowids["entity_search:alice"]
        );
        assert_eq!(
            updated_rowids["entity_search:bob"],
            initial_rowids["entity_search:bob"]
        );
        fs::remove_dir_all(root).expect("cleanup per-entity diff root");
    }

    #[test]
    fn entity_search_rename_with_preserved_mtime_repairs_paths() {
        let root = temp_root("entity-search-rename");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        write(
            &root,
            "facets/work/entities/alice/entity.json",
            r#"{"description":"Renamed relationship"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        fs::rename(root.join("entities/alice"), root.join("entities/alicia"))
            .expect("rename canonical entity without rewriting it");
        fs::rename(
            root.join("facets/work/entities/alice"),
            root.join("facets/work/entities/alicia"),
        )
        .expect("rename relationship without rewriting it");

        scan_journal(&root, false).expect("rename reconciliation scan");
        let conn = Connection::open(db_path(&root)).expect("open renamed index");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='entity_search:alice'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='entity_search:alicia'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup rename root");
    }

    #[test]
    fn entity_search_repairs_a_deleted_entity_chunk_on_light_scan() {
        let root = temp_root("entity-search-repair-partial");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        let conn = open_index(&root).expect("open initial index");
        conn.execute(
            "DELETE FROM chunks WHERE agent='entity' AND path='entity_search:alice'",
            [],
        )
        .expect("delete one entity path");
        drop(conn);

        scan_journal(&root, false).expect("repair light scan");
        let conn = Connection::open(db_path(&root)).expect("open repaired index");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE agent='entity' AND path='entity_search:alice'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup repair root");
    }

    #[test]
    fn entity_search_full_recovery_matches_a_from_empty_build() {
        let root = temp_root("entity-search-full-recovery");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        write(
            &root,
            "entities/bob/entity.json",
            r#"{"name":"Bob","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        let conn = open_index(&root).expect("open corruptible index");
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('stale', 'entity_search:stale', '', '', 'entity', '', 0, '')",
            [],
        )
        .expect("seed stale entity row");
        drop(conn);

        scan_journal(&root, true).expect("full recovery scan");
        let conn = Connection::open(db_path(&root)).expect("open recovered index");
        let recovered = stored_entity_search_rows(&conn).expect("read recovered rows");
        drop(conn);

        reset_index(&root).expect("reset index for from-empty comparison");
        scan_journal(&root, true).expect("from-empty full scan");
        let conn = Connection::open(db_path(&root)).expect("open from-empty index");
        assert_eq!(
            stored_entity_search_rows(&conn).expect("read from-empty rows"),
            recovered
        );
        fs::remove_dir_all(root).expect("cleanup full recovery root");
    }

    #[test]
    fn entity_search_removes_deleted_entities_and_adds_new_ones_without_orphans() {
        let root = temp_root("entity-search-delete-add");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        fs::remove_file(root.join("entities/alice/entity.json")).expect("remove alice");
        write(
            &root,
            "entities/bob/entity.json",
            r#"{"name":"Bob","type":"Person"}"#,
        );

        scan_journal(&root, false).expect("delete/add light scan");
        let conn = Connection::open(db_path(&root)).expect("open delete/add index");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE agent='entity' AND path='entity_search:alice'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE agent='entity' AND path='entity_search:bob'"
            ),
            1
        );
        assert_eq!(
            count(&conn, "SELECT count(*) FROM chunks WHERE agent='entity'"),
            1
        );
        fs::remove_dir_all(root).expect("cleanup delete/add root");
    }

    #[test]
    fn reset_then_light_scan_rebuilds_entity_search_and_watermark() {
        let root = temp_root("entity-search-reset-light");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        scan_journal(&root, false).expect("initial entity scan");
        reset_index(&root).expect("reset index");
        let conn = Connection::open(db_path(&root)).expect("open reset index");
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read reset watermark"),
            None
        );
        drop(conn);

        scan_journal(&root, false).expect("light scan after reset");
        let conn = Connection::open(db_path(&root)).expect("open rebuilt index");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE agent='entity' AND path='entity_search:alice'"
            ),
            1
        );
        assert_eq!(entity_watermark(&conn).count, 1);
        fs::remove_dir_all(root).expect("cleanup reset light root");
    }

    #[test]
    fn entity_search_migrates_legacy_watermarks_once_without_rebuilding_rows() {
        let root = temp_root("entity-search-legacy-migration");
        write(
            &root,
            "entities/alice/entity.json",
            r#"{"name":"Alice","type":"Person"}"#,
        );
        let build = build_entity_search(&root).expect("build legacy entity rows");
        let conn = open_index(&root).expect("open legacy index");
        insert_entity_search_rows(&conn, &build.rows).expect("seed legacy entity rows");
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES (?, ?)",
            params![
                ENTITY_SEARCH_WATERMARK_MTIME_PATH,
                build.watermark_mtime_secs as f64 + 0.93201
            ],
        )
        .expect("seed legacy real mtime");
        conn.execute(
            "REPLACE INTO files(path, mtime) VALUES (?, ?)",
            params![ENTITY_SEARCH_WATERMARK_COUNT_PATH, build.count],
        )
        .expect("seed legacy count");
        let rowids_before = entity_rowids(&conn);
        drop(conn);

        scan_journal(&root, false).expect("migrate legacy watermark");
        let conn = Connection::open(db_path(&root)).expect("open migrated index");
        assert_eq!(
            entity_watermark(&conn),
            EntitySearchWatermark {
                mtime: build.watermark_mtime_secs,
                count: build.count,
            }
        );
        assert_eq!(entity_rowids(&conn), rowids_before);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path LIKE 'entity_search:%'"
            ),
            0
        );
        fs::remove_dir_all(root).expect("cleanup legacy migration root");
    }

    #[test]
    fn scan_indexes_structured_imports_with_formatter_agent() {
        let root = temp_root("structured-import");
        write(
            &root,
            "chronicle/20260101/import.ics/imported.jsonl",
            r#"{"import":{"source":"ICS"},"entry_count":2}
{"type":"calendar_event","title":"Planning Session","ts":"2026-01-01T09:30:00-07:00","duration_minutes":30}
{"type":"generic"}
"#,
        );

        let report = scan_journal(&root, true).expect("scan structured import");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260101/import.ics/imported.jsonl'"
            ),
            1
        );
        let row: (
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='20260101/import.ics/imported.jsonl'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("structured import row");
        assert_eq!(row.0, "20260101");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "import.ics");
        assert_eq!(row.3, None);
        assert_eq!(row.4, "");
        assert!(row.5.contains("Planning Session"));
        fs::remove_dir_all(root).expect("cleanup structured import root");
    }

    #[test]
    fn scan_indexes_ai_chat_imports_without_metadata_facet() {
        let root = temp_root("ai-chat-import");
        write(
            &root,
            "chronicle/20260101/import.claude/thread_a/conversation_transcript.jsonl",
            r#"{"model":"claude-3","imported":{"facet":"work"}}
{"start":"00:00:01","speaker":"User","text":"Hello"}
{"start":"00:00:02","speaker":"Assistant","text":"Hi there"}
{"start":"00:00:03","speaker":"Assistant","text":""}
"#,
        );

        let report = scan_journal(&root, true).expect("scan ai chat import");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260101/import.claude/thread_a/conversation_transcript.jsonl'"
            ),
            2
        );
        let row: (
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='20260101/import.claude/thread_a/conversation_transcript.jsonl' ORDER BY idx LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("ai chat import row");
        assert_eq!(row.0, "20260101");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "import.claude");
        assert_eq!(row.3, None);
        assert_eq!(row.4, "");
        assert!(row.5.contains("**User:**"));
        fs::remove_dir_all(root).expect("cleanup ai chat import root");
    }

    #[test]
    fn scan_writes_file_row_for_zero_chunk_ai_chat_import() {
        let root = temp_root("zero-ai-chat-import");
        write(
            &root,
            "chronicle/20260101/import.gemini/thread_a/conversation_transcript.jsonl",
            r#"{"model":"gemini"}
{"start":"00:00:01","speaker":"User","text":""}
{"start":"00:00:02","speaker":"Assistant","text":""}
"#,
        );

        let report = scan_journal(&root, true).expect("scan zero ai chat import");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260101/import.gemini/thread_a/conversation_transcript.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260101/import.gemini/thread_a/conversation_transcript.jsonl'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup zero ai chat import root");
    }

    #[test]
    fn scan_indexes_chat_streams_with_segment_bucket() {
        let root = temp_root("chat-stream");
        write(
            &root,
            "chronicle/20260508/chat/120000_300/chat.jsonl",
            r#"{"kind":"owner_message","ts":1,"text":"Need a diff"}
{"kind":"owner_message","ts":2,"text":"   "}
{"kind":"sol_message","ts":3,"text":"I can do that"}
{"kind":"owner_chat_open","ts":4,"request_id":"req","surface":"convey"}
{"kind":"mystery","ts":5,"text":"skip me"}
"#,
        );

        let report = scan_journal(&root, true).expect("scan chat stream");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260508/chat/120000_300/chat.jsonl'"
            ),
            3
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260508/chat/120000_300/chat.jsonl'"
            ),
            1
        );
        let row: (String, String, String, Option<String>, String) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket FROM chunks WHERE path='20260508/chat/120000_300/chat.jsonl' ORDER BY idx LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("chat metadata row");
        assert_eq!(row.0, "20260508");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "chat");
        assert_eq!(row.3, None);
        assert_eq!(row.4, "afternoon");

        let contents: Vec<String> = conn
            .prepare(
                "SELECT content FROM chunks WHERE path='20260508/chat/120000_300/chat.jsonl' ORDER BY idx",
            )
            .expect("prepare chat contents")
            .query_map([], |row| row.get(0))
            .expect("query chat contents")
            .map(|row| row.expect("chat content row"))
            .collect();
        assert_eq!(
            contents,
            vec![
                "**Owner** Need a diff".to_string(),
                "**Owner**".to_string(),
                "**Sol** I can do that".to_string(),
            ]
        );
        fs::remove_dir_all(root).expect("cleanup chat stream root");
    }

    fn seed_owner_chat(root: &Path) -> &'static str {
        let rel = "20260508/chat/120000_300/chat.jsonl";
        write(
            root,
            &format!("chronicle/{rel}"),
            r#"{"kind":"owner_message","ts":1,"text":"Need a diff"}
{"kind":"sol_message","ts":2,"text":"I can do that"}
"#,
        );
        rel
    }

    fn assert_chat_labels(root: &Path, rel: &str, owner: &str, agent: &str) {
        let conn = Connection::open(db_path(root)).expect("open db");
        let contents: Vec<String> = conn
            .prepare("SELECT content FROM chunks WHERE path=? ORDER BY idx")
            .expect("prepare chat contents")
            .query_map([rel], |row| row.get(0))
            .expect("query chat contents")
            .map(|row| row.expect("chat content row"))
            .collect();
        assert_eq!(
            contents,
            vec![
                format!("**{owner}** Need a diff"),
                format!("**{agent}** I can do that"),
            ]
        );
    }

    #[test]
    fn scan_uses_journal_config_chat_labels_with_reference_precedence() {
        for (name, config, owner, agent) in [
            (
                "preferred",
                r#"{"identity":{"preferred":"Preferred","name":"Name"},"agent":{"name":"Helper"}}"#,
                "Preferred",
                "Helper",
            ),
            (
                "name",
                r#"{"identity":{"name":"Name"},"agent":{"name":"Helper"}}"#,
                "Name",
                "Helper",
            ),
            ("absent", r#"{}"#, "Owner", "Sol"),
        ] {
            let root = temp_root(&format!("chat-label-{name}"));
            let rel = seed_owner_chat(&root);
            write(&root, "config/journal.json", config);

            let report = scan_journal(&root, true).expect("scan chat labels");
            assert!(
                !report
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("chat labels unavailable")),
                "valid config with absent fields is not a fallback error"
            );
            assert_chat_labels(&root, rel, owner, agent);
            fs::remove_dir_all(root).expect("cleanup chat label root");
        }
    }

    #[test]
    fn scan_missing_chat_config_uses_default_labels_and_warns_per_chat_file() {
        let root = temp_root("chat-label-missing");
        let rel = seed_owner_chat(&root);

        let report = scan_journal(&root, true).expect("scan missing chat config");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("chat labels unavailable"))
        );
        assert_chat_labels(&root, rel, "Owner", "Sol");
        fs::remove_dir_all(root).expect("cleanup missing chat config root");
    }

    #[test]
    fn scan_corrupt_chat_config_never_indexes_fallback_labels() {
        let root = temp_root("chat-label-corrupt");
        let rel = seed_owner_chat(&root);
        write(
            &root,
            "config/journal.json",
            r#"{"identity":{"timezone":NaN}}"#,
        );
        write(
            &root,
            "chronicle/20260508/chat/120100_300/chat.jsonl",
            r#"{"kind":"owner_message","ts":3,"text":"Second chat"}
{"kind":"sol_message","ts":4,"text":"Still no fallback"}
"#,
        );
        write(
            &root,
            "chronicle/20260508/talents/flow.md",
            "# Flow\n\nStill indexed",
        );

        let report = scan_journal(&root, true).expect("scan corrupt chat config");
        assert_eq!(report.failed, 3);
        assert_eq!(report.warnings.len(), 2);
        assert!(
            report
                .warnings
                .iter()
                .all(|warning| warning.starts_with("I couldn't read your settings file"))
        );
        let conn = Connection::open(db_path(&root)).expect("open index after corrupt chat scan");
        assert_eq!(
            count(
                &conn,
                &format!("SELECT count(*) FROM chunks WHERE path='{rel}'")
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE content LIKE '**Owner**%' OR content LIKE '**Sol**%'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260508/talents/flow.md'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup corrupt chat config root");
    }

    #[test]
    fn rescan_chat_with_corrupt_config_preserves_prior_chunks() {
        let root = temp_root("chat-rescan-corrupt");
        let rel = seed_owner_chat(&root);
        scan_journal(&root, true).expect("scan missing chat config");
        write(
            &root,
            "config/journal.json",
            r#"{"identity":{"timezone":NaN}}"#,
        );

        assert!(matches!(
            rescan_file(&root, Path::new(rel)),
            Err(StoreError::JournalConfig(_))
        ));
        assert_chat_labels(&root, rel, "Owner", "Sol");
        fs::remove_dir_all(root).expect("cleanup corrupt chat rescan root");
    }

    #[test]
    fn scan_indexes_at_least_one_chunk_for_every_content_family() {
        let root = temp_root("all-content-families");
        for (rel, text) in [
            ("chronicle/20260101/talents/flow.md", "# Flow\n\nBody\n"),
            (
                "facets/work/events/20260101.jsonl",
                r#"{"type":"meeting","title":"Standup","start":"09:00:00"}
"#,
            ),
            (
                "facets/work/activities/20260101.jsonl",
                r#"{"id":"coding","created_at":1}
"#,
            ),
            (
                "config/actions/20260101.jsonl",
                r#"{"action":"identity_update","timestamp":"2026-01-01T00:00:00+00:00"}
"#,
            ),
            (
                "chronicle/20260101/import.ics/imported.jsonl",
                r#"{"import":{"source":"ics"}}
{"type":"calendar_event","title":"Planning","ts":"2026-01-01T09:30:00-07:00"}
"#,
            ),
            (
                "chronicle/20260101/import.claude/thread/conversation_transcript.jsonl",
                r#"{"model":"claude"}
{"start":"00:00:01","speaker":"User","text":"Hello"}
"#,
            ),
            (
                "chronicle/20260101/chat/090000_300/chat.jsonl",
                r#"{"kind":"owner_message","ts":1,"text":"Hello"}
"#,
            ),
            (
                "chronicle/20260101/default/090000_300/browser_example.jsonl",
                r#"{"t":"segment_start","ts":1,"url":"https://example.com","blocks":[{"type":"text","text":"Page"}]}
"#,
            ),
            (
                "chronicle/20260101/talents/pulse.jsonl",
                r#"{"ts":1,"summary":"steady"}
"#,
            ),
            (
                "facets/work/entities/20260101.jsonl",
                r#"{"type":"Person","name":"Alice"}
"#,
            ),
            (
                "facets/work/entities/alice/observations.jsonl",
                r#"{"content":"Observation","observed_at":1}
"#,
            ),
            (
                "chronicle/20260101/default/090000_300/talents/documents.json",
                r#"{"documents":[{"title":"Contract","summary":"Signed","kind":"pdf"}]}
"#,
            ),
            (
                "chronicle/20260101/default/090000_300/talents/screen.json",
                r#"{"summary":"Editor open","applications":["nvim"]}
"#,
            ),
            (
                "chronicle/20260101/default/090000_300/talents/sense.json",
                r#"{"entities":[{"name":"Alice","type":"Person"}]}
"#,
            ),
            (
                "chronicle/20260101/talents/morning_briefing.json",
                r#"{"greeting":"Morning","sections":[{"title":"Today","items":["Ship"]}]}
"#,
            ),
        ] {
            write(&root, rel, text);
        }

        let report = scan_journal(&root, true).expect("scan every content family");
        assert_eq!(report.indexed, 15);
        let conn = Connection::open(db_path(&root)).expect("open db");
        for rel in [
            "20260101/talents/flow.md",
            "facets/work/events/20260101.jsonl",
            "facets/work/activities/20260101.jsonl",
            "config/actions/20260101.jsonl",
            "20260101/import.ics/imported.jsonl",
            "20260101/import.claude/thread/conversation_transcript.jsonl",
            "20260101/chat/090000_300/chat.jsonl",
            "20260101/default/090000_300/browser_example.jsonl",
            "20260101/talents/pulse.jsonl",
            "facets/work/entities/20260101.jsonl",
            "facets/work/entities/alice/observations.jsonl",
            "20260101/default/090000_300/talents/documents.json",
            "20260101/default/090000_300/talents/screen.json",
            "20260101/default/090000_300/talents/sense.json",
            "20260101/talents/morning_briefing.json",
        ] {
            assert!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM chunks WHERE path='{rel}'")
                ) > 0,
                "{rel} did not produce a chunk"
            );
        }
        fs::remove_dir_all(root).expect("cleanup all families root");
    }

    #[test]
    fn scan_indexes_browser_streams_with_marker_stream() {
        let root = temp_root("browser-stream");
        write_stream(&root, "20260703", "suze.browser", "000141_317");
        write(
            &root,
            "chronicle/20260703/suze.browser/000141_317/browser_mail-google-com.jsonl",
            r#"{"t":"segment_start","ts":1783046501000,"site":"mail.google.com","url":"https://mail.google.com/mail/u/0/#inbox","title":"Inbox - Gmail","adapter":"gmail","blocks":[{"type":"heading","text":"Inbox"},{"type":"row","text":"Ari Patel - Browser stream contract review"},{"type":"link","text":"Open pull request"}]}
{"t":"delta","ts":1783046509120,"op":"add","block":{"type":"row","text":"Status toast: All changes saved"}}
{"t":"segment_start","ts":1783046594000,"url":"https://example.com/fallback","blocks":[{"type":"text","text":"Fallback page text"}]}
{"t":"delta","ts":1783046530100,"op":"remove","block":{"type":"row","text":"Promotions tab collapsed"}}
"#,
        );

        let report = scan_journal(&root, true).expect("scan browser stream");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260703/suze.browser/000141_317/browser_mail-google-com.jsonl'"
            ),
            3
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260703/suze.browser/000141_317/browser_mail-google-com.jsonl'"
            ),
            1
        );
        let row: (String, String, String, String, String, String) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='20260703/suze.browser/000141_317/browser_mail-google-com.jsonl' ORDER BY idx LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("browser metadata row");
        assert_eq!(row.0, "20260703");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "browser");
        assert_eq!(row.3, "suze.browser");
        assert_eq!(row.4, "night");
        assert!(row.5.contains("Inbox - Gmail"));
        assert!(row.5.contains("Ari Patel - Browser stream contract review"));

        let all_content: String = conn
            .prepare(
                "SELECT content FROM chunks WHERE path='20260703/suze.browser/000141_317/browser_mail-google-com.jsonl' ORDER BY idx",
            )
            .expect("prepare browser contents")
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query browser contents")
            .map(|row| row.expect("browser content row"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_content.contains("https://example.com/fallback"));
        assert!(all_content.contains("Fallback page text"));
        assert!(all_content.contains("Status toast: All changes saved"));
        assert!(!all_content.contains("Promotions tab collapsed"));
        fs::remove_dir_all(root).expect("cleanup browser stream root");
    }

    #[test]
    fn scan_indexes_day_accumulator_records_with_file_stem_agent() {
        let root = temp_root("day-accumulator");
        write(
            &root,
            "chronicle/20260304/talents/pulse.jsonl",
            r#"{"ts":10,"summary":"Clear morning","needs_you":[{"text":"Review proposal"}]}
{"title":"Second pulse","detail":"afternoon check"}
"#,
        );

        let report = scan_journal(&root, true).expect("scan day accumulator");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260304/talents/pulse.jsonl'"
            ),
            2
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260304/talents/pulse.jsonl'"
            ),
            1
        );
        let row: (String, String, String, Option<String>, String, String) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='20260304/talents/pulse.jsonl' ORDER BY idx LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("day accumulator row");
        assert_eq!(row.0, "20260304");
        assert_eq!(row.1, "");
        assert_eq!(row.2, "pulse");
        assert_eq!(row.3, None);
        assert_eq!(row.4, "");
        assert!(row.5.contains(r#""summary":"Clear morning""#));
        assert!(row.5.contains(r#""needs_you""#));

        let second: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE path='20260304/talents/pulse.jsonl' AND idx=1",
                [],
                |row| row.get(0),
            )
            .expect("second day accumulator row");
        assert!(second.contains(r#""title":"Second pulse""#));
        assert!(second.contains(r#""detail":"afternoon check""#));
        fs::remove_dir_all(root).expect("cleanup day accumulator root");
    }

    #[test]
    fn scan_indexes_talent_json_families_with_static_agents_and_metadata() {
        let root = temp_root("talent-json");
        write_stream(&root, "20260717", "default", "090000_300");
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/documents.json",
            r#"{"overview":"Trust update.","parties":[{"name":"Priya Shah","role":"trustee"}],"key_provisions":[{"text":"Trustee may distribute assets."}],"assets":[{"name":"Brokerage Account"}],"conditions":[{"trigger":"Settlor's death","effect":"Successor trustee takes office."}],"important_dates":[{"date":"2026-07-17","meaning":"Effective date."}],"summary":"Summary."}"#,
        );
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/screen.json",
            r#"{"narrative":"Viewed the release dashboard.","entities":[{"type":"Tool","name":"Grafana","context":"Latency dashboard."}]}"#,
        );
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/sense.json",
            r#"{"content_type":"meeting","emotional_register":"focused","activity_summary":"Reviewed launch status.","entities":[{"type":"Person","name":"Alice"}],"facets":[{"facet":"work","activity":"launch","level":"high"}],"meeting_detected":true,"speakers":["Alice"]}"#,
        );
        write(
            &root,
            "chronicle/20260717/talents/morning_briefing.json",
            r#"{"metadata":{"coverage_preamble":"Daily briefing."},"your_day":[{"time":"09:00","text":"Meet Alice."}],"yesterday":["Shipped."],"needs_attention":[{"text":"Review."}],"forward_look":["Prepare."],"reading":[{"facet":"work","summary":"News."}]}"#,
        );

        let report = scan_journal(&root, true).expect("scan talent json");
        assert_eq!(report.indexed, 4);
        let conn = Connection::open(db_path(&root)).expect("open db");

        for (path, agent) in [
            (
                "20260717/default/090000_300/talents/documents.json",
                "documents",
            ),
            ("20260717/default/090000_300/talents/screen.json", "screen"),
            ("20260717/default/090000_300/talents/sense.json", "sense"),
        ] {
            let row: (String, String, String, String, String) = conn
                .query_row(
                    "SELECT day, facet, agent, stream, time_bucket FROM chunks WHERE path=? ORDER BY idx LIMIT 1",
                    [path],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .expect("segment talent json metadata row");
            assert_eq!(
                row,
                (
                    "20260717".to_string(),
                    String::new(),
                    agent.to_string(),
                    "default".to_string(),
                    "morning".to_string(),
                ),
                "{path}"
            );
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM files WHERE path='{path}'")
                ),
                1,
                "{path}"
            );
        }

        let morning_row: (String, String, String, Option<String>, String, String) = conn
            .query_row(
                "SELECT day, facet, agent, stream, time_bucket, content FROM chunks WHERE path='20260717/talents/morning_briefing.json' ORDER BY idx LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("morning briefing row");
        assert_eq!(morning_row.0, "20260717");
        assert_eq!(morning_row.1, "");
        assert_eq!(morning_row.2, "morning_briefing");
        assert_eq!(morning_row.3, None);
        assert_eq!(morning_row.4, "");
        assert!(morning_row.5.contains("## Your Day"));
        fs::remove_dir_all(root).expect("cleanup talent json root");
    }

    #[test]
    fn scan_writes_file_rows_for_invalid_or_non_object_talent_json() {
        let root = temp_root("invalid-talent-json");
        for segment in ["090000_300", "100000_300", "110000_300", "120000_300"] {
            write_stream(&root, "20260717", "default", segment);
        }
        let cases = [
            (
                "chronicle/20260717/default/090000_300/talents/documents.json",
                "{",
            ),
            (
                "chronicle/20260717/default/100000_300/talents/screen.json",
                "null",
            ),
            (
                "chronicle/20260717/default/110000_300/talents/sense.json",
                "42",
            ),
            (
                "chronicle/20260717/default/120000_300/talents/documents.json",
                "[]",
            ),
            ("chronicle/20260717/talents/morning_briefing.json", ""),
        ];
        for (path, text) in cases {
            write(&root, path, text);
        }

        let report = scan_journal(&root, true).expect("scan invalid talent json");
        assert_eq!(report.indexed, 5);
        assert_eq!(report.skipped, 0);
        let conn = Connection::open(db_path(&root)).expect("open db");
        for path in [
            "20260717/default/090000_300/talents/documents.json",
            "20260717/default/100000_300/talents/screen.json",
            "20260717/default/110000_300/talents/sense.json",
            "20260717/default/120000_300/talents/documents.json",
            "20260717/talents/morning_briefing.json",
        ] {
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM chunks WHERE path='{path}'")
                ),
                0,
                "{path}"
            );
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM files WHERE path='{path}'")
                ),
                1,
                "{path}"
            );
        }
        fs::remove_dir_all(root).expect("cleanup invalid talent json root");
    }

    #[test]
    fn scan_preserves_empty_object_chunk_counts_by_talent_json_family() {
        let root = temp_root("empty-talent-json");
        write_stream(&root, "20260717", "default", "090000_300");
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/documents.json",
            "{}",
        );
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/screen.json",
            "{}",
        );
        write(
            &root,
            "chronicle/20260717/default/090000_300/talents/sense.json",
            "{}",
        );
        write(
            &root,
            "chronicle/20260717/talents/morning_briefing.json",
            "{}",
        );

        let report = scan_journal(&root, true).expect("scan empty talent json");
        assert_eq!(report.indexed, 4);
        let conn = Connection::open(db_path(&root)).expect("open db");
        for (path, chunks) in [
            ("20260717/default/090000_300/talents/documents.json", 1),
            ("20260717/default/090000_300/talents/screen.json", 1),
            ("20260717/default/090000_300/talents/sense.json", 0),
            ("20260717/talents/morning_briefing.json", 1),
        ] {
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM chunks WHERE path='{path}'")
                ),
                chunks,
                "{path}"
            );
            assert_eq!(
                count(
                    &conn,
                    &format!("SELECT count(*) FROM files WHERE path='{path}'")
                ),
                1,
                "{path}"
            );
        }
        fs::remove_dir_all(root).expect("cleanup empty talent json root");
    }

    #[test]
    fn scan_writes_file_row_for_zero_chunk_chat_stream() {
        let root = temp_root("zero-chat-stream");
        write(
            &root,
            "chronicle/20260508/chat/130000_300/chat.jsonl",
            r#"{"kind":"owner_chat_open","ts":1,"request_id":"req","surface":"convey"}
{"kind":"mystery","ts":2,"text":"skip me"}
"#,
        );

        let report = scan_journal(&root, true).expect("scan zero chat stream");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='20260508/chat/130000_300/chat.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260508/chat/130000_300/chat.jsonl'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup zero chat stream root");
    }

    #[test]
    fn scan_writes_files_rows_for_zero_chunk_jsonl() {
        let root = temp_root("zero-jsonl");
        write(
            &root,
            "facets/work/events/20240101.jsonl",
            r#"{"type":"meeting"}
{"title":""}
"#,
        );
        write(
            &root,
            "facets/work/logs/20240101.jsonl",
            r#"{"actor":"settings"}
{"action":""}
"#,
        );

        let report = scan_journal(&root, true).expect("scan zero jsonl");
        assert_eq!(report.indexed, 2);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/events/20240101.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/logs/20240101.jsonl'"
            ),
            0
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='facets/work/events/20240101.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='facets/work/logs/20240101.jsonl'"
            ),
            1
        );
        fs::remove_dir_all(root).expect("cleanup zero jsonl root");
    }

    #[test]
    fn scan_skips_non_object_jsonl_lines_and_keeps_file_row() {
        let root = temp_root("non-object-jsonl");
        write(
            &root,
            "facets/work/events/20240101.jsonl",
            r#"42
["not", "object"]
not json
{"type":"meeting","title":"Planning"}
"#,
        );

        let report = scan_journal(&root, true).expect("scan non-object jsonl");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM chunks WHERE path='facets/work/events/20240101.jsonl'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='facets/work/events/20240101.jsonl'"
            ),
            1
        );
        let content: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE path='facets/work/events/20240101.jsonl'",
                [],
                |row| row.get(0),
            )
            .expect("event content");
        assert!(content.contains("### Meeting: Planning"));
        fs::remove_dir_all(root).expect("cleanup non-object jsonl root");
    }

    #[test]
    fn short_segment_length_resolves_stream_and_bucket() {
        let root = temp_root("short-segment");
        write(
            &root,
            "chronicle/20260717/default/143022_60/talents/audio.md",
            "# Audio\n\nshort segment",
        );
        write_stream(&root, "20260717", "default", "143022_60");
        let report = scan_journal(&root, true).expect("scan short segment");
        assert_eq!(report.indexed, 1);
        let conn = Connection::open(db_path(&root)).expect("open db");
        let row: (String, String) = conn
            .query_row(
                "SELECT stream, time_bucket FROM chunks WHERE path='20260717/default/143022_60/talents/audio.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("short segment metadata row");
        assert_eq!(row, ("default".to_string(), "afternoon".to_string()));
        fs::remove_dir_all(root).expect("cleanup short segment root");
    }

    #[test]
    fn scan_lowercases_facet_and_agent_at_insert() {
        let root = temp_root("lowercase");
        write(
            &root,
            "apps/MyApp/talents/Digest.md",
            "# Digest\n\napp content",
        );
        write(
            &root,
            "facets/Work/news/20260101.md",
            "# News\n\nfacet content",
        );
        let report = scan_journal(&root, true).expect("scan mixed case");
        assert_eq!(report.indexed, 2);
        let conn = Connection::open(db_path(&root)).expect("open db");
        let app_agent: String = conn
            .query_row(
                "SELECT agent FROM chunks WHERE path='apps/MyApp/talents/Digest.md'",
                [],
                |row| row.get(0),
            )
            .expect("app agent row");
        assert_eq!(app_agent, "myapp:digest");
        let news_row: (String, String) = conn
            .query_row(
                "SELECT facet, agent FROM chunks WHERE path='facets/Work/news/20260101.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("news metadata row");
        assert_eq!(news_row, ("work".to_string(), "news".to_string()));
        fs::remove_dir_all(root).expect("cleanup lowercase root");
    }

    #[test]
    fn scan_runs_a_bounded_fts_merge_budget_until_converged() {
        let root = temp_root("bounded-fts-merge");
        let mut conn = open_index(&root).expect("open fragmented index");
        seed_fragmented_chunks(&mut conn);
        let before = fts_segment_count(&conn);
        assert!(before > 2, "fragmented bed has multiple FTS segments");
        drop(conn);

        let first = scan_journal(&root, false).expect("first bounded merge scan");
        assert_eq!(first.merge_steps_spent, MERGE_BUDGET);
        let mut conn = open_index(&root).expect("open after first merge scan");
        let after_first = fts_segment_count(&conn);
        assert!(after_first < before, "first scan makes structural progress");

        let tx = conn.transaction().expect("start probe merge transaction");
        let changes_before = tx.total_changes();
        tx.execute(
            "INSERT INTO chunks(chunks, rank) VALUES('merge', ?)",
            [MERGE_STEP],
        )
        .expect("probe merge");
        assert!(
            tx.total_changes() - changes_before >= 2,
            "a merge remains after exhausting the first scan budget"
        );
        tx.rollback().expect("discard probe merge");
        drop(conn);

        let second = scan_journal(&root, false).expect("second bounded merge scan");
        assert_eq!(second.merge_steps_spent, MERGE_BUDGET);
        let conn = open_index(&root).expect("open after second merge scan");
        let after_second = fts_segment_count(&conn);
        assert!(
            after_second < after_first,
            "second scan progresses to the floor"
        );
        drop(conn);

        let third = scan_journal(&root, false).expect("converged merge scan");
        assert!(third.merge_steps_spent < MERGE_BUDGET);
        let conn = open_index(&root).expect("open converged index");
        assert_eq!(fts_segment_count(&conn), after_second);
        fs::remove_dir_all(root).expect("cleanup bounded merge root");
    }

    #[test]
    fn merge_failure_is_informational_and_does_not_undo_prior_index_rows() {
        let root = temp_root("merge-failure-isolation");
        write(
            &root,
            "chronicle/20260717/talents/flow.md",
            "# Flow\n\nindexed before merge failure",
        );
        seed_edge_entity(&root, "alice", "Alice");
        seed_edge_entity(&root, "bob", "Bob");
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice","segments":["s1"]}
{"name":"Bob","segments":["s1"]}
"#,
        );
        scan_journal(&root, false).expect("seed indexed rows");

        let mut conn = open_index(&root).expect("open fragmented index");
        seed_fragmented_chunks(&mut conn);
        create_abort_trigger(
            &conn,
            "abort_fts_merge",
            "BEFORE",
            "INSERT",
            "chunks_idx",
            None,
        );
        drop(conn);

        let report = scan_journal(&root, false).expect("merge failure remains nonfatal");
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("correct and merely uncompacted"))
        );
        let conn = Connection::open(db_path(&root)).expect("open after merge failure");
        assert_eq!(
            chunk_content(&conn, "20260717/talents/flow.md"),
            "# Flow\n\nindexed before merge failure"
        );
        assert_eq!(
            chunk_content(&conn, "entity_search:alice"),
            "Alice (Person)"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM files WHERE path='20260717/talents/flow.md'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        drop_trigger(&conn, "abort_fts_merge");
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup merge failure root");
    }

    #[test]
    fn reset_then_full_scan_reindexes_from_empty() {
        let root = temp_root("reset-scan");
        seed_edge_entity(&root, "alice", "Alice Edge");
        seed_edge_entity(&root, "bob", "Bob Edge");
        write(
            &root,
            "chronicle/20260717/talents/flow.md",
            "# Flow\n\nbefore",
        );
        write(
            &root,
            "facets/work/entities/20260304.jsonl",
            r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
        );
        scan_journal(&root, true).expect("initial scan before reset");
        let conn = Connection::open(db_path(&root)).expect("open db before reset");
        assert!(count(&conn, "SELECT count(*) FROM chunks") > 0);
        assert!(count(&conn, "SELECT count(*) FROM edges") > 0);
        drop(conn);

        reset_index(&root).expect("reset index");
        let conn = Connection::open(db_path(&root)).expect("open db after reset");
        assert_eq!(count(&conn, "SELECT count(*) FROM chunks"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM edges"), 0);
        drop(conn);

        let report = scan_journal(&root, true).expect("full scan after reset");
        assert_eq!(report.indexed, 2);
        assert_eq!(report.edges_indexed, 1);
        assert_eq!(report.edge_rows_inserted, 1);
        let conn = Connection::open(db_path(&root)).expect("open db after full rescan");
        assert!(count(&conn, "SELECT count(*) FROM chunks") > 0);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM edges WHERE path='facets/work/entities/20260304.jsonl'"
            ),
            1
        );
        assert_sqlite_and_fts_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup reset root");
    }
}
