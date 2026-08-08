// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use solstone_core_entity::write_npy;
use solstone_core_speaker_id::resolve::{ResolveOutcome, resolve};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-resolve-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create temp");
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

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn i32_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
fn embedding(x: f32, y: f32) -> Vec<f32> {
    let mut value = vec![0.0; 256];
    value[0] = x;
    value[1] = y;
    value
}
fn archive(members: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(name, options).expect("member");
        writer.write_all(&bytes).expect("bytes");
    }
    writer.finish().expect("archive").into_inner()
}

fn segment(root: &Path) -> PathBuf {
    let path = root.join("chronicle/20260808/mic/120000_300");
    fs::create_dir_all(path.join("talents")).expect("segment");
    path
}
fn entity(root: &Path, id: &str, name: &str, principal: bool) {
    let path = root.join("entities").join(id);
    fs::create_dir_all(&path).expect("entity");
    fs::write(
        path.join("entity.json"),
        json!({"id": id, "name": name, "type": "Person", "is_principal": principal}).to_string(),
    )
    .expect("identity");
}
fn owner(root: &Path) {
    let centroid = embedding(1.0, 0.0);
    let bytes = archive(vec![
        (
            "centroid.npy",
            write_npy("<f4", "(256,)", &f32_bytes(&centroid)),
        ),
        ("threshold.npy", write_npy("<f4", "()", &f32_bytes(&[0.43]))),
        ("cluster_size.npy", write_npy("<i4", "()", &i32_bytes(&[1]))),
    ]);
    fs::write(root.join("entities/principal/owner_centroid.npz"), bytes).expect("owner centroid");
}
fn embeddings(path: &Path, ids: &[i32], values: &[Vec<f32>]) {
    let flat = values.iter().flatten().copied().collect::<Vec<_>>();
    let bytes = archive(vec![
        (
            "embeddings.npy",
            write_npy(
                "<f4",
                &format!("({}, 256)", values.len()),
                &f32_bytes(&flat),
            ),
        ),
        (
            "statement_ids.npy",
            write_npy("<i4", &format!("({},)", ids.len()), &i32_bytes(ids)),
        ),
    ]);
    fs::write(path, bytes).expect("embeddings");
}
fn seed_owner(root: &Path) {
    entity(root, "principal", "Principal", true);
    owner(root);
}

#[test]
fn resolve_smoke_applies_owner_and_structural_layers() {
    let temporary = TempDir::new();
    let segment = segment(temporary.path());
    seed_owner(temporary.path());
    entity(temporary.path(), "alice", "Alice", false);
    embeddings(
        &segment.join("mic_audio.npz"),
        &[1, 2],
        &[embedding(1.0, 0.0), embedding(0.0, 1.0)],
    );
    fs::write(segment.join("talents/speakers.json"), "[\"Alice\"]").expect("speakers");
    let outcome =
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve");
    let ResolveOutcome::Resolved(output) = outcome else {
        panic!("expected resolved");
    };
    assert_eq!(output.source.as_deref(), Some("mic_audio"));
    assert!(output.unmatched.is_empty());
    assert_eq!(
        output
            .labels
            .iter()
            .map(|label| label.speaker.as_deref())
            .collect::<Vec<_>>(),
        [Some("principal"), Some("alice")]
    );
    assert_eq!(output.metadata.candidate_evidence_gaps, None);
}

#[test]
fn ac7_unmatched_texts_follow_resolved_transcript_sentence_ids() {
    let temporary = TempDir::new();
    let segment = segment(temporary.path());
    seed_owner(temporary.path());
    embeddings(
        &segment.join("audio.npz"),
        &[5, 2],
        &[embedding(0.0, 1.0), embedding(0.0, 1.0)],
    );
    fs::write(segment.join("audio.jsonl"), "{\"schema\":1}\n{\"sentence_id\":5,\"text\":\"persisted five\"}\n{\"text\":\"positional two\"}\n").expect("transcript");
    let ResolveOutcome::Resolved(output) =
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve")
    else {
        panic!("expected resolved");
    };
    assert_eq!(output.unmatched, [5, 2]);
    assert_eq!(
        output.unmatched_texts.get(&5).map(String::as_str),
        Some("persisted five")
    );
    assert_eq!(
        output.unmatched_texts.get(&2).map(String::as_str),
        Some("positional two")
    );
}

#[test]
fn resolve_short_circuits_missing_owner_and_embeddings() {
    let temporary = TempDir::new();
    let _ = segment(temporary.path());
    assert_eq!(
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve"),
        ResolveOutcome::NoOwnerCentroid
    );
    seed_owner(temporary.path());
    assert_eq!(
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve"),
        ResolveOutcome::Empty { source: None }
    );
}

#[test]
fn resolve_zero_row_embeddings_are_empty_with_source() {
    let temporary = TempDir::new();
    let segment = segment(temporary.path());
    seed_owner(temporary.path());
    embeddings(&segment.join("mic_audio.npz"), &[], &[]);
    assert_eq!(
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve"),
        ResolveOutcome::Empty {
            source: Some("mic_audio".to_owned())
        }
    );
}

#[test]
fn resolve_corrupt_embeddings_are_an_empty_source_result() {
    let temporary = TempDir::new();
    let segment = segment(temporary.path());
    seed_owner(temporary.path());
    fs::write(segment.join("mic_audio.npz"), b"not an npz archive").expect("corrupt sidecar");
    assert_eq!(
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve"),
        ResolveOutcome::Empty {
            source: Some("mic_audio".to_owned())
        }
    );
}
