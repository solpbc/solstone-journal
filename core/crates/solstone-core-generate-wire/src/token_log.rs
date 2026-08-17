// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use chrono::Local;
use serde_json::{Map, Value, json};

static TOKEN_LOG_LOCK: Mutex<()> = Mutex::new(());

pub struct GenerateUsageMetadata<'a> {
    pub non_responsive_output: Option<&'a str>,
    pub non_responsive_matched_signal: Option<&'a str>,
}

struct UsageRecord<'a> {
    journal_path: &'a Path,
    model: &'a str,
    context: &'a str,
    usage: &'a Value,
    entry_type: &'a str,
    segment: Option<&'a str>,
    metadata: Option<&'a GenerateUsageMetadata<'a>>,
}

pub fn record_generate_usage(
    journal_path: &Path,
    model: &str,
    context: &str,
    usage: &Value,
    metadata: Option<&GenerateUsageMetadata<'_>>,
) -> io::Result<()> {
    record_generate_usage_at(journal_path, model, context, usage, metadata, Local::now())
}

fn record_generate_usage_at(
    journal_path: &Path,
    model: &str,
    context: &str,
    usage: &Value,
    metadata: Option<&GenerateUsageMetadata<'_>>,
    now: chrono::DateTime<Local>,
) -> io::Result<()> {
    record_usage_at(
        UsageRecord {
            journal_path,
            model,
            context,
            usage,
            entry_type: "generate",
            segment: None,
            metadata,
        },
        now,
    )
}

pub fn record_usage(
    journal_path: &Path,
    model: &str,
    context: &str,
    usage: &Value,
    entry_type: &str,
    segment: Option<&str>,
    metadata: Option<&GenerateUsageMetadata<'_>>,
) -> io::Result<()> {
    record_usage_at(
        UsageRecord {
            journal_path,
            model,
            context,
            usage,
            entry_type,
            segment,
            metadata,
        },
        Local::now(),
    )
}

fn record_usage_at(record: UsageRecord<'_>, now: chrono::DateTime<Local>) -> io::Result<()> {
    let _lock = TOKEN_LOG_LOCK.lock().expect("token log lock poisoned");
    let mut entry = serde_json::Map::from_iter([
        (
            "timestamp".into(),
            json!(
                now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1_000_000_000.0
            ),
        ),
        ("model".into(), json!(record.model)),
        ("context".into(), json!(record.context)),
        ("usage".into(), record.usage.clone()),
    ]);
    if let Some(segment) = record.segment {
        entry.insert("segment".into(), json!(segment));
    }
    entry.insert("type".into(), json!(record.entry_type));
    if let Some(metadata) = record.metadata {
        if let Some(output) = metadata.non_responsive_output {
            entry.insert("non_responsive_output".into(), json!(output));
        }
        if let Some(signal) = metadata.non_responsive_matched_signal {
            entry.insert("non_responsive_matched_signal".into(), json!(signal));
        }
    }
    let mut line = serde_json::to_vec(&Value::Object(entry)).expect("token log values serialize");
    line.push(b'\n');
    let directory = record.journal_path.join("tokens");
    fs::create_dir_all(&directory)?;
    let path = directory.join(now.format("%Y%m%d").to_string() + ".jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let written = file.write(&line)?;
    if written == line.len() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            format!(
                "token log short write: wrote {written} of {} bytes",
                line.len()
            ),
        ))
    }
}

