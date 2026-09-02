// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort ambient sound tagging over the runtime-installed ced.cpp engine.
//!
//! Classification runs out of process through `solstone-core-ced-analyze`
//! (Brief D): `solstone-core-ced-sys` `dlopen`s a dynamically-linked glibc
//! shared object, and every consumer of this crate
//! (`solstone-core`, via `solstone-core-transcribe`) is a `musl-static`-lane
//! binary with no in-process dynamic loader to satisfy that call.
//! `solstone-core-local::install::ced_runtime` owns resolving and invoking
//! the sibling helper; this module owns windowing the decoded audio,
//! building the request, and aggregating the per-window response -- the same
//! split `solstone-core-transcribe`'s own VAD/speakers callers use for their
//! sibling helpers.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_local::install::ced_readiness::{
    CED_UNAVAILABLE_GUIDANCE, CedVerdict, evaluate_ced_readiness,
};
use solstone_core_local::install::ced_runtime::{
    CED_ANALYZE_TIMEOUT, CedAnalyzeProgram, invoke_ced_analyze,
};

pub const SCORE_FLOOR: f64 = 0.1;
pub const WINDOW_S: usize = 10;
pub const MIN_TAIL_S: usize = 1;
pub const CLASSIFY_SAMPLE_RATE: i32 = 16_000;
pub const ABI_VERSION: i32 = 1;
pub const ENGINE: &str = "ced.cpp v0.1.0";
pub const MODEL: &str = "ced-tiny-q8_0";
pub const AGG: &str = "max";

const REQUEST_SCHEMA: &str = "solstone-ced-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-ced-response-v1";
/// ced.cpp's own top-k cutoff; sound tagging always wants every label the
/// engine reports so `SCORE_FLOOR` (applied here) is the only filter.
const TOP_K: i32 = 0;

/// Tag PCM audio using the locally installed ced.cpp model.
///
/// All tagger failures are best-effort and therefore represented as `None`.
pub fn tag_audio(audio: &[f32], journal_path: &Path) -> Option<Value> {
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    // The shared verdict hashes the model and load-probes once (out of
    // process, through the sibling helper). This call then sends its own
    // classify request: the verdict is the gate, not a second path
    // derivation, and transcribe invokes this once per audio file
    // (`process_one`), so the extra helper invocation is not a hot path.
    tag_audio_with_readiness(audio, evaluate_ced_readiness(journal_path, os, arch))
}

/// Tag PCM using an already-computed CED verdict.
///
/// Production [`tag_audio`] supplies the catalog verdict. Tests supply a
/// verdict against a fixture digest so classify can run without the 6 MiB pin.
pub fn tag_audio_with_readiness(audio: &[f32], readiness: CedVerdict) -> Option<Value> {
    tag_audio_with_readiness_and_program(audio, readiness, &CedAnalyzeProgram::SiblingHelper)
}

/// [`tag_audio_with_readiness`] with the CED helper program supplied by the
/// caller.
///
/// Production always resolves the real out-of-process
/// `solstone-core-ced-analyze` sibling (Brief D). This crate's own tests
/// substitute a stub script here for the same reason
/// `solstone-core-local::install::ced_readiness`'s tests do: there is no
/// compiled cross-lane `zig-gnu-2.27` binary available in a dev `cargo test`
/// run, and `solstone-core-local`'s test-only base-dir override
/// (`ced_runtime::set_test_helper_base_dir`) is `pub(crate)` to that crate,
/// not reachable from here.
pub fn tag_audio_with_readiness_and_program(
    audio: &[f32],
    readiness: CedVerdict,
    program: &CedAnalyzeProgram,
) -> Option<Value> {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return None;
    }

    let (library, model) = match readiness {
        CedVerdict::Ready { library, model } => (library, model),
        CedVerdict::Unsupported { os, arch } => {
            log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
            return None;
        }
        CedVerdict::Degraded(status) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced readiness degraded: {status:?}");
            return None;
        }
    };
    classify_windows(audio, &spans, &library, &model, program)
}

