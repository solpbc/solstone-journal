// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use chrono::Local;
use serde_json::{Value, json};

pub fn record_generate_usage(
    journal_path: &Path,
    model: &str,
    context: &str,
    usage: &Value,
) -> io::Result<()> {
    record_generate_usage_at(journal_path, model, context, usage, Local::now())
}

fn record_generate_usage_at(
    journal_path: &Path,
    model: &str,
    context: &str,
    usage: &Value,
    now: chrono::DateTime<Local>,
) -> io::Result<()> {
    let mut line = serde_json::to_vec(&json!({
        "timestamp": now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1_000_000_000.0,
        "model": model,
        "context": context,
        "usage": usage,
        "type": "generate",
    }))
    .expect("token log values serialize");
    line.push(b'\n');
    let directory = journal_path.join("tokens");
    fs::create_dir_all(&directory)?;
    let path = directory.join(now.format("%Y%m%d").to_string() + ".jsonl");
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(&line)
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
        record_generate_usage_at(&directory, "model", "context", &usage, now).unwrap();
        record_generate_usage_at(&directory, "model", "context", &usage, now).unwrap();
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
}
