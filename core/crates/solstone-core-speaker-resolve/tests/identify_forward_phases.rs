// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::{EncoderIdentity, VoiceprintItem};
use solstone_core_journal_io::segment_path;
use solstone_core_speaker_resolve::candidate_tracker::{CandidateTracker, ClusterInput};
use solstone_core_speaker_resolve::identify_forward_phases::RetroTrackerPhasePlan;
use solstone_core_speaker_resolve::identify_forward_phases::{
    EntityPhasePlan, ForwardPhaseError, KeepSeparatePhaseEntry, LabelPlanItem,
    SegmentCorrectionPlan, SegmentLabelPlan, SentinelPhasePlan, phase_corrections, phase_entity,
    phase_keep_separate, phase_labels, phase_retro_tracker, phase_sentinel,
};
use solstone_core_speaker_resolve::identify_operations::ForwardPhase;
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteInput, write_owner_centroid,
};
use solstone_core_speaker_resolve::voiceprint_metadata::VoiceprintMetadata;

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-identify-forward-{}-{}",
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

fn segment(root: &Path) -> PathBuf {
    let path = segment_path(root, "20260808", "120000_300", "mic", true).unwrap();
    fs::create_dir_all(path.join("talents")).unwrap();
    path
}

fn admitted_owner(root: &Path) {
    let path = root.join("entities/owner");
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("entity.json"),
        json!({"id":"owner","type":"Person","is_principal":true}).to_string(),
    )
    .unwrap();
}

fn owner_centroid(root: &Path, centroid: Vec<f32>) {
    write_owner_centroid(
        root,
        "owner",
        &OwnerCentroidWriteInput {
            centroid,
            cluster_size: 1,
            timestamp: "2026-08-08T00:00:00Z".into(),
            evidence_tier: "test".into(),
        },
    )
    .unwrap();
}

