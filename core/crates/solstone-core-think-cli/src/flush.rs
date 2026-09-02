// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_cortex_client::{TimedOutUse, UseEndState};
use solstone_core_talent_config::{TalentFilter, load_talent_configs};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, ModeResult, PendingUse, blocked_runtime_reason, dispatch_direct,
    failure_cause, item_label, merge_mode_result, named_failure, runtime, timeout_cause,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Port of `thinking.py:3500-3712`: flush filters `hook.flush`, records each
/// accepted request and terminal outcome, and waits once with a fixed 610s cap.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    segment: &str,
    stream: Option<&str>,
) -> Result<ModeResult, String> {
    let configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("segment"),
            include_disabled: false,
        },
    )?
    .into_iter()
    .filter(has_flush_hook)
    .collect::<Vec<_>>();
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }
    // Source-derived, not measured: thinking.py:3538-3554 records the flush
    // run before dispatching the eligible hooks.
    let start = fields(
        context,
        segment,
        Map::from_iter([
            ("count".to_owned(), Value::from(configs.len())),
            ("agents_total".to_owned(), Value::from(configs.len())),
            ("agents_completed".to_owned(), Value::from(0)),
        ]),
    );
    context.status.update(start.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", start.clone());
    log.log_event("started", context.now_ms, start);
    let runtime = runtime()?;
    let mut pending = Vec::new();
    let mut result = ModeResult::default();
    for config in configs {
        match queue(context, &runtime, &config, segment, stream) {
            Ok(item) => {
                log_dispatch(log, context, segment, &config.key, &item);
                pending.push(item);
            }
            Err(DispatchFailure::NotClaimed { use_id }) => {
                result.failed += 1;
                result
                    .failed_names
                    .push(format!("{} (request_lost)", config.key));
                log_fail(
                    log,
                    context,
                    segment,
                    &config.key,
                    Some(&use_id),
                    "request_lost",
                );
            }
            Err(DispatchFailure::Unavailable) => {
                result.failed += 1;
                result.failed_names.push(format!("{} (send)", config.key));
                log_fail(log, context, segment, &config.key, None, "send_failed");
            }
        }
    }
    merge(&mut result, drain(context, log, &runtime, pending, segment));
    context.status.update(Map::from_iter([(
        "agents_completed".to_owned(),
        Value::from(result.success + result.failed),
    )]));
    // Source-derived, not measured: thinking.py:3682-3703 records the
    // aggregate result after the fixed-deadline wait.
    let completed = fields(
        context,
        segment,
        Map::from_iter([
            ("success".to_owned(), Value::from(result.success)),
            ("failed".to_owned(), Value::from(result.failed)),
            ("duration_ms".to_owned(), Value::from(0)),
        ]),
    );
    let _ = helpers::emit(
        &context.journal,
        context.now_ms,
        "completed",
        completed.clone(),
    );
    helpers::day_log(
        &context.journal,
        &context.day,
        context.now_ms,
        &format!(
            "think --flush {segment}{}",
            if result.failed == 0 {
                String::new()
            } else {
                format!(" failed={}", result.failed)
            }
        ),
    );
    log.log_event("completed", context.now_ms, completed);
    Ok(result)
}

fn has_flush_hook(config: &solstone_core_talent_config::TalentConfig) -> bool {
    config
        .metadata
        .get("hook")
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("flush"))
        .and_then(Value::as_bool)
        == Some(true)
}

fn queue(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    config: &solstone_core_talent_config::TalentConfig,
    segment: &str,
    stream: Option<&str>,
) -> Result<PendingUse, DispatchFailure> {
    let generate = config.metadata.get("type").and_then(Value::as_str) == Some("generate");
    let mut request = Map::from_iter([
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("flush".to_owned(), Value::Bool(true)),
        ("refresh".to_owned(), Value::Bool(true)),
        ("schedule".to_owned(), Value::String("segment".to_owned())),
    ]);
    let mut env = Map::from_iter([
        ("SOL_DAY".to_owned(), Value::String(context.day.clone())),
        ("SOL_SEGMENT".to_owned(), Value::String(segment.to_owned())),
    ]);
    if let Some(stream) = stream {
        request.insert("stream".to_owned(), Value::String(stream.to_owned()));
        env.insert("SOL_STREAM".to_owned(), Value::String(stream.to_owned()));
    }
    request.insert("env".to_owned(), Value::Object(env));
    if generate {
        request.insert(
            "output".to_owned(),
            Value::String(
                config
                    .metadata
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("md")
                    .to_owned(),
            ),
        );
    }
    dispatch_direct(context, runtime, &config.key, String::new(), request, None)
}

