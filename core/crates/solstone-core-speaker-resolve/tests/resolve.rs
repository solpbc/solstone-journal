// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use solstone_core_npy::write_npy;
use solstone_core_speaker_resolve::resolve::{ResolveOutcome, resolve};
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
    entity_typed(root, id, name, "Person", principal);
}
fn entity_typed(root: &Path, id: &str, name: &str, entity_type: &str, principal: bool) {
    entity_flags(root, id, name, Some(entity_type), principal, false);
}
fn entity_flags(
    root: &Path,
    id: &str,
    name: &str,
    entity_type: Option<&str>,
    principal: bool,
    blocked: bool,
) {
    let path = root.join("entities").join(id);
    fs::create_dir_all(&path).expect("entity");
    let mut value = json!({"id": id, "name": name, "is_principal": principal});
    let object = value.as_object_mut().expect("entity object");
    if let Some(entity_type) = entity_type {
        object.insert("type".to_owned(), Value::String(entity_type.to_owned()));
    }
    if blocked {
        object.insert("blocked".to_owned(), Value::Bool(true));
    }
    fs::write(path.join("entity.json"), value.to_string()).expect("identity");
}
fn owner(root: &Path) {
    write_owner_centroid(root, false);
}
fn owner_with_margin(root: &Path) {
    write_owner_centroid(root, true);
}
fn write_owner_centroid(root: &Path, with_margin: bool) {
    let centroid = embedding(1.0, 0.0);
    let mut members = vec![
        (
            "centroid.npy",
            write_npy("<f4", "(256,)", &f32_bytes(&centroid)),
        ),
        ("threshold.npy", write_npy("<f4", "()", &f32_bytes(&[0.43]))),
        ("cluster_size.npy", write_npy("<i4", "()", &i32_bytes(&[1]))),
    ];
    if with_margin {
        members.push(("margin.npy", write_npy("<f4", "()", &f32_bytes(&[0.05]))));
    }
    let bytes = archive(members);
    fs::write(root.join("entities/principal/owner_centroid.npz"), bytes).expect("owner centroid");
}
fn write_rival_voiceprint(root: &Path, entity_id: &str) {
    let encoder = EncoderIdentity {
        id: "test".to_owned(),
        sha256: "0".repeat(64),
        width: 256,
    };
    save_voiceprints_batch(
        root,
        entity_id,
        &[VoiceprintItem {
            embedding: embedding(1.0, 0.0),
            metadata: json!({"stream": "mic", "added_at": 1}),
        }],
        &encoder,
    )
    .expect("write rival voiceprint");
}
fn write_malformed_voiceprint(root: &Path, entity_id: &str) {
    fs::write(
        root.join("entities")
            .join(entity_id)
            .join("voiceprints.npz"),
        b"not-an-npz",
    )
    .expect("write malformed voiceprint");
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
fn seed_owner_with_margin(root: &Path) {
    entity(root, "principal", "Principal", true);
    owner_with_margin(root);
}
fn resolve_owner_statement(root: &Path) -> solstone_core_speaker_resolve::resolve::ResolveOutput {
    let segment = segment(root);
    embeddings(&segment.join("mic_audio.npz"), &[1], &[embedding(1.0, 0.0)]);
    let outcome = resolve(root, "20260808", "mic", "120000_300", true, 1).expect("resolve");
    let ResolveOutcome::Resolved(output) = outcome else {
        panic!("expected resolved");
    };
    *output
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
fn ac5_resolve_candidates_contain_only_admitted_person_names() {
    let temporary = TempDir::new();
    let segment = segment(temporary.path());
    seed_owner(temporary.path());
    entity(temporary.path(), "alice", "Alice", false);
    fs::create_dir_all(temporary.path().join("entities/tool")).expect("tool entity");
    fs::write(
        temporary.path().join("entities/tool/entity.json"),
        json!({"id": "tool", "name": "Terminal", "type": "Tool", "is_principal": false})
            .to_string(),
    )
    .expect("tool identity");
    embeddings(
        &segment.join("mic_audio.npz"),
        &[1, 2],
        &[embedding(1.0, 0.0), embedding(0.0, 1.0)],
    );
    let speakers_path = segment.join("talents/speakers.json");
    fs::write(&speakers_path, "[\"Alice\", \"Terminal\"]").expect("speakers");
    let speakers_before = fs::read(&speakers_path).expect("read speakers before resolve");
    let outcome =
        resolve(temporary.path(), "20260808", "mic", "120000_300", true, 1).expect("resolve");
    let ResolveOutcome::Resolved(output) = outcome else {
        panic!("expected resolved");
    };
    assert_eq!(
        fs::read(&speakers_path).expect("read speakers after resolve"),
        speakers_before,
        "resolve must not rewrite talents/speakers.json"
    );
    assert_eq!(output.candidates, ["Alice"]);
    assert!(
        !output
            .labels
            .iter()
            .any(|label| label.speaker.as_deref() == Some("tool"))
    );
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

#[test]
fn ac8_person_rival_declines_owner_margin_and_non_persons_do_not() {
    let person = TempDir::new();
    seed_owner_with_margin(person.path());
    entity_typed(person.path(), "rival", "Rival", "Person", false);
    write_rival_voiceprint(person.path(), "rival");
    let output = resolve_owner_statement(person.path());
    assert_eq!(output.labels[0].owner_margin_declined, Some(true));
    assert_ne!(output.labels[0].speaker.as_deref(), Some("principal"));

    for (id, name, entity_type, blocked) in [
        ("tool", "Terminal", Some("Tool"), false),
        ("blocked", "Blocked", Some("Person"), true),
        ("untyped", "Untyped", None, false),
    ] {
        let temporary = TempDir::new();
        seed_owner_with_margin(temporary.path());
        entity_flags(temporary.path(), id, name, entity_type, false, blocked);
        write_rival_voiceprint(temporary.path(), id);
        let output = resolve_owner_statement(temporary.path());
        assert_eq!(
            output.labels[0].speaker.as_deref(),
            Some("principal"),
            "{id} must not decline the owner claim"
        );
        assert_eq!(output.labels[0].owner_margin_declined, None);
        assert!(!output.metadata.voiceprint_versions.contains_key(id));
        assert!(!output.candidates.iter().any(|candidate| candidate == name));
        assert!(
            output
                .metadata
                .voiceprint_gaps
                .as_ref()
                .is_none_or(|gaps| !gaps.iter().any(|gap| gap.entity_id == id))
        );
    }
}

#[test]
fn ac9_malformed_person_rival_still_loads_and_non_persons_do_not() {
    let person = TempDir::new();
    seed_owner_with_margin(person.path());
    entity_typed(person.path(), "rival", "Rival", "Person", false);
    write_malformed_voiceprint(person.path(), "rival");
    let output = resolve_owner_statement(person.path());
    let gaps = output
        .metadata
        .voiceprint_gaps
        .expect("malformed Person rival is loaded");
    assert!(
        gaps.iter()
            .any(|gap| gap.entity_id == "rival" && gap.reason == "unreadable")
    );

    for (id, name, entity_type, blocked) in [
        ("tool", "Terminal", Some("Tool"), false),
        ("blocked", "Blocked", Some("Person"), true),
        ("untyped", "Untyped", None, false),
    ] {
        let temporary = TempDir::new();
        seed_owner_with_margin(temporary.path());
        entity_flags(temporary.path(), id, name, entity_type, false, blocked);
        write_malformed_voiceprint(temporary.path(), id);
        let output = resolve_owner_statement(temporary.path());
        assert_eq!(output.labels[0].speaker.as_deref(), Some("principal"));
        assert!(
            output
                .metadata
                .voiceprint_gaps
                .as_ref()
                .is_none_or(|gaps| !gaps.iter().any(|gap| gap.entity_id == id)),
            "{id} must not be loaded for a voiceprint gap"
        );
    }
}
