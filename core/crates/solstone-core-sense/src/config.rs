// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::{LocalEndpointResolution, resolve_local_endpoint};

pub const HANDLERS: [&str; 3] = ["describe", "transcribe", "depict"];

pub fn max_runtime_default(handler: &str) -> Duration {
    Duration::from_secs(match handler {
        "describe" => 1800,
        "transcribe" => 2700,
        "depict" => 600,
        _ => 600,
    })
}

pub fn read_config(journal: &Path) -> Map<String, Value> {
    match read_journal_config(journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            eprintln!("sense: journal config unavailable: {error}");
            Map::new()
        }
    }
}

pub fn resolve_concurrency(config: &Map<String, Value>, handler: &str) -> usize {
    let raw = config
        .get(handler)
        .and_then(Value::as_object)
        .and_then(|v| v.get("max_concurrent"));
    match raw
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .filter(|v| *v > 0)
    {
        Some(value) => value,
        None if raw.is_none() => 1,
        None => {
            eprintln!("sense: invalid {handler}.max_concurrent; defaulting to 1");
            1
        }
    }
}

pub fn parse_duration_seconds(raw: &Value) -> Option<Duration> {
    if let Some(value) = raw.as_u64() {
        return (value > 0).then(|| Duration::from_secs(value));
    }
    let value = raw.as_str()?;
    let (amount, unit) = value.split_at(value.len().checked_sub(1)?);
    if amount.is_empty() || !amount.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let amount = amount.parse::<u64>().ok()?.checked_mul(match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    })?;
    (amount > 0).then(|| Duration::from_secs(amount))
}

pub fn resolve_max_runtime(config: &Map<String, Value>, handler: &str) -> Duration {
    let fallback = max_runtime_default(handler);
    let raw = config
        .get(handler)
        .and_then(Value::as_object)
        .and_then(|v| v.get("max_runtime"));
    match raw.and_then(parse_duration_seconds) {
        Some(value) => value,
        None if raw.is_none() => fallback,
        None => {
            eprintln!(
                "sense: invalid {handler}.max_runtime; defaulting to {}s",
                fallback.as_secs()
            );
            fallback
        }
    }
}

fn active_provider(config: &Map<String, Value>) -> Option<&str> {
    config
        .get("providers")?
        .as_object()?
        .get("active")?
        .as_object()?
        .get("provider")?
        .as_str()
        .filter(|v| !v.trim().is_empty())
}

/// Python's `describe_per_proc_jobs`, limited to the data the dispatcher owns.
pub fn describe_per_proc_jobs(
    config: &Map<String, Value>,
    effective: usize,
    journal: &Path,
) -> usize {
    if active_provider(config) != Some("local") {
        return 10;
    }
    let slots = match resolve_local_endpoint(config) {
        LocalEndpointResolution::Byo(endpoint) => endpoint.parallel_slots,
        // Bundled capacity has no generic public reader; local.ctx is its persisted
        // tier evidence and the reference's fallback is one.
        LocalEndpointResolution::Bundled => {
            std::fs::read_to_string(journal.join("health/local.ctx"))
                .ok()
                .and_then(|v| match v.trim() {
                    "16384" => Some(1),
                    "32768" => Some(2),
                    _ => None,
                })
                .or(Some(1))
        }
    };
    slots.map_or(10, |slots| {
        std::cmp::max(1, (2 * slots as usize) / effective.max(1))
    })
}

pub fn processing_deferred(config: &Map<String, Value>) -> bool {
    config
        .get("processing")
        .and_then(Value::as_object)
        .and_then(|v| v.get("mode"))
        .and_then(Value::as_str)
        == Some("deferred")
}

pub fn no_thinking_engine(config: &Map<String, Value>) -> bool {
    active_provider(config).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn duration_grammar_is_exact() {
        assert_eq!(
            parse_duration_seconds(&json!("30m")).unwrap().as_secs(),
            1800
        );
        for value in [
            json!(true),
            json!(0),
            json!("1800"),
            json!("1.5h"),
            json!("0s"),
        ] {
            assert!(parse_duration_seconds(&value).is_none());
        }
    }
    #[test]
    fn bool_concurrency_is_not_one() {
        assert_eq!(
            resolve_concurrency(
                &json!({"describe":{"max_concurrent":true}})
                    .as_object()
                    .unwrap()
                    .clone(),
                "describe"
            ),
            1
        );
    }
    #[test]
    fn local_byo_fanout_is_sized() {
        let c = json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"http://x","served_model_id":"x","parallel_slots":2}}});
        assert_eq!(
            describe_per_proc_jobs(c.as_object().unwrap(), 1, Path::new("/unused")),
            4
        );
    }
}
