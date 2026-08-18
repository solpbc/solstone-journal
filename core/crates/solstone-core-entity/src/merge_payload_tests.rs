// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use super::store::merge_payload::{
    load_entity_merge_payload, move_entity_merge_payload, record_entity_merge_payload,
    validate_merge_payload,
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-merge-payload-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn payload() -> Value {
    json!({"source_id":"source","target_id":"target","source_state":{"snapshots":[]},"manifest":{"identity":{"aka_support":[],"email_support":[],"scalar_support":[],"target_before":{}},"voiceprints":{"support":[]},"facets":{"entries":[]},"segments":{"entries":[]},"activities":{"entries":[]},"observation_relations":{"entries":[]},"rebased_merge_ids":[]}})
}

fn message(value: Value) -> String {
    validate_merge_payload(Path::new("/tmp/journal"), &value)
        .unwrap_err()
        .to_string()
}

#[test]
fn validator_reports_source_identity_message() {
    let mut value = payload();
    value.as_object_mut().unwrap().remove("source_id");
    assert_eq!(message(value), "merge payload missing source entity id");
}

#[test]
fn validator_reports_target_identity_message() {
    let mut value = payload();
    value.as_object_mut().unwrap().remove("target_id");
    assert_eq!(message(value), "merge payload missing target entity id");
}

#[test]
fn validator_reports_source_state_messages() {
    let mut absent = payload();
    absent.as_object_mut().unwrap().remove("source_state");
    assert_eq!(message(absent), "merge payload missing source_state");
    let mut non_object = payload();
    non_object["source_state"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload source_state is not an object"
    );
    let mut missing = payload();
    missing["source_state"]
        .as_object_mut()
        .unwrap()
        .remove("snapshots");
    assert_eq!(message(missing), "merge payload missing snapshots");
    let mut non_list = payload();
    non_list["source_state"]["snapshots"] = json!(false);
    assert_eq!(message(non_list), "merge payload snapshots is not a list");
}

#[test]
fn well_formed_payload_validates() {
    validate_merge_payload(Path::new("/tmp/journal"), &payload()).unwrap();
}

#[test]
fn validator_reports_non_object_payload() {
    assert_eq!(message(json!(false)), "merge payload is not an object");
}

#[test]
fn validator_reports_snapshot_messages() {
    let mut non_object = payload();
    non_object["source_state"]["snapshots"] = json!([false]);
    assert_eq!(
        message(non_object),
        "merge payload snapshot is not an object"
    );
    let mut missing_rel = payload();
    missing_rel["source_state"]["snapshots"] = json!([{}]);
    assert_eq!(
        message(missing_rel),
        "manifest snapshot missing relative path"
    );
    let mut files_not_list = payload();
    files_not_list["source_state"]["snapshots"] = json!([{"rel":"entities/source","files":false}]);
    assert_eq!(
        message(files_not_list),
        "manifest snapshot files is not a list"
    );
    let mut file_not_object = payload();
    file_not_object["source_state"]["snapshots"] =
        json!([{"rel":"entities/source","files":[false]}]);
    assert_eq!(
        message(file_not_object),
        "manifest snapshot file is not an object"
    );
    let mut missing_file_rel = payload();
    missing_file_rel["source_state"]["snapshots"] = json!([{"rel":"entities/source","files":[{}]}]);
    assert_eq!(
        message(missing_file_rel),
        "manifest snapshot file missing relative path"
    );
}

#[test]
fn validator_reports_manifest_messages() {
    let mut absent = payload();
    absent.as_object_mut().unwrap().remove("manifest");
    assert_eq!(message(absent), "merge payload missing manifest");
    let mut non_object = payload();
    non_object["manifest"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload manifest is not an object"
    );
    let mut missing = payload();
    missing["manifest"]
        .as_object_mut()
        .unwrap()
        .remove("identity");
    assert_eq!(message(missing), "merge payload missing identity manifest");
    let mut non_object = payload();
    non_object["manifest"]["identity"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload identity manifest is not an object"
    );
}

