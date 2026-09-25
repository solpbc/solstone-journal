// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};
use solstone_core_talent_config::{TalentFilter, load_talent_configs};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, dispatch_prepared, excluded,
    failure_cause, grouped, merge_mode_result, runtime, use_log_failure_detail,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Port of `thinking.py:2086-2556`.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    stream: Option<&str>,
    from_scratch: bool,
    max_concurrency: i64,
) -> Result<ModeResult, String> {
    let configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        solstone_core_talent_config::read_talent_overrides(&context.journal)?.as_ref(),
        TalentFilter {
            r#type: None,
            schedule: Some("daily"),
            include_disabled: false,
        },
    )?;
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }
    if configs.iter().any(|config| !excluded(config, stream)) {
        solstone_core_system::daily_coverage::register_daily_day(
            &context.journal,
            &context.day,
            chrono::Utc::now(),
        )?;
    }
    let status = Map::from_iter([
        ("mode".to_owned(), Value::String("daily".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("agents_total".to_owned(), Value::from(configs.len())),
        ("agents_completed".to_owned(), Value::from(0)),
    ]);
    context.status.update(status.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "started", status);
    let facets = solstone_core_facets::list_declared_facet_names(&context.journal)
        .map_err(|e| e.to_string())?;
    // Source-derived, not measured: thinking.py:2134-2136 loads this day's
    // active facets before multi-facet expansion, and 2220-2227 records
    // `no_active_facets` for an inactive non-`always` facet.
    let active_facets = solstone_core_system::activity_state::active_facets_checked(
        &context.journal,
        &context.day,
    )?;
    // Freeze the complete applicable identity set before any contract digest is captured.
    let mut write_facets = std::collections::BTreeSet::new();
    for config in &configs {
        if excluded(config, stream) {
            continue;
        }
        let Ok(hook) =
            solstone_core_indexer::daily_evidence::daily_hook(&config.key, &config.metadata)
        else {
            continue;
        };
        if hook == "schedule" {
            write_facets.extend(facets.iter().cloned());
        } else if config.metadata.get("multi_facet") == Some(&Value::Bool(true)) {
            write_facets.extend(
                facets
                    .iter()
                    .filter(|facet| {
                        config.metadata.get("always") == Some(&Value::Bool(true))
                            || active_facets.contains(*facet)
                    })
                    .cloned(),
            );
        }
    }
    for facet in write_facets {
        solstone_core_facets::ensure_daily_facet_id(&context.journal, &facet)?;
    }
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
                    let queued = queue_daily(
                        context,
                        log,
                        &runtime,
                        &config,
                        Some(facet),
                        from_scratch,
                        &mut pending,
                        &mut group_result,
                    );
                    if let Err(error) = queued {
                        record_preparation_failure(
                            log,
                            context,
                            &config.key,
                            Some(facet),
                            &error,
                            &mut group_result,
                        );
                    }
                    drain_if_full(
                        context,
                        log,
                        &runtime,
                        &mut pending,
                        &mut group_result,
                        max_concurrency,
                    );
                }
            } else {
                let queued = queue_daily(
                    context,
                    log,
                    &runtime,
                    &config,
                    None,
                    from_scratch,
                    &mut pending,
                    &mut group_result,
                );
                if let Err(error) = queued {
                    record_preparation_failure(
                        log,
                        context,
                        &config.key,
                        None,
                        &error,
                        &mut group_result,
                    );
                }
                drain_if_full(
                    context,
                    log,
                    &runtime,
                    &mut pending,
                    &mut group_result,
                    max_concurrency,
                );
            }
        }
        merge(
            &mut group_result,
            drain_daily(context, log, &runtime, std::mem::take(&mut pending)),
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
    log.summary(
        context.now_ms,
        format!(
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

fn record_preparation_failure(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    facet: Option<&str>,
    error: &str,
    result: &mut ModeResult,
) {
    let mut fields = daily_terminal_fields_for(context, name, facet, "preparation_failed");
    fields.insert(
        "reason_code".to_owned(),
        Value::String("daily_preparation_failed".to_owned()),
    );
    fields.insert("detail".to_owned(), Value::String(error.to_owned()));
    log.log("talent.fail", context.event_now_ms(), fields);
    if name != "daily_schedule" {
        result.failed += 1;
        result.failed_names.push(label(name, facet, error));
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "The reference keeps the daily completed and deterministic-failure guards explicit at this dispatch boundary."
)]
fn queue_daily(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    config: &solstone_core_talent_config::TalentConfig,
    facet: Option<&str>,
    from_scratch: bool,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
) -> Result<(), String> {
    let unit = (config.key.clone(), facet.map(ToOwned::to_owned));
    if config.key != "daily_schedule" {
        result.applicable_units.insert(unit.clone());
    }
    let today =
        solstone_core_system::daily_coverage::local_day(&context.journal, chrono::Utc::now())?;
    let evidence_day = if config.key == "daily_schedule" {
        &today
    } else {
        &context.day
    };
    let (evidence_rev, contract_dig) = crate::snapshot::compute_daily_evidence_revision(
        &context.journal,
        evidence_day,
        config,
        facet,
        None,
    )?;
    let identity = solstone_core_journal_io::DailyUnitIdentity::new(
        &context.day,
        &config.key,
        facet.map(ToOwned::to_owned),
    );
    let existing_record =
        solstone_core_journal_io::load_daily_unit_record(&context.journal, &identity)
            .map_err(|e| e.to_string())?;
    let retry = config.metadata.get("retry_on_deterministic_failure") == Some(&Value::Bool(true));
    if let Some(record) = &existing_record {
        if record.has_uncommitted_started_receipt() {
            // An in-doubt started write cannot be automatically retried, reset, or wiped.
            return Ok(());
        }
        if !from_scratch {
            if record.status.is_terminal_success()
                && record.is_reusable_for(&evidence_rev, &contract_dig)
                && solstone_core_journal_io::accepted_daily_artifacts_valid(
                    &context.journal,
                    record,
                )
                .map_err(|e| e.to_string())?
            {
                result.terminal_units.insert(unit);
                log_skip(log, context, &config.key, "evidence_unchanged", facet);
                return Ok(());
            }
            if record.status.is_terminal_success()
                && record.is_reusable_for(&evidence_rev, &contract_dig)
            {
                log_daily_failure(
                    log,
                    context,
                    &config.key,
                    facet,
                    None,
                    "required_artifact_missing",
                    "required_artifact_missing",
                );
                if config.key != "daily_schedule" {
                    result.failed += 1;
                }
                result
                    .failed_names
                    .push(label(&config.key, facet, "required_artifact_missing"));
                return Ok(());
            }
            if !retry
                && record.evidence_revision == evidence_rev
                && record.contract_digest == contract_dig
                && record.status == solstone_core_journal_io::DailyUnitStatus::Capped
                && !(record
                    .reason_code
                    .as_deref()
                    .is_some_and(solstone_core_system::daily_coverage::environmental_failure)
                    && record.environmental_retry_day.as_deref() != Some(&today))
            {
                result.terminal_units.insert(unit.clone());
                result.capped_units.insert(unit);
                return Ok(());
            }
            if config.key == "entities:entities_review"
                && record.evidence_revision == evidence_rev
                && record.contract_digest == contract_dig
                && record.status == solstone_core_journal_io::DailyUnitStatus::Conflicting
                && record.failure_count >= 2
            {
                // One automatic retry of review conflict on unchanged evidence has exhausted.
                // Do not dispatch, but do NOT insert into terminal_units or capped_units:
                // the day stays uncertified / Outstanding.
                return Ok(());
            }
        }
    }
    let use_id = solstone_core_journal_io::cortex_use::allocate_cortex_use_id(
        &context.journal,
        context.event_now_ms(),
    )
    .map_err(|e| e.to_string())?
    .to_string();
    let mut extra = Map::from_iter([
        ("lock_token".to_owned(), Value::String(use_id.clone())),
        (
            "daily_reserved_use_id".to_owned(),
            Value::String(use_id.clone()),
        ),
        (
            "evidence_revision".to_owned(),
            Value::String(evidence_rev.clone()),
        ),
        (
            "contract_digest".to_owned(),
            Value::String(contract_dig.clone()),
        ),
    ]);
    if config.key == "daily_schedule" {
        extra.insert("day".to_owned(), Value::String(today.clone()));
    }
    if config.key == "morning_briefing" {
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            &context.journal,
            &context.day,
            &context.talent_root,
            &context.apps_root,
        )?;
        let mut upstream = Map::new();
        for unit in coverage.units {
            if !matches!(unit.identity.name.as_str(), "schedule" | "facet_newsletter") {
                continue;
            }
            let key = unit.identity.facet.as_ref().map_or_else(
                || unit.identity.name.clone(),
                |facet| format!("{}:{facet}", unit.identity.name),
            );
            let record =
                solstone_core_journal_io::load_daily_unit_record(&context.journal, &unit.identity)
                    .map_err(|e| e.to_string())?;
            let usable = unit.state == solstone_core_system::daily_coverage::CoverageState::Current
                && record.is_some_and(|r| {
                    r.status == solstone_core_journal_io::DailyUnitStatus::Committed
                        && r.accepted.as_ref().is_some_and(|a| {
                            a.status == solstone_core_journal_io::DailyUnitStatus::Committed
                        })
                });
            upstream.insert(key, Value::Bool(usable));
        }
        extra.insert("_daily_upstream".to_owned(), Value::Object(upstream));
    }
    let retained = existing_record
        .as_ref()
        .filter(|record| {
            !from_scratch
                && record.evidence_revision == evidence_rev
                && record.contract_digest == contract_dig
        })
        .and_then(|record| record.frozen_packet.clone());
    let frozen_packet = match retained {
        Some(packet) => packet,
        None => crate::snapshot::prepare_daily_packet(context, config, facet, &extra)?,
    };
    let mut reservation_error = None;
    let mut review_exhausted = false;
    let mut prepare = |reserved_use: &str| -> std::io::Result<()> {
        match reserve_daily_attempt(
            &context.journal,
            &identity,
            &evidence_rev,
            &contract_dig,
            &frozen_packet,
            &today,
            reserved_use,
            from_scratch,
            retry,
            context.event_now_ms(),
        ) {
            Ok(()) => Ok(()),
            Err(solstone_core_journal_io::DailyUnitError::ReviewOwnerConflictExhausted) => {
                review_exhausted = true;
                Err(std::io::Error::other(
                    "entities_review conflict retry exhausted",
                ))
            }
            Err(error) => {
                reservation_error = Some(error.to_string());
                Err(std::io::Error::other(error.to_string()))
            }
        }
    };

    match dispatch_prepared(
        context,
        runtime,
        config,
        crate::dispatch::DispatchSettings {
            schedule: "daily",
            facet,
            force: true,
            extra,
        },
        &mut prepare,
    ) {
        Ok(item) => {
            let mut fields = Map::new();
            fields.insert("mode".to_owned(), Value::String("daily".to_owned()));
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
            fields.insert(
                "reason_code".to_owned(),
                Value::String("request_lost".to_owned()),
            );
            log.log("talent.fail", context.event_now_ms(), fields);
            if config.key != "daily_schedule" {
                result.failed += 1;
            }
            result
                .failed_names
                .push(label(&config.key, facet, "request_lost"));
        }
        Err(DispatchFailure::Unavailable) => {
            log_daily_failure(
                log,
                context,
                &config.key,
                facet,
                None,
                "unavailable",
                "unavailable",
            );
            if config.key != "daily_schedule" {
                result.failed += 1;
            }
            result.failed_names.push(label(&config.key, facet, "send"));
        }
    }
    if review_exhausted {
        return Ok(());
    }
    if let Some(error) = reservation_error {
        return Err(error);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn reserve_daily_attempt(
    journal: &std::path::Path,
    identity: &solstone_core_journal_io::DailyUnitIdentity,
    evidence_rev: &str,
    contract_dig: &str,
    frozen_packet: &Value,
    today: &str,
    reserved_use: &str,
    from_scratch: bool,
    retry: bool,
    now_ms: i64,
) -> Result<(), solstone_core_journal_io::DailyUnitError> {
    solstone_core_journal_io::with_locked_daily_unit_record(journal, identity, |slot| {
        let mut record = slot.take().unwrap_or_else(|| {
            solstone_core_journal_io::DailyUnitRecord::new(
                identity.clone(),
                evidence_rev,
                contract_dig,
            )
        });
        if record.has_uncommitted_started_receipt() {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "unit has ambiguous uncommitted owner action".to_owned(),
            ));
        }
        let same =
            record.evidence_revision == evidence_rev && record.contract_digest == contract_dig;
        if identity.name == "entities:entities_review"
            && same
            && !from_scratch
            && record.status == solstone_core_journal_io::DailyUnitStatus::Conflicting
            && record.failure_count >= 2
        {
            return Err(solstone_core_journal_io::DailyUnitError::ReviewOwnerConflictExhausted);
        }
        if same
            && !from_scratch
            && !retry
            && (record.status == solstone_core_journal_io::DailyUnitStatus::Capped
                || record.reason_code.as_deref().is_some_and(|reason| {
                    solstone_core_system::daily_coverage::daily_failure_capped(
                        reason,
                        record.failure_count,
                    )
                }))
        {
            let environmental = record
                .reason_code
                .as_deref()
                .is_some_and(solstone_core_system::daily_coverage::environmental_failure);
            if !environmental || record.environmental_retry_day.as_deref() == Some(today) {
                return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                    "daily retry allowance already consumed".to_owned(),
                ));
            }
            record.environmental_retry_day = Some(today.to_owned());
        }
        if identity.name == "daily_schedule"
            && record
                .frozen_packet
                .as_ref()
                .and_then(|packet| packet.pointer("/prepared/config/day"))
                .and_then(Value::as_str)
                .is_some_and(|day| day > today)
        {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "newer maintenance window already owns publication".to_owned(),
            ));
        }
        let keep_attempt_day = same
            && !from_scratch
            && record.status == solstone_core_journal_io::DailyUnitStatus::Unfinished
            && record.frozen_packet.is_some();
        let kept_attempt_day = record.attempt_day.clone();
        if !same || from_scratch {
            let accepted = record.accepted.take();
            record = solstone_core_journal_io::DailyUnitRecord::new(
                identity.clone(),
                evidence_rev,
                contract_dig,
            );
            record.accepted = accepted;
        }
        // An interrupted same-revision attempt retains the exact packet and publication plan.
        if record.frozen_packet.is_none() {
            record.frozen_packet = Some(frozen_packet.clone());
        }
        record.packet_digest = record
            .frozen_packet
            .as_ref()
            .map(solstone_core_talent_runtime::daily_prepare::packet_digest);
        record.lock_token = Some(reserved_use.to_owned());
        record.use_id = Some(reserved_use.to_owned());
        record.status = solstone_core_journal_io::DailyUnitStatus::Unfinished;
        record.attempts = record.attempts.saturating_add(1);
        if keep_attempt_day {
            record.attempt_day = kept_attempt_day;
        } else {
            record.attempt_day = Some(today.to_owned());
        }
        record.updated_at_ms = now_ms;
        *slot = Some(record);
        Ok(())
    })
}

