// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_system_health::{FilesystemHealthLogSource, read_completed_since};
use solstone_core_talent_config::{TalentConfig, TalentFilter, load_talent_configs};

use crate::cadence_state::CadenceState;
use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, dispatch,
    drain_with_deadline_observed, grouped, merge_mode_result, runtime, use_log_failure_detail,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// `thinking.py:2969-2972` needs this separate preflight: lib.rs calls it before
/// opening the cadence run sidecar, preserving the no-talent no-write branch.
pub(crate) fn configured(context: &ThinkContext) -> Result<Vec<TalentConfig>, String> {
    load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("cadence"),
            include_disabled: false,
        },
    )
}

/// Port of `thinking.py:2960-3081`: cadence state is loaded once, the completion
/// window is attached to each request, and only a clean one-use drain advances it.
pub(crate) fn run(
    context: &ThinkContext,
    configs: Vec<TalentConfig>,
    log: &mut RunLogWriter,
    force: bool,
) -> Result<ModeResult, String> {
    let status = Map::from_iter([
        ("mode".to_owned(), Value::String("cadence".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("agents_total".to_owned(), Value::from(configs.len())),
        ("agents_completed".to_owned(), Value::from(0)),
    ]);
    context.status.update(status.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", status);
    let mut state = CadenceState::load(&context.journal);
    let source = FilesystemHealthLogSource::new(&context.journal);
    let runtime = runtime()?;
    let mut result = ModeResult::default();
    let mut dirty = false;
    for (_, configs) in grouped(configs) {
        for config in configs {
            let now = context.now_ms;
            let last = state.timestamp(&config.key);
            let minutes = config
                .metadata
                .get("cadence_minutes")
                .and_then(Value::as_i64)
                .unwrap_or(5);
            if let Some(stamp) = last.filter(|stamp| now - *stamp < minutes * 60_000) {
                // Source-derived, not measured: thinking.py:2984-2991 records
                // every closed cadence interval in the run sidecar.
                log.log(
                    "talent.skip",
                    context.event_now_ms(),
                    cadence_skip_fields(
                        context,
                        &config.key,
                        "interval_not_elapsed",
                        format!("{}s since last < {minutes}m", (now - stamp) / 1_000),
                    ),
                );
                continue;
            }
            let completed = read_completed_since(&source, &context.day, last.unwrap_or(0))
                .map_err(|error| error.to_string())?
                .value;
            if completed.segments.is_empty() && completed.activities.is_empty() {
                // Source-derived, not measured: thinking.py:2994-3001 records
                // the no-work skip instead of silently omitting the talent.
                log.log(
                    "talent.skip",
                    context.event_now_ms(),
                    cadence_skip_fields(
                        context,
                        &config.key,
                        "no_new_work",
                        "no segment/activity completed since last cadence run".to_owned(),
                    ),
                );
                continue;
            }
            let extra = Map::from_iter([(
                "cadence_window".to_owned(),
                Value::Object(Map::from_iter([
                    ("since_ms".to_owned(), Value::from(last.unwrap_or(0))),
                    (
                        "segments".to_owned(),
                        Value::Array(
                            completed
                                .segments
                                .into_iter()
                                .map(|item| serde_json::json!({"day": item.day, "segment": item.segment, "stream": item.stream, "ts": item.ts}))
                                .collect(),
                        ),
                    ),
                    (
                        "activities".to_owned(),
                        Value::Array(
                            completed
                                .activities
                                .into_iter()
                                .map(|item| serde_json::json!({"day": item.day, "activity": item.activity, "facet": item.facet, "ts": item.ts}))
                                .collect(),
                        ),
                    ),
                ])),
            )]);
            match dispatch(context, &runtime, &config, "cadence", None, force, extra) {
                Ok(item) => {
                    let mut fields = Map::new();
                    fields.insert("mode".to_owned(), Value::String("cadence".to_owned()));
                    fields.insert("day".to_owned(), Value::String(context.day.clone()));
                    fields.insert("name".to_owned(), Value::String(config.key.clone()));
                    fields.insert("use_id".to_owned(), Value::String(item.use_id.clone()));
                    log.log("talent.dispatch", context.event_now_ms(), fields);
                    // Was a plain drain() (a no-op observer) until 2026-09-21: the
                    // aggregate success/failed counts below were always right, but a
                    // genuine cadence-talent failure wrote no talent.fail row at all --
                    // only the request_lost dispatch-claim failure above did.
                    let one = drain_with_deadline_observed(
                        context,
                        &runtime,
                        vec![item],
                        Some(DEFAULT_THINK_TIMEOUT),
                        &mut |item, outcome| log_cadence_terminal(log, context, item, outcome),
                    );
                    if one.success == 1 && one.failed == 0 {
                        state.set_timestamp(&config.key, now);
                        dirty = true;
                    }
                    merge(&mut result, one);
                }
                Err(DispatchFailure::NotClaimed { use_id }) => {
                    result.failed += 1;
                    result
                        .failed_names
                        .push(format!("{} (request_lost)", config.key));
                    let fields = Map::from_iter([
                        ("mode".to_owned(), Value::String("cadence".to_owned())),
                        ("day".to_owned(), Value::String(context.day.clone())),
                        ("name".to_owned(), Value::String(config.key.clone())),
                        ("use_id".to_owned(), Value::String(use_id)),
                        ("state".to_owned(), Value::String("request_lost".to_owned())),
                    ]);
                    log.log("talent.fail", context.event_now_ms(), fields);
                }
                Err(DispatchFailure::Unavailable) => {
                    result.failed += 1;
                    result.failed_names.push(format!("{} (send)", config.key));
                }
            }
        }
    }
    if dirty {
        state.save(&context.journal)?;
    }
    context.status.update(Map::from_iter([(
        "agents_completed".to_owned(),
        Value::from(result.success + result.failed),
    )]));
    let _ = helpers::emit(
        &context.journal,
        context.now_ms,
        "completed",
        Map::from_iter([
            ("mode".to_owned(), Value::String("cadence".to_owned())),
            ("day".to_owned(), Value::String(context.day.clone())),
            ("success".to_owned(), Value::from(result.success)),
            ("failed".to_owned(), Value::from(result.failed)),
        ]),
    );
    log.summary(
        context.now_ms,
        format!(
            "think --cadence{}",
            if result.failed == 0 {
                String::new()
            } else {
                format!(" failed={}", result.failed)
            }
        ),
    );
    Ok(result)
}

/// One dispatched item's terminal event: `talent.complete` on `Finish`, or
/// `talent.fail` on `Fail` carrying `reason_code` plus whatever detail
/// `use_log_failure_detail` finds on the durable use log (the same
/// mechanism `daily`/`segment`/`activity`/`flush`/`weekly` already use).
fn log_cadence_terminal(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    item: &PendingUse,
    outcome: DrainOutcome,
) {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("cadence".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("name".to_owned(), Value::String(item.name.clone())),
        ("use_id".to_owned(), Value::String(item.use_id.clone())),
    ]);
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

fn cadence_skip_fields(
    context: &ThinkContext,
    name: &str,
    reason: &str,
    detail: String,
) -> Map<String, Value> {
    Map::from_iter([
        ("name".to_owned(), Value::String(name.to_owned())),
        ("reason".to_owned(), Value::String(reason.to_owned())),
        ("detail".to_owned(), Value::String(detail)),
        ("mode".to_owned(), Value::String("cadence".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
    ])
}

#[cfg(test)]
pub(crate) fn record_clean_fire(
    state: &mut CadenceState,
    name: &str,
    now_ms: i64,
    succeeded: bool,
) -> bool {
    if !succeeded {
        return false;
    }
    state.set_timestamp(name, now_ms);
    true
}

fn merge(into: &mut ModeResult, from: ModeResult) {
    merge_mode_result(into, from);
}

#[cfg(test)]
mod terminal_event_tests {
    use super::*;

    // AC: 2026-09-21. cadence's drain used to be a plain, no-op-observer drain(), so
    // a genuine cadence-talent failure updated the aggregate failed count but wrote
    // no talent.fail row at all -- only the separate request_lost dispatch-claim
    // path did. This proves the fix: log_cadence_terminal must write both event
    // kinds, and a failure must carry whatever detail use_log_failure_detail finds
    // on the durable use log (the same mechanism daily/segment/activity/flush use).
    #[test]
    fn log_cadence_terminal_writes_complete_and_fail_events_with_detail() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260921";
        let use_dir = root.join("talents/some_cadence_talent");
        std::fs::create_dir_all(&use_dir).unwrap();
        std::fs::write(
            use_dir.join("use-fail.jsonl"),
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-fail\"}\n",
                "{\"event\":\"error\",\"terminal\":true,\"name\":\"some_cadence_talent\",",
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
        let mut log = RunLogWriter::open(root, day, "cadence");

        let finished = PendingUse {
            use_id: "use-ok".to_owned(),
            name: "some_cadence_talent".to_owned(),
            facet: None,
            output_path: None,
            index_output: false,
        };
        log_cadence_terminal(&mut log, &context, &finished, DrainOutcome::Finish);

        let failed = PendingUse {
            use_id: "use-fail".to_owned(),
            name: "some_cadence_talent".to_owned(),
            facet: None,
            output_path: None,
            index_output: false,
        };
        log_cadence_terminal(
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
        assert_eq!(complete["mode"], "cadence");

        let fail = rows
            .iter()
            .find(|row| row["event"] == "talent.fail" && row["use_id"] == "use-fail")
            .expect("talent.fail row for the failed item");
        assert_eq!(fail["reason_code"], "agent_stuck");
        assert_eq!(fail["detail"]["retryable"], false);
        assert!(fail["detail"].get("error").is_none());
    }
}
