// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
use serde_json::{Map, Value};
use solstone_core_cortex_client::{CortexRequest, UseEndState};
use solstone_core_talent_config::{TalentConfig, get_output_path};

use crate::context::{DispatchFailure, ThinkContext};

/// Daily, weekly, and cadence all use the reference's 610-second batch deadline.
pub(crate) const DEFAULT_THINK_TIMEOUT: Duration = Duration::from_secs(610);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModeResult {
    pub(crate) success: usize,
    pub(crate) failed: usize,
    pub(crate) failed_names: Vec<String>,
    pub(crate) applicable_units: BTreeSet<(String, Option<String>)>,
}

pub(crate) struct PendingUse {
    pub(crate) use_id: String,
    pub(crate) name: String,
    pub(crate) facet: Option<String>,
    pub(crate) output_path: Option<PathBuf>,
    pub(crate) index_output: bool,
}

/// Mirrors `thinking.py:2058`: daily, weekly, and cadence use this persistence
/// shaping helper; segment, activity, and flush deliberately set output directly.
pub(crate) fn apply_output_persistence(
    config: &TalentConfig,
    request: &mut Map<String, Value>,
    force: bool,
) {
    if config.metadata.get("accumulate") == Some(&Value::Bool(true)) {
        return;
    }
    let declared_output = config.metadata.get("output").and_then(Value::as_str);
    let is_generate = config.metadata.get("type").and_then(Value::as_str) == Some("generate");
    if is_generate || declared_output.is_some() {
        request.insert(
            "output".to_owned(),
            Value::String(declared_output.unwrap_or("md").to_owned()),
        );
        if force {
            request.insert("refresh".to_owned(), Value::Bool(true));
        }
    }
}

pub(crate) fn grouped(configs: Vec<TalentConfig>) -> BTreeMap<i64, Vec<TalentConfig>> {
    let mut groups = BTreeMap::new();
    for config in configs {
        groups
            .entry(
                config
                    .metadata
                    .get("priority")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            )
            .or_insert_with(Vec::new)
            .push(config);
    }
    for configs in groups.values_mut() {
        configs.sort_by(|left, right| left.key.cmp(&right.key));
    }
    groups
}

/// Python's `fnmatch.fnmatch` equivalent for `exclude_streams` metadata.
pub(crate) fn excluded(config: &TalentConfig, stream: Option<&str>) -> bool {
    let Some(stream) = stream else {
        return false;
    };
    config
        .metadata
        .get("exclude_streams")
        .and_then(Value::as_array)
        .is_some_and(|patterns| {
            patterns.iter().filter_map(Value::as_str).any(|pattern| {
                glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(stream))
            })
        })
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, String> {
    // Wait talks to Callosum over a Unix socket. Time-only is enough for the
    // claim poll, and then UnixStream::connect panics: "IO is disabled".
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())
}

