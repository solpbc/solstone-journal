// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort ambient sound tagging over the runtime-installed ced.cpp engine.
//!
//! Classification runs out of process through `solstone-core-ced-analyze`:
//! `solstone-core-ced-sys` `dlopen`s a dynamically-linked glibc shared object,
//! and every consumer of this crate (`solstone-core`, via
//! `solstone-core-transcribe`) is a `musl-static`-lane binary with no
//! in-process dynamic loader to satisfy that call.
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
use solstone_core_local::install::capability_status::CapabilityStatus;
use solstone_core_local::install::ced_readiness::{
    CedVerdict, evaluate_ced_readiness, evaluate_ced_readiness_in_package_with_probe,
    fresh_model_check, fresh_model_check_in_package, probe_ced_engine,
};
use solstone_core_local::install::ced_runtime::{CED_ANALYZE_TIMEOUT, CedAnalyzeProgram};

#[cfg(not(windows))]
use solstone_core_local::install::ced_runtime::invoke_ced_analyze;

pub const SCORE_FLOOR: f64 = 0.1;
pub const WINDOW_S: usize = 10;
pub const MIN_TAIL_S: usize = 1;
pub const CLASSIFY_SAMPLE_RATE: i32 = 16_000;
pub const ABI_VERSION: i32 = 1;
pub const ENGINE: &str = "ced.cpp v0.1.0";
pub const MODEL: &str = "ced-tiny-q8_0";
pub const AGG: &str = "max";
/// Name prefix of the scratch directory that holds one classification's decoded
/// audio. The journal reaps any such directory a killed process left behind.
pub const CED_ANALYZE_TEMP_PREFIX: &str = "solstone-ced-analyze-";

const REQUEST_SCHEMA: &str = "solstone-ced-request-v1";
const RESPONSE_SCHEMA: &str = "solstone-ced-response-v1";
/// ced.cpp's own top-k cutoff; sound tagging always wants every label the
/// engine reports so `SCORE_FLOOR` (applied here) is the only filter.
pub const TOP_K: i32 = 0;

/// Tag PCM audio using the locally installed ced.cpp model.
///
/// Returns `(tags, status)`. Degraded yields `(None, Some(status))`.
pub fn tag_audio(audio: &[f32], journal_path: &Path) -> (Option<Value>, Option<CapabilityStatus>) {
    tag_audio_with_program(audio, journal_path, &CedAnalyzeProgram::SiblingHelper)
}

/// [`tag_audio`] with caller-specified helper program.
pub fn tag_audio_with_program(
    audio: &[f32],
    journal_path: &Path,
    program: &CedAnalyzeProgram,
) -> (Option<Value>, Option<CapabilityStatus>) {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return (None, None);
    }
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    #[cfg(windows)]
    let readiness = solstone_core_check::evaluate_host_ced(journal_path, os, arch);
    #[cfg(not(windows))]
    let readiness = evaluate_ced_readiness(journal_path, os, arch);

    let library = match readiness {
        CedVerdict::Ready { library, .. } => library,
        CedVerdict::Unsupported { os, arch } => {
            log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
            return (None, None);
        }
        CedVerdict::Degraded(status) => {
            if let Some(detail) = status.detail() {
                log::warn!("{detail}");
            }
            return (None, Some(status));
        }
    };

    let model = match fresh_model_check(os, arch) {
        Ok(model) => model,
        Err(status) => {
            if let Some(detail) = status.detail() {
                log::warn!("{detail}");
            }
            return (None, Some(status));
        }
    };

    let tags = classify_windows(audio, &spans, &library, &model, program);
    (tags, None)
}

/// Tag audio using an explicit package path and program (used by tests).
pub fn tag_audio_in_package(
    audio: &[f32],
    package_root_or_exe: &Path,
    program: &CedAnalyzeProgram,
) -> (Option<Value>, Option<CapabilityStatus>) {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return (None, None);
    }
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    let readiness = evaluate_ced_readiness_in_package_with_probe(
        package_root_or_exe,
        os,
        arch,
        |library, model| probe_ced_engine(program, library, model),
    );

    let library = match readiness {
        CedVerdict::Ready { library, .. } => library,
        CedVerdict::Unsupported { os, arch } => {
            log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
            return (None, None);
        }
        CedVerdict::Degraded(status) => {
            if let Some(detail) = status.detail() {
                log::warn!("{detail}");
            }
            return (None, Some(status));
        }
    };

    let model = match fresh_model_check_in_package(package_root_or_exe, os, arch) {
        Ok(model) => model,
        Err(status) => {
            if let Some(detail) = status.detail() {
                log::warn!("{detail}");
            }
            return (None, Some(status));
        }
    };

    let tags = classify_windows(audio, &spans, &library, &model, program);
    (tags, None)
}

