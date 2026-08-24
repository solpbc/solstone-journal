// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use solstone_core_npy::{parse_npy, write_npy};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Byte-for-byte file snapshot of a journal tree, used to prove mutation
/// refusals write nothing.
pub fn snapshot_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    snapshot_walk(root, root, &mut files);
    files
}

fn snapshot_walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            snapshot_walk(root, &path, files);
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("snapshot path is under root")
                .to_string_lossy()
                .into_owned();
            files.insert(rel, fs::read(&path).unwrap_or_default());
        }
    }
}

const DAYS: [&str; 31] = [
    "20260701", "20260702", "20260703", "20260704", "20260705", "20260706", "20260707", "20260708",
    "20260709", "20260710", "20260711", "20260712", "20260713", "20260714", "20260715", "20260716",
    "20260717", "20260718", "20260719", "20260720", "20260721", "20260722", "20260723", "20260724",
    "20260725", "20260726", "20260727", "20260728", "20260729", "20260730", "20260731",
];
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Temporary populated journal used by the frozen speakers read-surface corpus.
pub struct PopulatedJournal {
    root: PathBuf,
    pub entity_ids: BTreeMap<String, String>,
}

impl PopulatedJournal {
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for PopulatedJournal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Build the deterministic journal state captured by convey_speakers_corpus.json.
pub fn build_populated_journal() -> PopulatedJournal {
    let root = temporary_journal_root();
    write_json(
        &root.join("config/journal.json"),
        &json!({"setup": {"completed_at": 1_767_225_600}}),
    );

    let mut entity_ids = BTreeMap::new();
    for (name, id, extra) in [
        (
            "Ada Lovelace",
            "ada_lovelace",
            json!({"is_principal": true}),
        ),
        ("Grace Hopper", "grace_hopper", json!({})),
        ("Alan Turing", "alan_turing", json!({})),
        ("Katherine Johnson", "katherine_johnson", json!({})),
        ("Barbara Liskov", "barbara_liskov", json!({})),
        ("Edsger Dijkstra", "edsger_dijkstra", json!({})),
        ("Radia Perlman", "radia_perlman", json!({})),
        ("Leslie Lamport", "leslie_lamport", json!({})),
        ("Margaret Hamilton", "margaret_hamilton", json!({})),
        ("Frances Allen", "frances_allen", json!({})),
        ("Blocked Person", "blocked_person", json!({"blocked": true})),
    ] {
        let mut entity = json!({
            "id": id,
            "name": name,
            "type": "Person",
            "created_at": 1_767_225_600,
        });
        entity
            .as_object_mut()
            .expect("entity is an object")
            .extend(extra.as_object().expect("extra is an object").clone());
        write_json(&root.join(format!("entities/{id}/entity.json")), &entity);
        entity_ids.insert(name.to_owned(), id.to_owned());
    }

    for day in DAYS {
        fs::create_dir_all(root.join("chronicle").join(day)).expect("day directory creates");
    }

    write_full_segment(&root, "20260731", "field", "090000_300");
    write_malformed_segment(&root);
    write_no_npz_segment(&root);
    write_empty_labels_segment(&root);
    write_corrupt_labels_segment(&root);
    write_discovery_cache(&root);
    write_voiceprints(&root, "grace_hopper", &oracle_voiceprints()["grace_hopper"]);
    write_voiceprints(&root, "alan_turing", &oracle_voiceprints()["alan_turing"]);
    write_voiceprints(&root, "katherine_johnson", &[unit_vector(63)]);

    PopulatedJournal { root, entity_ids }
}

/// Read the six load-bearing Python-generated vectors retained for p25 parity.
pub fn oracle_voiceprints() -> BTreeMap<String, Vec<Vec<f32>>> {
    // Known-voice cards expose a pairwise p25 over these exact vectors; segment
    // embeddings may be synthetic, but these six cannot be approximated.
    serde_json::from_str(include_str!(
        "../../../../fixtures/populated_journal_voiceprints.json"
    ))
    .expect("voiceprint oracle fixture parses")
}

/// Read one fixed-schema embeddings member back from a test NPZ archive.
pub fn read_embeddings_npz(path: &Path) -> Vec<Vec<f32>> {
    let file = File::open(path).expect("NPZ opens");
    let mut archive = ZipArchive::new(file).expect("NPZ parses");
    let mut member = archive
        .by_name("embeddings.npy")
        .expect("embeddings member exists");
    let mut bytes = Vec::new();
    member
        .read_to_end(&mut bytes)
        .expect("embeddings member reads");
    let blob = parse_npy(&bytes).expect("embeddings NPY parses");
    assert_eq!(blob.descr, "<f4");
    assert!(!blob.fortran_order);
    assert_eq!(blob.shape.len(), 2);
    assert_eq!(blob.shape[1], 256);
    blob.payload
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("f32 chunk")))
        .collect::<Vec<_>>()
        .chunks_exact(256)
        .map(|row| row.to_vec())
        .collect()
}

