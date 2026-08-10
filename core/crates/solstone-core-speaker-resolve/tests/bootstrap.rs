// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::{EncoderIdentity, VoiceprintItem, save_voiceprints_batch};
use solstone_core_npy::write_npy;
use solstone_core_speaker_resolve::bootstrap::{
    BootstrapOutcome, BootstrapRequest, BootstrapStats, MergeNamesOutcome, SeedFromImportsOutcome,
    bootstrap_voiceprints, merge_names, seed_from_imports,
};
use solstone_core_speaker_resolve::owner_centroid::{
    OwnerCentroidWriteInput, write_owner_centroid,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-bootstrap-{}-{}",
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

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn encoder() -> EncoderIdentity {
    EncoderIdentity {
        id: "test".to_owned(),
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

fn entity(root: &Path, id: &str, name: &str, kind: &str, principal: bool) {
    let path = root.join("entities").join(id);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("entity.json"),
        json!({"id": id, "name": name, "type": kind, "is_principal": principal}).to_string(),
    )
    .unwrap();
}

fn write_owner(root: &Path) {
    write_owner_centroid(
        root,
        "principal",
        &OwnerCentroidWriteInput {
            centroid: vector(1.0, 0.0),
            cluster_size: 1,
            timestamp: "2026-08-08T00:00:00Z".to_owned(),
            evidence_tier: "test".to_owned(),
        },
    )
    .unwrap();
}

fn flat(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn ints(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn segment(root: &Path, key: &str, speaker: &str) {
    let path = root.join("chronicle/20260808/mic").join(key);
    fs::create_dir_all(path.join("talents")).unwrap();
    fs::write(
        path.join("talents/speakers.json"),
        json!([speaker]).to_string(),
    )
    .unwrap();
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("embeddings.npy", options).unwrap();
    archive
        .write_all(&write_npy("<f4", "(1, 256)", &flat(&vector(0.0, 1.0))))
        .unwrap();
    archive.start_file("statement_ids.npy", options).unwrap();
    archive
        .write_all(&write_npy("<i4", "(1,)", &ints(&[7])))
        .unwrap();
    fs::write(
        path.join("audio.npz"),
        archive.finish().unwrap().into_inner(),
    )
    .unwrap();
}

fn import_segment(root: &Path, stream: &str, key: &str, speaker: &str) {
    let path = root.join("chronicle/20260808").join(stream).join(key);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        path.join("conversation_transcript.jsonl"),
        format!(
            "{{\"imported\":{{\"id\":\"fixture\"}}}}\n{{\"start\":\"12:00:00\",\"speaker\":\"{speaker}\"}}\n"
        ),
    )
    .unwrap();
    fs::write(
        path.join("audio.jsonl"),
        "{\"raw\":\"fixture\"}\n{\"start\":\"12:00:00\",\"text\":\"hello\"}\n",
    )
    .unwrap();
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    archive.start_file("embeddings.npy", options).unwrap();
    archive
        .write_all(&write_npy("<f4", "(1, 256)", &flat(&vector(0.0, 1.0))))
        .unwrap();
    archive.start_file("statement_ids.npy", options).unwrap();
    archive
        .write_all(&write_npy("<i4", "(1,)", &ints(&[1])))
        .unwrap();
    fs::write(
        path.join("audio.npz"),
        archive.finish().unwrap().into_inner(),
    )
    .unwrap();
}

fn request(root: &Path) -> BootstrapRequest {
    BootstrapRequest {
        journal_root: root.to_path_buf(),
        encoder: encoder(),
        added_at: 1,
        dry_run: false,
    }
}

fn bootstrap_stats_json(stats: &BootstrapStats) -> Value {
    json!({
        "segments_scanned":stats.segments_scanned,
        "single_speaker_segments":stats.single_speaker_segments,
        "speakers_found":stats.speakers_found,
        "entities_created":stats.entities_created,
        "embeddings_saved":stats.embeddings_saved,
        "embeddings_skipped_owner":stats.embeddings_skipped_owner,
        "embeddings_skipped_duplicate":stats.embeddings_skipped_duplicate,
        "speakers_unmatched":stats.speakers_unmatched,
        "errors":stats.errors,
    })
}

#[test]
fn ac4_ac22_bootstrap_guards_non_person_and_continues_after_entity_write_failure() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(temporary.path(), "good", "Good Person", "Person", false);
    entity(temporary.path(), "bad", "Bad Person", "Person", false);
    entity(temporary.path(), "tool", "Tool Speaker", "Tool", false);
    write_owner(temporary.path());
    segment(temporary.path(), "120000_300", "Good Person");
    segment(temporary.path(), "120500_300", "Bad Person");
    segment(temporary.path(), "121000_300", "Tool Speaker");
    let bad_path = temporary.path().join("entities/bad/voiceprints.npz");
    fs::write(&bad_path, b"not-an-npz").unwrap();

    let BootstrapOutcome::Completed(stats) =
        bootstrap_voiceprints(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };
    assert_eq!(stats.embeddings_saved, 1);
    assert!(
        stats
            .errors
            .iter()
            .any(|error| error.contains("Failed to save for Bad Person"))
    );
    assert!(
        stats
            .errors
            .iter()
            .any(|error| error.contains("Skipped non-Person entity tool"))
    );
    assert!(
        temporary
            .path()
            .join("entities/good/voiceprints.npz")
            .is_file()
    );
    assert_eq!(fs::read(&bad_path).unwrap(), b"not-an-npz");
    assert!(
        !temporary
            .path()
            .join("entities/tool/voiceprints.npz")
            .exists()
    );
}

#[test]
fn ac23_bootstrap_voiceprints_output_is_deterministic_against_a_literal() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(temporary.path(), "good", "Good Person", "Person", false);
    write_owner(temporary.path());
    segment(temporary.path(), "120000_300", "Good Person");
    let mut request = request(temporary.path());
    request.dry_run = true;

    let BootstrapOutcome::Completed(first) = bootstrap_voiceprints(&request).unwrap() else {
        panic!("owner centroid is present");
    };
    let BootstrapOutcome::Completed(second) = bootstrap_voiceprints(&request).unwrap() else {
        panic!("owner centroid is present");
    };
    let expected = json!({
        "segments_scanned":1,
        "single_speaker_segments":1,
        "speakers_found":{"Good Person":1},
        "entities_created":0,
        "embeddings_saved":1,
        "embeddings_skipped_owner":0,
        "embeddings_skipped_duplicate":0,
        "speakers_unmatched":[],
        "errors":[],
    });
    assert_eq!(bootstrap_stats_json(&first), expected);
    assert_eq!(bootstrap_stats_json(&second), expected);
}

#[test]
fn ac22_seed_from_imports_continues_after_one_entity_write_failure() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(temporary.path(), "good", "Good Person", "Person", false);
    entity(temporary.path(), "bad", "Bad Person", "Person", false);
    entity(temporary.path(), "chat", "Chat Person", "Person", false);
    write_owner(temporary.path());
    import_segment(
        temporary.path(),
        "import.granola",
        "120000_300",
        "Good Person",
    );
    import_segment(
        temporary.path(),
        "import.granola",
        "120500_300",
        "Bad Person",
    );
    import_segment(
        temporary.path(),
        "import.claude",
        "121000_300",
        "Chat Person",
    );
    let bad_path = temporary.path().join("entities/bad/voiceprints.npz");
    fs::write(&bad_path, b"not-an-npz").unwrap();

    let SeedFromImportsOutcome::Completed(stats) =
        seed_from_imports(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };
    assert_eq!(stats.segments_scanned, 2);
    assert_eq!(stats.segments_with_speakers, 2);
    assert_eq!(stats.embeddings_saved, 1);
    assert!(
        stats
            .errors
            .iter()
            .any(|error| error.contains("Failed to save for bad"))
    );
    assert!(
        temporary
            .path()
            .join("entities/good/voiceprints.npz")
            .is_file()
    );
    assert_eq!(fs::read(&bad_path).unwrap(), b"not-an-npz");
    assert!(
        !temporary
            .path()
            .join("entities/chat/voiceprints.npz")
            .exists()
    );
}