/// Tag PCM using an already-computed CED verdict.
pub fn tag_audio_with_readiness(
    audio: &[f32],
    readiness: CedVerdict,
) -> (Option<Value>, Option<CapabilityStatus>) {
    tag_audio_with_readiness_and_program(audio, readiness, &CedAnalyzeProgram::SiblingHelper)
}

/// [`tag_audio_with_readiness`] with caller-specified helper program.
pub fn tag_audio_with_readiness_and_program(
    audio: &[f32],
    readiness: CedVerdict,
    program: &CedAnalyzeProgram,
) -> (Option<Value>, Option<CapabilityStatus>) {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return (None, None);
    }

    let (library, model) = match readiness {
        CedVerdict::Ready { library, model } => (library, model),
        CedVerdict::Unsupported { os, arch } => {
            log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
            return (None, None);
        }
        CedVerdict::Degraded(status) => {
            if let Some(detail) = status.detail() {
                log::warn!("{detail}");
            }
            return (None, Some(status));
        }
    };
    let tags = classify_windows(audio, &spans, &library, &model, program);
    (tags, None)
}

fn classify_windows(
    audio: &[f32],
    spans: &[(usize, usize)],
    library: &Path,
    model: &Path,
    program: &CedAnalyzeProgram,
) -> Option<Value> {
    let temporary = match tempfile::Builder::new()
        .prefix(CED_ANALYZE_TEMP_PREFIX)
        .tempdir()
    {
        Ok(directory) => directory,
        Err(error) => {
            log::warn!("sound tagging could not prepare audio: {error}");
            return None;
        }
    };
    let audio_path = temporary.path().join("audio.f32le");
    if let Err(error) = write_audio_sidecar(&audio_path, audio) {
        log::warn!("sound tagging could not prepare audio: {error}");
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
    #[cfg(windows)]
    let temporary = std::sync::Arc::new(temporary);
    #[cfg(windows)]
    let mut resources = solstone_core_check::ced_windows::BoundedHelperResources::new();
    #[cfg(windows)]
    resources.retain(temporary.clone());
    #[cfg(windows)]
    let result = solstone_core_check::ced_windows::invoke(
        program,
        &[],
        &request,
        CED_ANALYZE_TIMEOUT,
        resources,
    );
    #[cfg(not(windows))]
    let result = invoke_ced_analyze(program, &request, CED_ANALYZE_TIMEOUT);
    let response = match result {
        Ok(response) => response,
        Err(error) => {
            log::warn!("sound tagging failed: {error}");
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

/// Windows kept from one ced response.
///
/// `ok: false` windows are omitted. An empty [`SoundTagWindows::Accepted`]
/// means every window failed. [`SoundTagWindows::Rejected`] means the schema,
/// the window count, or an `ok` field was unusable.
#[derive(Debug, PartialEq)]
pub enum SoundTagWindows {
    Accepted(Vec<Value>),
    Rejected,
}

/// Split one classify response the way desktop tagging does.
///
/// Schema, count, and missing-`ok` failures are logged and rejected. A window
/// with `ok: false` is skipped. When every window failed, this logs once and
/// returns an empty accepted list.
pub fn read_sound_tag_windows(response: &Value, expected_len: usize) -> SoundTagWindows {
    if response.get("schema").and_then(Value::as_str) != Some(RESPONSE_SCHEMA) {
        log::warn!("sound tagging returned an unexpected response schema");
        return SoundTagWindows::Rejected;
    }
    let windows = match response.get("windows").and_then(Value::as_array) {
        Some(windows) if windows.len() == expected_len => windows,
        _ => {
            log::warn!("sound tagging returned an unexpected window count");
            return SoundTagWindows::Rejected;
        }
    };

    let mut per_window = Vec::new();
    let mut first_failure = None;
    for window in windows {
        match window.get("ok").and_then(Value::as_bool) {
            Some(true) => per_window.push(window.clone()),
            Some(false) => {
                if first_failure.is_none() {
                    first_failure = window
                        .get("detail")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            None => {
                log::warn!("sound tagging returned a window missing an ok field");
                return SoundTagWindows::Rejected;
            }
        }
    }

    if per_window.is_empty()
        && let Some(detail) = first_failure
    {
        log::warn!("sound tagging failed for all windows: {detail}");
    }
    SoundTagWindows::Accepted(per_window)
}

/// Fold one accepted window into the per-label maximum. Scores under [`SCORE_FLOOR`] are ignored.
pub fn accumulate_sound_tag_window(scores: &mut BTreeMap<String, f64>, window: &Value) {
    let Some(tags) = window.get("tags").and_then(Value::as_object) else {
        return;
    };
    for (label, score) in tags {
        let Some(score) = score.as_f64() else {
            continue;
        };
        if score < SCORE_FLOOR {
            continue;
        }
        let entry = scores.entry(label.clone()).or_insert(0.0);
        if score > *entry {
            *entry = score;
        }
    }
}

/// The published sound-tag object from a max accumulator and the accepted-window count.
pub fn fold_sound_tag_scores(scores: &BTreeMap<String, f64>, windows: usize) -> Value {
    let mut tags_map = Map::new();
    for (label, score) in scores {
        tags_map.insert(label.clone(), json!(score));
    }
    json!({
        "engine": ENGINE,
        "model": MODEL,
        "threshold": SCORE_FLOOR,
        "window_s": WINDOW_S,
        "agg": AGG,
        "windows": windows,
        "tags": Value::Object(tags_map),
    })
}

/// Max score at or above [`SCORE_FLOOR`] for each label, then the published object.
///
/// `windows` are the accepted (`ok: true`) window objects. A lower score does
/// not replace a higher one. The object is the desktop sound-tag record.
pub fn fold_sound_tag_windows(windows: &[Value]) -> Value {
    let mut aggregated: BTreeMap<String, f64> = BTreeMap::new();
    for window in windows {
        accumulate_sound_tag_window(&mut aggregated, window);
    }
    fold_sound_tag_scores(&aggregated, windows.len())
}

fn windows_from_response(response: &Value, expected_len: usize) -> Option<Value> {
    match read_sound_tag_windows(response, expected_len) {
        SoundTagWindows::Rejected => None,
        SoundTagWindows::Accepted(windows) if windows.is_empty() => None,
        SoundTagWindows::Accepted(windows) => Some(fold_sound_tag_windows(&windows)),
    }
}

pub fn window_spans(sample_count: usize) -> Vec<(usize, usize)> {
    let window_samples = WINDOW_S * CLASSIFY_SAMPLE_RATE as usize;
    let min_tail_samples = MIN_TAIL_S * CLASSIFY_SAMPLE_RATE as usize;
    if sample_count < window_samples {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut start = 0;
    while start + window_samples <= sample_count {
        spans.push((start, start + window_samples));
        start += window_samples;
    }
    if sample_count - start >= min_tail_samples {
        spans.push((sample_count.saturating_sub(window_samples), sample_count));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_fold_keeps_the_higher_score_at_the_floor() {
        let response = json!({
            "schema": RESPONSE_SCHEMA,
            "windows": [
                {"ok": true, "tags": {"Speech": SCORE_FLOOR, "Music": SCORE_FLOOR - 0.01}},
                {"ok": true, "tags": {"Speech": 0.42, "Dog": SCORE_FLOOR}},
                {"ok": false, "detail": "skip"},
                {"ok": true, "tags": {"Speech": 0.2, "Dog": 0.8}}
            ]
        });
        let from_response = windows_from_response(&response, 4).expect("accepted windows");
        let SoundTagWindows::Accepted(accepted) = read_sound_tag_windows(&response, 4) else {
            panic!("response rejected");
        };
        let folded = fold_sound_tag_windows(&accepted);
        assert_eq!(from_response, folded);
        assert_eq!(folded["windows"], 3);
        assert_eq!(folded["agg"], AGG);
        assert_eq!(folded["threshold"], SCORE_FLOOR);
        assert_eq!(folded["tags"]["Speech"].as_f64(), Some(0.42));
        assert_eq!(folded["tags"]["Dog"].as_f64(), Some(0.8));
        assert!(folded["tags"].get("Music").is_none());
        assert!(folded["tags"].get("Speech").is_some());
    }
}
