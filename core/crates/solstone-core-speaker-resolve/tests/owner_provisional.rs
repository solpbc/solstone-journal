// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_npy::write_npy;
use solstone_core_speaker_id::calibration::OWNER_THRESHOLD;
use solstone_core_speaker_resolve::owner_provisional::{
    OwnerTierOutcome, OwnerTierReason, resolve_owner_tier,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
const DAY: &str = "20260808";
const SEGMENT: &str = "120000_300";
const SOURCE: &str = "audio";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-resolve-owner-provisional-{}-{sequence}",
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

fn seed_principal(temporary: &TempDir) {
    let entity_dir = temporary.path().join("entities/principal");
    fs::create_dir_all(&entity_dir).expect("create principal directory");
    fs::write(
        entity_dir.join("entity.json"),
        json!({"id":"principal","name":"Principal","type":"Person","is_principal":true})
            .to_string(),
    )
    .expect("write principal identity");
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

fn unicode_npy(values: &[String]) -> Vec<u8> {
    let width = values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0);
    let mut payload = Vec::new();
    for value in values {
        for character in value.chars() {
            payload.extend_from_slice(&(character as u32).to_le_bytes());
        }
        for _ in value.chars().count()..width {
            payload.extend_from_slice(&0_u32.to_le_bytes());
        }
    }
    write_npy(
        &format!("<U{width}"),
        &format!("({},)", values.len()),
        &payload,
    )
}

fn archive(members: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer.start_file(name, options).expect("start member");
        writer.write_all(&bytes).expect("write member");
    }
    writer.finish().expect("finish archive").into_inner()
}

fn vector(first: f32, second: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = first;
    values[1] = second;
    values
}

fn write_voiceprints(root: &Path, metadata: &[Value]) {
    let rows = metadata.len();
    let embeddings = (0..rows).flat_map(|_| vector(1.0, 0.0)).collect::<Vec<_>>();
    let metadata = metadata.iter().map(Value::to_string).collect::<Vec<_>>();
    let bytes = archive(vec![
        (
            "embeddings.npy",
            write_npy("<f4", &format!("({rows}, 256)"), &f32_bytes(&embeddings)),
        ),
        ("metadata.npy", unicode_npy(&metadata)),
    ]);
    fs::write(root.join("entities/principal/voiceprints.npz"), bytes).expect("write voiceprints");
}

fn write_embeddings(root: &Path, stream: &str, ids: &[i32], direction: (f32, f32)) {
    let directory = root.join("chronicle").join(DAY).join(stream).join(SEGMENT);
    fs::create_dir_all(&directory).expect("create segment");
    let values = ids
        .iter()
        .flat_map(|_| vector(direction.0, direction.1))
        .collect::<Vec<_>>();
    let bytes = archive(vec![
        (
            "embeddings.npy",
            write_npy("<f4", &format!("({}, 256)", ids.len()), &f32_bytes(&values)),
        ),
        (
            "statement_ids.npy",
            write_npy("<i4", &format!("({},)", ids.len()), &i32_bytes(ids)),
        ),
    ]);
    fs::write(directory.join("audio.npz"), bytes).expect("write embeddings");
}

fn write_labels(root: &Path, stream: &str, labels: Value) {
    let directory = root
        .join("chronicle")
        .join(DAY)
        .join(stream)
        .join(SEGMENT)
        .join("talents");
    fs::create_dir_all(&directory).expect("create talents directory");
    fs::write(directory.join("speaker_labels.json"), labels.to_string()).expect("write labels");
}

fn write_overlap(root: &Path, stream: &str, contents: &str) {
    let path = root
        .join("chronicle")
        .join(DAY)
        .join(stream)
        .join(SEGMENT)
        .join("audio.jsonl");
    fs::write(path, contents).expect("write overlap header");
}

