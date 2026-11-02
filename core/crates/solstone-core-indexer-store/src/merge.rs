// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write as _;
use std::path::Path;

use rusqlite::types::Value;
use sha2::{Digest, Sha256};

use crate::StoreError;
use crate::db::open_index_reader;

const EDGE_COLUMNS: [&str; 14] = [
    "src", "dst", "kind", "directed", "src_name", "dst_name", "day", "facet", "source", "path",
    "anchor", "label", "ts", "weight",
];

pub fn fingerprint_edge_rows(journal: &Path) -> Result<String, StoreError> {
    let conn = open_index_reader(journal)?;
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