fn classify_windows(
    audio: &[f32],
    spans: &[(usize, usize)],
    library: &Path,
    model: &Path,
    program: &CedAnalyzeProgram,
) -> Option<Value> {
    let temporary = match tempfile::Builder::new()
        .prefix("solstone-ced-analyze-")
        .tempdir()
    {
        Ok(directory) => directory,
        Err(error) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced audio sidecar directory failed: {error}");
            return None;
        }
    };
    let audio_path = temporary.path().join("audio.f32le");
    if let Err(error) = write_audio_sidecar(&audio_path, audio) {
        log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
        log::debug!("ced audio sidecar write failed: {error}");
        return None;
    }

    let request = json!({
        "schema": REQUEST_SCHEMA,
        "models": {
            "ced_library_path": library,
            "ced_model_path": model,
        },
        "audio_f32le_path": &audio_path,
        "sample_rate_hz": CLASSIFY_SAMPLE_RATE,
        "top_k": TOP_K,
        "windows": spans
            .iter()
            .map(|(start, end)| json!({"start_sample": start, "end_sample": end}))
            .collect::<Vec<_>>(),
    });
    let response = match invoke_ced_analyze(program, &request, CED_ANALYZE_TIMEOUT) {
        Ok(response) => response,
        Err(error) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced helper invocation failed: {error}");
            return None;
        }
    };
    windows_from_response(&response, spans.len())
}

fn write_audio_sidecar(path: &Path, audio: &[f32]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(audio));
    for sample in audio {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes)
}

/// Turn the helper's `solstone-ced-response-v1` payload into the same
/// aggregated shape [`tag_audio_with_readiness`] returned when this crate
/// classified in-process: one failed window is tolerated (best-effort keeps
/// the rest), but a malformed or wrong-shaped response degrades to `None`
/// exactly like an unreadable engine did before.
fn windows_from_response(response: &Value, expected_len: usize) -> Option<Value> {
    if response.get("schema").and_then(Value::as_str) != Some(RESPONSE_SCHEMA) {
        log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
        log::debug!("ced helper response had an unexpected schema: {response}");
        return None;
    }
    let windows = match response.get("windows").and_then(Value::as_array) {
        Some(windows) if windows.len() == expected_len => windows,
        _ => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced helper response had an unexpected windows shape: {response}");
            return None;
        }
    };

    let mut per_window = Vec::new();
    let mut first_failure = None;
    for (index, window) in windows.iter().enumerate() {
        match window_tags(window) {
            Some(tags) => per_window.push(tags),
            None => {
                let detail = window
                    .get("detail")
                    .and_then(Value::as_str)
                    .unwrap_or("ced helper reported an invalid window outcome")
                    .to_owned();
                log::debug!("sound tagger window {index} failed: {detail}");
                first_failure.get_or_insert(detail);
            }
        }
    }

    if per_window.is_empty() {
        let cause = first_failure.unwrap_or_else(|| "no successful windows".to_owned());
        log::warn!("sound tagger failed for all windows: {cause}");
        return None;
    }
    let tags = aggregate(&per_window);
    if tags.is_empty() {
        return None;
    }

    Some(json!({
        "engine": ENGINE,
        "model": MODEL,
        "threshold": SCORE_FLOOR,
        "window_s": WINDOW_S,
        "agg": AGG,
        "windows": per_window.len(),
        "tags": tags,
    }))
}

/// `Some` only for `{"ok": true, "tags": {...}}` with a well-formed `tags`
/// object. `solstone-core-ced-analyze` already validated and deduped ced's
/// raw per-window JSON, so this is a plain deserialize, not a re-parse of
/// ced's wire format.
fn window_tags(window: &Value) -> Option<BTreeMap<String, f64>> {
    if window.get("ok") != Some(&Value::Bool(true)) {
        return None;
    }
    serde_json::from_value(window.get("tags")?.clone()).ok()
}

fn window_spans(n_samples: usize) -> Vec<(usize, usize)> {
    if n_samples == 0 {
        return Vec::new();
    }
    let window_samples = WINDOW_S * CLASSIFY_SAMPLE_RATE as usize;
    let min_tail_samples = MIN_TAIL_S * CLASSIFY_SAMPLE_RATE as usize;
    let full_windows = n_samples / window_samples;
    let mut spans = (0..full_windows)
        .map(|index| {
            let start = index * window_samples;
            (start, start + window_samples)
        })
        .collect::<Vec<_>>();
    let tail_start = full_windows * window_samples;
    if n_samples - tail_start >= min_tail_samples {
        spans.push((tail_start, n_samples));
    }
    spans
}

