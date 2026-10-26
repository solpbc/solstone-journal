// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::Value;
use solstone_core_import::{AdmittedFolder, Timestamp, admit_folder_bound, validate_timestamp};
use solstone_core_journal_io::JournalRoot;
use std::{fs, path::Path, sync::Arc};
use tower::ServiceExt;

fn stamp() -> Timestamp {
    validate_timestamp("20261026_123456").unwrap()
}
fn admit(root: &Arc<JournalRoot>, source: &Path, reference: &str) -> AdmittedFolder {
    admit_folder_bound(root, source, reference, &stamp()).unwrap()
}
async fn get(root: &Path, path: &str) -> Value {
    let response = crate::routes(root.to_path_buf())
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

#[tokio::test]
async fn native_folder_registration_is_visible_through_maintained_owner_read_routes() {
    let journal = crate::test_support::phase_root("empty");
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("note.md"), b"folder row content").unwrap();
    let folder = admit(&root, source.path(), "native-read-route");
    let summary = crate::register_admitted_folder(
        &root,
        &folder,
        "obsidian",
        "selected notes",
        "native-row",
        &stamp(),
        chrono_tz::UTC,
    )
    .unwrap();
    assert_eq!(summary["timestamp"], folder.import_id.as_str());
    assert_eq!(summary["client_item_id"], "native-row");
    let recovered = get(
        journal.path(),
        "/app/import/api/list?client_item_id=native-row",
    )
    .await;
    assert_eq!(recovered["total"], 1);
    assert_eq!(
        recovered["imports"][0]["timestamp"],
        folder.import_id.as_str()
    );
    assert_eq!(recovered["imports"][0]["client_item_id"], "native-row");
    let unrelated = get(
        journal.path(),
        "/app/import/api/list?client_item_id=another-selection",
    )
    .await;
    assert_eq!(unrelated["total"], 0);
    let detail = get(
        journal.path(),
        &format!("/app/import/api/{}", folder.import_id.as_str()),
    )
    .await;
    assert_eq!(detail["import_json"]["source_hint"], "obsidian");
    assert_eq!(detail["import_json"]["folder_snapshot"], folder.snapshot);
    assert_eq!(detail["import_json"]["source_hash"], folder.legacy_hash);
    assert_eq!(detail["import_json"]["is_local_path"], false);
    assert_eq!(detail["import_json"]["client_item_id"], "native-row");
    let list = get(journal.path(), "/app/import/api/list").await;
    assert!(
        list["imports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["timestamp"] == folder.import_id.as_str()),
        "{list}"
    );
    assert_eq!(
        fs::read(
            root.canonical_path()
                .join("imports")
                .join(folder.import_id.as_str())
                .join("tree/note.md")
        )
        .unwrap(),
        b"folder row content"
    );
}

#[tokio::test]
async fn same_legacy_hash_changed_folder_versions_keep_distinct_saved_rows_and_bytes() {
    let journal = crate::test_support::phase_root("empty");
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let source = tempfile::tempdir().unwrap();
    let note = source.path().join("note.md");
    let modified = filetime::FileTime::from_unix_time(1_700_000_000, 0);
    fs::write(&note, b"first note").unwrap();
    filetime::set_file_mtime(&note, modified).unwrap();
    let first = admit(&root, source.path(), "native-first");
    crate::register_admitted_folder(
        &root,
        &first,
        "obsidian",
        "notes",
        "first",
        &stamp(),
        chrono_tz::UTC,
    )
    .unwrap();
    fs::write(&note, b"other note").unwrap();
    filetime::set_file_mtime(&note, modified).unwrap();
    let second = admit(&root, source.path(), "native-second");
    assert_eq!(first.legacy_hash, second.legacy_hash);
    assert_ne!(first.snapshot, second.snapshot);
    assert_ne!(first.import_id, second.import_id);
    crate::register_admitted_folder(
        &root,
        &second,
        "obsidian",
        "notes",
        "second",
        &stamp(),
        chrono_tz::UTC,
    )
    .unwrap();
    for (folder, bytes) in [(&first, b"first note"), (&second, b"other note")] {
        assert_eq!(
            fs::read(
                root.canonical_path()
                    .join("imports")
                    .join(folder.import_id.as_str())
                    .join("tree/note.md")
            )
            .unwrap(),
            bytes
        );
        let detail = get(
            journal.path(),
            &format!("/app/import/api/{}", folder.import_id.as_str()),
        )
        .await;
        assert_eq!(detail["import_json"]["folder_snapshot"], folder.snapshot);
    }
    let list = get(journal.path(), "/app/import/api/list").await;
    for folder in [&first, &second] {
        assert!(
            list["imports"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["timestamp"] == folder.import_id.as_str())
        );
    }
}

#[test]
fn native_folder_registration_does_not_open_body_or_unregistered_source_setup() {
    let journal = crate::test_support::phase_root("empty");
    let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("note.md"), b"kept original").unwrap();
    let folder = admit(&root, source.path(), "unsupported-source");
    for source in ["apple_health", "oura", "made_up"] {
        assert!(
            crate::register_admitted_folder(
                &root,
                &folder,
                source,
                "selected folder",
                "body-refusal",
                &stamp(),
                chrono_tz::UTC
            )
            .is_err()
        );
    }
    let directory = root
        .canonical_path()
        .join("imports")
        .join(folder.import_id.as_str());
    assert!(!directory.join("import.json").exists());
    assert_eq!(
        fs::read(directory.join("tree/note.md")).unwrap(),
        b"kept original"
    );
}
