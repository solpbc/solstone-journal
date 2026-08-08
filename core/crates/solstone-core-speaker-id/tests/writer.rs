// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use solstone_core_speaker_id::transcript::{SentenceIdSource, read_transcript_rows};
use solstone_core_speaker_id::writer::{SpeakerTranscriptWriteError, write_request};
use zip::ZipArchive;

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "solstone-speaker-id-writer-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create test directory");
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

fn payload(rows: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(rows * 256 * 4);
    for value in (0..rows * 256).map(|value| value as f32 / 10.0) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn request(
    directory: &TempDir,
    statements: Value,
    statement_ids: Value,
    durations_s: Value,
    rows: usize,
) -> Value {
    let payload_path = directory.path().join("payload.f32");
    fs::write(&payload_path, payload(rows)).expect("write payload");
    json!({
        "schema": "solstone-speaker-transcript-write-request-v1",
        "output": {
            "jsonl_path": directory.path().join("segment.jsonl"),
            "npz_path": directory.path().join("segment.npz"),
            "redo": false,
        },
        "base_time_us_of_day": 86_399_500_000_u64,
        "source": "mic_audio",
        "statements": statements,
        "header": {"raw": "audio.wav", "model": "model", "device": "cpu", "compute_type": "int8"},
        "embeddings": {
            "payload_path": payload_path,
            "payload_format": "raw-f32le-row-major-v1",
            "dtype": "float32-le",
            "shape": [rows, 256],
            "byte_count": rows * 256 * 4,
            "statement_ids": statement_ids,
            "durations_s": durations_s,
            "encoder": "caller-supplied-encoder",
        }
    })
}

fn write(
    value: Value,
) -> Result<solstone_core_speaker_id::writer::WriteResponse, SpeakerTranscriptWriteError> {
    write_request(&serde_json::to_vec(&value).expect("serialize request"))
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse line"))
        .collect()
}

fn npy_payload(bytes: &[u8]) -> &[u8] {
    assert_eq!(&bytes[..6], b"\x93NUMPY");
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    &bytes[10 + header_len..]
}

fn npy_header(bytes: &[u8]) -> &str {
    let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
    std::str::from_utf8(&bytes[10..10 + header_len]).expect("NPY header")
}

fn read_npz_member(archive: &mut ZipArchive<fs::File>, name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .expect("member")
        .read_to_end(&mut bytes)
        .expect("read");
    bytes
}

#[test]
fn ac1_every_written_sentence_id_is_persisted() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([
            {"id": 8, "start_offset_us": 0, "text": "one"},
            {"id": 4, "start_offset_us": 1_500_000, "text": "two"}
        ]),
        json!([4]),
        json!([1.0]),
        1,
    );
    write(value).expect("write");
    let read =
        read_transcript_rows(&fs::read(directory.path().join("segment.jsonl")).expect("read"))
            .expect("parse transcript");
    assert_eq!(
        read.rows
            .iter()
            .map(|row| row.sentence_id)
            .collect::<Vec<_>>(),
        [8, 4]
    );
    assert!(
        read.rows
            .iter()
            .all(|row| row.source == SentenceIdSource::Persisted)
    );
}

#[test]
fn ac2_missing_id_refuses_before_publication() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::MissingStatementId { .. })
    ));
    assert!(!directory.path().join("segment.jsonl").exists());
    assert!(!directory.path().join("segment.npz").exists());
}

#[test]
fn ac3_invalid_id_refuses_without_positional_fallback() {
    let directory = TempDir::new();
    for id in [json!(0), json!(-1), json!(2_147_483_648_i64), json!("7")] {
        let value = request(
            &directory,
            json!([{"id": id, "text": "one"}]),
            json!([]),
            json!([]),
            0,
        );
        assert!(matches!(
            write(value),
            Err(SpeakerTranscriptWriteError::InvalidStatementId { .. })
        ));
    }
}

