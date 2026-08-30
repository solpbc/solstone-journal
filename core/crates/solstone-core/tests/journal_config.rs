// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_journal_config::materialized_defaults;
use solstone_core_journal_config_write::{
    JournalConfigMutation, LockOptions, hold_lock, mutate_journal_config,
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_solstone-core")
}

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be available")
        .as_nanos();
    std::env::temp_dir().join(format!("solstone-core-journal-config-{name}-{stamp}"))
}

fn write(root: &Path, relative: &str, contents: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("test path should have parent"))
        .expect("create parent");
    fs::write(path, contents).expect("write test file");
}

#[derive(Debug, Eq, PartialEq)]
enum SnapshotEntry {
    Directory { mode: u32 },
    File { bytes: Vec<u8>, mode: u32 },
    Symlink { target: PathBuf, mode: u32 },
    Other { mode: u32 },
}

#[cfg(unix)]
fn snapshot_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn snapshot_mode(_: &fs::Metadata) -> u32 {
    0
}

fn snapshot_directory(root: &Path) -> Vec<(PathBuf, SnapshotEntry)> {
    fn visit(root: &Path, path: &Path, snapshot: &mut Vec<(PathBuf, SnapshotEntry)>) {
        let metadata = fs::symlink_metadata(path).expect("snapshot entry metadata");
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path is below root")
            .to_path_buf();
        let mode = snapshot_mode(&metadata);
        let file_type = metadata.file_type();
        let entry = if file_type.is_dir() {
            SnapshotEntry::Directory { mode }
        } else if file_type.is_file() {
            SnapshotEntry::File {
                bytes: fs::read(path).expect("snapshot file reads"),
                mode,
            }
        } else if file_type.is_symlink() {
            SnapshotEntry::Symlink {
                target: fs::read_link(path).expect("snapshot symlink reads"),
                mode,
            }
        } else {
            SnapshotEntry::Other { mode }
        };
        snapshot.push((relative, entry));

        if file_type.is_dir() {
            for entry in fs::read_dir(path).expect("snapshot directory reads") {
                let path = entry.expect("snapshot entry reads").path();
                visit(root, &path, snapshot);
            }
        }
    }

    let mut snapshot = Vec::new();
    if root.is_dir() {
        visit(root, root, &mut snapshot);
    }
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

fn run(root: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(bin())
        .arg("journal-config")
        .args(args)
        .arg("--journal")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("solstone-core should execute");
    child
        .stdin
        .as_mut()
        .expect("stdin should be piped")
        .write_all(stdin)
        .expect("write stdin");
    child.wait_with_output().expect("wait for solstone-core")
}

fn read(root: &Path) -> Value {
    let output = run(root, &["read"], b"");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stderr, b"");
    serde_json::from_slice(&output.stdout).expect("read output should be JSON")
}

fn expect_hash(root: &Path) -> String {
    read(root)["sha256"]
        .as_str()
        .expect("present config should have sha256")
        .to_owned()
}

