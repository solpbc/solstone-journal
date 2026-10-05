// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only search over validated, cached private memory originals.

use std::path::Path;
use std::time::Duration;

use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};
use solstone_core_indexer_store::db::{IndexBuildLifecycle, db_path, read_index_build_state};

use crate::compile::{CompileOutcome, compile_query};
use crate::predicate::{EffectiveDateConstraint, PredicateInput, QueryPredicate};
use crate::types::QueryBoundary;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnMemoryOpenError {
    Pending,
    Unavailable,
}

/// Cached descriptor and exact bytes for one indexed original.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryOriginalRow {
    pub path: String,
    pub day: String,
    pub stream: String,
    pub segment: String,
    pub source_key: String,
    pub bytes: Vec<u8>,
    pub origin_json: String,
    pub digest: String,
    pub byte_count: usize,
    pub created_at: String,
    pub creation_label: String,
    pub chain_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OwnMemoryQueryMode {
    Browse,
    Terms(String),
    NoSearchTerms,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnMemoryQuery {
    pub mode: OwnMemoryQueryMode,
    pub predicate: QueryPredicate,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OwnMemoryDateFilters {
    pub day: Option<String>,
    pub day_from: Option<String>,
    pub day_to: Option<String>,
}

/// Compile one private-recall query. An omitted query is a browse; any
/// supplied string, including whitespace, is sent through the shared compiler.
pub fn compile_own_memory_query(
    query: Option<&str>,
    filters: OwnMemoryDateFilters,
    reference_date: NaiveDate,
) -> OwnMemoryQuery {
    let compilation = compile_query(query.unwrap_or_default(), reference_date);
    let mode = match query {
        None => OwnMemoryQueryMode::Browse,
        Some(_) => match &compilation.outcome {
            CompileOutcome::Compiled { expression } => {
                OwnMemoryQueryMode::Terms(expression.clone())
            }
            CompileOutcome::FiltersOnly => OwnMemoryQueryMode::Browse,
            CompileOutcome::NoInput | CompileOutcome::NoTokenizableTerm => {
                OwnMemoryQueryMode::NoSearchTerms
            }
        },
    };
    let reason =
        matches!(mode, OwnMemoryQueryMode::NoSearchTerms).then_some("query_has_no_search_terms");
    let predicate = QueryPredicate::new(
        compilation.outcome,
        &compilation.temporal,
        PredicateInput {
            day: filters.day,
            day_from: filters.day_from,
            day_to: filters.day_to,
            ..PredicateInput::default()
        },
    );
    OwnMemoryQuery {
        mode,
        predicate,
        reason,
    }
}

/// Open a structurally complete, read-only index without requiring any FTS
/// chunks and without creating schema or database files.
pub fn open_own_memory_index(
    journal: &Path,
    busy_timeout: Duration,
) -> Result<Connection, OwnMemoryOpenError> {
    let connection = open_own_memory_connection(journal, busy_timeout)?;
    inspect_own_memory_index(&connection)?;
    Ok(connection)
}

pub fn open_own_memory_connection(
    journal: &Path,
    busy_timeout: Duration,
) -> Result<Connection, OwnMemoryOpenError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Err(OwnMemoryOpenError::Unavailable);
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| OwnMemoryOpenError::Unavailable)?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|_| OwnMemoryOpenError::Unavailable)?;
    Ok(connection)
}

pub fn inspect_own_memory_index(connection: &Connection) -> Result<(), OwnMemoryOpenError> {
    let state = read_index_build_state(connection).map_err(|_| OwnMemoryOpenError::Unavailable)?;
    if !state.is_some_and(|state| state.state == IndexBuildLifecycle::Complete) {
        return Err(OwnMemoryOpenError::Unavailable);
    }
    if !table_exists(connection, "files")? || !table_exists(connection, "chunks")? {
        return Err(OwnMemoryOpenError::Unavailable);
    }
    for statement in [
        "SELECT path, mtime FROM files LIMIT 0",
        "SELECT content, path, stream FROM chunks LIMIT 0",
    ] {
        connection
            .prepare(statement)
            .map_err(|_| OwnMemoryOpenError::Unavailable)?;
    }
    if !table_exists(connection, "memory_originals")? {
        return Err(OwnMemoryOpenError::Pending);
    }
    connection
        .prepare(
            "SELECT path, day, stream, segment, source_key, bytes, origin_json, digest, byte_count, created_at, creation_label, chain_json FROM memory_originals LIMIT 0",
        )
        .map_err(|_| OwnMemoryOpenError::Unavailable)?;
    Ok(())
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, OwnMemoryOpenError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
            [name],
            |row| row.get(0),
        )
        .map_err(|_| OwnMemoryOpenError::Unavailable)
}

