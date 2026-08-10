// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs, path::Path};

use solstone_core_indexer_store::db::open_index;

const EXPECTED_ZERO_EDGE_HINT: &str = "Zero edges indexed: edges are talent-derived, and the --rescan-full edge phase remains modification-time incremental — run journal indexer --rebuild-edges to force full edge re-extraction.";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    env::temp_dir().join(format!("solstone-core-indexer-{name}-{stamp}"))
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("test path should have parent"))
        .expect("create parent");
    fs::write(path, text).expect("write test file");
}

fn run_indexer(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(root)
        .args(args)
        .output()
        .expect("solstone-core should execute")
}

fn run_indexer_verb(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("indexer")
        .args(args)
        .arg("--journal")
        .arg(root)
        .output()
        .expect("solstone-core should execute")
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

#[test]
fn indexer_without_operation_prints_usage_to_stdout_and_exits_zero() {
    let output = Command::new(bin())
        .arg("indexer")
        .env_remove("SOLSTONE_JOURNAL")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        solstone_core_cli::USAGE
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
}

#[test]
fn indexer_rescan_full_succeeds_for_tiny_journal() {
    let root = temp_path("success");
    write(
        &root,
        "chronicle/20260717/talents/flow.md",
        "# Flow\n\nindexed",
    );

    let output = run_indexer(&root, &["--rescan-full"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        format!("{EXPECTED_ZERO_EDGE_HINT}\n")
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    assert!(root.join("indexer/journal.sqlite").is_file());
    fs::remove_dir_all(root).expect("cleanup success root");
}

#[test]
fn indexer_rescan_full_suppresses_zero_edge_hint_for_nonzero_rebuild_and_reset_cases() {
    let nonzero_root = temp_path("nonzero-edge-no-hint");
    seed_edge_entity(&nonzero_root, "alice", "Alice Edge");
    seed_edge_entity(&nonzero_root, "bob", "Bob Edge");
    write(
        &nonzero_root,
        "facets/work/entities/20260717.jsonl",
        r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
    );

    let output = run_indexer(&nonzero_root, &["--rescan-full"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    fs::remove_dir_all(nonzero_root).expect("cleanup nonzero root");

    let rebuild_root = temp_path("rebuild-suppression");
    write(
        &rebuild_root,
        "chronicle/20260717/talents/flow.md",
        "# Flow\n\nindexed",
    );
    let output = run_indexer(&rebuild_root, &["--rebuild-edges", "--rescan-full"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    fs::remove_dir_all(rebuild_root).expect("cleanup rebuild root");

    let reset_root = temp_path("reset-suppression");
    write(
        &reset_root,
        "chronicle/20260717/talents/flow.md",
        "# Flow\n\nindexed",
    );
    let output = run_indexer(&reset_root, &["--reset", "--rescan-full"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        ""
    );
    fs::remove_dir_all(reset_root).expect("cleanup reset root");
}

#[test]
fn indexer_scan_edge_failure_warns_and_exits_zero() {
    let root = temp_path("scan-edge-failure");
    seed_edge_entity(&root, "alice", "Alice Edge");
    seed_edge_entity(&root, "bob", "Bob Edge");
    write(
        &root,
        "facets/work/entities/20260230.jsonl",
        r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
    );

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rescan-full")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be utf-8")
            .contains("warning: Skipping edge extraction for facets/work/entities/20260230.jsonl")
    );
    fs::remove_dir_all(root).expect("cleanup scan edge failure root");
}

#[test]
fn indexer_write_failure_exits_tempfail() {
    let root = temp_path("write-failure");
    fs::write(&root, "not a dir").expect("write file journal path");

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rescan-full")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(75));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be utf-8")
            .starts_with("indexer scan failed: ")
    );
    fs::remove_file(root).expect("cleanup write failure path");
}

#[test]
fn indexer_busy_timeout_exits_tempfail_without_partial_commit() {
    let root = temp_path("busy-timeout");
    write(
        &root,
        "chronicle/20260717/talents/flow.md",
        "# Flow\n\nblocked",
    );
    let lock_conn = open_index(&root).expect("open index");
    lock_conn
        .execute_batch("BEGIN EXCLUSIVE")
        .expect("hold exclusive lock");

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rescan-full")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(75));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be utf-8")
            .starts_with("indexer scan failed: ")
    );
    lock_conn.execute_batch("ROLLBACK").expect("release lock");
    assert_eq!(
        lock_conn
            .query_row("SELECT count(*) FROM chunks", [], |row| row
                .get::<_, i64>(0))
            .expect("chunk count"),
        0
    );
    drop(lock_conn);
    fs::remove_dir_all(root).expect("cleanup busy timeout root");
}

#[test]
fn indexer_rescan_file_conflict_exits_usage() {
    let output = Command::new(bin())
        .arg("indexer")
        .arg("--rescan-file")
        .arg("20260717/talents/flow.md")
        .arg("--rescan")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        solstone_core_cli::USAGE
    );
}

#[test]
fn indexer_unsupported_rescan_file_exits_declined() {
    let root = temp_path("declined");
    write(&root, "notes/foo.txt", "unsupported\n");

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rescan-file")
        .arg("notes/foo.txt")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(69));
    assert_eq!(
        String::from_utf8(output.stdout).expect("stdout should be utf-8"),
        ""
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("stderr should be utf-8"),
        "indexer declined unsupported file\n"
    );
    assert!(!root.join("indexer/journal.sqlite").exists());
    fs::remove_dir_all(root).expect("cleanup declined root");
}

