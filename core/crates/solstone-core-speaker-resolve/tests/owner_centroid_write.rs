// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidRebuildInput, OwnerCentroidRebuildOutcome, OwnerCentroidWriteInput,
    load_owner_centroid, rebuild_owner_centroid, write_owner_centroid,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-owner-write-{}-{}",
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
fn principal(t: &TempDir) {
    let d = t.path().join("entities/owner");
    fs::create_dir_all(&d).unwrap();
    fs::write(
        d.join("entity.json"),
        json!({"id":"owner","is_principal":true,"type":"Person"}).to_string(),
    )
    .unwrap();
}
fn build(t: &TempDir) {
    write_owner_centroid(
        t.path(),
        "owner",
        &OwnerCentroidWriteInput {
            centroid: vec![1.0, 0.0],
            cluster_size: 10,
            timestamp: "2026-08-08T12:00:00Z".into(),
            evidence_tier: "high".into(),
        },
    )
    .unwrap();
}
fn rebuild_input(centroid: Vec<f32>) -> OwnerCentroidRebuildInput {
    OwnerCentroidRebuildInput {
        centroid,
        embeddings_count: 10,
        timestamp: "2026-08-09T12:00:00Z".into(),
        evidence_hash: "new-hash".into(),
        evidence_intra_cosine_p25: 0.9,
        evidence_tier: "high".into(),
        override_regression: false,
    }
}

#[test]
fn ac15_rebuild_short_circuits_matching_evidence_hash_without_rewriting() {
    let t = TempDir::new();
    principal(&t);
    build(&t);
    assert!(matches!(
        rebuild_owner_centroid(t.path(), "owner", &rebuild_input(vec![1.0, 0.0])).unwrap(),
        OwnerCentroidRebuildOutcome::Rebuilt { .. }
    ));
    let path = t.path().join("entities/owner/owner_centroid.npz");
    let before = fs::read(&path).unwrap();
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &rebuild_input(vec![0.0, 1.0])).unwrap(),
        OwnerCentroidRebuildOutcome::Unchanged
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn ac16_rebuild_refuses_without_an_incumbent() {
    let t = TempDir::new();
    principal(&t);
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &rebuild_input(vec![1.0, 0.0])).unwrap(),
        OwnerCentroidRebuildOutcome::Refused {
            reason: "no_owner_centroid".into()
        }
    );
}

#[test]
fn ac17_cluster_regression_only_applies_to_rebuild_sourced_incumbents() {
    let t = TempDir::new();
    principal(&t);
    build(&t);
    let mut input = rebuild_input(vec![1.0, 0.0]);
    input.embeddings_count = 1;
    assert!(matches!(
        rebuild_owner_centroid(t.path(), "owner", &input).unwrap(),
        OwnerCentroidRebuildOutcome::Rebuilt { .. }
    ));
    let mut next = rebuild_input(vec![1.0, 0.0]);
    next.evidence_hash = "different".into();
    next.embeddings_count = 0;
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &next).unwrap(),
        OwnerCentroidRebuildOutcome::Refused {
            reason: "cluster_size_regression".into()
        }
    );
}

#[test]
fn ac18_centroid_and_cohesion_regressions_refuse_unless_overridden() {
    let t = TempDir::new();
    principal(&t);
    build(&t);
    let mut bad = rebuild_input(vec![0.0, 1.0]);
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &bad).unwrap(),
        OwnerCentroidRebuildOutcome::Refused {
            reason: "centroid_agreement_too_low".into()
        }
    );
    bad.override_regression = true;
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &bad).unwrap(),
        OwnerCentroidRebuildOutcome::Rebuilt {
            override_applied: true
        }
    );
    let mut cohesion = rebuild_input(vec![0.0, 1.0]);
    cohesion.evidence_hash = "another".into();
    cohesion.evidence_intra_cosine_p25 = 0.7;
    assert_eq!(
        rebuild_owner_centroid(t.path(), "owner", &cohesion).unwrap(),
        OwnerCentroidRebuildOutcome::Refused {
            reason: "cohesion_regression".into()
        }
    );
}

#[test]
fn ac19_build_writes_seven_members_and_ac20_rebuild_writes_nine_members() {
    let t = TempDir::new();
    principal(&t);
    build(&t);
    let path = t.path().join("entities/owner/owner_centroid.npz");
    let bytes = fs::read(&path).unwrap();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 7);
    rebuild_owner_centroid(t.path(), "owner", &rebuild_input(vec![1.0, 0.0])).unwrap();
    let loaded = load_owner_centroid(t.path(), "owner").unwrap().unwrap();
    assert_eq!(loaded.evidence_hash.as_deref(), Some("new-hash"));
    let bytes = fs::read(path).unwrap();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 9);
}

#[test]
fn ac19_owner_centroid_busy_lock_preserves_bytes_and_sidecar() {
    let t = TempDir::new();
    principal(&t);
    build(&t);
    let path = t.path().join("entities/owner/owner_centroid.npz");
    let before = fs::read(&path).unwrap();
    let guard = hold_lock(&path, LockOptions::default()).unwrap();
    let error = write_owner_centroid(
        t.path(),
        "owner",
        &OwnerCentroidWriteInput {
            centroid: vec![0.0, 1.0],
            cluster_size: 9,
            timestamp: "2026-08-10T00:00:00Z".into(),
            evidence_tier: "high".into(),
        },
    )
    .unwrap_err();
    drop(guard);
    assert!(
        error
            .to_string()
            .contains("voiceprint storage is busy; try again")
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(path.with_file_name("owner_centroid.npz.lock").exists());
}