fn labels(method: &str, speaker: &str) -> Value {
    json!({"labels":(1..=5).map(|sentence_id| json!({"sentence_id":sentence_id,"speaker":speaker,"method":method})).collect::<Vec<_>>()})
}

fn rows(stream: Value) -> Vec<Value> {
    (1..=5)
        .map(|sentence_id| {
            json!({
                "day":DAY,
                "stream":stream,
                "segment_key":SEGMENT,
                "source":SOURCE,
                "sentence_id":sentence_id,
                "method":"structural_single_speaker",
                "unexpected":"discarded",
                "added_at":1,
            })
        })
        .collect()
}

fn seed_provisional_store(temporary: &TempDir, stream: &str) {
    seed_principal(temporary);
    write_voiceprints(temporary.path(), &rows(Value::String(stream.to_owned())));
    write_embeddings(temporary.path(), stream, &[1, 2, 3, 4, 5], (1.0, 0.0));
    write_labels(
        temporary.path(),
        stream,
        labels("user_assigned", "principal"),
    );
    write_overlap(temporary.path(), stream, "{}\n");
}

fn owner_archive(centroid: &[f32], threshold: f32, include_threshold: bool) -> Vec<u8> {
    let mut members = vec![
        (
            "centroid.npy",
            write_npy(
                "<f4",
                &format!("({},)", centroid.len()),
                &f32_bytes(centroid),
            ),
        ),
        ("cluster_size.npy", write_npy("<i4", "()", &i32_bytes(&[5]))),
    ];
    if include_threshold {
        members.push((
            "threshold.npy",
            write_npy("<f4", "()", &f32_bytes(&[threshold])),
        ));
    }
    archive(members)
}

fn owner_path(root: &Path) -> PathBuf {
    root.join("entities/principal/owner_centroid.npz")
}

/// R14: each call re-reads the current on-disk state rather than caching a tier.
#[test]
fn r1_existing_file_states_suppress_and_absence_allows_provisional() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");

    fs::write(
        owner_path(temporary.path()),
        owner_archive(&vector(3.0, 4.0), 0.55, true),
    )
    .expect("write confirmed centroid");
    let OwnerTierOutcome::Confirmed(confirmed) = resolve_owner_tier(temporary.path()).unwrap()
    else {
        panic!("valid confirmed centroid must win");
    };
    assert_eq!(confirmed.threshold, 0.55);

    fs::write(owner_path(temporary.path()), b"not an archive").expect("write unreadable centroid");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::ConfirmedUnreadable)
    );

    fs::write(
        owner_path(temporary.path()),
        owner_archive(&vector(1.0, 0.0), 0.55, false),
    )
    .expect("write incomplete centroid");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::ConfirmedIncomplete)
    );

    fs::write(
        owner_path(temporary.path()),
        owner_archive(&vector(0.0, 0.0), 0.55, true),
    )
    .expect("write zero centroid");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::ConfirmedZeroNorm)
    );

    fs::remove_file(owner_path(temporary.path())).expect("remove confirmed centroid");
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
    assert_eq!(OWNER_THRESHOLD, 0.43);
}

#[test]
fn r12_applies_row_and_embedding_floors_at_five() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");

    write_voiceprints(
        temporary.path(),
        &rows(Value::String("stream".to_owned()))[..4],
    );
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    write_voiceprints(temporary.path(), &rows(Value::String("stream".to_owned())));
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));

    write_embeddings(temporary.path(), "stream", &[1, 2, 3, 4], (1.0, 0.0));
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowEmbeddingFloor)
    );
}

#[test]
fn r7_uses_first_chronicle_label_not_voiceprint_method() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    let mut metadata = rows(Value::String("stream".to_owned()));
    for row in &mut metadata {
        row["method"] = json!("user_assigned");
    }
    write_voiceprints(temporary.path(), &metadata);
    write_labels(
        temporary.path(),
        "stream",
        json!({"labels":(1..=5).flat_map(|sentence_id| [
            json!({"sentence_id":sentence_id,"speaker":"principal","method":"acoustic"}),
            json!({"sentence_id":sentence_id,"speaker":"principal","method":"user_assigned"}),
        ]).collect::<Vec<_>>() }),
    );
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
}