#[test]
fn indexer_rescan_file_edge_failure_exits_tempfail() {
    let root = temp_path("rescan-file-edge-failure");
    seed_edge_entity(&root, "alice", "Alice Edge");
    seed_edge_entity(&root, "bob", "Bob Edge");
    write(
        &root,
        "facets/work/entities/20260230.jsonl",
        r#"{"name":"Alice Edge","segments":["s1"]}
{"name":"Bob Edge","segments":["s1"]}
"#,
    );

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rescan-file")
        .arg("facets/work/entities/20260230.jsonl")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(75));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be utf-8")
            .starts_with("indexer rescan-file failed: ")
    );
    fs::remove_dir_all(root).expect("cleanup rescan file edge failure root");
}

#[test]
fn indexer_rebuild_edges_failure_exits_tempfail() {
    let root = temp_path("rebuild-edge-failure");
    write(
        &root,
        "chronicle/20260430/default/090000_300/talents/documents.json",
        "{not json",
    );

    let output = Command::new(bin())
        .arg("indexer")
        .arg("--journal")
        .arg(&root)
        .arg("--rebuild-edges")
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(75));
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be utf-8")
            .contains(
                "warning: Skipping edge extraction for 20260430/default/090000_300/talents/documents.json"
            )
    );
    fs::remove_dir_all(root).expect("cleanup rebuild edge failure root");
}

#[test]
fn indexer_native_mutation_verbs_write_and_report_json() {
    let root = temp_path("mutation-verbs");
    let conn = open_index(&root).expect("open index");
    conn.execute(
        "INSERT INTO chunks(content,path,day,facet,agent,stream,idx,time_bucket) VALUES (?1,?2,'','','',?3,0,'')",
        rusqlite::params!["private text", "chronicle/private.md", "private"],
    )
    .expect("seed stream chunk");
    conn.execute(
        "INSERT INTO files(path,mtime) VALUES (?1,1)",
        ["chronicle/private.md"],
    )
    .expect("seed stream file");
    conn.execute(
        "INSERT INTO chunks(content,path,day,facet,agent,stream,idx,time_bucket) VALUES (?1,?2,'','','','',0,'')",
        rusqlite::params!["segment text", "20260809/default/090000_300/talents/flow.md"],
    )
    .expect("seed segment chunk");
    conn.execute(
        "INSERT INTO files(path,mtime) VALUES (?1,1)",
        ["20260809/default/090000_300/talents/flow.md"],
    )
    .expect("seed segment file");
    conn.execute(
        "INSERT INTO edges(src,dst,kind,directed,source,path,weight) VALUES ('source','target','knows',0,'fixture','fixture.jsonl',1)",
        [],
    )
    .expect("seed edge");
    drop(conn);

    let output = run_indexer_verb(&root, &["prune-stream", "private", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("stream JSON"),
        serde_json::json!({"chunks": 1, "files": 1})
    );

    let output = run_indexer_verb(
        &root,
        &["prune-paths", "20260809/default/090000_300", "--json"],
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("paths JSON"),
        serde_json::json!({"chunks": 1, "files": 1})
    );

    let output = run_indexer_verb(&root, &["fold-entity-edges", "source", "merged", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let fold = serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("fold JSON");
    assert_eq!(fold["rows_folded"], 1);
    assert_eq!(fold["self_edges_dropped"], 0);

    let output = run_indexer_verb(&root, &["edge-fingerprint", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let fingerprint =
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("fingerprint JSON");
    assert_eq!(fingerprint["fingerprint"].as_str().map(str::len), Some(64));
    fs::remove_dir_all(root).expect("cleanup mutation verbs root");
}