#[test]
fn ac4_all_jsonl_bytes_are_ascii() {
    let directory = TempDir::new();
    let emoji = char::from_u32(0x1f642).expect("emoji");
    let cjk = char::from_u32(0x4e2d).expect("cjk");
    let value = request(
        &directory,
        json!([{"id": 1, "text": format!("cafe\u{00e9} {cjk} {emoji}")}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    assert!(
        fs::read(directory.path().join("segment.jsonl"))
            .expect("read")
            .iter()
            .all(u8::is_ascii)
    );
}

#[test]
fn ac5_non_ascii_is_escaped_in_nested_header_values() {
    let directory = TempDir::new();
    let separator = char::from_u32(0x2028).expect("separator");
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["header"]["segment_meta"] = json!({"nested": format!("a{separator}b")});
    write(value).expect("write");
    let content = fs::read_to_string(directory.path().join("segment.jsonl")).expect("read");
    assert!(content.contains("\\u2028"));
    assert!(!content.as_bytes().contains(&0xe2));
}

#[test]
fn ac6_controls_use_json_escapes() {
    let directory = TempDir::new();
    let text = format!(
        "a{}{}{}{}{}",
        '\u{000b}', '\u{000c}', '\u{001c}', '\u{001d}', '\u{001e}'
    );
    let value = request(
        &directory,
        json!([{"id": 1, "text": text}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    let content = fs::read_to_string(directory.path().join("segment.jsonl")).expect("read");
    for escape in ["\\u000b", "\\f", "\\u001c", "\\u001d", "\\u001e"] {
        assert!(content.contains(escape), "missing {escape}");
    }
}

#[test]
fn ac7_splitlines_oracle_recognizes_all_raw_hazards() {
    let raw = format!(
        "a{}b{}c{}d",
        char::from_u32(0x0085).expect("u0085"),
        char::from_u32(0x2028).expect("u2028"),
        char::from_u32(0x2029).expect("u2029"),
    );
    assert!(python_splitlines(&raw).len() > raw.split('\n').count());
}

#[test]
fn ac8_writer_escapes_hazards_for_line_splitting() {
    let directory = TempDir::new();
    let hazards = [0x0085, 0x2028, 0x2029];
    let raw = format!(
        "a{}b{}c{}d",
        char::from_u32(hazards[0]).expect("u0085"),
        char::from_u32(hazards[1]).expect("u2028"),
        char::from_u32(hazards[2]).expect("u2029"),
    );
    let value = request(
        &directory,
        json!([{"id": 1, "text": raw}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    let output = fs::read_to_string(directory.path().join("segment.jsonl")).expect("read");
    let content_without_terminal_newline = output.strip_suffix('\n').unwrap_or(&output);
    assert_eq!(
        python_splitlines(&output).len(),
        content_without_terminal_newline.split('\n').count()
    );
}

#[test]
fn hazard_bytes_are_not_present_raw() {
    let directory = TempDir::new();
    let text = format!(
        "{}{}{}",
        char::from_u32(0x0085).expect("u0085"),
        char::from_u32(0x2028).expect("u2028"),
        char::from_u32(0x2029).expect("u2029"),
    );
    let value = request(
        &directory,
        json!([{"id": 1, "text": text}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    let bytes = fs::read(directory.path().join("segment.jsonl")).expect("read");
    assert!(bytes.iter().all(u8::is_ascii));
}

#[test]
fn ac9_absent_and_null_speaker_both_omit_the_output_key() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"id": 1, "text": "absent"}, {"id": 2, "text": "null", "speaker": null}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    for row in &lines(&directory.path().join("segment.jsonl"))[1..] {
        assert!(row.get("speaker").is_none());
    }
}

#[test]
fn present_string_or_integer_speaker_is_preserved() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"id": 1, "text": "one", "speaker": "speaker-a"}, {"id": 2, "text": "two", "speaker": 4}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    let rows = lines(&directory.path().join("segment.jsonl"));
    assert_eq!(rows[1]["speaker"], "speaker-a");
    assert_eq!(rows[2]["speaker"], 4);
}

#[test]
fn ac10_empty_source_is_omitted() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["source"] = json!("");
    write(value).expect("write");
    assert!(
        lines(&directory.path().join("segment.jsonl"))[1]
            .get("source")
            .is_none()
    );
}

#[test]
fn ac11_header_order_conditionals_and_rounding_match_contract() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["header"] = json!({
        "raw": "audio.wav", "backend": "", "model": "model", "device": "cpu", "compute_type": "int8",
        "observer": "observer", "duration": 12.3456789, "noisy": true, "noisy_rms": 1.0 / 3.0,
        "noisy_s": 2.34, "loud_windows": 2, "speech_loud_windows": 1, "loud_speech_ratio": 1.0 / 3.0,
        "overlap_fraction": 1.0 / 3.0, "overlap_detector": "overlap", "speaker_evidence": true,
        "speaker_evidence_multi_fraction": 1.0 / 3.0, "speaker_evidence_version": "v1",
        "speaker_analysis_producer": "producer", "segment_meta": {"facet": "work"},
        "_solstone_processing": {"state": "done"}, "sound_tags": {"tags": []}
    });
    write(value).expect("write");
    let header = fs::read_to_string(directory.path().join("segment.jsonl"))
        .expect("read")
        .lines()
        .next()
        .expect("header")
        .to_owned();
    let keys: Vec<_> = serde_json::from_str::<Value>(&header)
        .expect("header json")
        .as_object()
        .expect("object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        [
            "raw",
            "backend",
            "model",
            "device",
            "compute_type",
            "observer",
            "duration",
            "noisy",
            "noisy_rms",
            "noisy_s",
            "loud_windows",
            "speech_loud_windows",
            "loud_speech_ratio",
            "overlap_fraction",
            "overlap_detector",
            "speaker_evidence",
            "speaker_evidence_multi_fraction",
            "speaker_evidence_version",
            "speaker_analysis_producer",
            "facet",
            "_solstone_processing",
            "sound_tags"
        ]
    );
    let parsed: Value = serde_json::from_str(&header).expect("parse");
    assert_eq!(parsed["backend"], "unknown");
    assert_eq!(parsed["duration"], 12.35);
    assert_eq!(parsed["noisy_rms"], 0.3333);
    assert_eq!(parsed["noisy_s"], 2.3);
    assert_eq!(parsed["loud_speech_ratio"], 0.33);
    assert_eq!(parsed["overlap_fraction"], 0.3333);
    assert_eq!(parsed["speaker_evidence_multi_fraction"], 0.3333);
}

#[test]
fn ac12_segment_meta_overwrites_earlier_fields_but_not_later_fields() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["header"]["segment_meta"] = json!({
        "model": "meta-model",
        "sound_tags": {"from": "meta"},
    });
    value["header"]["sound_tags"] = json!({"from": "real"});
    value["header"]["_solstone_processing"] = json!({"state": "real"});
    write(value).expect("write");
    let header = lines(&directory.path().join("segment.jsonl"))[0].clone();
    assert_eq!(header["model"], "meta-model");
    assert_eq!(header["sound_tags"], json!({"from": "real"}));
    assert_eq!(header["_solstone_processing"], json!({"state": "real"}));
}

#[test]
fn optional_header_groups_are_omitted_when_conditions_do_not_match() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["header"]["overlap_fraction"] = json!(0.5);
    value["header"]["loud_windows"] = json!(0);
    write(value).expect("write");
    let header = lines(&directory.path().join("segment.jsonl"))[0].clone();
    assert!(header.get("overlap_fraction").is_none());
    assert!(header.get("loud_windows").is_none());
}

#[test]
fn ac13_output_paths_must_be_sibling_stems() {
    let directory = TempDir::new();
    let mut value = request(&directory, json!([]), json!([]), json!([]), 0);
    value["output"]["npz_path"] = json!(directory.path().join("other.npz"));
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::InvalidOutputPath { .. })
    ));
}