pub(crate) fn dispatch(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    config: &TalentConfig,
    schedule: &str,
    facet: Option<&str>,
    force: bool,
    extra: Map<String, Value>,
) -> Result<PendingUse, DispatchFailure> {
    let mut request = extra;
    request
        .entry("day".to_owned())
        .or_insert_with(|| Value::String(context.day.clone()));
    request.insert("schedule".to_owned(), Value::String(schedule.to_owned()));
    let mut env = Map::from_iter([("SOL_DAY".to_owned(), Value::String(context.day.clone()))]);
    if let Some(facet) = facet {
        request.insert("facet".to_owned(), Value::String(facet.to_owned()));
        env.insert("SOL_FACET".to_owned(), Value::String(facet.to_owned()));
    }
    request.insert("env".to_owned(), Value::Object(env));
    apply_output_persistence(config, &mut request, force);
    if schedule == "weekly" && config.key == "weekly_reflection" {
        let day = NaiveDate::parse_from_str(&context.day, "%Y%m%d").expect("validated day");
        let week_start =
            day - ChronoDuration::days(i64::from(day.weekday().num_days_from_sunday()));
        let week_start = week_start.format("%Y%m%d").to_string();
        request.insert("day".to_owned(), Value::String(week_start.clone()));
        request.insert("output".to_owned(), Value::String("md".to_owned()));
        request.insert(
            "output_path".to_owned(),
            Value::String(
                context
                    .journal
                    .join("reflections/weekly")
                    .join(format!("{week_start}.md"))
                    .display()
                    .to_string(),
            ),
        );
        if let Some(env) = request.get_mut("env").and_then(Value::as_object_mut) {
            env.insert("SOL_DAY".to_owned(), Value::String(week_start));
        }
    }
    if let Some(format) = request.get("output").and_then(Value::as_str)
        && !request.contains_key("output_path")
    {
        request.insert(
            "output_path".to_owned(),
            Value::String(
                get_output_path(
                    &context.day_dir,
                    &config.key,
                    None,
                    Some(format),
                    facet,
                    None,
                )
                .display()
                .to_string(),
            ),
        );
    }
    let prompt = if config.metadata.get("type").and_then(Value::as_str) == Some("generate") {
        String::new()
    } else if schedule == "weekly" && config.key == "weekly_reflection" && facet.is_none() {
        // Source-derived, not measured: thinking.py:2606 and 2834 include
        // the weekly reflection's ISO week-start and the day's input summary.
        format!(
            "Running scheduled weekly reflection for {}: {}.",
            request
                .get("day")
                .and_then(Value::as_str)
                .and_then(iso_day)
                .unwrap_or_else(|| iso_day(&context.day).unwrap_or_else(|| context.day.clone())),
            day_input_summary(&context.day_dir),
        )
    } else if schedule == "cadence" {
        // Source-derived, not measured: thinking.py:3012-3025 gives cadence
        // cogitate talents their own prompt form, without a day summary.
        format!(
            "Running cadence task for {}.",
            iso_day(&context.day).unwrap_or_else(|| context.day.clone())
        )
    } else if let Some(facet) = facet {
        // Source-derived, not measured: thinking.py:2134/2294 and
        // 2606/2728-2730 retain the facet context and input summary; the
        // name-keyed weekly reflection uses its week-start day here too.
        format!(
            "Processing facet '{facet}' for {}: {}. Use get_facet('{facet}') to load context.",
            if schedule == "weekly" && config.key == "weekly_reflection" {
                request
                    .get("day")
                    .and_then(Value::as_str)
                    .and_then(iso_day)
                    .unwrap_or_else(|| iso_day(&context.day).unwrap_or_else(|| context.day.clone()))
            } else {
                iso_day(&context.day).unwrap_or_else(|| context.day.clone())
            },
            day_input_summary(&context.day_dir),
        )
    } else {
        // Source-derived, not measured: thinking.py:2134/2425 and
        // 2606/2836 include the day's recording summary for scheduled work.
        format!(
            "Running scheduled task for {}: {}.",
            iso_day(&context.day).unwrap_or_else(|| context.day.clone()),
            day_input_summary(&context.day_dir),
        )
    };
    let request = CortexRequest::new(prompt, config.key.clone()).with_config(request);
    let output_path = request
        .config
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let index_output = config.metadata.get("type").and_then(Value::as_str) == Some("generate")
        && config.metadata.get("output").and_then(Value::as_str) != Some("json");
    let use_id = context.cortex.dispatch(runtime, &request)?;
    Ok(PendingUse {
        use_id,
        name: config.key.clone(),
        facet: facet.map(ToOwned::to_owned),
        output_path,
        index_output,
    })
}

fn iso_day(day: &str) -> Option<String> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .map(|day| day.format("%Y-%m-%d").to_string())
}

fn day_input_summary(day_dir: &std::path::Path) -> String {
    let mut segments = Vec::new();
    collect_segment_keys(day_dir, &mut segments);
    let total_seconds = segments
        .iter()
        .filter_map(|segment| segment.split_once('_'))
        .filter_map(|(_, duration)| duration.parse::<u64>().ok())
        .sum::<u64>();
    if segments.is_empty() {
        return "No recordings".to_owned();
    }
    let duration = if total_seconds < 60 {
        format!("~{total_seconds} seconds")
    } else if total_seconds < 3_600 {
        format!("~{} minutes", (total_seconds as f64 / 60.0).round() as u64)
    } else {
        format!("~{:.1} hours", total_seconds as f64 / 3_600.0)
    };
    let count = segments.len();
    if count < 5 || total_seconds < 1_800 {
        format!(
            "Light activity: {count} segment{}, {duration}",
            if count == 1 { "" } else { "s" }
        )
    } else {
        format!("{count} segments, {duration}")
    }
}

