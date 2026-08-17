// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::types::Value;
use rusqlite::{Connection, Row, params};
use sha2::{Digest, Sha256};
use solstone_core_indexer::edges::discovery::discover_edge_files;

use crate::StoreError;
use crate::db::open_index;
use crate::scan::rebuild_edges;

const EDGE_COLUMNS: [&str; 14] = [
    "src", "dst", "kind", "directed", "src_name", "dst_name", "day", "facet", "source", "path",
    "anchor", "label", "ts", "weight",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityEdgeFoldReport {
    pub rows_folded: usize,
    pub self_edges_dropped: usize,
    pub fallback_rebuild: bool,
}

pub fn fold_entity_edges_for_recorded_merge(
    journal: &Path,
    source_id: &str,
    target_id: &str,
) -> Result<EntityEdgeFoldReport, StoreError> {
    let mut conn = open_index(journal)?;
    let fallback_rebuild = edge_rows_all_from_sources(&conn, journal)?;
    let transaction = conn.transaction()?;
    let rows_folded = transaction.query_row(
        "SELECT COUNT(*) FROM edges WHERE src=? OR dst=?",
        params![source_id, source_id],
        edge_count,
    )?;
    transaction.execute(
        "UPDATE edges SET src=? WHERE src=?",
        params![target_id, source_id],
    )?;
    transaction.execute(
        "UPDATE edges SET dst=? WHERE dst=?",
        params![target_id, source_id],
    )?;
    let self_edges_dropped = transaction.query_row(
        "SELECT COUNT(*) FROM edges WHERE src=dst AND src=?",
        [target_id],
        edge_count,
    )?;
    transaction.execute("DELETE FROM edges WHERE src=dst AND src=?", [target_id])?;
    transaction.execute(
        "UPDATE edges
         SET src=dst, dst=src, src_name=dst_name, dst_name=src_name
         WHERE directed=0 AND src>dst AND (src=? OR dst=?)",
        params![target_id, target_id],
    )?;
    transaction.commit()?;
    drop(conn);

    if fallback_rebuild {
        let rebuild = rebuild_edges(journal)?;
        if rebuild.failed > 0 {
            return Err(StoreError::EdgeRebuildFailed(rebuild));
        }
    }
    Ok(EntityEdgeFoldReport {
        rows_folded,
        self_edges_dropped,
        fallback_rebuild,
    })
}

pub fn rebuild_edges_for_recorded_merge_undo(journal: &Path) -> Result<String, StoreError> {
    let rebuild = rebuild_edges(journal)?;
    if rebuild.failed > 0 {
        return Err(StoreError::EdgeRebuildFailed(rebuild));
    }
    fingerprint_edge_rows(journal)
}

pub fn fingerprint_edge_rows(journal: &Path) -> Result<String, StoreError> {
    let conn = open_index(journal)?;
    let columns = EDGE_COLUMNS.join(", ");
    let mut statement = conn.prepare(&format!("SELECT {columns} FROM edges ORDER BY {columns}"))?;
    let mut rows = statement.query([])?;
    let mut payload = String::from("[");
    let mut first_row = true;
    while let Some(row) = rows.next()? {
        if !first_row {
            payload.push(',');
        }
        first_row = false;
        payload.push('[');
        for index in 0..EDGE_COLUMNS.len() {
            if index > 0 {
                payload.push(',');
            }
            append_json_value(&mut payload, row.get(index)?)?;
        }
        payload.push(']');
    }
    payload.push(']');
    Ok(format!("{:x}", Sha256::digest(payload.as_bytes())))
}

fn edge_rows_all_from_sources(conn: &Connection, journal: &Path) -> Result<bool, StoreError> {
    let mut statement = conn.prepare("SELECT DISTINCT path FROM edges")?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if paths.is_empty() {
        return Ok(false);
    }
    let discovered = discover_edge_files(journal)?;
    Ok(paths.iter().all(|path| discovered.contains_key(path)))
}

fn edge_count(row: &Row<'_>) -> rusqlite::Result<usize> {
    let count = row.get::<_, i64>(0)?;
    usize::try_from(count).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(0, count))
}

