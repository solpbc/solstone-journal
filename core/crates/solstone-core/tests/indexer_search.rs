// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, path::Path};

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use solstone_core_indexer_store::db::{db_path, open_index, reset_index};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    env::temp_dir().join(format!("solstone-core-indexer-search-{name}-{stamp}"))
}

#[allow(clippy::too_many_arguments)]
fn insert(
    connection: &Connection,
    content: &str,
    path: &str,
    day: &str,
    facet: &str,
    agent: &str,
    stream: &str,
    idx: i64,
) {
    connection
        .execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '')",
            params![content, path, day, facet, agent, stream, idx],
        )
        .expect("seed chunk");
}

fn mark_complete(connection: &Connection) {
    let (files_count, chunks_count): (i64, i64) = connection
        .query_row(
            "SELECT (SELECT count(*) FROM files), (SELECT count(*) FROM chunks)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read complete state counts");
    connection
        .execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', ?1, ?2)",
            params![files_count, chunks_count],
        )
        .expect("seed complete state");
}

fn run_indexer_verb(root: &Path, verb: &str, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("indexer")
        .arg(verb)
        .arg("--journal")
        .arg(root)
        .args(args)
        .output()
        .expect("solstone-core should execute")
}

fn json_stdout(output: Output) -> Value {
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stderr).expect("stderr utf-8"), "");
    serde_json::from_slice(&output.stdout).expect("stdout JSON")
}