#[test]
fn ac14_start_offsets_wrap_across_day_boundaries() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"id": 1, "start_offset_us": 1_000_000, "text": "after"}, {"id": 2, "start_offset_us": -1_500_000, "text": "before"}]),
        json!([]),
        json!([]),
        0,
    );
    write(value).expect("write");
    let rows = lines(&directory.path().join("segment.jsonl"));
    assert_eq!(rows[1]["start"], "00:00:00");
    assert_eq!(rows[2]["start"], "23:59:58");
}

#[test]
fn ac15_empty_statements_write_a_header_only_jsonl() {
    let directory = TempDir::new();
    write(request(&directory, json!([]), json!([]), json!([]), 0)).expect("write");
    let content = fs::read_to_string(directory.path().join("segment.jsonl")).expect("read");
    assert_eq!(content.lines().count(), 1);
}

#[test]
fn ac16_jsonl_has_exactly_one_trailing_newline() {
    let directory = TempDir::new();
    write(request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    ))
    .expect("write");
    let bytes = fs::read(directory.path().join("segment.jsonl")).expect("read");
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
}

#[test]
fn payload_descriptor_is_strict() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    );
    value["embeddings"]["dtype"] = json!("float32");
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::PayloadInvalid { .. })
    ));
}

#[test]
fn ac17_payload_nonfinite_is_refused() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    );
    let payload_path = PathBuf::from(value["embeddings"]["payload_path"].as_str().expect("path"));
    let mut bytes = payload(1);
    bytes[..4].copy_from_slice(&f32::NAN.to_le_bytes());
    fs::write(payload_path, bytes).expect("overwrite payload");
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::PayloadNonFinite { row: 0, col: 0 })
    ));
}

