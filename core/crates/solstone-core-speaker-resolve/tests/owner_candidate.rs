// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use solstone_core_journal_io::{LockOptions, hold_lock};
use solstone_core_speaker_resolve::owner_candidate::{
    OwnerCandidate, load_owner_candidate, write_owner_candidate,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-owner-candidate-{}-{}",
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

#[test]
fn ac20_owner_candidate_busy_lock_preserves_bytes_and_sidecar() {
    let temporary = TempDir::new();
    let candidate = OwnerCandidate {
        centroid: vec![1.0, 0.0],
        cluster_size: 2,
        threshold: 0.4,
        version: "v1".into(),
        evidence_tier: "high".into(),
    };
    write_owner_candidate(temporary.path(), &candidate).unwrap();
    let path = temporary.path().join("awareness/owner_candidate.npz");
    let before = fs::read(&path).unwrap();
    let guard = hold_lock(&path, LockOptions::default()).unwrap();
    let error = write_owner_candidate(
        temporary.path(),
        &OwnerCandidate {
            centroid: vec![0.0, 1.0],
            ..candidate
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
    assert!(path.with_file_name("owner_candidate.npz.lock").exists());
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ac21_owner_candidate_round_trips_the_five_member_record() {
    let temporary = TempDir::new();
    let candidate = OwnerCandidate {
        centroid: vec![3.0, 4.0],
        cluster_size: 17,
        threshold: 0.41,
        version: "owner-candidate-v1".to_owned(),
        evidence_tier: "high".to_owned(),
    };
    write_owner_candidate(temporary.path(), &candidate).unwrap();
    let loaded = load_owner_candidate(temporary.path()).unwrap().unwrap();
    assert_eq!(loaded.cluster_size, 17);
    assert_eq!(loaded.threshold, 0.41);
    assert_eq!(loaded.version, "owner-candidate-v1");
    assert_eq!(loaded.evidence_tier, "high");
    assert_eq!(loaded.centroid, vec![0.6, 0.8]);
    let bytes = fs::read(temporary.path().join("awareness/owner_candidate.npz")).unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(archive.len(), 5);
    for name in [
        "centroid.npy",
        "cluster_size.npy",
        "threshold.npy",
        "version.npy",
        "evidence_tier.npy",
    ] {
        assert!(archive.by_name(name).is_ok(), "missing {name}");
    }
}