fn temporary_journal_root() -> PathBuf {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "solstone-populated-speakers-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    fs::create_dir(&root).expect("temporary journal creates");
    root
}

fn segment_dir(root: &Path, day: &str, stream: &str, key: &str) -> PathBuf {
    let path = root.join("chronicle").join(day).join(stream).join(key);
    fs::create_dir_all(path.join("talents")).expect("segment talents directory creates");
    path
}

fn write_full_segment(root: &Path, day: &str, stream: &str, key: &str) {
    let segment = segment_dir(root, day, stream, key);
    write_transcript(
        &segment.join("mic_audio.jsonl"),
        "mic_audio",
        &[
            "Good morning.",
            "The index rebuild finished.",
            "We should ship it.",
        ],
        None,
    );
    write_embeddings_npz(&segment.join("mic_audio.npz"), 3, true, 0);
    fs::write(segment.join("mic_audio.flac"), [0_u8; 64]).expect("audio writes");
    fs::write(segment.join("mic_audio.xyz"), [0_u8; 8]).expect("unknown media writes");
    write_json(
        &segment.join("talents/speakers.json"),
        &json!(["Grace Hopper", "Alan Turing"]),
    );
    write_json(
        &segment.join("talents/speaker_labels.json"),
        &json!({"labels": [
            {"sentence_id": 1, "speaker": "grace_hopper", "confidence": "high", "method": "user_assigned"},
            {"sentence_id": 2, "speaker": "alan_turing", "confidence": "low", "method": "acoustic"},
        ]}),
    );
    write_json(
        &segment.join("talents/speaker_corrections.json"),
        &json!({"corrections": [{
            "sentence_id": 2,
            "original_speaker": "grace_hopper",
            "corrected_speaker": "alan_turing",
        }]}),
    );
}

fn write_malformed_segment(root: &Path) {
    let segment = segment_dir(root, "20260731", "desk", "140000_600");
    write_transcript(
        &segment.join("sys_audio.jsonl"),
        "sys_audio",
        &["First.", "Second.", "Third.", "Fourth."],
        Some(1),
    );
    write_embeddings_npz(&segment.join("sys_audio.npz"), 4, false, 10);
    fs::write(segment.join("sys_audio.flac"), [0_u8; 32]).expect("audio writes");
}

fn write_no_npz_segment(root: &Path) {
    let segment = segment_dir(root, "20260730", "field", "101500_120");
    write_transcript(
        &segment.join("mic_audio.jsonl"),
        "mic_audio",
        &["Only text here.", "No vectors."],
        None,
    );
    fs::write(segment.join("mic_audio.flac"), []).expect("empty audio writes");
}

fn write_empty_labels_segment(root: &Path) {
    let segment = segment_dir(root, "20260729", "field", "173000_240");
    write_transcript(
        &segment.join("mic_audio.jsonl"),
        "mic_audio",
        &["Alpha.", "Beta."],
        None,
    );
    write_embeddings_npz(&segment.join("mic_audio.npz"), 2, true, 20);
    fs::write(segment.join("mic_audio.flac"), [0_u8; 16]).expect("audio writes");
    fs::write(segment.join("talents/speaker_labels.json"), "{}").expect("labels write");
}

fn write_corrupt_labels_segment(root: &Path) {
    let segment = segment_dir(root, "20260728", "desk", "080000_180");
    write_transcript(
        &segment.join("mic_audio.jsonl"),
        "mic_audio",
        &["Gamma."],
        None,
    );
    write_embeddings_npz(&segment.join("mic_audio.npz"), 1, true, 30);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0_u8; 24]);
    fs::write(segment.join("mic_audio.png"), png).expect("image writes");
    fs::write(segment.join("talents/speaker_labels.json"), "{not json")
        .expect("corrupt labels write");
}

fn write_transcript(path: &Path, source: &str, sentences: &[&str], malformed_at: Option<usize>) {
    let mut lines = vec![
        serde_json::to_string(&json!({
            "raw": format!("{source}.flac"),
            "model": "medium.en",
        }))
        .expect("transcript metadata serializes"),
    ];
    for (index, text) in sentences.iter().enumerate() {
        if malformed_at == Some(index) {
            lines.push("{ this line is not json".to_owned());
            continue;
        }
        let seconds = 3_600 + index * 5;
        lines.push(
            serde_json::to_string(&json!({
                "start": format!("{:02}:{:02}:{:02}", seconds / 3_600, (seconds % 3_600) / 60, seconds % 60),
                "text": text,
            }))
            .expect("transcript sentence serializes"),
        );
    }
    fs::write(path, format!("{}\n", lines.join("\n"))).expect("transcript writes");
}

