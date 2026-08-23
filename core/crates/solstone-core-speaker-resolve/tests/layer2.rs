// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::{
    EntityResolutionError, JournalEntity, ambiguity_id, load_all_journal_entities,
};
use solstone_core_speaker_resolve::layer1::Label;
use solstone_core_speaker_resolve::layer2::{
    Layer2Inputs, Layer2Result, apply_structural_heuristics,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-layer2-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temporary journal");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_entity(root: &Path, id: &str, name: &str, entity_type: Option<&str>, principal: bool) {
    let path = root.join("entities").join(id).join("entity.json");
    fs::create_dir_all(path.parent().expect("entity parent")).expect("create entity parent");
    let mut value = json!({"id": id, "name": name});
    let object = value.as_object_mut().expect("entity object");
    if let Some(entity_type) = entity_type {
        object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
    }
    if principal {
        object.insert("is_principal".to_owned(), Value::Bool(true));
    }
    fs::write(path, value.to_string()).expect("write entity");
}

fn journal_entities(root: &Path) -> Vec<JournalEntity> {
    load_all_journal_entities(root).expect("load journal entities")
}

fn write_resolved_choice(root: &Path, query: &str, entity_id: &str) {
    let normalized = solstone_core_entity_matching::normalize_resolution_query(query);
    let row = json!({
        "schema_version": 1,
        "ambiguity_id": ambiguity_id(&format!("journal|{normalized}")),
        "scope": {"kind": "journal"},
        "normalized_query": normalized,
        "original_query": query,
        "latest_query": query,
        "first_seen": "2026-08-01T00:00:00Z",
        "last_seen": "2026-08-01T00:00:00Z",
        "observed_tier": 8,
        "status": "resolved",
        "resolved_entity_id": entity_id,
        "resolved_at": "2026-08-01T00:00:00Z",
        "ranked_candidates": [{
            "id": entity_id,
            "name": query,
            "tier": 8,
            "score": 90.0
        }],
        "origins": [{"lane": "test"}],
        "origin_keys": ["test"],
        "occurrence_count": 1,
        "audit": {"prior_choices": []}
    });
    let path = root.join("entities/ambiguities.jsonl");
    fs::create_dir_all(path.parent().expect("ambiguities parent")).expect("create entities");
    fs::write(&path, format!("{row}\n")).expect("write resolved choice");
}

fn labels(sentence_ids: &[i64]) -> BTreeMap<i64, Label> {
    sentence_ids
        .iter()
        .map(|sentence_id| {
            (
                *sentence_id,
                Label {
                    sentence_id: *sentence_id,
                    speaker: None,
                    confidence: None,
                    method: None,
                    owner_margin_declined: None,
                    acoustic_margin_declined: None,
                },
            )
        })
        .collect()
}

fn apply(
    temporary: &TempDir,
    speakers: &[String],
    setting_names: &[String],
    entities: &[JournalEntity],
    margin_declined_sids: &HashSet<i64>,
) -> Result<Layer2Result, EntityResolutionError> {
    apply_structural_heuristics(
        labels(&[1]),
        Layer2Inputs {
            speakers,
            setting_names,
            screen_names: &[],
            meeting_names: &[],
            entities,
            non_owner_sids: &[1],
            margin_declined_sids,
            journal_root: temporary.path(),
            day: "20260808",
            segment_key: "120000_300",
            read_only: true,
        },
    )
}

#[test]
fn ac1_setting_person_is_labeled_by_structural_setting() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alice", "Alice", Some("Person"), false);
    let settings = ["Alice".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("alice"));
    assert_eq!(
        result.labels[&1].method.as_deref(),
        Some("structural_setting")
    );
}

#[test]
fn ac2_setting_non_person_is_not_labeled_or_admitted() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "tool", "Terminal", Some("Tool"), false);
    let settings = ["Terminal".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker, None);
    assert!(!result.candidate_entity_ids.contains("tool"));
}

#[test]
fn ac5_principal_person_remains_admissible() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "owner", "Avery", Some("Person"), true);
    let settings = ["Avery".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("owner"));
    assert!(result.candidate_entity_ids.contains("owner"));
}

#[test]
fn ac6_setting_person_has_label_and_candidate_evidence() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alice", "Alice", Some("Person"), false);
    let settings = ["Alice".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("alice"));
    assert_eq!(
        result.candidate_evidence,
        [solstone_core_speaker_resolve::evidence::CandidateEvidence {
            entity_id: "alice".to_owned(),
            sources: vec!["setting".to_owned()],
        }]
    );
}

