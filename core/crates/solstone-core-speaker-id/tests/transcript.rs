// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_speaker_id::transcript::*;

fn ids(read: &TranscriptRead) -> Vec<i64> {
    read.rows.iter().map(|row| row.sentence_id).collect()
}

#[test]
fn ac1_no_persisted_ids_resolve_positionally() {
    let read = read_transcript_rows(
        b"header\n{\"text\":\"one\"}\n{\"text\":\"two\"}\n{\"text\":\"three\"}",
    )
    .expect("read transcript");

    assert_eq!(ids(&read), [1, 2, 3]);
    assert!(
        read.rows
            .iter()
            .all(|row| row.source == SentenceIdSource::Positional)
    );
}

#[test]
fn ac2_blank_line_consumes_an_ordinal() {
    let read = read_transcript_rows(b"header\n{\"text\":\"one\"}\n\n{\"text\":\"three\"}")
        .expect("read transcript");

    assert_eq!(ids(&read), [1, 3]);
}

#[test]
fn ac3_malformed_line_consumes_an_ordinal() {
    let read = read_transcript_rows(b"header\n{\"text\":\"one\"}\n{not json\n{\"text\":\"three\"}")
        .expect("read transcript");

    assert_eq!(ids(&read), [1, 3]);
    assert_eq!(read.malformed_lines, 1);
}

