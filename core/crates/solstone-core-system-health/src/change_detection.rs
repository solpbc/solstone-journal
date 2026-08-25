// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only native port of Sense's per-segment change detector.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use chrono::NaiveDate;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use solstone_core_format::segment::{is_date_key, segment_parse, segment_start_and_end_seconds};
use solstone_core_journal_io::{DEFAULT_STREAM, PathOrDay, iter_segments};

const SCREEN_DHASH_THRESHOLD: u32 = 8;
const TRANSCRIPT_WORD_DELTA_FLOOR: usize = 5;
const GAP_THRESHOLD_SECONDS: i64 = 600;

/// Assemble current sensor state without writing any journal data.
pub(crate) fn assemble_sensor_state(segment_dir: &Path) -> Value {
    let mut monitors = Map::new();
    let mut files = fs::read_dir(segment_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    files.sort();
    for path in files.iter().filter(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("screen.jsonl"))
    }) {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let monitor = screen_monitor_name(stem);
        let header = fs::read_to_string(path).ok().and_then(|contents| {
            contents
                .lines()
                .next()
                .and_then(|line| serde_json::from_str::<Value>(line).ok())
        });
        monitors.insert(monitor, json!({
            "first_hash": header.as_ref().and_then(|value| value.get("first_hash")).and_then(normalize_hash),
            "last_hash": header.as_ref().and_then(|value| value.get("last_hash")).and_then(normalize_hash),
            "qualified_count": header.as_ref().and_then(|value| value.get("qualified_count")).and_then(normalize_count).unwrap_or(0),
        }));
    }
    let text = normalized_transcript(segment_dir);
    let present = !text.is_empty();
    json!({
        "screen": {"monitors": monitors},
        "transcript": {"present": present, "word_count": if present { text.split_whitespace().count() } else { 0 }, "content_hash": if present { Some(format!("sha256:{:x}", Sha256::digest(text.as_bytes()))) } else { None::<String> }},
    })
}

/// Return the chronological prior same-stream segment ref, if comparable.
///
/// This must derive from `iter_segments(day)` only. Do not read
/// `last_segment_key` or `awareness/activity_state.json` here: those track
/// last-processed state, not the chronological predecessor, and are wrong under
/// backfill or reprocess.
pub fn resolve_predecessor(
    journal: &Path,
    day: &str,
    stream: Option<&str>,
    segment: &str,
) -> Option<Value> {
    let stream = stream.unwrap_or(DEFAULT_STREAM);
    let mut comparable = Vec::new();
    for entry in iter_segments(journal, PathOrDay::Day(day)).ok()? {
        if !entry.stream().matches(stream) {
            continue;
        }
        let Some(name) = entry.name().to_str().map(str::to_owned) else {
            return Some(json!({"day": day, "stream": stream, "unresolvable": true}));
        };
        comparable.push((name, entry.path().to_path_buf()));
    }
    let index = comparable.iter().position(|(name, _)| name == segment)?;
    if index == 0 {
        return None;
    }
    let predecessor = &comparable[index - 1].0;
    let (_, previous_end) = segment_start_and_end_seconds(predecessor)?;
    let (current_start, _) = segment_start_and_end_seconds(segment)?;
    let current = i64::from(current_start.hour) * 3600
        + i64::from(current_start.minute) * 60
        + i64::from(current_start.second);
    ((current - i64::try_from(previous_end).ok()?) <= GAP_THRESHOLD_SECONDS)
        .then(|| json!({"day": day, "stream": stream, "segment": predecessor}))
}

/// Read the prior persisted sensor snapshot, failing open on malformed input.
pub(crate) fn read_predecessor_state(
    journal: &Path,
    day: &str,
    predecessor: Option<&Value>,
) -> Option<Value> {
    let predecessor = predecessor?.as_object()?;
    if predecessor.get("unresolvable").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let stream = predecessor.get("stream")?.as_str()?;
    let segment = predecessor.get("segment")?.as_str()?;
    let path = iter_segments(journal, PathOrDay::Day(day))
        .ok()?
        .into_iter()
        .find_map(|entry| {
            let name = entry.name().to_str()?;
            (entry.stream().matches(stream) && name == segment).then(|| entry.path().to_path_buf())
        })?
        .join("talents/change.json");
    let sensors = serde_json::from_slice::<Value>(&fs::read(path).ok()?)
        .ok()?
        .get("sensors")?
        .clone();
    (sensors.get("screen")?.is_object() && sensors.get("transcript")?.is_object())
        .then_some(sensors)
}

