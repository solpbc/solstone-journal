// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_entity::EncoderIdentity;
use solstone_core_npy::write_npy;
use solstone_core_speaker_id::calibration::OWNER_THRESHOLD;
use solstone_core_speaker_resolve::owner_contamination_screen::{
    ContaminationProbe, ContaminationScreen, OwnerContaminationScreenError, classify_tier, decide,
    screen_owner_contamination,
};
use solstone_core_speaker_resolve::owner_provisional::{OwnerTierOutcome, OwnerTierReason};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const DAY: &str = "20260808";
const STREAM: &str = "main";
const SEGMENT: &str = "120000_1";
const SOURCE: &str = "audio";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-owner-contamination-screen-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create journal");
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

fn encoder(width: usize) -> EncoderIdentity {
    EncoderIdentity {
        id: "test".to_owned(),
        sha256: "a".repeat(64),
        width,
    }
}

fn vector(first: f32, second: f32) -> Vec<f32> {
    let mut values = vec![0.0; 256];
    values[0] = first;
    values[1] = second;
    values
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

fn seed_principal(root: &Path) {
    let directory = root.join("entities/principal");
    fs::create_dir_all(&directory).expect("create entity");
    fs::write(
        directory.join("entity.json"),
        json!({"id":"principal","name":"Synthetic","type":"Person","is_principal":true})
            .to_string(),
    )
    .expect("write principal");
}

fn write_owner(root: &Path, centroid: &[f32], threshold: f32) {
    fs::write(
        root.join("entities/principal/owner_centroid.npz"),
        archive(vec![
            (
                "centroid.npy",
                write_npy("<f4", "(256,)", &f32_bytes(centroid)),
            ),
            (
                "threshold.npy",
                write_npy("<f4", "()", &f32_bytes(&[threshold])),
            ),
            ("cluster_size.npy", write_npy("<i4", "()", &i32_bytes(&[5]))),
        ]),
    )
    .expect("write owner centroid");
}

fn write_embeddings(root: &Path, rows: &[(i32, Vec<f32>)]) {
    let directory = root.join("chronicle").join(DAY).join(STREAM).join(SEGMENT);
    fs::create_dir_all(&directory).expect("create segment");
    let embeddings = rows
        .iter()
        .flat_map(|(_, values)| values.iter().copied())
        .collect::<Vec<_>>();
    let ids = rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    fs::write(
        directory.join("audio.npz"),
        archive(vec![
            (
                "embeddings.npy",
                write_npy(
                    "<f4",
                    &format!("({}, 256)", rows.len()),
                    &f32_bytes(&embeddings),
                ),
            ),
            (
                "statement_ids.npy",
                write_npy("<i4", &format!("({},)", ids.len()), &i32_bytes(&ids)),
            ),
        ]),
    )
    .expect("write probe embeddings");
}

fn probe(sentence_id: i64) -> ContaminationProbe {
    ContaminationProbe {
        day: DAY.to_owned(),
        stream: STREAM.to_owned(),
        segment_key: SEGMENT.to_owned(),
        source: SOURCE.to_owned(),
        sentence_id,
    }
}

fn seed_confirmed(root: &Path, threshold: f32) {
    seed_principal(root);
    write_owner(root, &vector(1.0, 0.0), threshold);
}

fn seed_provisional(root: &Path) {
    seed_principal(root);
    let metadata = (1..=5)
        .map(|sentence_id| {
            json!({"day":DAY,"stream":STREAM,"segment_key":SEGMENT,"source":SOURCE,"sentence_id":sentence_id,"added_at":1}).to_string()
        })
        .collect::<Vec<_>>();
    let embeddings = (0..5).flat_map(|_| vector(1.0, 0.0)).collect::<Vec<_>>();
    fs::write(
        root.join("entities/principal/voiceprints.npz"),
        archive(vec![
            (
                "embeddings.npy",
                write_npy("<f4", "(5, 256)", &f32_bytes(&embeddings)),
            ),
            ("metadata.npy", unicode_npy(&metadata)),
        ]),
    )
    .expect("write voiceprints");
    write_embeddings(
        root,
        &(1..=5).map(|id| (id, vector(1.0, 0.0))).collect::<Vec<_>>(),
    );
    let labels = json!({"labels":(1..=5).map(|sentence_id| json!({"sentence_id":sentence_id,"speaker":"principal","method":"user_assigned"})).collect::<Vec<_>>()});
    let directory = root.join("chronicle").join(DAY).join(STREAM).join(SEGMENT);
    fs::create_dir_all(directory.join("talents")).expect("create talents");
    fs::write(
        directory.join("talents/speaker_labels.json"),
        labels.to_string(),
    )
    .expect("write labels");
    fs::write(directory.join("audio.jsonl"), "{}\n").expect("write overlap");
}

#[test]
fn ac1_all_tier_reasons_are_indeterminate_with_stable_strings() {
    let expected = [
        "confirmed_absent",
        "confirmed_unreadable",
        "confirmed_incomplete",
        "confirmed_zero_norm",
        "voiceprints_absent",
        "voiceprints_unreadable",
        "below_row_floor",
        "below_embedding_floor",
        "provisional_zero_norm",
    ];
    for (reason, expected) in OwnerTierReason::ALL.into_iter().zip(expected) {
        assert_eq!(
            classify_tier(OwnerTierOutcome::None(reason)),
            Err(ContaminationScreen::Indeterminate {
                reason: expected.to_owned()
            })
        );
    }
    assert_eq!(
        classify_tier(OwnerTierOutcome::IdentityInvalid),
        Err(ContaminationScreen::Indeterminate {
            reason: "speaker_owner_identity_invalid".to_owned()
        })
    );
}

#[test]
fn confirmed_threshold_normalization_and_wire_shape_are_exact() {
    let temporary = TempDir::new();
    seed_confirmed(temporary.path(), 0.55);
    write_embeddings(
        temporary.path(),
        &[
            (1, vector(1.0, 0.0)),
            (3, vector(0.0, 1.0)),
            (4, vec![0.0; 256]),
        ],
    );
    let exact = screen_owner_contamination(temporary.path(), &probe(1), &encoder(256)).unwrap();
    assert_eq!(
        exact,
        ContaminationScreen::Contaminated {
            basis: "confirmed".to_owned(),
            similarity: 1.0,
            threshold: 0.55,
        }
    );
    assert!(matches!(
        screen_owner_contamination(temporary.path(), &probe(3), &encoder(256)).unwrap(),
        ContaminationScreen::Clear { .. }
    ));
    assert_eq!(
        screen_owner_contamination(temporary.path(), &probe(4), &encoder(256)).unwrap(),
        ContaminationScreen::Indeterminate {
            reason: "probe_zero_norm".to_owned()
        }
    );
    let value = serde_json::to_value(exact).unwrap();
    assert_eq!(value["status"], "contaminated");
    let mut without_status = value.as_object().unwrap().clone();
    without_status.remove("status");
    assert!(serde_json::from_value::<ContaminationScreen>(Value::Object(without_status)).is_err());
}

#[test]
fn ac3_similarity_exactly_at_threshold_is_contaminated_on_both_tiers() {
    assert_eq!(
        decide("confirmed".to_owned(), 0.55, 0.55),
        ContaminationScreen::Contaminated {
            basis: "confirmed".to_owned(),
            similarity: 0.55,
            threshold: 0.55,
        }
    );
    assert_eq!(
        decide("provisional".to_owned(), OWNER_THRESHOLD, OWNER_THRESHOLD,),
        ContaminationScreen::Contaminated {
            basis: "provisional".to_owned(),
            similarity: OWNER_THRESHOLD,
            threshold: OWNER_THRESHOLD,
        }
    );
}

#[test]
fn ac5_probe_scaled_by_two_yields_identical_similarity() {
    let temporary = TempDir::new();
    seed_confirmed(temporary.path(), 0.55);
    write_embeddings(
        temporary.path(),
        &[(1, vector(1.0, 0.0)), (2, vector(2.0, 0.0))],
    );
    let first = screen_owner_contamination(temporary.path(), &probe(1), &encoder(256)).unwrap();
    let second = screen_owner_contamination(temporary.path(), &probe(2), &encoder(256)).unwrap();
    let ContaminationScreen::Contaminated {
        similarity: first_similarity,
        ..
    } = first
    else {
        panic!("first probe is contaminated")
    };
    let ContaminationScreen::Contaminated {
        similarity: second_similarity,
        ..
    } = second
    else {
        panic!("scaled probe is contaminated")
    };
    assert_eq!(first_similarity, second_similarity);
}

#[test]
fn provisional_scoring_uses_constant_threshold_and_real_resolver() {
    let temporary = TempDir::new();
    seed_provisional(temporary.path());
    write_embeddings(
        temporary.path(),
        &[
            (1, vector(1.0, 0.0)),
            (2, vector(1.0, 0.0)),
            (3, vector(1.0, 0.0)),
            (4, vector(1.0, 0.0)),
            (5, vector(1.0, 0.0)),
            (10, vector(1.0, 0.0)),
            (11, vector(0.0, 1.0)),
        ],
    );
    let contaminated =
        screen_owner_contamination(temporary.path(), &probe(10), &encoder(256)).unwrap();
    assert_eq!(
        contaminated,
        ContaminationScreen::Contaminated {
            basis: "provisional".to_owned(),
            similarity: 1.0,
            threshold: OWNER_THRESHOLD,
        }
    );
    assert!(matches!(
        screen_owner_contamination(temporary.path(), &probe(11), &encoder(256)).unwrap(),
        ContaminationScreen::Clear { basis, threshold, .. } if basis == "provisional" && threshold == OWNER_THRESHOLD
    ));
}

#[test]
fn absent_and_invalid_probes_are_distinct_from_scored_results() {
    let temporary = TempDir::new();
    seed_confirmed(temporary.path(), 0.55);
    write_embeddings(
        temporary.path(),
        &[(1, vector(1.0, 0.0)), (2, vector(f32::NAN, 0.0))],
    );
    assert_eq!(
        screen_owner_contamination(temporary.path(), &probe(99), &encoder(256)).unwrap(),
        ContaminationScreen::Indeterminate {
            reason: "probe_not_found".to_owned()
        }
    );
    assert!(matches!(
        screen_owner_contamination(temporary.path(), &probe(1), &encoder(255)),
        Err(OwnerContaminationScreenError::InvalidEmbeddingWidth)
    ));
    assert!(matches!(
        screen_owner_contamination(temporary.path(), &probe(2), &encoder(256)),
        Err(OwnerContaminationScreenError::NonFiniteEmbedding)
    ));
    let mut invalid = encoder(256);
    invalid.sha256 = "not-a-sha".to_owned();
    assert!(matches!(
        screen_owner_contamination(temporary.path(), &probe(1), &invalid),
        Err(OwnerContaminationScreenError::InvalidEncoderIdentity)
    ));
}
