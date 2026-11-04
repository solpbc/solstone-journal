// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only search over validated, cached private memory originals.

use std::path::Path;
use std::time::Duration;

use chrono::NaiveDate;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};
use solstone_core_format::agent_memory::{
    ChainPredecessor, Coordinate, Origin, ReadyDocument, SourceKey,
};
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

/// Compare a cached row to an original already validated by the memory reader.
/// Call only with the bytes, origin and readiness from `OriginalRead::Ready`.
/// Cached timestamps compare by instant, and cached JSON compares as typed data.
/// This does not validate originals, authorize access or read journal state.
pub fn memory_row_matches_original(
    row: &MemoryOriginalRow,
    source_key: &SourceKey,
    coordinate: &Coordinate,
    bytes: &[u8],
    origin: &Origin,
    ready: &ReadyDocument,
) -> bool {
    let Ok(cached_origin) = serde_json::from_str::<Origin>(&row.origin_json) else {
        return false;
    };
    let Ok(cached_chain) = serde_json::from_str::<ChainPredecessor>(&row.chain_json) else {
        return false;
    };
    let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(&row.created_at) else {
        return false;
    };
    row.path
        == format!(
            "{}/{}/{}/note.txt",
            coordinate.day, coordinate.stream, coordinate.segment
        )
        && row.day == coordinate.day
        && row.stream == coordinate.stream
        && row.segment == coordinate.segment
        && row.source_key == source_key.as_str()
        && row.bytes == bytes
        && row.digest == ready.digest
        && row.byte_count == bytes.len()
        && row.byte_count == ready.byte_count
        && row.creation_label == origin.creation_label
        && cached_origin == *origin
        && created_at.with_timezone(&chrono::Utc) == ready.created_at
        && cached_chain == ready.chain
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
    report_memory_damage(journal, &connection);
    inspect_own_memory_index(&connection)?;
    Ok(connection)
}

/// The memory readers keep no error text, so this asks the file directly.
/// Reading the build state is what a damaged file fails first here.
fn report_memory_damage(journal: &Path, connection: &Connection) {
    if let Err(error) = read_index_build_state(connection) {
        solstone_core_indexer_store::recovery::report_if_damage(journal, &error);
    }
}

/// Ask for a check when a memory read fails the way a damaged file does.
pub fn report_memory_read_damage(journal: &Path, error: &rusqlite::Error) {
    solstone_core_indexer_store::recovery::report_if_damage_text(journal, &error.to_string());
}