fn drain(
    context: &ThinkContext,
    log: &mut RunLogWriter<std::fs::File>,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    segment: &str,
) -> ModeResult {
    let mut result = ModeResult::default();
    if pending.is_empty() {
        return result;
    }
    let ids = pending
        .iter()
        .map(|item| item.use_id.clone())
        .collect::<Vec<_>>();
    let Ok(report) = context
        .cortex
        .wait(runtime, &ids, Some(DEFAULT_THINK_TIMEOUT))
    else {
        let cause =
            blocked_runtime_reason(&context.journal).unwrap_or_else(|| "unavailable".to_owned());
        for item in pending {
            result.failed += 1;
            result
                .failed_names
                .push(named_failure(&item_label(&item.name, None), &cause));
            log_fail(
                log,
                context,
                segment,
                &item.name,
                Some(&item.use_id),
                "unknown",
            );
        }
        return result;
    };
    for item in pending {
        let label = item_label(&item.name, None);
        if let Some(timeout) = report
            .timed_out
            .iter()
            .find(|timeout| timeout.use_id() == item.use_id)
        {
            result.failed += 1;
            result.failed_names.push(named_failure(
                &label,
                blocked_runtime_reason(&context.journal)
                    .as_deref()
                    .unwrap_or_else(|| timeout_cause(timeout)),
            ));
            log_fail(
                log,
                context,
                segment,
                &item.name,
                Some(&item.use_id),
                match timeout {
                    TimedOutUse::LostAtDeadline { .. } => "unknown",
                    TimedOutUse::GenuineTimeout { .. } => "running",
                },
            );
            continue;
        }
        match report.completed.get(&item.use_id) {
            Some(completion) if completion.end_state == UseEndState::Finish => {
                result.success += 1;
                result.success_names.push(label);
                log_complete(log, context, segment, &item.name, &item.use_id, "finish");
            }
            Some(completion) => {
                result.failed += 1;
                result.failed_names.push(named_failure(
                    &label,
                    &failure_cause(
                        &context.journal,
                        &item.use_id,
                        completion.end_state.as_str(),
                    ),
                ));
                log_fail(
                    log,
                    context,
                    segment,
                    &item.name,
                    Some(&item.use_id),
                    completion.end_state.as_str(),
                );
            }
            None => {
                result.failed += 1;
                result.failed_names.push(named_failure(
                    &label,
                    &failure_cause(&context.journal, &item.use_id, "unknown"),
                ));
                log_fail(
                    log,
                    context,
                    segment,
                    &item.name,
                    Some(&item.use_id),
                    "unknown",
                );
            }
        }
    }
    result
}

fn fields(
    context: &ThinkContext,
    segment: &str,
    mut extra: Map<String, Value>,
) -> Map<String, Value> {
    extra.insert("mode".to_owned(), Value::String("flush".to_owned()));
    extra.insert("day".to_owned(), Value::String(context.day.clone()));
    extra.insert("segment".to_owned(), Value::String(segment.to_owned()));
    extra
}
fn log_dispatch(
    log: &mut RunLogWriter<std::fs::File>,
    context: &ThinkContext,
    segment: &str,
    name: &str,
    item: &PendingUse,
) {
    let base = fields(
        context,
        segment,
        Map::from_iter([
            ("name".to_owned(), Value::String(name.to_owned())),
            ("use_id".to_owned(), Value::String(item.use_id.clone())),
        ]),
    );
    // Source-derived, not measured: thinking.py:3604-3617 records accepted flush starts and dispatches.
    log.log_event("talent.started", context.now_ms, base.clone());
    log.log_event("talent.dispatch", context.now_ms, base);
}
fn log_complete(
    log: &mut RunLogWriter<std::fs::File>,
    context: &ThinkContext,
    segment: &str,
    name: &str,
    use_id: &str,
    state: &str,
) {
    let base = fields(
        context,
        segment,
        Map::from_iter([
            ("name".to_owned(), Value::String(name.to_owned())),
            ("use_id".to_owned(), Value::String(use_id.to_owned())),
            ("state".to_owned(), Value::String(state.to_owned())),
        ]),
    );
    log.log_event("talent.completed", context.now_ms, base.clone());
    log.log_event("talent.complete", context.now_ms, base);
}
fn log_fail(
    log: &mut RunLogWriter<std::fs::File>,
    context: &ThinkContext,
    segment: &str,
    name: &str,
    use_id: Option<&str>,
    state: &str,
) {
    let mut extra = Map::from_iter([
        ("name".to_owned(), Value::String(name.to_owned())),
        ("state".to_owned(), Value::String(state.to_owned())),
    ]);
    if let Some(use_id) = use_id {
        extra.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
    }
    let base = fields(context, segment, extra);
    log.log_event("talent.completed", context.now_ms, base.clone());
    log.log_event("talent.fail", context.now_ms, base);
}
fn merge(into: &mut ModeResult, from: ModeResult) {
    merge_mode_result(into, from);
}
