// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};
use solstone_core_system_health::{
    FilesystemHealthLogSource, read_completed_units, read_daily_deterministic_failures,
};
use solstone_core_talent_config::{TalentFilter, load_talent_configs};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    ModeResult, PendingUse, dispatch, drain, excluded, grouped, merge_mode_result, runtime,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Port of `thinking.py:2086-2556`.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    stream: Option<&str>,
    from_scratch: bool,
    max_concurrency: i64,
) -> Result<ModeResult, String> {
    let configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("daily"),
            include_disabled: false,
        },
    )?;
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }
    let status = Map::from_iter([
        ("mode".to_owned(), Value::String("daily".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("agents_total".to_owned(), Value::from(configs.len())),
        ("agents_completed".to_owned(), Value::from(0)),
    ]);
    context.status.update(status.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", status);
    let source = FilesystemHealthLogSource::new(&context.journal);
    let completed = read_completed_units(&source, &context.day)
        .map_err(|error| error.to_string())?
        .value;
    let deterministic = read_daily_deterministic_failures(&source, &context.day)
        .map_err(|error| error.to_string())?
        .value;
    let facets =
        solstone_core_facets::list_declared_facet_names(&context.journal).unwrap_or_default();
    // Source-derived, not measured: thinking.py:2134-2136 loads this day's
    // active facets before multi-facet expansion, and 2220-2227 records
    // `no_active_facets` for an inactive non-`always` facet.
    let active_facets =
        solstone_core_system::activity_state::active_facets(&context.journal, &context.day);
    let mut total = ModeResult::default();
    let runtime = runtime()?;
    for (priority, group) in grouped(configs) {
        let mut fields = Map::new();
        fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
        fields.insert("day".to_owned(), Value::String(context.day.clone()));
        fields.insert("priority".to_owned(), Value::from(priority));
        fields.insert("count".to_owned(), Value::from(group.len()));
        log.log("group.start", context.now_ms, fields);
        let mut pending = Vec::new();
        let mut group_result = ModeResult::default();
        for config in group {
            if excluded(&config, stream) {
                continue;
            }
            if config.metadata.get("multi_facet") == Some(&Value::Bool(true)) {
                for facet in &facets {
                    if config.metadata.get("always") != Some(&Value::Bool(true))
                        && !active_facets.contains(facet)
                    {
                        log_skip(log, context, &config.key, "no_active_facets", Some(facet));
                        continue;
                    }
                    queue_daily(
                        context,
                        log,
                        &runtime,
                        &config,
                        Some(facet),
                        &completed,
                        &deterministic,
                        from_scratch,
                        &mut pending,
                        &mut group_result,
                    )?;
                    drain_if_full(
                        context,
                        &runtime,
                        &mut pending,
                        &mut group_result,
                        max_concurrency,
                    );
                }
            } else {
                queue_daily(
                    context,
                    log,
                    &runtime,
                    &config,
                    None,
                    &completed,
                    &deterministic,
                    from_scratch,
                    &mut pending,
                    &mut group_result,
                )?;
                drain_if_full(
                    context,
                    &runtime,
                    &mut pending,
                    &mut group_result,
                    max_concurrency,
                );
            }
        }
        merge(
            &mut group_result,
            drain(context, &runtime, std::mem::take(&mut pending)),
        );
        let mut completed_fields = Map::new();
        completed_fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
        completed_fields.insert("day".to_owned(), Value::String(context.day.clone()));
        completed_fields.insert("priority".to_owned(), Value::from(priority));
        completed_fields.insert("success".to_owned(), Value::from(group_result.success));
        completed_fields.insert("failed".to_owned(), Value::from(group_result.failed));
        log.log("group.complete", context.now_ms, completed_fields);
        merge(&mut total, group_result);
    }
    context.status.update(Map::from_iter([(
        "agents_completed".to_owned(),
        Value::from(total.success + total.failed),
    )]));
    let _ = helpers::emit(
        &context.journal,
        context.now_ms,
        "completed",
        Map::from_iter([
            ("mode".to_owned(), Value::String("daily".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("success".to_owned(), Value::from(total.success)),
            ("failed".to_owned(), Value::from(total.failed)),
        ]),
    );
    helpers::day_log(
        &context.journal,
        &context.day,
        context.now_ms,
        &format!(
            "think{}",
            if total.failed == 0 {
                String::new()
            } else {
                format!(" failed={}", total.failed)
            }
        ),
    );
    Ok(total)
}

#[allow(
    clippy::too_many_arguments,
    reason = "The reference keeps the daily completed and deterministic-failure guards explicit at this dispatch boundary."
)]
fn queue_daily(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    runtime: &tokio::runtime::Runtime,
    config: &solstone_core_talent_config::TalentConfig,
    facet: Option<&str>,
    completed: &BTreeSet<solstone_core_system_health::CompletedUnit>,
    deterministic: &BTreeMap<
        solstone_core_system_health::DailyUnit,
        solstone_core_system_health::DeterministicFailure,
    >,
    from_scratch: bool,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
) -> Result<(), String> {
    let unit = (config.key.clone(), facet.map(ToOwned::to_owned));
    result.applicable_units.insert(unit.clone());
    let completed_key = solstone_core_system_health::CompletedUnit {
        mode: "daily".to_owned(),
        name: config.key.clone(),
        facet: unit.1.clone(),
    };
    let failure_key = solstone_core_system_health::DailyUnit {
        name: config.key.clone(),
        facet: unit.1.clone(),
    };
    let retry = config.metadata.get("retry_on_deterministic_failure") == Some(&Value::Bool(true));
    if !from_scratch && completed.contains(&completed_key) {
        result.terminal_units.insert(unit);
        return Ok(());
    }
    if !from_scratch && !retry && deterministic.contains_key(&failure_key) {
        result.terminal_units.insert(unit.clone());
        result.capped_units.insert(unit);
        return Ok(());
    }
    match dispatch(context, runtime, config, "daily", facet, true, Map::new()) {
        Ok(item) => {
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(item.use_id.clone()));
            if let Some(facet) = facet {
                fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            }
            log.log("talent.dispatch", context.now_ms, fields);
            pending.push(item);
        }
        Err(DispatchFailure::NotClaimed { use_id }) => {
            // Source-derived, not measured: thinking.py:2316/2444 records a
            // patient-ladder request loss separately from a send failure.
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(use_id));
            fields.insert("state".to_owned(), Value::String("request_lost".to_owned()));
            if let Some(facet) = facet {
                fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            }
            log.log("talent.fail", context.now_ms, fields);
            result.failed += 1;
            result
                .failed_names
                .push(label(&config.key, facet, "request_lost"));
        }
        Err(DispatchFailure::Unavailable) => {
            result.failed += 1;
            result.failed_names.push(label(&config.key, facet, "send"));
        }
    }
    Ok(())
}

