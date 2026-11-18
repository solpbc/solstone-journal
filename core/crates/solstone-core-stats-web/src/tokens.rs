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
        // Group by the canonical id so a model that self-reports two different
        // raw identifiers (e.g. a local server's own wire name vs our install
        // id) still lands as one row with one provider (G2-31).
        let canonical = canonical_model(model);
        let provider = provider(&canonical);
        total.add(&usage, model);
        providers
            .entry(provider.to_owned())
            .or_default()
            .add(&usage, model);
        models.entry(canonical).or_default().add(&usage, model);
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
        let (requests, tokens) = fold_day_tokens(root, day)?;
        if requests > 0 {
            rows.insert(day.to_owned(), number_value(tokens));
        }
    }
    Ok(Value::Object(rows))
}

pub(super) fn read_token_log(root: &Path, day: &str) -> Result<Vec<Value>, String> {
    let text = read_token_log_text(root, day)?;
    let mut entries = Vec::new();
    for line in text.lines() {
        if let Some(entry) = parse_token_line(line)? {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn read_token_log_text(root: &Path, day: &str) -> Result<String, String> {
    let path =
        resolve_journal_path(root, &format!("tokens/{day}.jsonl")).map_err(|e| e.to_string())?;
    read_text(path, String::new()).map_err(|e| e.to_string())
}

fn parse_token_line(line: &str) -> Result<Option<Value>, String> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(line) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        Ok(_) => Err("token log entry is not an object".to_owned()),
        Err(_) => Ok(None),
    }
}

fn fold_day_tokens(root: &Path, day: &str) -> Result<(u64, f64), String> {
    let text = read_token_log_text(root, day)?;
    let mut requests = 0u64;
    let mut tokens = 0.0f64;
    for line in text.lines() {
        let Some(entry) = parse_token_line(line)? else {
            continue;
        };
        requests += 1;
        tokens += entry
            .get("usage")
            .and_then(Value::as_object)
            .map(|usage| usage_number(usage, "total_tokens"))
            .unwrap_or(0.0);
    }
    Ok((requests, tokens))
}

fn usage_number(usage: &Map<String, Value>, key: &str) -> f64 {
    usage.get(key).and_then(Value::as_f64).unwrap_or(0.0)
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
        self.requests += 1;
        self.tokens += usage_number(usage, "total_tokens");
        self.input += usage_number(usage, "input_tokens");
        self.output += usage_number(usage, "output_tokens");
        self.cached += usage_number(usage, "cached_tokens");
        self.reasoning += usage_number(usage, "reasoning_tokens");
        self.models.insert(model.to_owned());
    }
}

