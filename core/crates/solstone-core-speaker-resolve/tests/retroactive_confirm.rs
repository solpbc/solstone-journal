// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use solstone_core_npy::write_npy;
use solstone_core_speaker_id::calibration::VP_OUTLIER_MIN_SAMPLES;
use solstone_core_speaker_resolve::candidate_tracker::retroactive_voiceprint_metadata;
use solstone_core_speaker_resolve::candidate_tracker::{
    CandidateProfile, CandidateTracker, ClusterInput,
};
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteInput, write_owner_centroid,
};
use solstone_core_speaker_resolve::retroactive_confirm::{
    RetroactiveConfirmError, RetroactiveConfirmPlan, apply_retroactive_confirm_plan,
    plan_retroactive_confirm,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "solstone-retro-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
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
fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".into(),
        sha256: "a".repeat(64),
        width: 256,
    }
}
fn vector(x: f32, y: f32) -> Vec<f32> {
    let mut v = vec![0.; 256];
    v[0] = x;
    v[1] = y;
    v
}
fn entity(root: &Path, id: &str, kind: &str, principal: bool) {
    let p = root.join("entities").join(id);
    fs::create_dir_all(&p).unwrap();
    fs::write(
        p.join("entity.json"),
        json!({"id":id,"name":id,"type":kind,"is_principal":principal}).to_string(),
    )
    .unwrap();
}
fn item(id: i64) -> VoiceprintItem {
    VoiceprintItem {
        embedding: vector(0., 1.),
        metadata: retroactive_voiceprint_metadata(
            "20260808",
            "mic",
            "120000_300",
            "audio",
            id,
            1,
            1,
        ),
    }
}
fn flat(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn ints(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn embeddings(root: &Path) {
    let d = root.join("chronicle/20260808/mic/120000_300");
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("audio.jsonl"), "{}\n").unwrap();
    let mut z = ZipWriter::new(Cursor::new(Vec::new()));
    let o = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    z.start_file("embeddings.npy", o).unwrap();
    z.write_all(&write_npy("<f4", "(1, 256)", &flat(&vector(0., 1.))))
        .unwrap();
    z.start_file("statement_ids.npy", o).unwrap();
    z.write_all(&write_npy("<i4", "(1,)", &ints(&[7]))).unwrap();
    fs::write(d.join("audio.npz"), z.finish().unwrap().into_inner()).unwrap();
}
fn candidate() -> CandidateProfile {
    CandidateProfile {
        cand_id: 1,
        centroid: vector(0., 1.),
        n_segments: 1,
        n_intervals: 1,
        total_duration_s: 1.,
        source_segments: vec![
            json!({"day":"20260808","segment_key":"120000_300","stream":"mic","source":"audio","sentence_ids":[7]}),
        ],
        confirmed_entity: None,
        status: "pending".into(),
        merge_events: vec![],
    }
}
fn owner(root: &Path) {
    write_owner_centroid(
        root,
        "principal",
        &OwnerCentroidWriteInput {
            centroid: vector(1., 0.),
            cluster_size: 1,
            timestamp: "2026-08-08T00:00:00Z".into(),
            evidence_tier: "test".into(),
        },
    )
    .unwrap();
}

#[test]
fn ac4_retroactive_apply_refuses_non_person_and_persists_person_confirmation() {
    let t = Temp::new();
    entity(t.path(), "tool", "Tool", false);
    entity(t.path(), "person", "Person", false);
    let e = encoder();
    save_voiceprints_batch(t.path(), "tool", &[item(1)], &e).unwrap();
    let p = t.path().join("entities/tool/voiceprints.npz");
    let before = fs::read(&p).unwrap();
    let mut tracker = CandidateTracker::new(t.path());
    tracker.process_segment(&[ClusterInput{source_segment:json!({"day":"d","segment_key":"s","stream":"m","source":"a","cluster_label":1}),embeddings:vec![vector(0.,1.)],durations_s:vec![1.]}]).unwrap();
    let bad = RetroactiveConfirmPlan {
        matched: true,
        candidate_id: Some(1),
        entity_id: "tool".into(),
        items: vec![item(2)],
    };
    assert!(matches!(
        apply_retroactive_confirm_plan(&mut tracker, t.path(), &bad, &e),
        Err(RetroactiveConfirmError::NonPerson)
    ));
    assert_eq!(fs::read(&p).unwrap(), before);
    let good = RetroactiveConfirmPlan {
        matched: true,
        candidate_id: Some(1),
        entity_id: "person".into(),
        items: vec![item(3)],
    };
    assert_eq!(
        apply_retroactive_confirm_plan(&mut tracker, t.path(), &good, &e).unwrap(),
        1
    );
    let saved = tracker.snapshot_candidates_locked().unwrap();
    assert_eq!(saved[0].confirmed_entity.as_deref(), Some("person"));
    assert_eq!(saved[0].status, "confirmed");
}

#[test]
fn ac17_retroactive_plan_outlier_floor_and_threshold() {
    let t = Temp::new();
    entity(t.path(), "principal", "Person", true);
    entity(t.path(), "target", "Person", false);
    owner(t.path());
    embeddings(t.path());
    let c = candidate();
    let e = encoder();
    let existing = (0..VP_OUTLIER_MIN_SAMPLES - 1)
        .map(|id| VoiceprintItem {
            embedding: vector(1., 0.),
            metadata: retroactive_voiceprint_metadata(
                "20260808", "mic", "prior", "audio", id as i64, 1, 1,
            ),
        })
        .collect::<Vec<_>>();
    save_voiceprints_batch(t.path(), "target", &existing, &e).unwrap();
    let below = plan_retroactive_confirm(t.path(), &c, &vector(0., 1.), "target", 1);
    assert_eq!(below.items.len(), 1);
    save_voiceprints_batch(
        t.path(),
        "target",
        &[VoiceprintItem {
            embedding: vector(1., 0.),
            metadata: retroactive_voiceprint_metadata(
                "20260808", "mic", "prior", "audio", 99, 1, 1,
            ),
        }],
        &e,
    )
    .unwrap();
    let at_floor = plan_retroactive_confirm(t.path(), &c, &vector(0., 1.), "target", 1);
    assert!(at_floor.items.is_empty());
}
