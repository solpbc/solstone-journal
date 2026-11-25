// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Event-driven Experience composition dispatch (`journal think --day <anchor_day> --facet <facet> --experience <id>`).

use serde_json::{Map, Value};
use solstone_core_talent_config::TalentFilter;

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, dispatch,
    drain_with_deadline_observed, runtime, use_log_failure_detail,
};
use crate::run_log::RunLogWriter;

pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    facet: &str,
    experience_id: &str,
) -> Result<ModeResult, String> {
    if solstone_core_journal_config::no_thinking_engine_chosen(&context.journal) {
        return Ok(ModeResult::default());
    }
    let configs = solstone_core_talent_config::load_talent_configs_in(
        context.locale(),
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: None,
            include_disabled: false,
        },
    )?;
    let Some(config) = configs.into_iter().find(|c| c.key == "experience_compose") else {
        return Err("experience_compose talent is not installed".to_owned());
    };
    let runtime = runtime()?;
    let mut extra = Map::new();
    extra.insert(
        "experience_id".to_owned(),
        Value::String(experience_id.to_owned()),
    );
    let mut result = ModeResult::default();
    let pending: PendingUse = match dispatch(
        context,
        &runtime,
        &config,
        "experience",
        Some(facet),
        true,
        extra,
    ) {
        Ok(item) => {
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("experience".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            fields.insert(
                "experience_id".to_owned(),
                Value::String(experience_id.to_owned()),
            );
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(item.use_id.clone()));
            log.log("talent.dispatch", context.event_now_ms(), fields);
            item
        }
        Err(DispatchFailure::NotClaimed { use_id }) => {
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("experience".to_owned()));
            fields.insert("day".to_owned(), Value::String(context.day.clone()));
            fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
            fields.insert("name".to_owned(), Value::String(config.key.clone()));
            fields.insert("use_id".to_owned(), Value::String(use_id));
            fields.insert("state".to_owned(), Value::String("request_lost".to_owned()));
            log.log("talent.fail", context.event_now_ms(), fields);
            result.failed += 1;
            result
                .failed_names
                .push(format!("experience_compose:{facet}:{experience_id}:request_lost").into());
            return Ok(result);
        }
        Err(DispatchFailure::Unavailable) => {
            result.failed += 1;
            result
                .failed_names
                .push(format!("experience_compose:{facet}:{experience_id}:send").into());
            return Ok(result);
        }
    };

    let drained = drain_with_deadline_observed(
        context,
        &runtime,
        vec![pending],
        Some(DEFAULT_THINK_TIMEOUT),
        &mut |item, outcome| {
            let mut fields = Map::from_iter([
                ("mode".to_owned(), Value::String("experience".to_owned())),
                ("day".to_owned(), Value::String(context.day.clone())),
                ("facet".to_owned(), Value::String(facet.to_owned())),
                (
                    "experience_id".to_owned(),
                    Value::String(experience_id.to_owned()),
                ),
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
        },
    );
    Ok(drained)
}