fn write_embeddings_npz(path: &Path, count: usize, durations: bool, offset: usize) {
    let vectors = (0..count)
        .flat_map(|index| unit_vector(offset + index))
        .collect::<Vec<_>>();
    let statement_ids = (1..=count)
        .flat_map(|id| i32::try_from(id).expect("statement id fits").to_le_bytes())
        .collect::<Vec<_>>();
    let mut members = vec![
        (
            "embeddings.npy",
            write_npy("<f4", &format!("({count}, 256)"), &f32_payload(&vectors)),
        ),
        (
            "statement_ids.npy",
            write_npy("<i4", &format!("({count},)"), &statement_ids),
        ),
    ];
    if durations {
        members.push((
            "durations_s.npy",
            write_npy(
                "<f4",
                &format!("({count},)"),
                &f32_payload(&vec![2.5; count]),
            ),
        ));
    }
    write_npz(path, &members);
}

fn write_voiceprints(root: &Path, entity_id: &str, embeddings: &[Vec<f32>]) {
    let metadata = embeddings
        .iter()
        .enumerate()
        .map(|(index, _)| {
            serde_json::to_string(&json!({
                "day": "20260731",
                "stream": "field",
                "segment_key": "090000_300",
                "source": "mic_audio",
                "sentence_id": index + 1,
                "added_at": 1_767_225_600 + index,
                "last_seen_ts": 1_767_225_600 + index,
                "method": "user_assigned",
            }))
            .expect("voiceprint metadata serializes")
        })
        .collect::<Vec<_>>();
    let vectors = embeddings.iter().flatten().copied().collect::<Vec<_>>();
    assert!(embeddings.iter().all(|row| row.len() == 256));
    let metadata_payload = unicode_vector_payload(&metadata);
    let width = metadata
        .iter()
        .map(|item| item.chars().count())
        .max()
        .unwrap_or(0);
    write_npz(
        &root.join(format!("entities/{entity_id}/voiceprints.npz")),
        &[
            (
                "embeddings.npy",
                write_npy(
                    "<f4",
                    &format!("({}, 256)", embeddings.len()),
                    &f32_payload(&vectors),
                ),
            ),
            (
                "metadata.npy",
                write_npy(
                    &format!("<U{width}"),
                    &format!("({},)", metadata.len()),
                    &metadata_payload,
                ),
            ),
        ],
    );
}

fn write_discovery_cache(root: &Path) {
    write_json(
        &root.join("awareness/discovery_clusters.json"),
        &json!({"clusters": {
            "1": [
                {"day": "20260731", "stream": "field", "segment_key": "090000_300", "source": "mic_audio", "sentence_id": 1},
                {"day": "20260731", "stream": "field", "segment_key": "090000_300", "source": "mic_audio", "sentence_id": 3},
            ],
            "2": [
                {"day": "20260731", "stream": "desk", "segment_key": "140000_600", "source": "sys_audio", "sentence_id": 1},
            ],
        }}),
    );
}

fn write_json(path: &Path, value: &Value) {
    let parent = path.parent().expect("file has a parent");
    fs::create_dir_all(parent).expect("parent directory creates");
    fs::write(path, serde_json::to_vec(value).expect("JSON serializes")).expect("JSON writes");
}

fn write_npz(path: &Path, members: &[(&str, Vec<u8>)]) {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in members {
        writer
            .start_file(*name, options)
            .expect("NPZ member starts");
        writer.write_all(bytes).expect("NPZ member writes");
    }
    let parent = path.parent().expect("NPZ has a parent");
    fs::create_dir_all(parent).expect("NPZ parent creates");
    fs::write(path, writer.finish().expect("NPZ finishes").into_inner()).expect("NPZ writes");
}

fn unit_vector(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0; 256];
    vector[index % 256] = 1.0;
    vector
}

fn f32_payload(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn unicode_vector_payload(values: &[String]) -> Vec<u8> {
    let width = values
        .iter()
        .map(|value| value.chars().count())
        .max()
        .unwrap_or(0);
    values
        .iter()
        .flat_map(|value| {
            let mut row = value
                .chars()
                .flat_map(|character| (character as u32).to_le_bytes())
                .collect::<Vec<_>>();
            row.resize(width * 4, 0);
            row
        })
        .collect()
}
