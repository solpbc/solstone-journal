// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::{
    EncoderIdentity, VoiceprintItem, load_entity_voiceprints_file, save_voiceprints_batch,
};
use solstone_core_journal_io::segment_path;
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteInput, write_owner_centroid,
};
use solstone_core_speaker_resolve::voiceprint_accumulation::{
    AccumulationEmbedding, AccumulationLabel, AccumulationOutcome, AccumulationRequest,
    AccumulationSkipReason, EntityWriteStatus, accumulate_voiceprints,
};
use solstone_core_speaker_resolve::voiceprint_centroid::decay_weighted_centroid;

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-accumulate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
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

fn content_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("journal directory reads") {
            let entry = entry.expect("journal entry reads");
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, snapshot);
            } else if path.is_file() {
                snapshot.insert(
                    path.strip_prefix(root)
                        .expect("snapshot path is relative")
                        .to_path_buf(),
                    fs::read(path).expect("journal file reads"),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    collect(root, root, &mut snapshot);
    snapshot
}

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test-encoder".into(),
        sha256: "a".repeat(64),
        width: 256,
    }
}
fn vector(x: f32, y: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = x;
    values[1] = y;
    values
}
fn entity(t: &TempDir, id: &str, kind: &str, principal: bool) {
    let dir = t.path().join("entities").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("entity.json"),
        json!({"id":id,"name":id,"type":kind,"is_principal":principal}).to_string(),
    )
    .unwrap();
}
fn owner(t: &TempDir) {
    entity(t, "owner", "Person", true);
    write_owner_centroid(
        t.path(),
        "owner",
        &OwnerCentroidWriteInput {
            centroid: vector(1.0, 0.0),
            cluster_size: 10,
            timestamp: "2026-08-08T12:00:00Z".into(),
            evidence_tier: "high".into(),
        },
    )
    .unwrap();
}
fn request(
    t: &TempDir,
    labels: Vec<AccumulationLabel>,
    embeddings: Vec<AccumulationEmbedding>,
    entities: Vec<&str>,
) -> AccumulationRequest {
    AccumulationRequest {
        journal_root: t.path().to_path_buf(),
        day: "20260808".into(),
        stream: "main".into(),
        segment_key: "120000_300".into(),
        source: "transcript".into(),
        now_ms: 1234,
        encoder: encoder(),
        labels,
        embeddings,
        entity_ids: entities.into_iter().map(str::to_owned).collect(),
    }
}
fn label(id: i64, speaker: &str) -> AccumulationLabel {
    AccumulationLabel {
        sentence_id: id,
        speaker: Some(speaker.into()),
        confidence: Some("high".into()),
        method: Some("acoustic".into()),
    }
}
fn embedding(id: i64, values: Vec<f32>) -> AccumulationEmbedding {
    AccumulationEmbedding {
        sentence_id: id,
        values,
    }
}
fn source(t: &TempDir, header: &str) {
    let dir = segment_path(t.path(), "20260808", "120000_300", "main", true).unwrap();
    fs::write(dir.join("transcript.jsonl"), header).unwrap();
}
fn skip(outcome: &AccumulationOutcome, reason: AccumulationSkipReason) -> usize {
    match outcome {
        AccumulationOutcome::NothingEligible { skipped_rows, .. }
        | AccumulationOutcome::Completed { skipped_rows, .. } => {
            *skipped_rows.get(&reason).unwrap_or(&0)
        }
        AccumulationOutcome::IdentityInvalid { .. }
        | AccumulationOutcome::NoOwnerCentroid { .. } => 0,
    }
}

#[test]
fn ac3_absent_owner_centroid_writes_nothing_with_a_specific_outcome() {
    let t = TempDir::new();
    entity(&t, "owner", "Person", true);
    entity(&t, "alice", "Person", false);
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(1, "alice")],
        vec![embedding(1, vector(0.0, 1.0))],
        vec!["alice"],
    ))
    .unwrap();
    assert!(matches!(
        result,
        AccumulationOutcome::NoOwnerCentroid { .. }
    ));
    assert!(load_entity_voiceprints_file(t.path(), "alice").is_none());
}