fn log_skip(
    log: &mut RunLogWriter,
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
    log.log("talent.skip", context.event_now_ms(), fields);
}

fn drain_if_full(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    pending: &mut Vec<PendingUse>,
    result: &mut ModeResult,
    max: i64,
) {
    if max != 0 && pending.len() as i64 >= max {
        merge(
            result,
            drain_daily(context, log, runtime, std::mem::take(pending)),
        );
    }
}

/// Daily completion folding is based on the durable health run-log rather than
/// transient Cortex replies.  Record every dispatched unit's one terminal
/// disposition before the lifecycle rereads that fold.
fn drain_daily(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
) -> ModeResult {
    crate::dispatch::drain_with_failure_policy(
        context,
        runtime,
        pending,
        Some(DEFAULT_THINK_TIMEOUT),
        &mut |item, outcome| log_daily_terminal(log, context, item, outcome),
        &|item| item.name != "daily_schedule",
    )
}

fn canonical_daily_failure(reason: &str) -> &str {
    match reason {
        "schema_validation_failed" => "schema_invalid",
        _ => reason,
    }
}

pub(crate) fn log_daily_terminal(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    item: &PendingUse,
    outcome: DrainOutcome,
) {
    if let DrainOutcome::Fail { state, cause } = &outcome {
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            &context.day,
            &item.name,
            item.facet.clone(),
        );
        let reason = canonical_daily_failure(cause.as_deref().unwrap_or(state));
        if let Err(error) = solstone_core_journal_io::with_locked_daily_unit_record(
            &context.journal,
            &identity,
            |slot| {
                if let Some(record) = slot.as_mut() {
                    if record.use_id.as_deref() != Some(&item.use_id)
                        || record.status == solstone_core_journal_io::DailyUnitStatus::Conflicting
                        || record.status.is_success()
                    {
                        return Ok(());
                    }
                    record.failure_count = if record.reason_code.as_deref() == Some(reason) {
                        record.failure_count.saturating_add(1)
                    } else {
                        1
                    };
                    record.reason_code = Some(reason.to_owned());
                    record.status = if solstone_core_system::daily_coverage::daily_failure_capped(
                        reason,
                        record.failure_count,
                    ) {
                        solstone_core_journal_io::DailyUnitStatus::Capped
                    } else {
                        solstone_core_journal_io::DailyUnitStatus::Failed
                    };
                    if reason == "provider_request_rejected"
                        && solstone_core_system::daily_coverage::daily_failure_capped(
                            reason,
                            record.failure_count,
                        )
                    {
                        let day = if let Some(day) = record.attempt_day.as_ref() {
                            day.clone()
                        } else {
                            let instant = if let Some(instant) =
                                chrono::DateTime::from_timestamp_millis(record.updated_at_ms)
                            {
                                instant
                            } else if let Some(instant) =
                                chrono::DateTime::from_timestamp_millis(context.event_now_ms())
                            {
                                instant
                            } else {
                                return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                                    "unrepresentable daily attempt timestamp".to_owned(),
                                ));
                            };
                            solstone_core_system::daily_coverage::local_day(
                                &context.journal,
                                instant,
                            )
                            .map_err(solstone_core_journal_io::DailyUnitError::Malformed)?
                        };
                        record.attempt_day = Some(day.clone());
                        record.environmental_retry_day = Some(day);
                    }
                }
                Ok(())
            },
        ) {
            log_daily_failure(
                log,
                context,
                &item.name,
                item.facet.as_deref(),
                Some(&item.use_id),
                "record_failed",
                &error.to_string(),
            );
        }
    }
    match outcome {
        DrainOutcome::Finish => {
            let mut fields = daily_terminal_fields(context, item, "finish");
            log.log(
                "talent.complete",
                context.event_now_ms(),
                std::mem::take(&mut fields),
            );
        }
        DrainOutcome::Fail { state, cause } => {
            // The cause travelled with the outcome; re-reading the use log here raced the
            // flush and silently degraded the reason to the state word.
            let reason_code =
                cause.unwrap_or_else(|| failure_cause(&context.journal, &item.use_id, state));
            let reason_code = canonical_daily_failure(&reason_code);
            log_daily_failure(
                log,
                context,
                &item.name,
                item.facet.as_deref(),
                Some(&item.use_id),
                state,
                reason_code,
            );
        }
    }
}

fn log_daily_failure(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    facet: Option<&str>,
    use_id: Option<&str>,
    state: &str,
    reason_code: &str,
) {
    let mut fields = daily_terminal_fields_for(context, name, facet, state);
    if let Some(use_id) = use_id {
        fields.insert("use_id".to_owned(), Value::String(use_id.to_owned()));
        // reason_code is the canonical, capped-on string; detail is whatever else the
        // worker's terminal error carried (e.g. schema_validation's per-field jsonschema
        // violations) — see use_log_failure_detail's own doc for why this is separate.
        if let Some(detail) = use_log_failure_detail(&context.journal, use_id) {
            fields.insert("detail".to_owned(), detail);
        }
    }
    fields.insert(
        "reason_code".to_owned(),
        Value::String(reason_code.to_owned()),
    );
    log.log("talent.fail", context.event_now_ms(), fields);
}

