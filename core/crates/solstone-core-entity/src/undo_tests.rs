// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};
use solstone_core_journal_io::{AtomicWriteOptions, JsonWriteOptions, write_json, write_jsonl};

use super::store::undo_entity_merge_with_injector;
use super::store::voiceprints::write_voiceprints_npz as write_voiceprints_npz_with_envelope;
use crate::{
    EncoderIdentity, EntityMergeError, EntityMergeOptions,
    commit_entity_merge as commit_entity_merge_with_encoder, guard_restore_does_not_cross_merge,
    read_entity_identity, read_visible_history, save_entity_identity, undo_entity_merge,
};

static NEXT_UNDO_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test-encoder".to_owned(),
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        width: 256,
    }
}

fn commit_entity_merge(
    journal: &std::path::Path,
    source_id: &str,
    target_id: &str,
    options: EntityMergeOptions,
) -> Result<crate::EntityMergeReport, EntityMergeError> {
    commit_entity_merge_with_encoder(journal, source_id, target_id, options, &test_encoder())
}

fn write_voiceprints_npz(
    embeddings: &[f32],
    metadata: &[String],
) -> Result<Vec<u8>, crate::VoiceprintNpzError> {
    write_voiceprints_npz_with_envelope(
        embeddings,
        metadata,
        &crate::VoiceprintEnvelope::default(),
        &test_encoder(),
    )
}

fn undo_journal() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-undo-{}-{}",
        std::process::id(),
        NEXT_UNDO_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    fs::canonicalize(path).unwrap()
}

fn journal_tree(journal: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    fn collect(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut Vec<(String, Vec<u8>)>,
    ) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if entry.file_type().unwrap().is_dir() {
                files.push((format!("{relative}/"), Vec::new()));
                collect(root, &path, files);
            } else if entry.file_type().unwrap().is_file() {
                files.push((relative, fs::read(path).unwrap()));
            }
        }
    }

    let mut files = Vec::new();
    collect(journal, journal, &mut files);
    files
}

fn comparable_journal_tree(journal: &std::path::Path, target_id: &str) -> Vec<(String, Vec<u8>)> {
    fn excluded(relative: &str, target_id: &str) -> bool {
        relative.ends_with(".lock")
            || relative == "indexer/"
            || relative.starts_with("indexer/")
            || relative == format!("entities/{target_id}/history/")
            || relative.starts_with(&format!("entities/{target_id}/history/"))
            || relative == "logs/entity-merges.jsonl"
            || relative == "awareness/discovery_clusters.json"
    }

    fn collect(
        root: &std::path::Path,
        directory: &std::path::Path,
        target_id: &str,
        files: &mut Vec<(String, Vec<u8>)>,
    ) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let relative_directory = format!("{relative}/");
            if excluded(&relative, target_id) || excluded(&relative_directory, target_id) {
                continue;
            }
            if entry.file_type().unwrap().is_dir() {
                files.push((relative_directory, Vec::new()));
                collect(root, &path, target_id, files);
            } else if entry.file_type().unwrap().is_file() {
                files.push((relative, fs::read(path).unwrap()));
            }
        }
    }

    let mut files = Vec::new();
    collect(journal, journal, target_id, &mut files);
    files
}