#[test]
fn r7_skips_unparseable_label_sentence_ids_before_finding_a_match() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    write_labels(
        temporary.path(),
        "stream",
        json!({"labels":(1..=5).flat_map(|sentence_id| [
            json!({"sentence_id":"bad","speaker":"principal","method":"acoustic"}),
            json!({"sentence_id":sentence_id,"speaker":"principal","method":"user_assigned"}),
        ]).collect::<Vec<_>>() }),
    );
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
}

#[test]
fn r8_requires_label_speaker_to_match_principal() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    write_labels(temporary.path(), "stream", labels("user_assigned", "other"));
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
}

#[test]
fn r11_overlap_is_strict_and_fail_open() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");

    write_overlap(temporary.path(), "stream", "{\"overlap_fraction\":0.11}\n");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
    write_overlap(temporary.path(), "stream", "{\"overlap_fraction\":0.10}\n");
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));

    for contents in ["", "not-json\n", "{\"overlap_fraction\":\"bad\"}\n"] {
        write_overlap(temporary.path(), "stream", contents);
        assert!(matches!(
            resolve_owner_tier(temporary.path()).unwrap(),
            OwnerTierOutcome::Provisional(_)
        ));
    }
    fs::remove_file(
        temporary
            .path()
            .join("chronicle")
            .join(DAY)
            .join("stream")
            .join(SEGMENT)
            .join("audio.jsonl"),
    )
    .expect("remove overlap file");
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
    let overlap_path = temporary
        .path()
        .join("chronicle")
        .join(DAY)
        .join("stream")
        .join(SEGMENT)
        .join("audio.jsonl");
    fs::create_dir(&overlap_path).expect("make unreadable overlap path");
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
}

#[test]
fn r6_stream_resolution_requires_one_glob_match_and_never_falls_back() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    write_voiceprints(temporary.path(), &rows(Value::Null));
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
    for stream in [Value::String(String::new()), json!(7)] {
        write_voiceprints(temporary.path(), &rows(stream));
        assert!(matches!(
            resolve_owner_tier(temporary.path()).unwrap(),
            OwnerTierOutcome::Provisional(_)
        ));
    }

    let mut metadata = rows(Value::Null);
    for row in &mut metadata {
        row["segment_key"] = json!("missing_segment");
    }
    write_voiceprints(temporary.path(), &metadata);
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    write_embeddings(temporary.path(), "second", &[1, 2, 3, 4, 5], (1.0, 0.0));
    write_labels(
        temporary.path(),
        "second",
        labels("user_assigned", "principal"),
    );
    write_overlap(temporary.path(), "second", "{}\n");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    fs::remove_dir_all(temporary.path().join("chronicle").join(DAY).join("second"))
        .expect("remove second stream");
    write_voiceprints(temporary.path(), &rows(Value::String("missing".to_owned())));
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
}

#[test]
fn r5_dedupes_without_stream_and_uses_added_at_then_index() {
    let temporary = TempDir::new();
    seed_principal(&temporary);
    write_embeddings(temporary.path(), "present", &[1, 2, 3, 4, 5], (1.0, 0.0));
    write_labels(
        temporary.path(),
        "present",
        labels("user_assigned", "principal"),
    );
    write_overlap(temporary.path(), "present", "{}\n");
    let mut metadata = rows(Value::String("present".to_owned()));
    metadata.extend(
        rows(Value::String("missing".to_owned()))
            .into_iter()
            .map(|mut row| {
                row["added_at"] = json!(2);
                row
            }),
    );
    write_voiceprints(temporary.path(), &metadata);
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    let mut metadata = rows(Value::String("present".to_owned()));
    metadata.extend(rows(Value::String("missing".to_owned())));
    write_voiceprints(temporary.path(), &metadata);
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    let mut metadata = rows(Value::String("missing".to_owned()));
    metadata.extend(rows(Value::String("present".to_owned())));
    write_voiceprints(temporary.path(), &metadata);
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
}