#[test]
fn invalid_owner_identity_writes_nothing_with_a_distinct_outcome() {
    let t = TempDir::new();
    entity(&t, "alice", "Person", false);
    let before = content_snapshot(t.path());
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(1, "alice")],
        vec![embedding(1, vector(0.0, 1.0))],
        vec!["alice"],
    ))
    .unwrap();
    assert!(matches!(
        result,
        AccumulationOutcome::IdentityInvalid { .. }
    ));
    assert!(load_entity_voiceprints_file(t.path(), "alice").is_none());
    assert_eq!(content_snapshot(t.path()), before);
}

#[test]
fn ac4_noisy_overlap_from_string_header_skips_all_rows() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "alice", "Person", false);
    source(&t, "{\"overlap_fraction\":\"0.11\"}\n");
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(1, "alice")],
        vec![embedding(1, vector(0.0, 1.0))],
        vec!["alice"],
    ))
    .unwrap();
    assert_eq!(skip(&result, AccumulationSkipReason::NoisyOverlap), 1);
    assert!(load_entity_voiceprints_file(t.path(), "alice").is_none());
}

#[test]
fn ac5_threshold_is_inclusive_and_ac6_to_ac8_apply_in_order() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "alice", "Person", false);
    let mut low = label(1, "alice");
    low.confidence = Some("medium".into());
    let mut method = label(2, "alice");
    method.method = Some("manual".into());
    let owner_label = label(3, "owner");
    let exact = label(4, "alice");
    let below = label(5, "alice");
    let result = accumulate_voiceprints(&request(
        &t,
        vec![low, method, owner_label, exact, below],
        vec![
            embedding(4, vector(0.43, (1.0 - 0.43f32.powi(2)).sqrt())),
            embedding(5, vector(0.42, 1.0)),
        ],
        vec!["alice"],
    ))
    .unwrap();
    assert_eq!(skip(&result, AccumulationSkipReason::LowConfidence), 1);
    assert_eq!(skip(&result, AccumulationSkipReason::UnsupportedMethod), 1);
    assert_eq!(skip(&result, AccumulationSkipReason::OwnerEntity), 1);
    assert_eq!(skip(&result, AccumulationSkipReason::OwnerContamination), 1);
    assert!(matches!(
        result,
        AccumulationOutcome::Completed {
            written_rows: 1,
            ..
        }
    ));
}

#[test]
fn ac9_plain_mean_outlier_and_ac10_ac11_deduplicate_without_a_write() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "alice", "Person", false);
    let now_ms = 25_920_000_000;
    let candidate = vector(0.42, -1.0);
    let mut rows = (0..4)
        .map(|id| VoiceprintItem {
            embedding: vector(-0.42, 1.0),
            metadata: json!({"source":"old","sentence_id":id,"stream":"main","added_at":0}),
        })
        .collect::<Vec<_>>();
    rows.push(VoiceprintItem {
        embedding: candidate.clone(),
        metadata: json!({"source":"old","sentence_id":4,"stream":"main","added_at":now_ms}),
    });
    save_voiceprints_batch(t.path(), "alice", &rows, &encoder()).unwrap();
    let decay_rows = rows
        .iter()
        .map(|row| (row.embedding.clone(), row.metadata.clone()))
        .collect::<Vec<_>>();
    let decay = decay_weighted_centroid(&decay_rows, "main", now_ms).unwrap();
    assert!(
        decay[0] * 0.42 - decay[1] > 0.18,
        "decay would admit this row"
    );
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(9, "alice"), label(9, "alice")],
        vec![embedding(9, vector(1.0, 0.0))],
        vec!["alice"],
    ));
    assert!(
        result.is_err(),
        "duplicate label IDs are rejected before writes"
    );
    let mut outlier = request(
        &t,
        vec![label(8, "alice")],
        vec![embedding(8, candidate)],
        vec!["alice"],
    );
    outlier.now_ms = now_ms;
    let result = accumulate_voiceprints(&outlier).unwrap();
    assert_eq!(skip(&result, AccumulationSkipReason::Outlier), 1);
}