/// Fetch an ordered examination batch. The caller retains the connection so
/// its interrupt handle can cancel an executing SQLite statement.
pub fn own_memory_candidates(
    connection: &Connection,
    boundary: &QueryBoundary,
    query: &OwnMemoryQuery,
    anchor: Option<(&str, &str)>,
    inclusive_anchor: bool,
    limit: usize,
) -> rusqlite::Result<Vec<MemoryOriginalRow>> {
    let Some((sql, values)) =
        own_memory_candidate_statement(boundary, query, anchor, inclusive_anchor, limit)
    else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(&sql)?;
    statement
        .query_map(params_from_iter(values.iter()), memory_row_from_sql)?
        .collect()
}

pub fn own_memory_candidate_statement(
    boundary: &QueryBoundary,
    query: &OwnMemoryQuery,
    anchor: Option<(&str, &str)>,
    inclusive_anchor: bool,
    limit: usize,
) -> Option<(String, Vec<Value>)> {
    let QueryBoundary::OwnMemory { source_key } = boundary else {
        return None;
    };
    if matches!(&query.mode, OwnMemoryQueryMode::NoSearchTerms) || limit == 0 {
        return None;
    }
    let stream = format!(
        "agent-memory-{}",
        source_key.strip_prefix("sha256:").unwrap_or("")
    );
    let mut sql = String::from("SELECT ");
    if matches!(&query.mode, OwnMemoryQueryMode::Terms(_)) {
        sql.push_str("DISTINCT ");
    }
    sql.push_str("mo.path, mo.day, mo.stream, mo.segment, mo.source_key, mo.bytes, mo.origin_json, mo.digest, mo.byte_count, mo.created_at, mo.creation_label, mo.chain_json FROM memory_originals mo ");
    if matches!(&query.mode, OwnMemoryQueryMode::Terms(_)) {
        sql.push_str("JOIN (SELECT DISTINCT path, stream FROM chunks WHERE chunks MATCH ?) matched ON matched.path=mo.path AND matched.stream=mo.stream ");
    }
    sql.push_str("WHERE mo.source_key=? AND mo.stream=?");
    let mut values = Vec::new();
    if let OwnMemoryQueryMode::Terms(expression) = &query.mode {
        values.push(Value::Text(expression.clone()));
    }
    values.extend([Value::Text(source_key.clone()), Value::Text(stream)]);
    append_date_predicate(&mut sql, &mut values, query);
    if let Some((day, path)) = anchor {
        sql.push_str(if inclusive_anchor {
            " AND (mo.day < ? OR (mo.day = ? AND mo.path <= ?))"
        } else {
            " AND (mo.day < ? OR (mo.day = ? AND mo.path < ?))"
        });
        values.extend([
            Value::Text(day.to_owned()),
            Value::Text(day.to_owned()),
            Value::Text(path.to_owned()),
        ]);
    }
    sql.push_str(" ORDER BY mo.day DESC, mo.path DESC LIMIT ?");
    values.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
    Some((sql, values))
}

pub fn read_own_memory_row(
    connection: &Connection,
    source_key: &str,
    path: &str,
) -> rusqlite::Result<Option<MemoryOriginalRow>> {
    connection
        .query_row(
            "SELECT path, day, stream, segment, source_key, bytes, origin_json, digest, byte_count, created_at, creation_label, chain_json FROM memory_originals WHERE path=? AND source_key=?",
            [path, source_key],
            memory_row_from_sql,
        )
        .optional()
}