#[test]
fn ac4_no_trailing_newline_resolves_last_row() {
    let input = ["header", r#"{"text":"one"}"#, r#"{"text":"two"}"#].join("\n");
    let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

    assert_eq!(ids(&read), [1, 2]);
}

#[test]
fn ac5_crlf_resolves_ids_1_2() {
    let input = ["header", r#"{"text":"one"}"#, r#"{"text":"two"}"#, ""].join("\r\n");
    let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

    assert_eq!(ids(&read), [1, 2]);
}

#[test]
fn ac6_header_only_file_yields_zero_rows() {
    let read = read_transcript_rows(b"header").expect("read transcript");

    assert!(read.had_header);
    assert!(read.rows.is_empty());
}

#[test]
fn ac7_lone_cr_is_a_line_terminator() {
    let read = read_transcript_rows(b"header\r{\"text\":\"one\"}\r{\"text\":\"two\"}\r")
        .expect("read transcript");

    assert_eq!(ids(&read), [1, 2]);
}

#[test]
fn ac8_persisted_id_wins_over_position() {
    let read =
        read_transcript_rows(b"header\n{\"text\":\"one\"}\n{\"text\":\"two\",\"sentence_id\":7}")
            .expect("read transcript");

    assert_eq!(read.rows[1].sentence_id, 7);
    assert_eq!(read.rows[1].source, SentenceIdSource::Persisted);
    assert_eq!(read.disagreements, 1);
}

#[test]
fn ac9_disagreements_zero_when_persisted_ids_match_position() {
    let read = read_transcript_rows(
        b"header\n{\"text\":\"one\",\"sentence_id\":1}\n{\"text\":\"two\",\"sentence_id\":2}\n{\"text\":\"three\",\"sentence_id\":3}",
    )
    .expect("read transcript");

    assert_eq!(read.disagreements, 0);
    assert!(
        read.rows
            .iter()
            .all(|row| row.source == SentenceIdSource::Persisted)
    );
}

#[test]
fn ac10_invalid_persisted_id_forms_are_ignored() {
    for value in [r#""7""#, "7.5", "7.0", "null", "[]", "{}"] {
        let input = format!("header\n{{\"text\":\"row\",\"sentence_id\":{value}}}");
        let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

        assert_eq!(ids(&read), [1], "value {value}");
        assert_eq!(
            read.rows[0].source,
            SentenceIdSource::PositionalAfterIgnoredId,
            "value {value}"
        );
        assert_eq!(read.ignored_ids, 1, "value {value}");
    }
}

#[test]
fn ac11_persisted_id_zero_or_negative_is_ignored() {
    for value in ["0", "-5"] {
        let input = format!("header\n{{\"text\":\"row\",\"sentence_id\":{value}}}");
        let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

        assert_eq!(ids(&read), [1], "value {value}");
        assert_eq!(
            read.rows[0].source,
            SentenceIdSource::PositionalAfterIgnoredId,
            "value {value}"
        );
        assert_eq!(read.ignored_ids, 1, "value {value}");
    }
}

#[test]
fn ac12_mixed_persisted_and_positional_rows_resolve_independently() {
    let read = read_transcript_rows(
        b"header\n{\"text\":\"one\"}\n{\"text\":\"two\",\"sentence_id\":7}\n{\"text\":\"three\"}",
    )
    .expect("read transcript");

    assert_eq!(ids(&read), [1, 7, 3]);
    assert_eq!(
        read.rows.iter().map(|row| row.source).collect::<Vec<_>>(),
        [
            SentenceIdSource::Positional,
            SentenceIdSource::Persisted,
            SentenceIdSource::Positional,
        ]
    );
}

#[test]
fn ac13_malformed_lines_counted_alongside_valid_rows() {
    let read = read_transcript_rows(
        b"header\n{not json\n{\"text\":\"two\"}\n[\n{\"text\":\"four\"}\ninvalid",
    )
    .expect("read transcript");

    assert_eq!(read.malformed_lines, 3);
    assert_eq!(ids(&read), [2, 4]);
}

#[test]
fn ac14_zero_bytes_distinguishable_from_header_only() {
    let read = read_transcript_rows(b"").expect("read transcript");

    assert!(!read.had_header);
}

#[test]
fn ac15_valid_json_non_object_line_is_malformed() {
    let read = read_transcript_rows(b"header\n[1,2]").expect("read transcript");

    assert_eq!(read.malformed_lines, 1);
    assert!(read.rows.is_empty());
}

#[test]
fn ac16_duplicate_sentence_ids_are_counted() {
    let read = read_transcript_rows(
        b"header\n{\"text\":\"one\",\"sentence_id\":1}\n{\"text\":\"two\",\"sentence_id\":1}",
    )
    .expect("read transcript");

    assert!(read.duplicate_ids >= 1);
}

#[test]
fn ac17_invalid_utf8_returns_err() {
    assert!(matches!(
        read_transcript_rows(&[0xff, 0xfe, 0x00]),
        Err(TranscriptError::InvalidUtf8)
    ));
}

#[test]
fn ac18_u2028_in_text_is_not_a_line_boundary() {
    let input = format!(
        "header\n{{\"text\":\"one{}two\"}}\n{{\"text\":\"three\"}}",
        '\u{2028}'
    );
    let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

    assert_eq!(ids(&read), [1, 2]);
    assert_eq!(read.rows[0].value["text"].as_str(), Some("one\u{2028}two"));
}

#[test]
fn ac19_u0085_in_text_is_not_a_line_boundary() {
    let input = format!(
        "header\n{{\"text\":\"one{}two\"}}\n{{\"text\":\"three\"}}",
        '\u{0085}'
    );
    let read = read_transcript_rows(input.as_bytes()).expect("read transcript");

    assert_eq!(ids(&read), [1, 2]);
    assert_eq!(read.rows[0].value["text"].as_str(), Some("one\u{0085}two"));
}

#[test]
fn ac20_a_persisted_id_above_the_sampled_maximum_is_still_honoured() {
    // The highest sentence_id observed on a reference journal is an
    // OBSERVATION, not a contract: a segment may hold any number of
    // statements. Bounding the resolver on a sampled maximum silently replaces
    // the persisted identity of every row above it with a positional guess,
    // which is the exact re-attribution this resolver exists to prevent.
    for persisted in [1_i64, 209, 210, 211, 250, 10_000] {
        let body = format!(
            "{}\n{{\"start\":\"00:00:01\",\"text\":\"one\",\"sentence_id\":{persisted}}}\n",
            r#"{"raw":"audio.flac"}"#
        );
        let read = read_transcript_rows(body.as_bytes()).expect("read");
        assert_eq!(
            (read.rows[0].sentence_id, read.rows[0].source),
            (persisted, SentenceIdSource::Persisted),
            "persisted sentence_id {persisted} must be honoured"
        );
    }

    // The negative twin: non-positive ids remain out of band.
    for persisted in [0_i64, -1, -250] {
        let body = format!(
            "{}\n{{\"start\":\"00:00:01\",\"text\":\"one\",\"sentence_id\":{persisted}}}\n",
            r#"{"raw":"audio.flac"}"#
        );
        let read = read_transcript_rows(body.as_bytes()).expect("read");
        assert_eq!(
            (read.rows[0].sentence_id, read.rows[0].source),
            (1, SentenceIdSource::PositionalAfterIgnoredId),
            "persisted sentence_id {persisted} must be ignored"
        );
        assert_eq!(read.ignored_ids, 1);
    }
}