fn append_json_value(output: &mut String, value: Value) -> Result<(), StoreError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Integer(value) => write!(output, "{value}").expect("writing to string cannot fail"),
        Value::Real(value) if value.is_nan() => output.push_str("NaN"),
        Value::Real(value) if value.is_infinite() && value.is_sign_positive() => {
            output.push_str("Infinity");
        }
        Value::Real(value) if value.is_infinite() => output.push_str("-Infinity"),
        Value::Real(value) => write!(output, "{value}").expect("writing to string cannot fail"),
        Value::Text(value) => append_json_string(output, &value),
        Value::Blob(_) => {
            return Err(StoreError::EdgeFileFailed(
                "edge row contains an unsupported blob value".to_owned(),
            ));
        }
    }
    Ok(())
}

fn append_json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0C}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to string cannot fail");
            }
            character if character.is_ascii() => output.push(character),
            character if (character as u32) <= 0xFFFF => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to string cannot fail");
            }
            character => {
                let scalar = character as u32 - 0x1_0000;
                let high = 0xD800 + (scalar >> 10);
                let low = 0xDC00 + (scalar & 0x3FF);
                write!(output, "\\u{high:04x}\\u{low:04x}").expect("writing to string cannot fail");
            }
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::db::open_index;
    use crate::test_support::reserve_temp_path;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-store-merge-{name}"))
    }

    fn insert_edge(
        conn: &Connection,
        src: &str,
        dst: &str,
        directed: i64,
        src_name: &str,
        dst_name: &str,
        path: &str,
    ) {
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, src_name, dst_name, source, path, weight) VALUES (?, ?, 'works-with', ?, ?, ?, 'observation', ?, 1)",
            params![src, dst, directed, src_name, dst_name, path],
        )
        .expect("seed edge");
    }

    #[test]
    fn folds_edges_drops_self_edges_and_recanonicalizes_undirected_rows() {
        let root = temp_root("fold");
        let conn = open_index(&root).expect("open index");
        insert_edge(&conn, "source", "target", 0, "Source", "Target", "manual");
        insert_edge(&conn, "source", "alpha", 0, "Source", "Alpha", "manual");
        drop(conn);

        let report =
            fold_entity_edges_for_recorded_merge(&root, "source", "target").expect("fold edges");
        assert_eq!(report.rows_folded, 2);
        assert_eq!(report.self_edges_dropped, 1);
        assert!(!report.fallback_rebuild);
        let conn = open_index(&root).expect("reopen index");
        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT src, dst, src_name, dst_name FROM edges",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("folded row");
        assert_eq!(
            row,
            (
                "alpha".to_owned(),
                "target".to_owned(),
                "Alpha".to_owned(),
                "Source".to_owned()
            )
        );
        fs::remove_dir_all(root).expect("cleanup root");
    }

    #[test]
    fn rebuilds_when_all_current_rows_are_from_discovered_sources() {
        let root = temp_root("fallback");
        let source = root.join("facets/work/entities/source/observations.jsonl");
        fs::create_dir_all(source.parent().expect("source parent")).expect("create source parent");
        fs::write(&source, "").expect("write source");
        let conn = open_index(&root).expect("open index");
        insert_edge(
            &conn,
            "source",
            "target",
            0,
            "Source",
            "Target",
            "facets/work/entities/source/observations.jsonl",
        );
        drop(conn);

        let report = fold_entity_edges_for_recorded_merge(&root, "source", "target")
            .expect("fold and rebuild edges");

        assert!(report.fallback_rebuild);
        let conn = open_index(&root).expect("reopen index");
        let count = conn
            .query_row("SELECT COUNT(*) FROM edges", [], edge_count)
            .expect("edge count");
        assert_eq!(count, 0);
        fs::remove_dir_all(root).expect("cleanup root");
    }

    #[test]
    fn fingerprints_rows_in_total_column_order_with_ascii_json() {
        let root = temp_root("fingerprint");
        let conn = open_index(&root).expect("open index");
        insert_edge(&conn, "b", "a", 0, "Béla", "Alice", "manual-b");
        insert_edge(&conn, "a", "b", 1, "Alice", "Béla", "manual-a");
        drop(conn);

        let fingerprint = fingerprint_edge_rows(&root).expect("fingerprint");
        let payload = "[[\"a\",\"b\",\"works-with\",1,\"Alice\",\"B\\u00e9la\",null,null,\"observation\",\"manual-a\",null,null,null,1],[\"b\",\"a\",\"works-with\",0,\"B\\u00e9la\",\"Alice\",null,null,\"observation\",\"manual-b\",null,null,null,1]]";
        assert_eq!(
            fingerprint,
            format!("{:x}", Sha256::digest(payload.as_bytes()))
        );
        fs::remove_dir_all(root).expect("cleanup root");
    }
}
