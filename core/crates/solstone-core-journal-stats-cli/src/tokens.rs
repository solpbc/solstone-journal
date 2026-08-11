// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, path::Path};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::document::TokenUsage;

pub(crate) fn scan_tokens(
    journal_root: &Path,
    now: DateTime<Utc>,
    use_cache: bool,
    diagnostics: &mut Vec<String>,
) -> TokenUsage {
    let tokens_dir = journal_root.join("tokens");
    if !tokens_dir.is_dir() {
        return TokenUsage::default();
    }
    let today = now.format("%Y%m%d").to_string();
    let Ok(entries) = fs::read_dir(&tokens_dir) else {
        diagnostics.push(format!(
            "Token directory read failed: {}",
            tokens_dir.display()
        ));
        return TokenUsage::default();
    };
    let mut token_files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    token_files.sort();

    let mut usage = TokenUsage::default();
    for token_file in token_files {
        let Some(day) = token_file.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let day = day.to_owned();
        let cache_file = tokens_dir.join(format!("{day}.tokens_cache.json"));
        if use_cache
            && day != today
            && try_cache_hit(&token_file, &cache_file, &day, &mut usage, diagnostics)
        {
            continue;
        }

        match fs::read_to_string(&token_file) {
            Ok(contents) => {
                for line in contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                {
                    match serde_json::from_str::<Value>(line) {
                        Ok(entry) => process_entry(&entry, &mut usage),
                        Err(error) => diagnostics.push(format!(
                            "Invalid token JSON in {}: {error}",
                            token_file.display()
                        )),
                    }
                }
            }
            Err(error) => {
                diagnostics.push(format!(
                    "Token read failed for {}: {error}",
                    token_file.display()
                ));
                continue;
            }
        }

        if use_cache && day != today {
            let payload = usage.by_day.get(&day).cloned().unwrap_or_default();
            match serde_json::to_string(&payload) {
                Ok(contents) => {
                    if let Err(error) = fs::write(&cache_file, contents) {
                        diagnostics.push(format!(
                            "Token cache save failed for {}: {error}",
                            token_file.display()
                        ));
                    }
                }
                Err(error) => diagnostics.push(format!(
                    "Token cache save failed for {}: {error}",
                    token_file.display()
                )),
            }
        }
    }
    usage
}

fn try_cache_hit(
    token_file: &Path,
    cache_file: &Path,
    day: &str,
    usage: &mut TokenUsage,
    diagnostics: &mut Vec<String>,
) -> bool {
    let fresh = match (fs::metadata(cache_file), fs::metadata(token_file)) {
        (Ok(cache), Ok(token)) => match (cache.modified(), token.modified()) {
            (Ok(cache), Ok(token)) => cache > token,
            _ => false,
        },
        _ => false,
    };
    if !fresh {
        return false;
    }
    match fs::read_to_string(cache_file)
        .and_then(|contents| serde_json::from_str(&contents).map_err(std::io::Error::other))
    {
        Ok(cached) => {
            usage.by_day.insert(day.to_owned(), cached);
            let cached = usage.by_day.get(day).expect("inserted token cache");
            for (model, counts) in cached {
                let totals = usage.by_model.entry(model.clone()).or_default();
                for (token_type, count) in counts {
                    *totals.entry(token_type.clone()).or_default() += count;
                }
            }
            true
        }
        Err(error) => {
            diagnostics.push(format!(
                "Token cache load failed for {}: {error}",
                token_file.display()
            ));
            false
        }
    }
}

fn process_entry(entry: &Value, usage: &mut TokenUsage) {
    let Some(timestamp) = entry.get("timestamp").and_then(Value::as_f64) else {
        return;
    };
    let seconds = timestamp.floor() as i64;
    let nanos = ((timestamp - seconds as f64) * 1_000_000_000.0).round() as u32;
    let Some(timestamp) = DateTime::<Utc>::from_timestamp(seconds, nanos) else {
        return;
    };
    let day = timestamp.format("%Y%m%d").to_string();
    let model = entry
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let Some(values) = entry.get("usage").and_then(Value::as_object) else {
        return;
    };
    for (token_type, value) in values {
        let Some(count) = value.as_i64() else {
            continue;
        };
        *usage
            .by_day
            .entry(day.clone())
            .or_default()
            .entry(model.clone())
            .or_default()
            .entry(token_type.clone())
            .or_default() += count;
        *usage
            .by_model
            .entry(model.clone())
            .or_default()
            .entry(token_type.clone())
            .or_default() += count;
    }
}