fn daily_terminal_fields(
    context: &ThinkContext,
    item: &PendingUse,
    state: &str,
) -> Map<String, Value> {
    let mut fields = daily_terminal_fields_for(context, &item.name, item.facet.as_deref(), state);
    fields.insert("use_id".to_owned(), Value::String(item.use_id.clone()));
    fields
}

fn daily_terminal_fields_for(
    context: &ThinkContext,
    name: &str,
    facet: Option<&str>,
    state: &str,
) -> Map<String, Value> {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("daily".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("name".to_owned(), Value::String(name.to_owned())),
        ("state".to_owned(), Value::String(state.to_owned())),
    ]);
    if let Some(facet) = facet {
        fields.insert("facet".to_owned(), Value::String(facet.to_owned()));
    }
    fields
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
mod tests {
    use super::*;
    use serde_json::json;
    use solstone_core_journal_io::{
        DailyUnitIdentity, DailyUnitRecord, DailyUnitStatus, load_daily_unit_record,
        save_daily_unit_record,
    };

    #[test]
    fn capped_environmental_retry_is_one_durable_reservation_per_day_even_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Capped;
        record.failure_count = 1;
        record.reason_code = Some("model_not_found".to_owned());
        save_daily_unit_record(root, &record).unwrap();
        let successes = std::thread::scope(|scope| {
            let handles = (0..2)
                .map(|i| {
                    let identity = &identity;
                    scope.spawn(move || {
                        reserve_daily_attempt(
                            root,
                            identity,
                            "E",
                            "C",
                            &json!({"packet":"original"}),
                            "20260914",
                            &format!("use-{i}"),
                            false,
                            false,
                            1,
                        )
                        .is_ok()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap() as usize)
                .sum::<usize>()
        });
        assert_eq!(successes, 1);
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({}),
                "20260914",
                "restart",
                false,
                false,
                2
            )
            .is_err()
        );
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({}),
                "20260915",
                "tomorrow",
                false,
                false,
                3
            )
            .is_ok()
        );
        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.environmental_retry_day.as_deref(), Some("20260915"));
        assert_eq!(loaded.frozen_packet, Some(json!({"packet":"original"})));
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "new-contract",
                &json!({"packet":"new"}),
                "20260915",
                "new",
                false,
                false,
                4
            )
            .is_ok()
        );
        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.failure_count, 0);
        assert_eq!(loaded.frozen_packet, Some(json!({"packet":"new"})));
    }

    // AC: 2026-09-21, a real suze `morning_briefing` failure. Before this fix, the
    // chronicle `talent.fail` row for a schema_invalid failure carried only
    // `reason_code`, and the actual jsonschema violations (which field the local model
    // got wrong) were readable only by opening the raw per-use log by hand. This proves
    // the fix end to end: `log_daily_failure` must now attach that detail to the
    // durable chronicle event it writes.
    #[test]
    fn log_daily_failure_writes_schema_validation_detail_into_the_chronicle_event() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260625";
        let use_dir = root.join("talents/morning_briefing");
        std::fs::create_dir_all(&use_dir).unwrap();
        std::fs::write(
            use_dir.join("use-mb.jsonl"),
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-mb\"}\n",
                "{\"event\":\"error\",\"terminal\":true,\"name\":\"morning_briefing\",",
                "\"error\":\"talent output failed schema validation\",",
                "\"schema_validation\":{\"valid\":false,\"errors\":[{\"path\":\"/your_day/5/time\",",
                "\"constraint\":\"pattern\",\"message\":\"bad time\"}]},",
                "\"reason_code\":\"schema_validation_failed\"}\n",
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
        let mut log = RunLogWriter::open(root, day, "daily");
        log_daily_failure(
            &mut log,
            &context,
            "morning_briefing",
            None,
            Some("use-mb"),
            "error",
            "schema_invalid",
        );
        log.finish().unwrap();

        let health_dir = root.join("chronicle").join(day).join("health");
        let mut found = None;
        for entry in std::fs::read_dir(&health_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                let row: Value = serde_json::from_str(line).unwrap();
                if row.get("event") == Some(&Value::String("talent.fail".to_owned()))
                    && row.get("name") == Some(&Value::String("morning_briefing".to_owned()))
                {
                    found = Some(row);
                }
            }
        }
        let row = found.expect("talent.fail row for morning_briefing");
        assert_eq!(row["reason_code"], "schema_invalid");
        assert_eq!(
            row["detail"]["schema_validation"]["errors"][0]["path"],
            "/your_day/5/time"
        );
    }

    #[test]
    fn provider_request_rejected_caps_and_forbids_second_same_day_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"original"}),
            day,
            "use-rej-1",
            false,
            false,
            1,
        )
        .unwrap();

        let before_fold = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(before_fold.status, DailyUnitStatus::Unfinished);
        assert_eq!(before_fold.attempt_day.as_deref(), Some("20260914"));
        assert_eq!(before_fold.environmental_retry_day, None);
        assert_eq!(before_fold.use_id.as_deref(), Some("use-rej-1"));
        assert_eq!(before_fold.updated_at_ms, 1);
        assert_eq!(before_fold.failure_count, 0);

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-rej-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.status, DailyUnitStatus::Capped);
        assert_eq!(
            loaded.reason_code,
            Some("provider_request_rejected".to_owned())
        );
        assert_eq!(loaded.attempt_day, Some("20260914".to_owned()));
        assert_eq!(loaded.environmental_retry_day, Some("20260914".to_owned()));
        assert_eq!(loaded.failure_count, 1);

        let second = reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"retry"}),
            day,
            "use-rej-2",
            false,
            false,
            2,
        );
        assert!(
            second.is_err(),
            "second reservation was allowed: {second:?}"
        );
    }

    #[test]
    fn provider_request_rejected_multi_day_fold_preserves_reservation_day_and_allows_next_day() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();

        let day1 = "20260914";
        let day2 = "20260915";
        let identity = DailyUnitIdentity::new(day1, "schedule", None);
        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"original"}),
            day1,
            "use-multiday-1",
            false,
            false,
            1,
        )
        .unwrap();

        // Terminal fold occurs with ThinkContext event_now_ms 1789453800000 (2026-09-15T06:30:00Z)
        let context = ThinkContext::new(
            root,
            day1.to_owned(),
            root.join("chronicle").join(day1),
            1_789_453_800_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day1, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-multiday-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.status, DailyUnitStatus::Capped);
        assert_eq!(loaded.attempt_day.as_deref(), Some(day1));
        assert_eq!(loaded.environmental_retry_day.as_deref(), Some(day1));

        // One reserve on "20260915" succeeds
        let second = reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"retry"}),
            day2,
            "use-multiday-2",
            false,
            false,
            2,
        );
        assert!(second.is_ok(), "reservation on next day failed: {second:?}");

        // A second reserve on "20260915" fails
        let third = reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"retry2"}),
            day2,
            "use-multiday-3",
            false,
            false,
            3,
        );
        assert!(
            third.is_err(),
            "second reservation on day 2 succeeded: {third:?}"
        );
    }

    #[test]
    fn legacy_record_without_attempt_day_recovers_day_from_updated_at_ms() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Los_Angeles"}}"#,
        )
        .unwrap();

        // 1. Unfinished record, attempt_day None, updated_at_ms = 1789453800000
        let day = "20260914";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Unfinished;
        record.attempts = 1;
        record.failure_count = 1;
        record.attempt_day = None;
        record.updated_at_ms = 1_789_453_800_000;
        record.use_id = Some("use-legacy-1".to_owned());
        record.lock_token = Some("use-legacy-1".to_owned());
        save_daily_unit_record(root, &record).unwrap();

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-legacy-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.status, DailyUnitStatus::Capped);
        assert_eq!(loaded.attempt_day.as_deref(), Some("20260914"));
        assert_eq!(loaded.environmental_retry_day.as_deref(), Some("20260914"));

        // 2. A second record with updated_at_ms = i64::MAX and event_now_ms = 1789453800000 consumes 20260914, not 20260915
        let identity2 = DailyUnitIdentity::new(day, "summarize", None);
        let mut record2 = DailyUnitRecord::new(identity2.clone(), "E", "C");
        record2.status = DailyUnitStatus::Unfinished;
        record2.attempts = 1;
        record2.failure_count = 1;
        record2.attempt_day = None;
        record2.updated_at_ms = i64::MAX;
        record2.use_id = Some("use-legacy-2".to_owned());
        record2.lock_token = Some("use-legacy-2".to_owned());
        save_daily_unit_record(root, &record2).unwrap();

        let context2 = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_789_453_800_000,
        )
        .unwrap();
        let mut log2 = RunLogWriter::open(root, day, "daily");
        let pending2 = PendingUse {
            name: "summarize".to_owned(),
            facet: None,
            use_id: "use-legacy-2".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log2,
            &context2,
            &pending2,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log2.finish().unwrap();

        let loaded2 = load_daily_unit_record(root, &identity2).unwrap().unwrap();
        assert_eq!(loaded2.status, DailyUnitStatus::Capped);
        assert_eq!(loaded2.attempt_day.as_deref(), Some("20260914"));
        assert_eq!(loaded2.environmental_retry_day.as_deref(), Some("20260914"));
    }

    #[test]
    fn fresh_reserve_with_timezone_stores_attempt_day() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Los_Angeles"}}"#,
        )
        .unwrap();

        let instant = chrono::DateTime::from_timestamp_millis(1_789_453_800_000).unwrap();
        let today = solstone_core_system::daily_coverage::local_day(root, instant).unwrap();
        assert_eq!(today, "20260914");

        let identity = DailyUnitIdentity::new(&today, "schedule", None);
        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"original"}),
            &today,
            "use-tz-1",
            false,
            false,
            1,
        )
        .unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.attempt_day.as_deref(), Some("20260914"));

        let context = ThinkContext::new(
            root,
            today.clone(),
            root.join("chronicle").join(&today),
            1_789_453_800_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, &today, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-tz-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let folded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(folded.status, DailyUnitStatus::Capped);
        assert_eq!(folded.attempt_day.as_deref(), Some("20260914"));
        assert_eq!(folded.environmental_retry_day.as_deref(), Some("20260914"));
    }

    #[test]
    fn provider_request_rejected_concurrent_reserves_on_same_day_fail() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Capped;
        record.failure_count = 1;
        record.reason_code = Some("provider_request_rejected".to_owned());
        record.attempt_day = Some(day.to_owned());
        record.environmental_retry_day = Some(day.to_owned());
        save_daily_unit_record(root, &record).unwrap();

        let successes = std::thread::scope(|scope| {
            let handles = (0..2)
                .map(|i| {
                    let identity = &identity;
                    scope.spawn(move || {
                        reserve_daily_attempt(
                            root,
                            identity,
                            "E",
                            "C",
                            &json!({"packet":"retry"}),
                            day,
                            &format!("use-concurrent-{i}"),
                            false,
                            false,
                            1,
                        )
                        .is_ok()
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap() as usize)
                .sum::<usize>()
        });
        assert_eq!(successes, 0);

        let _ = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({"packet":"retry"}),
                day,
                "use-after-reload",
                false,
                false,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn provider_request_rejected_concurrent_reserves_on_next_day_yield_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let next_day = "20260915";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Capped;
        record.failure_count = 1;
        record.reason_code = Some("provider_request_rejected".to_owned());
        record.attempt_day = Some(day.to_owned());
        record.environmental_retry_day = Some(day.to_owned());
        save_daily_unit_record(root, &record).unwrap();

        let results = std::thread::scope(|scope| {
            let handles = (0..2)
                .map(|i| {
                    let identity = &identity;
                    let use_id = format!("use-next-{i}");
                    scope.spawn(move || {
                        (
                            use_id.clone(),
                            reserve_daily_attempt(
                                root,
                                identity,
                                "E",
                                "C",
                                &json!({"packet":"retry"}),
                                next_day,
                                &use_id,
                                false,
                                false,
                                2,
                            )
                            .is_ok(),
                        )
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let success_count = results.iter().filter(|(_, ok)| *ok).count();
        assert_eq!(success_count, 1);
        let (winner_use_id, _) = results.iter().find(|(_, ok)| *ok).unwrap().clone();
        let (loser_use_id, _) = results.iter().find(|(_, ok)| !*ok).unwrap().clone();

        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({"packet":"retry"}),
                next_day,
                "use-later",
                false,
                false,
                2,
            )
            .is_err()
        );

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.use_id.as_deref(), Some(winner_use_id.as_str()));

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let loser_pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: loser_use_id,
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &loser_pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );

        let winner_pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: winner_use_id.clone(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &winner_pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let final_loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(final_loaded.use_id.as_deref(), Some(winner_use_id.as_str()));
        assert_eq!(final_loaded.attempt_day.as_deref(), Some(next_day));
        assert_eq!(
            final_loaded.environmental_retry_day.as_deref(),
            Some(next_day)
        );
    }

    #[test]
    fn failed_record_retry_on_next_day_stamps_attempt_day_and_caps_on_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let next_day = "20260915";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Failed;
        record.reason_code = None;
        record.failure_count = 0;
        record.attempt_day = Some(day.to_owned());
        record.environmental_retry_day = None;
        save_daily_unit_record(root, &record).unwrap();

        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"original"}),
            next_day,
            "use-failed-retry",
            false,
            false,
            1,
        )
        .unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.attempt_day.as_deref(), Some(next_day));
        assert_eq!(loaded.environmental_retry_day, None);

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-failed-retry".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let folded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(folded.status, DailyUnitStatus::Capped);
        assert_eq!(folded.attempt_day.as_deref(), Some(next_day));
        assert_eq!(folded.environmental_retry_day.as_deref(), Some(next_day));

        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({"packet":"original"}),
                next_day,
                "use-second-retry",
                false,
                false,
                2,
            )
            .is_err()
        );
    }

    #[test]
    fn capped_record_new_contract_digest_resets_failure_count_and_stamps_attempt_day() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let next_day = "20260915";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C1");
        record.status = DailyUnitStatus::Capped;
        record.failure_count = 1;
        record.reason_code = Some("provider_request_rejected".to_owned());
        record.attempt_day = Some(day.to_owned());
        record.environmental_retry_day = Some(day.to_owned());
        save_daily_unit_record(root, &record).unwrap();

        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C2",
            &json!({"packet":"new_contract"}),
            next_day,
            "use-c2-1",
            false,
            false,
            1,
        )
        .unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.failure_count, 0);
        assert_eq!(loaded.environmental_retry_day, None);
        assert_eq!(loaded.attempt_day.as_deref(), Some(next_day));

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-c2-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "provider_request_rejected"),
        );
        log.finish().unwrap();

        let folded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(folded.status, DailyUnitStatus::Capped);
        assert_eq!(folded.attempt_day.as_deref(), Some(next_day));
        assert_eq!(folded.environmental_retry_day.as_deref(), Some(next_day));

        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C2",
                &json!({"packet":"new_contract"}),
                next_day,
                "use-c2-2",
                false,
                false,
                2,
            )
            .is_err()
        );
    }

    #[test]
    fn model_not_found_sets_environmental_retry_day_on_next_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260914";
        let identity = DailyUnitIdentity::new(day, "schedule", None);
        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"packet":"original"}),
            day,
            "use-mnf-1",
            false,
            false,
            1,
        )
        .unwrap();

        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let pending = PendingUse {
            name: "schedule".to_owned(),
            facet: None,
            use_id: "use-mnf-1".to_owned(),
            output_path: None,
            index_output: false,
        };
        log_daily_terminal(
            &mut log,
            &context,
            &pending,
            DrainOutcome::fail("error", "model_not_found"),
        );
        log.finish().unwrap();

        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.status, DailyUnitStatus::Capped);
        assert_eq!(loaded.failure_count, 1);
        assert_eq!(loaded.environmental_retry_day, None);

        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({"packet":"retry"}),
                day,
                "use-mnf-2",
                false,
                false,
                2,
            )
            .is_ok()
        );
        let loaded2 = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded2.environmental_retry_day.as_deref(), Some(day));

        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({"packet":"retry"}),
                day,
                "use-mnf-3",
                false,
                false,
                3,
            )
            .is_err()
        );
    }

    // The loopback HTTP 400 case lives in
    // `tests/daily_provider_rejection_loopback.rs`. Binding a TCP listener
    // trips the routine unit harness's hard-boundary ("network") topology
    // check, so that case runs in the `test-hooks` integration harness.

    #[test]
    fn ordinary_resume_keeps_retained_result_actions_and_owner_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.frozen_packet = Some(json!({"memory":"admitted"}));
        record.generated_result = Some(json!({"response":"[original]"}));
        record.action_plan = Some(json!({"plan":"prepared"}));
        record.receipts = vec![json!({"action":"committed"})];
        record.attempt_day = Some("20260901".to_owned());
        save_daily_unit_record(root, &record).unwrap();
        reserve_daily_attempt(
            root,
            &identity,
            "E",
            "C",
            &json!({"memory":"mutated"}),
            "20260914",
            "replacement",
            false,
            false,
            2,
        )
        .unwrap();
        let loaded = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(loaded.frozen_packet, record.frozen_packet);
        assert_eq!(loaded.generated_result, record.generated_result);
        assert_eq!(loaded.action_plan, record.action_plan);
        assert_eq!(loaded.receipts, record.receipts);
        assert_eq!(loaded.lock_token.as_deref(), Some("replacement"));
        assert_eq!(loaded.attempt_day.as_deref(), Some("20260901"));
    }

    #[test]
    fn unrelated_caps_do_not_get_daily_environmental_retry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = DailyUnitStatus::Capped;
        record.reason_code = Some("schema_invalid".to_owned());
        record.failure_count = 3;
        save_daily_unit_record(root, &record).unwrap();
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E",
                "C",
                &json!({}),
                "20260915",
                "retry",
                false,
                false,
                2
            )
            .is_err()
        );
        assert!(
            reserve_daily_attempt(
                root,
                &identity,
                "E2",
                "C",
                &json!({}),
                "20260915",
                "new-evidence",
                false,
                false,
                3
            )
            .is_ok()
        );
    }
    #[cfg(all(test, feature = "full-tests"))]
    use solstone_core_cortex_client::{
        CortexRequest, UseCompletion, UseEndState, WaitForUsesReport,
    };
    #[cfg(unix)]
    #[cfg(all(test, feature = "full-tests"))]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(all(test, feature = "full-tests"))]
    use std::path::PathBuf;
    #[cfg(all(test, feature = "full-tests"))]
    use std::sync::Arc;
    #[cfg(all(test, feature = "full-tests"))]
    use std::sync::Mutex;
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    fn model_response(text: &str, validation: Value) -> Value {
        json!({"schema":"solstone-generate-response-v2","id":null,"outcome":"generated","text":text,"model":"test-model","usage":{},"finish_reason":"stop","thinking":null,"schema_validation":validation,"input_budget":null,"request_budget":null,"inference":null})
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    struct Worker {
        context: solstone_core_talent_runtime::ExecutionContext,
        paths: solstone_core_talent_runtime::prepare::RuntimePaths,
        stub: PathBuf,
        outcomes: Mutex<std::collections::BTreeMap<String, UseEndState>>,
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    impl crate::context::CortexBoundary for Worker {
        fn dispatch(
            &self,
            _: &tokio::runtime::Runtime,
            _: &CortexRequest,
        ) -> Result<String, DispatchFailure> {
            panic!("daily must reserve before send")
        }
        fn dispatch_prepared(
            &self,
            _: &tokio::runtime::Runtime,
            request: &CortexRequest,
            reserved: Option<&str>,
            prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
        ) -> Result<String, DispatchFailure> {
            let id = reserved.expect("durably allocated use id");
            prepare(id).map_err(|_| DispatchFailure::Unavailable)?;
            let mut config = request.config.clone();
            config.insert("name".to_owned(), json!(request.name));
            let outcome = solstone_core_talent_runtime::execute_request(
                config,
                &self.paths,
                &self.context,
                &solstone_core_generate::OneShotClient::at_path(&self.stub),
                &solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
                    self.stub.with_extension("absent"),
                ),
                &mut Vec::new(),
            );
            let end_state = match &outcome {
                solstone_core_talent_runtime::RuntimeOutcome::Finished { .. } => {
                    UseEndState::Finish
                }
                solstone_core_talent_runtime::RuntimeOutcome::StageFailed(error) => {
                    let log = self
                        .context
                        .journal
                        .join("talents")
                        .join(&request.name)
                        .join(format!("{id}.jsonl"));
                    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
                    std::fs::write(log, format!("{}\n", json!({"event":"error","terminal":true,"use_id":id,"reason_code":error.reason_code(),"error":error.to_string()}))).unwrap();
                    UseEndState::Error
                }
                solstone_core_talent_runtime::RuntimeOutcome::SchemaValidationFailed { .. } => {
                    let log = self
                        .context
                        .journal
                        .join("talents")
                        .join(&request.name)
                        .join(format!("{id}.jsonl"));
                    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
                    std::fs::write(log, format!("{}\n", json!({"event":"error","terminal":true,"use_id":id,"reason_code":"schema_validation_failed"}))).unwrap();
                    UseEndState::Error
                }
                other => panic!("unexpected worker outcome: {other:?}"),
            };
            self.outcomes
                .lock()
                .unwrap()
                .insert(id.to_owned(), end_state);
            Ok(id.to_owned())
        }
        fn wait(
            &self,
            _: &tokio::runtime::Runtime,
            ids: &[String],
            _: Option<std::time::Duration>,
        ) -> Result<WaitForUsesReport, String> {
            Ok(WaitForUsesReport {
                completed: ids
                    .iter()
                    .map(|id| {
                        (
                            id.clone(),
                            UseCompletion {
                                end_state: self.outcomes.lock().unwrap()[id],
                                finish_fields: Default::default(),
                            },
                        )
                    })
                    .collect(),
                timed_out: Vec::new(),
            })
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn ordinary_daily_rerun_consumes_new_evidence_and_commits_changed_owner_result() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"),r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("facets/work")).unwrap();
        std::fs::write(root.join("facets/work/facet.json"), "{}").unwrap();
        std::fs::write(talent.join("schedule.md"),"{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":10,\"output\":\"json\",\"hook\":{\"post\":\"schedule\"},\"load\":{\"transcripts\":true}\n}\nExtract upcoming scheduled events.").unwrap();
        let source = root.join("chronicle/20260910/default/090000_60/note_transcript.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow is red.",
        )
        .unwrap();
        let stub = root.join("generate-stub.sh");
        for title in ["red", "blue"] {
            let event = json!({"activity":"meeting","target_date":"2026-09-11","title":title,"description":format!("The meeting topic is {title}"),"facet":"work","start":"09:00:00","end":"10:00:00","participation":[]});
            std::fs::write(
                root.join(format!("response-{title}.json")),
                model_response(&json!([event]).to_string(), Value::Null).to_string(),
            )
            .unwrap();
        }
        std::fs::write(
            &stub,
            r#"#!/bin/sh
request=$(cat)
case "$request" in *blue*) response=blue;; *) response=red;; esac
cat "${0%/*}/response-$response.json"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let mut log = RunLogWriter::open(root, day, "daily");
        let enabled = std::fs::read_to_string(talent.join("schedule.md")).unwrap();
        std::fs::write(
            talent.join("schedule.md"),
            enabled.replace("\"priority\":10", "\"priority\":10,\"disabled\":true"),
        )
        .unwrap();
        run(&context, &mut log, None, false, 1).unwrap();
        assert!(
            serde_json::from_str::<Value>(
                &std::fs::read_to_string(root.join("facets/work/facet.json")).unwrap()
            )
            .unwrap()
            .get("id")
            .is_none()
        );
        std::fs::write(talent.join("schedule.md"), enabled).unwrap();
        let first = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(first.failed, 0);
        let admitted_facet = std::fs::read(root.join("facets/work/facet.json")).unwrap();
        assert!(serde_json::from_slice::<Value>(&admitted_facet).unwrap()["id"].is_string());
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            root, day, &talent, &apps,
        )
        .unwrap();
        assert_eq!(
            coverage.state,
            solstone_core_system::daily_coverage::CoverageState::Current
        );
        let first_e = coverage.units[0].evidence_revision.clone();
        let output = root.join("facets/work/activities/20260911.jsonl");
        assert!(std::fs::read_to_string(&output).unwrap().contains("red"));
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), 1);
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow changed to blue.",
        )
        .unwrap();
        assert!(
            !solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
                root, day, &talent, &apps
            )
            .unwrap()
            .state
            .is_current()
        );
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), 2);
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            root, day, &talent, &apps,
        )
        .unwrap();
        assert_eq!(
            coverage.state,
            solstone_core_system::daily_coverage::CoverageState::Current
        );
        assert_ne!(first_e, coverage.units[0].evidence_revision);
        let rows = std::fs::read_to_string(output).unwrap();
        let rows = rows
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["title"], "blue");
        // A conflicting replacement cannot borrow the previous accepted result's success.
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(day, "schedule", None);
        let mut record = solstone_core_journal_io::load_daily_unit_record(root, &identity)
            .unwrap()
            .unwrap();
        record.status = solstone_core_journal_io::DailyUnitStatus::Conflicting;
        solstone_core_journal_io::save_daily_unit_record(root, &record).unwrap();
        assert_eq!(
            solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
                root, day, &talent, &apps
            )
            .unwrap()
            .state,
            solstone_core_system::daily_coverage::CoverageState::Outstanding
        );
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), 3);
        assert_eq!(
            std::fs::read(root.join("facets/work/facet.json")).unwrap(),
            admitted_facet
        );
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn repeated_invalid_worker_outputs_cap_and_new_failure_causes_reset_the_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"),r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("facets/work")).unwrap();
        std::fs::write(root.join("facets/work/facet.json"), "{}").unwrap();
        std::fs::write(talent.join("schedule.md"),"{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":10,\"output\":\"json\",\"schema\":\"schedule-schema.json\",\"hook\":{\"post\":\"schedule\"},\"load\":{\"transcripts\":true}\n}\nExtract upcoming scheduled events.").unwrap();
        std::fs::write(talent.join("schedule-schema.json"), r#"{"type":"array"}"#).unwrap();
        let source = root.join("chronicle/20260910/default/090000_60/note_transcript.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow is red.",
        )
        .unwrap();
        let stub = root.join("generate-stub.sh");
        for (mode, text, validation) in [
            ("parse", "not json", Value::Null),
            (
                "schema",
                r#"[{"activity":"meeting"}]"#,
                json!({"valid":false}),
            ),
        ] {
            std::fs::write(
                root.join(format!("response-{mode}.json")),
                model_response(text, validation).to_string(),
            )
            .unwrap();
        }
        std::fs::write(
            &stub,
            r#"#!/bin/sh
cat >/dev/null
mode=$(cat "${0%/*}/response-mode")
if [ "$mode" = transport ]; then
    printf '%s\n' 'broken wire response'
else
    cat "${0%/*}/response-$mode.json"
fi
"#,
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let mut log = RunLogWriter::open(root, day, "daily");
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(day, "schedule", None);
        let modes = [
            ("parse", "schema_invalid", 1),
            ("schema", "schema_invalid", 2),
            ("transport", "talent_stage_failed", 1),
            ("parse", "schema_invalid", 1),
            ("schema", "schema_invalid", 2),
            ("parse", "schema_invalid", 3),
        ];
        for (mode, reason, count) in modes {
            std::fs::write(root.join("response-mode"), mode).unwrap();
            let result = run(&context, &mut log, None, false, 1).unwrap();
            assert_eq!(result.failed, 1, "{mode}: {result:?}");
            let record = solstone_core_journal_io::load_daily_unit_record(root, &identity)
                .unwrap()
                .unwrap();
            assert_eq!(record.reason_code.as_deref(), Some(reason));
            assert_eq!(record.failure_count, count);
            assert!(record.generated_result.is_none());
        }
        let record = solstone_core_journal_io::load_daily_unit_record(root, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            record.status,
            solstone_core_journal_io::DailyUnitStatus::Capped
        );
        let dispatched = worker.outcomes.lock().unwrap().len();
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), dispatched);
        std::fs::write(&source, "# Imported note\nA changed event.").unwrap();
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), dispatched + 1);
        let record = solstone_core_journal_io::load_daily_unit_record(root, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(record.failure_count, 1);
        assert_eq!(
            record.status,
            solstone_core_journal_io::DailyUnitStatus::Failed
        );
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn invalid_observer_references_regenerate_until_capped_and_new_evidence_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(apps.join("entities/talent")).unwrap();
        solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::attach_or_reactivate_entity(
            root, "work", "Person", "Ada", "Engineer",
        )
        .unwrap();
        solstone_core_facets::save_detected_entity(
            root,
            "work",
            day,
            "Person",
            "Ada",
            "Discussed preferences",
        )
        .unwrap();
        solstone_core_facets::add_observation(
            root,
            "work",
            "ada",
            "Prefers concise updates",
            None,
            None,
        )
        .unwrap();
        let owner_path = root.join("facets/work/entities/ada/observations.jsonl");
        let owner_before = std::fs::read(&owner_path).unwrap();
        let sugg_dir = root.join("facets/work/entities");
        std::fs::create_dir_all(&sugg_dir).unwrap();
        std::fs::write(
            sugg_dir.join("20260910_observer_suggestions.json"),
            json!({
                "facet": "work",
                "day": "20260910",
                "entities": [
                    {
                        "entity_id": "ada",
                        "suggestions": [
                            {"content": "Prefers concise weekly updates"}
                        ]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(apps.join("entities/talent/entity_observer.md"), r#"{
"type":"generate","schedule":"daily","priority":58,"multi_facet":true,"output":"json","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"load":{"transcripts":false,"percepts":false,"talents":false}
}
$observer_context"#).unwrap();
        let source = root.join("chronicle/20260910/mic/090000_60/note_transcript.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            "# Imported note\nAda prefers concise weekly updates.",
        )
        .unwrap();
        let classification = source.parent().unwrap().join("talents");
        std::fs::create_dir_all(&classification).unwrap();
        std::fs::write(classification.join("facets.json"), r#"[{"facet":"work"}]"#).unwrap();
        let good_output = json!({"entities":[{"entity_id":"ada","decisions":[{"op":"replace","target_id":1,"target_quote":"Prefers concise updates","content":"Prefers concise weekly updates"}]}]}).to_string();
        std::fs::write(
            root.join("response-good.json"),
            model_response(&good_output, Value::Null).to_string(),
        )
        .unwrap();
        let bad_output = json!({"invalid": true}).to_string();
        std::fs::write(
            root.join("response-bad.json"),
            model_response(&bad_output, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(root.join("response-mode"), "bad").unwrap();
        let calls = root.join("model-calls");
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            &stub,
            r#"#!/bin/sh
cat >/dev/null
printf x >> "${0%/*}/model-calls"
mode=$(cat "${0%/*}/response-mode")
cat "${0%/*}/response-$mode.json"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent, apps)
        .with_boundary(worker.clone());
        let mut log = RunLogWriter::open(root, day, "daily");
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            day,
            "entities:entity_observer",
            Some("work".into()),
        );
        for count in 1..=3 {
            let result = run(&context, &mut log, None, false, 1).unwrap();
            assert_eq!(result.failed, 1, "{result:?}");
            let record = solstone_core_journal_io::load_daily_unit_record(root, &identity)
                .unwrap()
                .unwrap();
            assert_eq!(record.reason_code.as_deref(), Some("schema_invalid"));
            assert_eq!(record.failure_count, count);
            assert!(record.generated_result.is_none());
            assert!(record.action_plan.is_none());
            assert_eq!(std::fs::read(&calls).unwrap().len(), count as usize);
            assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before);
        }
        let capped = solstone_core_journal_io::load_daily_unit_record(root, &identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            capped.status,
            solstone_core_journal_io::DailyUnitStatus::Capped
        );
        std::fs::write(root.join("response-mode"), "good").unwrap();
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(
            std::fs::read(&calls).unwrap().len(),
            3,
            "capped evidence must not dispatch"
        );
        assert_eq!(std::fs::read(&owner_path).unwrap(), owner_before);
        std::fs::write(
            &source,
            "# Imported note\nAda confirmed concise weekly updates today.",
        )
        .unwrap();
        let result = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(result.failed, 0, "{result:?}");
        assert_eq!(std::fs::read(&calls).unwrap().len(), 4);
        let accepted = solstone_core_journal_io::load_daily_unit_record(root, &identity)
            .unwrap()
            .unwrap();
        assert_ne!(accepted.evidence_revision, capped.evidence_revision);
        assert_eq!(accepted.contract_digest, capped.contract_digest);
        assert_eq!(
            accepted.status,
            solstone_core_journal_io::DailyUnitStatus::Committed
        );
        let published = std::fs::read_to_string(&owner_path).unwrap();
        assert!(published.contains("Prefers concise weekly updates"));
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(std::fs::read(&calls).unwrap().len(), 4);
        assert_eq!(std::fs::read_to_string(&owner_path).unwrap(), published);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn all_applicable_legacy_facet_ids_are_assigned_before_the_first_unit_contract() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        for facet in ["home", "work", "inactive"] {
            std::fs::create_dir_all(root.join("facets").join(facet)).unwrap();
            std::fs::write(root.join("facets").join(facet).join("facet.json"), "{}").unwrap();
        }
        let segment = root.join("chronicle/20260910/mic/090000_60/talents");
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(
            segment.join("facets.json"),
            r#"[{"facet":"home"},{"facet":"work"}]"#,
        )
        .unwrap();
        for facet in ["home", "work"] {
            std::fs::create_dir_all(segment.join(facet)).unwrap();
            std::fs::write(
                segment.join(facet).join("flow.md"),
                format!("# {facet}\n\nA meeting was rescheduled at this facet."),
            )
            .unwrap();
        }
        std::fs::write(talent.join("schedule.md"), "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":1,\"output\":\"json\",\"disabled\":true,\"hook\":{\"post\":\"schedule\"}\n}\nDisabled scheduling.").unwrap();
        std::fs::write(talent.join("facet_newsletter.md"), "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":2,\"multi_facet\":true,\"output\":\"md\",\"hook\":{\"pre\":\"facet_newsletter\",\"post\":\"facet_newsletter\"},\"load\":{\"transcripts\":false}\n}\nWrite a newsletter from $source_packet.").unwrap();
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            root.join("newsletter-response.json"),
            model_response(
                "# Daily newsletter\nThe meeting was rescheduled.",
                Value::Null,
            )
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &stub,
            r#"#!/bin/sh
cat >/dev/null
cat "${0%/*}/newsletter-response.json"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let mut log = RunLogWriter::open(root, day, "daily");
        let result = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!((result.success, result.failed), (2, 0), "{result:?}");
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            root, day, &talent, &apps,
        )
        .unwrap();
        assert_eq!(coverage.units.len(), 2);
        assert_eq!(
            coverage.state,
            solstone_core_system::daily_coverage::CoverageState::Current
        );
        for unit in coverage.units {
            let record = solstone_core_journal_io::load_daily_unit_record(root, &unit.identity)
                .unwrap()
                .unwrap();
            assert!(record.is_reusable_for(&unit.evidence_revision, &unit.contract_digest));
        }
        assert!(
            serde_json::from_slice::<Value>(
                &std::fs::read(root.join("facets/inactive/facet.json")).unwrap()
            )
            .unwrap()
            .get("id")
            .is_none()
        );
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), 2);
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn actual_upstream_no_output_does_not_lend_old_artifacts_to_briefing_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None).unwrap();
        for (name, metadata, body) in [
            (
                "schedule",
                json!({"priority":1,"output":"json","hook":{"post":"schedule"},"load":{"transcripts":true}}),
                "SCHEDULE_PROOF: extract future appointments.",
            ),
            (
                "facet_newsletter",
                json!({"priority":2,"output":"md","multi_facet":true,"always":true,"hook":{"pre":"facet_newsletter","post":"facet_newsletter"},"load":{"transcripts":false}}),
                "NEWSLETTER_PROOF: $source_packet",
            ),
            (
                "morning_briefing",
                json!({"priority":3,"output":"json","hook":{"pre":"morning_briefing"},"load":{"transcripts":false}}),
                "BRIEFING_PROOF: $facet_newsletters $anticipated_today",
            ),
        ] {
            let mut metadata = metadata.as_object().unwrap().clone();
            metadata.insert("type".into(), json!("generate"));
            metadata.insert("schedule".into(), json!("daily"));
            std::fs::write(
                talent.join(format!("{name}.md")),
                format!(
                    "{}\n{body}",
                    serde_json::to_string_pretty(&metadata).unwrap()
                ),
            )
            .unwrap();
        }
        let source = root.join("chronicle/20260910/mic/090000_60/talents/work/flow.md");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let transcript = root.join("chronicle/20260910/mic/090000_60/note_transcript.md");
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            &stub,
            r#"#!/bin/sh
request=$(cat)
case "$request" in
 *SCHEDULE_PROOF*) kind=schedule;;
 *NEWSLETTER_PROOF*) kind=newsletter;;
 *BRIEFING_PROOF*) kind=briefing;;
 *) exit 91;;
