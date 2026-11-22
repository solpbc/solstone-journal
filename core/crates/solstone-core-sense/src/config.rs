// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value};
use solstone_core_journal_config::read_journal_config;
use solstone_core_local::{
    LocalEndpointResolution, LocalInferenceAuthority, bundled_parallel_slots_with_authority,
    resolve_local_endpoint,
};

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
    read_config_in(solstone_core_cli_copy::ENGLISH, journal)
}

pub(crate) fn read_config_in(locale: &str, journal: &Path) -> Map<String, Value> {
    match read_journal_config(journal) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            let solstone_core_journal_config::ConfigLoadError::Corrupt { path, .. } = error;
            let error = solstone_core_window_catalog::strings(
                locale,
                "journal.window.facets.awareness.response.detail.unreadable.settings",
                [("settings_path", &path.display().to_string())],
            );
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.sense.config.read",
                    [("error", &(error))]
                )
            );
            Map::new()
        }
    }
}

pub fn resolve_concurrency(config: &Map<String, Value>, handler: &str) -> usize {
    resolve_concurrency_in(solstone_core_cli_copy::ENGLISH, config, handler)
}

pub(crate) fn resolve_concurrency_in(
    locale: &str,
    config: &Map<String, Value>,
    handler: &str,
) -> usize {
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
            eprintln!(
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.sense.config.concurrency",
                    [("handler", &(handler))]
                )
            );
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
    resolve_max_runtime_in(solstone_core_cli_copy::ENGLISH, config, handler)
}

pub(crate) fn resolve_max_runtime_in(
    locale: &str,
    config: &Map<String, Value>,
    handler: &str,
) -> Duration {
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
                "{}",
                solstone_core_cli_copy::display_for(
                    locale,
                    "journal.cli.think.sense.config.runtime",
                    [("handler", &(handler)), ("seconds", &(fallback.as_secs()))]
                )
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

/// Size describe batches to the active endpoint's serving capacity.
pub fn describe_per_proc_jobs(
    config: &Map<String, Value>,
    effective: usize,
    journal: &Path,
) -> usize {
    if active_provider(config) != Some("local") {
        return 10;
    }
    let slots = match resolve_local_endpoint(config) {
        // An endpoint's calls take turns in the journal's one call line.
        LocalEndpointResolution::Byo(_) => Some(1),
        LocalEndpointResolution::Bundled => Some(bundled_slots(journal)),
    };
    slots.map_or(10, |slots| {
        std::cmp::max(1, (2 * slots as usize) / effective.max(1))
    })
}

/// The bundled engine's serving capacity. Linux reads it with the engine's
/// per-launch key; Windows keeps its single-slot sizing.
fn bundled_slots(journal: &Path) -> u32 {
    let mut authority = if cfg!(target_os = "linux") {
        solstone_core_callosum::local_inference_refresh(journal)
            .map(|refresh| LocalInferenceAuthority::new(0, 0, String::new(), Some(refresh)))
    } else {
        None
    };
    bundled_parallel_slots_with_authority(journal, authority.as_mut())
}

pub fn processing_deferred(config: &Map<String, Value>) -> bool {
    solstone_core_journal_config::processing::processing_settings(config)
        .map(|settings| settings.deferred)
        .unwrap_or(true)
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
    fn local_byo_fanout_keeps_one_call_waiting_behind_the_running_one() {
        let c = json!({"providers":{"active":{"provider":"local"},"local":{"endpoint_url":"http://x","served_model_id":"x","parallel_slots":4}}});
        assert_eq!(
            describe_per_proc_jobs(c.as_object().unwrap(), 1, Path::new("/unused")),
            2
        );
    }
}