#[test]
fn validator_reports_identity_support_messages() {
    for field in ["aka_support", "email_support", "scalar_support"] {
        let mut absent = payload();
        absent["manifest"]["identity"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_eq!(
            message(absent),
            format!("merge payload identity missing {field}")
        );
        let mut non_list = payload();
        non_list["manifest"]["identity"][field] = json!(false);
        assert_eq!(
            message(non_list),
            format!("merge payload identity {field} is not a list")
        );
    }
}

#[test]
fn validator_reports_identity_target_before_messages() {
    let mut absent = payload();
    absent["manifest"]["identity"]
        .as_object_mut()
        .unwrap()
        .remove("target_before");
    assert_eq!(
        message(absent),
        "merge payload identity missing target_before"
    );
    let mut non_object = payload();
    non_object["manifest"]["identity"]["target_before"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload identity target_before is not an object"
    );
}

#[test]
fn validator_reports_voiceprint_messages() {
    let mut absent = payload();
    absent["manifest"]
        .as_object_mut()
        .unwrap()
        .remove("voiceprints");
    assert_eq!(
        message(absent),
        "merge payload missing voiceprints manifest"
    );
    let mut non_object = payload();
    non_object["manifest"]["voiceprints"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload voiceprints manifest is not an object"
    );
    let mut support = payload();
    support["manifest"]["voiceprints"]
        .as_object_mut()
        .unwrap()
        .remove("support");
    assert_eq!(
        message(support),
        "merge payload voiceprints missing support"
    );
    let mut non_list = payload();
    non_list["manifest"]["voiceprints"]["support"] = json!(false);
    assert_eq!(
        message(non_list),
        "merge payload voiceprints support is not a list"
    );
}

#[test]
fn validator_reports_facet_messages() {
    let mut absent = payload();
    absent["manifest"].as_object_mut().unwrap().remove("facets");
    assert_eq!(message(absent), "merge payload missing facets manifest");
    let mut non_object = payload();
    non_object["manifest"]["facets"] = json!(false);
    assert_eq!(
        message(non_object),
        "merge payload facets manifest is not an object"
    );
    let mut missing_entries = payload();
    missing_entries["manifest"]["facets"]
        .as_object_mut()
        .unwrap()
        .remove("entries");
    assert_eq!(
        message(missing_entries),
        "merge payload facets missing entries"
    );
    let mut non_list = payload();
    non_list["manifest"]["facets"]["entries"] = json!(false);
    assert_eq!(
        message(non_list),
        "merge payload facets entries is not a list"
    );
    let mut non_object_entry = payload();
    non_object_entry["manifest"]["facets"]["entries"] = json!([false]);
    assert_eq!(
        message(non_object_entry),
        "merge payload facet entry is not an object"
    );
    let mut missing_name = payload();
    missing_name["manifest"]["facets"]["entries"] = json!([{}]);
    assert_eq!(
        message(missing_name),
        "manifest facet entry missing facet name"
    );
    let mut empty_name = payload();
    empty_name["manifest"]["facets"]["entries"] = json!([{"facet":""}]);
    assert_eq!(
        message(empty_name),
        "manifest facet entry missing facet name"
    );
}

#[test]
fn validator_reports_path_section_messages() {
    for section in ["segments", "activities", "observation_relations"] {
        let mut absent = payload();
        absent["manifest"].as_object_mut().unwrap().remove(section);
        assert_eq!(
            message(absent),
            format!("merge payload missing {section} manifest")
        );
        let mut non_object = payload();
        non_object["manifest"][section] = json!(false);
        assert_eq!(
            message(non_object),
            format!("merge payload {section} manifest is not an object")
        );
        let mut missing_entries = payload();
        missing_entries["manifest"][section]
            .as_object_mut()
            .unwrap()
            .remove("entries");
        assert_eq!(
            message(missing_entries),
            format!("merge payload {section} missing entries")
        );
        let mut non_list = payload();
        non_list["manifest"][section]["entries"] = json!(false);
        assert_eq!(
            message(non_list),
            format!("merge payload {section} entries is not a list")
        );
        let mut non_object_entry = payload();
        non_object_entry["manifest"][section]["entries"] = json!([false]);
        assert_eq!(
            message(non_object_entry),
            format!("merge payload {section} entry is not an object")
        );
        let mut missing_path = payload();
        missing_path["manifest"][section]["entries"] = json!([{}]);
        assert_eq!(
            message(missing_path),
            format!("manifest {section} entry missing path")
        );
    }
}

#[test]
fn validator_reports_contained_path_messages() {
    let expected = "journal path contains invalid component";
    let mut source = payload();
    source["source_id"] = json!("../outside");
    assert_eq!(message(source), expected);
    let mut target = payload();
    target["target_id"] = json!("../outside");
    assert_eq!(message(target), expected);
    let mut snapshot = payload();
    snapshot["source_state"]["snapshots"] = json!([{"rel":"../outside","files":[]}]);
    assert_eq!(message(snapshot), expected);
    let mut snapshot_file = payload();
    snapshot_file["source_state"]["snapshots"] =
        json!([{"rel":"entities/source","files":[{"rel":"../../outside"}]}]);
    assert_eq!(message(snapshot_file), expected);
    let mut facet = payload();
    facet["manifest"]["facets"]["entries"] = json!([{"facet":"../outside"}]);
    assert_eq!(message(facet), expected);
    for section in ["segments", "activities", "observation_relations"] {
        let mut value = payload();
        value["manifest"][section]["entries"] = json!([{"path":"../outside"}]);
        assert_eq!(message(value), expected);
    }
}

#[test]
fn validator_reports_rebased_messages_from_manifest() {
    let mut absent = payload();
    absent["manifest"]
        .as_object_mut()
        .unwrap()
        .remove("rebased_merge_ids");
    assert_eq!(message(absent), "merge payload missing rebased_merge_ids");
    let mut non_list = payload();
    non_list["manifest"]["rebased_merge_ids"] = json!(false);
    assert_eq!(
        message(non_list),
        "merge payload rebased_merge_ids is not a list"
    );
}

#[test]
fn load_revalidates_tampered_payload() {
    let directory = TempDir::new();
    let value = payload();
    record_entity_merge_payload(&directory.0, "source", "em_1", &value).unwrap();
    let path = directory
        .0
        .join("entities/source/history/private/em_1.json");
    let mut tampered: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    tampered["manifest"]
        .as_object_mut()
        .unwrap()
        .remove("rebased_merge_ids");
    fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
    assert_eq!(
        load_entity_merge_payload(&directory.0, "source", "em_1")
            .unwrap_err()
            .to_string(),
        "invalid private merge payload for source:em_1: merge payload missing rebased_merge_ids"
    );
}

#[test]
fn load_distinguishes_missing_and_non_object_payloads() {
    let directory = TempDir::new();
    assert_eq!(
        load_entity_merge_payload(&directory.0, "source", "missing")
            .unwrap_err()
            .to_string(),
        "missing private merge payload for source: missing"
    );
    let path = directory
        .0
        .join("entities/source/history/private/non-object.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"[]").unwrap();
    assert_eq!(
        load_entity_merge_payload(&directory.0, "source", "non-object")
            .unwrap_err()
            .to_string(),
        format!("private merge payload is not an object: {}", path.display())
    );
}

#[test]
fn move_rebases_and_removes_only_a_distinct_source() {
    let directory = TempDir::new();
    let value = payload();
    record_entity_merge_payload(&directory.0, "source", "em_1", &value).unwrap();
    let (moved, rel) = move_entity_merge_payload(
        &directory.0,
        "source",
        "target",
        "target",
        "em_1",
        Some("ancestor"),
    )
    .unwrap();
    assert_eq!(rel, "entities/target/history/private/em_1.json");
    assert_eq!(moved["target_id"], "target");
    assert_eq!(moved["rebased_from_entity_id"], "ancestor");
    assert!(
        !directory
            .0
            .join("entities/source/history/private/em_1.json")
            .exists()
    );
    assert!(
        directory
            .0
            .join("entities/target/history/private/em_1.json")
            .is_file()
    );
    let (_, _) =
        move_entity_merge_payload(&directory.0, "target", "target", "target", "em_1", None)
            .unwrap();
    let same = load_entity_merge_payload(&directory.0, "target", "em_1").unwrap();
    assert!(same.get("rebased_from_entity_id").is_some());
    record_entity_merge_payload(&directory.0, "target", "em_2", &payload()).unwrap();
    let (same, _) =
        move_entity_merge_payload(&directory.0, "target", "target", "target", "em_2", None)
            .unwrap();
    assert!(same.get("rebased_from_entity_id").is_none());
    assert!(
        directory
            .0
            .join("entities/target/history/private/em_2.json")
            .is_file()
    );
}