#[test]
fn seed_from_imports_collapses_unresolved_speakers_without_creating_entities() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    write_owner(temporary.path());
    import_segment(
        temporary.path(),
        "import.granola",
        "120000_300",
        "Unknown Person",
    );
    import_segment(
        temporary.path(),
        "import.granola",
        "120500_300",
        "Unknown Person",
    );

    let SeedFromImportsOutcome::Completed(stats) =
        seed_from_imports(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };
    assert_eq!(stats.speakers_unmatched, ["Unknown Person"]);
    assert_eq!(stats.embeddings_saved, 0);
    assert!(!temporary.path().join("entities/unknown_person").exists());
}

#[test]
fn ac19_bootstrap_keeps_ambiguous_single_speaker_names_unmatched() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(temporary.path(), "alex-one", "Alex One", "Person", false);
    entity(temporary.path(), "alex-two", "Alex Two", "Person", false);
    write_owner(temporary.path());
    segment(temporary.path(), "120000_300", "Alex");

    let BootstrapOutcome::Completed(stats) =
        bootstrap_voiceprints(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };
    assert_eq!(stats.speakers_unmatched, ["Alex"]);
    assert_eq!(stats.embeddings_saved, 0);
}

#[test]
fn bootstrap_records_a_genuine_no_match_without_creating_a_person() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    write_owner(temporary.path());
    segment(temporary.path(), "120000_300", "New Person");

    let BootstrapOutcome::Completed(stats) =
        bootstrap_voiceprints(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };
    assert_eq!(stats.entities_created, 0);
    assert_eq!(stats.speakers_unmatched, ["New Person"]);
    assert!(!stats.speakers_found.contains_key("New Person"));
    assert!(!temporary.path().join("entities/new_person").exists());
}