pub(crate) fn compare_screen(previous: &Value, current: &Value) -> Value {
    let previous = screen_monitors(previous);
    let current = screen_monitors(current);
    if previous.is_empty() && current.is_empty() {
        return json!({"present": false, "changed": false});
    }
    if previous.keys().collect::<BTreeSet<_>>() != current.keys().collect::<BTreeSet<_>>() {
        return json!({"present": true, "changed": true});
    }
    for key in current.keys() {
        let before = previous
            .get(key)
            .and_then(|value| value.get("last_hash"))
            .and_then(hash_to_int);
        let after = current
            .get(key)
            .and_then(|value| value.get("first_hash"))
            .and_then(hash_to_int);
        if before
            .zip(after)
            .is_none_or(|(before, after)| (before ^ after).count_ones() >= SCREEN_DHASH_THRESHOLD)
        {
            return json!({"present": true, "changed": true});
        }
    }
    json!({"present": true, "changed": false})
}

pub(crate) fn compare_transcript(previous: &Value, current: &Value) -> Value {
    if !previous.get("present").is_some_and(python_truthy)
        || !current.get("present").is_some_and(python_truthy)
    {
        return json!({"present": false, "changed": false});
    }
    let before = previous.get("content_hash").and_then(Value::as_str);
    let after = current.get("content_hash").and_then(Value::as_str);
    let Some((before, after)) = before.zip(after) else {
        return json!({"present": true, "changed": true});
    };
    json!({"present": true, "changed": before != after && word_count(current).abs_diff(word_count(previous)) > TRANSCRIPT_WORD_DELTA_FLOOR})
}

pub(crate) fn classify(vectors: &Value) -> (String, Vec<String>) {
    let vectors = vectors.as_object().cloned().unwrap_or_default();
    if !vectors
        .values()
        .any(|value| value.get("present").is_some_and(python_truthy))
    {
        return ("idle".to_owned(), Vec::new());
    }
    let changed = vectors
        .iter()
        .filter(|(_, value)| {
            value.get("present").is_some_and(python_truthy)
                && value.get("changed").is_some_and(python_truthy)
        })
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    if changed.is_empty() {
        ("redundant".to_owned(), changed)
    } else {
        ("active".to_owned(), changed)
    }
}

pub fn detect_segment_change(
    journal: &Path,
    day: &str,
    _stream: Option<&str>,
    _segment: &str,
    segment_dir: &Path,
    predecessor: Option<Value>,
    timestamp: &str,
) -> Value {
    let current = assemble_sensor_state(segment_dir);
    let previous = read_predecessor_state(journal, day, predecessor.as_ref());
    let vectors = if let Some(previous) = previous {
        json!({"screen": compare_screen(&previous["screen"], &current["screen"]), "transcript": compare_transcript(&previous["transcript"], &current["transcript"])})
    } else {
        let screen = !screen_monitors(&current["screen"]).is_empty();
        let transcript = current["transcript"]
            .get("present")
            .is_some_and(python_truthy);
        json!({"screen": {"present": screen, "changed": screen}, "transcript": {"present": transcript, "changed": transcript}})
    };
    let (change_class, changed_sensors) = classify(&vectors);
    json!({"timestamp": timestamp, "predecessor": predecessor, "change_class": change_class, "changed_sensors": changed_sensors, "sensors": current})
}

fn screen_monitor_name(stem: &str) -> String {
    let mut parts = stem.split('_');
    let (Some(position), Some(connector), Some("screen"), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return "unknown:unknown".to_owned();
    };
    if stem.split('_').count() != 3
        || position.is_empty()
        || connector.is_empty()
        || !position
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
        || !connector
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return "unknown:unknown".to_owned();
    }
    format!("{position}:{connector}")
}
fn hash_to_int(value: &Value) -> Option<u64> {
    if value.is_boolean() {
        None
    } else if let Some(value) = value.as_u64() {
        Some(value)
    } else {
        u64::from_str_radix(value.as_str()?, 16).ok()
    }
}
fn normalize_hash(value: &Value) -> Option<String> {
    Some(format!("{:016x}", hash_to_int(value)?))
}
fn normalize_count(value: &Value) -> Option<u64> {
    (!value.is_boolean()).then(|| value.as_u64()).flatten()
}
fn screen_monitors(value: &Value) -> BTreeMap<String, Value> {
    value
        .get("monitors")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}