/// A local model occasionally self-reports a raw wire id that differs from
/// the canonical `local/<slug>` id we install and label it under (G2-31:
/// `unsloth/Qwen3.5-4B-GGUF`'s runtime reports the base model's HF repo id,
/// `Qwen/Qwen3.5-4B`, on some completions and our own `local/qwen3.5-4b` on
/// others). Both name the same physical model; fold the known raw form to
/// the canonical id before grouping so the dashboard doesn't split one
/// model's usage across two rows under two different providers. Unknown
/// models pass through unchanged — this never invents an id.
pub(super) fn canonical_model(model: &str) -> String {
    match model.to_ascii_lowercase().as_str() {
        "qwen/qwen3.5-4b" => "local/qwen3.5-4b".to_owned(),
        _ => model.to_owned(),
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
    fn raw_and_canonical_local_model_ids_merge_into_one_row_under_one_provider() {
        // G2-31: the same physical local model self-reports under two ids on
        // different completions. Both must collapse to one `by_model` row,
        // with combined usage, a `local` provider (not `unknown`), and both
        // raw ids preserved for a disclosure.
        let data = aggregate_entries(
            "20260904",
            vec![
                json!({"model":"Qwen/Qwen3.5-4B","usage":{"total_tokens":195,"input_tokens":190,"output_tokens":5}}),
                json!({"model":"local/qwen3.5-4b","usage":{"total_tokens":4,"input_tokens":3,"output_tokens":1}}),
            ],
        );
        assert_eq!(data["by_model"].as_array().expect("by_model").len(), 1);
        let row = &data["by_model"][0];
        assert_eq!(row["model"], "local/qwen3.5-4b");
        assert_eq!(row["provider"], "local");
        assert_eq!(row["requests"], 2);
        assert_eq!(row["tokens"], 199);
        let models_used = row["models_used"]
            .as_array()
            .expect("models_used")
            .iter()
            .map(|v| v.as_str().expect("model id"))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            models_used,
            BTreeSet::from(["Qwen/Qwen3.5-4B", "local/qwen3.5-4b"])
        );
        assert_eq!(
            data["by_provider"].as_array().expect("by_provider").len(),
            1
        );
        assert_eq!(data["by_provider"][0]["provider"], "local");
        assert_eq!(data["by_provider"][0]["tokens"], 199);
    }

    #[test]
    fn canonical_model_passes_through_unrecognized_ids() {
        assert_eq!(canonical_model("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(canonical_model("my-engine"), "my-engine");
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

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::io::Write;
    use tower::ServiceExt;

    fn write_token_log(root: &Path, name: &str, content: &str) {
        let tokens_dir = root.join("tokens");
        fs::create_dir_all(&tokens_dir).expect("tokens");
        fs::write(tokens_dir.join(name), content).expect("write token log");
    }

    fn get_json(root: &Path, path: &str) -> (StatusCode, Value) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let response = crate::routes(
                root.to_path_buf(),
                crate::Clock::new(|| jiff::Timestamp::UNIX_EPOCH),
            )
            .oneshot(Request::get(path).body(Body::empty()).expect("request"))
            .await
            .expect("response");
            let status = response.status();
            let bytes = to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("bytes");
            let value = serde_json::from_slice(&bytes).expect("json");
            (status, value)
        })
    }

    #[test]
    fn day_totals_match_detailed_totals_and_number_kinds() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        write_token_log(
            root.path(),
            "20260801.jsonl",
            "{\"usage\":{\"total_tokens\":2400}}\n",
        );
        write_token_log(
            root.path(),
            "20260802.jsonl",
            "{\"usage\":{\"total_tokens\":0}}\n",
        );
        write_token_log(
            root.path(),
            "20260803.jsonl",
            "{\"usage\":{\"total_tokens\":-3}}\n",
        );
        write_token_log(
            root.path(),
            "20260804.jsonl",
            "{\"usage\":{\"total_tokens\":1.5}}\n",
        );
        write_token_log(
            root.path(),
            "20260805.jsonl",
            "{\"usage\":{\"total_tokens\":1e20}}\n",
        );
        write_token_log(root.path(), "20260806.jsonl", "{}\n");
        write_token_log(root.path(), "20260807.jsonl", "{\"usage\":[]}\n");
        write_token_log(
            root.path(),
            "20260808.jsonl",
            "{\"usage\":{\"total_tokens\":\"12\"}}\n",
        );
        write_token_log(
            root.path(),
            "20260809.jsonl",
            "{\"usage\":{\"total_tokens\":true}}\n",
        );
        write_token_log(
            root.path(),
            "20260810.jsonl",
            "{\"model\":\"my-engine\",\"usage\":{\"total_tokens\":20}}\n{\"model\":\"local/test\",\"usage\":{\"total_tokens\":22}}\n",
        );
        write_token_log(
            root.path(),
            "20260811.jsonl",
            "{\"usage\":{\"total_tokens\":1e16}}\n{\"usage\":{\"total_tokens\":1}}\n{\"usage\":{\"total_tokens\":-1e16}}\n",
        );
        write_token_log(
            root.path(),
            "20260814.jsonl",
            "{\"usage\":{\"total_tokens\":20}}\n{\"usage\":{\"total_tokens\":22}}\n",
        );

        let stats = usage_stats(root.path(), None).expect("stats");
        let expected = json!({
            "20260801": 2400,
            "20260802": 0,
            "20260803": -3,
            "20260804": 1.5,
            "20260805": u64::MAX,
            "20260806": 0,
            "20260807": 0,
            "20260808": 0,
            "20260809": 0,
            "20260810": 42,
            "20260811": 0,
            "20260814": 42,
        });
        assert_eq!(stats, expected);

        for day in [
            "20260801", "20260802", "20260803", "20260804", "20260805", "20260806", "20260807",
            "20260808", "20260809", "20260810", "20260811", "20260814",
        ] {
            assert_eq!(
                stats[day],
                aggregate(root.path(), day).expect("aggregate")["total"]["tokens"]
            );
        }

        assert_ne!(stats["20260801"], json!(2400.0));
        assert_ne!(stats["20260802"], json!(0.0));
        assert_ne!(stats["20260803"], json!(-3.0));
        assert_ne!(stats["20260805"], json!(1e20));
        assert_ne!(stats["20260806"], json!(0.0));
        assert_ne!(stats["20260811"], json!(0.0));
        assert_eq!(stats["20260814"], stats["20260810"]);
    }

    #[test]
    fn blank_and_malformed_lines_are_skipped_and_repeated_rows_add() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        write_token_log(
            root.path(),
            "20260815.jsonl",
            "{\"usage\":{\"total_tokens\":5}}\n\nnot json\n{\"usage\":{\"total_tokens\":7}}\n{\"usage\":{\"total_tokens\":1}}\n",
        );
        let stats = usage_stats(root.path(), None).expect("stats");
        assert_eq!(stats["20260815"], json!(13));
        assert_eq!(
            stats["20260815"],
            aggregate(root.path(), "20260815").expect("aggregate")["total"]["tokens"]
        );
    }

    #[test]
    fn empty_day_is_omitted_and_zero_token_day_stays_in_coverage() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        write_token_log(root.path(), "20260801.jsonl", "\nnot json\n   \n");
        write_token_log(root.path(), "20260802.jsonl", "{}\n");
        write_token_log(
            root.path(),
            "20260804.jsonl",
            "{\"usage\":{\"total_tokens\":5}}\n",
        );

        assert_eq!(
            aggregate(root.path(), "20260801").expect("agg")["total"]["requests"],
            0
        );

        let expected_days = json!({"20260802":0,"20260804":5});
        let stats_none = usage_stats(root.path(), None).expect("none");
        let stats_month = usage_stats(root.path(), Some("202608")).expect("month");
        assert_eq!(stats_none, expected_days);
        assert_eq!(stats_month, expected_days);
        assert_eq!(stats_none.get("20260801"), None);
        for day in ["20260802", "20260804"] {
            assert_eq!(
                stats_none[day],
                aggregate(root.path(), day).expect("aggregate")["total"]["tokens"]
            );
        }
        assert_ne!(stats_none["20260802"], json!(0.0));

        let (index_status, index_body) = get_json(root.path(), "/app/stats/api/index");
        assert_eq!(index_status, StatusCode::OK);
        assert_eq!(
            index_body["coverage"],
            json!({"start":"20260802","end":"20260804"})
        );
        assert_eq!(index_body["months"]["202608"], json!(5.0));
        assert_ne!(index_body["months"]["202608"], json!(5));

        let (month_status, month_body) = get_json(root.path(), "/app/stats/api/stats/202608");
        assert_eq!(month_status, StatusCode::OK);
        assert_eq!(month_body, expected_days);
        assert_eq!(month_body.get("20260801"), None);
    }

    #[test]
    fn later_writes_replace_what_the_next_read_returns() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");

        write_token_log(
            root.path(),
            "20260820.jsonl",
            "{\"usage\":{\"total_tokens\":1}}\n",
        );
        let stats = usage_stats(root.path(), None).expect("step 1");
        assert_eq!(stats, json!({"20260820":1}));
        assert_eq!(
            stats["20260820"],
            aggregate(root.path(), "20260820").expect("agg")["total"]["tokens"]
        );

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(root.path().join("tokens/20260820.jsonl"))
            .expect("open append");
        file.write_all(b"{\"usage\":{\"total_tokens\":2}}\n")
            .expect("append");
        drop(file);
        let stats = usage_stats(root.path(), None).expect("step 2");
        assert_eq!(stats, json!({"20260820":3}));
        assert_eq!(
            stats["20260820"],
            aggregate(root.path(), "20260820").expect("agg")["total"]["tokens"]
        );

        write_token_log(
            root.path(),
            "20260820.jsonl",
            "{\"usage\":{\"total_tokens\":10}}\n",
        );
        let stats = usage_stats(root.path(), None).expect("step 3");
        assert_eq!(stats, json!({"20260820":10}));
        assert_eq!(
            stats["20260820"],
            aggregate(root.path(), "20260820").expect("agg")["total"]["tokens"]
        );

        fs::remove_file(root.path().join("tokens/20260820.jsonl")).expect("remove");
        let stats = usage_stats(root.path(), None).expect("step 4");
        assert_eq!(stats, json!({}));

        write_token_log(
            root.path(),
            "20260821.jsonl",
            "{\"usage\":{\"total_tokens\":4}}\n",
        );
        let stats = usage_stats(root.path(), None).expect("step 5");
        assert_eq!(stats, json!({"20260821":4}));
        assert_eq!(
            stats["20260821"],
            aggregate(root.path(), "20260821").expect("agg")["total"]["tokens"]
        );
    }

    #[test]
    fn non_object_fails_the_unfiltered_read_and_index_only() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        write_token_log(root.path(), "20260809.jsonl", "[]\n");
        write_token_log(
            root.path(),
            "20260901.jsonl",
            "{\"usage\":{\"total_tokens\":4}}\n",
        );
        fs::write(root.path().join("tokens/20261001.jsonl"), [0xFF]).expect("invalid utf8");

        assert!(usage_stats(root.path(), None).is_err());
        assert!(usage_stats(root.path(), Some("202608")).is_err());
        assert!(usage_stats(root.path(), Some("202610")).is_err());

        let stats_sep = usage_stats(root.path(), Some("202609")).expect("sep");
        assert_eq!(stats_sep, json!({"20260901":4}));
        assert_eq!(
            stats_sep["20260901"],
            aggregate(root.path(), "20260901").expect("agg")["total"]["tokens"]
        );

        let (index_status, index_body) = get_json(root.path(), "/app/stats/api/index");
        assert_eq!(index_status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(index_body["reason_code"], "file_read_failed");

        let (month_status, month_body) = get_json(root.path(), "/app/stats/api/stats/202609");
        assert_eq!(month_status, StatusCode::OK);
        assert_eq!(month_body, json!({"20260901":4}));
        assert_eq!(month_body.get("20260809"), None);
    }

    #[test]
    fn inventory_reads_reconstructed_jsonl_not_the_listed_name() {
        let root = tempfile::tempdir().expect("root");
        let tokens_dir = root.path().join("tokens");
        fs::create_dir(&tokens_dir).expect("tokens");

        fs::write(
            tokens_dir.join("20260807.txt"),
            "{\"usage\":{\"total_tokens\":999}}\n",
        )
        .expect("write");
        fs::write(
            tokens_dir.join("20260809.txt"),
            "{\"usage\":{\"total_tokens\":999}}\n",
        )
        .expect("write");
        fs::write(
            tokens_dir.join("20260809.jsonl"),
            "{\"usage\":{\"total_tokens\":4}}\n",
        )
        .expect("write");
        fs::write(
            tokens_dir.join("notes.jsonl"),
            "{\"usage\":{\"total_tokens\":1000}}\n",
        )
        .expect("write");
        fs::write(
            tokens_dir.join("20260811.jsonl.tmp"),
            "{\"usage\":{\"total_tokens\":1000}}\n",
        )
        .expect("write");
        fs::create_dir(tokens_dir.join("20260810.jsonl")).expect("create dir");
        fs::write(
            tokens_dir.join("20260813.jsonl"),
            "{\"usage\":{\"total_tokens\":1}}\n",
        )
        .expect("write");

        #[cfg(unix)]
        {
            let target = root.path().join("symlink_target.jsonl");
            fs::write(&target, "{\"usage\":{\"total_tokens\":1000}}\n").expect("write target");
            std::os::unix::fs::symlink(&target, tokens_dir.join("20260812.jsonl"))
                .expect("symlink");
        }

        let stats = usage_stats(root.path(), None).expect("stats");
        assert_eq!(stats, json!({"20260809":4,"20260813":1}));
        for day in ["20260809", "20260813"] {
            assert_eq!(
                stats[day],
                aggregate(root.path(), day).expect("aggregate")["total"]["tokens"]
            );
        }
    }

    #[test]
    fn index_month_sums_converted_saturated_days() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir(root.path().join("tokens")).expect("tokens");
        write_token_log(
            root.path(),
            "20260401.jsonl",
            "{\"usage\":{\"total_tokens\":1e20}}\n",
        );
        write_token_log(
            root.path(),
            "20260402.jsonl",
            "{\"usage\":{\"total_tokens\":1e20}}\n",
        );

        let (month_status, month_body) = get_json(root.path(), "/app/stats/api/stats/202604");
        assert_eq!(month_status, StatusCode::OK);
        assert_eq!(
            month_body,
            json!({"20260401": u64::MAX, "20260402": u64::MAX})
        );
        for day in ["20260401", "20260402"] {
            assert_eq!(
                month_body[day],
                aggregate(root.path(), day).expect("aggregate")["total"]["tokens"]
            );
        }

        let sum = (u64::MAX as f64) + (u64::MAX as f64);
        let raw = 1e20_f64 + 1e20_f64;
        assert_ne!(sum, raw);

        let (index_status, index_body) = get_json(root.path(), "/app/stats/api/index");
        assert_eq!(index_status, StatusCode::OK);
        assert_eq!(index_body["months"]["202604"], json!(sum));
        assert_ne!(index_body["months"]["202604"], json!(raw));
        assert_eq!(
            index_body["coverage"],
            json!({"start":"20260401","end":"20260402"})
        );
    }
}

