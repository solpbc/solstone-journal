// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_system_health::{FilesystemHealthLogSource, read_completed_since};
use solstone_core_talent_config::{TalentConfig, TalentFilter, load_talent_configs};

use crate::cadence_state::CadenceState;
use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{ModeResult, dispatch, drain, grouped, merge_mode_result, runtime};
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
                    let one = drain(context, &runtime, vec![item]);
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