fn word_count(value: &Value) -> usize {
    value.get("word_count").and_then(Value::as_u64).unwrap_or(0) as usize
}
fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn normalized_transcript(segment_dir: &Path) -> String {
    let mut files = fs::read_dir(segment_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name.ends_with("audio.jsonl")
                || name.ends_with("_transcript.jsonl")
                || name.ends_with("_transcript.md")
                || name == "imported.md"
        })
        .collect::<Vec<_>>();
    files.sort();
    let mut parts = Vec::new();
    for path in files {
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            if let Ok(text) = fs::read_to_string(path)
                && !text.trim().is_empty()
            {
                parts.push(text);
            }
        } else if let Some(output) = formatted_jsonl_transcript(&path)
            && !output.trim().is_empty()
        {
            parts.push(output);
        }
    }
    parts
        .join("\n")
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn formatted_jsonl_transcript(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let records = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Value>(line.trim())
                .ok()?
                .as_object()
                .cloned()
        })
        .collect::<Option<Vec<Map<String, Value>>>>()?;
    let rel = normalize_transcript_path(&path.to_string_lossy());
    format_transcript_for_fingerprint(&rel, &records)
}

fn normalize_transcript_path(path: &str) -> String {
    path.replace('\\', "/")
}

/// Format exactly the text that Python's `hear._format_transcript_entries`
/// supplies to change detection.
///
/// This intentionally does not reuse the shared raw-audio renderer. That
/// renderer has established transcript-web consumers and its JSON display
/// coercions are observable there; this fingerprint must instead preserve
/// `hear.py`'s truthiness and `str()` semantics without changing that output.
fn format_transcript_for_fingerprint(rel: &str, records: &[Map<String, Value>]) -> Option<String> {
    let metadata = records
        .first()
        .filter(|record| !record.contains_key("start"));
    let mut header_parts = transcript_start_header(rel).into_iter().collect::<Vec<_>>();
    if let Some(metadata) = metadata {
        for (key, value) in metadata {
            if matches!(
                key.as_str(),
                "error" | "raw" | "imported" | "_solstone_processing" | "sound_tags"
            ) {
                continue;
            }
            let value = transcript_header_value(value);
            if !value.is_empty() {
                header_parts.push(format!("{}: {value}", python_capitalize(key)));
            }
        }
        if let Some(imported) = metadata.get("imported").and_then(Value::as_object) {
            if let Some(facet) = imported.get("facet") {
                header_parts.push(format!("Facet: {}", python_display(facet)));
            }
            if let Some(id) = imported.get("id") {
                header_parts.push(format!("Import ID: {}", python_display(id)));
            }
        }
    }

    let mut parts = (!header_parts.is_empty())
        .then(|| header_parts.join(" "))
        .into_iter()
        .collect::<Vec<_>>();
    for (index, entry) in records.iter().enumerate() {
        if index == 0 && metadata.is_some() {
            continue;
        }
        if !entry.contains_key("start") {
            continue;
        }
        let mut entry_parts = Vec::new();
        if let Some(start) = entry.get("start").filter(|value| python_truthy(value)) {
            entry_parts.push(format!("[{}]", python_display(start)));
        }
        if let Some(source) = entry.get("source").filter(|value| python_truthy(value)) {
            entry_parts.push(format!("({})", python_display(source)));
        }
        match entry.get("speaker") {
            Some(Value::Bool(value)) => entry_parts.push(format!("Speaker {value}:")),
            Some(Value::Number(value)) if value.is_i64() || value.is_u64() => {
                entry_parts.push(format!("Speaker {value}:"));
            }
            Some(value) => entry_parts.push(format!("{}:", python_display(value))),
            None => entry_parts.push(String::new()),
        }
        let text = entry
            .get("corrected")
            .filter(|value| python_truthy(value))
            .or_else(|| entry.get("text").filter(|value| python_truthy(value)))
            .map(python_display)
            .unwrap_or_default();
        let prefix = entry_parts.join(" ").trim().to_owned();
        let mut markdown = if !prefix.is_empty() {
            if text.is_empty() {
                prefix
            } else {
                format!("{prefix} {text}")
            }
        } else if !text.is_empty() {
            text
        } else {
            continue;
        };
        if let Some(emotion) = entry.get("emotion").filter(|value| python_truthy(value)) {
            let emotion = emotion.as_str()?;
            if !emotion.eq_ignore_ascii_case("neutral") {
                markdown.push_str(&format!(" *({emotion})*"));
            }
        }
        parts.push(markdown);
    }
    Some(parts.join("\n"))
}

