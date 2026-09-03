// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Value, json};
use solstone_core_timeline::SegmentBindingV1;

use crate::segments::DEFAULT_STREAM;

use super::{browser, projection};

pub fn load(root: &Path, day: &str, stream: &str, segment: &str) -> Value {
    let binding = SegmentBindingV1 {
        day: day.to_owned(),
        stream: stream.to_owned(),
        segment: segment.to_owned(),
    };
    let timeline = projection::segment(root, &binding);
    let dir = if stream == DEFAULT_STREAM {
        root.join("chronicle").join(day).join(segment)
    } else {
        root.join("chronicle").join(day).join(stream).join(segment)
    };
    let mut payload = json!({
        "day": day,
        "stream": if stream == DEFAULT_STREAM { "" } else { stream },
        "segment": segment,
        "status": timeline.status.as_str(),
        "artifact_outcome": timeline.outcome.as_str(),
        "timeline": timeline.value.as_ref().map(|value| json!({
            "binding": value.binding,
            "input_digest": value.input_digest,
            "generated_at_ms": value.generated_at_ms,
            "summary": value.summary,
            "provenance": value.provenance,
        })),
        "audio": null,
        "screen": null,
        "browser": [],
    });
    if !dir.is_dir() {
        payload["error"] = Value::String(format!("segment dir not found: {}", dir.display()));
        return payload;
    }
    if let Some(audio) = read_jsonl(&dir.join("audio.jsonl")) {
        payload["audio"] = json!({"header": audio.0, "lines": audio.1});
    }
    let mut screens = matching(&dir, |name| name.ends_with("screen.jsonl"));
    screens.sort();
    if let Some(screen) = screens
        .first()
        .and_then(|path| read_jsonl(path).map(|rows| (path, rows)))
    {
        payload["screen"] = json!({"header": screen.1.0, "frames": screen.1.1, "filename": screen.0.file_name().and_then(|name| name.to_str()).unwrap_or_default()});
    }
    let mut browsers = matching(&dir, |name| {
        name.starts_with("browser_") && name.ends_with(".jsonl")
    });
    browsers.sort();
    payload["browser"] = Value::Array(browsers.iter().map(|path| load_browser(path)).collect());
    payload
}

