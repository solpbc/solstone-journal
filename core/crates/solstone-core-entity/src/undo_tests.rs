// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::store::undo_entity_merge_with_injector;
use crate::{
    EntityMergeOptions, commit_entity_merge, read_entity_identity, save_entity_identity,
    undo_entity_merge,
};

static NEXT_UNDO_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn undo_journal() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "solstone-undo-{}-{}",
        std::process::id(),
        NEXT_UNDO_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
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
    assert!(target_facet.exists());
    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert!(!target_facet.exists());
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
    let moved_target = journal.join("facets/moved/entities/target/entity.json");
    let moved_after_merge = fs::read(&moved_target).unwrap();
    let merged_after_merge = fs::read(merged_target.join("entity.json")).unwrap();
    assert!(
        undo_entity_merge_with_injector(
            &journal,
            &merge.merge_id,
            Value::Null,
            Some(&|phase| phase == "facets"),
        )
        .is_err()
    );
    assert_eq!(fs::read(&moved_target).unwrap(), moved_after_merge);
    assert_eq!(
        fs::read(merged_target.join("entity.json")).unwrap(),
        merged_after_merge
    );

    undo_entity_merge(&journal, &merge.merge_id, Value::Null).unwrap();
    assert!(!moved_target.exists());
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
            Some(&|phase| phase == "observations"),
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
            Some(&|phase| phase == "segments"),
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
            Some(&|phase| phase == "activities"),
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
