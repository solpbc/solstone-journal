// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value, json};
use solstone_core_journal_io::{DirEntryKind, list_dir_entries, read_text, resolve_journal_path};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub(super) fn aggregate(root: &Path, day: &str) -> Result<Value, String> {
    let entries = read_token_log(root, day)?;
    Ok(aggregate_entries(day, entries))
}

pub(super) fn aggregate_entries(day: &str, entries: Vec<Value>) -> Value {
    let mut providers: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut models: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut contexts: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut segments: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut types: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut total = Bucket::default();
    for entry in entries {
        let model = entry
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let usage = entry
            .get("usage")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let context = entry
            .get("context")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown");
        let segment = entry
            .get("segment")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or("[unattributed]");
        // Model-name recognition is a label only; it must never discard measured usage.
        let provider = provider(model);
        total.add(&usage, model);
        providers
            .entry(provider.to_owned())
            .or_default()
            .add(&usage, model);
        models
            .entry(model.to_owned())
            .or_default()
            .add(&usage, model);
        contexts
            .entry(normalize_legacy_context(context).to_owned())
            .or_default()
            .add(&usage, model);
        segments
            .entry(segment.to_owned())
            .or_default()
            .add(&usage, model);
        if let Some(kind) = entry
            .get("type")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            types.entry(kind.to_owned()).or_default().add(&usage, model);
        }
    }
    let segment_count = segments
        .keys()
        .filter(|key| key.as_str() != "[unattributed]")
        .count();
    let total_tokens = total.tokens;
    let list = |key: &str, buckets: BTreeMap<String, Bucket>| {
        sorted(
            buckets
                .into_iter()
                .map(|(name, bucket)| display_bucket(key, name, bucket, total_tokens))
                .collect(),
        )
    };
    let mut model_list = list("model", models);
    for row in &mut model_list {
        row["provider"] = json!(provider(row["model"].as_str().expect("model")));
    }
    let by_type = types
        .into_iter()
        .map(|(name, bucket)| {
            (
                name,
                json!({"requests":bucket.requests,"tokens":number_value(bucket.tokens)}),
            )
        })
        .collect::<Map<_, _>>();
    json!({"day":day,"total":{"requests":total.requests,"tokens":number_value(total.tokens),"segment_count":segment_count},
        "by_provider":list("provider",providers),"by_model":model_list,
        "by_context":list("context",contexts),"by_segment":list("segment",segments),"by_type":by_type,
        "by_token_type":{"input":{"tokens":number_value(total.input)},"output":{"tokens":number_value(total.output)},
            "cached":{"tokens":number_value(total.cached)},"reasoning":{"tokens":number_value(total.reasoning)}}})
}

pub(super) fn usage_stats(root: &Path, month: Option<&str>) -> Result<Value, String> {
    let tokens = resolve_journal_path(root, "tokens").map_err(|e| e.to_string())?;
    let entries = list_dir_entries(&tokens).map_err(|error| error.to_string())?;
    let mut rows = Map::new();
    for entry in entries {
        if entry.kind != DirEntryKind::File {
            continue;
        }
        let path = entry.path;
        let Some(day) = path.file_stem().and_then(|v| v.to_str()) else {
            continue;
        };
        if !is_day(day) || month.is_some_and(|value| !day.starts_with(value)) {
            continue;
        }
        let usage = aggregate(root, day)?;
        if usage["total"]["requests"].as_u64().unwrap_or(0) > 0 {
            rows.insert(day.to_owned(), usage["total"]["tokens"].clone());
        }
    }
    Ok(Value::Object(rows))
}

pub(super) fn read_token_log(root: &Path, day: &str) -> Result<Vec<Value>, String> {
    let path =
        resolve_journal_path(root, &format!("tokens/{day}.jsonl")).map_err(|e| e.to_string())?;
    let text = read_text(path, String::new()).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(value) if value.is_object() => entries.push(value),
            Ok(_) => return Err("token log entry is not an object".to_owned()),
            Err(_) => continue,
        }
    }
    Ok(entries)
}

