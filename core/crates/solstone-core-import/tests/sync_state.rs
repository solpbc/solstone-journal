// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde_json::Value;
use solstone_core_import::{
    FileSyncBackend, FileSyncState, load_sync_state, state_path, write_sync_state,
};

#[test]
fn oracle_envelopes_round_trip_atomically_and_owner_private() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../../fixtures/import_sync_state_oracle.json"
    ))
    .expect("oracle fixture");
    let temporary = tempfile::tempdir().expect("temporary journal");
    let journal = temporary.path();

    for (name, backend) in [
        ("plaud", FileSyncBackend::Plaud),
        ("obsidian", FileSyncBackend::Obsidian),
        ("audio", FileSyncBackend::Audio),
    ] {
        let expected = oracle.get(name).expect("fixture envelope").clone();
        let state: FileSyncState = serde_json::from_value(expected.clone()).expect("typed state");
        write_sync_state(journal, &state).expect("atomic state write");
        let reread = load_sync_state(journal, backend)
            .expect("read state")
            .expect("state exists");
        assert_eq!(serde_json::to_value(reread).unwrap(), expected);
    }

    let imports = journal.join("imports");
    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&imports).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for backend in [
            FileSyncBackend::Plaud,
            FileSyncBackend::Obsidian,
            FileSyncBackend::Audio,
        ] {
            assert_eq!(
                fs::metadata(state_path(journal, backend))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    assert!(
        fs::read_dir(&imports).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("tmp"))
    );
}