#[test]
fn r3_r4_match_python_integer_coercion_and_added_at_fallback() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    let mut metadata = rows(Value::String("stream".to_owned()));
    metadata[0]["sentence_id"] = json!(true);
    metadata[2]["sentence_id"] = json!(3.9);
    metadata[2]["added_at"] = json!("not-an-int");
    write_voiceprints(temporary.path(), &metadata);
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));

    metadata[2]["sentence_id"] = json!("not-an-int");
    write_voiceprints(temporary.path(), &metadata);
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );

    metadata[2]["sentence_id"] = json!("3");
    metadata[4]["sentence_id"] = json!("5_0");
    write_embeddings(temporary.path(), "stream", &[1, 2, 3, 4, 50], (1.0, 0.0));
    write_labels(
        temporary.path(),
        "stream",
        json!({"labels":[
            {"sentence_id":true,"speaker":"principal","method":"user_assigned"},
            {"sentence_id":2,"speaker":"principal","method":"user_assigned"},
            {"sentence_id":3,"speaker":"principal","method":"user_assigned"},
            {"sentence_id":4,"speaker":"principal","method":"user_assigned"},
            {"sentence_id":50,"speaker":"principal","method":"user_assigned"}
        ]}),
    );
    write_voiceprints(temporary.path(), &metadata);
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));

    metadata[4]["sentence_id"] = json!("5__0");
    write_voiceprints(temporary.path(), &metadata);
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
}

#[test]
fn r9_rejects_nonmanual_user_prefixed_label_method() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    write_labels(
        temporary.path(),
        "stream",
        labels("user_bogus", "principal"),
    );
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::BelowRowFloor)
    );
}

#[test]
fn r13_zero_mean_returns_the_provisional_zero_norm_reason() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    write_embeddings(temporary.path(), "stream", &[1, 2, 3, 4, 5], (0.0, 0.0));
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::ProvisionalZeroNorm)
    );
}

#[test]
fn r2_distinguishes_absent_and_unreadable_voiceprints() {
    let temporary = TempDir::new();
    seed_principal(&temporary);
    fs::write(
        temporary.path().join("entities/principal/voiceprints.npz"),
        b"not an archive",
    )
    .expect("write corrupt voiceprints");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::VoiceprintsUnreadable)
    );
    fs::remove_file(temporary.path().join("entities/principal/voiceprints.npz"))
        .expect("remove corrupt voiceprints");
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::None(OwnerTierReason::VoiceprintsAbsent)
    );
}

#[test]
fn r10_extra_voiceprint_metadata_does_not_block_resolution() {
    let temporary = TempDir::new();
    seed_provisional_store(&temporary, "stream");
    assert!(matches!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::Provisional(_)
    ));
}

#[test]
fn reasons_are_complete_and_identity_invalid_is_terminal() {
    assert_eq!(
        OwnerTierReason::ALL,
        [
            OwnerTierReason::ConfirmedAbsent,
            OwnerTierReason::ConfirmedUnreadable,
            OwnerTierReason::ConfirmedIncomplete,
            OwnerTierReason::ConfirmedZeroNorm,
            OwnerTierReason::VoiceprintsAbsent,
            OwnerTierReason::VoiceprintsUnreadable,
            OwnerTierReason::BelowRowFloor,
            OwnerTierReason::BelowEmbeddingFloor,
            OwnerTierReason::ProvisionalZeroNorm,
        ]
    );

    let temporary = TempDir::new();
    assert_eq!(
        resolve_owner_tier(temporary.path()).unwrap(),
        OwnerTierOutcome::IdentityInvalid
    );
}
