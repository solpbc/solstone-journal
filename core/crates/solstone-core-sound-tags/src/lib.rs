// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Best-effort ambient sound tagging over the runtime-installed ced.cpp engine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use solstone_core_ced_sys::CedLibrary;

pub const SCORE_FLOOR: f64 = 0.1;
pub const WINDOW_S: usize = 10;
pub const MIN_TAIL_S: usize = 1;
pub const CLASSIFY_SAMPLE_RATE: i32 = 16_000;
pub const ABI_VERSION: i32 = 1;
pub const ENGINE: &str = "ced.cpp v0.1.0";
pub const MODEL: &str = "ced-tiny-q8_0";
pub const AGG: &str = "max";
pub const MODEL_REVISION: &str = "b5e9a4aad6438763c8da16079d77563fbed35c65";

const ENGINE_VERSION: &str = "v0.1.0";
const MODEL_REPOSITORY_DIRECTORY: &str = "mudler__ced-gguf";
const MODEL_SIZE_BYTES: u64 = 6_211_616;

/// Tag PCM audio using the locally installed ced.cpp model.
///
/// All tagger failures are best-effort and therefore represented as `None`.
pub fn tag_audio(audio: &[f32], journal_path: &Path) -> Option<Value> {
    let spans = window_spans(audio.len());
    if spans.is_empty() {
        return None;
    }

    let paths = match asset_paths(journal_path) {
        Ok(paths) => paths,
        Err(detail) => {
            log::warn!("sound tagger disabled: {detail}");
            return None;
        }
    };
    let library = match CedLibrary::open(&paths.library) {
        Ok(library) => library,
        Err(error) => {
            log::warn!("sound tagger disabled: {error}");
            return None;
        }
    };
    let context = match library.load_model(&paths.model) {
        Ok(context) => context,
        Err(error) => {
            log::warn!("sound tagger disabled: ced model load failed: {error}");
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

fn asset_paths(journal_path: &Path) -> Result<AssetPaths, String> {
    let artifact = artifact_key().ok_or_else(|| "ced assets unsupported here".to_owned())?;
    let root = journal_path
        .join("cache")
        .join("providers")
        .join("ced")
        .join(ENGINE_VERSION);
    let library_name = if std::env::consts::OS == "macos" {
        "libced.dylib"
    } else {
        "libced.so"
    };
    let library = root.join("engine").join(artifact).join(library_name);
    let header = root.join("engine").join(artifact).join("ced_capi.h");
    let model = root
        .join("models")
        .join(MODEL_REPOSITORY_DIRECTORY)
        .join(MODEL_REVISION)
        .join("ced-tiny-q8_0.gguf");

    require_nonempty(&library, "ced engine library")?;
    require_nonempty(&header, "ced C API header")?;
    let metadata =
        fs::metadata(&model).map_err(|_| format!("ced model missing: {}", model.display()))?;
    if !metadata.is_file() {
        return Err(format!("ced model missing: {}", model.display()));
    }
    if metadata.len() != MODEL_SIZE_BYTES {
        return Err(format!(
            "ced model size mismatch: expected {MODEL_SIZE_BYTES}, got {}",
            metadata.len()
        ));
    }
    Ok(AssetPaths { library, model })
}

fn require_nonempty(path: &Path, label: &str) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|_| format!("{label} missing: {}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{label} missing: {}", path.display()));
    }
    if metadata.len() == 0 {
        return Err(format!("{label} is empty: {}", path.display()));
    }
    Ok(())
}

fn artifact_key() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("linux-cpu-x64"),
        ("linux", "aarch64") => Some("linux-cpu-arm64"),
        ("macos", "aarch64") => Some("macos-metal-arm64"),
        _ => None,
    }
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

struct AssetPaths {
    library: PathBuf,
    model: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::{aggregate, window_spans, MIN_TAIL_S, SCORE_FLOOR, WINDOW_S};
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