esac
cat "${0%/*}/response-$kind.json"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let mut log = RunLogWriter::open(root, day, "daily");
        let identity = |name, facet| DailyUnitIdentity::new(day, name, facet);
        let record = |name, facet| {
            load_daily_unit_record(root, &identity(name, facet))
                .unwrap()
                .unwrap()
        };
        let news_path = root.join("facets/work/news/20260910.md");
        let calendar_path = root.join("facets/work/activities/20260911.jsonl");
        for phase in ["OLD", "EMPTY", "NEW"] {
            if phase == "EMPTY" {
                std::fs::remove_file(&source).unwrap();
                std::fs::remove_file(&transcript).unwrap();
            } else {
                std::fs::write(
                    &source,
                    format!("# Work\n\n{phase} source: planning appointment tomorrow."),
                )
                .unwrap();
                std::fs::write(
                    &transcript,
                    format!("# Source\n{phase} planning appointment tomorrow."),
                )
                .unwrap();
            }
            let events = if phase == "EMPTY" {
                json!([])
            } else {
                json!([{"activity":"meeting","target_date":"2026-09-11","title":format!("{phase} appointment"),"description":"Planning","facet":"work","start":"09:00:00","end":"10:00:00","participation":[]}])
            };
            for (kind, response) in [
                ("schedule", events.to_string()),
                (
                    "newsletter",
                    format!("# {phase} newsletter\nA planning update."),
                ),
                (
                    "briefing",
                    json!({"summary":"Prepared from this pass"}).to_string(),
                ),
            ] {
                std::fs::write(
                    root.join(format!("response-{kind}.json")),
                    model_response(&response, Value::Null).to_string(),
                )
                .unwrap();
            }
            let outcome = run(&context, &mut log, None, false, 1).unwrap();
            assert_eq!(outcome.failed, 0, "phase {phase}: {outcome:?}");
            let expected = if phase == "EMPTY" {
                DailyUnitStatus::CommittedNoOutput
            } else {
                DailyUnitStatus::Committed
            };
            assert_eq!(
                record("schedule", None).accepted.unwrap().status,
                expected,
                "schedule {phase}"
            );
            assert_eq!(
                record("facet_newsletter", Some("work".to_owned()))
                    .accepted
                    .unwrap()
                    .status,
                expected,
                "newsletter {phase}"
            );
            let briefing = record("morning_briefing", None);
            let packet = briefing.frozen_packet.unwrap();
            let news = packet
                .pointer("/state/MorningBriefing/values/facet_newsletters")
                .unwrap()
                .as_str()
                .unwrap();
            let agenda = packet
                .pointer("/state/MorningBriefing/values/anticipated_today")
                .unwrap()
                .as_str()
                .unwrap();
            if phase == "EMPTY" {
                assert!(
                    std::fs::read_to_string(&news_path)
                        .unwrap()
                        .contains("OLD newsletter"),
                    "positive control: prior newsletter still exists"
                );
                assert!(
                    std::fs::read_to_string(&calendar_path)
                        .unwrap()
                        .contains("OLD appointment"),
                    "no output does not imply cancellation"
                );
                assert!(!news.contains("OLD newsletter"), "{news}");
                assert!(!agenda.contains("OLD appointment"), "{agenda}");
                assert_eq!(
                    packet.pointer("/prepared/config/_daily_upstream/schedule"),
                    Some(&json!(false))
                );
                assert_eq!(
                    packet.pointer("/prepared/config/_daily_upstream/facet_newsletter:work"),
                    Some(&json!(false))
                );
            } else {
                assert!(news.contains(&format!("{phase} newsletter")), "{news}");
                assert!(agenda.contains(&format!("{phase} appointment")), "{agenda}");
                if phase == "NEW" {
                    assert!(!news.contains("OLD newsletter"));
                    assert!(!agenda.contains("OLD appointment"));
                }
            }
            assert_eq!(
                solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
                    root, day, &talent, &apps
                )
                .unwrap()
                .state,
                solstone_core_system::daily_coverage::CoverageState::Current
            );
        }
        let requests = worker.outcomes.lock().unwrap().len();
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(
            worker.outcomes.lock().unwrap().len(),
            requests,
            "unchanged recovery is reusable"
        );
    }
    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn historical_days_share_the_current_maintenance_packet_and_failure_is_separate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        std::fs::write(talent.join("daily_schedule.md"), format!("{}\nChoose a daily processing time from $activity_spans.", serde_json::to_string_pretty(&json!({"type":"generate","schedule":"daily","priority":1,"output":"json","hook":{"pre":"daily_schedule","post":"daily_schedule"},"load":{"transcripts":false}})).unwrap())).unwrap();
        // Keep one real historical unit independently current when maintenance fails.
        std::fs::write(talent.join("schedule.md"), format!("{}\nExtract appointments.", serde_json::to_string_pretty(&json!({"type":"generate","schedule":"daily","priority":2,"output":"json","hook":{"post":"schedule"},"load":{"transcripts":true}})).unwrap())).unwrap();
        let today =
            solstone_core_system::daily_coverage::local_day(root, chrono::Utc::now()).unwrap();
        std::fs::create_dir_all(root.join("chronicle").join(&today).join("mic/090000_600"))
            .unwrap();
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            root.join("response.json"),
            model_response(r#"{"primary":"03:00"}"#, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(
            &stub,
            "#!/bin/sh\ncat >/dev/null\ncat \"${0%/*}/response.json\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let worker = Arc::new(Worker {
            context: solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_owned(),
            },
            paths: solstone_core_talent_runtime::prepare::RuntimePaths {
                talent_root: talent.clone(),
                apps_root: apps.clone(),
                templates_dir: root.join("payload/think/templates"),
            },
            stub,
            outcomes: Mutex::new(Default::default()),
        });
        let identity = DailyUnitIdentity::new("20200101", "daily_schedule", None);
        let mut accepted_token = None;
        for day in ["20200101", "20200201"] {
            let context = ThinkContext::new(
                root,
                day.into(),
                root.join("chronicle").join(day),
                1789400000000,
            )
            .unwrap()
            .with_talent_roots(talent.clone(), apps.clone())
            .with_boundary(worker.clone());
            let outcome = run(
                &context,
                &mut RunLogWriter::open(root, day, "daily"),
                None,
                false,
                1,
            )
            .unwrap();
            assert_eq!(outcome.failed, 0, "{outcome:?}");
            let record = load_daily_unit_record(root, &identity).unwrap().unwrap();
            assert_eq!(
                record
                    .frozen_packet
                    .as_ref()
                    .unwrap()
                    .pointer("/prepared/config/day"),
                Some(&json!(today))
            );
            assert!(
                record
                    .frozen_packet
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .contains("09:00"),
                "actual current-window span was frozen"
            );
            if let Some(token) = &accepted_token {
                assert_eq!(record.lock_token.as_ref(), Some(token));
            }
            accepted_token = record.lock_token;
        }
        assert_eq!(
            worker.outcomes.lock().unwrap().len(),
            3,
            "two historical units share one committed maintenance use"
        );
        let before = std::fs::read(root.join("config/schedules.json")).unwrap();
        std::fs::create_dir_all(root.join("chronicle").join(&today).join("mic/120000_600"))
            .unwrap();
        std::fs::write(
            root.join("response.json"),
            model_response("invalid schedule", Value::Null).to_string(),
        )
        .unwrap();
        let day = "20200301";
        let context = ThinkContext::new(
            root,
            day.into(),
            root.join("chronicle").join(day),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let outcome = run(
            &context,
            &mut RunLogWriter::open(root, day, "daily"),
            None,
            false,
            1,
        )
        .unwrap();
        assert_eq!(
            outcome.failed, 0,
            "global maintenance failure does not fail historical coverage"
        );
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            root, day, &talent, &apps,
        )
        .unwrap();
        assert_eq!(
            coverage.units.len(),
            1,
            "historical coverage must not be vacuous"
        );
        assert_eq!(
            coverage.state,
            solstone_core_system::daily_coverage::CoverageState::Current
        );
        assert_eq!(
            coverage.maintenance.unwrap().state,
            solstone_core_system::daily_coverage::CoverageState::Outstanding
        );
        assert_eq!(
            std::fs::read(root.join("config/schedules.json")).unwrap(),
            before
        );
        assert_eq!(worker.outcomes.lock().unwrap().len(), 5);
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn newer_maintenance_window_fences_late_worker_and_refuses_older_readmission() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let context = solstone_core_talent_runtime::ExecutionContext {
            journal: root.to_owned(),
        };
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        let current_day = chrono::Utc::now().date_naive();
        let today = current_day.format("%Y%m%d").to_string();
        let previous = (current_day - chrono::Duration::days(1))
            .format("%Y%m%d")
            .to_string();
        let make_packet = |day: &str| {
            solstone_core_talent_runtime::daily_prepare::freeze(
                solstone_core_talent_runtime::PreparedTalent { name:"daily_schedule".into(), config:json!({
                    "name":"daily_schedule", "day":day,"type":"generate","model":"test-model","provider":"test",
                    "prompt":"Choose daily time from $activity_spans.","hook":{"pre":"daily_schedule","post":"daily_schedule"}
                }).as_object().unwrap().clone() }, &context
            ).unwrap()
        };
        // The persisted prior-window packet is the restart boundary. Publication
        // must fence it by its admitted window/token even while its model runs.
        let old_packet = make_packet(&previous);
        let current_packet = make_packet(&today);
        let identity = DailyUnitIdentity::new("20200101", "daily_schedule", None);
        reserve_daily_attempt(
            root,
            &identity,
            "old-window-E",
            "C",
            &old_packet,
            &previous,
            "old-worker",
            false,
            false,
            1,
        )
        .unwrap();
        let started = root.join("model-started");
        let release = root.join("model-release");
        let old_stub = root.join("old-model.sh");
        std::fs::write(
            root.join("old-response.json"),
            model_response(r#"{"primary":"02:00"}"#, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(&old_stub, "#!/bin/sh\ncat >/dev/null\ntouch \"${0%/*}/model-started\"\nattempt=0\nwhile [ ! -f \"${0%/*}/model-release\" ]; do\n attempt=$((attempt+1))\n [ \"$attempt\" -lt 500 ] || exit 93\n sleep 0.01\ndone\ncat \"${0%/*}/old-response.json\"\n").unwrap();
        std::fs::set_permissions(&old_stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Publish both executables before any model subprocess starts.
        let new_stub = root.join("new-model.sh");
        std::fs::write(
            root.join("new-response.json"),
            model_response(r#"{"primary":"04:00"}"#, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(
            &new_stub,
            "#!/bin/sh\ncat >/dev/null\ncat \"${0%/*}/new-response.json\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&new_stub, std::fs::Permissions::from_mode(0o700)).unwrap();
        let paths = solstone_core_talent_runtime::prepare::RuntimePaths {
            talent_root: root.join("no-live-talent"),
            apps_root: root.join("no-live-apps"),
            templates_dir: root.join("no-live-templates"),
        };
        let worker_context = context.clone();
        let worker_paths = paths.clone();
        let old = std::thread::spawn(move || {
            solstone_core_talent_runtime::execute_request(
                json!({"name":"daily_schedule","day":"20200101","lock_token":"old-worker"})
                    .as_object()
                    .unwrap()
                    .clone(),
                &worker_paths,
                &worker_context,
                &solstone_core_generate::OneShotClient::at_path(old_stub),
                &solstone_core_cogitate_wire::CogitateOneShotClient::at_path(
                    worker_context.journal.join("no-cogitate"),
                ),
                &mut Vec::new(),
            )
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !started.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            started.exists(),
            "old maintenance model did not reach barrier"
        );
        let newer_identity = DailyUnitIdentity::new("20200201", "daily_schedule", None);
        assert_eq!(identity, newer_identity);
        reserve_daily_attempt(
            root,
            &newer_identity,
            "current-window-E",
            "C",
            &current_packet,
            &today,
            "new-worker",
            false,
            false,
            2,
        )
        .unwrap();
        let newer = solstone_core_talent_runtime::execute_request(
            json!({"name":"daily_schedule","day":"20200201","lock_token":"new-worker"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &solstone_core_generate::OneShotClient::at_path(new_stub),
            &solstone_core_cogitate_wire::CogitateOneShotClient::at_path(root.join("no-cogitate")),
            &mut Vec::new(),
        );
        assert!(
            matches!(
                newer,
                solstone_core_talent_runtime::RuntimeOutcome::Finished { .. }
            ),
            "{newer:?}"
        );
        let committed = std::fs::read(root.join("config/schedules.json")).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&committed).unwrap()["daily_time"],
            "04:00"
        );
        std::fs::write(release, b"resume").unwrap();
        let old_outcome = old.join().unwrap();
        let solstone_core_talent_runtime::RuntimeOutcome::StageFailed(error) = old_outcome else {
            panic!("old maintenance worker must lose publication authority: {old_outcome:?}");
        };
        assert_eq!(error.phase, "write");
        assert_eq!(error.stage, "daily_publication");
        assert!(
            error.detail.contains("publication token was replaced"),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(root.join("config/schedules.json")).unwrap(),
            committed
        );
        let refused = reserve_daily_attempt(
            root,
            &identity,
            "old-window-E",
            "C",
            &old_packet,
            &previous,
            "late-readmission",
            false,
            false,
            3,
        )
        .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("newer maintenance window already owns publication"),
            "{refused}"
        );
        let record = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(record.lock_token.as_deref(), Some("new-worker"));
        assert_eq!(
            record.accepted.unwrap().evidence_revision,
            "current-window-E"
        );
    }

    #[test]
    fn uncommitted_started_receipt_blocks_reserve_daily_attempt() {
        let root = tempfile::tempdir().unwrap();
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            "20260910",
            "entities:entities_review",
            Some("work".into()),
        );
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(identity.clone(), "E", "C");
        record.receipts.push(serde_json::json!({
            "kind": "owner_action",
            "action_id": "0:action_1",
            "token": "old_token",
            "state": "started"
        }));
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();

        let packet = Value::Object(Default::default());
        let err = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &packet,
            "20260910",
            "worker-token",
            false,
            false,
            3,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("ambiguous uncommitted owner action"),
            "expected started action refusal: {err}"
        );

        // Even with from_scratch: true, uncommitted started receipt must refuse and not wipe
        let err_scratch = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &packet,
            "20260910",
            "worker-token",
            true,
            false,
            3,
        )
        .unwrap_err();

        assert!(
            err_scratch
                .to_string()
                .contains("ambiguous uncommitted owner action"),
            "expected started action refusal even from_scratch: {err_scratch}"
        );

        let stored = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert!(
            stored.has_uncommitted_started_receipt(),
            "started receipts must not be wiped"
        );
    }

    #[test]
    fn entities_review_one_retry_then_suppress_and_fresh_attempt_resets() {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path();
        std::fs::create_dir_all(journal.join("chronicle/20260910/segments")).unwrap();
        std::fs::create_dir_all(journal.join("facets/work")).unwrap();
        std::fs::write(journal.join("facets/work/facet.json"), r#"{"name":"work"}"#).unwrap();
        solstone_core_facets::ensure_daily_facet_id(journal, "work").unwrap();
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
        )
        .unwrap();

        let talent_root = journal.join("payload/talent");
        let apps_root = journal.join("payload/apps");
        std::fs::create_dir_all(&talent_root).unwrap();
        std::fs::create_dir_all(apps_root.join("entities/talent")).unwrap();
        std::fs::write(
            apps_root.join("entities/talent/entities_review.md"),
            "{\n\"type\":\"generate\",\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nprompt",
        )
        .unwrap();

        struct MockCortex;
        impl crate::context::CortexBoundary for MockCortex {
            fn dispatch(
                &self,
                _: &tokio::runtime::Runtime,
                _: &solstone_core_cortex_client::CortexRequest,
            ) -> Result<String, DispatchFailure> {
                panic!("daily must reserve before send")
            }
            fn dispatch_prepared(
                &self,
                _: &tokio::runtime::Runtime,
                _: &solstone_core_cortex_client::CortexRequest,
                reserved: Option<&str>,
                prepare: &mut (dyn FnMut(&str) -> std::io::Result<()> + Send),
            ) -> Result<String, DispatchFailure> {
                let id = reserved.expect("durably allocated use id");
                prepare(id).map_err(|_| DispatchFailure::Unavailable)?;
                Ok(id.to_string())
            }
            fn wait(
                &self,
                _: &tokio::runtime::Runtime,
                _: &[String],
                _: Option<std::time::Duration>,
            ) -> Result<solstone_core_cortex_client::WaitForUsesReport, String> {
                Ok(solstone_core_cortex_client::WaitForUsesReport {
                    completed: Default::default(),
                    timed_out: Vec::new(),
                })
            }
        }

        let context = ThinkContext::new(
            journal,
            "20260910".to_owned(),
            journal.join("chronicle/20260910"),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent_root, apps_root)
        .with_boundary(std::sync::Arc::new(MockCortex));

        let configs = load_talent_configs(
            &context.talent_root,
            &context.apps_root,
            None,
            TalentFilter {
                r#type: None,
                schedule: Some("daily"),
                include_disabled: false,
            },
        )
        .unwrap();
        let config = configs
            .into_iter()
            .find(|c| c.key == "entities:entities_review")
            .unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut log = RunLogWriter::open(journal, &context.day, "daily");

        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            "20260910",
            "entities:entities_review",
            Some("work".into()),
        );

        let (evidence_rev, contract_dig) = crate::snapshot::compute_daily_evidence_revision(
            journal,
            "20260910",
            &config,
            Some("work"),
            None,
        )
        .unwrap();

        // 1. Initial Conflicting failure (failure_count == 1)
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(
            identity.clone(),
            &evidence_rev,
            &contract_dig,
        );
        record.status = solstone_core_journal_io::DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        record.owner_conflict_kind = Some("alias_claimed".into());
        record.failure_count = 1;
        solstone_core_journal_io::save_daily_unit_record(journal, &record).unwrap();

        let mut pending = Vec::new();
        let mut result = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            false,
            &mut pending,
            &mut result,
        )
        .unwrap();
        assert_eq!(
            pending.len(),
            1,
            "failure_count == 1 must dispatch once on retry"
        );
        assert!(result.terminal_units.is_empty());
        assert!(result.capped_units.is_empty());

        // 2. Second Conflicting failure (failure_count == 2) -> suppressed
        record.failure_count = 2;
        solstone_core_journal_io::save_daily_unit_record(journal, &record).unwrap();

        let mut pending2 = Vec::new();
        let mut result2 = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            false,
            &mut pending2,
            &mut result2,
        )
        .unwrap();
        assert_eq!(
            pending2.len(),
            0,
            "failure_count == 2 must be suppressed on unchanged evidence"
        );
        assert!(
            result2.terminal_units.is_empty(),
            "suppressed unit must not be terminal"
        );
        assert!(
            result2.capped_units.is_empty(),
            "suppressed unit must not be capped"
        );
        assert!(
            !journal
                .join("chronicle/20260910/health/daily.updated")
                .exists()
        );

        // 3. Fresh attempt via from_scratch: true resets suppression
        let mut pending_scratch = Vec::new();
        let mut result_scratch = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            true,
            &mut pending_scratch,
            &mut result_scratch,
        )
        .unwrap();
        assert_eq!(
            pending_scratch.len(),
            1,
            "from_scratch must reset suppression and dispatch"
        );

        // 4. Fresh attempt via evidence change resets suppression
        std::fs::write(
            journal.join("facets/work/facet.json"),
            r#"{"name":"work","extra":"1"}"#,
        )
        .unwrap();
        solstone_core_facets::ensure_daily_facet_id(journal, "work").unwrap();
        let mut pending_ev = Vec::new();
        let mut result_ev = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            false,
            &mut pending_ev,
            &mut result_ev,
        )
        .unwrap();
        assert_eq!(
            pending_ev.len(),
            1,
            "evidence change must reset suppression and dispatch"
        );

        // 5. Uncommitted started receipt blocks dispatch under all conditions
        record.receipts.push(serde_json::json!({
            "kind": "owner_action",
            "action_id": "0:test",
            "token": "tok",
            "state": "started"
        }));
        solstone_core_journal_io::save_daily_unit_record(journal, &record).unwrap();

        let mut pending_started = Vec::new();
        let mut result_started = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            false,
            &mut pending_started,
            &mut result_started,
        )
        .unwrap();
        assert_eq!(
            pending_started.len(),
            0,
            "started receipt must block queue_daily"
        );

        let mut pending_started_scratch = Vec::new();
        let mut result_started_scratch = ModeResult::default();
        queue_daily(
            &context,
            &mut log,
            &runtime,
            &config,
            Some("work"),
            true,
            &mut pending_started_scratch,
            &mut result_started_scratch,
        )
        .unwrap();
        assert_eq!(
            pending_started_scratch.len(),
            0,
            "started receipt must block queue_daily even from_scratch"
        );
    }

    #[test]
    fn exhausted_review_conflict_is_refused_inside_locked_reserve() {
        let root = tempfile::tempdir().unwrap();
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            "20260910",
            "entities:entities_review",
            Some("work".into()),
        );
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = solstone_core_journal_io::DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        record.owner_conflict_kind = Some("alias_claimed".into());
        record.failure_count = 2;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
        let packet = Value::Object(Default::default());
        let journal = root.path().to_path_buf();
        let identity_one = identity.clone();
        let identity_two = identity.clone();
        let packet_one = packet.clone();
        let packet_two = packet.clone();
        let first = std::thread::spawn(move || {
            reserve_daily_attempt(
                &journal,
                &identity_one,
                "E",
                "C",
                &packet_one,
                "20260910",
                "worker-a",
                false,
                false,
                3,
            )
        });
        let journal = root.path().to_path_buf();
        let second = std::thread::spawn(move || {
            reserve_daily_attempt(
                &journal,
                &identity_two,
                "E",
                "C",
                &packet_two,
                "20260910",
                "worker-b",
                false,
                false,
                4,
            )
        });
        let first_err = first.join().unwrap().unwrap_err();
        let second_err = second.join().unwrap().unwrap_err();
        assert!(
            matches!(
                first_err,
                solstone_core_journal_io::DailyUnitError::ReviewOwnerConflictExhausted
            ),
            "{first_err}"
        );
        assert!(
            matches!(
                second_err,
                solstone_core_journal_io::DailyUnitError::ReviewOwnerConflictExhausted
            ),
            "{second_err}"
        );
        let stored = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.status,
            solstone_core_journal_io::DailyUnitStatus::Conflicting
        );
        assert_eq!(stored.failure_count, 2);
        assert_eq!(stored.lock_token, None);
    }
}