#[test]
fn journal_config_read_missing_prints_absent_envelope() {
    let root = temp_path("read-missing");
    fs::create_dir(&root).expect("create root");

    assert_eq!(
        read(&root),
        json!({
            "present": false,
            "sha256": null,
            "config": serde_json::to_value(materialized_defaults()).unwrap(),
        })
    );
    assert!(!root.join("config/journal.json").exists());
    assert!(!root.join("config/journal.json.lock").exists());
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_present_prints_exact_fingerprint_and_config() {
    let root = temp_path("read-present");
    write(&root, "config/journal.json", b"{\"known\":\"value\"}\n");

    let output = read(&root);

    assert_eq!(output["present"], true);
    assert!(
        output["sha256"]
            .as_str()
            .is_some_and(|hash| hash.starts_with("sha256:"))
    );
    assert_eq!(output["config"], json!({"known": "value"}));
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_with_mcp_endpoint_disabled_key_is_unaffected() {
    let root = temp_path("read-mcp-endpoint-disabled");
    write(
        &root,
        "config/journal.json",
        b"{\"mcp_endpoint\": {\"enabled\": false}}",
    );
    let before = snapshot_directory(&root);

    let output = read(&root);

    assert_eq!(snapshot_directory(&root), before);
    assert_eq!(output["present"], true);
    assert_eq!(
        output["config"],
        json!({"mcp_endpoint": {"enabled": false}})
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_with_mcp_endpoint_enabled_key_is_unaffected() {
    let root = temp_path("read-mcp-endpoint-enabled");
    write(
        &root,
        "config/journal.json",
        b"{\"mcp_endpoint\": {\"enabled\": true}}",
    );
    let before = snapshot_directory(&root);

    let output = read(&root);

    assert_eq!(snapshot_directory(&root), before);
    assert_eq!(output["present"], true);
    assert_eq!(output["config"], json!({"mcp_endpoint": {"enabled": true}}));
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_with_malformed_mcp_endpoint_key_is_unaffected() {
    let root = temp_path("read-mcp-endpoint-malformed");
    write(
        &root,
        "config/journal.json",
        b"{\"mcp_endpoint\": {\"enabled\": \"yes\"}}",
    );
    let before = snapshot_directory(&root);

    let output = read(&root);

    assert_eq!(snapshot_directory(&root), before);
    assert_eq!(output["present"], true);
    assert_eq!(
        output["config"],
        json!({"mcp_endpoint": {"enabled": "yes"}})
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_corrupt_exits_unavailable_without_echoing_content() {
    let root = temp_path("read-corrupt");
    let secret = b"{super-secret-content";
    write(&root, "config/journal.json", secret);

    let output = run(&root, &["read"], b"");

    assert_eq!(output.status.code(), Some(69));
    assert_eq!(output.stdout, b"");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("journal-config read failed: "),
        "{stderr}"
    );
    assert!(!stderr.contains("super-secret-content"));
    assert_eq!(fs::read(root.join("config/journal.json")).unwrap(), secret);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_read_rejects_invalid_argv() {
    let output = Command::new(bin())
        .args(["journal-config", "read", "--expect", "absent"])
        .output()
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(64));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, solstone_core_cli::USAGE.as_bytes());
}

#[test]
fn journal_config_commit_matching_expectation_replaces_config() {
    let root = temp_path("commit-match");
    write(&root, "config/journal.json", b"{\"known\":\"before\"}\n");
    let expected = expect_hash(&root);

    let output = run(
        &root,
        &["commit", "--expect", &expected],
        b"{\"known\":\"after\"}",
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(read(&root)["config"], json!({"known": "after"}));
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_expect_absent_creates_config() {
    let root = temp_path("commit-absent");
    fs::create_dir(&root).expect("create root");

    let output = run(
        &root,
        &["commit", "--expect", "absent"],
        b"{\"created\":true}",
    );

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"");
    assert_eq!(read(&root)["config"], json!({"created": true}));
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_expect_absent_conflicts_when_config_now_present() {
    let root = temp_path("commit-absent-conflict");
    let original = b"{\"appeared\":true}\n";
    write(&root, "config/journal.json", original);

    let output = run(
        &root,
        &["commit", "--expect", "absent"],
        b"{\"replacement\":true}",
    );

    assert_eq!(output.status.code(), Some(65));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        fs::read(root.join("config/journal.json")).unwrap(),
        original
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_expect_hash_conflicts_when_config_now_absent() {
    let root = temp_path("commit-hash-absent-conflict");
    fs::create_dir(&root).expect("create root");
    let expected = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    let output = run(
        &root,
        &["commit", "--expect", expected],
        b"{\"replacement\":true}",
    );

    assert_eq!(output.status.code(), Some(65));
    assert_eq!(output.stdout, b"");
    assert!(!root.join("config/journal.json").exists());
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_expect_hash_mismatch_leaves_existing_bytes_unchanged() {
    let root = temp_path("commit-hash-mismatch");
    let original = b"{\"known\":\"actual\"}\n";
    write(&root, "config/journal.json", original);
    let expected = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    let output = run(
        &root,
        &["commit", "--expect", expected],
        b"{\"replacement\":true}",
    );

    assert_eq!(output.status.code(), Some(65));
    assert_eq!(output.stdout, b"");
    assert_eq!(
        fs::read(root.join("config/journal.json")).unwrap(),
        original
    );
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_rejects_malformed_nonobject_and_oversized_stdin() {
    let root = temp_path("commit-invalid-stdin");
    fs::create_dir(&root).expect("create root");
    for stdin in [b"{not json".as_slice(), b"[]".as_slice()] {
        let output = run(&root, &["commit", "--expect", "absent"], stdin);
        assert_eq!(output.status.code(), Some(64));
        assert_eq!(output.stdout, b"");
        assert!(!root.join("config/journal.json").exists());
    }
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let output = run(&root, &["commit", "--expect", "absent"], &oversized);
    assert_eq!(output.status.code(), Some(64));
    assert_eq!(output.stdout, b"");
    assert!(!root.join("config/journal.json").exists());
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_corrupt_existing_file_exits_unavailable() {
    let root = temp_path("commit-corrupt");
    let secret = b"{super-secret-content";
    write(&root, "config/journal.json", secret);

    let output = run(
        &root,
        &["commit", "--expect", "absent"],
        b"{\"replacement\":true}",
    );

    assert_eq!(output.status.code(), Some(69));
    assert_eq!(output.stdout, b"");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("journal-config commit failed: "),
        "{stderr}"
    );
    assert!(!stderr.contains("super-secret-content"));
    assert_eq!(fs::read(root.join("config/journal.json")).unwrap(), secret);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_lock_timeout_exits_tempfail() {
    let root = temp_path("commit-timeout");
    fs::create_dir(&root).expect("create root");
    let config = root.join("config/journal.json");
    let _lock = hold_lock(&config, LockOptions::default()).expect("hold config lock");

    let output = run(
        &root,
        &["commit", "--expect", "absent", "--lock-timeout-ms", "50"],
        b"{\"replacement\":true}",
    );

    assert_eq!(output.status.code(), Some(75));
    assert_eq!(output.stdout, b"");
    assert!(!root.join("config/journal.json").exists());
    drop(_lock);
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_lock_io_exits_ioerr() {
    let root = temp_path("commit-lock-io");
    fs::write(&root, b"not a directory").expect("write journal path as file");
    let child_journal = root.join("child");

    let output = Command::new(bin())
        .args([
            "journal-config",
            "commit",
            "--journal",
            child_journal.to_str().expect("test path should be utf-8"),
            "--expect",
            "absent",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .as_mut()
                .expect("stdin should be piped")
                .write_all(b"{\"replacement\":true}")?;
            child.wait_with_output()
        })
        .expect("solstone-core should execute");

    assert_eq!(output.status.code(), Some(74));
    assert_eq!(output.stdout, b"");
    fs::remove_file(root).expect("cleanup root");
}

fn noop_materialize(
    root: &Path,
) -> solstone_core_journal_config_write::JournalConfigTransaction<()> {
    mutate_journal_config(root, LockOptions::default(), |_config| {
        JournalConfigMutation {
            changed: false,
            value: (),
        }
    })
    .expect("materialize journal config")
}

#[test]
fn journal_config_materialization_is_deterministic_across_empty_journals() {
    let first = temp_path("materialize-a");
    let second = temp_path("materialize-b");
    fs::create_dir(&first).expect("create first journal");
    fs::create_dir(&second).expect("create second journal");

    let first_txn = noop_materialize(&first);
    let second_txn = noop_materialize(&second);
    assert!(first_txn.written);
    assert!(!first_txn.changed);
    assert!(second_txn.written);
    assert!(!second_txn.changed);

    let first_bytes = fs::read(first.join("config/journal.json")).expect("read first config");
    let second_bytes = fs::read(second.join("config/journal.json")).expect("read second config");
    assert_eq!(first_bytes, second_bytes);
    fs::remove_dir_all(first).expect("cleanup first");
    fs::remove_dir_all(second).expect("cleanup second");
}

#[test]
fn journal_config_mutate_cycle_writes_only_on_materialize_or_change() {
    let root = temp_path("mutate-cycle");
    fs::create_dir(&root).expect("create journal");

    let materialized = noop_materialize(&root);
    assert!(materialized.written);
    assert!(!materialized.changed);
    assert!(root.join("config/journal.json").is_file());

    let noop = noop_materialize(&root);
    assert!(!noop.written);
    assert!(!noop.changed);

    let changed = mutate_journal_config(&root, LockOptions::default(), |config| {
        config.insert("appended".to_owned(), json!("new"));
        JournalConfigMutation {
            changed: true,
            value: (),
        }
    })
    .expect("changed mutation");
    assert!(changed.written);
    assert!(changed.changed);
    let committed: Value =
        serde_json::from_slice(&fs::read(root.join("config/journal.json")).expect("read config"))
            .expect("committed JSON");
    assert_eq!(committed["appended"], "new");
    fs::remove_dir_all(root).expect("cleanup root");
}

#[test]
fn journal_config_commit_preserves_unknown_order_and_raw_utf8() {
    let root = temp_path("preserve-utf8");
    write(
        &root,
        "config/journal.json",
        b"{\"known\": \"before\", \"unknown\": \"Ren\xc3\xa9e\", \"ordered\": true}\n",
    );

    mutate_journal_config(&root, LockOptions::default(), |config| {
        config.insert("known".to_owned(), json!("after"));
        config.insert("appended".to_owned(), json!("new"));
        JournalConfigMutation {
            changed: true,
            value: (),
        }
    })
    .expect("preserve mutation");

    let committed = fs::read(root.join("config/journal.json")).expect("read committed config");
    assert!(
        committed
            .windows(b"\"unknown\": \"Ren\xc3\xa9e\"".len())
            .any(|window| window == b"\"unknown\": \"Ren\xc3\xa9e\""),
        "unknown UTF-8 value should remain raw"
    );
    assert!(
        !committed
            .windows(b"Ren\\u00e9e".len())
            .any(|window| window == b"Ren\\u00e9e"),
        "unknown UTF-8 value must not be escaped"
    );
    let text = String::from_utf8(committed).expect("committed UTF-8");
    let positions: Vec<_> = ["known", "unknown", "ordered"]
        .into_iter()
        .map(|key| {
            text.find(&format!("\"{key}\""))
                .expect("top-level key should be present")
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(text.contains("\"appended\""));
    fs::remove_dir_all(root).expect("cleanup root");
}