#[test]
fn ac18_embedding_statement_subset_keeps_caller_order() {
    let directory = TempDir::new();
    let value = request(
        &directory,
        json!([{"id": 1, "text": "one"}, {"id": 2, "text": "two"}, {"id": 3, "text": "three"}]),
        json!([3, 1]),
        json!([3.0, 1.0]),
        2,
    );
    write(value).expect("write");
    let mut archive =
        ZipArchive::new(fs::File::open(directory.path().join("segment.npz")).expect("open"))
            .expect("archive");
    let bytes = read_npz_member(&mut archive, "statement_ids.npy");
    let ids = npy_payload(&bytes)
        .chunks_exact(4)
        .map(|bytes| i32::from_le_bytes(bytes.try_into().expect("i32")))
        .collect::<Vec<_>>();
    assert_eq!(ids, [3, 1]);
    let transcript = lines(&directory.path().join("segment.jsonl"));
    assert_eq!(transcript[1]["sentence_id"], 1);
    assert_eq!(transcript[3]["sentence_id"], 3);
}

#[test]
fn ac19_npz_has_exactly_four_expected_members() {
    let directory = TempDir::new();
    write(request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    ))
    .expect("write");
    let mut archive =
        ZipArchive::new(fs::File::open(directory.path().join("segment.npz")).expect("open"))
            .expect("archive");
    let mut names = (0..archive.len())
        .map(|index| archive.by_index(index).expect("member").name().to_owned())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "durations_s.npy",
            "embeddings.npy",
            "encoder.npy",
            "statement_ids.npy"
        ]
    );
    let embeddings = read_npz_member(&mut archive, "embeddings.npy");
    let statement_ids = read_npz_member(&mut archive, "statement_ids.npy");
    let durations = read_npz_member(&mut archive, "durations_s.npy");
    assert!(npy_header(&embeddings).contains("'descr': '<f4'"));
    assert!(npy_header(&embeddings).contains("'shape': (1, 256)"));
    assert!(npy_header(&statement_ids).contains("'descr': '<i4'"));
    assert!(npy_header(&durations).contains("'descr': '<f4'"));
}

#[test]
fn ac20_encoder_is_caller_supplied() {
    let directory = TempDir::new();
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    );
    value["embeddings"]["encoder"] = json!("test-encoder-id");
    write(value).expect("write");
    let mut archive =
        ZipArchive::new(fs::File::open(directory.path().join("segment.npz")).expect("open"))
            .expect("archive");
    let mut bytes = Vec::new();
    archive
        .by_name("encoder.npy")
        .expect("member")
        .read_to_end(&mut bytes)
        .expect("read");
    assert!(bytes.windows("test-encoder-id".len() * 4).any(|window| {
        window
            == "test-encoder-id"
                .chars()
                .flat_map(|character| (character as u32).to_le_bytes())
                .collect::<Vec<_>>()
    }));
}

