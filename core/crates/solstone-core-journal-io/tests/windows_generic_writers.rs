// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(windows)]

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use serde_json::json;
use solstone_core_journal_io::{AtomicWriteOptions, JsonWriteOptions, atomic_replace, write_json};

fn temporary(label: &str) -> tempfile::TempDir {
    tempfile::Builder::new().prefix(label).tempdir().unwrap()
}

fn assert_only_destination(parent: &Path, destination_name: &OsStr) {
    assert!(
        fs::read_dir(parent)
            .unwrap()
            .all(|entry| { entry.unwrap().file_name() == destination_name })
    );
}

mod via_root {
    use std::fs;

    use serde_json::json;
    use solstone_core_journal_io::{
        AtomicWriteOptions, JsonWriteOptions, atomic_replace, write_json,
    };

    #[test]
    fn real_caller_patterns_publish() {
        let temporary = super::temporary("generic-root-");

        let state = json!({"entries": []});
        let state_path = temporary.path().join("catchup.json");
        write_json(&state_path, &state, JsonWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&state_path).unwrap(), b"{\n  \"entries\": []\n}\n");

        let value = json!({"generation": 1});
        let health_path = temporary.path().join("direct-door.json");
        write_json(
            &health_path,
            &value,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(&health_path).unwrap(),
            b"{\n  \"generation\": 1\n}\n"
        );

        let bytes = b"schedule".to_vec();
        let schedule_path = temporary.path().join("schedules.json");
        atomic_replace(&schedule_path, &bytes, AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&schedule_path).unwrap(), bytes);
    }
}

mod via_module_path {
    use std::fs;

    use serde_json::json;
    use solstone_core_journal_io::atomic::{
        AtomicWriteOptions, JsonWriteOptions, atomic_replace, write_json,
    };

    #[test]
    fn real_caller_patterns_publish() {
        let temporary = super::temporary("generic-module-");

        let state = json!({"entries": []});
        let state_path = temporary.path().join("catchup.json");
        write_json(&state_path, &state, JsonWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&state_path).unwrap(), b"{\n  \"entries\": []\n}\n");

        let value = json!({"generation": 1});
        let health_path = temporary.path().join("direct-door.json");
        write_json(
            &health_path,
            &value,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(&health_path).unwrap(),
            b"{\n  \"generation\": 1\n}\n"
        );

        let bytes = b"schedule".to_vec();
        let schedule_path = temporary.path().join("schedules.json");
        atomic_replace(&schedule_path, &bytes, AtomicWriteOptions::default()).unwrap();
        assert_eq!(fs::read(&schedule_path).unwrap(), bytes);
    }
}

#[test]
fn prepublication_failure_preserves_destination_without_a_stage() {
    let temporary = temporary("generic-failure-");
    let path = temporary.path().join("unit.service");
    fs::write(&path, b"old").unwrap();

    let result = atomic_replace(&path, b"new", AtomicWriteOptions { mode: Some(0o1000) });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert_only_destination(temporary.path(), path.file_name().unwrap());
}

#[test]
fn json_options_publish_exact_bytes() {
    let temporary = temporary("generic-format-");
    let value = json!({"zebra": {"middle": 1, "alpha": 2}, "beta": 3});

    let default_path = temporary.path().join("default.json");
    write_json(&default_path, &value, JsonWriteOptions::default()).unwrap();
    assert_eq!(
        fs::read(&default_path).unwrap(),
        b"{\n  \"zebra\": {\n    \"middle\": 1,\n    \"alpha\": 2\n  },\n  \"beta\": 3\n}\n"
    );

    let sorted_path = temporary.path().join("sorted.json");
    write_json(
        &sorted_path,
        &value,
        JsonWriteOptions {
            mode: None,
            indent: Some(4),
            sort_keys: true,
        },
    )
    .unwrap();
    assert_eq!(
        fs::read(&sorted_path).unwrap(),
        b"{\n    \"beta\": 3,\n    \"zebra\": {\n        \"alpha\": 2,\n        \"middle\": 1\n    }\n}\n"
    );
}

#[test]
fn writers_create_missing_nested_parents() {
    let temporary = temporary("generic-parent-");

    let atomic_path = temporary.path().join("atomic/a/b/value.bin");
    atomic_replace(&atomic_path, b"atomic", AtomicWriteOptions::default()).unwrap();
    assert!(atomic_path.parent().unwrap().is_dir());
    assert_eq!(fs::read(&atomic_path).unwrap(), b"atomic");

    let json_path = temporary.path().join("json/a/b/value.json");
    write_json(
        &json_path,
        &json!({"state": "json"}),
        JsonWriteOptions::default(),
    )
    .unwrap();
    assert!(json_path.parent().unwrap().is_dir());
    assert_eq!(
        fs::read(&json_path).unwrap(),
        b"{\n  \"state\": \"json\"\n}\n"
    );
}
