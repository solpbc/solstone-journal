// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use solstone_core_speaker_resolve::layer1::{OwnerSeparationContext, separate_owner_statements};
use solstone_core_speaker_resolve::owner_centroid::OwnerCentroid;
use solstone_core_speaker_resolve::voiceprint_centroid::VoiceprintCentroidCache;

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-layer1-{}-{sequence}",
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

fn owner(threshold: f32, margin: Option<f32>) -> OwnerCentroid {
    OwnerCentroid {
        centroid: vec![1.0, 0.0],
        threshold,
        margin,
        cluster_size: 1,
        last_refreshed_at: None,
        created_at: None,
        evidence_tier: None,
        evidence_hash: None,
        evidence_intra_cosine_p25: None,
    }
}

fn run(
    temporary: &TempDir,
    statements: &[(i64, Vec<f32>)],
    owner: &OwnerCentroid,
    ids: &[String],
) -> solstone_core_speaker_resolve::layer1::Layer1Result {
    separate_owner_statements(
        statements,
        OwnerSeparationContext {
            owner,
            owner_entity_id: "owner",
            margin_non_principal_entity_ids: ids,
            journal_root: temporary.path(),
            stream: "mic",
            now_ms: 1,
        },
        &mut VoiceprintCentroidCache::default(),
        &mut Vec::new(),
    )
}

fn write_voiceprint(root: &Path, entity_id: &str) {
    let identity = root.join("entities").join(entity_id).join("entity.json");
    fs::create_dir_all(identity.parent().expect("identity parent")).expect("create entity");
    fs::write(
        identity,
        json!({"id": entity_id, "name": "Other"}).to_string(),
    )
    .expect("write identity");
    let encoder = EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    let mut embedding = vec![0.0; 256];
    embedding[0] = 1.0;
    save_voiceprints_batch(
        root,
        entity_id,
        &[VoiceprintItem {
            embedding,
            metadata: json!({"stream": "mic", "added_at": 1}),
        }],
        &encoder,
    )
    .expect("write voiceprint");
}

#[test]
fn ac9_threshold_decline_becomes_non_owner() {
    let temporary = TempDir::new();
    let result = run(
        &temporary,
        &[(1, vec![0.42, 0.907_524])],
        &owner(0.43, None),
        &[],
    );
    assert_eq!(result.non_owner_sids, [1]);
    assert_eq!(result.labels[&1].speaker, None);
}

#[test]
fn ac10_margin_none_never_marks_a_decline() {
    let temporary = TempDir::new();
    let result = run(&temporary, &[(1, vec![1.0, 0.0])], &owner(0.43, None), &[]);
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("owner"));
    assert_eq!(result.labels[&1].owner_margin_declined, None);
    assert!(result.margin_declined_sids.is_empty());
}

#[test]
fn ac10_margin_some_declines_a_too_close_owner_claim() {
    let temporary = TempDir::new();
    write_voiceprint(temporary.path(), "other");
    let result = run(
        &temporary,
        &[(1, vec![1.0, 0.0])],
        &owner(0.43, Some(0.05)),
        &["other".to_owned()],
    );
    assert_eq!(result.labels[&1].speaker, None);
    assert_eq!(result.labels[&1].owner_margin_declined, Some(true));
    assert_eq!(result.non_owner_sids, [1]);
    assert!(result.margin_declined_sids.contains(&1));
}

#[test]
fn ac12_non_normalizable_embedding_is_unreachable_by_later_layers() {
    let temporary = TempDir::new();
    let result = run(
        &temporary,
        &[(1, vec![0.0, 0.0])],
        &owner(0.43, Some(0.05)),
        &[],
    );
    assert_eq!(result.labels[&1].speaker, None);
    assert!(result.non_owner_sids.is_empty());
    assert!(result.margin_declined_sids.is_empty());
}

#[test]
fn owner_margin_zero_usable_comparisons_keeps_owner_claim_with_negative_infinity_floor() {
    let temporary = TempDir::new();
    let identity = temporary.path().join("entities/other/entity.json");
    fs::create_dir_all(identity.parent().expect("identity parent")).expect("create entity");
    fs::write(
        identity,
        json!({"id": "other", "name": "Other"}).to_string(),
    )
    .expect("write entity");
    let result = run(
        &temporary,
        &[(1, vec![1.0, 0.0])],
        &owner(0.43, Some(0.05)),
        &["other".to_owned()],
    );
    assert_eq!(result.labels[&1].speaker.as_deref(), Some("owner"));
    assert!(result.margin_declined_sids.is_empty());
}