pub fn usage_for_log(usage: &Value) -> Value {
    let mut normalized = Map::new();
    for name in [
        "input_tokens",
        "output_tokens",
        "total_tokens",
        "cached_tokens",
        "reasoning_tokens",
        "cache_creation_tokens",
        "requests",
    ] {
        if usage
            .get(name)
            .and_then(Value::as_u64)
            .is_some_and(|value| value != 0)
        {
            normalized.insert(name.to_owned(), usage[name].clone());
        }
    }
    if !normalized.contains_key("cached_tokens")
        && usage
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|value| value != 0)
    {
        normalized.insert("cached_tokens".into(), usage["cached_input_tokens"].clone());
    }
    if !normalized.contains_key("total_tokens") {
        let input = normalized
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output = normalized
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input != 0 || output != 0 {
            normalized.insert(
                "total_tokens".into(),
                Value::from(input.saturating_add(output)),
            );
        }
    }
    Value::Object(normalized)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    const REQUIRED_USAGE_KEYS: [&str; 5] = ["timestamp", "model", "context", "usage", "type"];
    const FORBIDDEN_SECRET_KEYS: [&str; 6] = [
        "api_key",
        "authorization",
        "secret",
        "credential",
        "access_token",
        "api_key_override",
    ];

    fn fixed_local() -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(2020, 1, 2, 3, 4, 5)
            .single()
            .expect("fixed local timestamp")
    }

    fn assert_generate_line(value: &Value) {
        let object = value.as_object().expect("usage line is an object");
        for key in REQUIRED_USAGE_KEYS {
            assert!(object.contains_key(key), "missing required key {key}");
        }
        assert_eq!(value["type"], "generate");
        for key in FORBIDDEN_SECRET_KEYS {
            assert!(
                !object.contains_key(key),
                "usage line must not carry secret field {key}"
            );
        }
    }

    #[test]
    fn appends_exactly_one_line_per_completion() {
        let directory = crate::validation::isolated_journal_dir("token-log");
        let now = fixed_local();
        let usage = json!({"input_tokens": 2, "output_tokens": 1, "total_tokens": 3});
        let record = || UsageRecord {
            journal_path: &directory,
            model: "model",
            context: "context",
            usage: &usage,
            entry_type: "generate",
            segment: None,
            metadata: None,
        };
        record_usage_at(record(), now).unwrap();
        record_usage_at(record(), now).unwrap();
        let path = directory
            .join("tokens")
            .join(now.format("%Y%m%d").to_string() + ".jsonl");
        let text = fs::read_to_string(path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let value: Value = serde_json::from_str(line).unwrap();
            assert_generate_line(&value);
            assert!(value.get("non_responsive_output").is_none());
            assert!(value.get("non_responsive_matched_signal").is_none());
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn writes_non_responsive_metadata_only_when_present() {
        let directory = crate::validation::isolated_journal_dir("token-log-metadata");
        let now = fixed_local();
        let usage = json!({});
        let metadata = GenerateUsageMetadata {
            non_responsive_output: Some("I cannot do that."),
            non_responsive_matched_signal: Some("i cannot"),
        };
        record_usage_at(
            UsageRecord {
                journal_path: &directory,
                model: "model",
                context: "context",
                usage: &usage,
                entry_type: "generate",
                segment: None,
                metadata: Some(&metadata),
            },
            now,
        )
        .unwrap();
        let path = directory
            .join("tokens")
            .join(now.format("%Y%m%d").to_string() + ".jsonl");
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_generate_line(&value);
        assert_eq!(value["non_responsive_output"], "I cannot do that.");
        assert_eq!(value["non_responsive_matched_signal"], "i cannot");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generate_wrapper_remains_identical_to_generalized_generate_record() {
        let wrapper_directory = crate::validation::isolated_journal_dir("token-wrapper");
        let generic_directory = crate::validation::isolated_journal_dir("token-generic");
        let now = fixed_local();
        let usage = json!({"input_tokens": 2});
        record_generate_usage_at(&wrapper_directory, "model", "context", &usage, None, now)
            .unwrap();
        record_usage_at(
            UsageRecord {
                journal_path: &generic_directory,
                model: "model",
                context: "context",
                usage: &usage,
                entry_type: "generate",
                segment: None,
                metadata: None,
            },
            now,
        )
        .unwrap();
        let name = now.format("%Y%m%d").to_string() + ".jsonl";
        assert_eq!(
            fs::read(wrapper_directory.join("tokens").join(&name)).unwrap(),
            fs::read(generic_directory.join("tokens").join(name)).unwrap()
        );
        fs::remove_dir_all(wrapper_directory).unwrap();
        fs::remove_dir_all(generic_directory).unwrap();
    }
}