#[test]
fn bootstrap_does_not_adopt_a_written_id_slug_collision() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(
        temporary.path(),
        "new_person",
        "Someone Else",
        "Person",
        false,
    );
    write_owner(temporary.path());
    save_voiceprints_batch(
        temporary.path(),
        "new_person",
        &[VoiceprintItem {
            embedding: vector(0.0, 1.0),
            metadata: json!({
                "day": "20260808",
                "segment_key": "prior",
                "source": "audio",
                "sentence_id": 1,
            }),
        }],
        &encoder(),
    )
    .unwrap();
    let voiceprints = temporary.path().join("entities/new_person/voiceprints.npz");
    let before = fs::read(&voiceprints).unwrap();
    segment(temporary.path(), "120000_300", "New Person");

    let BootstrapOutcome::Completed(stats) =
        bootstrap_voiceprints(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };

    assert!(
        fs::read(voiceprints).unwrap() == before,
        "incumbent voiceprint archive changed after NoMatch"
    );
    assert_eq!(stats.speakers_unmatched, ["New Person"]);
    assert_eq!(stats.embeddings_saved, 0);
}

#[test]
fn bootstrap_dry_run_leaves_collision_and_genuine_no_match_unchanged() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(
        temporary.path(),
        "new_person",
        "Someone Else",
        "Person",
        false,
    );
    write_owner(temporary.path());
    save_voiceprints_batch(
        temporary.path(),
        "new_person",
        &[VoiceprintItem {
            embedding: vector(0.0, 1.0),
            metadata: json!({"day":"20260808","segment_key":"prior","source":"audio","sentence_id":1}),
        }],
        &encoder(),
    )
    .unwrap();
    let voiceprints = temporary.path().join("entities/new_person/voiceprints.npz");
    let before = fs::read(&voiceprints).unwrap();
    segment(temporary.path(), "120000_300", "New Person");
    segment(temporary.path(), "120500_300", "Unknown Person");
    let mut dry_run = request(temporary.path());
    dry_run.dry_run = true;

    let BootstrapOutcome::Completed(stats) = bootstrap_voiceprints(&dry_run).unwrap() else {
        panic!("owner centroid is present");
    };

    assert_eq!(stats.entities_created, 0);
    assert_eq!(stats.embeddings_saved, 0);
    assert_eq!(stats.speakers_unmatched, ["New Person", "Unknown Person"]);
    assert!(!temporary.path().join("entities/unknown_person").exists());
    assert_eq!(fs::read(voiceprints).unwrap(), before);
}

