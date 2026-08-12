// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_import::{cli_argv, cli_journal_source};
use tempfile::TempDir;

const GRAMMAR: &str = include_str!("../../../fixtures/journal_source_reference_grammar.json");
const AUTHORITY: &str = include_str!("../../../../solstone/think/native/import/authority.toml");

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn run(root: &Path, values: &[&str]) -> cli_argv::CliRun {
    cli_journal_source::run_cli(&args(values), root)
}

fn sources_dir(root: &Path) -> std::path::PathBuf {
    root.join("apps/import/journal_sources")
}

fn record_path(root: &Path, name: &str) -> std::path::PathBuf {
    sources_dir(root).join(format!("{name}.json"))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_record(root: &Path, filename: &str, record: Value) {
    fs::create_dir_all(sources_dir(root)).unwrap();
    fs::write(
        sources_dir(root).join(filename),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
}

fn stats() -> Value {
    json!({
        "segments_received": 0,
        "entities_received": 0,
        "facets_received": 0,
        "imports_received": 0,
        "config_received": 0,
    })
}

fn dl_record(name: &str, created_at: i64) -> Value {
    json!({
        "key": "abcdefgh0123456789abcdefghijklmnopqrstuvw",
        "name": name,
        "created_at": created_at,
        "enabled": true,
        "revoked": false,
        "revoked_at": null,
        "stats": stats(),
    })
}

fn pl_record(
    fingerprint: &str,
    created_at: i64,
    device_label: &str,
    peer_instance_id: Option<&str>,
) -> Value {
    let mut record = json!({
        "fingerprint": fingerprint,
        "pair_mode": "pl",
        "device_label": device_label,
        "paired_at": "2026-08-10T12:00:00+00:00",
        "created_at": created_at,
        "enabled": true,
        "revoked": false,
        "revoked_at": null,
        "stats": stats(),
    });
    if let Some(peer_instance_id) = peer_instance_id {
        record["peer_instance_id"] = json!(peer_instance_id);
    }
    record
}

fn pl_filename(fingerprint: &str) -> String {
    format!("{}.json", &fingerprint[7..23])
}

#[test]
fn direct_entry_point_handles_create_list_status_and_revoke() {
    let root = TempDir::new().unwrap();
    let start = now_ms();
    let created = run(root.path(), &["create", "phone"]);
    let end = now_ms();

    assert_eq!(created.exit_code, 0);
    assert!(created.stdout.contains("Journal source created:"));

    let record = read_json(&record_path(root.path(), "phone"));
    let key = record["key"].as_str().unwrap();
    assert_eq!(key.len(), 43);
    assert!(
        key.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    );
    assert!((start..=end).contains(&record["created_at"].as_i64().unwrap()));
    assert_eq!(record["stats"], stats());
    assert_eq!(run(root.path(), &["create", "laptop"]).exit_code, 0);
    assert_ne!(
        key,
        read_json(&record_path(root.path(), "laptop"))["key"]
            .as_str()
            .unwrap()
    );

    let listed = run(root.path(), &["--json", "list"]);
    assert_eq!(listed.exit_code, 0);
    assert!(
        serde_json::from_str::<Value>(listed.stdout.trim())
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["name"] == "phone")
    );

    let status = run(root.path(), &["--json", "status", "phone"]);
    assert_eq!(status.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(status.stdout.trim()).unwrap()["status"],
        "active"
    );

    let revoked = run(root.path(), &["--json", "revoke", "phone"]);
    assert_eq!(revoked.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(revoked.stdout.trim()).unwrap()["revoked"],
        true
    );
}

#[test]
fn parent_treats_journal_source_as_media_and_authority_declares_an_argument() {
    let root = TempDir::new().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    fs::create_dir_all(root.path().join("health")).unwrap();
    fs::write(
        root.path().join("health/convey.port"),
        listener.local_addr().unwrap().port().to_string(),
    )
    .unwrap();

    let parent = cli_argv::run_cli(&args(&["journal-source"]), root.path());
    assert!(parent.stderr.contains("import: unimplemented: cli_argv"));
    assert!(!parent.stderr.contains("unrecognized arguments"));
    assert!(AUTHORITY.contains("journal_source"));
    let block = AUTHORITY.split("journal_source").nth(1).unwrap();
    assert!(block.contains("kind = \"argument\""));
    assert!(!block.contains("kind = \"option\""));
}

#[test]
fn list_and_status_json_match_dl_and_pl_shapes() {
    let root = TempDir::new().unwrap();
    let fingerprint_a = format!("sha256:{}", "a".repeat(64));
    let fingerprint_b = format!("sha256:{}", "b".repeat(64));
    write_record(root.path(), "dl-source.json", dl_record("dl-source", 1_000));
    write_record(
        root.path(),
        &pl_filename(&fingerprint_a),
        pl_record(&fingerprint_a, 3_000, "Phone", Some("peer-1")),
    );
    write_record(
        root.path(),
        &pl_filename(&fingerprint_b),
        pl_record(&fingerprint_b, 2_000, "Tablet", None),
    );
    fs::create_dir_all(root.path().join("link")).unwrap();
    fs::write(
        root.path().join("link/authorized_clients.json"),
        serde_json::to_vec(&json!([{
            "kind": "cert",
            "fingerprint": fingerprint_a,
            "last_seen_at": "2026-08-10T14:30:00+00:00",
        }]))
        .unwrap(),
    )
    .unwrap();

    let listed = run(root.path(), &["--json", "list"]);
    assert_eq!(listed.exit_code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(listed.stdout.trim()).unwrap(),
        json!([
            {
                "mode": "pl", "prefix": "aaaaaaaaaaaaaaaa", "fingerprint": fingerprint_a,
                "device_label": "Phone", "status": "active", "paired_at": "2026-08-10T12:00:00+00:00",
                "last_seen_at": "2026-08-10T14:30:00+00:00", "auth_status": "present",
                "created_at": 3_000, "peer_instance_id": "peer-1",
            },
            {
                "mode": "pl", "prefix": "bbbbbbbbbbbbbbbb", "fingerprint": fingerprint_b,
                "device_label": "Tablet", "status": "active", "paired_at": "2026-08-10T12:00:00+00:00",
                "last_seen_at": null, "auth_status": "missing", "created_at": 2_000,
            },
            {
                "mode": "dl", "prefix": "abcdefgh", "name": "dl-source", "status": "active", "created_at": 1_000,
            },
        ])
    );

    let single = run(root.path(), &["--json", "status", "dl-source"]);
    assert_eq!(
        serde_json::from_str::<Value>(single.stdout.trim()).unwrap(),
        json!({
            "name": "dl-source", "prefix": "abcdefgh", "status": "active", "created_at": 1_000,
            "revoked": false, "revoked_at": null,
            "state_dir": root.path().join("imports/abcdefgh"), "stats": stats(),
        })
    );
    let overview = run(root.path(), &["--json", "status"]);
    assert_eq!(
        serde_json::from_str::<Value>(overview.stdout.trim()).unwrap(),
        json!([{
            "name": "dl-source", "prefix": "abcdefgh", "status": "active", "created_at": 1_000,
            "stats": stats(), "state_dir": root.path().join("imports/abcdefgh"),
        }])
    );
}

#[test]
fn list_modes_and_empty_registry_follow_the_grammar() {
    let root = TempDir::new().unwrap();
    assert_eq!(run(root.path(), &["--json", "list"]).stdout, "[]\n");
    assert_eq!(
        run(root.path(), &["list"]).stdout,
        "No journal sources registered.\n"
    );
    assert_eq!(
        run(root.path(), &["list", "--mode", "dl"]).stdout,
        "No journal sources match --mode dl.\n"
    );
    let invalid = run(root.path(), &["list", "--mode", "xyz"]);
    assert_eq!(invalid.exit_code, 2);
    assert!(
        invalid
            .stderr
            .starts_with("usage: journal importer journal-source")
    );
    assert!(invalid.stderr.contains("invalid choice"));
}

#[test]
fn parser_rejects_unknown_or_late_top_level_options_and_distinguishes_help() {
    let root = TempDir::new().unwrap();
    for values in [
        &["--nonsense"][..],
        &["create", "--json", "name"][..],
        &["status", "--nonsense"][..],
    ] {
        let result = run(root.path(), values);
        assert_eq!(result.exit_code, 2);
        assert_eq!(result.stdout, "");
        assert!(
            result
                .stderr
                .starts_with("usage: journal importer journal-source")
        );
    }
    assert_eq!(run(root.path(), &["-h"]).exit_code, 0);
    assert_eq!(run(root.path(), &[]).exit_code, 1);
    assert_eq!(
        run(root.path(), &["-v", "--debug", "list", "--mode", "dl"]).exit_code,
        0
    );
}

#[test]
fn create_rejects_duplicates_and_exclusive_publish_preserves_existing_bytes() {
    let root = TempDir::new().unwrap();
    assert_eq!(run(root.path(), &["create", "phone"]).exit_code, 0);
    let original = fs::read(record_path(root.path(), "phone")).unwrap();
    let duplicate = run(root.path(), &["create", "phone"]);
    assert_eq!(duplicate.exit_code, 1);
    assert_eq!(
        duplicate.stderr,
        "Error: journal source 'phone' already exists\n"
    );
    assert_eq!(
        fs::read(record_path(root.path(), "phone")).unwrap(),
        original
    );

    fs::create_dir_all(sources_dir(root.path())).unwrap();
    let raced = record_path(root.path(), "raced");
    fs::write(&raced, b"not a journal source").unwrap();
    let race_result = run(root.path(), &["create", "raced"]);
    assert_eq!(race_result.exit_code, 1);
    assert_eq!(
        race_result.stderr,
        "Error: journal source 'raced' already exists\n"
    );
    assert_eq!(fs::read(raced).unwrap(), b"not a journal source");
}

#[test]
fn revoke_sets_the_contract_field_without_changing_enabled() {
    let root = TempDir::new().unwrap();
    assert_eq!(run(root.path(), &["create", "phone"]).exit_code, 0);
    let before = read_json(&record_path(root.path(), "phone"));
    let start = now_ms();
    assert_eq!(run(root.path(), &["revoke", "phone"]).exit_code, 0);
    let end = now_ms();
    let after = read_json(&record_path(root.path(), "phone"));
    assert_eq!(after["revoked"], true);
    assert_eq!(after["enabled"], before["enabled"]);
    assert!((start..=end).contains(&after["revoked_at"].as_i64().unwrap()));

    let already_revoked = run(root.path(), &["revoke", "phone"]);
    let missing = run(root.path(), &["status", "unknown"]);
    assert_eq!(
        already_revoked.stderr,
        "Journal source 'phone' is already revoked.\n"
    );
    assert_eq!(
        missing.stderr,
        "Error: journal source 'unknown' not found\n"
    );
    assert_ne!(already_revoked.stderr, missing.stderr);
}

#[test]
fn status_and_revoke_never_target_pl_records() {
    let root = TempDir::new().unwrap();
    let fingerprint = format!("sha256:{}", "c".repeat(64));
    let mut record = pl_record(&fingerprint, 1_000, "Phone", None);
    record["name"] = json!(fingerprint);
    write_record(root.path(), &pl_filename(&fingerprint), record);

    for verb in ["status", "revoke"] {
        let result = run(root.path(), &[verb, &fingerprint]);
        assert_eq!(result.exit_code, 1);
        assert_eq!(
            result.stderr,
            format!("Error: journal source '{fingerprint}' not found\n")
        );
    }
    assert_eq!(
        read_json(&sources_dir(root.path()).join(pl_filename(&fingerprint)))["revoked"],
        false
    );
}

#[test]
fn malformed_or_misnamed_registry_records_are_silently_skipped() {
    let root = TempDir::new().unwrap();
    write_record(root.path(), "good.json", dl_record("good", 1_000));
    write_record(
        root.path(),
        "bad-fingerprint.json",
        pl_record("sha256:abc", 9_000, "Bad", None),
    );
    write_record(
        root.path(),
        "key-pl.json",
        json!({"key": "abcdefgh", "name": "key-pl", "pair_mode": "pl"}),
    );
    write_record(
        root.path(),
        "peer-dl.json",
        json!({"key": "abcdefgh", "name": "peer-dl", "peer_instance_id": "peer-1"}),
    );
    write_record(
        root.path(),
        "non-boundary-key.json",
        json!({"key": "abcdefgé", "name": "non-boundary-key"}),
    );
    write_record(root.path(), "wrong.json", dl_record("different", 8_000));
    fs::write(sources_dir(root.path()).join("broken.json"), b"{").unwrap();

    let listed = run(root.path(), &["--json", "list"]);
    assert_eq!(listed.exit_code, 0);
    let rows: Value = serde_json::from_str(listed.stdout.trim()).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["name"], "good");
    assert_eq!(run(root.path(), &["status", "different"]).exit_code, 1);
    assert_eq!(
        run(root.path(), &["status", "non-boundary-key"]).exit_code,
        1
    );
}

