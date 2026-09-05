// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, EntityResolutionEntity, EntityResolutionOutcome, VoiceprintItem,
    hold_entity_trust_lock, load_entity_voiceprints_file, record_entity_resolution,
    repair_entity_identities, save_voiceprints_batch,
};

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-core-entity-lock-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn entity(id: Option<&str>, name: &str, blocked: bool) -> EntityResolutionEntity {
    EntityResolutionEntity {
        id: id.map(str::to_owned),
        name: name.to_owned(),
        aka: Vec::new(),
        emails: Vec::new(),
        blocked,
    }
}

fn journal_scope() -> Value {
    json!({"kind": "journal"})
}

fn origin(name: &str) -> Value {
    json!({"lane": "resolution-tests", "name": name})
}

fn resolve(
    root: &Path,
    query: &str,
    entities: &[EntityResolutionEntity],
    scope: Value,
    origin: Value,
    read_only: bool,
) -> Result<solstone_core_entity::EntityResolution, solstone_core_entity::EntityResolutionError> {
    record_entity_resolution(root, query, entities, scope, origin, 90.0, read_only)
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn test_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test-encoder".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn embedding(value: f32) -> Vec<f32> {
    let mut embedding = vec![0.0; 256];
    embedding[0] = value;
    embedding
}

fn fixture_entity_id() -> &'static str {
    "voiceprint_fixture"
}

fn voiceprint_path(root: &Path) -> PathBuf {
    root.join("entities")
        .join(fixture_entity_id())
        .join("voiceprints.npz")
}

#[test]
fn mutation_resolution_waits_for_the_outermost_trust_guard() {
    let temporary = TempDir::new();
    let outer = hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_path_buf();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = resolve(
            &root,
            "Sarah",
            &[
                entity(Some("sarah_connor"), "Sarah Connor", false),
                entity(Some("sarah_lee"), "Sarah Lee", false),
            ],
            journal_scope(),
            origin("lock-worker"),
            false,
        );
        finished_tx.send(result).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "resolution completed before the trust guard dropped"
    );
    drop(outer);
    let result = finished_rx
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .unwrap();
    assert_eq!(result.outcome, EntityResolutionOutcome::Ambiguous);
    worker.join().unwrap();
}

#[test]
fn identity_repair_waits_for_the_journal_trust_lock() {
    let temporary = TempDir::new();
    write_json(
        temporary.path(),
        "entities/alice/entity.json",
        &json!({"name": "Alice"}),
    );
    let outer = hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_path_buf();
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        finished_tx.send(repair_entity_identities(&root)).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    drop(outer);
    assert!(
        finished_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_ok()
    );
    worker.join().unwrap();
}

#[test]
fn second_thread_waits_for_the_outermost_guard_to_drop() {
    let temporary = TempDir::new();
    let outer = hold_entity_trust_lock(temporary.path()).unwrap();
    let inner = hold_entity_trust_lock(temporary.path()).unwrap();
    let root = temporary.path().to_path_buf();
    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();

    let worker = thread::spawn(move || {
        started_tx.send(()).unwrap();
        let lock = hold_entity_trust_lock(&root).unwrap();
        acquired_tx.send(()).unwrap();
        drop(lock);
    });

    started_rx.recv().unwrap();
    assert!(
        acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "second thread acquired before the outermost guard dropped"
    );
    drop(inner);
    assert!(
        acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "second thread acquired after only the nested guard dropped"
    );
    drop(outer);
    acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    worker.join().unwrap();
}

#[test]
fn concurrent_batch_saves_preserve_both_updates() {
    let temporary = TempDir::new();
    let identity_path = temporary
        .path()
        .join("entities")
        .join(fixture_entity_id())
        .join("entity.json");
    fs::create_dir_all(identity_path.parent().unwrap()).unwrap();
    fs::write(
        &identity_path,
        json!({"id": fixture_entity_id(), "name": "Voiceprint Fixture", "type": "Person"})
            .to_string(),
    )
    .unwrap();
    let path = voiceprint_path(temporary.path());
    let _ = fs::remove_file(&path);
    let root = Arc::new(temporary.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));
    let (sender, receiver) = mpsc::channel();
    let mut workers = Vec::new();
    for sentence_id in [21_u64, 22] {
        let root = Arc::clone(&root);
        let barrier = Arc::clone(&barrier);
        let sender = sender.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            let result = save_voiceprints_batch(
                &root,
                fixture_entity_id(),
                &[VoiceprintItem {
                    embedding: embedding(sentence_id as f32),
                    metadata: json!({
                        "day": "20260805",
                        "segment_key": format!("parallel-{sentence_id}"),
                        "source": "mic_audio",
                        "sentence_id": sentence_id,
                    }),
                }],
                &test_encoder(),
            );
            sender.send(result).unwrap();
        }));
    }
    drop(sender);
    for _ in 0..2 {
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap(),
            1
        );
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        load_entity_voiceprints_file(temporary.path(), fixture_entity_id())
            .unwrap()
            .rows,
        2
    );
}

