// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_cortex_client::{TimedOutUse, UseEndState};
use solstone_core_facets::get_activity_record;
use solstone_core_talent_config::{TalentFilter, get_output_name, load_talent_configs};
use solstone_core_talent_runtime::activity_contract;

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, ModeResult, PendingUse, dispatch_direct, failure_cause, grouped,
    item_label, maybe_rescan_output, merge_mode_result, named_failure, runtime, timeout_cause,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Port of `thinking.py:3084-3499`. Activity records select matching talents,
/// discard synthetic/empty-span records, apply the low-level work guard, then
/// run sorted priority batches with the fixed 610-second deadline.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity_id: &str,
    facet: &str,
    refresh: bool,
    max_concurrency: i64,
) -> Result<ModeResult, String> {
    let Some(record) = get_activity_record(&context.journal, facet, &context.day, activity_id)
        .map_err(|error| error.to_string())?
    else {
        // Source-derived, not measured: thinking.py:3109-3117 treats a missing
        // activity record as failure rather than an empty successful run.
        return Ok(failed("activity record"));
    };
    if activity_contract::is_synthetic(&record) || !activity_contract::has_nonempty_span(&record) {
        // Source-derived, not measured: thinking.py:3122-3130 skips synthetic
        // records and records with no input span as a successful no-op.
        return Ok(ModeResult::default());
    }
    let kind = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("activity"),
            include_disabled: false,
        },
    )?
    .into_iter()
    .filter(|config| activity_contract::matches_activity(&config.metadata, kind))
    .collect::<Vec<_>>();
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }

    let groups = grouped(configs);
    let total_count = groups.values().map(Vec::len).sum::<usize>();
    let start = fields(
        context,
        activity_id,
        facet,
        Map::from_iter([
            ("count".to_owned(), Value::from(total_count)),
            ("groups".to_owned(), Value::from(groups.len())),
            ("agents_total".to_owned(), Value::from(total_count)),
            ("agents_completed".to_owned(), Value::from(0)),
        ]),
    );
    context.status.update(start.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", start.clone());
    log.log("started", context.now_ms, start);

    let runtime = runtime()?;
    let mut total = ModeResult::default();
    for (priority, configs) in groups {
        log.log(
            "group.start",
            context.now_ms,
            fields(
                context,
                activity_id,
                facet,
                Map::from_iter([
                    ("priority".to_owned(), Value::from(priority)),
                    ("count".to_owned(), Value::from(configs.len())),
                ]),
            ),
        );
        let mut pending = Vec::new();
        let mut group = ModeResult::default();
        for config in configs {
            if activity_contract::skips_low_level_work(&config.key, kind, &record) {
                // Source-derived, not measured: thinking.py:3330-3343 skips
                // `work` below 0.4 for browsing and reading activities.
                log.log(
                    "talent.skip",
                    context.event_now_ms(),
                    fields(
                        context,
                        activity_id,
                        facet,
                        Map::from_iter([
                            ("name".to_owned(), Value::String(config.key.clone())),
                            (
                                "reason".to_owned(),
                                Value::String("low_level_activity".to_owned()),
                            ),
                        ]),
                    ),
                );
                continue;
            }
            match queue(
                context,
                &runtime,
                &config,
                &record,
                activity_id,
                facet,
                kind,
                refresh,
            ) {
                Ok(item) => {
                    log_dispatch(log, context, &config.key, activity_id, facet, &item);
                    pending.push(item);
                }
                Err(DispatchFailure::NotClaimed { use_id }) => {
                    group.failed += 1;
                    group
                        .failed_names
                        .push(format!("{} (request_lost)", config.key));
                    log_fail(
                        log,
                        context,
                        activity_id,
                        facet,
                        &config.key,
                        Some(&use_id),
                        "request_lost",
                        Some("request_lost"),
                    );
                }
                Err(DispatchFailure::Unavailable) => {
                    group.failed += 1;
                    group.failed_names.push(format!("{} (send)", config.key));
                    log_fail(
                        log,
                        context,
                        activity_id,
                        facet,
                        &config.key,
                        None,
                        "send_failed",
                        Some("send_failed"),
                    );
                }
            }
            if max_concurrency != 0 && pending.len() as i64 >= max_concurrency {
                merge(
                    &mut group,
                    drain_activity(
                        context,
                        log,
                        &runtime,
                        std::mem::take(&mut pending),
                        activity_id,
                        facet,
                    ),
                );
            }
        }
        merge(
            &mut group,
            drain_activity(context, log, &runtime, pending, activity_id, facet),
        );
        log.log(
            "group.complete",
            context.now_ms,
            fields(
                context,
                activity_id,
                facet,
                Map::from_iter([
                    ("priority".to_owned(), Value::from(priority)),
                    ("success".to_owned(), Value::from(group.success)),
                    ("failed".to_owned(), Value::from(group.failed)),
                ]),
            ),
        );
        merge(&mut total, group);
        context.status.update(Map::from_iter([(
            "agents_completed".to_owned(),
            Value::from(total.success + total.failed),
        )]));
    }
    // Source-derived, not measured: thinking.py:3456-3465 writes a terminal
    // activity completion record after every priority group has drained.
    let completed = fields(
        context,
        activity_id,
        facet,
        Map::from_iter([
            ("success".to_owned(), Value::from(total.success)),
            ("failed".to_owned(), Value::from(total.failed)),
            ("duration_ms".to_owned(), Value::from(0)),
        ]),
    );
    let _ = helpers::emit(
        &context.journal,
        context.now_ms,
        "completed",
        completed.clone(),
    );
    log.summary(
        context.now_ms,
        format!(
            "think --activity {activity_id}{}",
            if total.failed == 0 {
                String::new()
            } else {
                format!(" failed={}", total.failed)
            }
        ),
    );
    log.log("completed", context.now_ms, completed);
    Ok(total)
}