fn transcript_start_header(rel: &str) -> Option<String> {
    let parts = rel.split('/').collect::<Vec<_>>();
    let (day_index, day) = parts
        .iter()
        .enumerate()
        .rev()
        .find(|(_, part)| is_date_key(part))?;
    let times = parts
        .iter()
        .skip(day_index + 1)
        .rev()
        .find_map(|part| segment_parse(part))?;
    let day = NaiveDate::parse_from_str(day, "%Y%m%d").ok()?;
    let start = day.and_hms_opt(times.hour.into(), times.minute.into(), times.second.into())?;
    Some(start.format("Start: %Y-%m-%d %I:%M%P").to_string())
}

fn transcript_header_value(value: &Value) -> String {
    match value {
        Value::Array(values) => values
            .iter()
            .map(python_display)
            .collect::<Vec<_>>()
            .join(", "),
        _ => python_display(value),
    }
}

fn python_display(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => python_repr(value),
    }
}

fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(value) => {
            if *value {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        Value::Number(value) => value.to_string(),
        Value::String(value) => python_string_repr(value),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(python_repr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", python_string_repr(key), python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn python_string_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character == quote => {
                escaped.push('\\');
                escaped.push(character);
            }
            character if character.is_control() => escaped.extend(character.escape_default()),
            character => escaped.push(character),
        }
    }
    format!("{quote}{escaped}{quote}")
}