fn log_skip(
    log: &mut RunLogWriter<std::fs::File>,
    context: &ThinkContext,
    name: &str,
    reason: &str,
    facet: Option<&str>,
) {
    let mut fields = Map::new();
    fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
    fields.insert("day".to_owned(), Value::String(context.day.clone()));
    fields.insert("name".to_owned(), Value::String(name.to_owned()));
    fields.insert("reason".to_owned(), Value::String(reason.to_owned()));
    if let Some(facet) = facet {
        fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    log.log("talent.skip", context.now_ms, fields);
}

fn drain_if_full(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
    max: i64,
) {
    if max != 0 && pending.len() as i64 >= max {
        merge(result, drain(context, runtime, std::mem::take(pending)));
    }
}

fn merge(into: &mut ModeResult, from: ModeResult) {
    for label in &from.success_names {
        if let Some(unit) = into
            .applicable_units
            .iter()
            .find(|(name, facet)| crate::dispatch::item_label(name, facet.as_deref()) == *label)
        {
            into.terminal_units.insert(unit.clone());
        }
    }
    merge_mode_result(into, from);
}

fn label(name: &str, facet: Option<&str>, reason: &str) -> String {
    facet.map_or_else(
        || format!("{name} ({reason})"),
        |facet| format!("{name}/{facet} ({reason})"),
    )
}
