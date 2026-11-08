// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use serde_json::json;

#[test]
fn speech_occurrences_keep_coordinates_order_and_multiplicity() {
    let (records, _) = parse_raw_percept_records(
        &[
            json!({"raw":"audio.flac"}),
            json!({"speaker":99,"text":"no start, not rendered"}),
            json!({"start":"00:00:01","speaker":3,"sentence_id":7,"text":"same"}),
            json!({"start":"00:00:01","speaker":3,"sentence_id":7,"text":"same"}),
            json!({"start":"00:00:02","speaker":5,"text":"fallback"}),
            json!({"start":"00:00:03","speaker":9,"sentence_id":"bad","text":"unbound"}),
            json!({"start":"00:00:04","speaker":"You","text":"named"}),
            json!({"start":"00:00:05","speaker":1,"sentence_id":null,"text":"fallback"}),
        ]
        .map(|row| row.to_string())
        .join("\n"),
    );
    let mut produced = produce_raw_percept_chunks_by_shape(RawPerceptFamily::Audio, None, &records);
    assert!(
        produced.error.is_some(),
        "formatter's deliberate skipped-record diagnostic"
    );
    let path = Path::new("audio.jsonl");
    let actual = numeric_speech_evidence(path, &records, &produced).unwrap();
    assert_eq!(
        actual.lines,
        [
            (3, Some(7)),
            (3, Some(7)),
            (5, Some(3)),
            (9, None),
            (1, Some(6))
        ]
        .map(|(number, coordinate)| NumericSpeechLine { number, coordinate })
    );
    produced.chunks[0].source = None;
    assert!(numeric_speech_evidence(path, &records, &produced).is_err());
    produced.chunks[0].source = Some(
        json!({"start":"absent","speaker":3})
            .as_object()
            .unwrap()
            .clone(),
    );
    assert!(numeric_speech_evidence(path, &records, &produced).is_err());
}

#[test]
fn padding_changes_no_speech_evidence_or_rendering_and_keeps_bulk_cap() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("chronicle/20260101/100000_300");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audio.jsonl");
    let other = dir.join("other_audio.jsonl");
    fs::write(
        &other,
        "{\"start\":\"00:00:01\",\"sentence_id\":1,\"speaker\":3,\"text\":\"other\"}\n",
    )
    .unwrap();
    let sources = Sources {
        transcripts: true,
        percepts: false,
        talents: TalentSource::Disabled,
        voices: None,
        heard_owner: None,
        strava_tiles: false,
    };
    let mut results = Vec::new();
    for padding in [0, ENTRY_SOURCE_MAX_BYTES + 1] {
        fs::write(
            &path,
            format!(
                "{}\nnot JSON\n{}\n{}\n",
                json!({"raw":"audio.flac","ignored": "x".repeat(padding)}),
                json!({"start":"00:00:01","speaker":3,"text":"first"}),
                json!({"start":"00:00:02","speaker":3,"text":"last"})
            ),
        )
        .unwrap();
        let (rendered, _, _) =
            cluster_span_with_memory(root.path(), "20260101", &["100000_300"], &sources, None)
                .unwrap();
        assert_eq!(
            rendered.sources.iter().flatten().count(),
            if padding == 0 { 2 } else { 1 }
        );
        let evidence =
            speech_evidence_for_span(root.path(), "20260101", &["100000_300"], None, None).unwrap();
        assert_eq!(
            evidence,
            vec![
                SpeechSourceEvidence {
                    path: path.clone(),
                    lines: vec![
                        NumericSpeechLine {
                            number: 3,
                            coordinate: Some(1)
                        },
                        NumericSpeechLine {
                            number: 3,
                            coordinate: Some(2)
                        }
                    ]
                },
                SpeechSourceEvidence {
                    path: other.clone(),
                    lines: vec![NumericSpeechLine {
                        number: 3,
                        coordinate: Some(1)
                    }]
                },
            ]
        );
        results.push((rendered.text, evidence));
    }
    assert_eq!(results[0], results[1]);
}

#[test]
fn successful_empty_source_is_distinct_from_failed_selected_source() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("chronicle/20260101/100000_300");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audio.jsonl");
    fs::write(&path, "").unwrap();
    assert_eq!(
        speech_evidence_for_span(root.path(), "20260101", &["100000_300"], None, None).unwrap(),
        vec![SpeechSourceEvidence {
            path: path.clone(),
            lines: vec![]
        }]
    );
    fs::write(
        dir.join("other_audio.jsonl"),
        "{\"start\":\"00:00:01\",\"speaker\":3,\"text\":\"surviving source\"}\n",
    )
    .unwrap();
    fs::write(&path, [0xff]).unwrap();
    assert!(
        speech_evidence_for_span(root.path(), "20260101", &["100000_300"], None, None)
            .unwrap_err()
            .contains("unable to read speech evidence")
    );
}
