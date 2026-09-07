// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::activity_work::ActivityWork;
use serde_json::{Map, Value};
use solstone_core_cortex_client::{
    UseEndState, UseFileStatus, get_use_end_state, read_use_events, use_file_status,
};
use solstone_core_facets::get_activity_record;
use solstone_core_talent_config::{TalentFilter, get_output_name, load_talent_configs};
use solstone_core_talent_runtime::activity_contract;

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, drain_with_deadline_observed,
    grouped, merge_mode_result, runtime,
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
    let input_hash = crate::segment::compute_activity_input_hash(context, &context.day, &record)
        .ok_or_else(|| "cannot fingerprint activity inputs".to_owned())?;
    let mut work = ActivityWork::begin(
        context,
        facet,
        activity_id,
        input_hash,
        configs.iter().map(|c| c.key.clone()).collect(),
        refresh,
    )?;
    if !work.due(context.event_now_ms()) {
        return Ok(failed("activity retry pending"));
    }
    work.start_attempt(context.event_now_ms())?;
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
            if !work.contains(&config.key) {
                continue;
            }
            if work.parked(&config.key) {
                group.failed += 1;
                group
                    .failed_names
                    .push(format!("{} (requires repair)", config.key));
                continue;
            }
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
                work.complete(&config.key)?;
                continue;
            }
            // Reattach an accepted use after a caller crash or lost wait; never
            // submit another request while the previous use is still active.
            let previous = work.use_id(&config.key).map(str::to_owned);
            let mut reserved = None;
            let resume = if let Some(id) = previous {
                let status = use_file_status(&context.journal.join("talents"), &id)
                    .map_err(|e| e.to_string())?;
                if status == UseFileStatus::Running
                    || get_use_end_state(&context.journal, &id).map_err(|e| e.to_string())?
                        == UseEndState::Finish
                {
                    // The shared drain folds durable finishes and preserves
                    // output-indexing behavior, including after a lost wait.
                    Some(id)
                } else {
                    if status == UseFileStatus::NotFound {
                        reserved = Some(id);
                    }
                    None
                }
            } else {
                None
            };
            match queue(
                context,
                &runtime,
                &config,
                &record,
                activity_id,
                facet,
                kind,
                refresh,
                resume,
                reserved,
                &mut work,
            ) {
                Ok(item) => {
                    work.dispatched(&config.key, &item.use_id)?;
                    log_dispatch(log, context, &config.key, activity_id, facet, &item);
                    pending.push(item);
                }
                Err(DispatchFailure::NotClaimed { use_id }) => {
                    work.dispatched(&config.key, &use_id)?;
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
                        &mut work,
                    )?,
                );
            }
        }
        merge(
            &mut group,
            drain_activity(
                context,
                log,
                &runtime,
                pending,
                activity_id,
                facet,
                &mut work,
            )?,
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
    log.finish()?;
    work.finish(context)?;
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
    resume: Option<String>,
    reserved: Option<String>,
    work: &mut ActivityWork,
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
    if let Some(use_id) = resume {
        return Ok(PendingUse {
            use_id,
            name: config.key.clone(),
            facet: Some(facet.to_owned()),
            output_path: request
                .get("output_path")
                .and_then(Value::as_str)
                .map(std::path::PathBuf::from),
            index_output: generate && format != "json",
        });
    }
    let request = solstone_core_cortex_client::CortexRequest::new(
        if generate {
            String::new()
        } else {
            activity_contract::cogitate_prompt(activity_id, kind, facet, &context.day)
        },
        config.key.clone(),
    )
    .with_config(request);
    let use_id =
        context
            .cortex
            .dispatch_prepared(runtime, &request, reserved.as_deref(), &mut |id| {
                work.dispatched(&config.key, id)
                    .map_err(std::io::Error::other)
            })?;
    Ok(PendingUse {
        use_id,
        name: config.key.clone(),
        facet: Some(facet.to_owned()),
        output_path: request
            .config
            .get("output_path")
            .and_then(Value::as_str)
            .map(std::path::PathBuf::from),
        index_output: generate && format != "json",
    })
}

#[allow(clippy::too_many_arguments)] // Mode identity plus its durable completion owner.
fn drain_activity(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    activity_id: &str,
    facet: &str,
    work: &mut ActivityWork,
) -> Result<ModeResult, String> {
    let mut persistence_error = None;
    let result = drain_with_deadline_observed(
        context,
        runtime,
        pending,
        Some(DEFAULT_THINK_TIMEOUT),
        &mut |item, outcome| {
            let saved = match outcome {
                DrainOutcome::Finish => {
                    log_complete(
                        log,
                        context,
                        activity_id,
                        facet,
                        &item.name,
                        &item.use_id,
                        "finish",
                    );
                    log.finish().and_then(|()| work.complete(&item.name))
                }
                DrainOutcome::Fail { state, cause } => {
                    log_fail(
                        log,
                        context,
                        activity_id,
                        facet,
                        &item.name,
                        Some(&item.use_id),
                        state,
                        cause.as_deref(),
                    );
                    // The worker's explicit non-retryable terminal is respected;
                    // connection interruption never enters that parked set.
                    if cause.as_deref() != Some("local_endpoint_unreachable")
                        && read_use_events(&context.journal, &item.use_id)
                            .ok()
                            .is_some_and(|events| {
                                events
                                    .iter()
                                    .rev()
                                    .find(|e| {
                                        e["event"] == "error"
                                            && e.get("terminal")
                                                .and_then(Value::as_bool)
                                                .unwrap_or(true)
                                    })
                                    .is_some_and(|e| e["retryable"] == false)
                            })
                    {
                        work.park(&item.name)
                    } else {
                        Ok(())
                    }
                }
            };
            if let Err(error) = saved {
                persistence_error.get_or_insert(error);
            }
        },
    );
    if let Some(error) = persistence_error {
        Err(error)
    } else {
        Ok(result)
    }
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
