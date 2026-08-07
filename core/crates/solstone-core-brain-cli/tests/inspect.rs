// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "the real-binary bed owns its temporary journal"
)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

const BINARY: &str = env!("CARGO_BIN_EXE_solstone-brain");
static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Journal {
    root: PathBuf,
}

impl Journal {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "solstone-brain-cli-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).expect("temporary journal");
        Self { root }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        Command::new(BINARY)
            .args(arguments)
            .output()
            .expect("run solstone-brain")
    }

    fn write_none_record(&self) {
        let health = self.root.join("health");
        fs::create_dir_all(&health).expect("health directory");
        let record = json!({
            "schema_version": 1,
            "revision": 0,
            "updated_at": "2026-01-01T00:00:00Z",
            "aggregate_state": "ready",
            "reason_code": null,
            "active_lane": "none",
            "active_provider": "none",
            "active_model": null,
            "fingerprint_sha256": null,
            "checking": null,
            "runtime_failure_marker": null,
            "diagnostic": {},
            "evidence": {
                "configuration": {
                    "status": "ok",
                    "observed_at": "2026-01-01T00:00:00Z",
                    "expires_at": "2099-01-01T00:00:00Z"
                },
                "lane_prerequisites": null,
                "generate": null,
                "cogitate": null
            }
        });
        fs::write(
            health.join("brain.json"),
            serde_json::to_vec(&record).expect("record JSON"),
        )
        .expect("write record");
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    serde_json::from_slice(&output.stdout).expect("stdout JSON")
}

fn journal_argument(journal: &Journal) -> String {
    journal.root.to_string_lossy().into_owned()
}

#[test]
fn malformed_invocations_are_usage_errors() {
    let journal = Journal::new();
    let path = journal_argument(&journal);
    for arguments in [
        vec![],
        vec!["unknown"],
        vec!["inspect", "--help"],
        vec!["inspect", "--journal"],
        vec!["inspect", "--journal", &path, "--journal", &path],
    ] {
        let output = Command::new(BINARY)
            .args(arguments)
            .output()
            .expect("run solstone-brain");
        assert_eq!(output.status.code(), Some(64));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn inspect_missing_record_emits_a_single_json_outcome() {
    let journal = Journal::new();
    let path = journal_argument(&journal);
    let output = journal.run(&["inspect", "--journal", &path]);
    let body = output_json(&output);
    assert_eq!(body["status"], "unavailable");
    assert_eq!(body["projection"]["reason_code"], "brain_record_missing");
    assert_eq!(
        body["record_path"],
        journal
            .root
            .join("health/brain.json")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
}

#[test]
fn inspect_valid_none_lane_record_emits_ok_json_only() {
    let journal = Journal::new();
    journal.write_none_record();
    let path = journal_argument(&journal);
    let output = journal.run(&["inspect", "--journal", &path]);
    let body = output_json(&output);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["projection"]["active_lane"], "none");
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
}

#[test]
fn inspect_carries_the_record_it_read_and_null_when_there_is_none() {
    // The inspection's own answer about the record is not derivable from the
    // projection: a caller cannot tell an `ok` inspection's revision, checking
    // block or evidence from an aggregate state. The writer that follows needs
    // all three, and this field was dropped once already.
    let journal = Journal::new();
    let path = journal_argument(&journal);

    let missing = output_json(&journal.run(&["inspect", "--journal", &path]));
    assert_eq!(missing["status"], "unavailable");
    assert!(
        missing["record"].is_null(),
        "a journal with no record must carry a null record, got {}",
        missing["record"]
    );

    journal.write_none_record();
    let present = output_json(&journal.run(&["inspect", "--journal", &path]));
    assert_eq!(present["status"], "ok");
    // Byte-for-byte what is on disk, not a re-serialization of the parsed
    // record: re-encoding a durable format would re-decide its null-versus-
    // absent distinctions, and the point of this field is that it does not.
    let on_disk: Value =
        serde_json::from_slice(&fs::read(journal.root.join("health/brain.json")).expect("record"))
            .expect("record parses");
    assert_eq!(present["record"], on_disk);

    fs::write(journal.root.join("health/brain.json"), b"not json").expect("corrupt the record");
    let corrupt = output_json(&journal.run(&["inspect", "--journal", &path]));
    assert_eq!(corrupt["status"], "corrupt");
    assert!(
        corrupt["record"].is_null(),
        "a record that does not validate is not half-carried"
    );
}
