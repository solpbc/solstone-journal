// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentConfig, TalentFilter, load_talent_configs};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, dispatch,
    drain_with_deadline_observed, excluded, grouped, merge_mode_result, runtime,
    use_log_failure_detail,
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
                        log,
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
                    log,
                    &mut pending,
                    &mut group_result,
                    max_concurrency,
                );
            }
        }
        merge(
            &mut group_result,
            drain_and_log(context, &runtime, log, std::mem::take(&mut pending)),
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
    log: &mut RunLogWriter,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
    max: i64,
) {
    if max != 0 && pending.len() as i64 >= max {
        merge(
            result,
            drain_and_log(context, runtime, log, std::mem::take(pending)),
        );
    }
}

/// The shared drain call for both `drain_if_full` (mid-batch) and the
/// end-of-group drain: was a plain `drain()` (a no-op observer) until
/// 2026-09-21, so the aggregate success/failed counts here were always
/// right, but a genuine weekly-talent failure (weekly_reflection, partner)
/// wrote no talent.fail row at all -- only the request_lost dispatch-claim
/// failure in `queue` above did.
fn drain_and_log(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    log: &mut RunLogWriter,
    pending: Vec<PendingUse>,
) -> ModeResult {
    drain_with_deadline_observed(
        context,
        runtime,
        pending,
        Some(DEFAULT_THINK_TIMEOUT),
        &mut |item, outcome| log_weekly_terminal(log, context, item, outcome),
    )
}

/// One `PendingUse`'s terminal event: `talent.complete` on `Finish`, or
/// `talent.fail` on `Fail` carrying `reason_code` plus whatever detail
/// `use_log_failure_detail` finds on the durable use log (the same
/// mechanism `daily`/`segment`/`activity`/`flush` already use).
fn log_weekly_terminal(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    item: &PendingUse,
    outcome: DrainOutcome,
) {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("weekly".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("name".to_owned(), Value::String(item.name.clone())),
        ("use_id".to_owned(), Value::String(item.use_id.clone())),
    ]);
    if let Some(facet) = item.facet.as_deref() {
        fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    match outcome {
        DrainOutcome::Finish => {
            log.log("talent.complete", context.event_now_ms(), fields);
        }
        DrainOutcome::Fail { state, cause } => {
            fields.insert("state".to_owned(), Value::String(state.to_owned()));
            let reason_code = cause.unwrap_or_else(|| state.to_owned());
            if let Some(detail) = use_log_failure_detail(&context.journal, &item.use_id) {
                fields.insert("detail".to_owned(), detail);
            }
            fields.insert("reason_code".to_owned(), Value::String(reason_code));
            log.log("talent.fail", context.event_now_ms(), fields);
        }
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

#[cfg(test)]
mod terminal_event_tests {
    use super::*;

    // AC: 2026-09-21. weekly's group drain used to be a plain, no-op-observer
    // drain(), so a genuine weekly-talent failure (weekly_reflection, partner)
    // updated the aggregate failed count but wrote no talent.fail row at all --
    // only the separate request_lost dispatch-claim path did. This proves the
    // fix: log_weekly_terminal must write both event kinds, a failure carries
    // whatever detail use_log_failure_detail finds on the durable use log, and
    // the facet (when present) is carried on both event kinds.
    #[test]
    fn log_weekly_terminal_writes_complete_and_fail_events_with_detail_and_facet() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260921";
        let use_dir = root.join("talents/partner");
        std::fs::create_dir_all(&use_dir).unwrap();
        std::fs::write(
            use_dir.join("use-fail.jsonl"),
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-fail\"}\n",
                "{\"event\":\"error\",\"terminal\":true,\"name\":\"partner\",",
                "\"error\":\"agent stuck\",\"reason_code\":\"agent_stuck\",",
                "\"retryable\":false}\n",
            ),
        )
        .unwrap();

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "weekly");

        let finished = PendingUse {
            use_id: "use-ok".to_owned(),
            name: "partner".to_owned(),
            facet: Some("work".to_owned()),
            output_path: None,
            index_output: false,
        };
        log_weekly_terminal(&mut log, &context, &finished, DrainOutcome::Finish);

        let failed = PendingUse {
            use_id: "use-fail".to_owned(),
            name: "partner".to_owned(),
            facet: Some("work".to_owned()),
            output_path: None,
            index_output: false,
        };
        log_weekly_terminal(
            &mut log,
            &context,
            &failed,
            DrainOutcome::Fail {
                state: "error",
                cause: Some("agent_stuck".to_owned()),
            },
        );
        log.finish().unwrap();

        let health_dir = root.join("chronicle").join(day).join("health");
        let mut rows = Vec::new();
        for entry in std::fs::read_dir(&health_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                rows.push(serde_json::from_str::<Value>(line).unwrap());
            }
        }
        let complete = rows
            .iter()
            .find(|row| row["event"] == "talent.complete" && row["use_id"] == "use-ok")
            .expect("talent.complete row for the finished item");
        assert_eq!(complete["mode"], "weekly");
        assert_eq!(complete["facet"], "work");

        let fail = rows
            .iter()
            .find(|row| row["event"] == "talent.fail" && row["use_id"] == "use-fail")
            .expect("talent.fail row for the failed item");
        assert_eq!(fail["facet"], "work");
        assert_eq!(fail["reason_code"], "agent_stuck");
        assert_eq!(fail["detail"]["retryable"], false);
        assert!(fail["detail"].get("error").is_none());
    }
}