fn memory_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryOriginalRow> {
    // Inspect borrowed SQLite values before allocating cached originals.
    for column in 0..12 {
        let value = row.get_ref(column)?;
        let maximum = match column {
            5 => solstone_core_format::agent_memory::MAX_NOTE_BYTES,
            6 | 11 => solstone_core_format::agent_memory::MAX_METADATA_BYTES,
            _ => 1024,
        };
        let length = match value {
            rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
                bytes.len()
            }
            _ => 0,
        };
        if length > maximum {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                column,
                value.data_type(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "memory cache value exceeds its bound",
                )
                .into(),
            ));
        }
    }
    let byte_count: i64 = row.get(8)?;
    Ok(MemoryOriginalRow {
        path: row.get(0)?,
        day: row.get(1)?,
        stream: row.get(2)?,
        segment: row.get(3)?,
        source_key: row.get(4)?,
        bytes: row.get(5)?,
        origin_json: row.get(6)?,
        digest: row.get(7)?,
        byte_count: usize::try_from(byte_count).unwrap_or_default(),
        created_at: row.get(9)?,
        creation_label: row.get(10)?,
        chain_json: row.get(11)?,
    })
}

fn append_date_predicate(sql: &mut String, values: &mut Vec<Value>, query: &OwnMemoryQuery) {
    match &query.predicate.effective_date {
        EffectiveDateConstraint::None => {}
        EffectiveDateConstraint::Exact(day) => {
            sql.push_str(" AND mo.day=?");
            values.push(Value::Text(day.clone()));
        }
        EffectiveDateConstraint::Range { day_from, day_to } => {
            if let Some(day_from) = day_from {
                sql.push_str(" AND mo.day>=?");
                values.push(Value::Text(day_from.clone()));
            }
            if let Some(day_to) = day_to {
                sql.push_str(" AND mo.day<=?");
                values.push(Value::Text(day_to.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn reference_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
    }

    #[test]
    fn own_memory_query_distinguishes_browse_from_supplied_empty_queries() {
        assert_eq!(
            compile_own_memory_query(None, OwnMemoryDateFilters::default(), reference_date()).mode,
            OwnMemoryQueryMode::Browse
        );
        for supplied in ["", "  \t", "📅📆"] {
            let compiled = compile_own_memory_query(
                Some(supplied),
                OwnMemoryDateFilters::default(),
                reference_date(),
            );
            assert_eq!(compiled.mode, OwnMemoryQueryMode::NoSearchTerms);
            assert_eq!(compiled.reason, Some("query_has_no_search_terms"));
        }
    }

    #[test]
    fn own_memory_filters_only_is_a_date_browse() {
        let compiled = compile_own_memory_query(
            Some("yesterday"),
            OwnMemoryDateFilters::default(),
            reference_date(),
        );
        assert_eq!(compiled.mode, OwnMemoryQueryMode::Browse);
        assert!(matches!(
            compiled.predicate.effective_date,
            EffectiveDateConstraint::Range { .. }
        ));
    }

    #[test]
    fn own_memory_keyset_has_no_row_identifier_and_supports_inclusive_anchor() {
        let boundary = QueryBoundary::OwnMemory {
            source_key: format!("sha256:{}", "a".repeat(64)),
        };
        let query =
            compile_own_memory_query(None, OwnMemoryDateFilters::default(), reference_date());
        let (inclusive, _) = own_memory_candidate_statement(
            &boundary,
            &query,
            Some(("20260901", "20260901/agent-memory-x/segment/note.txt")),
            true,
            5,
        )
        .unwrap();
        let (exclusive, _) = own_memory_candidate_statement(
            &boundary,
            &query,
            Some(("20260901", "20260901/agent-memory-x/segment/note.txt")),
            false,
            5,
        )
        .unwrap();
        assert!(inclusive.contains("mo.path <= ?"));
        assert!(exclusive.contains("mo.path < ?"));
        assert!(!inclusive.contains("rowid"));
    }
}
