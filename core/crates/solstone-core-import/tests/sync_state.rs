// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::{Map, Value, json};
use solstone_core_import::{
    BackendName, SYNC_BACKEND_INVENTORY, SyncState, SyncStateRead, read_sync_state, state_path,
    write_sync_state,
};
use tempfile::TempDir;

const GRAMMAR: &str = include_str!("../../../fixtures/import_reference_grammar.json");
const ORACLE: &str = include_str!("../../../fixtures/import_sync_reference_oracle.json");

#[test]
fn inventory_matches_the_grammar_and_sync_oracle() {
    let grammar: Value = serde_json::from_str(GRAMMAR).unwrap();
    let expected = grammar["syncable_backends_instantiated"]
        .as_array()
        .unwrap()
        .iter()
        .chain(grammar["native_sync_backends"].as_array().unwrap())
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()
        .unwrap();
    let actual = SYNC_BACKEND_INVENTORY.map(BackendName::as_str);
    assert_eq!(actual.as_slice(), expected);

    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let oracle_names = oracle["sync"]["backends"]
        .as_array()
        .unwrap()
        .iter()
        .map(|backend| backend["name"].as_str())
        .collect::<Option<Vec<_>>>()
        .unwrap();
    assert_eq!(&actual[..3], oracle_names);
}

#[test]
fn oracle_backed_schema_fixture_round_trips_every_backend_union_member() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    assert!(
        oracle["sync"]["state_path_shape"]
            .as_str()
            .is_some_and(|shape| shape.contains("imports/<backend>.json"))
    );
    for (backend, reference) in reference_states() {
        let tree = TempDir::new().unwrap();
        let path = state_path(tree.path(), backend);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec_pretty(&reference).unwrap()).unwrap();

        let state = match read_sync_state(tree.path(), backend) {
            SyncStateRead::Loaded(state) => state,
            other => panic!("unexpected state read: {other:?}"),
        };
        write_sync_state(tree.path(), &state).unwrap();
        let round_tripped: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(round_tripped, reference, "{} union state", backend.as_str());
    }
}

fn reference_states() -> [(BackendName, Value); 3] {
    [
        (
            BackendName::Plaud,
            json!({
                "backend": "plaud",
                "last_sync": "2026-08-11T12:00:00+00:00",
                "unknown_root": {"keep": [1, true]},
                "files": {
                    "imported": {
                        "filename": "recording", "fullname": "recording.opus", "filesize": 12,
                        "start_time": 1725000000000.5, "duration": 60000.25, "is_trash": false,
                        "status": "imported", "import_timestamp": "20240801_010203",
                        "matched_at": "2026-08-11T12:00:00+00:00", "imported_at": "2026-08-11T12:00:00+00:00"
                    },
                    "trash": {"filename": "trash", "status": "skipped", "skip_reason": "trashed"},
                    "available": {"filename": "available", "status": "available", "last_error": "download failed"},
                    "unknown": {"unknown_entry": {"preserve": "yes"}}
                }
            }),
        ),
        (
            BackendName::Obsidian,
            json!({
                "backend": "obsidian", "source_path": "/example/vault",
                "last_sync": "2026-08-11T12:00:00+00:00", "unknown_root": ["keep"],
                "files": {
                    "note.md": {"filename": "note.md", "title": "note", "mtime": 1725000000.5,
                        "content_hash": "abc", "status": "imported", "imported_at": "2026-08-11T12:00:00+00:00",
                        "segments": 2, "edit_count": 3},
                    "gone.md": {"status": "removed"},
                    "available.md": {"status": "available", "last_error": "pipeline failed"}
                }
            }),
        ),
        (
            BackendName::Audio,
            json!({
                "backend": "audio", "source_path": "/example/audio",
                "last_sync": "2026-08-11T12:00:00+00:00", "unknown_root": {"keep": true},
                "files": {
                    "nested/track.wav": {"filename": "track.wav", "filesize": 12, "hash": "abc",
                        "duration": 45.5, "status": "imported", "imported_at": "2026-08-11T12:00:00+00:00"},
                    "short.wav": {"status": "skipped", "duration": 29.5, "skip_reason": "too_short"},
                    "unreadable.wav": {"status": "unreadable"},
                    "retry.wav": {"status": "available", "last_error": "pipeline failed"},
                    "gone.wav": {"status": "removed"}
                }
            }),
        ),
    ]
}

#[test]
fn authored_state_uses_private_atomic_python_style_json() {
    let tree = TempDir::new().unwrap();
    let mut state = SyncState::empty(BackendName::Audio);
    state
        .root_mut()
        .insert("label".to_owned(), Value::String("café".to_owned()));
    write_sync_state(tree.path(), &state).unwrap();

    let path = state_path(tree.path(), BackendName::Audio);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "{\n  \"backend\": \"audio\",\n  \"files\": {},\n  \"label\": \"caf\\u00e9\"\n}"
    );
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(tree.path().join("imports"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn unreadable_sync_state_is_a_benign_recatalogue_value() {
    let tree = TempDir::new().unwrap();
    let path = state_path(tree.path(), BackendName::Plaud);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"not json").unwrap();
    assert!(matches!(
        read_sync_state(tree.path(), BackendName::Plaud),
        SyncStateRead::Unreadable { .. }
    ));
}

#[test]
fn known_state_numbers_over_i64_become_a_benign_recatalogue_value() {
    let tree = TempDir::new().unwrap();
    let path = state_path(tree.path(), BackendName::Audio);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        br#"{"files":{"track.wav":{"filesize":9223372036854775808}}}"#,
    )
    .unwrap();
    assert!(matches!(
        read_sync_state(tree.path(), BackendName::Audio),
        SyncStateRead::Unreadable { .. }
    ));
}

#[test]
fn state_root_is_an_ordered_object() {
    let state = SyncState::empty(BackendName::Plaud);
    assert_eq!(
        state.root(),
        &Map::from_iter([
            ("backend".to_owned(), Value::String("plaud".to_owned())),
            ("files".to_owned(), Value::Object(Map::new())),
        ])
    );
}