#[test]
fn seed_from_imports_does_not_adopt_a_written_id_slug_collision() {
    let temporary = Temp::new();
    entity(temporary.path(), "principal", "Principal", "Person", true);
    entity(
        temporary.path(),
        "new_person",
        "Someone Else",
        "Person",
        false,
    );
    write_owner(temporary.path());
    save_voiceprints_batch(
        temporary.path(),
        "new_person",
        &[VoiceprintItem {
            embedding: vector(0.0, 1.0),
            metadata: json!({"day":"20260808","segment_key":"prior","source":"audio","sentence_id":1}),
        }],
        &encoder(),
    )
    .unwrap();
    let voiceprints = temporary.path().join("entities/new_person/voiceprints.npz");
    let before = fs::read(&voiceprints).unwrap();
    import_segment(
        temporary.path(),
        "import.granola",
        "120000_300",
        "New Person",
    );

    let SeedFromImportsOutcome::Completed(stats) =
        seed_from_imports(&request(temporary.path())).unwrap()
    else {
        panic!("owner centroid is present");
    };

    assert_eq!(stats.speakers_unmatched, ["New Person"]);
    assert_eq!(stats.embeddings_saved, 0);
    assert_eq!(fs::read(voiceprints).unwrap(), before);
}

#[test]
fn ac18_merge_names_returns_ambiguity_for_each_side_and_ready_ids_without_merging() {
    let temporary = Temp::new();
    entity(temporary.path(), "alex-one", "Alex One", "Person", false);
    entity(temporary.path(), "alex-two", "Alex Two", "Person", false);
    entity(temporary.path(), "canon-one", "Canon One", "Person", false);
    entity(temporary.path(), "canon-two", "Canon Two", "Person", false);
    entity(temporary.path(), "alias", "Alias Person", "Person", false);
    entity(
        temporary.path(),
        "canonical",
        "Canonical Person",
        "Person",
        false,
    );

    let alias = merge_names(temporary.path(), "Alex", "Canonical Person").unwrap();
    assert!(matches!(
        alias,
        MergeNamesOutcome::Ambiguous { field: "alias", ref candidates, .. }
            if candidates.iter().map(|candidate| candidate.id.as_str()).eq(["alex-one", "alex-two"])
    ));
    let canonical = merge_names(temporary.path(), "Alias Person", "Canon").unwrap();
    assert!(matches!(
        canonical,
        MergeNamesOutcome::Ambiguous { field: "canonical", ref candidates, .. }
            if candidates.iter().map(|candidate| candidate.id.as_str()).eq(["canon-one", "canon-two"])
    ));
    assert_eq!(
        merge_names(temporary.path(), "Alias Person", "Canonical Person").unwrap(),
        MergeNamesOutcome::Ready {
            alias_entity_id: "alias".to_owned(),
            canonical_entity_id: "canonical".to_owned(),
        }
    );
}

#[test]
fn merge_names_does_not_adopt_a_written_id_slug_collision() {
    let temporary = Temp::new();
    entity(
        temporary.path(),
        "new_person",
        "Someone Else",
        "Person",
        false,
    );
    entity(
        temporary.path(),
        "canonical",
        "Canonical Person",
        "Person",
        false,
    );

    assert_eq!(
        merge_names(temporary.path(), "New Person", "Canonical Person").unwrap(),
        MergeNamesOutcome::AliasNotFound
    );
}
