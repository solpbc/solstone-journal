// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort ambient sound tagging over the runtime-installed ced.cpp engine.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_assets::canonical_host_pair;
use solstone_core_ced_sys::CedLibrary;
use solstone_core_local::install::ced_readiness::{
    CED_UNAVAILABLE_GUIDANCE, CedVerdict, evaluate_ced_readiness,
};

pub const SCORE_FLOOR: f64 = 0.1;
pub const WINDOW_S: usize = 10;
pub const MIN_TAIL_S: usize = 1;
pub const CLASSIFY_SAMPLE_RATE: i32 = 16_000;
pub const ABI_VERSION: i32 = 1;
pub const ENGINE: &str = "ced.cpp v0.1.0";
pub const MODEL: &str = "ced-tiny-q8_0";
pub const AGG: &str = "max";

/// Tag PCM audio using the locally installed ced.cpp model.
///
/// All tagger failures are best-effort and therefore represented as `None`.
pub fn tag_audio(audio: &[f32], journal_path: &Path) -> Option<Value> {
    let (os, arch) = canonical_host_pair(std::env::consts::OS, std::env::consts::ARCH);
    // The shared verdict hashes the model and load-probes once. This call
    // then opens a working handle of its own: the verdict is the gate, not a
    // second path-derivation, and transcribe invokes this once per audio file
    // (`process_one`), so the extra dlopen/load is not a hot path.
    tag_audio_with_readiness(audio, evaluate_ced_readiness(journal_path, os, arch))
}

/// Tag PCM using an already-computed CED verdict.
///
/// Production [`tag_audio`] supplies the catalog verdict. Tests supply a
/// verdict against a fixture digest so classify can run without the 6 MiB pin.
pub fn tag_audio_with_readiness(audio: &[f32], verdict: CedVerdict) -> Option<Value> {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return None;
    }

    let (library, model) = match verdict {
        CedVerdict::Ready { library, model } => (library, model),
        CedVerdict::Unsupported { os, arch } => {
            log::warn!("sound tagger disabled: ced assets unsupported on {os}/{arch}");
            return None;
        }
        CedVerdict::Degraded(status) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced status: {status:?}");
            return None;
        }
    };
    let library = match CedLibrary::open(&library) {
        Ok(library) => library,
        Err(error) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced engine open failed: {error}");
            return None;
        }
    };
    let context = match library.load_model(&model) {
        Ok(context) => context,
        Err(error) => {
            log::warn!("{CED_UNAVAILABLE_GUIDANCE}");
            log::debug!("ced model load failed: {error}");
            return None;
        }
    };

    let mut per_window = Vec::new();
    let mut first_failure = None;
    for (index, (start, end)) in spans.into_iter().enumerate() {
        match context.classify_pcm_json(&audio[start..end], CLASSIFY_SAMPLE_RATE, 0) {
            Ok(raw) => match parse_classify_json(&raw) {
                Ok(tags) => per_window.push(tags),
                Err(detail) => {
                    log::debug!("sound tagger window {index} failed: {detail}");
                    first_failure.get_or_insert(detail);
                }
            },
            Err(error) => {
                let detail = error.to_string();
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

fn parse_classify_json(raw: &str) -> Result<BTreeMap<String, f64>, String> {
    let data: Value = serde_json::from_str(raw)
        .map_err(|error| format!("ced classify JSON was invalid: {error}"))?;
    let entries = data
        .as_array()
        .ok_or_else(|| "ced classify JSON must be an array".to_owned())?;
    let mut tags: BTreeMap<String, f64> = BTreeMap::new();
    for item in entries {
        let object = item
            .as_object()
            .ok_or_else(|| "ced classify JSON entries must be objects".to_owned())?;
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "ced classify JSON entry label must be a non-empty string".to_owned())?;
        let score = object
            .get("score")
            .and_then(Value::as_f64)
            .ok_or_else(|| "ced classify JSON entry score must be numeric".to_owned())?;
        tags.entry(label.to_owned())
            .and_modify(|current| *current = current.max(score))
            .or_insert(score);
    }
    Ok(tags)
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
    use super::{MIN_TAIL_S, SCORE_FLOOR, WINDOW_S, aggregate, window_spans};
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
}
