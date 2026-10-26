// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::JournalRoot;

fn fixture() -> (tempfile::TempDir, Arc<JournalRoot>, AdmittedFolder) {
    let container = tempfile::tempdir().unwrap();
    let journal = container.path().join("journal");
    fs::create_dir(&journal).unwrap();
    let source = container.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("note.md"), b"registered folder original").unwrap();
    let root = Arc::new(JournalRoot::open(&journal).unwrap());
    let folder = admit_folder_bound(
        &root,
        &source,
        "registration-control",
        &validate_timestamp("20261026_123456").unwrap(),
    )
    .unwrap();
    (container, root, folder)
}

#[test]
fn bound_metadata_keeps_the_durable_attempt_and_task_without_changing_originals() {
    let (_container, root, folder) = fixture();
    let first = json!({"source":"obsidian", "attempt":{"generation":7}, "task_id":"kept-task"});
    folder
        .write_metadata_bound(&root, first.as_object().unwrap())
        .unwrap();
    let replacement = json!({"source":"obsidian", "attempt":{"generation":1}, "task_id":"stale-task", "client_item_id":"owner-folder"});
    folder
        .write_metadata_bound(&root, replacement.as_object().unwrap())
        .unwrap();
    let read =
        crate::read_import_metadata(root.canonical_path(), folder.import_id.as_str()).unwrap();
    assert_eq!(read["attempt"]["generation"], 7);
    assert_eq!(read["task_id"], "kept-task");
    assert_eq!(read["client_item_id"], "owner-folder");
    assert_eq!(
        fs::read(
            root.canonical_path()
                .join("imports")
                .join(folder.import_id.as_str())
                .join("tree/note.md")
        )
        .unwrap(),
        b"registered folder original"
    );
}

#[test]
fn bound_metadata_refuses_each_replaced_namespace_and_preserves_foreign_sentinels() {
    for replaced in ["root", "imports", "directory", "tree"] {
        let (container, root, folder) = fixture();
        let journal = root.canonical_path().to_path_buf();
        let path = match replaced {
            "root" => journal.clone(),
            "imports" => journal.join("imports"),
            "directory" => journal.join("imports").join(folder.import_id.as_str()),
            "tree" => journal
                .join("imports")
                .join(folder.import_id.as_str())
                .join("tree"),
            _ => unreachable!(),
        };
        let moved = container.path().join("displaced");
        fs::rename(&path, &moved).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("sentinel"), b"foreign replacement").unwrap();
        assert!(
            folder
                .write_metadata_bound(&root, json!({"source":"obsidian"}).as_object().unwrap())
                .is_err(),
            "{replaced}"
        );
        assert_eq!(
            fs::read(path.join("sentinel")).unwrap(),
            b"foreign replacement"
        );
        let original_record = match replaced {
            "root" => moved
                .join("imports")
                .join(folder.import_id.as_str())
                .join("import.json"),
            "imports" => moved.join(folder.import_id.as_str()).join("import.json"),
            "directory" => moved.join("import.json"),
            "tree" => path.parent().unwrap().join("import.json"),
            _ => unreachable!(),
        };
        assert!(
            !original_record.exists(),
            "registration wrote after {replaced} replacement"
        );
        assert!(
            !path.join("import.json").exists(),
            "registration touched foreign {replaced}"
        );
    }
}

#[test]
fn bound_metadata_preserves_corrupt_and_symlink_records_on_refusal() {
    use std::os::unix::fs::symlink;
    let (container, root, folder) = fixture();
    let path = root
        .canonical_path()
        .join("imports")
        .join(folder.import_id.as_str())
        .join("import.json");
    fs::write(&path, b"{broken record").unwrap();
    assert!(
        folder
            .write_metadata_bound(&root, json!({"source":"obsidian"}).as_object().unwrap())
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"{broken record");
    fs::remove_file(&path).unwrap();
    let sentinel = container.path().join("foreign.json");
    fs::write(&sentinel, b"foreign record").unwrap();
    symlink(&sentinel, &path).unwrap();
    assert!(
        folder
            .write_metadata_bound(&root, json!({"source":"obsidian"}).as_object().unwrap())
            .is_err()
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"foreign record");
    assert!(fs::symlink_metadata(path).unwrap().is_symlink());
}

#[test]
#[cfg(feature = "full-tests")]
fn registration_waits_for_the_maintained_metadata_lock_before_reading_and_writing() {
    use std::sync::mpsc;
    use std::time::Duration;
    let (_container, root, folder) = fixture();
    let initial = json!({"attempt":{"generation":1},"task_id":"initial"});
    folder
        .write_metadata_bound(&root, initial.as_object().unwrap())
        .unwrap();
    let held = crate::hold_import_lock(root.canonical_path(), folder.import_id.as_str()).unwrap();
    let worker_root = Arc::clone(&root);
    let worker_folder = folder.clone();
    let (ready_send, ready) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        ready_send.send(()).unwrap();
        done_send
            .send(worker_folder.write_metadata_bound(
                &worker_root,
                json!({"source":"obsidian"}).as_object().unwrap(),
            ))
            .unwrap();
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let early = done.recv_timeout(Duration::from_millis(100));
    let premature = early.is_ok();
    let path = root
        .canonical_path()
        .join("imports")
        .join(folder.import_id.as_str())
        .join("import.json");
    fs::write(
        path,
        serde_json::to_vec(&json!({"attempt":{"generation":9},"task_id":"durable-owner"})).unwrap(),
    )
    .unwrap();
    drop(held);
    early
        .unwrap_or_else(|_| done.recv_timeout(Duration::from_secs(5)).unwrap())
        .unwrap();
    worker.join().unwrap();
    assert!(
        !premature,
        "registration wrote while the maintained metadata lock was held"
    );
    let read =
        crate::read_import_metadata(root.canonical_path(), folder.import_id.as_str()).unwrap();
    assert_eq!(read["attempt"]["generation"], 9);
    assert_eq!(read["task_id"], "durable-owner");
    assert_eq!(read["source"], "obsidian");
}