#[test]
fn undo_reverts_target_identity_restores_source_and_removes_payload() {
    let journal = undo_journal();
    let source = json!({"id":"source","name":"Source","aka":[],"emails":[],"title":"Engineer"});
    let target = json!({"id":"target","name":"Target","aka":[],"emails":[],"title":"Director"});
    save_entity_identity(&journal, "source", &source, None).unwrap();
    save_entity_identity(&journal, "target", &target, None).unwrap();
    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert_eq!(
        read_entity_identity(&journal, "target")
            .unwrap()
            .unwrap()
            .value(),
        &target
    );
    assert!(journal.join("entities/source/entity.json").exists());
    assert!(read_entity_identity(&journal, "source").unwrap().is_some());
    assert!(
        !journal
            .join(format!(
                "entities/target/history/private/{}.json",
                merge.merge_id
            ))
            .exists()
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_produces_merge_undo_event_that_arms_the_restore_guard() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let events = read_visible_history(&journal, "target").unwrap();
    let merge_undo = events
        .iter()
        .find(|event| event.value()["kind"] == "merge_undo")
        .unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(merge_undo, &events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot target a recorded merge event; use recorded-merge undo instead"
    );
    let earlier = events
        .iter()
        .find(|event| !matches!(event.value()["kind"].as_str(), Some("merge" | "merge_undo")))
        .unwrap();
    assert_eq!(
        guard_restore_does_not_cross_merge(earlier, &events)
            .unwrap_err()
            .to_string(),
        "generic identity restore cannot cross a recorded merge event; use recorded-merge undo instead"
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_refuses_invalid_active_sibling_payload_without_mutation() {
    let journal = undo_journal();
    for id in ["source1", "source2", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let first =
        commit_entity_merge(&journal, "source1", "target", EntityMergeOptions::default()).unwrap();
    let second =
        commit_entity_merge(&journal, "source2", "target", EntityMergeOptions::default()).unwrap();
    let sibling_path = journal.join(format!(
        "entities/target/history/private/{}.json",
        second.merge_id
    ));
    fs::write(&sibling_path, b"not json").unwrap();
    let index_before = solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap();
    let before = journal_tree(&journal);

    let error = undo_entity_merge(&journal, &first.merge_id, Value::Null).unwrap_err();
    assert!(error.to_string().contains(&second.merge_id));
    assert_eq!(journal_tree(&journal), before);
    assert_eq!(
        solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap(),
        index_before
    );
    assert!(
        journal
            .join(format!(
                "entities/target/history/private/{}.json",
                first.merge_id
            ))
            .exists()
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_refuses_hostile_payload_without_mutation() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let payload_path = journal.join(format!(
        "entities/target/history/private/{}.json",
        merge.merge_id
    ));
    let mut payload: Value = serde_json::from_slice(&fs::read(&payload_path).unwrap()).unwrap();
    payload["source_id"] = json!("../outside");
    fs::write(&payload_path, serde_json::to_vec(&payload).unwrap()).unwrap();
    let index_before = solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap();
    let tree_before = journal_tree(&journal);

    assert!(undo_entity_merge(&journal, &merge.merge_id, Value::Null).is_err());
    assert_eq!(journal_tree(&journal), tree_before);
    assert_eq!(
        solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap(),
        index_before
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_respects_sibling_alias_support() {
    let journal = undo_journal();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"Target","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    for id in ["source1", "source2"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":["Shared Alias"],"emails":[]}),
            None,
        )
        .unwrap();
    }

    let first =
        commit_entity_merge(&journal, "source1", "target", EntityMergeOptions::default()).unwrap();
    commit_entity_merge(&journal, "source2", "target", EntityMergeOptions::default()).unwrap();

    undo_entity_merge(&journal, &first.merge_id, Value::Null).unwrap();
    assert_eq!(
        read_entity_identity(&journal, "target")
            .unwrap()
            .unwrap()
            .value()["aka"],
        json!(["Shared Alias"])
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_preserves_later_owner_edit() {
    let journal = undo_journal();
    save_entity_identity(
        &journal,
        "source",
        &json!({"id":"source","name":"Source","aka":["Merge Alias"],"emails":[],"title":"Engineer"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "target",
        &json!({"id":"target","name":"Target","aka":[],"emails":[]}),
        None,
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let mut owner_edit = read_entity_identity(&journal, "target")
        .unwrap()
        .unwrap()
        .value()
        .clone();
    owner_edit["aka"] = json!(["Merge Alias", "Owner Alias"]);
    owner_edit["title"] = json!("Lead");
    save_entity_identity(&journal, "target", &owner_edit, None).unwrap();

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let target = read_entity_identity(&journal, "target").unwrap().unwrap();
    assert_eq!(target.value()["aka"], json!(["Owner Alias"]));
    assert_eq!(target.value()["title"], json!("Lead"));
    fs::remove_dir_all(journal).unwrap();
}

#[cfg(unix)]
#[test]
fn undo_restores_file_modes_not_directory_modes() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    let source_directory = journal.join("entities/source");
    let source_identity = source_directory.join("entity.json");
    fs::set_permissions(&source_identity, fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(&source_directory, fs::Permissions::from_mode(0o750)).unwrap();
    let source_facet = journal.join("facets/work/entities/source");
    fs::create_dir_all(&source_facet).unwrap();
    fs::write(
        source_facet.join("entity.json"),
        br#"{"entity_id":"source"}"#,
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();

    assert_eq!(
        fs::metadata(&source_identity).unwrap().permissions().mode() & 0o7777,
        0o640
    );
    let default_directory = journal.join("default-directory-mode");
    fs::create_dir(&default_directory).unwrap();
    assert_eq!(
        fs::metadata(&source_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        fs::metadata(&default_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_byte_identical_journal_outside_excluded_paths() {
    let journal = undo_journal();
    for id in ["source", "target", "other"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }
    fs::create_dir_all(journal.join("logs")).unwrap();
    let discovery = journal.join("awareness/discovery_clusters.json");
    fs::create_dir_all(discovery.parent().unwrap()).unwrap();
    fs::write(&discovery, b"{\"clusters\":[]}").unwrap();

    let source_facet = journal.join("facets/work/entities/source");
    fs::create_dir_all(&source_facet).unwrap();
    write_json(
        source_facet.join("entity.json"),
        &json!({"entity_id":"source","description":"source relationship"}),
        JsonWriteOptions {
            indent: Some(2),
            sort_keys: false,
            mode: None,
        },
    )
    .unwrap();
    let labels = journal.join("chronicle/20260102/080000_300/talents/speaker_labels.json");
    fs::create_dir_all(labels.parent().unwrap()).unwrap();
    write_json(
        &labels,
        &json!({"labels":[{"speaker":"source"}]}),
        JsonWriteOptions {
            indent: Some(2),
            sort_keys: false,
            mode: None,
        },
    )
    .unwrap();
    let activity = journal.join("facets/work/activities/20260102.jsonl");
    fs::create_dir_all(activity.parent().unwrap()).unwrap();
    write_jsonl(
        &activity,
        vec![json!({"id":"activity","active_entities":["source"]})],
        AtomicWriteOptions::default(),
    )
    .unwrap();
    let observations = journal.join("facets/work/entities/other/observations.jsonl");
    fs::create_dir_all(observations.parent().unwrap()).unwrap();
    write_jsonl(
        &observations,
        vec![json!({"observed_at":1,"relation":{"kind":"works-with","target_entity_id":"source","target_name":"source"}})],
        AtomicWriteOptions::default(),
    )
    .unwrap();
    let source_voiceprints = journal.join("entities/source/voiceprints.npz");
    fs::write(
        &source_voiceprints,
        write_voiceprints_npz(
            &[2.0; 256],
            &[
                "{\"day\":\"d\",\"segment_key\":\"s\",\"source\":\"x\",\"sentence_id\":\"1\"}"
                    .to_owned(),
            ],
        )
        .unwrap(),
    )
    .unwrap();

    solstone_core_indexer_store::scan::rebuild_edges(&journal).unwrap();
    let index_before = solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap();
    let tree_before = comparable_journal_tree(&journal, "target");

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();

    assert_eq!(comparable_journal_tree(&journal, "target"), tree_before);
    assert_eq!(
        solstone_core_indexer_store::merge::fingerprint_edge_rows(&journal).unwrap(),
        index_before
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_rebase_chain_restores_intermediate_target() {
    let journal = undo_journal();
    for id in ["a", "b", "c"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id":id,"name":id,"aka":[],"emails":[]}),
            None,
        )
        .unwrap();
    }

    let child = commit_entity_merge(&journal, "a", "b", EntityMergeOptions::default()).unwrap();
    let child_path = journal.join(format!(
        "entities/b/history/private/{}.json",
        child.merge_id
    ));
    let child_before_rebase = fs::read(&child_path).unwrap();
    let parent = commit_entity_merge(&journal, "b", "c", EntityMergeOptions::default()).unwrap();
    let parent_payload: Value = serde_json::from_slice(
        &fs::read(journal.join(format!(
            "entities/c/history/private/{}.json",
            parent.merge_id
        )))
        .unwrap(),
    )
    .unwrap();
    assert!(
        parent_payload["manifest"]["rebased_merge_ids"]
            .as_array()
            .unwrap()
            .contains(&Value::String(child.merge_id.clone()))
    );
    assert!(!child_path.exists());

    undo_entity_merge(&journal, &parent.merge_id, Value::Null).unwrap();
    assert!(read_entity_identity(&journal, "b").unwrap().is_some());
    assert_eq!(fs::read(&child_path).unwrap(), child_before_rebase);

    undo_entity_merge(&journal, &child.merge_id, Value::Null).unwrap();
    assert!(read_entity_identity(&journal, "a").unwrap().is_some());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_removes_moved_target_facet_relationship() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let source_facet = journal.join("facets/work/entities/source");
    fs::create_dir_all(&source_facet).unwrap();
    fs::write(
        source_facet.join("entity.json"),
        br#"{"entity_id":"source"}"#,
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let target_facet = journal.join("facets/work/entities/target");
    assert!(!target_facet.exists());
    let source_after: Value =
        serde_json::from_slice(&fs::read(source_facet.join("entity.json")).unwrap()).unwrap();
    assert_eq!(source_after["entity_id"], "target");
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert!(!target_facet.exists());
    let source_undone: Value =
        serde_json::from_slice(&fs::read(source_facet.join("entity.json")).unwrap()).unwrap();
    assert_eq!(source_undone["entity_id"], "source");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_merged_target_facet_relationship() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let source_facet = journal.join("facets/work/entities/source");
    let target_facet = journal.join("facets/work/entities/target");
    fs::create_dir_all(&source_facet).unwrap();
    fs::create_dir_all(&target_facet).unwrap();
    fs::write(
        source_facet.join("entity.json"),
        br#"{"entity_id":"source","attached_at":"2026-01-01"}"#,
    )
    .unwrap();
    let target_before = json!({
        "entity_id": "target",
        "attached_at": "2026-02-01",
        "description": "target description"
    });
    fs::write(
        target_facet.join("entity.json"),
        serde_json::to_vec(&target_before).unwrap(),
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored: Value =
        serde_json::from_slice(&fs::read(target_facet.join("entity.json")).unwrap()).unwrap();
    assert_eq!(restored, target_before);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_target_observations_from_merged_facet() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let source_facet = journal.join("facets/work/entities/source");
    let target_facet = journal.join("facets/work/entities/target");
    fs::create_dir_all(&source_facet).unwrap();
    fs::create_dir_all(&target_facet).unwrap();
    fs::write(
        source_facet.join("entity.json"),
        br#"{"entity_id":"source"}"#,
    )
    .unwrap();
    fs::write(
        target_facet.join("entity.json"),
        br#"{"entity_id":"target"}"#,
    )
    .unwrap();
    let source_observation = json!({"content": "source", "observed_at": "2026-01-01"});
    let target_observation = json!({"content": "target", "observed_at": "2026-01-02"});
    fs::write(
        source_facet.join("observations.jsonl"),
        format!("{}\n", source_observation),
    )
    .unwrap();
    fs::write(
        target_facet.join("observations.jsonl"),
        format!("{}\n", target_observation),
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored = fs::read_to_string(target_facet.join("observations.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(restored, vec![target_observation]);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_observation_relation_target() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let observations = journal.join("facets/work/entities/other/observations.jsonl");
    fs::create_dir_all(observations.parent().unwrap()).unwrap();
    fs::write(
        &observations,
        b"{\"content\":\"note\",\"observed_at\":\"2026-01-01\",\"relation\":{\"kind\":\"works-with\",\"target_entity_id\":\"source\"}}\n",
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let remapped: Value =
        serde_json::from_str(&fs::read_to_string(&observations).unwrap()).unwrap();
    assert_eq!(remapped["relation"]["target_entity_id"], "target");
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored: Value =
        serde_json::from_str(&fs::read_to_string(&observations).unwrap()).unwrap();
    assert_eq!(restored["relation"]["target_entity_id"], "source");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_segment_speaker_label() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let labels = journal.join("chronicle/20260102/080000_300/talents/speaker_labels.json");
    fs::create_dir_all(labels.parent().unwrap()).unwrap();
    fs::write(&labels, br#"{"labels":[{"speaker":"source"}]}"#).unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let rewritten: Value = serde_json::from_slice(&fs::read(&labels).unwrap()).unwrap();
    assert_eq!(rewritten["labels"][0]["speaker"], "target");
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored: Value = serde_json::from_slice(&fs::read(&labels).unwrap()).unwrap();
    assert_eq!(restored["labels"][0]["speaker"], "source");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_activity_active_entity() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let activities = journal.join("facets/work/activities/20260102.jsonl");
    fs::create_dir_all(activities.parent().unwrap()).unwrap();
    fs::write(
        &activities,
        b"{\"id\":\"activity\",\"active_entities\":[\"source\"]}\n",
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let rewritten: Value = serde_json::from_str(&fs::read_to_string(&activities).unwrap()).unwrap();
    assert_eq!(rewritten["active_entities"][0], "target");
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored: Value = serde_json::from_str(&fs::read_to_string(&activities).unwrap()).unwrap();
    assert_eq!(restored["active_entities"][0], "source");
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn facets_undo_injection_rolls_back_and_retry_succeeds() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let moved_source = journal.join("facets/moved/entities/source");
    fs::create_dir_all(&moved_source).unwrap();
    fs::write(
        moved_source.join("entity.json"),
        br#"{"entity_id":"source"}"#,
    )
    .unwrap();
    let merged_source = journal.join("facets/merged/entities/source");
    let merged_target = journal.join("facets/merged/entities/target");
    fs::create_dir_all(&merged_source).unwrap();
    fs::create_dir_all(&merged_target).unwrap();
    fs::write(
        merged_source.join("entity.json"),
        br#"{"entity_id":"source","attached_at":"2026-01-01"}"#,
    )
    .unwrap();
    let merged_before = json!({"entity_id":"target","attached_at":"2026-02-01"});
    fs::write(
        merged_target.join("entity.json"),
        serde_json::to_vec(&merged_before).unwrap(),
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let moved_after_merge = fs::read(moved_source.join("entity.json")).unwrap();
    let merged_after_merge = fs::read(merged_target.join("entity.json")).unwrap();
    let error = undo_entity_merge_with_injector(
        &journal,
        &merge.merge_id,
        Value::Null,
        Some(&|phase, artifact_index| phase == "facets" && artifact_index == 0),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "entity merge undo failed during facets: injected failure after facets artifact 0"
    );
    assert_eq!(
        fs::read(moved_source.join("entity.json")).unwrap(),
        moved_after_merge
    );
    assert_eq!(
        fs::read(merged_target.join("entity.json")).unwrap(),
        merged_after_merge
    );

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let restored_moved: Value =
        serde_json::from_slice(&fs::read(moved_source.join("entity.json")).unwrap()).unwrap();
    assert_eq!(restored_moved["entity_id"], "source");
    assert!(!journal.join("facets/moved/entities/target").exists());
    let restored: Value =
        serde_json::from_slice(&fs::read(merged_target.join("entity.json")).unwrap()).unwrap();
    assert_eq!(restored, merged_before);
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn observations_undo_injection_rolls_back_and_retry_succeeds() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let first = journal.join("facets/one/entities/other/observations.jsonl");
    let second = journal.join("facets/two/entities/other/observations.jsonl");
    for path in [&first, &second] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            b"{\"relation\":{\"kind\":\"works-with\",\"target_entity_id\":\"source\"}}\n",
        )
        .unwrap();
    }

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let first_after_merge = fs::read(&first).unwrap();
    let second_after_merge = fs::read(&second).unwrap();
    assert!(
        undo_entity_merge_with_injector(
            &journal,
            &merge.merge_id,
            Value::Null,
            Some(&|phase, artifact_index| phase == "observations" && artifact_index == 0),
        )
        .is_err()
    );
    assert_eq!(fs::read(&first).unwrap(), first_after_merge);
    assert_eq!(fs::read(&second).unwrap(), second_after_merge);

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    for path in [&first, &second] {
        let restored: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(restored["relation"]["target_entity_id"], "source");
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn segments_undo_injection_rolls_back_and_retry_succeeds() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let first = journal.join("chronicle/20260102/080000_300/talents/speaker_labels.json");
    let second = journal.join("chronicle/20260102/090000_300/talents/speaker_labels.json");
    for path in [&first, &second] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, br#"{"labels":[{"speaker":"source"}]}"#).unwrap();
    }

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let first_after_merge = fs::read(&first).unwrap();
    let second_after_merge = fs::read(&second).unwrap();
    assert!(
        undo_entity_merge_with_injector(
            &journal,
            &merge.merge_id,
            Value::Null,
            Some(&|phase, artifact_index| phase == "segments" && artifact_index == 0),
        )
        .is_err()
    );
    assert_eq!(fs::read(&first).unwrap(), first_after_merge);
    assert_eq!(fs::read(&second).unwrap(), second_after_merge);

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    for path in [&first, &second] {
        let restored: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(restored["labels"][0]["speaker"], "source");
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn activities_undo_injection_rolls_back_and_retry_succeeds() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(
            &journal,
            id,
            &json!({"id": id, "name": id, "aka": [], "emails": []}),
            None,
        )
        .unwrap();
    }
    let first = journal.join("facets/work/activities/20260102.jsonl");
    let second = journal.join("facets/work/activities/20260103.jsonl");
    for path in [&first, &second] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            b"{\"id\":\"activity\",\"active_entities\":[\"source\"]}\n",
        )
        .unwrap();
    }

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let first_after_merge = fs::read(&first).unwrap();
    let second_after_merge = fs::read(&second).unwrap();
    assert!(
        undo_entity_merge_with_injector(
            &journal,
            &merge.merge_id,
            Value::Null,
            Some(&|phase, artifact_index| phase == "activities" && artifact_index == 0),
        )
        .is_err()
    );
    assert_eq!(fs::read(&first).unwrap(), first_after_merge);
    assert_eq!(fs::read(&second).unwrap(), second_after_merge);

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    for path in [&first, &second] {
        let restored: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(restored["active_entities"][0], "source");
    }
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_restores_divergent_relationship_observations() {
    let journal = undo_journal();
    save_entity_identity(
        &journal,
        "src-dir",
        &json!({"id":"src-id","name":"src-id","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "tgt-dir",
        &json!({"id":"tgt-id","name":"tgt-id","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    let src_rel = journal.join("facets/work/entities/src-rel");
    let tgt_rel = journal.join("facets/work/entities/tgt-rel");
    fs::create_dir_all(&src_rel).unwrap();
    fs::create_dir_all(&tgt_rel).unwrap();
    fs::write(src_rel.join("entity.json"), br#"{"entity_id":"src-id"}"#).unwrap();
    fs::write(tgt_rel.join("entity.json"), br#"{"entity_id":"tgt-id"}"#).unwrap();
    fs::write(
        src_rel.join("observations.jsonl"),
        b"{\"content\":\"source\"}\n",
    )
    .unwrap();
    fs::write(
        tgt_rel.join("observations.jsonl"),
        b"{\"content\":\"target\"}\n",
    )
    .unwrap();

    let merge = commit_entity_merge(
        &journal,
        "src-dir",
        "tgt-dir",
        EntityMergeOptions::default(),
    )
    .unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert_eq!(
        fs::read_to_string(src_rel.join("observations.jsonl")).unwrap(),
        "{\"content\":\"source\"}\n"
    );
    assert_eq!(
        fs::read_to_string(tgt_rel.join("observations.jsonl")).unwrap(),
        "{\"content\":\"target\"}\n"
    );
    assert!(!journal.join("facets/work/entities/src-id").exists());
    assert!(!journal.join("facets/work/entities/tgt-id").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_relink_restores_source_entity_id() {
    let journal = undo_journal();
    save_entity_identity(
        &journal,
        "src-dir",
        &json!({"id":"src-id","name":"src-id","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &journal,
        "tgt-dir",
        &json!({"id":"tgt-id","name":"tgt-id","aka":[],"emails":[]}),
        None,
    )
    .unwrap();
    let src_rel = journal.join("facets/work/entities/src-rel");
    fs::create_dir_all(&src_rel).unwrap();
    fs::write(src_rel.join("entity.json"), br#"{"entity_id":"src-id"}"#).unwrap();
    fs::write(
        src_rel.join("observations.jsonl"),
        b"{\"content\":\"kept\"}\n",
    )
    .unwrap();

    let merge = commit_entity_merge(
        &journal,
        "src-dir",
        "tgt-dir",
        EntityMergeOptions::default(),
    )
    .unwrap();
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    let link: Value =
        serde_json::from_slice(&fs::read(src_rel.join("entity.json")).unwrap()).unwrap();
    assert_eq!(link["entity_id"], "src-id");
    assert_eq!(
        fs::read_to_string(src_rel.join("observations.jsonl")).unwrap(),
        "{\"content\":\"kept\"}\n"
    );
    assert!(!journal.join("facets/work/entities/tgt-id").exists());
    fs::remove_dir_all(journal).unwrap();
}

fn seed_identity_at(journal: &std::path::Path, directory: &str, effective_id: &str) {
    let path = journal.join(format!("entities/{directory}/entity.json"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!(r#"{{"id":"{effective_id}"}}"#)).unwrap();
}

#[test]
fn undo_entity_merge_restores_remapped_source_and_target_directories() {
    let journal = undo_journal();
    seed_identity_at(&journal, "dir-s", "source");
    seed_identity_at(&journal, "dir-t", "target");
    let source_voice = write_voiceprints_npz(
        &[2.0; 256],
        &[r#"{"day":"d","segment_key":"s","source":"x","sentence_id":"S"}"#.to_owned()],
    )
    .unwrap();
    let target_voice = write_voiceprints_npz(
        &[3.0; 256],
        &[r#"{"day":"d","segment_key":"s","source":"x","sentence_id":"U"}"#.to_owned()],
    )
    .unwrap();
    fs::write(
        journal.join("entities/dir-s/voiceprints.npz"),
        &source_voice,
    )
    .unwrap();
    fs::write(
        journal.join("entities/dir-t/voiceprints.npz"),
        &target_voice,
    )
    .unwrap();

    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    assert!(!journal.join("entities/dir-s").exists());
    let report = undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert_eq!(report.target_id, "target");
    assert_eq!(report.source_id, "source");

    assert!(journal.join("entities/dir-s/entity.json").is_file());
    assert_eq!(
        fs::read(journal.join("entities/dir-s/voiceprints.npz")).unwrap(),
        source_voice
    );
    assert_eq!(
        fs::read(journal.join("entities/dir-t/voiceprints.npz")).unwrap(),
        target_voice
    );
    assert!(
        !journal
            .join(format!(
                "entities/dir-t/history/private/{}.json",
                merge.merge_id
            ))
            .exists()
    );
    assert!(!journal.join("entities/source").exists());
    assert!(!journal.join("entities/target").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn failed_undo_preserves_unrelated_index_commit_and_pending_repair_retries() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
    }
    let facet = journal.join("facets/work/entities/source");
    fs::create_dir_all(&facet).unwrap();
    fs::write(facet.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
    let merge =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let connection = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    connection
        .execute("CREATE TABLE independent_write(value TEXT)", [])
        .unwrap();
    let failure = undo_entity_merge_with_injector(
        &journal,
        &merge.merge_id,
        Value::Null,
        Some(&move |phase, _| {
            if phase == "facets" {
                connection
                    .execute("INSERT INTO independent_write VALUES ('acknowledged')", [])
                    .unwrap();
                true
            } else {
                false
            }
        }),
    );
    assert!(failure.is_err());
    let fresh = solstone_core_indexer_store::db::open_index(&journal).unwrap();
    let value: String = fresh
        .query_row("SELECT value FROM independent_write", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "acknowledged");
    drop(fresh);

    let pending = undo_entity_merge_with_injector(
        &journal,
        &merge.merge_id,
        Value::Null,
        Some(&|phase, _| phase == "edges"),
    )
    .unwrap_err();
    assert!(
        pending
            .to_string()
            .contains("undo committed; index repair pending")
    );
    assert!(journal.join("entities/source/entity.json").exists());
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert!(!journal.join("health/entity-merge-recovery").exists());
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_preserves_new_relinked_facet_memory_and_refuses_recreated_source() {
    for occupied in [
        None,
        Some("entities/source"),
        Some("facets/work/entities/source"),
    ] {
        let journal = undo_journal();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        }
        let facet = journal.join("facets/work/entities/source");
        fs::create_dir_all(&facet).unwrap();
        fs::write(facet.join("entity.json"), br#"{"entity_id":"source"}"#).unwrap();
        // A merged facet leaves an absent source destination; relink intentionally keeps it.
        if occupied == Some("facets/work/entities/source") {
            let target = journal.join("facets/work/entities/target");
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("entity.json"), br#"{"entity_id":"target"}"#).unwrap();
        }
        let merged =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap();
        if let Some(occupied) = occupied {
            let path = journal.join(occupied);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("new-owner-file"), "keep me").unwrap();
            let entities =
                solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap();
            let facets = solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap();
            let error = undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap_err();
            assert!(error.to_string().contains("source destination is occupied"));
            assert_eq!(
                solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap(),
                entities
            );
            assert_eq!(
                solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap(),
                facets
            );
        } else {
            fs::write(
                facet.join("observations.jsonl"),
                "{\"content\":\"new owner memory\"}\n",
            )
            .unwrap();
            undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap();
            assert!(
                fs::read_to_string(facet.join("observations.jsonl"))
                    .unwrap()
                    .contains("new owner memory")
            );
            let link: Value =
                serde_json::from_slice(&fs::read(facet.join("entity.json")).unwrap()).unwrap();
            assert_eq!(link["entity_id"], "source");
        }
        fs::remove_dir_all(journal).unwrap();
    }
}

#[test]
fn merged_facet_undo_without_recorded_after_state_refuses_before_source_mutation() {
    let journal = undo_journal();
    for id in ["source", "target"] {
        save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        let facet = journal.join(format!("facets/work/entities/{id}"));
        fs::create_dir_all(&facet).unwrap();
        fs::write(
            facet.join("entity.json"),
            serde_json::to_vec(&json!({"entity_id":id})).unwrap(),
        )
        .unwrap();
    }
    let merged =
        commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default()).unwrap();
    let mut payload = super::store::merge_payload::load_entity_merge_payload(
        &journal,
        "target",
        &merged.merge_id,
    )
    .unwrap();
    payload["manifest"]["facets"]["entries"][0]
        .as_object_mut()
        .unwrap()
        .remove("undo_expected");
    super::store::merge_payload::record_entity_merge_payload(
        &journal,
        "target",
        &merged.merge_id,
        &payload,
    )
    .unwrap();
    let entities = solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap();
    let facets = solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap();
    let error = undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("recorded target state is missing")
    );
    assert_eq!(
        solstone_core_journal_io::capture_snapshot(&journal, "entities").unwrap(),
        entities
    );
    assert_eq!(
        solstone_core_journal_io::capture_snapshot(&journal, "facets").unwrap(),
        facets
    );
    fs::remove_dir_all(journal).unwrap();
}

#[test]
fn undo_refuses_later_positional_edits_before_any_source_restoration() {
    for kind in [
        "labels",
        "corrections",
        "activities",
        "relations",
        "voiceprints",
    ] {
        let journal = undo_journal();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        }
        let (relative, before, later) = match kind {
            "labels" => (
                "chronicle/20260102/080000_300/talents/speaker_labels.json",
                json!({"labels":[{"speaker":"source"}]}),
                json!({"labels":[{"speaker":"later-owner-choice"}]}),
            ),
            "corrections" => (
                "chronicle/20260102/080000_300/talents/speaker_corrections.json",
                json!({"corrections":[{"original_speaker":"source","corrected_speaker":"source"}]}),
                json!({"corrections":[{"original_speaker":"target","corrected_speaker":"later-owner-choice"}]}),
            ),
            "activities" => (
                "facets/work/activities/20260102.jsonl",
                json!({"id":"activity","active_entities":["source"]}),
                json!({"id":"activity","active_entities":["later-owner-choice"]}),
            ),
            "relations" => (
                "facets/work/entities/other/observations.jsonl",
                json!({"content":"note","relation":{"target_entity_id":"source"}}),
                json!({"content":"note","relation":{"target_entity_id":"later-owner-choice"}}),
            ),
            "voiceprints" => ("entities/target/voiceprints.npz", Value::Null, Value::Null),
            _ => unreachable!(),
        };
        if relative.ends_with(".jsonl") {
            write_jsonl(
                journal.join(relative),
                vec![before],
                AtomicWriteOptions::default(),
            )
            .unwrap();
        } else if kind != "voiceprints" {
            write_json(journal.join(relative), &before, JsonWriteOptions::default()).unwrap();
        }
        let merged =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap();
        if kind == "voiceprints" {
            let bytes = write_voiceprints_npz(&vec![1.0; 256], &[json!({"day":"20260102","segment_key":"080000_300","source":"owner","sentence_id":1}).to_string()]).unwrap();
            solstone_core_journal_io::atomic_replace(
                journal.join(relative),
                &bytes,
                AtomicWriteOptions::default(),
            )
            .unwrap();
        } else if relative.ends_with(".jsonl") {
            write_jsonl(
                journal.join(relative),
                vec![later],
                AtomicWriteOptions::default(),
            )
            .unwrap();
        } else {
            write_json(journal.join(relative), &later, JsonWriteOptions::default()).unwrap();
        }
        let before_undo = journal_tree(&journal);
        let error = undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap_err();
        assert!(
            error.to_string().contains("target artifact changed"),
            "{kind}: {error}"
        );
        assert_eq!(journal_tree(&journal), before_undo, "{kind}");
        assert!(!journal.join("entities/source").exists());
        fs::remove_dir_all(journal).unwrap();
    }
}

#[test]
fn undo_refuses_missing_positional_or_voiceprint_proof_before_any_source_restoration() {
    for relative in [
        "chronicle/20260102/080000_300/talents/speaker_labels.json",
        "entities/target/voiceprints.npz",
    ] {
        let journal = undo_journal();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        }
        if relative.starts_with("chronicle/") {
            write_json(
                journal.join(relative),
                &json!({"labels":[{"speaker":"source"}]}),
                JsonWriteOptions::default(),
            )
            .unwrap();
        }
        let merged =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap();
        if relative.ends_with("voiceprints.npz") {
            let bytes = write_voiceprints_npz(
                &vec![1.0; 256],
                &[json!({"day":"20260102","segment_key":"080000_300","source":"owner","sentence_id":1}).to_string()],
            ).unwrap();
            solstone_core_journal_io::atomic_replace(
                journal.join(relative),
                &bytes,
                AtomicWriteOptions::default(),
            )
            .unwrap();
        }
        let mut payload = super::store::merge_payload::load_entity_merge_payload(
            &journal,
            "target",
            &merged.merge_id,
        )
        .unwrap();
        payload["manifest"]["undo_expected"]
            .as_object_mut()
            .unwrap()
            .remove(relative);
        super::store::merge_payload::record_entity_merge_payload(
            &journal,
            "target",
            &merged.merge_id,
            &payload,
        )
        .unwrap();
        let before_undo = journal_tree(&journal);
        let error = undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("recorded target state is missing"),
            "{error}"
        );
        assert_eq!(journal_tree(&journal), before_undo);
        fs::remove_dir_all(journal).unwrap();
    }
}

#[test]
fn older_payload_allows_voiceprint_restoration_only_when_it_is_a_noop() {
    for existing in [false, true] {
        let journal = undo_journal();
        for id in ["source", "target"] {
            save_entity_identity(&journal, id, &json!({"id":id,"name":id}), None).unwrap();
        }
        if existing {
            let bytes = write_voiceprints_npz(
                &vec![1.0; 256],
                &[json!({"day":"20260102","segment_key":"080000_300","source":"owner","sentence_id":1}).to_string()],
            ).unwrap();
            solstone_core_journal_io::atomic_replace(
                journal.join("entities/target/voiceprints.npz"),
                &bytes,
                AtomicWriteOptions::default(),
            )
            .unwrap();
        }
        let merged =
            commit_entity_merge(&journal, "source", "target", EntityMergeOptions::default())
                .unwrap();
        let mut payload = super::store::merge_payload::load_entity_merge_payload(
            &journal,
            "target",
            &merged.merge_id,
        )
        .unwrap();
        payload["manifest"]
            .as_object_mut()
            .unwrap()
            .remove("undo_expected");
        super::store::merge_payload::record_entity_merge_payload(
            &journal,
            "target",
            &merged.merge_id,
            &payload,
        )
        .unwrap();
        undo_entity_merge(&journal, &merged.merge_id, Value::Null).unwrap();
        assert!(journal.join("entities/source/entity.json").is_file());
        assert_eq!(
            journal.join("entities/target/voiceprints.npz").is_file(),
            existing
        );
        fs::remove_dir_all(journal).unwrap();
    }
}
