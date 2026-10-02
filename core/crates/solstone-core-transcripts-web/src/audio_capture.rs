// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only projection of client evidence, independent of transcription.
//! Earlier capture failures survive duplicate receipts and later recovery.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::segment::WarningDetail;

const SOURCE_LIMIT: usize = 32;
const FAILURE_LIMIT: usize = 16;
const CAPTURE_BYTES_LIMIT: usize = 262144;

#[derive(Clone, Deserialize, Serialize)]
struct Failure {
    stage: String,
    domain: String,
    code: i64,
    count: u64,
}

#[derive(Clone, Deserialize, Serialize)]
struct Source {
    source_id: String,
    kind: String,
    expected: bool,
    started: bool,
    state: String,
    received_frames: u64,
    accepted_frames: u64,
    dropped_frames: u64,
    writer_status: String,
    failures: Vec<Failure>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Remix {
    source_id: String,
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<i64>,
    frames_copied: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    music_analysis: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_outcome: Option<Box<Remix>>,
}

#[derive(Deserialize)]
struct Capture {
    version: u32,
    state: String,
    #[serde(default)]
    sources: Vec<Source>,
    #[serde(default)]
    remix: Vec<Remix>,
    app_version: Option<String>,
    app_build: Option<String>,
}

pub(crate) fn read_audio_capture(dir: &Path) -> Option<Value> {
    match solstone_core_callosum::read_device_ingest_events(dir) {
        Ok(report) => {
            let mut result = project(
                report
                    .records
                    .iter()
                    .filter_map(|event| event.meta.get("audio_capture")),
            );
            if report.unparseable > 0 {
                let value = result.get_or_insert_with(|| json!({"state": "unknown"}));
                value["evidence_unavailable"] = json!(true);
                if value["state"] == "finished" {
                    value["state"] = json!("unknown");
                }
            }
            result
        }
        Err(_) => Some(json!({"state": "unknown", "evidence_unavailable": true})),
    }
}

fn state_rank(state: &str) -> Option<u8> {
    match state {
        "finished" => Some(0),
        "unknown" => Some(1),
        "recording" | "interrupted" => Some(2),
        "partial" => Some(3),
        "failed" => Some(4),
        _ => None,
    }
}

fn text_valid(text: &str, limit: usize) -> bool {
    !text.is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

fn remix_valid(row: &Remix, allow_prior: bool) -> bool {
    text_valid(&row.source_id, 512)
        && matches!(
            row.state.as_str(),
            "complete" | "partial" | "unreadable" | "failed"
        )
        && row.stage.as_ref().is_none_or(|s| text_valid(s, 64))
        && row.error_domain.as_ref().is_none_or(|s| text_valid(s, 128))
        && row
            .music_analysis
            .as_ref()
            .is_none_or(|s| text_valid(s, 64))
        && row
            .prior_outcome
            .as_ref()
            .is_none_or(|prior| allow_prior && remix_valid(prior, false))
}

fn capture_valid(capture: &Capture) -> bool {
    capture.version == 1
        && state_rank(&capture.state).is_some()
        && capture.sources.len() <= SOURCE_LIMIT
        && capture.remix.len() <= SOURCE_LIMIT
        && capture
            .app_version
            .as_ref()
            .is_none_or(|s| text_valid(s, 64))
        && capture.app_build.as_ref().is_none_or(|s| text_valid(s, 64))
        && capture.sources.iter().all(|source| {
            text_valid(&source.source_id, 512)
                && matches!(source.kind.as_str(), "system" | "microphone")
                && state_rank(&source.state).is_some()
                && matches!(
                    source.writer_status.as_str(),
                    "recording" | "completed" | "failed" | "no_audio"
                )
                && source.accepted_frames <= source.received_frames
                && source.dropped_frames <= source.received_frames
                && source.failures.len() <= FAILURE_LIMIT
                && source
                    .failures
                    .iter()
                    .all(|f| text_valid(&f.stage, 64) && text_valid(&f.domain, 128) && f.count > 0)
        })
        && capture.remix.iter().all(|row| remix_valid(row, true))
}

fn project<'a>(values: impl Iterator<Item = &'a Value>) -> Option<Value> {
    let mut seen = false;
    let mut unavailable = false;
    let mut state = "finished".to_owned();
    let mut sources = BTreeMap::<String, Source>::new();
    let mut remix = BTreeMap::<String, Remix>::new();
    let mut app_version = None;
    let mut app_build = None;
    for value in values {
        seen = true;
        if value.to_string().len() > CAPTURE_BYTES_LIMIT {
            unavailable = true;
            continue;
        }
        let Ok(capture) = serde_json::from_value::<Capture>(value.clone()) else {
            unavailable = true;
            continue;
        };
        if !capture_valid(&capture) {
            unavailable = true;
            continue;
        }
        if state_rank(&capture.state) > state_rank(&state) {
            state = capture.state.clone();
        }
        app_version = capture.app_version.or(app_version);
        app_build = capture.app_build.or(app_build);
        for mut source in capture.sources {
            if !source.failures.is_empty()
                || source.dropped_frames > 0
                || source.writer_status == "failed"
            {
                source.state = "partial".to_owned();
            }
            if let Some(previous) = sources.get(&source.source_id) {
                source.expected |= previous.expected;
                source.started |= previous.started;
                source.received_frames = source.received_frames.max(previous.received_frames);
                source.accepted_frames = source.accepted_frames.max(previous.accepted_frames);
                source.dropped_frames = source.dropped_frames.max(previous.dropped_frames);
                if state_rank(&previous.state) > state_rank(&source.state) {
                    source.state = previous.state.clone();
                }
                let incoming = std::mem::replace(&mut source.failures, previous.failures.clone());
                for failure in incoming {
                    if let Some(current) = source.failures.iter_mut().find(|f| {
                        f.stage == failure.stage
                            && f.domain == failure.domain
                            && f.code == failure.code
                    }) {
                        current.count = current.count.max(failure.count);
                    } else if source.failures.len() < FAILURE_LIMIT {
                        source.failures.push(failure);
                    } else {
                        unavailable = true;
                    }
                }
            }
            if state_rank(&source.state) > state_rank(&state) {
                state = source.state.clone();
            }
            if sources.contains_key(&source.source_id) || sources.len() < SOURCE_LIMIT {
                sources.insert(source.source_id.clone(), source);
            } else {
                unavailable = true;
            }
        }
        for mut row in capture.remix {
            if let Some(previous) = remix.get(&row.source_id) {
                row.prior_outcome = previous.prior_outcome.clone().or(row.prior_outcome);
                if row.prior_outcome.is_none() && previous.state != "complete" {
                    row.prior_outcome = Some(Box::new(previous.clone()));
                }
            }
            if remix.contains_key(&row.source_id) || remix.len() < SOURCE_LIMIT {
                remix.insert(row.source_id.clone(), row);
            } else {
                unavailable = true;
            }
        }
    }
    if !seen {
        return None;
    }
    if unavailable && state == "finished" {
        state = "unknown".to_owned();
    }
    if state == "recording" {
        state = "interrupted".to_owned();
    }
    Some(
        json!({"version": 1, "state": state, "evidence_unavailable": unavailable,
        "app_version": app_version, "app_build": app_build,
        "sources": sources.into_values().collect::<Vec<_>>(), "remix": remix.into_values().collect::<Vec<_>>() }),
    )
}

pub(crate) fn warnings(capture: &Value, now: DateTime<Utc>) -> Vec<WarningDetail> {
    let mut warnings = Vec::new();
    if let Some(sources) = capture["sources"].as_array() {
        for source in sources {
            let Some(failure) = source["failures"].as_array().and_then(|rows| rows.first()) else {
                continue;
            };
            warnings.push(WarningDetail {
                kind: "audio_capture".into(),
                file: source["source_id"].as_str().unwrap_or("audio").into(),
                message: format!(
                    "audio may be incomplete for this segment. {}: {} ({})",
                    failure["stage"].as_str().unwrap_or("capture"),
                    failure["domain"].as_str().unwrap_or("unknown"),
                    failure["code"]
                ),
                ts: now.to_rfc3339(),
            });
        }
    }
    if let Some(rows) = capture["remix"].as_array() {
        for row in rows.iter().filter(|row| row["state"] != "complete") {
            warnings.push(WarningDetail {
                kind: "audio_capture".into(),
                file: row["source_id"].as_str().unwrap_or("audio").into(),
                message: format!(
                    "audio may be incomplete for this segment. {}: {}",
                    row["stage"].as_str().unwrap_or("remix"),
                    row["state"].as_str().unwrap_or("unknown")
                ),
                ts: now.to_rfc3339(),
            });
        }
        for row in rows
            .iter()
            .filter(|row| row["state"] == "complete" && row["prior_outcome"].is_object())
        {
            warnings.push(WarningDetail {
                kind: "audio_capture_history".into(),
                file: row["source_id"].as_str().unwrap_or("audio").into(),
                message: "an earlier audio copy attempt had a problem. the latest copy completed."
                    .into(),
                ts: now.to_rfc3339(),
            });
        }
    }
    if !warnings.iter().any(|w| w.kind == "audio_capture")
        && matches!(
            capture["state"].as_str(),
            Some("partial" | "failed" | "interrupted")
        )
    {
        warnings.push(WarningDetail {
            kind: "audio_capture".into(),
            file: "audio".into(),
            message: "audio may be incomplete for this segment.".into(),
            ts: now.to_rfc3339(),
        });
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/audio-capture-v1.example.json"
        ))
        .unwrap()
    }
    #[test]
    fn generated_swift_schema_and_reader_limits_agree() {
        let schema: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/audio-capture-v1.schema.json"
        ))
        .unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        validator.validate(&fixture()["audio_capture"]).unwrap();
        assert_eq!(schema["properties"]["sources"]["maxItems"], SOURCE_LIMIT);
        assert_eq!(
            schema["properties"]["sources"]["items"]["properties"]["failures"]["maxItems"],
            FAILURE_LIMIT
        );
        for state in schema["properties"]["state"]["enum"].as_array().unwrap() {
            assert!(state_rank(state.as_str().unwrap()).is_some());
        }
    }

    #[test]
    fn recovered_copy_history_does_not_claim_current_loss() {
        let capture = json!({"version": 1, "state": "finished", "sources": [], "remix": [
            {"source_id": "mic", "state": "partial", "stage": "reader", "frames_copied": 960}]});
        let recovered = json!({"version": 1, "state": "finished", "sources": [], "remix": [
            {"source_id": "mic", "state": "complete", "frames_copied": 1920}]});
        let result = project([&capture, &recovered].into_iter()).unwrap();
        let detail = warnings(&result, Utc::now());
        assert_eq!(detail.len(), 1);
        assert_eq!(detail[0].kind, "audio_capture_history");
        assert!(detail[0].message.contains("latest copy completed"));
    }
    #[test]
    fn swift_emitted_capture_survives_duplicate_receipts_without_audio() {
        let original = fixture();
        let mut later = original["audio_capture"].clone();
        later["state"] = json!("finished");
        for source in later["sources"].as_array_mut().unwrap() {
            source["state"] = json!("finished");
            source["failures"] = json!([]);
            source["dropped_frames"] = json!(0);
        }
        let result = project([&original["audio_capture"], &later].into_iter()).unwrap();
        assert_eq!(result["state"], "partial");
        assert_eq!(result["sources"][1]["dropped_frames"], 960);
        assert_eq!(result["sources"][1]["failures"][0]["count"], 3);
        assert!(!warnings(&result, Utc::now()).is_empty());
    }
    #[test]
    fn malformed_future_and_missing_evidence_are_unknown_without_false_loss() {
        assert!(project(std::iter::empty()).is_none());
        for invalid in [json!({"version": 2}), json!("malformed")] {
            let result = project([&invalid].into_iter()).unwrap();
            assert_eq!(result["state"], "unknown");
            assert_eq!(result["evidence_unavailable"], true);
            assert!(warnings(&result, Utc::now()).is_empty());
        }
    }
    #[test]
    fn malformed_later_evidence_cannot_erase_an_earlier_capture_failure() {
        let original = fixture();
        let invalid = json!({"version": 99});
        let result = project([&original["audio_capture"], &invalid].into_iter()).unwrap();
        assert_eq!(result["state"], "partial");
        assert_eq!(result["evidence_unavailable"], true);
        assert_eq!(warnings(&result, Utc::now()).len(), 2);
    }
}