fn collect_segment_keys(directory: &std::path::Path, keys: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        if name
            .split_once('_')
            .is_some_and(|(_, duration)| duration.parse::<u64>().is_ok())
        {
            keys.push(name.to_owned());
        } else {
            collect_segment_keys(&path, keys);
        }
    }
}

/// Segment, activity, and flush intentionally own their output fields.  The
/// reference's `thinking.py:2058` limits `apply_output_persistence` to daily,
/// weekly, and cadence, so these callers must not use it.
pub(crate) fn dispatch_direct(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    prompt: String,
    config: Map<String, Value>,
    facet: Option<&str>,
) -> Result<PendingUse, DispatchFailure> {
    let request = CortexRequest::new(prompt, name.to_owned()).with_config(config);
    let output_path = request
        .config
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let index_output = request.config.contains_key("output")
        && request.config.get("output").and_then(Value::as_str) != Some("json");
    let use_id = context.cortex.dispatch(runtime, &request)?;
    Ok(PendingUse {
        use_id,
        name: name.to_owned(),
        facet: facet.map(ToOwned::to_owned),
        output_path,
        index_output,
    })
}

/// Shared equivalent of `_drain_priority_batch` (`thinking.py:991-1042`).
/// The client policy is `think()` and its explicit outcome deadline is 610 seconds.
pub(crate) fn drain(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
) -> ModeResult {
    drain_with_deadline(context, runtime, pending, Some(DEFAULT_THINK_TIMEOUT))
}

pub(crate) fn drain_with_deadline(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    deadline: Option<Duration>,
) -> ModeResult {
    let mut result = ModeResult::default();
    if pending.is_empty() {
        return result;
    }
    let use_ids = pending
        .iter()
        .map(|item| item.use_id.clone())
        .collect::<Vec<_>>();
    match context.cortex.wait(runtime, &use_ids, deadline) {
        Ok(report) => {
            for item in pending {
                let label = item.facet.as_ref().map_or_else(
                    || item.name.clone(),
                    |facet| format!("{}/{facet}", item.name),
                );
                match report.completed.get(&item.use_id) {
                    Some(completion) if completion.end_state == UseEndState::Finish => {
                        // Source-derived, not measured: thinking.py:240-242 and
                        // 1102 require both a literal changed flag and a path.
                        maybe_rescan_output(context, &item, completion);
                        result.success += 1;
                    }
                    _ => {
                        result.failed += 1;
                        result.failed_names.push(format!("{label} (error)"));
                    }
                }
            }
        }
        Err(_) => {
            for item in pending {
                result.failed += 1;
                let label = item.facet.as_ref().map_or_else(
                    || item.name.clone(),
                    |facet| format!("{}/{facet}", item.name),
                );
                result.failed_names.push(format!("{label} (error)"));
            }
        }
    }
    result
}

/// Preserve the reference's literal changed flag and existing-file gate.
pub(crate) fn maybe_rescan_output(
    context: &ThinkContext,
    item: &PendingUse,
    completion: &solstone_core_cortex_client::UseCompletion,
) {
    // Source-derived, not measured: thinking.py:240-242 requires both a
    // literal `True` finish field and an existing output path.
    if completion.finish_fields.output_changed == Some(true)
        && item.index_output
        && item.output_path.as_ref().is_some_and(|path| path.exists())
    {
        context.index.rescan_file(
            &context.journal,
            item.output_path.as_ref().expect("checked output path"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::runtime;

    #[test]
    fn think_runtime_can_connect_a_unix_socket() {
        let directory = tempfile::tempdir().expect("socket directory");
        let path = directory.path().join("callosum.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let runtime = runtime().expect("think runtime");
        runtime.block_on(async {
            tokio::net::UnixStream::connect(&path)
                .await
                .expect("think runtime must enable IO");
        });
        drop(listener);
    }
}