fn aggregate(per_window: &[BTreeMap<String, f64>]) -> Map<String, Value> {
    let mut max_scores: BTreeMap<String, f64> = BTreeMap::new();
    for tags in per_window {
        for (label, score) in tags {
            max_scores
                .entry(label.clone())
                .and_modify(|current: &mut f64| *current = current.max(*score))
                .or_insert(*score);
        }
    }
    let mut kept = max_scores
        .into_iter()
        .filter(|(_, score)| *score > SCORE_FLOOR)
        .collect::<Vec<_>>();
    kept.sort_by(|(left_label, left_score), (right_label, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_label.cmp(right_label))
    });
    kept.into_iter()
        .fold(Map::new(), |mut tags, (label, score)| {
            tags.insert(
                label,
                Value::from((score * 1_000.0).round_ties_even() / 1_000.0),
            );
            tags
        })
}

#[cfg(test)]
mod tests {
    use super::{
        MIN_TAIL_S, RESPONSE_SCHEMA, SCORE_FLOOR, WINDOW_S, aggregate, window_spans,
        windows_from_response,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn window_spans_include_only_a_tail_of_at_least_one_second() {
        let sample_rate = 16_000_usize;
        assert_eq!(
            window_spans(WINDOW_S * sample_rate),
            vec![(0, WINDOW_S * sample_rate)]
        );
        assert_eq!(
            window_spans(WINDOW_S * sample_rate + MIN_TAIL_S * sample_rate - 1),
            vec![(0, WINDOW_S * sample_rate)]
        );
        // WINDOW_S=10, MIN_TAIL_S=1, 16 kHz → window=160_000, min_tail=16_000.
        // 8_000: 0 full windows, 8_000-sample tail < 16_000 → no spans.
        // 176_000: 1 full window + 16_000-sample tail == min_tail → two spans.
        // 168_000: 1 full window + 8_000-sample tail < min_tail → one span.
        assert_eq!(window_spans(8_000), vec![]);
        assert_eq!(
            window_spans(176_000),
            vec![(0, 160_000), (160_000, 176_000)]
        );
        assert_eq!(window_spans(168_000), vec![(0, 160_000)]);
    }

    #[test]
    fn aggregate_uses_max_score_and_strict_floor() {
        let first = BTreeMap::from([
            ("Below".to_owned(), SCORE_FLOOR),
            ("Music".to_owned(), 0.11),
        ]);
        let second = BTreeMap::from([("Music".to_owned(), 0.9)]);

        assert_eq!(
            serde_json::Value::Object(aggregate(&[first, second])),
            serde_json::json!({"Music": 0.9})
        );
    }

    #[test]
    fn windows_from_response_rejects_the_wrong_schema() {
        let response = json!({"schema": "solstone-ced-response-v2", "windows": []});
        assert_eq!(windows_from_response(&response, 0), None);
    }

    #[test]
    fn windows_from_response_rejects_a_length_mismatch() {
        let response = json!({
            "schema": RESPONSE_SCHEMA,
            "windows": [{"ok": true, "tags": {}}],
        });
        assert_eq!(windows_from_response(&response, 2), None);
    }

    #[test]
    fn windows_from_response_keeps_a_successful_window_despite_one_failure() {
        let response = json!({
            "schema": RESPONSE_SCHEMA,
            "windows": [
                {"ok": false, "reason": "classify-failed", "detail": "boom"},
                {"ok": true, "tags": {"Music": 0.9, "Above": 0.11}},
            ],
        });
        let tags = windows_from_response(&response, 2).expect("one successful window");
        assert_eq!(tags["windows"], json!(1));
        assert_eq!(tags["tags"], json!({"Music": 0.9, "Above": 0.11}));
        assert_eq!(tags["engine"], json!(super::ENGINE));
        assert_eq!(tags["model"], json!(super::MODEL));
    }

    #[test]
    fn windows_from_response_is_none_when_every_window_fails() {
        let response = json!({
            "schema": RESPONSE_SCHEMA,
            "windows": [{"ok": false, "reason": "classify-failed", "detail": "boom"}],
        });
        assert_eq!(windows_from_response(&response, 1), None);
    }
}