fn python_capitalize(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    let mut result = first.to_uppercase().collect::<String>();
    result.push_str(&characters.as_str().to_lowercase());
    result
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::{
        assemble_sensor_state, classify, compare_screen, compare_transcript, detect_segment_change,
        format_transcript_for_fingerprint, formatted_jsonl_transcript, normalize_transcript_path,
        read_predecessor_state, resolve_predecessor,
    };

    fn segment(root: &std::path::Path, stream: Option<&str>, name: &str) -> std::path::PathBuf {
        let mut path = root.join("chronicle/20260101");
        if let Some(stream) = stream {
            path.push(stream);
        }
        path.push(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn strict_sensor_thresholds_match_the_reference() {
        let prior = json!({"monitors":{"main":{"last_hash":"0000000000000000"}}});
        let seven = json!({"monitors":{"main":{"first_hash":"000000000000007f"}}});
        let eight = json!({"monitors":{"main":{"first_hash":"00000000000000ff"}}});
        assert!(!compare_screen(&prior, &seven)["changed"].as_bool().unwrap());
        assert!(compare_screen(&prior, &eight)["changed"].as_bool().unwrap());
        let earlier = json!({"present":true,"content_hash":"a","word_count":2});
        let five = json!({"present":true,"content_hash":"b","word_count":7});
        let six = json!({"present":true,"content_hash":"b","word_count":8});
        assert!(
            !compare_transcript(&earlier, &five)["changed"]
                .as_bool()
                .unwrap()
        );
        assert!(
            compare_transcript(&earlier, &six)["changed"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(
            classify(&json!({"screen":{"present":true,"changed":false}})).0,
            "redundant"
        );
    }

    #[test]
    fn predecessor_gap_boundaries_filter_streams_and_use_the_iterator() {
        let root = tempfile::tempdir().unwrap();
        segment(root.path(), None, "090000_60");
        segment(root.path(), None, "091100_60");
        segment(root.path(), None, "092201_60");
        segment(root.path(), Some("other"), "091000_60");
        let exact = resolve_predecessor(root.path(), "20260101", None, "091100_60").unwrap();
        assert_eq!(exact["segment"], "090000_60");
        assert_eq!(exact["stream"], "_default");
        assert_eq!(
            resolve_predecessor(root.path(), "20260101", None, "092201_60"),
            None
        );
        assert_eq!(
            resolve_predecessor(root.path(), "20260101", Some("other"), "091000_60"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_utf8_sibling_makes_predecessor_unresolvable() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().unwrap();
        segment(root.path(), None, "090000_60");
        segment(root.path(), None, "091100_60");
        fs::create_dir_all(
            root.path()
                .join("chronicle/20260101")
                .join(OsStr::from_bytes(b"090530_60\xff")),
        )
        .unwrap();
        let result = resolve_predecessor(root.path(), "20260101", None, "091100_60").unwrap();
        assert_eq!(result["unresolvable"], true);
        assert!(result.get("segment").is_none());
    }

    #[test]
    fn malformed_predecessor_shapes_all_use_missing_predecessor_vectors() {
        let root = tempfile::tempdir().unwrap();
        let prior = segment(root.path(), None, "090000_60");
        let current = segment(root.path(), None, "090100_60");
        fs::write(
            current.join("main_DP-1_screen.jsonl"),
            "{\"first_hash\":\"0\",\"last_hash\":\"0\",\"qualified_count\":1}\n",
        )
        .unwrap();
        let predecessor = json!({"day":"20260101","stream":"default","segment":"090000_60"});
        for contents in [
            "{",
            "[]",
            "{\"sensors\":[]}",
            "{\"sensors\":{\"screen\":[],\"transcript\":{}}}",
            "{\"sensors\":{\"screen\":{},\"transcript\":[]}}",
        ] {
            let talents = prior.join("talents");
            fs::create_dir_all(&talents).unwrap();
            fs::write(talents.join("change.json"), contents).unwrap();
            assert_eq!(
                read_predecessor_state(root.path(), "20260101", Some(&predecessor)),
                None
            );
            let detected = detect_segment_change(
                root.path(),
                "20260101",
                None,
                "090100_60",
                &current,
                Some(predecessor.clone()),
                "now",
            );
            assert_eq!(detected["change_class"], "active");
            assert_eq!(detected["changed_sensors"], json!(["screen"]));
        }
        fs::remove_file(prior.join("talents/change.json")).unwrap();
        assert_eq!(
            read_predecessor_state(root.path(), "20260101", Some(&predecessor)),
            None
        );
    }

    #[test]
    fn formatted_transcripts_include_metadata_and_entry_formatting() {
        let root = tempfile::tempdir().unwrap();
        let segment = segment(root.path(), None, "090000_60");
        let transcript = concat!(
            "{\"setting\":\"work\",\"topics\":[\"planning\"],\"topic\":null,\"rank\":7}\n",
            "{\"start\":\"00:00:01\",\"source\":false,\"speaker\":\"Alex\",\"text\":\"bare words\",\"corrected\":\"Corrected words\",\"emotion\":\"focused\"}\n"
        );
        let path = segment.join("audio.jsonl");
        fs::write(&path, transcript).unwrap();
        let expected = "Start: 2026-01-01 09:00am Setting: work Topics: planning Topic: None Rank: 7\n[00:00:01] Alex: Corrected words *(focused)*";
        assert_eq!(formatted_jsonl_transcript(&path), Some(expected.to_owned()));
        let records = transcript
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .map(|value| value.as_object().unwrap().clone())
            .collect::<Vec<_>>();
        let windows_rel = normalize_transcript_path(r"chronicle\20260101\090000_60\audio.jsonl");
        assert_eq!(
            format_transcript_for_fingerprint(&windows_rel, &records),
            Some(expected.to_owned())
        );
        let state = assemble_sensor_state(&segment);
        let expected = "start: 2026-01-01 09:00am setting: work topics: planning topic: none rank: 7 [00:00:01] alex: corrected words *(focused)*";
        assert_eq!(state["transcript"]["word_count"], 16);
        assert_eq!(
            state["transcript"]["content_hash"],
            format!("sha256:{:x}", Sha256::digest(expected.as_bytes()))
        );
        assert_ne!(
            state["transcript"]["content_hash"],
            format!("sha256:{:x}", Sha256::digest(b"bare words"))
        );
    }

    #[test]
    fn malformed_screen_filenames_use_the_unknown_monitor() {
        let root = tempfile::tempdir().unwrap();
        let segment = segment(root.path(), None, "090000_60");
        fs::write(
            segment.join("bad_position_DP-1_screen.jsonl"),
            "{\"first_hash\":\"0\",\"last_hash\":\"0\",\"qualified_count\":1}\n",
        )
        .unwrap();
        assert!(
            assemble_sensor_state(&segment)["screen"]["monitors"]
                .get("unknown:unknown")
                .is_some()
        );
    }
}