mod merge_recovery {
    use super::*;
    use solstone_core_entity::{EntityMergeOptions, save_entity_identity};
    fn commit_entity_merge(
        journal: &Path,
        source: &str,
        target: &str,
        options: EntityMergeOptions,
    ) -> Result<solstone_core_entity::EntityMergeReport, solstone_core_entity::EntityMergeError>
    {
        solstone_core_entity::commit_entity_merge(journal, source, target, options, &test_encoder())
    }
    fn commit_entity_merge_with_injector(
        journal: &Path,
        source: &str,
        target: &str,
        options: EntityMergeOptions,
        injector: Option<&solstone_core_entity::MergeFailureInjectorForTest>,
    ) -> Result<solstone_core_entity::EntityMergeReport, solstone_core_entity::EntityMergeError>
    {
        solstone_core_entity::commit_entity_merge_with_injector_for_test(
            journal,
            source,
            target,
            options,
            &test_encoder(),
            injector,
        )
    }
    #[test]
    fn interrupted_merge_child() {
        let Some(journal) = std::env::var_os("SOLSTONE_ENTITY_MERGE_CRASH_JOURNAL") else {
            return;
        };
        let phase = std::env::var("SOLSTONE_ENTITY_MERGE_CRASH_PHASE").unwrap();
        let _ = commit_entity_merge_with_injector(
            Path::new(&journal),
            "source",
            "target",
            EntityMergeOptions::default(),
            Some(&move |observed, _| {
                if observed == phase {
                    std::process::exit(77);
                }
                false
            }),
        );
        panic!("crash point was not reached");
    }

    fn crash_merge(journal: &Path, phase: &str) {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "merge_recovery::interrupted_merge_child",
                "--nocapture",
            ])
            .env("SOLSTONE_ENTITY_MERGE_CRASH_JOURNAL", journal)
            .env("SOLSTONE_ENTITY_MERGE_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
    }

    fn recovery_fixture() -> PathBuf {
        let journal = std::env::temp_dir().join(format!(
            "solstone-merge-recovery-{}-{}",
            std::process::id(),
            NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&journal).unwrap();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
            let facet = journal.join(format!("facets/work/entities/{id}"));
            fs::create_dir_all(&facet).unwrap();
            fs::write(
                facet.join("entity.json"),
                format!("{{\"entity_id\":\"{id}\"}}"),
            )
            .unwrap();
            fs::write(
                facet.join("observations.jsonl"),
                format!("{{\"content\":\"{id} memory\"}}\n"),
            )
            .unwrap();
        }
        journal
    }

    #[test]
    fn checkpointed_process_interruption_recovers_through_merge_retry() {
        for phase in ["history", "edges"] {
            let journal = recovery_fixture();
            crash_merge(&journal, phase);
            assert!(!journal.join("entities/source").exists());
            assert!(
                journal
                    .join("health/entity-merge-recovery/state.json")
                    .exists()
            );
            let report =
                commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                    .unwrap();
            assert_eq!(report.target_id, "target");
            assert!(!journal.join("health/entity-merge-recovery").exists());
            let rows =
                fs::read_to_string(journal.join("facets/work/entities/target/observations.jsonl"))
                    .unwrap();
            assert!(rows.contains("source memory"));
            assert!(rows.contains("target memory"));
            fs::remove_dir_all(journal).unwrap();
        }
    }

    #[test]
    fn interrupted_recovery_refuses_new_owner_changes_and_retains_before_images() {
        let journal = recovery_fixture();
        crash_merge(&journal, "history");
        save_entity_identity(
            &journal,
            "target",
            &json!({"id":"target","name":"new owner name"}),
            None,
        )
        .unwrap();
        let before = fs::read(journal.join("entities/target/entity.json")).unwrap();
        let error =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap_err();
        assert!(error.to_string().contains("recovery conflicts"));
        assert_eq!(
            fs::read(journal.join("entities/target/entity.json")).unwrap(),
            before
        );
        assert!(
            journal
                .join("health/entity-merge-recovery/00000000.json")
                .exists()
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn interruption_inside_a_phase_refuses_without_discarding_recovery_evidence() {
        let journal = recovery_fixture();
        crash_merge(&journal, "facets");
        let before = fs::read(journal.join("facets/work/entities/target/entity.json")).unwrap();
        let error =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap_err();
        assert!(error.to_string().contains("recovery conflicts"));
        assert_eq!(
            fs::read(journal.join("facets/work/entities/target/entity.json")).unwrap(),
            before
        );
        assert!(
            journal
                .join("health/entity-merge-recovery/00000000.json")
                .exists()
        );
        fs::remove_dir_all(journal).unwrap();
    }

    #[test]
    fn interrupted_recovery_rejects_corrupt_or_missing_before_images_before_any_restore() {
        for corruption in [
            "nested_path",
            "missing_record",
            "new_child",
            "unexpected_record",
        ] {
            let journal = recovery_fixture();
            crash_merge(&journal, "history");
            let root = journal.join("health/entity-merge-recovery");
            match corruption {
                "nested_path" => {
                    let path = root.join("00000000.json");
                    let mut value: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    value["entries"][0]["path"] = json!("entities/other/entity.json");
                    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
                }
                "unexpected_record" => {
                    fs::write(root.join("unknown-before-image"), "keep evidence").unwrap();
                }
                "missing_record" => {
                    fs::remove_file(root.join("00000000.json")).unwrap();
                }
                "new_child" => {
                    fs::write(journal.join("entities/target/new-owner-file"), "new bytes").unwrap();
                }
                _ => unreachable!(),
            }
            let entities =
                solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap();
            let facets = solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap();
            let error =
                commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                    .unwrap_err();
            assert!(error.to_string().contains("recovery"), "{error}");
            assert_eq!(
                solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap(),
                entities
            );
            assert_eq!(
                solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap(),
                facets
            );
            assert!(root.join("state.json").exists());
            fs::remove_dir_all(journal).unwrap();
        }
    }
}