#[test]
fn ac10_cross_call_idempotency_uses_day_segment_source_and_sentence_id() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "alice", "Person", false);
    save_voiceprints_batch(t.path(), "alice", &[VoiceprintItem { embedding: vector(0.0, 1.0), metadata: json!({"day":"20260808","segment_key":"120000_300","source":"transcript","sentence_id":1}) }], &encoder()).unwrap();
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(1, "alice")],
        vec![embedding(1, vector(0.0, 1.0))],
        vec!["alice"],
    ))
    .unwrap();
    assert_eq!(skip(&result, AccumulationSkipReason::ExistingVoiceprint), 1);
    assert_eq!(
        load_entity_voiceprints_file(t.path(), "alice")
            .unwrap()
            .rows,
        1
    );
}

#[test]
fn ac12_non_person_refusal_leaves_archive_bytes_unchanged_and_ac13_ac14_report_guards() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "tool", "Tool", false);
    entity(&t, "alice", "Person", false);
    save_voiceprints_batch(
        t.path(),
        "tool",
        &[VoiceprintItem {
            embedding: vector(0.0, 1.0),
            metadata: json!({"source":"before","sentence_id":7}),
        }],
        &encoder(),
    )
    .unwrap();
    let path = t.path().join("entities/tool/voiceprints.npz");
    let before = fs::read(&path).unwrap();
    let unknown = label(1, "missing");
    let missing = label(2, "tool");
    let zero = label(3, "tool");
    let non_person = label(4, "tool");
    let result = accumulate_voiceprints(&request(
        &t,
        vec![unknown, missing, zero, non_person],
        vec![
            embedding(3, vector(0.0, 0.0)),
            embedding(4, vector(0.0, 1.0)),
        ],
        vec!["tool"],
    ))
    .unwrap();
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(skip(&result, AccumulationSkipReason::MissingEmbedding), 1);
    assert_eq!(skip(&result, AccumulationSkipReason::ZeroNormEmbedding), 1);
    assert_eq!(skip(&result, AccumulationSkipReason::UnknownEntity), 2);
    let accepted = accumulate_voiceprints(&request(
        &t,
        vec![label(4, "alice")],
        vec![embedding(4, vector(0.0, 1.0))],
        vec!["alice"],
    ))
    .unwrap();
    assert!(matches!(
        accepted,
        AccumulationOutcome::Completed {
            written_rows: 1,
            ..
        }
    ));
    assert_eq!(
        load_entity_voiceprints_file(t.path(), "alice")
            .unwrap()
            .rows,
        1
    );
}

#[test]
fn ac22_one_entity_write_failure_does_not_abort_another_and_ac23_uses_segment_time() {
    let t = TempDir::new();
    owner(&t);
    entity(&t, "alice", "Person", false);
    entity(&t, "bob", "Person", false);
    let other = EncoderIdentity {
        id: "other".into(),
        sha256: "b".repeat(64),
        width: 256,
    };
    save_voiceprints_batch(
        t.path(),
        "bob",
        &[VoiceprintItem {
            embedding: vector(0.0, 1.0),
            metadata: json!({}),
        }],
        &other,
    )
    .unwrap();
    let result = accumulate_voiceprints(&request(
        &t,
        vec![label(1, "alice"), label(2, "bob")],
        vec![
            embedding(1, vector(0.0, 1.0)),
            embedding(2, vector(0.0, 1.0)),
        ],
        vec!["alice", "bob"],
    ))
    .unwrap();
    let AccumulationOutcome::Completed { entity_reports, .. } = result else {
        panic!("alice must succeed")
    };
    assert!(matches!(
        entity_reports["alice"].write_status,
        EntityWriteStatus::Written
    ));
    assert!(matches!(
        entity_reports["bob"].write_status,
        EntityWriteStatus::Failed { .. }
    ));
    let archive = load_entity_voiceprints_file(t.path(), "alice").unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&archive.metadata[0]).unwrap();
    assert_eq!(metadata["added_at"], 1234);
    assert_eq!(metadata["last_seen_ts"], 1_786_190_400_000i64);
}
