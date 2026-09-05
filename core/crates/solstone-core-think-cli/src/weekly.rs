// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentConfig, TalentFilter, load_talent_configs};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    ModeResult, PendingUse, dispatch, drain, excluded, grouped, merge_mode_result, runtime,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Port of `thinking.py:2559-2957`: sorted priority groups, multi-facet
/// expansion, stream exclusion, bounded batches, and a final group drain.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    force: bool,
    stream: Option<&str>,
    max_concurrency: i64,
) -> Result<ModeResult, String> {
    let configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("weekly"),
            include_disabled: false,
        },
    )?;
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }
    let status = Map::from_iter([
        ("mode".to_owned(), Value::String("weekly".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("agents_total".to_owned(), Value::from(configs.len())),
        ("agents_completed".to_owned(), Value::from(0)),
    ]);
    context.status.update(status.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", status);
    let facets =
        solstone_core_facets::list_declared_facet_names(&context.journal).unwrap_or_default();
    // Source-derived, not measured: thinking.py:2606-2608 loads this day's
    // active facets before multi-facet expansion, and 2689-2696 records
    // `no_active_facets` for an inactive non-`always` facet.
    let active_facets =
        solstone_core_system::activity_state::active_facets(&context.journal, &context.day);
    let runtime = runtime()?;
    let mut total = ModeResult::default();
    for (priority, group) in grouped(configs) {
        let mut start = Map::new();
        start.insert("mode".to_owned(), Value::String("weekly".to_owned()));
        start.insert("day".to_owned(), Value::String(context.day.clone()));
        start.insert("priority".to_owned(), Value::from(priority));
        start.insert("count".to_owned(), Value::from(group.len()));
        log.log("group.start", context.now_ms, start);
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
                    queue(
                        context,
                        log,
                        &runtime,
                        &config,
                        Some(facet),
                        force,
                        &mut pending,
                        &mut group_result,
                    );
                    drain_if_full(
                        context,
                        &runtime,
                        &mut pending,
                        &mut group_result,
                        max_concurrency,
                    );
                }
            } else {
                queue(
                    context,
                    log,
                    &runtime,
                    &config,
                    None,
                    force,
                    &mut pending,
                    &mut group_result,
                );
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
        let mut complete = Map::new();
        complete.insert("mode".to_owned(), Value::String("weekly".to_owned()));
        complete.insert("day".to_owned(), Value::String(context.day.clone()));
        complete.insert("priority".to_owned(), Value::from(priority));
        complete.insert("success".to_owned(), Value::from(group_result.success));
        complete.insert("failed".to_owned(), Value::from(group_result.failed));
        log.log("group.complete", context.now_ms, complete);
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
            ("mode".to_owned(), Value::String("weekly".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("success".to_owned(), Value::from(total.success)),
            ("failed".to_owned(), Value::from(total.failed)),
        ]),
    );
    log.summary(
        context.now_ms,
        format!(
            "think --weekly{}",
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
    reason = "The weekly dispatch boundary keeps the reference's facet, force, and batch state visible."
)]
fn queue(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    config: &TalentConfig,
    facet: Option<&str>,
    force: bool,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
) {
    match dispatch(context, runtime, config, "weekly", facet, force, Map::new()) {
        Ok(item) => {
            // Source-derived, not measured: thinking.py:2875 records every accepted weekly dispatch.
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("weekly".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(item.use_id.clone()));
            if let Some(facet) = facet {
                fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            }
            log.log("talent.dispatch", context.event_now_ms(), fields);
            pending.push(item);
        }
        Err(DispatchFailure::NotClaimed { use_id }) => {
            // Source-derived, not measured: thinking.py:2753/2856 retains a
            // never-claimed request as `request_lost`, not a send failure.
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("weekly".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(use_id));
            fields.insert("state".to_owned(), Value::String("request_lost".to_owned()));
            if let Some(facet) = facet {
                fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            }
            log.log("talent.fail", context.event_now_ms(), fields);
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
}

fn log_skip(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    reason: &str,
    facet: Option<&str>,
) {
    let mut fields = Map::new();
    fields.insert("mode".to_owned(), Value::String("weekly".to_owned()));
    fields.insert("day".to_owned(), Value::String(context.day.clone()));
    fields.insert("name".to_owned(), Value::String(name.to_owned()));
    fields.insert("reason".to_owned(), Value::String(reason.to_owned()));
    if let Some(facet) = facet {
        fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    log.log("talent.skip", context.event_now_ms(), fields);
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
    merge_mode_result(into, from);
}
fn label(name: &str, facet: Option<&str>, reason: &str) -> String {
    facet.map_or_else(
        || format!("{name} ({reason})"),
        |facet| format!("{name}/{facet} ({reason})"),
    )
}