#[test]
fn structural_single_speaker_does_not_label_a_non_person() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "tool", "Terminal", Some("Tool"), false);
    let speakers = ["Terminal".to_owned()];
    let result = apply(
        &temporary,
        &speakers,
        &[],
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker, None);
    assert!(result.candidate_entity_ids.is_empty());
}

#[test]
fn structural_single_speaker_does_not_adopt_a_written_id_slug_collision() {
    let temporary = TempDir::new();
    write_entity(
        temporary.path(),
        "new_person",
        "Someone Else",
        Some("Person"),
        false,
    );
    let speakers = ["New Person".to_owned()];
    let result = apply(
        &temporary,
        &speakers,
        &[],
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker, None);
    assert!(!result.candidate_entity_ids.contains("new_person"));
}

#[test]
fn structural_single_speaker_and_setting_branches_are_mutually_exclusive() {
    let temporary = TempDir::new();
    write_entity(
        temporary.path(),
        "speaker",
        "Speaker",
        Some("Person"),
        false,
    );
    write_entity(
        temporary.path(),
        "setting",
        "Setting",
        Some("Person"),
        false,
    );
    let speakers = ["Speaker".to_owned()];
    let settings = ["Setting".to_owned()];
    let result = apply(
        &temporary,
        &speakers,
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("speaker"));
    assert_eq!(
        result.labels[&1].method.as_deref(),
        Some("structural_single_speaker")
    );
}

#[test]
fn write_path_resolution_error_propagates_without_a_gap() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alice", "Alice", Some("Person"), false);
    let ambiguities = temporary.path().join("entities/ambiguities.jsonl");
    fs::write(ambiguities, b"not json\n").expect("write corrupt ambiguities");
    let settings = ["Alice".to_owned()];
    let error = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect_err("corrupt ambiguity ledger must propagate");
    assert!(matches!(error, EntityResolutionError::Read(_)));
}

#[test]
fn ac11_margin_declined_structural_relabel_is_demoted_and_preserved() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alice", "Alice", Some("Person"), false);
    let settings = ["Alice".to_owned()];
    let margin_declined_sids = HashSet::from([1]);
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &margin_declined_sids,
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].confidence.as_deref(), Some("medium"));
    assert_eq!(result.labels[&1].owner_margin_declined, Some(true));
}

#[test]
fn same_name_person_and_tool_admits_the_person_as_candidate() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alex", "Alex", Some("Person"), false);
    write_entity(temporary.path(), "tool", "Alex", Some("Tool"), false);
    let speakers = ["Alex".to_owned(), "Other".to_owned()];
    let result = apply(
        &temporary,
        &speakers,
        &[],
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(
        result.candidate_entity_ids.iter().collect::<Vec<_>>(),
        [&"alex".to_owned()]
    );
    assert_eq!(result.resolved_candidate_names, ["Alex"]);
    assert_eq!(result.labels[&1].speaker, None);
}

#[test]
fn same_name_person_and_tool_labels_via_structural_setting() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alex", "Alex", Some("Person"), false);
    write_entity(temporary.path(), "tool", "Alex", Some("Tool"), false);
    let settings = ["Alex".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("alex"));
    assert_eq!(
        result.labels[&1].method.as_deref(),
        Some("structural_setting")
    );
    assert!(result.candidate_entity_ids.contains("alex"));
}

#[test]
fn two_same_named_unblocked_persons_remain_ambiguous() {
    let temporary = TempDir::new();
    write_entity(
        temporary.path(),
        "sam-one",
        "Sam Person",
        Some("Person"),
        false,
    );
    write_entity(
        temporary.path(),
        "sam-two",
        "Sam Person",
        Some("Person"),
        false,
    );
    let settings = ["Sam Person".to_owned()];
    let result = apply(
        &temporary,
        &[],
        &settings,
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("apply layer 2");
    assert_eq!(result.labels[&1].speaker, None);
    assert!(result.candidate_entity_ids.is_empty());
    assert!(result.resolved_candidate_names.is_empty());
}

#[test]
fn saved_choice_naming_a_present_tool_is_unmatched_without_error() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "tool", "Terminal", Some("Tool"), false);
    write_resolved_choice(temporary.path(), "Terminal", "tool");
    let speakers = ["Terminal".to_owned()];
    let result = apply(
        &temporary,
        &speakers,
        &[],
        &journal_entities(temporary.path()),
        &HashSet::new(),
    )
    .expect("saved Tool choice must not raise");
    assert_eq!(result.labels[&1].speaker, None);
    assert!(result.candidate_entity_ids.is_empty());
    assert!(result.resolved_candidate_names.is_empty());
}