pub fn open_own_memory_connection(
    journal: &Path,
    busy_timeout: Duration,
) -> Result<Connection, OwnMemoryOpenError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Err(OwnMemoryOpenError::Unavailable);
    }
    let connection =
        Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|error| {
            report_memory_read_damage(journal, &error);
            OwnMemoryOpenError::Unavailable
        })?;
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
    use chrono::{NaiveDate, TimeZone, Utc};
    use solstone_core_format::agent_memory::{
        OperationRecord, OriginKind, Readiness, digest, ready_document, validate_complete_original,
    };

    use super::*;

    fn reference_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
    }

    fn original_fixture() -> (MemoryOriginalRow, ReadyDocument) {
        let source = SourceKey::from_verified_id("test connection");
        let bytes = b"a saved memory".to_vec();
        let record = OperationRecord {
            operation_id: "op:1".into(),
            digest: digest(&bytes),
            byte_count: bytes.len(),
            created_at: Utc.with_ymd_and_hms(2026, 9, 1, 3, 4, 5).unwrap(),
            origin_kind: OriginKind::AgentMemory,
            creation_label: "test connection".into(),
            coordinate: Coordinate {
                day: "20260901".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "030405_1".into(),
            },
            phase: Readiness::Chained,
            chain: Some(ChainPredecessor {
                prev_day: None,
                prev_segment: None,
                seq: 1,
            }),
        };
        let ready = ready_document(&record, &source).unwrap();
        validate_complete_original(
            &ready,
            &source,
            &ready.coordinate,
            &ready.origin,
            &ready.chain,
            &bytes,
        )
        .unwrap();
        let row = MemoryOriginalRow {
            path: format!(
                "{}/{}/{}/note.txt",
                ready.coordinate.day, ready.coordinate.stream, ready.coordinate.segment
            ),
            day: ready.coordinate.day.clone(),
            stream: ready.coordinate.stream.clone(),
            segment: ready.coordinate.segment.clone(),
            source_key: source.as_str().into(),
            bytes,
            origin_json: serde_json::to_string(&ready.origin).unwrap(),
            digest: ready.digest.clone(),
            byte_count: ready.byte_count,
            created_at: ready.created_at.to_rfc3339(),
            creation_label: ready.origin.creation_label.clone(),
            chain_json: serde_json::to_string(&ready.chain).unwrap(),
        };
        (row, ready)
    }

    fn matches_original(row: &MemoryOriginalRow, ready: &ReadyDocument) -> bool {
        memory_row_matches_original(
            row,
            &ready.source_key,
            &ready.coordinate,
            b"a saved memory",
            &ready.origin,
            ready,
        )
    }

    #[test]
    fn cached_memory_equality_rejects_each_changed_row_field() {
        let (row, ready) = original_fixture();
        assert!(matches_original(&row, &ready));
        type RowChange = (&'static str, fn(&mut MemoryOriginalRow));
        let changes: [RowChange; 12] = [
            ("path", |row| row.path.push('x')),
            ("day", |row| row.day = "20260902".into()),
            ("stream", |row| row.stream.push('x')),
            ("segment", |row| row.segment = "030406_1".into()),
            ("source_key", |row| {
                row.source_key = SourceKey::from_verified_id("other connection")
                    .as_str()
                    .into();
            }),
            ("bytes", |row| row.bytes[0] = b'A'),
            ("origin_json", |row| {
                let mut origin: Origin = serde_json::from_str(&row.origin_json).unwrap();
                origin.creation_label = "other label".into();
                row.origin_json = serde_json::to_string(&origin).unwrap();
            }),
            ("digest", |row| row.digest = digest(b"other memory")),
            ("byte_count", |row| row.byte_count += 1),
            ("created_at", |row| {
                row.created_at = "2026-09-01T03:04:06Z".into()
            }),
            ("creation_label", |row| {
                row.creation_label = "other label".into()
            }),
            ("chain_json", |row| {
                let mut chain: ChainPredecessor = serde_json::from_str(&row.chain_json).unwrap();
                chain.seq += 1;
                row.chain_json = serde_json::to_string(&chain).unwrap();
            }),
        ];
        for (field, change) in changes {
            let mut changed = row.clone();
            change(&mut changed);
            assert!(!matches_original(&changed, &ready), "changed {field}");
        }
    }

    #[test]
    fn cached_memory_equality_compares_timestamp_instants_and_typed_json() {
        let (mut row, ready) = original_fixture();
        for timestamp in [
            "2026-09-01T03:04:05Z",
            "2026-09-01T03:04:05+00:00",
            "2026-08-31T21:04:05-06:00",
        ] {
            row.created_at = timestamp.into();
            row.origin_json = serde_json::to_string_pretty(&ready.origin).unwrap();
            row.chain_json = serde_json::to_string_pretty(&ready.chain).unwrap();
            assert!(matches_original(&row, &ready), "{timestamp}");
        }
    }

    #[test]
    fn cached_memory_equality_rejects_malformed_cached_metadata() {
        let (row, ready) = original_fixture();
        for field in ["origin", "chain", "created_at"] {
            let mut changed = row.clone();
            match field {
                "origin" => changed.origin_json = "{".into(),
                "chain" => changed.chain_json = "{".into(),
                _ => changed.created_at = "not a timestamp".into(),
            }
            assert!(!matches_original(&changed, &ready), "malformed {field}");
        }
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