#[test]
fn phase_entity_creates_once_and_replays_its_operation_history() {
    let temporary = Temp::new();
    let plan = EntityPhasePlan {
        target_entity_id: "alice".into(),
        will_create: true,
        intended_identity: json!({"id":"alice","name":"Alice","type":"Person"}),
        operation_id: "idop_fixture".into(),
    };
    let first = phase_entity(temporary.path(), &plan).unwrap();
    assert_eq!(
        first.fields["history_event_refs"].as_array().unwrap().len(),
        1
    );
    let replay = phase_entity(temporary.path(), &plan).unwrap();
    assert_eq!(
        replay.fields["history_event_refs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let conflicting = EntityPhasePlan {
        intended_identity: json!({"id":"alice","name":"Other","type":"Person"}),
        ..plan
    };
    assert!(matches!(
        phase_entity(temporary.path(), &conflicting),
        Err(ForwardPhaseError::RepairRequired {
            phase: ForwardPhase::Entity,
            code: "concurrent_change",
            ..
        })
    ));
}

#[test]
fn phase_keep_separate_dedupes_by_operation_and_source_kind() {
    let temporary = Temp::new();
    let entry = KeepSeparatePhaseEntry {
        pair_key: "a|b".into(),
        entity_id_a: "a".into(),
        entity_id_b: "b".into(),
        source_kind: "explicit_create_near_match".into(),
        detection_count_used: 1,
    };
    let first = phase_keep_separate(
        temporary.path(),
        "idop_fixture",
        std::slice::from_ref(&entry),
    )
    .unwrap();
    assert_eq!(first.fields["recorded_count"], 1);
    let replay = phase_keep_separate(temporary.path(), "idop_fixture", &[entry]).unwrap();
    assert_eq!(replay.fields["already_present_count"], 1);
}

#[test]
fn phase_corrections_replays_identify_correction_without_duplicate_append() {
    let temporary = Temp::new();
    segment(temporary.path());
    let plan = SegmentCorrectionPlan {
        day: "20260808".into(),
        stream: "mic".into(),
        segment_key: "120000_300".into(),
        rows_to_append: vec![
            json!({"sentence_id":7,"corrected_speaker":"alice","operation_id":"idop_fixture","correction_kind":"identify"}),
        ],
    };
    assert_eq!(
        phase_corrections(
            temporary.path(),
            "idop_fixture",
            std::slice::from_ref(&plan),
        )
        .unwrap()
        .fields["appended_count"],
        1
    );
    assert_eq!(
        phase_corrections(temporary.path(), "idop_fixture", &[plan])
            .unwrap()
            .fields["appended_count"],
        1
    );
}

#[test]
fn phase_labels_patches_matching_prior_and_repairs_concurrent_change() {
    let temporary = Temp::new();
    let directory = segment(temporary.path());
    fs::write(
        directory.join("talents/speaker_labels.json"),
        json!({"labels":[{"sentence_id":7,"speaker":"old","confidence":"low","method":"cluster"}]})
            .to_string(),
    )
    .unwrap();
    let plan = SegmentLabelPlan {
        day: "20260808".into(),
        stream: "mic".into(),
        segment_key: "120000_300".into(),
        labels: vec![LabelPlanItem {
            sentence_id: 7,
            intended_label: json!({"sentence_id":7,"speaker":"alice","confidence":"high","method":"user_identified"}),
            prior_state: "present".into(),
            prior_label: Some(
                json!({"sentence_id":7,"speaker":"old","confidence":"low","method":"cluster"}),
            ),
        }],
    };
    assert_eq!(
        phase_labels(temporary.path(), std::slice::from_ref(&plan))
            .unwrap()
            .fields["patched_count"],
        1
    );
    fs::write(directory.join("talents/speaker_labels.json"), json!({"labels":[{"sentence_id":7,"speaker":"other","confidence":"high","method":"user_assigned"}]}).to_string()).unwrap();
    assert!(matches!(
        phase_labels(temporary.path(), &[plan]),
        Err(ForwardPhaseError::RepairRequired {
            phase: ForwardPhase::Labels,
            code: "concurrent_change",
            ..
        })
    ));
}

#[test]
fn phase_retro_tracker_unmatched_returns_the_durable_empty_checkpoint_shape() {
    let temporary = Temp::new();
    admitted_owner(temporary.path());
    let mut tracker = CandidateTracker::new(temporary.path());
    let result = phase_retro_tracker(
        temporary.path(),
        &mut tracker,
        &RetroTrackerPhasePlan {
            matched: false,
            candidate_id: None,
            target_entity_id: "target".into(),
            planning_owner_entity_id: Some("owner".into()),
            candidate_before: None,
            candidate_after: None,
            voiceprints_to_add: vec![],
        },
        &EncoderIdentity {
            id: "test".into(),
            sha256: "a".repeat(64),
            width: 256,
        },
    )
    .unwrap();
    assert_eq!(result.fields["matched"], false);
    assert_eq!(result.fields["saved_keys"], json!([]));
}

#[test]
fn phase_retro_tracker_confirms_a_matching_candidate_and_repairs_a_missing_one() {
    let temporary = Temp::new();
    admitted_owner(temporary.path());
    let mut owner = vec![0.0; 256];
    owner[0] = 1.0;
    owner_centroid(temporary.path(), owner);
    let entity_dir = temporary.path().join("entities/target");
    fs::create_dir_all(&entity_dir).unwrap();
    fs::write(
        entity_dir.join("entity.json"),
        json!({"id":"target","name":"Target","type":"Person"}).to_string(),
    )
    .unwrap();
    let mut tracker = CandidateTracker::new(temporary.path());
    tracker
        .process_segment(&[ClusterInput {
            source_segment: json!({"day":"20260808","stream":"mic","segment_key":"120000_300","source":"audio","cluster_label":1}),
            embeddings: vec![vector()],
            durations_s: vec![1.0],
        }])
        .unwrap();
    let before = tracker.snapshot_candidates_locked().unwrap().remove(0);
    let mut after = before.clone();
    after.status = "confirmed".to_owned();
    after.confirmed_entity = Some("target".to_owned());
    let item = VoiceprintItem {
        embedding: vector(),
        metadata: VoiceprintMetadata::new("20260808", "120000_300", "audio", "mic", 7, 1, 1)
            .to_json(),
    };
    let plan = RetroTrackerPhasePlan {
        matched: true,
        candidate_id: Some(before.cand_id),
        target_entity_id: "target".into(),
        planning_owner_entity_id: Some("owner".into()),
        candidate_before: Some(before.to_json()),
        candidate_after: Some(after.to_json()),
        voiceprints_to_add: vec![
            solstone_core_speaker_resolve::identify_forward_phases::RetroVoiceprintEntry {
                key: solstone_core_speaker_resolve::direct_voiceprints::DirectVoiceprintKey {
                    day: "20260808".into(),
                    segment_key: "120000_300".into(),
                    source: "audio".into(),
                    sentence_id: 7,
                },
                metadata: item.metadata.clone(),
                item,
            },
        ],
    };
    assert_eq!(
        phase_retro_tracker(temporary.path(), &mut tracker, &plan, &encoder())
            .unwrap()
            .fields["voiceprints_saved_count"],
        1
    );
    assert!(matches!(
        phase_retro_tracker(
            temporary.path(),
            &mut tracker,
            &RetroTrackerPhasePlan {
                candidate_id: Some(99),
                ..plan
            },
            &encoder(),
        ),
        Err(ForwardPhaseError::RepairRequired {
            phase: ForwardPhase::RetroTracker,
            code: "candidate_missing",
            ..
        })
    ));
}

#[test]
fn phase_retro_tracker_rescreens_frozen_embeddings_before_any_write() {
    let temporary = Temp::new();
    admitted_owner(temporary.path());
    let mut clear_owner = vec![0.0; 256];
    clear_owner[0] = 1.0;
    owner_centroid(temporary.path(), clear_owner);
    let entity_dir = temporary.path().join("entities/target");
    fs::create_dir_all(&entity_dir).unwrap();
    fs::write(
        entity_dir.join("entity.json"),
        json!({"id":"target","name":"Target","type":"Person"}).to_string(),
    )
    .unwrap();
    let mut tracker = CandidateTracker::new(temporary.path());
    tracker
        .process_segment(&[ClusterInput {
            source_segment: json!({"day":"20260808","stream":"mic","segment_key":"120000_300","source":"audio","cluster_label":1}),
            embeddings: vec![vector()],
            durations_s: vec![1.0],
        }])
        .unwrap();
    let before = tracker.snapshot_candidates_locked().unwrap().remove(0);
    let mut after = before.clone();
    after.status = "confirmed".to_owned();
    after.confirmed_entity = Some("target".to_owned());
    let item = VoiceprintItem {
        embedding: vector(),
        metadata: VoiceprintMetadata::new("20260808", "120000_300", "audio", "mic", 7, 1, 1)
            .to_json(),
    };
    let plan = RetroTrackerPhasePlan {
        matched: true,
        candidate_id: Some(before.cand_id),
        target_entity_id: "target".into(),
        planning_owner_entity_id: Some("owner".into()),
        candidate_before: Some(before.to_json()),
        candidate_after: Some(after.to_json()),
        voiceprints_to_add: vec![
            solstone_core_speaker_resolve::identify_forward_phases::RetroVoiceprintEntry {
                key: solstone_core_speaker_resolve::direct_voiceprints::DirectVoiceprintKey {
                    day: "20260808".into(),
                    segment_key: "120000_300".into(),
                    source: "audio".into(),
                    sentence_id: 7,
                },
                metadata: item.metadata.clone(),
                item,
            },
        ],
    };
    let candidate_bytes =
        fs::read(temporary.path().join("awareness/speaker_candidates.json")).unwrap();
    owner_centroid(temporary.path(), vector());

    assert!(matches!(
        phase_retro_tracker(temporary.path(), &mut tracker, &plan, &encoder()),
        Err(ForwardPhaseError::RepairRequired {
            phase: ForwardPhase::RetroTracker,
            code: "owner_similarity",
            ..
        })
    ));
    assert_eq!(
        fs::read(temporary.path().join("awareness/speaker_candidates.json")).unwrap(),
        candidate_bytes
    );
    assert!(
        !temporary
            .path()
            .join("entities/target/voiceprints.npz")
            .exists()
    );
}

#[test]
fn phase_sentinel_replays_intended_entry_and_repairs_conflicts() {
    let temporary = Temp::new();
    let plan = SentinelPhasePlan {
        cluster_key: "42".into(),
        prior_entry: None,
        intended_entry: json!({"operation_id":"idop_fixture"}),
    };
    phase_sentinel(temporary.path(), &plan).unwrap();
    phase_sentinel(temporary.path(), &plan).unwrap();
    fs::write(
        temporary
            .path()
            .join("awareness/discovery_clusters.resolved.json"),
        json!({"42":{"operation_id":"other"}}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        phase_sentinel(temporary.path(), &plan),
        Err(ForwardPhaseError::RepairRequired {
            phase: ForwardPhase::Sentinel,
            code: "concurrent_change",
            ..
        })
    ));
}

fn vector() -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[1] = 1.0;
    values
}

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".to_owned(),
        sha256: "a".repeat(64),
        width: 256,
    }
}