#[cfg(all(test, feature = "full-tests", unix))]
mod permission_tests {
    use super::*;
    use std::fs::{self, File};
    use std::path::Path;
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[cfg(unix)]
    struct ModeGuard<'a> {
        path: &'a Path,
        saved_mode: u32,
    }

    #[cfg(unix)]
    impl Drop for ModeGuard<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.path, fs::Permissions::from_mode(self.saved_mode));
        }
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_or_directory_fails_usage_stats() {
        let root = tempdir().expect("root");
        let tokens_dir = root.path().join("tokens");
        fs::create_dir(&tokens_dir).expect("tokens dir");
        let file_path = tokens_dir.join("20260901.jsonl");
        fs::write(&file_path, "{\"usage\":{\"total_tokens\":10}}\n").expect("write file");

        let initial = usage_stats(root.path(), None).expect("initial stats");

        {
            let metadata = fs::metadata(&file_path).expect("metadata");
            let saved_mode = metadata.permissions().mode();
            let _guard = ModeGuard {
                path: &file_path,
                saved_mode,
            };
            fs::set_permissions(&file_path, fs::Permissions::from_mode(0o000)).expect("chmod file");
            if File::open(&file_path).is_err() {
                let result = usage_stats(root.path(), None);
                assert!(result.is_err());
                assert_ne!(result.ok(), Some(initial.clone()));
            }
        }
        assert_eq!(
            usage_stats(root.path(), None).expect("restored file stats"),
            initial
        );

        {
            let metadata = fs::metadata(&tokens_dir).expect("dir metadata");
            let saved_mode = metadata.permissions().mode();
            let _guard = ModeGuard {
                path: &tokens_dir,
                saved_mode,
            };
            fs::set_permissions(&tokens_dir, fs::Permissions::from_mode(0o000)).expect("chmod dir");
            if fs::read_dir(&tokens_dir).is_err() {
                let result = usage_stats(root.path(), None);
                assert!(result.is_err());
                assert_ne!(result.ok(), Some(initial.clone()));
            }
        }
        assert_eq!(
            usage_stats(root.path(), None).expect("restored dir stats"),
            initial
        );
    }
}