/// Python's legacy and current talent prefixes are both `talent.` today.
pub(super) fn normalize_legacy_context(context: &str) -> &str {
    context
}
fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn round(value: f64, decimals: i32) -> f64 {
    let factor = 10_f64.powi(decimals);
    (value * factor).round() / factor
}
fn percent(value: f64, total: f64) -> f64 {
    if total > 0.0 {
        value / total * 100.0
    } else {
        0.0
    }
}
fn number_value(value: f64) -> Value {
    if value.fract() == 0.0 {
        if value < 0.0 {
            json!(value as i64)
        } else {
            json!(value as u64)
        }
    } else {
        json!(value)
    }
}
fn sorted(mut values: Vec<Value>) -> Vec<Value> {
    values.sort_by(|left, right| {
        right["tokens"]
            .as_f64()
            .unwrap_or_default()
            .total_cmp(&left["tokens"].as_f64().unwrap_or_default())
    });
    values
}
fn display_bucket(key: &str, name: String, bucket: Bucket, total: f64) -> Value {
    json!({key:name,"requests":bucket.requests,"tokens":number_value(bucket.tokens),"input_tokens":number_value(bucket.input),
        "output_tokens":number_value(bucket.output),"cached_tokens":number_value(bucket.cached),"reasoning_tokens":number_value(bucket.reasoning),
        "models_used":bucket.models,"percent":number_value(round(percent(bucket.tokens,total),1))})
}
#[derive(Default)]
struct Bucket {
    requests: u64,
    tokens: f64,
    input: f64,
    output: f64,
    cached: f64,
    reasoning: f64,
    models: BTreeSet<String>,
}
impl Bucket {
    fn add(&mut self, usage: &Map<String, Value>, model: &str) {
        let number = |key: &str| usage.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        self.requests += 1;
        self.tokens += number("total_tokens");
        self.input += number("input_tokens");
        self.output += number("output_tokens");
        self.cached += number("cached_tokens");
        self.reasoning += number("reasoning_tokens");
        self.models.insert(model.to_owned());
    }
}

pub(super) fn provider(model: &str) -> &'static str {
    let model = model.to_ascii_lowercase();
    if model == "local"
        || model.starts_with("local/")
        || model == "qwen3.5:9b"
        || model == "gemma-4-26b-a4b-it-mlx-4bit"
    {
        "local"
    } else if model.starts_with("gpt") {
        "openai"
    } else if model.starts_with("claude") {
        "anthropic"
    } else if model.starts_with("gemini") {
        "google"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn legacy_context_is_identity() {
        assert_eq!(
            normalize_legacy_context("talent.system.work"),
            "talent.system.work"
        );
    }

    #[test]
    fn unknown_models_keep_measured_usage_in_every_breakdown() {
        let data = aggregate_entries(
            "20260904",
            vec![
                json!({"model":"my-engine","context":"talent.work","type":"generate","segment":"sample","usage":{"total_tokens":99,"input_tokens":90,"cached_tokens":20,"output_tokens":9,"reasoning_tokens":2}}),
                json!({"model":"local/test","usage":{"total_tokens":10,"output_tokens":3,"reasoning_tokens":2}}),
            ],
        );
        assert_eq!(data["total"]["requests"], 2);
        assert_eq!(data["total"]["tokens"], 109);
        assert_eq!(data["total"]["segment_count"], 1);
        for key in ["by_provider", "by_model", "by_context", "by_segment"] {
            assert_eq!(data[key][0]["tokens"], 99, "{key}");
            assert_eq!(data[key][0]["cached_tokens"], 20, "{key}");
        }
        assert_eq!(data["by_type"]["generate"]["tokens"], 99);
        assert_eq!(data["by_token_type"]["output"]["tokens"], 12);
        assert_eq!(data["by_token_type"]["reasoning"]["tokens"], 4);
        assert!(data["total"].get("cost").is_none());
    }

    #[test]
    fn coverage_includes_local_and_unrecognized_models() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        for (day, model, tokens) in [
            ("20260903", "local/test", 12),
            ("20260904", "my-engine", 34),
        ] {
            fs::write(
                root.path().join(format!("tokens/{day}.jsonl")),
                json!({"model":model,"usage":{"total_tokens":tokens}}).to_string(),
            )
            .expect("write");
        }
        assert_eq!(
            usage_stats(root.path(), None).expect("stats"),
            json!({"20260903":12,"20260904":34})
        );
        assert_eq!(
            usage_stats(root.path(), Some("202608")).expect("month"),
            json!({})
        );
    }

    #[test]
    fn reader_skips_blank_and_malformed_but_rejects_a_valid_non_object() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        fs::write(root.path().join("tokens/20260809.jsonl"), "{\"model\":\"local/a\",\"usage\":{}}\n\nnot json\n{\"model\":\"local/b\",\"usage\":{}}\n").expect("log");
        assert_eq!(
            read_token_log(root.path(), "20260809").expect("log").len(),
            2
        );
        fs::write(root.path().join("tokens/20260809.jsonl"), "[]\n").expect("scalar");
        assert!(read_token_log(root.path(), "20260809").is_err());
        assert!(
            read_token_log(root.path(), "20260810")
                .expect("missing")
                .is_empty()
        );
    }
}