#[test]
fn fixture_and_outputs_preserve_the_native_grammar_contract() {
    let fixture: Value = serde_json::from_str(GRAMMAR).unwrap();
    assert_eq!(
        fixture["oracle"]["grammar"]["prog"],
        "sol import journal-source"
    );
    assert_eq!(
        fixture["oracle"]["grammar"]["subcommands"]["list"]["args"][0]["choices"],
        json!(["dl", "pl"])
    );
    assert_eq!(
        fixture["native_delta"]["injected_setup_cli_options"],
        json!(["-v/--verbose", "-d/--debug"])
    );

    let root = TempDir::new().unwrap();
    let outputs = [
        run(root.path(), &["-h"]).stdout,
        run(root.path(), &[]).stdout,
        run(root.path(), &["--nonsense"]).stderr,
        run(root.path(), &["list"]).stdout,
    ];
    assert!(
        outputs
            .iter()
            .all(|output| !output.contains("sol import journal-source"))
    );
}

#[test]
fn create_and_revoke_append_action_log_records() {
    let root = TempDir::new().unwrap();
    assert_eq!(run(root.path(), &["create", "phone"]).exit_code, 0);
    assert_eq!(run(root.path(), &["revoke", "phone"]).exit_code, 0);

    let actions = fs::read_dir(root.path().join("config/actions"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let entries: Vec<Value> = fs::read_to_string(actions)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["action"], "journal_source_create");
    assert_eq!(entries[0]["params"]["name"], "phone");
    assert_eq!(entries[1]["action"], "journal_source_revoke");
    assert_eq!(entries[1]["params"]["name"], "phone");
    assert_eq!(
        entries[0]["params"]["key_prefix"],
        entries[1]["params"]["key_prefix"]
    );
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