#[allow(
    clippy::too_many_arguments,
    reason = "Mirrors the activity request shape at thinking.py:3345-3383."
)]
fn queue(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    config: &solstone_core_talent_config::TalentConfig,
    record: &Map<String, Value>,
    activity_id: &str,
    facet: &str,
    kind: &str,
    refresh: bool,
) -> Result<PendingUse, DispatchFailure> {
    let generate = activity_contract::is_explicit_generate(&config.metadata);
    let format = config
        .metadata
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or("md");
    let mut request = Map::from_iter([
        ("facet".to_owned(), Value::String(facet.to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("activity".to_owned(), Value::Object(record.clone())),
        ("schedule".to_owned(), Value::String("activity".to_owned())),
        (
            "env".to_owned(),
            Value::Object(Map::from_iter([
                ("SOL_DAY".to_owned(), Value::String(context.day.clone())),
                ("SOL_FACET".to_owned(), Value::String(facet.to_owned())),
                (
                    "SOL_ACTIVITY".to_owned(),
                    Value::String(activity_id.to_owned()),
                ),
            ])),
        ),
        (
            "output_path".to_owned(),
            Value::String(
                context
                    .journal
                    .join("facets")
                    .join(facet)
                    .join("activities")
                    .join(&context.day)
                    .join(activity_id)
                    .join(format!(
                        "{}.{}",
                        get_output_name(&config.key),
                        if format == "json" { "json" } else { "md" }
                    ))
                    .display()
                    .to_string(),
            ),
        ),
    ]);
    if let Some(span) = record.get("segments") {
        request.insert("span".to_owned(), span.clone());
    }
    if generate {
        request.insert("output".to_owned(), Value::String(format.to_owned()));
        if refresh {
            request.insert("refresh".to_owned(), Value::Bool(true));
        }
    }
    dispatch_direct(
        context,
        runtime,
        &config.key,
        if generate {
            String::new()
        } else {
            activity_contract::cogitate_prompt(activity_id, kind, facet, &context.day)
        },
        request,
        Some(facet),
    )
}

fn drain_activity(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    activity_id: &str,
    facet: &str,
) -> ModeResult {
    let mut result = ModeResult::default();
    if pending.is_empty() {
        return result;
    }
    let ids = pending
        .iter()
        .map(|item| item.use_id.clone())
        .collect::<Vec<_>>();
    let report = match context
        .cortex
        .wait(runtime, &ids, Some(DEFAULT_THINK_TIMEOUT))
    {
        Ok(report) => report,
        Err(error) => {
            let wait_error = format!("wait failed: {error:?}");
            for item in pending {
                result.failed += 1;
                let cause = failure_cause(&context.journal, &item.use_id, &wait_error);
                result
                    .failed_names
                    .push(named_failure(&item_label(&item.name, Some(facet)), &cause));
                log_fail(
                    log,
                    context,
                    activity_id,
                    facet,
                    &item.name,
                    Some(&item.use_id),
                    "unknown",
                    Some(&cause),
                );
            }
            return result;
        }
    };
    for item in pending {
        let label = item_label(&item.name, Some(facet));
        if let Some(timeout) = report
            .timed_out
            .iter()
            .find(|timeout| timeout.use_id() == item.use_id)
        {
            result.failed += 1;
            let cause = failure_cause(&context.journal, &item.use_id, timeout_cause(timeout));
            result.failed_names.push(named_failure(&label, &cause));
            let state = match timeout {
                TimedOutUse::LostAtDeadline { .. } => "unknown",
                TimedOutUse::GenuineTimeout { .. } => "running",
            };
            log_fail(
                log,
                context,
                activity_id,
                facet,
                &item.name,
                Some(&item.use_id),
                state,
                Some(&cause),
            );
            continue;
        }
        match report.completed.get(&item.use_id) {
            Some(completion) if completion.end_state == UseEndState::Finish => {
                maybe_rescan_output(context, &item, completion);
                result.success += 1;
                result.success_names.push(label);
                log_complete(
                    log,
                    context,
                    activity_id,
                    facet,
                    &item.name,
                    &item.use_id,
                    "finish",
                );
            }
            Some(completion) => {
                result.failed += 1;
                let cause = failure_cause(
                    &context.journal,
                    &item.use_id,
                    completion.end_state.as_str(),
                );
                result.failed_names.push(named_failure(&label, &cause));
                log_fail(
                    log,
                    context,
                    activity_id,
                    facet,
                    &item.name,
                    Some(&item.use_id),
                    completion.end_state.as_str(),
                    Some(&cause),
                );
            }
            None => {
                result.failed += 1;
                let cause = failure_cause(&context.journal, &item.use_id, "unknown");
                result.failed_names.push(named_failure(&label, &cause));
                log_fail(
                    log,
                    context,
                    activity_id,
                    facet,
                    &item.name,
                    Some(&item.use_id),
                    "unknown",
                    Some(&cause),
                );
            }
        }
    }
    result
}

fn fields(
    context: &ThinkContext,
    activity: &str,
    facet: &str,
    mut extra: Map<String, Value>,
) -> Map<String, Value> {
    extra.insert("mode".to_owned(), Value::String("activity".to_owned()));
    extra.insert("day".to_owned(), Value::String(context.day.clone()));
    extra.insert("activity".to_owned(), Value::String(activity.to_owned()));
    extra.insert("facet".to_owned(), Value::String(facet.to_owned()));
    extra
}

fn log_dispatch(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    activity: &str,
    facet: &str,
    item: &PendingUse,
) {
    let base = fields(
        context,
        activity,
        facet,
        Map::from_iter([
            ("name".to_owned(), Value::String(name.to_owned())),
            ("use_id".to_owned(), Value::String(item.use_id.clone())),
        ]),
    );
    // Source-derived, not measured: thinking.py:3403-3417 records both the
    // accepted start and the durable `talent.dispatch` sidecar event.
    let event_ms = context.event_now_ms();
    log.log("talent.started", event_ms, base.clone());
    log.log("talent.dispatch", event_ms, base);
}

fn log_complete(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    activity: &str,
    facet: &str,
    name: &str,
    use_id: &str,
    state: &str,
) {
    let base = fields(
        context,
        activity,
        facet,
        Map::from_iter([
            ("name".to_owned(), Value::String(name.to_owned())),
            ("use_id".to_owned(), Value::String(use_id.to_owned())),
            ("state".to_owned(), Value::String(state.to_owned())),
        ]),
    );
    let event_ms = context.event_now_ms();
    log.log("talent.completed", event_ms, base.clone());
    log.log("talent.complete", event_ms, base);
}

#[allow(clippy::too_many_arguments)] // An activity terminal is keyed by activity+facet+use.
fn log_fail(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    activity: &str,
    facet: &str,
    name: &str,
    use_id: Option<&str>,
    state: &str,
    reason: Option<&str>,
) {
    let mut extra = Map::from_iter([
        ("name".to_owned(), Value::String(name.to_owned())),
        ("state".to_owned(), Value::String(state.to_owned())),
    ]);
    if let Some(use_id) = use_id {
        extra.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
    }
    // The caller already computed this cause for the operator-facing name; recording it is
    // what makes a `talent.fail` row explainable. Without it the durable record carries only
    // `state`, and 195 of 374 failures on 2026-09-04 were unexplained by construction.
    if let Some(reason) = reason {
        extra.insert("reason_code".to_owned(), Value::String(reason.to_owned()));
    }
    let base = fields(context, activity, facet, extra);
    let event_ms = context.event_now_ms();
    log.log("talent.completed", event_ms, base.clone());
    log.log("talent.fail", event_ms, base);
}

fn merge(into: &mut ModeResult, from: ModeResult) {
    merge_mode_result(into, from);
}
fn failed(name: &str) -> ModeResult {
    ModeResult {
        failed: 1,
        failed_names: vec![name.to_owned()],
        ..ModeResult::default()
    }
}
