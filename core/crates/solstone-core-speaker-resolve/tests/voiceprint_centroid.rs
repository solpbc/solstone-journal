// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_speaker_resolve::voiceprint_centroid::{
    VoiceprintCentroidCache, decay_weighted_centroid,
};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-centroid-{}-{sequence}",
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

fn row(embedding: Vec<f32>, stream: &str, added_at: Value) -> (Vec<f32>, Value) {
    (embedding, json!({"stream": stream, "added_at": added_at}))
}

#[test]
fn ac17_now_ms_is_deterministic_and_changes_decay_weights() {
    let rows = vec![
        row(vec![1.0, 0.0], "mic", json!(0)),
        row(vec![0.0, 1.0], "mic", json!(86_400_000)),
    ];
    let first = decay_weighted_centroid(&rows, "mic", 0).expect("centroid");
    let repeat = decay_weighted_centroid(&rows, "mic", 0).expect("centroid");
    let later = decay_weighted_centroid(&rows, "mic", 86_400_000).expect("centroid");
    assert_eq!(first, repeat);
    assert_ne!(first, later);
}

#[test]
fn ac18_same_stream_threshold_and_invalid_added_at_match_python_rules() {
    let mut five_same = (0..5)
        .map(|_| row(vec![1.0, 0.0], "mic", json!(0)))
        .collect::<Vec<_>>();
    five_same.push(row(vec![0.0, 1.0], "other", json!(0)));
    let centroid = decay_weighted_centroid(&five_same, "mic", 0).expect("centroid");
    assert_eq!(centroid, vec![1.0, 0.0]);

    let mut four_same = (0..4)
        .map(|_| row(vec![1.0, 0.0], "mic", json!(0)))
        .collect::<Vec<_>>();
    four_same.push(row(vec![0.0, 1.0], "other", json!(0)));
    let centroid = decay_weighted_centroid(&four_same, "mic", 0).expect("centroid");
    assert!(centroid[0] < 1.0 && centroid[1] > 0.0);

    let invalid_added_at = vec![
        row(vec![1.0, 0.0], "mic", json!(0)),
        row(vec![0.0, 1.0], "mic", json!("not-a-number")),
    ];
    let centroid = decay_weighted_centroid(&invalid_added_at, "mic", 86_400_000).expect("centroid");
    assert!(centroid[1] > centroid[0]);
}

#[test]
fn ac23_corrupt_voiceprint_produces_one_cached_gap() {
    let temporary = TempDir::new();
    let identity = temporary.path().join("entities/alice/entity.json");
    fs::create_dir_all(identity.parent().expect("identity parent")).expect("create entity");
    fs::write(
        identity,
        json!({"id": "alice", "name": "Alice"}).to_string(),
    )
    .expect("write identity");
    fs::write(
        temporary.path().join("entities/alice/voiceprints.npz"),
        b"not an archive",
    )
    .expect("write corrupt archive");

    let mut cache = VoiceprintCentroidCache::default();
    let mut gaps = Vec::new();
    assert!(
        !cache
            .entry_for(temporary.path(), "alice", "mic", 1, &mut gaps)
            .usable
    );
    assert!(
        !cache
            .entry_for(temporary.path(), "alice", "mic", 1, &mut gaps)
            .usable
    );
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].source, "voiceprint");
    assert_eq!(gaps[0].reason, "unreadable");
    assert_eq!(gaps[0].entity_id, "alice");
}