fn object_keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn indexer_search_reaches_query_engine_and_pins_json_envelope() {
    let root = temp_path("json");
    let connection = open_index(&root).expect("create test index");
    insert(
        &connection,
        "José completed the migration",
        "notes/jose.md",
        "20260102",
        "work",
        "flow",
        "default",
        7,
    );
    mark_complete(&connection);
    drop(connection);

    let response = json_stdout(run_indexer_verb(
        &root,
        "search",
        &["--json", "--counts", "José"],
    ));
    assert_eq!(
        object_keys(&response),
        BTreeSet::from([
            "cleaned_query",
            "counts",
            "order",
            "relaxed",
            "results",
            "total",
        ])
    );
    assert_eq!(response["cleaned_query"], "José");
    assert_eq!(response["order"], "relevance");
    assert_eq!(response["relaxed"], false);
    assert_eq!(response["total"], 1);
    let hit = &response["results"][0];
    assert_eq!(
        object_keys(hit),
        BTreeSet::from(["id", "metadata", "score", "text"])
    );
    assert_eq!(hit["id"], "notes/jose.md:7");
    assert_eq!(hit["text"], "José completed the migration");
    assert!(hit["score"].is_number());
    assert_eq!(
        object_keys(&hit["metadata"]),
        BTreeSet::from(["agent", "day", "facet", "idx", "path", "stream"])
    );
    assert_eq!(
        hit["metadata"],
        json!({
            "day": "20260102",
            "facet": "work",
            "agent": "flow",
            "stream": "default",
            "path": "notes/jose.md",
            "idx": 7,
        })
    );
    assert_eq!(
        object_keys(&response["counts"]),
        BTreeSet::from(["agents", "days", "facets", "relaxed", "streams", "total"])
    );
    assert_eq!(
        response["counts"],
        json!({
            "total": 1,
            "facets": {"work": 1},
            "agents": {"flow": 1},
            "days": {"20260102": 1},
            "streams": {"default": 1},
            "relaxed": false,
        })
    );

    assert_eq!(
        json_stdout(run_indexer_verb(&root, "counts", &["--json", "José"])),
        response["counts"]
    );
    assert_eq!(
        json_stdout(run_indexer_verb(&root, "agents", &["--json"])),
        json!(["flow"])
    );

    let bare = json_stdout(run_indexer_verb(&root, "search", &["--json", "Jos"]));
    assert!(
        bare["results"]
            .as_array()
            .expect("results array")
            .is_empty()
    );
    let non_tokenizable = json_stdout(run_indexer_verb(&root, "search", &["--json", "📅"]));
    assert!(
        non_tokenizable["results"]
            .as_array()
            .expect("results array")
            .is_empty()
    );
    assert_eq!(non_tokenizable["reason"], "not_tokenizable");
    let invalid_order = run_indexer_verb(
        &root,
        "search",
        &["--json", "--order", "unexpected_order", "José"],
    );
    assert_eq!(invalid_order.status.code(), Some(64));
    assert_eq!(
        String::from_utf8(invalid_order.stdout).expect("stdout utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(invalid_order.stderr).expect("stderr utf-8"),
        solstone_core_cli::USAGE
    );
    fs::remove_dir_all(root).expect("cleanup JSON index");
}

#[test]
fn indexer_query_verbs_classify_absent_unreadable_and_empty_indexes() {
    for (kind, expected_reason) in [
        ("absent", "index_absent"),
        ("unreadable", "index_unreadable"),
        ("empty", "empty_index"),
    ] {
        let root = temp_path(kind);
        match kind {
            "unreadable" => {
                fs::create_dir_all(root.join("indexer")).expect("create index directory");
                fs::write(db_path(&root), "not sqlite").expect("write invalid database");
            }
            "empty" => drop(open_index(&root).expect("create empty index")),
            "absent" => {}
            _ => unreachable!(),
        }
        for (verb, args) in [
            ("search", vec!["--json", "needle"]),
            ("counts", vec!["--json", "needle"]),
            ("agents", vec!["--json"]),
            ("coverage", vec!["--json"]),
        ] {
            let output = run_indexer_verb(&root, verb, &args);
            assert_eq!(output.status.code(), Some(69), "{kind} {verb}");
            assert_eq!(String::from_utf8(output.stderr).expect("stderr utf-8"), "");
            let body: Value = serde_json::from_slice(&output.stdout).expect("error JSON");
            assert_eq!(body["error"]["reason"], expected_reason, "{kind} {verb}");
            assert!(body["error"]["message"].is_string());
        }
        if root.exists() {
            fs::remove_dir_all(root).expect("cleanup error index");
        }
    }
}

#[test]
fn interrupted_full_rescan_reports_building_state_after_reopen() {
    let root = temp_path("interrupted-rescan");
    reset_index(&root).expect("reset index into building state");
    let connection = open_index(&root).expect("open reset index");
    insert(
        &connection,
        "needle retained before the interrupted rescan",
        "notes/retained.md",
        "20260102",
        "work",
        "flow",
        "default",
        0,
    );
    connection
        .execute(
            "CREATE TRIGGER abort_full_rescan BEFORE INSERT ON files \
             WHEN NEW.path='20260717/talents/flow.md' \
             BEGIN SELECT RAISE(ABORT, 'abort_full_rescan'); END",
            [],
        )
        .expect("create full-rescan abort trigger");
    drop(connection);
    let source = root.join("chronicle/20260717/talents/flow.md");
    fs::create_dir_all(source.parent().expect("source parent")).expect("create source parent");
    fs::write(source, "# Flow\n\nthis rescan is interrupted").expect("write scan source");

    let failed = Command::new(bin())
        .args(["indexer", "--journal"])
        .arg(&root)
        .arg("--rescan-full")
        .output()
        .expect("run interrupted full rescan");
    assert!(!failed.status.success(), "full rescan should fail");
    assert!(
        String::from_utf8(failed.stderr)
            .expect("stderr utf-8")
            .contains("abort_full_rescan"),
        "full rescan reports the trigger failure"
    );

    let response = json_stdout(run_indexer_verb(&root, "search", &["--json", "needle"]));
    assert_eq!(response["degraded"]["kind"], "building");
    fs::remove_dir_all(root).expect("cleanup interrupted rescan index");
}

#[test]
fn indexer_coverage_and_filter_only_search_are_reachable_through_the_binary() {
    let root = temp_path("coverage");
    let connection = open_index(&root).expect("create test index");
    insert(
        &connection,
        "dated browse row",
        "notes/dated.md",
        "20260102",
        "work",
        "flow",
        "default",
        1,
    );
    insert(
        &connection,
        "undated row",
        "notes/undated.md",
        "",
        "",
        "flow",
        "",
        2,
    );
    mark_complete(&connection);
    drop(connection);

    let filter_only = json_stdout(run_indexer_verb(
        &root,
        "search",
        &["--json", "--day-from", "20260102", "--day-to", "20260102"],
    ));
    assert_eq!(filter_only["order"], "recency");
    assert_eq!(filter_only["results"][0]["id"], "notes/dated.md:1");
    assert_eq!(
        json_stdout(run_indexer_verb(&root, "coverage", &["--json"])),
        json!({"state": "available", "start": "20260102", "end": "20260102"})
    );
    fs::remove_dir_all(root).expect("cleanup dated index");

    let undated_root = temp_path("undated-coverage");
    let connection = open_index(&undated_root).expect("create undated index");
    insert(
        &connection,
        "undated only",
        "notes/undated.md",
        "",
        "",
        "flow",
        "",
        0,
    );
    mark_complete(&connection);
    drop(connection);
    assert_eq!(
        json_stdout(run_indexer_verb(&undated_root, "coverage", &["--json"])),
        json!({"state": "no_dated_chunks", "start": null, "end": null})
    );
    fs::remove_dir_all(undated_root).expect("cleanup undated index");
}
