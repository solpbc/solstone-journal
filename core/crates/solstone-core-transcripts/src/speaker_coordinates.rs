// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sentence coordinates shared by the owner view and the bounded agent projection.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

/// Count statements from the start of each source, including non-rendered starts.
/// Stored sentence_id fields are not the coordinates used by local speaker labels.
pub fn next_statement_ordinal(line: &str, ordinal: &mut u64) -> Option<i64> {
    match serde_json::from_str::<Value>(line.trim()) {
        Ok(Value::Object(record)) => next_record_ordinal(&record, ordinal),
        _ => None,
    }
}

fn next_record_ordinal(record: &Map<String, Value>, ordinal: &mut u64) -> Option<i64> {
    if !record.contains_key("start") {
        return None;
    }
    *ordinal += 1;
    i64::try_from(*ordinal).ok()
}

/// The coordinate who-spoke rows name a transcript line by: its stored
/// `sentence_id`, or its statement `ordinal` when it stores none. Transcripts
/// written before the transcriber stored sentence ids are named by ordinal; a
/// stored id that isn't an integer names no line.
pub fn line_coordinate(record: &Map<String, Value>, ordinal: Option<i64>) -> Option<i64> {
    match record.get("sentence_id") {
        None | Some(Value::Null) => ordinal,
        Some(stored) => stored.as_i64(),
    }
}

/// The next record's [`line_coordinate`], advancing `ordinal` exactly as
/// [`next_statement_ordinal`] does for the line the record parsed from.
pub fn next_line_coordinate(record: &Map<String, Value>, ordinal: &mut u64) -> Option<i64> {
    let statement = next_record_ordinal(record, ordinal);
    line_coordinate(record, statement)
}

fn audio_sources(dir: &Path) -> Vec<String> {
    let mut values = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            (name.ends_with("audio.jsonl")
                || solstone_core_format::audio_archive::is_audio_archive_transcript_name(&name))
            .then(|| name.trim_end_matches(".jsonl").into())
        })
        .collect::<Vec<String>>();
    values.sort();
    values
}
/// Select the source whose sentence ordinals the segment-local labels address.
pub fn speaker_label_source(dir: &Path) -> (Option<String>, bool) {
    let audio = audio_sources(dir);
    let mut npz = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".npz")?;
            (stem == "audio"
                || stem.ends_with("_audio")
                || solstone_core_format::audio_archive::is_audio_archive_part_stem(stem))
            .then(|| stem.into())
        })
        .collect::<Vec<String>>();
    npz.sort();
    if let Some(value) = npz.into_iter().next() {
        (Some(value), false)
    } else if audio.len() == 1 {
        (Some(audio[0].clone()), false)
    } else {
        (None, audio.len() > 1)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    fn record(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn a_line_is_named_by_its_stored_sentence_id_else_its_ordinal() {
        let mut ordinal = 0;
        let coordinates = [
            json!({"start": "00:00:01", "text": "no stored id"}),
            json!({"raw": "audio.flac"}),
            json!({"start": "00:00:02", "sentence_id": 9}),
            json!({"start": "00:00:03", "sentence_id": null}),
            json!({"start": "00:00:04", "sentence_id": "4"}),
        ]
        .map(|value| super::next_line_coordinate(&record(value), &mut ordinal));
        assert_eq!(coordinates, [Some(1), None, Some(9), Some(3), None]);
        assert_eq!(ordinal, 4);
    }

    #[test]
    fn only_start_bearing_objects_advance_the_statement_ordinal() {
        let mut ordinal = 0;
        for line in ["", "broken", "[]", "{\"raw\":\"audio.flac\"}"] {
            assert_eq!(super::next_statement_ordinal(line, &mut ordinal), None);
        }
        assert_eq!(
            super::next_statement_ordinal("{\"start\":null}", &mut ordinal),
            Some(1)
        );
        assert_eq!(
            super::next_statement_ordinal("{\"start\":1}", &mut ordinal),
            Some(2)
        );
        assert_eq!(
            super::next_statement_ordinal(
                "{\"start\":\"00:00:01\",\"sentence_id\":99}",
                &mut ordinal
            ),
            Some(3)
        );
    }
}
