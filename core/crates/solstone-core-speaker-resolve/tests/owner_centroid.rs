// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_npy::write_npy;
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteError, OwnerCentroidWriteInput, load_owner_centroid, write_owner_centroid,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-resolve-owner-centroid-{}-{sequence}",
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

fn seed_principal(temporary: &TempDir) -> PathBuf {
    let entity_dir = temporary.path().join("entities/principal");
    fs::create_dir_all(&entity_dir).expect("create principal directory");
    fs::write(
        entity_dir.join("entity.json"),
        json!({"id": "principal", "name": "Principal", "type":"Person", "is_principal": true})
            .to_string(),
    )
    .expect("write principal identity");
    entity_dir.join("owner_centroid.npz")
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn i32_bytes(value: i32) -> Vec<u8> {
    value.to_le_bytes().to_vec()
}

fn unicode_scalar_bytes(value: &str) -> Vec<u8> {
    value
        .chars()
        .flat_map(|character| (character as u32).to_le_bytes())
        .collect()
}

fn owner_archive(centroid: &[f32], margin: Option<f32>) -> Vec<u8> {
    owner_archive_with_threshold(
        centroid,
        margin,
        write_npy("<f4", "()", &f32_bytes(&[0.43])),
    )
}

fn owner_archive_with_threshold(
    centroid: &[f32],
    margin: Option<f32>,
    threshold: Vec<u8>,
) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut members = vec![
        (
            "centroid.npy",
            write_npy(
                "<f4",
                &format!("({},)", centroid.len()),
                &f32_bytes(centroid),
            ),
        ),
        ("threshold.npy", threshold),
        ("cluster_size.npy", write_npy("<i4", "()", &i32_bytes(12))),
        (
            "last_refreshed_at.npy",
            write_npy("<U20", "()", &unicode_scalar_bytes("2026-08-08T12:00:00Z")),
        ),
    ];
    if let Some(margin) = margin {
        members.push(("margin.npy", write_npy("<f4", "()", &f32_bytes(&[margin]))));
    }
    for (name, bytes) in members {
        writer.start_file(name, options).expect("start member");
        writer.write_all(&bytes).expect("write member");
    }
    writer.finish().expect("finish archive").into_inner()
}

#[test]
fn ac1_full_record_preserves_calibration_and_refresh_metadata() {
    let temporary = TempDir::new();
    let path = seed_principal(&temporary);
    fs::write(path, owner_archive(&[3.0, 4.0], Some(0.05))).expect("write centroid");

    let centroid = load_owner_centroid(temporary.path(), "principal")
        .expect("load centroid")
        .expect("centroid present");
    assert_eq!(centroid.threshold, 0.43);
    assert_eq!(centroid.margin, Some(0.05));
    assert_eq!(centroid.cluster_size, 12);
    assert_eq!(
        centroid.last_refreshed_at.as_deref(),
        Some("2026-08-08T12:00:00Z")
    );
    assert!((centroid.centroid[0] - 0.6).abs() < 0.000_001);
    assert!((centroid.centroid[1] - 0.8).abs() < 0.000_001);
}

#[test]
fn ac2_missing_margin_remains_none() {
    let temporary = TempDir::new();
    let path = seed_principal(&temporary);
    fs::write(path, owner_archive(&[1.0, 0.0], None)).expect("write centroid");

    let centroid = load_owner_centroid(temporary.path(), "principal")
        .expect("load centroid")
        .expect("centroid present");
    assert_eq!(centroid.margin, None);
}

#[test]
fn ac3_absent_centroid_returns_none() {
    let temporary = TempDir::new();
    let _ = seed_principal(&temporary);
    assert!(
        load_owner_centroid(temporary.path(), "principal")
            .expect("load absent centroid")
            .is_none()
    );
}

#[test]
fn foreign_centroid_target_is_refused_without_creating_or_retargeting_an_artifact() {
    let temporary = TempDir::new();
    let principal_path = seed_principal(&temporary);
    let before = content_snapshot(temporary.path());

    let error = write_owner_centroid(
        temporary.path(),
        "foreign",
        &OwnerCentroidWriteInput {
            centroid: vec![1.0, 0.0],
            cluster_size: 1,
            timestamp: "2026-08-08T12:00:00Z".to_owned(),
            evidence_tier: "standard".to_owned(),
        },
    )
    .expect_err("foreign target is refused");

    assert!(matches!(
        error,
        OwnerCentroidWriteError::TargetMismatch { .. }
    ));
    assert!(!principal_path.exists());
    assert!(!temporary.path().join("entities/foreign").exists());
    assert_eq!(content_snapshot(temporary.path()), before);
}

#[test]
fn ac4_zero_norm_centroid_returns_none() {
    let temporary = TempDir::new();
    let path = seed_principal(&temporary);
    fs::write(path, owner_archive(&[0.0, 0.0], Some(0.05))).expect("write centroid");
    assert!(
        load_owner_centroid(temporary.path(), "principal")
            .expect("load zero centroid")
            .is_none()
    );
}

#[test]
fn ac5_corrupt_centroid_is_reported_as_an_error() {
    let temporary = TempDir::new();
    let path = seed_principal(&temporary);
    fs::write(path, b"not an npz archive").expect("write corrupt centroid");
    assert!(load_owner_centroid(temporary.path(), "principal").is_err());
}

#[test]
fn owner_centroid_rejects_well_formed_npy_with_overlong_payload() {
    let temporary = TempDir::new();
    let path = seed_principal(&temporary);
    let mut threshold = write_npy("<f4", "()", &f32_bytes(&[0.43]));
    threshold.push(0);
    fs::write(
        path,
        owner_archive_with_threshold(&[1.0, 0.0], None, threshold),
    )
    .expect("write malformed threshold");
    assert!(load_owner_centroid(temporary.path(), "principal").is_err());
}
