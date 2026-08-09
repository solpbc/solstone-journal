// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_speaker_resolve::identify_target::{
    IdentifyTargetOutcome, IdentifyTargetRequest, resolve_identify_target,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-identify-target-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entity(root: &Path, id: &str, name: &str, entity_type: &str) {
    let path = root.join("entities").join(id);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("entity.json"),
        json!({"id": id, "name": name, "type": entity_type}).to_string(),
    )
    .unwrap();
}

fn request(root: &Path) -> IdentifyTargetRequest {
    IdentifyTargetRequest {
        journal_root: root.to_path_buf(),
        cluster_id: 42,
        name: None,
        entity_id: None,
        resolve_only: false,
        create_new: false,
        entity_type: "Person".to_owned(),
        reviewed_near_match_entity_ids: Vec::new(),
    }
}

#[test]
fn ac3_entity_id_refuses_non_person_and_resolves_person_without_writing() {
    let temporary = Temp::new();
    entity(temporary.path(), "tool", "Audio Tool", "Tool");
    entity(temporary.path(), "person", "Ava Person", "Person");
    let tool_voiceprints = temporary.path().join("entities/tool/voiceprints.npz");
    assert!(!tool_voiceprints.exists());

    let mut tool_request = request(temporary.path());
    tool_request.entity_id = Some("tool".to_owned());
    assert!(matches!(
        resolve_identify_target(&tool_request).unwrap(),
        IdentifyTargetOutcome::NonPersonEntity { entity_id, entity_type }
            if entity_id == "tool" && entity_type.as_deref() == Some("Tool")
    ));
    assert!(!tool_voiceprints.exists());

    let mut person_request = request(temporary.path());
    person_request.entity_id = Some("person".to_owned());
    assert!(matches!(
        resolve_identify_target(&person_request).unwrap(),
        IdentifyTargetOutcome::Ready(target)
            if target.entity_id == "person" && !target.will_create
    ));
}

#[test]
fn ac3_entity_id_not_found_is_distinct_from_person_refusal() {
    let temporary = Temp::new();
    let mut target_request = request(temporary.path());
    target_request.entity_id = Some("missing".to_owned());
    assert!(matches!(
        resolve_identify_target(&target_request).unwrap(),
        IdentifyTargetOutcome::EntityNotFound { entity_id } if entity_id == "missing"
    ));
}

#[test]
fn ac3_create_new_refuses_non_person_type_before_entity_creation() {
    let temporary = Temp::new();
    let mut tool_request = request(temporary.path());
    tool_request.name = Some("New Tool".to_owned());
    tool_request.create_new = true;
    tool_request.entity_type = "Tool".to_owned();
    assert!(matches!(
        resolve_identify_target(&tool_request).unwrap(),
        IdentifyTargetOutcome::NonPersonCreateType { entity_type } if entity_type == "Tool"
    ));
    assert!(!temporary.path().join("entities/new_tool").exists());

    let mut person_request = tool_request;
    person_request.entity_type = "Person".to_owned();
    assert!(matches!(
        resolve_identify_target(&person_request).unwrap(),
        IdentifyTargetOutcome::Ready(target)
            if target.entity_id == "new_tool" && target.will_create
    ));
}

#[test]
fn ac3_name_resolution_excludes_non_person_and_resolves_person() {
    let temporary = Temp::new();
    entity(temporary.path(), "tool", "Only Tool", "Tool");
    entity(temporary.path(), "person", "Ava Person", "Person");
    let tool_voiceprints = temporary.path().join("entities/tool/voiceprints.npz");

    let mut tool_request = request(temporary.path());
    tool_request.name = Some("Only Tool".to_owned());
    assert!(matches!(
        resolve_identify_target(&tool_request).unwrap(),
        IdentifyTargetOutcome::NoMatch { candidates }
            if candidates.iter().map(|candidate| candidate.id.as_str()).collect::<Vec<_>>() == vec!["person"]
    ));
    assert!(!tool_voiceprints.exists());

    let mut person_request = request(temporary.path());
    person_request.name = Some("Ava Person".to_owned());
    assert!(matches!(
        resolve_identify_target(&person_request).unwrap(),
        IdentifyTargetOutcome::Ready(target)
            if target.entity_id == "person" && target.entity_name == "Ava Person"
    ));
}

#[test]
fn ac20_ambiguous_name_returns_ambiguity_id_and_visible_candidates() {
    let temporary = Temp::new();
    entity(temporary.path(), "alex-one", "Alex One", "Person");
    entity(temporary.path(), "alex-two", "Alex Two", "Person");
    let mut target_request = request(temporary.path());
    target_request.name = Some("Alex".to_owned());

    let IdentifyTargetOutcome::Ambiguous {
        ambiguity_id: _,
        candidates,
    } = resolve_identify_target(&target_request).unwrap()
    else {
        panic!("Alex should retain the two low-confidence Person candidates");
    };
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<Vec<_>>(),
        vec!["alex-one", "alex-two"]
    );
}
