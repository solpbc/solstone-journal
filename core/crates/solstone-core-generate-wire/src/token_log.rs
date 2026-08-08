// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use chrono::Local;
use serde_json::{Value, json};

static TOKEN_LOG_LOCK: Mutex<()> = Mutex::new(());

pub struct GenerateUsageMetadata<'a> {
    pub non_responsive_output: Option<&'a str>,
    pub non_responsive_matched_signal: Option<&'a str>,
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
    let _lock = TOKEN_LOG_LOCK.lock().expect("token log lock poisoned");
    let mut entry = serde_json::Map::from_iter([
        (
            "timestamp".into(),
            json!(
                now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1_000_000_000.0
            ),
        ),
        ("model".into(), json!(model)),
        ("context".into(), json!(context)),
        ("usage".into(), usage.clone()),
        ("type".into(), json!("generate")),
    ]);
    if let Some(metadata) = metadata {
        if let Some(output) = metadata.non_responsive_output {
            entry.insert("non_responsive_output".into(), json!(output));
        }
        if let Some(signal) = metadata.non_responsive_matched_signal {
            entry.insert("non_responsive_matched_signal".into(), json!(signal));
        }
    }
    let mut line = serde_json::to_vec(&Value::Object(entry)).expect("token log values serialize");
    line.push(b'\n');
    let directory = journal_path.join("tokens");
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

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn appends_exactly_one_line_per_completion() {
        let directory = std::env::temp_dir().join(format!(
            "solstone-generate-wire-token-log-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let now = Local::now();
        let usage = json!({"input_tokens": 2, "output_tokens": 1, "total_tokens": 3});
        record_generate_usage_at(&directory, "model", "context", &usage, None, now).unwrap();
        record_generate_usage_at(&directory, "model", "context", &usage, None, now).unwrap();
        let path = directory
            .join("tokens")
            .join(now.format("%Y%m%d").to_string() + ".jsonl");
        let text = fs::read_to_string(path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let value: Value = serde_json::from_str(line).unwrap();
            assert_eq!(value.as_object().unwrap().len(), 5);
            assert_eq!(value["type"], "generate");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn writes_non_responsive_metadata_only_when_present() {
        let directory = std::env::temp_dir().join(format!(
            "solstone-generate-wire-token-log-metadata-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let now = Local::now();
        let usage = json!({});
        let metadata = GenerateUsageMetadata {
            non_responsive_output: Some("I cannot do that."),
            non_responsive_matched_signal: Some("i cannot"),
        };
        record_generate_usage_at(&directory, "model", "context", &usage, Some(&metadata), now)
            .unwrap();
        let path = directory
            .join("tokens")
            .join(now.format("%Y%m%d").to_string() + ".jsonl");
        let value: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(value["non_responsive_output"], "I cannot do that.");
        assert_eq!(value["non_responsive_matched_signal"], "i cannot");
        fs::remove_dir_all(directory).unwrap();
    }
}