#[test]
fn ac21_npz_write_creates_no_lock_sidecar() {
    let directory = TempDir::new();
    write(request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    ))
    .expect("write");
    assert!(!directory.path().join("segment.npz.lock").exists());
}

#[test]
fn ac22_empty_embeddings_leave_existing_npz_untouched() {
    let directory = TempDir::new();
    let npz_path = directory.path().join("segment.npz");
    fs::write(&npz_path, b"existing").expect("seed npz");
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([]),
        json!([]),
        0,
    );
    value["output"]["redo"] = json!(true);
    write(value).expect("write");
    assert_eq!(fs::read(npz_path).expect("read npz"), b"existing");
}

#[test]
fn ac25_npz_publishes_before_jsonl() {
    let directory = TempDir::new();
    fs::create_dir(directory.path().join("segment.jsonl")).expect("block jsonl destination");
    let mut value = request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    );
    value["output"]["redo"] = json!(true);
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::OutputUnwritable { .. })
    ));
    assert!(directory.path().join("segment.npz").exists());
}

#[test]
fn ac23_existing_destination_stays_unchanged_without_temp_artifacts() {
    let directory = TempDir::new();
    let jsonl_path = directory.path().join("segment.jsonl");
    fs::write(&jsonl_path, b"pre-existing-content").expect("seed jsonl");
    let value = request(&directory, json!([]), json!([]), json!([]), 0);
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::DestinationExists { .. })
    ));
    assert_eq!(
        fs::read(&jsonl_path).expect("read jsonl"),
        b"pre-existing-content"
    );
    assert!(
        fs::read_dir(directory.path())
            .expect("read directory")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with(".tmp_"))
    );
}

#[test]
fn ac26_redo_allows_overwriting_an_existing_destination() {
    let directory = TempDir::new();
    write(request(
        &directory,
        json!([{"id": 1, "text": "before"}]),
        json!([]),
        json!([]),
        0,
    ))
    .expect("first write");
    let mut replacement = request(
        &directory,
        json!([{"id": 1, "text": "after"}]),
        json!([]),
        json!([]),
        0,
    );
    replacement["output"]["redo"] = json!(true);
    write(replacement).expect("redo write");
    assert_eq!(
        lines(&directory.path().join("segment.jsonl"))[1]["text"],
        "after"
    );
}

#[test]
fn success_response_reports_both_paths_and_counts() {
    let directory = TempDir::new();
    let response = write(request(
        &directory,
        json!([{"id": 1, "text": "one"}]),
        json!([1]),
        json!([1.0]),
        1,
    ))
    .expect("write");
    assert_eq!(response.statement_count, 1);
    assert_eq!(response.embedding_row_count, 1);
    assert!(response.jsonl_path.ends_with("segment.jsonl"));
    assert!(response.npz_path.ends_with("segment.npz"));
}

#[test]
fn unknown_schema_is_distinct_from_malformed_request() {
    let directory = TempDir::new();
    let mut value = request(&directory, json!([]), json!([]), json!([]), 0);
    value["schema"] = json!("unknown");
    assert!(matches!(
        write(value),
        Err(SpeakerTranscriptWriteError::UnknownSchema { .. })
    ));
}

fn python_splitlines(value: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut iterator = value.char_indices().peekable();
    while let Some((index, character)) = iterator.next() {
        let separator = matches!(
            character,
            '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}'
        );
        if separator {
            lines.push(&value[start..index]);
            if character == '\r' && iterator.peek().is_some_and(|(_, next)| *next == '\n') {
                let _ = iterator.next();
            }
            start = index + character.len_utf8();
        }
    }
    if start < value.len() {
        lines.push(&value[start..]);
    }
    lines
}
