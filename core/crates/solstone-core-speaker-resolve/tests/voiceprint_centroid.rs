// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
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

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    }
}

fn voiceprint_item() -> VoiceprintItem {
    let mut embedding = vec![0.0; 256];
    embedding[0] = 1.0;
    VoiceprintItem {
        embedding,
        metadata: json!({"stream": "mic", "added_at": 1}),
    }
}

fn write_identity(root: &Path, entity_dir: &str, entity_id: &str) {
    let identity = root.join("entities").join(entity_dir).join("entity.json");
    fs::create_dir_all(identity.parent().expect("identity parent")).expect("create entity");
    fs::write(
        identity,
        json!({"id": entity_id, "name": entity_id}).to_string(),
    )
    .expect("write identity");
}

fn write_voiceprint(root: &Path, entity_id: &str) {
    write_identity(root, entity_id, entity_id);
    save_voiceprints_batch(root, entity_id, &[voiceprint_item()], &encoder())
        .expect("write voiceprint");
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

/// Resolving an entity id through the entity store rebuilds the journal identity
/// map, which reads every `entities/*/entity.json` on disk. Attribution resolves
/// one id per admissible person, so a per-lookup resolve is quadratic in journal
/// size. The cache reads that map once and reuses it.
///
/// Removing an identity record after the first lookup makes the difference
/// observable: a per-lookup resolve no longer places `bob`, the snapshot still
/// does.
#[test]
fn entry_for_reads_the_identity_map_once_per_cache() {
    let temporary = TempDir::new();
    write_voiceprint(temporary.path(), "alice");
    write_voiceprint(temporary.path(), "bob");

    let mut cache = VoiceprintCentroidCache::default();
    let mut gaps = Vec::new();
    assert!(
        cache
            .entry_for(temporary.path(), "alice", "mic", 1, &mut gaps)
            .usable
    );

    fs::remove_file(temporary.path().join("entities/bob/entity.json"))
        .expect("remove bob identity");

    assert!(
        cache
            .entry_for(temporary.path(), "bob", "mic", 1, &mut gaps)
            .usable
    );
    assert!(gaps.is_empty());
}

/// The snapshot maps an effective id to its directory, which is not always the
/// id itself. Reading `entities/<id>/` directly would silently miss the archive.
#[test]
fn entry_for_resolves_an_entity_whose_directory_is_not_its_id() {
    let temporary = TempDir::new();
    write_identity(temporary.path(), "legacy-0001", "alice");
    save_voiceprints_batch(temporary.path(), "alice", &[voiceprint_item()], &encoder())
        .expect("write voiceprint");

    let mut cache = VoiceprintCentroidCache::default();
    let mut gaps = Vec::new();
    let entry = cache.entry_for(temporary.path(), "alice", "mic", 1, &mut gaps);

    assert!(entry.usable);
    assert_eq!(entry.embedding_count, 1);
    assert!(
        temporary
            .path()
            .join("entities/legacy-0001/voiceprints.npz")
            .exists()
    );
    assert!(!temporary.path().join("entities/alice").exists());
    assert!(gaps.is_empty());
}

/// An entity written after the snapshot is absent from it, and still resolves
/// through a fresh lookup.
#[test]
fn entry_for_falls_back_for_an_entity_written_after_the_snapshot() {
    let temporary = TempDir::new();
    write_voiceprint(temporary.path(), "alice");

    let mut cache = VoiceprintCentroidCache::default();
    let mut gaps = Vec::new();
    assert!(
        cache
            .entry_for(temporary.path(), "alice", "mic", 1, &mut gaps)
            .usable
    );

    write_voiceprint(temporary.path(), "carol");

    assert!(
        cache
            .entry_for(temporary.path(), "carol", "mic", 1, &mut gaps)
            .usable
    );
    assert!(gaps.is_empty());
}
