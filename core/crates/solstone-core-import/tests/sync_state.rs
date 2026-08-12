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
fn reference_shaped_state_round_trips_every_known_and_unknown_member() {
    let tree = TempDir::new().unwrap();
    let path = state_path(tree.path(), BackendName::Audio);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let reference = json!({
        "backend": "audio",
        "source_path": "/example/audio",
        "unknown_root": {"keep": [1, true]},
        "files": {
            "nested/track.wav": {
                "filename": "track.wav",
                "status": "available",
                "hash": "abc",
                "duration": 45,
                "unknown_entry": {"preserve": "yes"}
            }
        }
    });
    fs::write(&path, serde_json::to_vec_pretty(&reference).unwrap()).unwrap();

    let state = match read_sync_state(tree.path(), BackendName::Audio) {
        SyncStateRead::Loaded(state) => state,
        other => panic!("unexpected state read: {other:?}"),
    };
    write_sync_state(tree.path(), &state).unwrap();
    let round_tripped: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(round_tripped, reference);
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
