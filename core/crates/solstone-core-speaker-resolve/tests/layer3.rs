// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::{
    EncoderIdentity, JournalEntity, VoiceprintItem, load_all_journal_entities,
    save_voiceprints_batch,
};
use solstone_core_speaker_resolve::layer1::Label;
use solstone_core_speaker_resolve::layer3::{Layer3Inputs, apply_acoustic_matching};
use solstone_core_speaker_resolve::voiceprint_centroid::VoiceprintCentroidCache;

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-layer3-{}-{sequence}",
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

fn embedding(x: f32, y: f32) -> Vec<f32> {
    let mut embedding = vec![0.0; 256];
    embedding[0] = x;
    embedding[1] = y;
    embedding
}

fn write_entity(root: &Path, id: &str, entity_type: Option<&str>, principal: bool) {
    let path = root.join("entities").join(id).join("entity.json");
    fs::create_dir_all(path.parent().expect("entity parent")).expect("create entity parent");
    let mut value = json!({"id": id, "name": id});
    let object = value.as_object_mut().expect("entity object");
    if let Some(entity_type) = entity_type {
        object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
    }
    if principal {
        object.insert("is_principal".to_owned(), Value::Bool(true));
    }
    fs::write(path, value.to_string()).expect("write entity");
}

fn write_voiceprint(root: &Path, id: &str, embedding: Vec<f32>) {
    let encoder = EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    save_voiceprints_batch(
        root,
        id,
        &[VoiceprintItem {
            embedding,
            metadata: json!({"stream": "mic", "added_at": 0}),
        }],
        &encoder,
    )
    .expect("write voiceprint");
}

fn entities(root: &Path) -> Vec<JournalEntity> {
    load_all_journal_entities(root).expect("load entities")
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

fn run(
    temporary: &TempDir,
    labels: BTreeMap<i64, Label>,
    non_owner_sids: &[i64],
    candidate_entity_ids: &BTreeSet<String>,
    entities: &[JournalEntity],
    statements: &[(i64, Vec<f32>)],
    integer_speakers: &HashMap<i64, i64>,
) -> solstone_core_speaker_resolve::layer3::Layer3Result {
    apply_acoustic_matching(
        Layer3Inputs {
            labels,
            non_owner_sids,
            margin_declined_sids: &HashSet::new(),
            candidate_entity_ids,
            entities,
            journal_root: temporary.path(),
            stream: "mic",
            now_ms: 1,
            statements,
            integer_speakers,
        },
        &mut VoiceprintCentroidCache::default(),
        &mut Vec::new(),
    )
}

#[test]
fn ac3_empty_candidate_fallback_only_loads_person_voiceprints() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "person", Some("Person"), false);
    write_entity(temporary.path(), "tool", Some("Tool"), false);
    write_voiceprint(temporary.path(), "person", embedding(1.0, 0.0));
    write_voiceprint(temporary.path(), "tool", embedding(0.0, 1.0));
    let loaded_entities = entities(temporary.path());
    let result = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::new(),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("person"));
    assert!(result.voiceprint_versions.contains_key("person"));
    assert!(!result.voiceprint_versions.contains_key("tool"));
    assert_ne!(result.labels[&1].speaker.as_deref(), Some("tool"));
}

