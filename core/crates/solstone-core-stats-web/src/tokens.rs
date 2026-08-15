// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::pricing::{calc_token_cost, provider};
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
    let mut models: BTreeMap<String, ModelBucket> = BTreeMap::new();
    let mut contexts: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut segments: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut types: BTreeMap<String, SimpleBucket> = BTreeMap::new();
    let mut token_types = BTreeMap::from([
        ("input", TokenBucket::default()),
        ("output", TokenBucket::default()),
        ("cached", TokenBucket::default()),
        ("reasoning", TokenBucket::default()),
    ]);
    let mut total_requests = 0_u64;
    let mut total_tokens = 0_f64;
    let mut total_cost = 0.0;
    let mut skipped_unknown = 0_u64;
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
        let number = |name: &str| usage.get(name).and_then(Value::as_f64).unwrap_or(0.0);
        let input = number("input_tokens");
        let output = number("output_tokens");
        let cached = number("cached_tokens");
        let reasoning = number("reasoning_tokens");
        let tokens = number("total_tokens");
        let costs = calc_token_cost(&entry)
            .unwrap_or_else(|| json!({"total_cost":0.0,"input_cost":0.0,"output_cost":0.0}));
        let cost = costs["total_cost"].as_f64().unwrap_or(0.0);
        let input_cost = costs["input_cost"].as_f64().unwrap_or(0.0);
        let output_cost = costs["output_cost"].as_f64().unwrap_or(0.0);
        let provider = provider(model);
        if provider == "unknown" {
            skipped_unknown += 1;
            continue;
        }
        total_requests += 1;
        total_tokens += tokens;
        total_cost += cost;
        let bucket = providers.entry(provider.to_owned()).or_default();
        bucket.add(tokens, cost, model);
        bucket.input += input;
        bucket.output += output;
        bucket.cached += cached;
        bucket.reasoning += reasoning;
        bucket.input_cost += input_cost;
        bucket.output_cost += output_cost;
        let bucket = models
            .entry(model.to_owned())
            .or_insert_with(|| ModelBucket {
                provider: provider.to_owned(),
                ..Default::default()
            });
        bucket.add(tokens, cost);
        let context = normalize_legacy_context(
            entry
                .get("context")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        );
        contexts
            .entry(if context.is_empty() {
                "unknown".to_owned()
            } else {
                context.to_owned()
            })
            .or_default()
            .add(tokens, cost, model);
        let segment = entry
            .get("segment")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or("[unattributed]");
        segments
            .entry(segment.to_owned())
            .or_default()
            .add(tokens, cost, model);
        if let Some(kind) = entry
            .get("type")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            types.entry(kind.to_owned()).or_default().add(tokens, cost);
        }
        token_types
            .get_mut("input")
            .expect("input")
            .add(input, input_cost);
        token_types
            .get_mut("output")
            .expect("output")
            .add(output + reasoning, output_cost);
        token_types.get_mut("cached").expect("cached").tokens += cached;
        token_types.get_mut("reasoning").expect("reasoning").tokens += reasoning;
    }
    let provider_list = sorted(providers.into_iter().map(|(name, b)| json!({"provider":name,"requests":b.requests,"tokens":number_value(b.tokens),"input_tokens":number_value(b.input),"output_tokens":number_value(b.output),"cached_tokens":number_value(b.cached),"reasoning_tokens":number_value(b.reasoning),"cost":round(b.cost,2),"input_cost":round(b.input_cost,2),"output_cost":round(b.output_cost,2),"models_used":b.models,"percent":number_value(round(percent(b.cost,total_cost),1))})).collect());
    let model_list = sorted(models.into_iter().map(|(name, b)| json!({"model":name,"provider":b.provider,"requests":b.requests,"tokens":number_value(b.tokens),"cost":round(b.cost,2),"avg_cost_per_request":round(if b.requests > 0 { b.cost / b.requests as f64 } else { 0.0 },2),"percent":number_value(round(percent(b.cost,total_cost),1))})).collect());
    let context_list = sorted(
        contexts
            .into_iter()
            .map(|(name, b)| display_bucket("context", name, b, total_cost))
            .collect(),
    );
    let segment_list = sorted(
        segments
            .into_iter()
            .map(|(name, b)| display_bucket("segment", name, b, total_cost))
            .collect(),
    );
    let segment_count = segment_list
        .iter()
        .filter(|entry| entry["segment"] != "[unattributed]")
        .count();
    let input_total = token_types["input"].tokens;
    let output_total = token_types["output"].tokens;
    let cached_total = token_types["cached"].tokens;
    let reasoning_total = token_types["reasoning"].tokens;
    let mut type_json = Map::new();
    for (name, b) in token_types {
        let mut value = json!({"tokens":number_value(b.tokens),"cost":b.cost});
        if name == "input" {
            value["cached_pct"] = json!(round(
                if input_total > 0.0 {
                    cached_total / input_total * 100.0
                } else {
                    0.0
                },
                1
            ));
        }
        if name == "output" {
            value["reasoning_pct"] = json!(round(
                if output_total > 0.0 {
                    reasoning_total / output_total * 100.0
                } else {
                    0.0
                },
                1
            ));
        }
        if name == "input" || name == "output" {
            value["avg_rate"] = json!(round(
                if b.tokens > 0.0 {
                    b.cost / b.tokens * 1000.0
                } else {
                    0.0
                },
                4
            ));
            value["percent"] = number_value(round(percent(b.cost, total_cost), 1));
            value["cost"] = json!(round(b.cost, 2));
        }
        type_json.insert(name.to_owned(), value);
    }
    let by_type = types.into_iter().map(|(name,b)| (name,json!({"requests":b.requests,"tokens":number_value(b.tokens),"cost":round(b.cost,2)}))).collect::<Map<_,_>>();
    json!({"day":day,"total":{"requests":total_requests,"tokens":number_value(total_tokens),"cost":round(total_cost,2),"segment_count":segment_count,"skipped_unknown":skipped_unknown},"by_provider":provider_list,"by_model":model_list,"by_token_type":type_json,"by_context":context_list,"by_segment":segment_list,"by_type":by_type})
}