fn matching(dir: &Path, predicate: impl Fn(&str) -> bool) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_file() && predicate(&entry.file_name().to_string_lossy())).then_some(path)
        })
        .collect()
}
fn read_jsonl(path: &Path) -> Option<(Value, Vec<Value>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let rows = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).ok())
        .collect::<Option<Vec<Value>>>()?;
    let (header, lines) = rows.split_first()?;
    Some((header.clone(), lines.to_vec()))
}
fn load_browser(path: &Path) -> Value {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let result = (|| {
        let text = std::fs::read_to_string(path).ok()?;
        let rows = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).ok())
            .collect::<Option<Vec<Value>>>()?;
        let start = rows
            .iter()
            .find(|row| row.get("t").and_then(Value::as_str) == Some("segment_start"));
        let (chunks, error) = browser::format_browser(&rows);
        if error.is_some() && chunks.is_empty() {
            return None;
        }
        Some(
            json!({"file": filename, "site_name": site_name(filename, start), "site": start.and_then(|row| row.get("site")).and_then(Value::as_str).unwrap_or_default(), "title": start.and_then(|row| row.get("title")).and_then(Value::as_str).unwrap_or_default(), "entries": chunks.into_iter().map(|chunk| json!({"ts": chunk["timestamp"].as_i64().unwrap_or(0), "kind": if chunk["source"]["t"].as_str() == Some("segment_start") { "snapshot" } else { "change" }, "markdown": chunk["markdown"]})).collect::<Vec<_>>(), "error": Value::Null}),
        )
    })();
    result.unwrap_or_else(|| json!({"file": filename, "site_name": site_name(filename, None), "site": "", "title": "", "entries": [], "error": "couldn't read this file"}))
}
fn site_name(filename: &str, start: Option<&Value>) -> String {
    let adapter = start
        .and_then(|row| row.get("adapter"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if !adapter.is_empty() {
        return title_case(adapter);
    }
    let site = start
        .and_then(|row| row.get("site"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if !site.is_empty() {
        return site.to_owned();
    }
    let stem = filename.strip_prefix("browser_").unwrap_or(filename);
    let stem = stem.strip_suffix(".jsonl").unwrap_or(stem);
    let stem = stem.replace('-', ".");
    if stem.is_empty() {
        filename.to_owned()
    } else {
        stem
    }
}

fn title_case(value: &str) -> String {
    let mut output = String::new();
    let mut at_word_start = true;
    for character in value.chars() {
        if character.is_alphabetic() {
            if at_word_start {
                output.extend(character.to_uppercase());
            } else {
                output.extend(character.to_lowercase());
            }
            at_word_start = false;
        } else {
            output.push(character);
            at_word_start = true;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::{
        segments::{DEFAULT_STREAM, origin},
        test_support::phase_root,
    };

    use super::{load, site_name};

    fn write(path: &std::path::Path, text: &str) {
        fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        fs::write(path, text).expect("file");
    }

    #[test]
    fn ac9_default_origin_and_missing_segment_contract() {
        let root = TempDir::new().expect("root");
        let payload = load(root.path(), "20260510", DEFAULT_STREAM, "100000_300");
        assert_eq!(payload["stream"], "");
        assert!(
            payload["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
        assert_eq!(
            origin("20260510", DEFAULT_STREAM, "100000_300"),
            "20260510/100000_300"
        );
        assert_eq!(
            origin("20260510", "workstation.browser", "100000_300"),
            "20260510/workstation.browser/100000_300"
        );
    }

    #[test]
    fn ac20_screen_and_browser_files_are_lexicographically_ordered() {
        let root = TempDir::new().expect("root");
        let segment = root.path().join("chronicle/20260510/100000_300");
        write(&segment.join("b.screen.jsonl"), "{\"t\":\"header\"}\n");
        write(&segment.join("a.screen.jsonl"), "{\"t\":\"header\"}\n");
        write(
            &segment.join("browser_b.jsonl"),
            "{\"t\":\"segment_start\",\"ts\":1}\n",
        );
        write(
            &segment.join("browser_a.jsonl"),
            "{\"t\":\"segment_start\",\"ts\":1}\n",
        );
        let payload = load(root.path(), "20260510", DEFAULT_STREAM, "100000_300");
        assert_eq!(payload["screen"]["filename"], "a.screen.jsonl");
        assert_eq!(
            payload["browser"]
                .as_array()
                .expect("browser")
                .iter()
                .map(|item| item["file"].as_str().expect("file"))
                .collect::<Vec<_>>(),
            ["browser_a.jsonl", "browser_b.jsonl"]
        );
    }

    #[test]
    fn ac3_site_name_matches_python_title_and_independent_stem_stripping() {
        assert_eq!(site_name("browser_foo.txt", None), "foo.txt");
        assert_eq!(site_name("browser_.jsonl", None), "browser_.jsonl");
        assert_eq!(
            site_name("browser_x.jsonl", Some(&json!({"adapter": "GENERIC"}))),
            "Generic"
        );
        assert_eq!(
            site_name("browser_x.jsonl", Some(&json!({"adapter": "my adapter"}))),
            "My Adapter"
        );
    }

    #[test]
    fn segment_timeline_status_and_provenance_are_projected_from_v1_artifact() {
        let root = phase_root("populated");
        let payload = load(root.path(), "20260510", DEFAULT_STREAM, "100000_300");

        assert_eq!(payload["status"], "current");
        assert_eq!(payload["artifact_outcome"], "current");
        assert_eq!(payload["timeline"]["summary"]["title"], "Both streams");
        assert_eq!(
            payload["timeline"]["provenance"]["model"],
            "corpus-segment-model"
        );
    }

    #[test]
    fn segment_status_is_stale_after_a_newer_failed_recuration_attempt() {
        let root = phase_root("populated");
        let state_path = root.path().join("health/timeline/state.json");
        let mut state: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&state_path).expect("state"))
                .expect("state JSON");
        state["attempts"]["segment:20260510/_default/100000_300:newer-failed"] = json!({
            "attempt_id": "newer-failed",
            "input_digest": "changed-source-input",
            "started_at_ms": 1770000050001_i64,
            "finished_at_ms": 1770000050002_i64,
            "outcome": "failed",
            "detail": "fixture failure",
        });
        crate::test_support::write(
            &state_path,
            &serde_json::to_string(&state).expect("state JSON"),
        );

        let payload = load(root.path(), "20260510", DEFAULT_STREAM, "100000_300");
        assert_eq!(payload["status"], "stale");
        assert_eq!(payload["artifact_outcome"], "digest_mismatch");
    }
}
