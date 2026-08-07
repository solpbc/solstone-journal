// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde_json::{Value, json};
use solstone_core_indexer_store::db::open_index;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("store crate should be nested below repo root")
        .to_path_buf()
}

fn temp_root() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-schema-drift-{stamp}"))
}

fn run_python(root: &Path, script: &str) -> std::io::Result<Output> {
    let venv_python = root.join(".venv/bin/python3");
    let python = if venv_python.exists() {
        venv_python
    } else {
        PathBuf::from("python3")
    };
    Command::new(python)
        .current_dir(root)
        .env("PYTHONPATH", root)
        .arg("-c")
        .arg(script)
        .output()
}

fn table_schema_json(conn: &Connection, table: &str) -> Value {
    let columns = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare table_info")
        .query_map([], |row| {
            Ok(json!({
                "name": row.get::<_, String>(1)?,
                "type": row.get::<_, String>(2)?,
                "notnull": row.get::<_, i64>(3)?,
                "pk": row.get::<_, i64>(5)?,
            }))
        })
        .expect("query table_info")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect table_info");
    let mut indexes = conn
        .prepare(&format!("PRAGMA index_list({table})"))
        .expect("prepare index_list")
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .expect("query index_list")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect index_list")
        .into_iter()
        .map(|(name, unique, origin)| {
            let columns = conn
                .prepare(&format!("PRAGMA index_info({name})"))
                .expect("prepare index_info")
                .query_map([], |row| row.get::<_, String>(2))
                .expect("query index_info")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect index_info");
            json!({
                "name": name,
                "unique": unique,
                "origin": origin,
                "columns": columns,
            })
        })
        .collect::<Vec<_>>();
    indexes.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    json!({"columns": columns, "indexes": indexes})
}

fn normalized_chunks_ddl(conn: &Connection) -> String {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='chunks'",
            [],
            |row| row.get(0),
        )
        .expect("chunks DDL");
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[test]
fn native_and_python_index_schemas_match() {
    let repo = repo_root();
    let probe = match run_python(&repo, "import solstone.think.indexer.journal") {
        Ok(probe) if probe.status.success() => probe,
        Ok(probe) => {
            eprintln!(
                "skipping schema drift test: python3 cannot import local journal module: {}",
                String::from_utf8_lossy(&probe.stderr).trim()
            );
            return;
        }
        Err(error) => {
            eprintln!("skipping schema drift test: python3 unavailable: {error}");
            return;
        }
    };
    drop(probe);

    let root = temp_root();
    let conn = open_index(&root).expect("materialize native schema");
    let native = json!({
        "files": table_schema_json(&conn, "files"),
        "edge_files": table_schema_json(&conn, "edge_files"),
        "edges": table_schema_json(&conn, "edges"),
        "chunks_sql": normalized_chunks_ddl(&conn),
    });
    drop(conn);

    let python = run_python(
        &repo,
        r#"
import json
import sqlite3
from solstone.think.indexer.journal import _ensure_schema

conn = sqlite3.connect(":memory:")
_ensure_schema(conn)

def table_schema(name):
    columns = [
        {"name": row[1], "type": row[2], "notnull": row[3], "pk": row[5]}
        for row in conn.execute(f"PRAGMA table_info({name})")
    ]
    indexes = []
    for row in conn.execute(f"PRAGMA index_list({name})"):
        index_name, unique, origin = row[1], row[2], row[3]
        indexes.append({
            "name": index_name,
            "unique": unique,
            "origin": origin,
            "columns": [column[2] for column in conn.execute(f"PRAGMA index_info({index_name})")],
        })
    indexes.sort(key=lambda item: item["name"])
    return {"columns": columns, "indexes": indexes}

chunks_sql = conn.execute("SELECT sql FROM sqlite_master WHERE name='chunks'").fetchone()[0]
print(json.dumps({
    "files": table_schema("files"),
    "edge_files": table_schema("edge_files"),
    "edges": table_schema("edges"),
    "chunks_sql": " ".join(chunks_sql.split()).lower(),
}))
"#,
    )
    .expect("run python schema materialization");
    assert!(
        python.status.success(),
        "python schema materialization failed: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let python: Value = serde_json::from_slice(&python.stdout).expect("parse python schema JSON");

    for table in ["files", "edge_files", "edges", "chunks_sql"] {
        assert_eq!(
            native[table], python[table],
            "native and Python schema differ for {table}"
        );
    }
    fs::remove_dir_all(root).expect("cleanup schema drift root");
}