pub(super) fn cost_stats(root: &Path, month: Option<&str>) -> Result<Value, String> {
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
        let cost = aggregate(root, day)?["total"]["cost"]
            .as_f64()
            .unwrap_or(0.0);
        if cost > 0.0 {
            rows.insert(day.to_owned(), json!(cost));
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
        right["cost"]
            .as_f64()
            .unwrap_or_default()
            .partial_cmp(&left["cost"].as_f64().unwrap_or_default())
            .expect("finite costs")
    });
    values
}
fn display_bucket(key: &str, name: String, bucket: Bucket, total: f64) -> Value {
    json!({key:name,"requests":bucket.requests,"tokens":number_value(bucket.tokens),"cost":round(bucket.cost,2),"models_used":bucket.models,"percent":number_value(round(percent(bucket.cost,total),1))})
}
#[derive(Default)]
struct Bucket {
    requests: u64,
    tokens: f64,
    cost: f64,
    input: f64,
    output: f64,
    cached: f64,
    reasoning: f64,
    input_cost: f64,
    output_cost: f64,
    models: BTreeSet<String>,
}
impl Bucket {
    fn add(&mut self, tokens: f64, cost: f64, model: &str) {
        self.requests += 1;
        self.tokens += tokens;
        self.cost += cost;
        self.models.insert(model.to_owned());
    }
}
#[derive(Default)]
struct ModelBucket {
    provider: String,
    requests: u64,
    tokens: f64,
    cost: f64,
}
impl ModelBucket {
    fn add(&mut self, tokens: f64, cost: f64) {
        self.requests += 1;
        self.tokens += tokens;
        self.cost += cost;
    }
}
#[derive(Default)]
struct SimpleBucket {
    requests: u64,
    tokens: f64,
    cost: f64,
}
impl SimpleBucket {
    fn add(&mut self, tokens: f64, cost: f64) {
        self.requests += 1;
        self.tokens += tokens;
        self.cost += cost;
    }
}
#[derive(Default)]
struct TokenBucket {
    tokens: f64,
    cost: f64,
}
impl TokenBucket {
    fn add(&mut self, tokens: f64, cost: f64) {
        self.tokens += tokens;
        self.cost += cost;
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
    fn unknown_entries_skip_all_breakdowns_and_reasoning_is_billed_as_output() {
        let data = aggregate_entries(
            "20260809",
            vec![
                json!({"model":"unknown-model","type":"generate","segment":"kept-out","usage":{"total_tokens":99,"input_tokens":99}}),
                json!({"model":"local/test","segment":"","usage":{"total_tokens":10,"output_tokens":3,"reasoning_tokens":2}}),
            ],
        );
        assert_eq!(data["total"]["requests"], 1);
        assert_eq!(data["total"]["skipped_unknown"], 1);
        assert_eq!(data["by_type"], json!({}));
        assert_eq!(data["by_segment"][0]["segment"], "[unattributed]");
        assert_eq!(data["total"]["segment_count"], 0);
        assert_eq!(data["by_token_type"]["output"]["tokens"], 5);
        assert_eq!(data["by_token_type"]["reasoning"]["tokens"], 2);
        assert_eq!(data["by_token_type"]["reasoning"]["cost"], 0.0);
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

    #[test]
    fn rounding_rates_segments_and_sort_follow_the_reference() {
        let data = aggregate_entries(
            "20260809",
            vec![
                json!({"model":"gpt-5.5","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}),
                json!({"model":"claude-sonnet-4-6","segment":"s","usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}),
            ],
        );
        assert_eq!(data["total"]["segment_count"], 1);
        assert!(
            data["by_segment"]
                .as_array()
                .expect("segments")
                .iter()
                .any(|entry| entry["segment"] == "[unattributed]")
        );
        assert!(data["by_provider"][0]["cost"].as_f64() >= data["by_provider"][1]["cost"].as_f64());
        let scaled_rate = data["by_token_type"]["input"]["avg_rate"]
            .as_f64()
            .expect("rate")
            * 10_000.0;
        assert!((scaled_rate - scaled_rate.round()).abs() < 0.000_001);
        assert_eq!(number_value(-2.0), json!(-2));
    }
}
