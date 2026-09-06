// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "solstone-core-doctor-cli-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary test directory");
        Self { path }
    }

    fn journal(&self, name: &str) -> PathBuf {
        let path = self.path.join(name);
        fs::create_dir(&path).expect("create temporary journal");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

const OWNER_PREFIX: &str = "your settings file at ";
const CORRUPT_FIX: &str = "repair or restore config/journal.json from a backup";
const STT_CHECKS: [&str; 2] = ["default_stt_ready", "parakeet_cpp_stt_ready"];

fn write_config(journal: &Path, contents: &str) {
    let config = journal.join("config");
    fs::create_dir_all(&config).expect("create journal config directory");
    fs::write(config.join("journal.json"), contents).expect("write journal config");
}

fn write_symlink_loop_config(journal: &Path) {
    let config = journal.join("config");
    fs::create_dir_all(&config).expect("create journal config directory");
    symlink("b", config.join("a")).expect("create first config symlink");
    symlink("a", config.join("b")).expect("create second config symlink");
    symlink("a", config.join("journal.json")).expect("create looping journal config");
}

fn doctor_stt_checks(journal: &Path) -> Vec<Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_solstone-core"))
        .args(["doctor", "--jsonl"])
        .env("SOLSTONE_JOURNAL", journal)
        .env("HOME", journal)
        .output()
        .expect("solstone-core doctor --jsonl runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let records: Vec<Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("doctor JSONL line"))
        .collect();
    assert!(
        records
            .iter()
            .any(|record| record["event"] == "doctor.completed"),
        "doctor must emit completion JSONL\nstdout:\n{}\nstderr:\n{}",
        stdout,
        String::from_utf8_lossy(&output.stderr),
    );
    STT_CHECKS
        .iter()
        .map(|name| {
            records
                .iter()
                .find(|record| record["event"] == "check.completed" && record["name"] == *name)
                .cloned()
                .unwrap_or_else(|| panic!("missing doctor check {name}"))
        })
        .collect()
}

fn carries_corrupt_signature(check: &Value) -> bool {
    check["detail"]
        .as_str()
        .is_some_and(|detail| detail.starts_with(OWNER_PREFIX))
        && check["fix"] == CORRUPT_FIX
}

#[test]
fn doctor_jsonl_reports_corrupt_config_only_through_stt_readiness_checks() {
    let temp = TempDir::new("stt");
    let malformed = temp.journal("malformed");
    let looped = temp.journal("looped");
    let missing = temp.journal("missing");
    let valid = temp.journal("valid");
    write_config(&malformed, "{bad json");
    write_symlink_loop_config(&looped);
    write_config(
        &valid,
        r#"{"setup": {"completed_at": 1}, "transcribe": {"backend": "other"}}"#,
    );

    for journal in [&malformed, &looped] {
        let checks = doctor_stt_checks(journal);
        for check in checks {
            assert_eq!(check["status"], "failed", "{check}");
            assert!(
                check["detail"]
                    .as_str()
                    .expect("detail is a string")
                    .starts_with(OWNER_PREFIX),
                "{check}"
            );
            assert_eq!(check["fix"], CORRUPT_FIX, "{check}");
            assert!(check["execution_error"].is_null(), "{check}");
        }
    }

    for journal in [&missing, &valid] {
        let checks = doctor_stt_checks(journal);
        for check in checks {
            assert!(
                !carries_corrupt_signature(&check),
                "unexpected corrupt-config signature: {check}"
            );
        }
    }
}