#[test]
fn ac13_acoustic_tiers_and_margin_follow_calibration() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alpha", Some("Person"), false);
    write_entity(temporary.path(), "beta", Some("Person"), false);
    write_entity(temporary.path(), "gamma", Some("Person"), false);
    write_voiceprint(temporary.path(), "alpha", embedding(1.0, 0.0));
    write_voiceprint(
        temporary.path(),
        "beta",
        embedding(0.99, (1.0 - 0.99_f32.powi(2)).sqrt()),
    );
    write_voiceprint(
        temporary.path(),
        "gamma",
        embedding(0.3, (1.0 - 0.3_f32.powi(2)).sqrt()),
    );
    let loaded_entities = entities(temporary.path());

    let clear = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["alpha".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(clear.labels[&1].confidence.as_deref(), Some("high"));

    let thin = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["alpha".to_owned(), "beta".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(thin.labels[&1].confidence.as_deref(), Some("medium"));
    assert_eq!(thin.labels[&1].acoustic_margin_declined, Some(true));

    let medium = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["gamma".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(medium.labels[&1].confidence.as_deref(), Some("medium"));
    assert_eq!(medium.labels[&1].acoustic_margin_declined, None);
}

#[test]
fn ac14_cluster_coverage_and_confidence_gates_defer_to_sentence_matching() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alpha", Some("Person"), false);
    write_entity(temporary.path(), "beta", Some("Person"), false);
    write_voiceprint(temporary.path(), "alpha", embedding(1.0, 0.0));
    write_voiceprint(
        temporary.path(),
        "beta",
        embedding(0.25, (1.0 - 0.25_f32.powi(2)).sqrt()),
    );
    let loaded_entities = entities(temporary.path());

    let coverage = run(
        &temporary,
        labels(&[1, 2, 3]),
        &[1, 2, 3],
        &BTreeSet::from(["alpha".to_owned()]),
        &loaded_entities,
        &[
            (1, embedding(1.0, 0.0)),
            (2, embedding(1.0, 0.0)),
            (3, embedding(1.0, 0.0)),
        ],
        &HashMap::from([(1, 7)]),
    );
    assert_eq!(coverage.labels[&1].method.as_deref(), Some("acoustic"));

    let confidence = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["beta".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::from([(1, 7)]),
    );
    assert_eq!(confidence.labels[&1].method.as_deref(), Some("acoustic"));
    assert_eq!(confidence.labels[&1].confidence.as_deref(), Some("medium"));
}

#[test]
fn ac14_cluster_assignment_is_one_cluster_one_entity_and_drops_low_pair() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alpha", Some("Person"), false);
    write_entity(temporary.path(), "beta", Some("Person"), false);
    write_voiceprint(temporary.path(), "alpha", embedding(1.0, 0.0));
    write_voiceprint(temporary.path(), "beta", embedding(0.0, 1.0));
    let loaded_entities = entities(temporary.path());
    let result = run(
        &temporary,
        labels(&[1, 2]),
        &[1, 2],
        &BTreeSet::from(["alpha".to_owned(), "beta".to_owned()]),
        &loaded_entities,
        &[
            (1, embedding(1.0, 0.0)),
            (2, embedding((1.0 - 0.1_f32.powi(2)).sqrt(), 0.1)),
        ],
        &HashMap::from([(1, 1), (2, 2)]),
    );
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("alpha"));
    assert_eq!(
        result.labels[&1].method.as_deref(),
        Some("acoustic_cluster")
    );
    assert_eq!(result.labels[&2].method.as_deref(), Some("acoustic"));
}

#[test]
fn ac15_negative_and_zero_scores_never_become_sentence_matches() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alpha", Some("Person"), false);
    write_entity(temporary.path(), "beta", Some("Person"), false);
    write_voiceprint(temporary.path(), "alpha", embedding(-1.0, 0.0));
    write_voiceprint(temporary.path(), "beta", embedding(0.0, 1.0));
    let loaded_entities = entities(temporary.path());
    let result = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["alpha".to_owned(), "beta".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(result.labels[&1].speaker, None);
}

#[test]
fn ac16_exact_acoustic_ties_choose_the_lowest_entity_id() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "alpha", Some("Person"), false);
    write_entity(temporary.path(), "zeta", Some("Person"), false);
    write_voiceprint(temporary.path(), "alpha", embedding(1.0, 0.0));
    write_voiceprint(temporary.path(), "zeta", embedding(1.0, 0.0));
    let loaded_entities = entities(temporary.path());
    let result = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::from(["alpha".to_owned(), "zeta".to_owned()]),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("alpha"));
}

#[test]
fn voiceprint_versions_include_unusable_centroids() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "zero", Some("Person"), false);
    write_voiceprint(temporary.path(), "zero", vec![0.0; 256]);
    let loaded_entities = entities(temporary.path());
    let result = run(
        &temporary,
        labels(&[1]),
        &[1],
        &BTreeSet::new(),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert_eq!(result.voiceprint_versions.get("zero"), Some(&1));
    assert_eq!(result.labels[&1].speaker, None);
}

#[test]
fn empty_unresolved_short_circuits_before_voiceprint_loading() {
    let temporary = TempDir::new();
    write_entity(temporary.path(), "person", Some("Person"), false);
    write_voiceprint(temporary.path(), "person", embedding(1.0, 0.0));
    let loaded_entities = entities(temporary.path());
    let mut resolved_labels = labels(&[1]);
    resolved_labels.get_mut(&1).expect("label").speaker = Some("already".to_owned());
    let result = run(
        &temporary,
        resolved_labels,
        &[1],
        &BTreeSet::new(),
        &loaded_entities,
        &[(1, embedding(1.0, 0.0))],
        &HashMap::new(),
    );
    assert!(result.voiceprint_versions.is_empty());
}
