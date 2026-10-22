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
    let mut configs = load_talent_configs(
        &context.talent_root,
        &context.apps_root,
        solstone_core_talent_config::read_talent_overrides(&context.journal)?.as_ref(),
        TalentFilter {
            r#type: None,
            schedule: Some("daily"),
            include_disabled: false,
        },
    )?;
    if let Some((solstone_core_system::work_retry::Unit::Daily { talent, .. }, _)) = &context.retry
    {
        configs.retain(|config| &config.key == talent);
    }
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
        if solstone_core_indexer::daily_evidence::daily_hook(&config.key, &config.metadata).is_err()
        {
            continue;
        }
        if config.metadata.get("multi_facet") == Some(&Value::Bool(true)) {
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
    if let Some((solstone_core_system::work_retry::Unit::Daily { talent, facet, .. }, _)) =
        &context.retry
        && talent != "daily_schedule"
    {
        write_facets.retain(|name| facet.as_ref() == Some(name));
    }
    for facet in write_facets {
        solstone_core_facets::ensure_daily_facet_id(&context.journal, &facet)?;
    }
    solstone_core_system::daily_coverage::activate_daily_inputs(
        &context.journal,
        chrono::Utc::now(),
    )?;
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
    let retry_unit = solstone_core_system::work_retry::Unit::Daily {
        day: context.day.clone(),
        talent: config.key.clone(),
        facet: facet.map(str::to_owned),
    };
    if context
        .retry
        .as_ref()
        .is_some_and(|(unit, _)| unit.id().ok() != retry_unit.id().ok())
    {
        return Ok(());
    }
    let grant = context.retry.as_ref().map(|(_, attempt)| attempt);
    let unit = (config.key.clone(), facet.map(ToOwned::to_owned));
    if config.key != "daily_schedule" {
        result.applicable_units.insert(unit.clone());
    }
    let today =
        solstone_core_system::daily_coverage::local_day(&context.journal, chrono::Utc::now());
    let evidence_day = if config.key == "daily_schedule" {
        &today
    } else {
        &context.day
    };
    #[cfg(test)]
    {
        let early =
            BEFORE_DAILY_EVIDENCE.with(|slot| slot.borrow_mut().as_mut().map(|hook| hook(context)));
        if let Some(result) = early {
            return result;
        }
    }
    let evidence =
        crate::snapshot::compute_daily_evidence(&context.journal, evidence_day, config, facet)?;
    let (evidence_rev, contract_dig) = (evidence.revision.clone(), evidence.contract.clone());
    let identity = solstone_core_journal_io::DailyUnitIdentity::new(
        &context.day,
        &config.key,
        facet.map(ToOwned::to_owned),
    );
    let existing_record =
        solstone_core_journal_io::load_daily_unit_record(&context.journal, &identity)
            .map_err(|e| e.to_string())?;
    let retry = grant.is_some();
    // The owner asked to retry an entity review whose conflict a fresh
    // preparation can clear; its retained packet and plan are what conflicted.
    let reprepare = retry
        && existing_record
            .as_ref()
            .is_some_and(solstone_core_system::daily_coverage::review_conflict_reprepares);
    if let Some(record) = &existing_record {
        if record.has_uncommitted_started_receipt() {
            // An in-doubt started write cannot be automatically retried, reset, or wiped.
            return Ok(());
        }
        if !from_scratch {
            // The same reuse rule the coverage reader applies, so a unit that
            // reads current is never regenerated by its own day's run.
            let reuse = solstone_core_system::daily_coverage::accepted_reuse(
                record,
                &evidence,
                solstone_core_system::daily_coverage::local_now(
                    &context.journal,
                    chrono::TimeZone::timestamp_millis_opt(&chrono::Utc, context.now_ms)
                        .single()
                        .unwrap_or_else(chrono::Utc::now),
                ),
            );
            if let Some(kept) = reuse {
                result.terminal_units.insert(unit);
                let reason = match kept {
                    None => "evidence_unchanged",
                    Some(solstone_core_system::daily_coverage::AcceptedReuse::FrozenBriefing) => {
                        "frozen_briefing"
                    }
                    Some(solstone_core_system::daily_coverage::AcceptedReuse::EarlierContract) => {
                        "earlier_contract"
                    }
                };
                log_skip(log, context, &config.key, reason, facet);
                return Ok(());
            }
            if (record.status == solstone_core_journal_io::DailyUnitStatus::Capped
                || solstone_core_system::work_retry::read(&context.journal, &retry_unit.id()?)?
                    .is_some_and(|r| r.held))
                && grant.is_none()
            {
                result.terminal_units.insert(unit.clone());
                result.capped_units.insert(unit);
                return Ok(());
            }
            if !reprepare
                && record.evidence_revision == evidence_rev
                && record.contract_digest == contract_dig
            {
                match solstone_core_system::daily_coverage::conflict_recovery(
                    record,
                    &today,
                    &context.journal,
                ) {
                    solstone_core_system::daily_coverage::ConflictRecovery::StaleDeferred
                    | solstone_core_system::daily_coverage::ConflictRecovery::Exhausted => {
                        return Ok(());
                    }
                    solstone_core_system::daily_coverage::ConflictRecovery::Open => {}
                }
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
    if let Some(attempt) = grant {
        if evidence.retry_revision_under(solstone_core_system::work_retry::daily::INPUT_CONTRACT)
            != attempt.input
        {
            return Err("daily retry input changed".into());
        }
        extra.insert(
            "_work_retry".into(),
            serde_json::to_value((&retry_unit, attempt)).map_err(|e| e.to_string())?,
        );
    }
    if config.key == "daily_schedule" {
        extra.insert("day".to_owned(), Value::String(today.clone()));
    }
    let retained = existing_record
        .as_ref()
        .filter(|record| {
            !from_scratch
                && !reprepare
                && record.evidence_revision == evidence_rev
                && record.contract_digest == contract_dig
        })
        .and_then(|record| record.frozen_packet.clone());
    let hook = solstone_core_indexer::daily_evidence::daily_hook(&config.key, &config.metadata)?;
    let recovering = existing_record
        .as_ref()
        .is_some_and(solstone_core_talent_runtime::daily_location::has_owner_actions)
        && retained.is_some();
    let freshness = if recovering {
        solstone_core_talent_runtime::daily_location::DailyLocationFreshness::NotApplicable
    } else {
        solstone_core_talent_runtime::daily_location::daily_location_freshness(
            &context.journal,
            &hook,
            evidence_day,
            retained.as_ref(),
        )?
    };
    let frozen_packet = match freshness {
        solstone_core_talent_runtime::daily_location::DailyLocationFreshness::NotApplicable => {
            match retained {
                Some(packet) => packet,
                None => crate::snapshot::prepare_daily_packet(context, config, facet, &extra)?,
            }
        }
        solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Reuse => {
            retained.ok_or_else(|| "daily location reuse has no stored packet".to_owned())?
        }
        solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Stale => {
            crate::snapshot::prepare_daily_packet(context, config, facet, &extra)?
        }
        solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Unreadable => {
            return Err("daily location could not be read".into());
        }
    };
    let mut reservation_error = None;
    let mut conflict_exhausted = false;
    let checkpoint = || context.checkpoint();
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
            &checkpoint,
        ) {
            Ok(()) => Ok(()),
            Err(ReserveAttemptError::Unit(
                solstone_core_journal_io::DailyUnitError::OwnerConflictExhausted,
            )) => {
                conflict_exhausted = true;
                Err(std::io::Error::other("owner conflict retry exhausted"))
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
    if conflict_exhausted {
        return Ok(());
    }
    if let Some(error) = reservation_error {
        return Err(error);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum ReserveAttemptError {
    Unit(solstone_core_journal_io::DailyUnitError),
    Stopped(String),
}

impl std::fmt::Display for ReserveAttemptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unit(error) => error.fmt(formatter),
            Self::Stopped(message) => formatter.write_str(message),
        }
    }
}

impl From<solstone_core_journal_io::DailyUnitError> for ReserveAttemptError {
    fn from(error: solstone_core_journal_io::DailyUnitError) -> Self {
        Self::Unit(error)
    }
}

enum ReserveStamp {
    Reserved,
    Stopped(String),
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
    checkpoint: &dyn Fn() -> Result<(), String>,
) -> Result<(), ReserveAttemptError> {
    let _execution = solstone_core_journal_io::claim_daily_execution(journal, identity)?;
    match solstone_core_journal_io::with_daily_unit_authority(journal, identity, |authority| {
        let existed = authority.record().is_some();
        let mut record = authority.record_mut().take().unwrap_or_else(|| {
            solstone_core_journal_io::DailyUnitRecord::new(
                identity.clone(),
                evidence_rev,
                contract_dig,
            )
        });
        let baseline = record.clone();
        if record.has_uncommitted_started_receipt() {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "unit has ambiguous uncommitted owner action".to_owned(),
            ));
        }
        let same =
            record.evidence_revision == evidence_rev && record.contract_digest == contract_dig;
        // Re-checked under the lock. Committed changes stay in the journal and
        // the replacement attempt prepares against them.
        let reprepare =
            retry && solstone_core_system::daily_coverage::review_conflict_reprepares(&record);
        if (!same || from_scratch)
            && !reprepare
            && !record.status.is_terminal_success()
            && record
                .receipts
                .iter()
                .any(|receipt| receipt.get("kind").and_then(Value::as_str) == Some("owner_action"))
        {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "unit has a change recorded in its log; resume that change before replacing the attempt".to_owned(),
            ));
        }
        let keep_attempt_day = same
            && !from_scratch
            && record.status == solstone_core_journal_io::DailyUnitStatus::Unfinished
            && record.frozen_packet.is_some();
        if same && !from_scratch && !reprepare {
            match solstone_core_system::daily_coverage::conflict_recovery(&record, today, journal) {
                solstone_core_system::daily_coverage::ConflictRecovery::Exhausted
                | solstone_core_system::daily_coverage::ConflictRecovery::StaleDeferred => {
                    return Err(solstone_core_journal_io::DailyUnitError::OwnerConflictExhausted);
                }
                solstone_core_system::daily_coverage::ConflictRecovery::Open => {
                    if solstone_core_system::daily_coverage::stale_receipt_free_conflict(&record)
                        && solstone_core_system::daily_coverage::conflict_attempt_day(
                            &record, journal,
                        )
                        .as_deref()
                        .is_some_and(|day| day < today)
                        && !keep_attempt_day
                    {
                        record.failure_count = 0;
                    }
                }
            }
        }

        if !from_scratch
            && !retry
            && (record.status == solstone_core_journal_io::DailyUnitStatus::Capped
                || record.reason_code.as_deref().is_some_and(|reason| {
                    solstone_core_system::daily_coverage::daily_failure_capped(
                        reason,
                        record.failure_count,
                    )
                }))
        {
            return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                "daily retry allowance already consumed".to_owned(),
            ));
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
        let kept_attempt_day = record.attempt_day.clone();
        if !same || from_scratch || reprepare {
            let accepted = record.accepted.take();
            record = solstone_core_journal_io::DailyUnitRecord::new(
                identity.clone(),
                evidence_rev,
                contract_dig,
            );
            record.accepted = accepted;
        }
        // An interrupted same-revision attempt retains the exact packet unless its
        // location fingerprint is stale. A stale caller is stored only when it
        // matches a fresh read; otherwise the four attempt fields stay cleared.
        let freshness = if solstone_core_talent_runtime::daily_location::has_owner_actions(&record)
        {
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::NotApplicable)
        } else {
            solstone_core_talent_runtime::daily_location::daily_location_freshness(
                journal,
                &identity.name,
                &identity.day,
                record.frozen_packet.as_ref(),
            )
        };
        match freshness {
            Err(error) => return Err(solstone_core_journal_io::DailyUnitError::Malformed(error)),
            Ok(
                solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Unreadable,
            ) => {
                return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                    "daily location could not be read".to_owned(),
                ));
            }
            Ok(
                solstone_core_talent_runtime::daily_location::DailyLocationFreshness::NotApplicable,
            ) => {
                if record.frozen_packet.is_none() {
                    record.frozen_packet = Some(frozen_packet.clone());
                }
            }
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Reuse) => {}
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Stale) => {
                let caller_matches = matches!(
                    solstone_core_talent_runtime::daily_location::daily_location_freshness(
                        journal,
                        &identity.name,
                        &identity.day,
                        Some(frozen_packet),
                    ),
                    Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Reuse)
                );
                if caller_matches {
                    record.packet_digest = None;
                    record.generated_result = None;
                    record.action_plan = None;
                    record.frozen_packet = Some(frozen_packet.clone());
                } else if existed {
                    if let Err(message) = checkpoint() {
                        return Ok(ReserveStamp::Stopped(message));
                    }
                    let mut cleared = baseline;
                    cleared.frozen_packet = None;
                    cleared.packet_digest = None;
                    cleared.generated_result = None;
                    cleared.action_plan = None;
                    *authority.record_mut() = Some(cleared);
                    authority.checkpoint()?;
                    return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                        "daily location fingerprint is stale".to_owned(),
                    ));
                } else {
                    return Err(solstone_core_journal_io::DailyUnitError::Malformed(
                        "daily location fingerprint is stale".to_owned(),
                    ));
                }
            }
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
        if let Err(message) = checkpoint() {
            // A stop is not a record. Leave nothing staged for a later Ok.
            authority.record_mut().take();
            return Ok(ReserveStamp::Stopped(message));
        }
        *authority.record_mut() = Some(record);
        authority.checkpoint()?;
        Ok(ReserveStamp::Reserved)
    })? {
        ReserveStamp::Reserved => Ok(()),
        ReserveStamp::Stopped(message) => Err(ReserveAttemptError::Stopped(message)),
    }
}

#[cfg(test)]
type DailyEvidenceHook = Box<dyn FnMut(&crate::context::ThinkContext) -> Result<(), String>>;

#[cfg(test)]
thread_local! {
    static BEFORE_DAILY_EVIDENCE: std::cell::RefCell<Option<DailyEvidenceHook>> = const { std::cell::RefCell::new(None) };
}

/// Run `hook` at the start of each daily queue, before evidence is read.
#[cfg(test)]
pub(crate) fn set_before_daily_evidence(hook: Option<DailyEvidenceHook>) {
    BEFORE_DAILY_EVIDENCE.with(|slot| *slot.borrow_mut() = hook);
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

    fn at(day: &str, hour: u32, minute: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::parse_from_str(day, "%Y%m%d")
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap()
    }

    fn at_midnight(day: &str) -> chrono::NaiveDateTime {
        at(day, 0, 0)
    }

    #[test]
    fn calendar_contract_and_concurrent_reservations_never_reset_a_cap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        for reason in [
            "model_not_found",
            "provider_request_rejected",
            "schema_invalid",
        ] {
            let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
            record.status = DailyUnitStatus::Capped;
            record.reason_code = Some(reason.into());
            record.failure_count = 3;
            save_daily_unit_record(root, &record).unwrap();
            std::thread::scope(|scope| {
                for contract in ["C", "new-contract"] {
                    let identity = &identity;
                    scope.spawn(move || {
                        assert!(
                            reserve_daily_attempt(
                                root,
                                identity,
                                "E",
                                contract,
                                &json!({}),
                                "20261011",
                                "new",
                                false,
                                false,
                                1,
                                &|| Ok(())
                            )
                            .is_err()
                        );
                    });
                }
            });
            let held = load_daily_unit_record(root, &identity).unwrap().unwrap();
            assert_eq!(held.status, DailyUnitStatus::Capped);
            assert_eq!(held.failure_count, 3);
        }
    }

    #[cfg(feature = "full-tests")]
    fn permit_owner_daily(context: &ThinkContext, identity: &DailyUnitIdentity) -> ThinkContext {
        use solstone_core_system::work_retry::{self, Unit};
        let unit = Unit::Daily {
            day: context.day.clone(),
            talent: identity.name.clone(),
            facet: identity.facet.clone(),
        };
        let record = load_daily_unit_record(&context.journal, identity)
            .unwrap()
            .unwrap();
        let evidence = work_retry::daily::evidence_with_roots(
            &context.journal,
            &unit,
            &context.talent_root,
            &context.apps_root,
        )
        .unwrap();
        let input = evidence.retry_revision_under(work_retry::daily::INPUT_CONTRACT);
        work_retry::observe_held(
            &context.journal,
            &unit,
            record.reason_code.as_deref().unwrap(),
            &input,
        )
        .unwrap();
        work_retry::request(&context.journal, &unit).unwrap();
        let grant = work_retry::read(&context.journal, &unit.id().unwrap())
            .unwrap()
            .unwrap()
            .permission(&input, work_retry::DECLARED_FIXES)
            .unwrap();
        assert!(grant.cause.starts_with("owner:"));
        let mut context = context.clone();
        context.retry = Some((unit, grant));
        context
    }

    #[cfg(feature = "full-tests")]
    fn permit_changed_daily(context: &ThinkContext, identity: &DailyUnitIdentity) -> ThinkContext {
        use solstone_core_system::work_retry::{self, Unit};
        let unit = Unit::Daily {
            day: context.day.clone(),
            talent: identity.name.clone(),
            facet: identity.facet.clone(),
        };
        let record = load_daily_unit_record(&context.journal, identity)
            .unwrap()
            .unwrap();
        let evidence = work_retry::daily::evidence_with_roots(
            &context.journal,
            &unit,
            &context.talent_root,
            &context.apps_root,
        )
        .unwrap();
        assert_ne!(
            evidence.revision_under(&record.contract_digest),
            record.evidence_revision
        );
        work_retry::observe_held(
            &context.journal,
            &unit,
            record.reason_code.as_deref().unwrap(),
            &record.evidence_revision,
        )
        .unwrap();
        let current = evidence.retry_revision_under(work_retry::daily::INPUT_CONTRACT);
        let grant = work_retry::read(&context.journal, &unit.id().unwrap())
            .unwrap()
            .unwrap()
            .permission(&current, work_retry::DECLARED_FIXES)
            .unwrap();
        assert!(grant.cause.starts_with("input:"));
        let mut context = context.clone();
        context.retry = Some((unit, grant));
        context
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
            &|| Ok(()),
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
            &|| Ok(()),
        );
        assert!(
            second.is_err(),
            "second reservation was allowed: {second:?}"
        );
    }

    #[test]
    fn provider_request_rejected_multi_day_fold_preserves_cap_across_days() {
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
            &|| Ok(()),
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

        // A new calendar day does not grant retry permission.
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
            &|| Ok(()),
        );
        assert!(
            second.is_err(),
            "calendar day minted permission: {second:?}"
        );

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
            &|| Ok(()),
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
        let today = solstone_core_system::daily_coverage::local_day(root, instant);
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
            &|| Ok(()),
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
                            &|| Ok(()),
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
                &|| Ok(())
            )
            .is_err()
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
            &|| Ok(()),
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
                &|| Ok(())
            )
            .is_err()
        );
    }

    // The loopback HTTP 400 case lives in
    // `tests/daily_provider_rejection_loopback.rs`. Binding a TCP listener
    // trips the routine unit harness's hard-boundary ("network") topology
    // check, so that case runs in the `test-hooks` integration harness.

    #[test]
    fn stale_conflict_rollover_renews_even_one_failure_and_legacy_records() {
        for count in [1, 2] {
            for attempt_day in [Some("20260914"), None] {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path();
                std::fs::create_dir_all(root.join("config")).unwrap();
                std::fs::write(
                    root.join("config/journal.json"),
                    r#"{"identity":{"timezone":"America/Denver"}}"#,
                )
                .unwrap();
                let identity = DailyUnitIdentity::new(
                    "20260910",
                    "entities:entities_review",
                    Some("work".into()),
                );
                let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
                record.status = DailyUnitStatus::Conflicting;
                record.reason_code = Some("daily_owner_conflict".into());
                record.owner_conflict_kind = Some("merge_proposals_changed".into());
                record.failure_count = count;
                record.attempt_day = attempt_day.map(str::to_owned);
                record.updated_at_ms = chrono::DateTime::parse_from_rfc3339("2026-09-15T05:30:00Z")
                    .unwrap()
                    .timestamp_millis();
                save_daily_unit_record(root, &record).unwrap();
                reserve_daily_attempt(
                    root,
                    &identity,
                    "E",
                    "C",
                    &json!({}),
                    "20260915",
                    "new-use",
                    false,
                    false,
                    record.updated_at_ms + 3_600_000,
                    &|| Ok(()),
                )
                .unwrap();
                let admitted = load_daily_unit_record(root, &identity).unwrap().unwrap();
                assert_eq!(
                    admitted.failure_count, 0,
                    "count={count}, day={attempt_day:?}"
                );
                assert_eq!(admitted.attempt_day.as_deref(), Some("20260915"));
            }
        }
    }

    #[test]
    fn replacing_partial_publication_refuses_but_terminal_history_can_reowe() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let identity = DailyUnitIdentity::new("20260910", "schedule", None);
        let mut record = DailyUnitRecord::new(identity.clone(), "E", "C");
        record.frozen_packet = Some(json!({"frozen":"original"}));
        record.action_plan = Some(json!({"plan":"original"}));
        record.receipts =
            vec![json!({"kind":"owner_action", "action_id":"0:test", "state":"committed"})];
        for status in [
            DailyUnitStatus::Unfinished,
            DailyUnitStatus::Conflicting,
            DailyUnitStatus::Failed,
        ] {
            record.status = status;
            save_daily_unit_record(root, &record).unwrap();
            let path = solstone_core_journal_io::daily_unit_record_path(root, &identity);
            let before = std::fs::read(&path).unwrap();
            for (evidence, from_scratch) in [("new-E", false), ("E", true)] {
                assert!(
                    reserve_daily_attempt(
                        root,
                        &identity,
                        evidence,
                        "C",
                        &json!({"other":"packet"}),
                        "20260915",
                        "new-use",
                        from_scratch,
                        false,
                        2,
                        &|| Ok(())
                    )
                    .is_err()
                );
                assert_eq!(std::fs::read(&path).unwrap(), before);
            }
        }
        record.status = DailyUnitStatus::Committed;
        record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: "E".into(),
            contract_digest: "C".into(),
            status: DailyUnitStatus::Committed,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(json!({"response":"accepted", "output":""})),
            receipts: record.receipts.clone(),
            committed_at_ms: 1,
        });
        save_daily_unit_record(root, &record).unwrap();
        reserve_daily_attempt(
            root,
            &identity,
            "new-E",
            "C",
            &json!({}),
            "20260915",
            "new-use",
            false,
            false,
            2,
            &|| Ok(()),
        )
        .unwrap();
        let admitted = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(admitted.evidence_revision, "new-E");
        assert!(admitted.receipts.is_empty());
    }

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
            &|| Ok(()),
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
                2,
                &|| Ok(())
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
                3,
                &|| Ok(())
            )
            .is_err()
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
        sibling_rewrites: Option<std::sync::atomic::AtomicUsize>,
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
            if let Some(rewrites) = self.sibling_rewrites.as_ref() {
                let attempt = rewrites.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if attempt < 2 {
                    publish_test_proposal(
                        &self.context.journal,
                        ["20260911", "20260912"][attempt],
                        "Ada",
                        "Ada Lovelace",
                        ["first sibling rewrite", "second sibling rewrite"][attempt],
                    );
                }
            }
            let mut config = request.config.clone();
            config.insert("name".to_owned(), json!(request.name));
            let outcome = solstone_core_talent_runtime::execute_request(
                config,
                &self.paths,
                &self.context,
                &solstone_core_generate::OneShotClient::at_path(&self.stub),
                &mut Vec::new(),
                &solstone_core_generate::Cancellation::never(),
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

    #[cfg(all(test, feature = "full-tests", unix))]
    fn publish_test_proposal(
        root: &std::path::Path,
        day: &str,
        source: &str,
        target: &str,
        summary: &str,
    ) {
        let proposal = json!({"facet":"work", "day":day, "source":source,
            "source_slug":solstone_core_entity_matching::entity_slug(source), "target":target,
            "target_slug":solstone_core_entity_matching::entity_slug(target), "summary":summary});
        let prepared = solstone_core_entity::prepare_merge_proposals(root, &[proposal]).unwrap();
        solstone_core_entity::publish_merge_proposals(root, &prepared, true, || Ok(()), || Ok(()))
            .unwrap();
    }

    #[cfg(all(test, feature = "full-tests", unix))]
    #[test]
    fn two_actual_sibling_review_rewrites_defer_then_publish_without_losing_owner_edits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None).unwrap();
        for detection_day in ["20260908", "20260909"] {
            solstone_core_facets::upsert_detection_segment(
                root,
                "work",
                detection_day,
                "090000_300",
                &["Ada", "Ada Lovelace", "Grace", "Grace Hopper"].map(|name| {
                    solstone_core_facets::DetectedEntityInput {
                        entity_type: "Person".into(),
                        name: name.into(),
                        description: "Collaborator".into(),
                    }
                }),
            )
            .unwrap();
        }
        let segment = root.join("chronicle/20260910/default/090000_300/talents");
        std::fs::create_dir_all(&segment).unwrap();
        std::fs::write(segment.join("facets.json"), r#"[{"facet":"work"}]"#).unwrap();
        std::fs::write(
            segment.parent().unwrap().join("note_transcript.md"),
            "Discussed Ada and Grace.",
        )
        .unwrap();
        publish_test_proposal(root, "20260909", "Ada", "Ada Lovelace", "original");
        publish_test_proposal(root, "20260909", "Grace", "Grace Hopper", "unrelated");
        solstone_core_entity::dismiss_merge_candidate(root, "work", "grace", "grace_hopper")
            .unwrap()
            .unwrap();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(apps.join("entities/talent")).unwrap();
        std::fs::write(apps.join("entities/talent/entities_review.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nReview entities.").unwrap();
        let response = json!({"promotions":[], "merges":[{
            "source":"Ada", "canonical":"Ada Lovelace", "evidence":"same person"
        }]});
        std::fs::write(
            root.join("response.json"),
            model_response(&response.to_string(), Value::Null).to_string(),
        )
        .unwrap();
        let stub = root.join("generate-stub.sh");
        std::fs::write(&stub, "#!/bin/sh\ncat \"${0%/*}/response.json\"\n").unwrap();
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
            sibling_rewrites: Some(std::sync::atomic::AtomicUsize::new(0)),
        });
        let context = ThinkContext::new(
            root,
            day.into(),
            root.join("chronicle").join(day),
            chrono::Utc::now().timestamp_millis(),
        )
        .unwrap()
        .with_talent_roots(talent.clone(), apps.clone())
        .with_boundary(worker.clone());
        let identity = DailyUnitIdentity::new(day, "entities:entities_review", Some("work".into()));
        let mut log = RunLogWriter::open(root, day, "daily");
        let mut evidence = None;
        for expected_count in 1..=2 {
            let result = run(&context, &mut log, None, false, 1).unwrap();
            assert_eq!(
                result.failed,
                1,
                "{result:?}; worker outcomes={:?}",
                worker.outcomes.lock().unwrap()
            );
            let record = load_daily_unit_record(root, &identity).unwrap().unwrap();
            assert_eq!(record.failure_count, expected_count);
            assert_eq!(
                record.owner_conflict_kind.as_deref(),
                Some("merge_proposal_preparation")
            );
            assert!(
                record.frozen_packet.is_none()
                    && record.generated_result.is_none()
                    && record.receipts.is_empty()
            );
            if let Some(previous) = evidence.as_ref() {
                assert_eq!(&record.evidence_revision, previous);
            }
            evidence = Some(record.evidence_revision);
        }
        let deferred = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(deferred.failed, 0);
        assert_eq!(
            worker.outcomes.lock().unwrap().len(),
            2,
            "same-day bound must refuse a third worker"
        );
        let mut record = load_daily_unit_record(root, &identity).unwrap().unwrap();
        // Model the persisted record crossing midnight; all following admission,
        // preparation, model execution and publication use the ordinary run.
        record.attempt_day = Some(
            (chrono::Utc::now().date_naive() - chrono::Duration::days(1))
                .format("%Y%m%d")
                .to_string(),
        );
        save_daily_unit_record(root, &record).unwrap();
        let rows_before =
            solstone_core_entity::load_merge_candidates(root, Some("work"), None).unwrap();
        let owner_row = rows_before
            .iter()
            .find(|row| row["source_slug"] == "grace")
            .unwrap()
            .clone();
        assert_eq!(owner_row["status"], "dismissed");
        let recovered = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(recovered.failed, 0, "{recovered:?}");
        let committed = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert!(committed.status.is_terminal_success());
        assert_eq!(worker.outcomes.lock().unwrap().len(), 3);
        let rows = solstone_core_entity::load_merge_candidates(root, Some("work"), None).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .find(|row| row["source_slug"] == "grace")
                .unwrap(),
            &owner_row
        );
        assert_eq!(
            rows.iter().find(|row| row["source_slug"] == "ada").unwrap()["evidence"]["summary"],
            "same person"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(unix)]
    #[test]
    fn ordinary_daily_rerun_consumes_new_evidence_and_commits_changed_owner_result() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"),r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        // Today's briefing is never frozen, so new evidence always re-owes it.
        let day = solstone_core_system::daily_coverage::local_day(root, chrono::Utc::now());
        let day = day.as_str();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(talent.join("morning_briefing.md"),"{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":10,\"output\":\"json\",\"hook\":{\"pre\":\"morning_briefing\"},\"load\":{\"transcripts\":true}\n}\nWrite the morning briefing. $briefing_metadata $activity_digest").unwrap();
        let source = root.join(format!(
            "chronicle/{day}/default/090000_60/note_transcript.md"
        ));
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow is red.",
        )
        .unwrap();
        let stub = root.join("generate-stub.sh");
        for title in ["red", "blue"] {
            let briefing = json!({"summary":format!("The meeting topic is {title}")});
            std::fs::write(
                root.join(format!("response-{title}.json")),
                model_response(&briefing.to_string(), Value::Null).to_string(),
            )
            .unwrap();
        }
        std::fs::write(root.join("response-mode"), "red").unwrap();
        std::fs::write(
            &stub,
            r#"#!/bin/sh
cat >/dev/null
cat "${0%/*}/response-$(cat "${0%/*}/response-mode").json"
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
            sibling_rewrites: None,
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
        let first = run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(first.failed, 0, "{first:?}");
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_with_roots(
            root, day, &talent, &apps,
        )
        .unwrap();
        assert_eq!(
            coverage.state,
            solstone_core_system::daily_coverage::CoverageState::Current
        );
        let first_e = coverage.units[0].evidence_revision.clone();
        let output = root.join(format!("chronicle/{day}/talents/morning_briefing.json"));
        assert!(std::fs::read_to_string(&output).unwrap().contains("red"));
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(worker.outcomes.lock().unwrap().len(), 1);
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow changed to blue.",
        )
        .unwrap();
        std::fs::write(root.join("response-mode"), "blue").unwrap();
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
        let briefing = std::fs::read_to_string(&output).unwrap();
        assert!(briefing.contains("blue"), "{briefing}");
        assert!(!briefing.contains("red"), "{briefing}");
        // A conflicting replacement cannot borrow the previous accepted result's success.
        let identity =
            solstone_core_journal_io::DailyUnitIdentity::new(day, "morning_briefing", None);
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
        let entities = apps.join("entities/talent");
        std::fs::create_dir_all(&entities).unwrap();
        std::fs::write(entities.join("entity_suggest.md"),"{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":10,\"multi_facet\":true,\"output\":\"json\",\"schema\":\"entity_suggest-schema.json\",\"hook\":{\"pre\":\"entities:entity_suggest\",\"post\":\"entities:entity_suggest\"},\"load\":{\"transcripts\":true}\n}\nSuggest entity observations.").unwrap();
        std::fs::write(
            entities.join("entity_suggest-schema.json"),
            r#"{"type":"object"}"#,
        )
        .unwrap();
        let source = root.join("chronicle/20260910/default/090000_60/note_transcript.md");
        std::fs::create_dir_all(source.parent().unwrap().join("talents")).unwrap();
        std::fs::write(
            source.parent().unwrap().join("talents/facets.json"),
            r#"[{"facet":"work"}]"#,
        )
        .unwrap();
        std::fs::write(
            &source,
            "# Imported note\nThe meeting topic tomorrow is red.",
        )
        .unwrap();
        let stub = root.join("generate-stub.sh");
        for (mode, text, validation) in [
            ("parse", "not json", Value::Null),
            ("schema", r#"{"entities":[]}"#, json!({"valid":false})),
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
            sibling_rewrites: None,
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
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            day,
            "entities:entity_suggest",
            Some("work".to_owned()),
        );
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
        for owner_attempt in 1..=2 {
            let retry_context = permit_owner_daily(&context, &identity);
            run(&retry_context, &mut log, None, false, 1).unwrap();
            assert_eq!(
                worker.outcomes.lock().unwrap().len(),
                dispatched + owner_attempt
            );
            run(&context, &mut log, None, false, 1).unwrap();
            assert_eq!(
                worker.outcomes.lock().unwrap().len(),
                dispatched + owner_attempt
            );
        }
        let dispatched = worker.outcomes.lock().unwrap().len();
        std::fs::write(&source, "# Imported note\nA changed event.").unwrap();
        let retry_context = permit_changed_daily(&context, &identity);
        run(&retry_context, &mut log, None, false, 1).unwrap();
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
"type":"generate","max_output_tokens":1024,"schedule":"daily","priority":58,"multi_facet":true,"output":"json","hook":{"pre":"entities:entity_observer","post":"entities:entity_observer"},"load":{"transcripts":false,"percepts":false,"talents":false}
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
            sibling_rewrites: None,
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
        let retry_context = permit_changed_daily(&context, &identity);
        let result = run(&retry_context, &mut log, None, false, 1).unwrap();
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
        std::fs::create_dir_all(apps.join("entities/talent")).unwrap();
        std::fs::write(apps.join("entities/talent/entities_review.md"), "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":2,\"multi_facet\":true,\"output\":\"json\",\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"},\"load\":{\"transcripts\":false}\n}\nReview entities.").unwrap();
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            root.join("review-response.json"),
            model_response("{\"promotions\":[],\"merges\":[]}", Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(
            &stub,
            r#"#!/bin/sh
cat >/dev/null
cat "${0%/*}/review-response.json"
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
            sibling_rewrites: None,
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
    fn historical_days_share_the_current_maintenance_packet_and_failure_is_separate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config/journal.json"), r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#).unwrap();
        std::fs::write(talent.join("daily_schedule.md"), format!("{}\nChoose a daily processing time from $activity_spans.", serde_json::to_string_pretty(&json!({"type":"generate","max_output_tokens":1024,"schedule":"daily","priority":1,"output":"json","hook":{"pre":"daily_schedule","post":"daily_schedule"},"load":{"transcripts":false}})).unwrap())).unwrap();
        // Keep one real historical unit independently current when maintenance fails.
        std::fs::write(talent.join("morning_briefing.md"), format!("{}\nBRIEFING_PROOF: $briefing_metadata $activity_digest", serde_json::to_string_pretty(&json!({"type":"generate","max_output_tokens":1024,"schedule":"daily","priority":2,"output":"json","hook":{"pre":"morning_briefing"},"load":{"transcripts":false}})).unwrap())).unwrap();
        let today = solstone_core_system::daily_coverage::local_day(root, chrono::Utc::now());
        std::fs::create_dir_all(root.join("chronicle").join(&today).join("mic/090000_600"))
            .unwrap();
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            root.join("response.json"),
            model_response(r#"{"primary":"03:00"}"#, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("response-briefing.json"),
            model_response(r#"{"summary":"Prepared"}"#, Value::Null).to_string(),
        )
        .unwrap();
        std::fs::write(
            &stub,
            r#"#!/bin/sh
request=$(cat)
case "$request" in
 *BRIEFING_PROOF*) cat "${0%/*}/response-briefing.json";;
 *) cat "${0%/*}/response.json";;
esac
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
            sibling_rewrites: None,
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
                    "name":"daily_schedule", "day":day,"type":"generate","max_output_tokens":1024,"model":"test-model","provider":"test",
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
            &|| Ok(()),
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
                &mut Vec::new(),
                &solstone_core_generate::Cancellation::never(),
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
        assert!(
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
                &|| Ok(())
            )
            .is_err()
        );
        // A lower-level external replacement still fences the old token.
        let mut replacement = DailyUnitRecord::new(newer_identity.clone(), "current-window-E", "C");
        replacement.frozen_packet = Some(current_packet.clone());
        replacement.packet_digest = Some(
            solstone_core_talent_runtime::daily_prepare::packet_digest(&current_packet),
        );
        replacement.lock_token = Some("new-worker".into());
        replacement.use_id = Some("new-worker".into());
        replacement.attempt_day = Some(today.clone());
        save_daily_unit_record(root, &replacement).unwrap();
        std::fs::write(&release, b"resume").unwrap();
        let old_outcome = old.join().unwrap();
        let solstone_core_talent_runtime::RuntimeOutcome::StageFailed(error) = old_outcome else {
            panic!("old worker retained authority: {old_outcome:?}");
        };
        assert!(
            error.detail.contains("publication token was replaced"),
            "{error:?}"
        );
        let newer = solstone_core_talent_runtime::execute_request(
            json!({"name":"daily_schedule","day":"20200201","lock_token":"new-worker"})
                .as_object()
                .unwrap()
                .clone(),
            &paths,
            &context,
            &solstone_core_generate::OneShotClient::at_path(new_stub),
            &mut Vec::new(),
            &solstone_core_generate::Cancellation::never(),
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
            &|| Ok(()),
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
            &|| Ok(()),
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
            &|| Ok(()),
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
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nprompt",
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
                &|| Ok(()),
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
                &|| Ok(()),
            )
        });
        let first_err = first.join().unwrap().unwrap_err();
        let second_err = second.join().unwrap().unwrap_err();
        assert!(
            matches!(
                first_err,
                ReserveAttemptError::Unit(
                    solstone_core_journal_io::DailyUnitError::OwnerConflictExhausted
                        | solstone_core_journal_io::DailyUnitError::Lock(_)
                )
            ),
            "{first_err}"
        );
        assert!(
            matches!(
                second_err,
                ReserveAttemptError::Unit(
                    solstone_core_journal_io::DailyUnitError::OwnerConflictExhausted
                        | solstone_core_journal_io::DailyUnitError::Lock(_)
                )
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

    #[test]
    fn alias_claimed_conflict_exhausted_on_later_day() {
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
        record.attempt_day = Some("20260910".into());
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();
        let packet = Value::Object(Default::default());
        let res = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &packet,
            "20260915",
            "worker-alias",
            false,
            false,
            100,
            &|| Ok(()),
        );
        assert!(
            matches!(
                res,
                Err(ReserveAttemptError::Unit(
                    solstone_core_journal_io::DailyUnitError::OwnerConflictExhausted
                ))
            ),
            "alias_claimed with count 2 must remain exhausted on a later day: {res:?}"
        );
    }

    #[test]
    fn legacy_stale_record_without_attempt_day_renews_and_read_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            "20260910",
            "entities:entities_review",
            Some("work".into()),
        );
        let mut record = solstone_core_journal_io::DailyUnitRecord::new(identity.clone(), "E", "C");
        record.status = solstone_core_journal_io::DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        record.owner_conflict_kind = Some("merge_proposal_preparation".into());
        record.failure_count = 2;
        record.attempt_day = None;
        let early_ms = 1789000000000i64;
        record.updated_at_ms = early_ms;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();

        let read1 = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(read1.attempt_day, None);
        assert_eq!(read1.updated_at_ms, early_ms);

        let packet = Value::Object(Default::default());
        let res = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &packet,
            "20260911",
            "worker-renew",
            false,
            false,
            early_ms + 86400000,
            &|| Ok(()),
        );
        assert!(res.is_ok(), "legacy stale record must renew: {res:?}");
        let read2 = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(read2.failure_count, 0);
        assert_eq!(read2.attempt_day.as_deref(), Some("20260911"));
    }

    #[test]
    fn accepted_reuse_sentinel_and_variant_arms() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path();
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "hook".into(),
            serde_json::json!({"pre": "morning_briefing"}),
        );
        let evidence = solstone_core_indexer::daily_evidence::compute_daily_evidence(
            journal,
            "20260101",
            "morning_briefing",
            &metadata,
            "prompt",
            None,
            None,
        )
        .unwrap();

        // 1. FrozenBriefing case
        let identity_mb =
            solstone_core_journal_io::DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let mut rec_mb =
            solstone_core_journal_io::DailyUnitRecord::new(identity_mb.clone(), "E_old", "C_old");
        rec_mb.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: "E_old".into(),
            contract_digest: "C_old".into(),
            status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response": "resp", "output": "resp"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        assert_eq!(
            solstone_core_system::daily_coverage::accepted_reuse(
                &rec_mb,
                &evidence,
                at_midnight("20260105")
            ),
            Some(Some(
                solstone_core_system::daily_coverage::AcceptedReuse::FrozenBriefing
            ))
        );

        // Its morning is over at noon the next day: live until then, frozen
        // from then on.
        for (now, frozen) in [
            (at("20260102", 0, 0), false),
            (at("20260102", 11, 59), false),
            (at("20260102", 12, 0), true),
            (at("20260102", 23, 59), true),
        ] {
            assert_eq!(
                solstone_core_system::daily_coverage::accepted_reuse(&rec_mb, &evidence, now),
                frozen.then_some(Some(
                    solstone_core_system::daily_coverage::AcceptedReuse::FrozenBriefing
                )),
                "{now}"
            );
        }

        // When sentinel is present, returns None
        rec_mb.evidence_revision = solstone_core_journal_io::OWNER_REPROCESS_SENTINEL.into();
        rec_mb.contract_digest = solstone_core_journal_io::OWNER_REPROCESS_SENTINEL.into();
        assert_eq!(
            solstone_core_system::daily_coverage::accepted_reuse(
                &rec_mb,
                &evidence,
                at_midnight("20260105")
            ),
            None
        );

        // 2. EarlierContract case
        let old_contract = "old_contract_digest";
        let accepted_rev = evidence.revision_under(old_contract);
        let identity_ec =
            solstone_core_journal_io::DailyUnitIdentity::new("20260101", "schedule", None);
        let mut rec_ec = solstone_core_journal_io::DailyUnitRecord::new(
            identity_ec.clone(),
            &evidence.revision,
            &evidence.contract,
        );
        rec_ec.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: accepted_rev,
            contract_digest: old_contract.into(),
            status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response": "resp", "output": "resp"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        assert_eq!(
            solstone_core_system::daily_coverage::accepted_reuse(
                &rec_ec,
                &evidence,
                at_midnight("20260115")
            ),
            Some(Some(
                solstone_core_system::daily_coverage::AcceptedReuse::EarlierContract
            ))
        );

        // When sentinel is present, returns None
        rec_ec.evidence_revision = solstone_core_journal_io::OWNER_REPROCESS_SENTINEL.into();
        rec_ec.contract_digest = solstone_core_journal_io::OWNER_REPROCESS_SENTINEL.into();
        assert_eq!(
            solstone_core_system::daily_coverage::accepted_reuse(
                &rec_ec,
                &evidence,
                at_midnight("20260115")
            ),
            None
        );
    }

    #[test]
    fn queue_daily_dispatches_reset_record_and_skips_qualifying_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().to_path_buf();
        std::fs::create_dir_all(journal.join("config")).unwrap();
        std::fs::write(
            journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(journal.join("chronicle/20260101")).unwrap();
        let talent_root = journal.join("payload/talent");
        let apps_root = journal.join("payload/apps");
        std::fs::create_dir_all(&talent_root).unwrap();
        std::fs::create_dir_all(apps_root.join("entities/talent")).unwrap();
        std::fs::write(
            talent_root.join("morning_briefing.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":10,\"output\":\"markdown\",\"hook\":{\"pre\":\"morning_briefing\"}\n}\nprompt",
        )
        .unwrap();
        std::fs::write(
            apps_root.join("entities/talent/entities_review.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nprompt",
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
            &journal,
            "20260101".to_owned(),
            journal.join("chronicle/20260101"),
            1789400000000,
        )
        .unwrap()
        .with_talent_roots(talent_root, apps_root)
        .with_boundary(std::sync::Arc::new(MockCortex));

        let mb_identity =
            solstone_core_journal_io::DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let mut mb_record = solstone_core_journal_io::DailyUnitRecord::new(
            mb_identity.clone(),
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL,
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL,
        );
        mb_record.status = solstone_core_journal_io::DailyUnitStatus::Unfinished;
        mb_record.failure_count = 0;
        mb_record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: "E_hist".into(),
            contract_digest: "C_hist".into(),
            status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(serde_json::json!({"response": "resp", "output": "resp"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        solstone_core_journal_io::save_daily_unit_record(&journal, &mb_record).unwrap();

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

        let review_config = configs
            .iter()
            .find(|c| c.key == "entities:entities_review")
            .unwrap();
        let review_identity = solstone_core_journal_io::DailyUnitIdentity::new(
            "20260101",
            "entities:entities_review",
            Some("work".to_owned()),
        );
        let review_evidence = crate::snapshot::compute_daily_evidence(
            &journal,
            "20260101",
            review_config,
            Some("work"),
        )
        .unwrap();
        let old_contract = "old_contract_digest";
        let accepted_rev = review_evidence.revision_under(old_contract);
        let mut review_record = solstone_core_journal_io::DailyUnitRecord::new(
            review_identity.clone(),
            &review_evidence.revision,
            &review_evidence.contract,
        );
        review_record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: accepted_rev,
            contract_digest: old_contract.into(),
            status: solstone_core_journal_io::DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("b".repeat(64)),
            generated_result: Some(serde_json::json!({"response": "resp", "output": "resp"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        solstone_core_journal_io::save_daily_unit_record(&journal, &review_record).unwrap();

        let mb_config = configs
            .iter()
            .find(|c| c.key == "morning_briefing")
            .unwrap();
        let tokio_rt = tokio::runtime::Runtime::new().unwrap();
        let mut pending = Vec::new();
        let mut result = ModeResult::default();
        let mut log = crate::run_log::RunLogWriter::open(&journal, "20260101", "daily");

        queue_daily(
            &context,
            &mut log,
            &tokio_rt,
            mb_config,
            None,
            false,
            &mut pending,
            &mut result,
        )
        .unwrap();

        assert_eq!(pending.len(), 1, "reset record must be dispatched");
        let reserved_rec = solstone_core_journal_io::load_daily_unit_record(&journal, &mb_identity)
            .unwrap()
            .unwrap();
        assert_ne!(
            reserved_rec.evidence_revision,
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
        );
        assert_ne!(
            reserved_rec.contract_digest,
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
        );

        // Queue sibling qualifying with EarlierContract
        queue_daily(
            &context,
            &mut log,
            &tokio_rt,
            review_config,
            Some("work"),
            false,
            &mut pending,
            &mut result,
        )
        .unwrap();
        assert_eq!(
            pending.len(),
            1,
            "qualifying sibling must be skipped without dispatch"
        );
        let review_loaded =
            solstone_core_journal_io::load_daily_unit_record(&journal, &review_identity)
                .unwrap()
                .unwrap();
        assert_eq!(
            review_loaded.evidence_revision,
            review_record.evidence_revision
        );
        assert_eq!(review_loaded.contract_digest, review_record.contract_digest);

        // Subsequent ordinary failure has the real digests, so accepted_reuse is FrozenBriefing again
        let mut fail_rec = reserved_rec.clone();
        fail_rec.status = solstone_core_journal_io::DailyUnitStatus::Failed;
        fail_rec.reason_code = Some("talent_stage_failed".into());
        solstone_core_journal_io::save_daily_unit_record(&journal, &fail_rec).unwrap();

        let evidence =
            crate::snapshot::compute_daily_evidence(&journal, "20260101", mb_config, None).unwrap();
        let reuse = solstone_core_system::daily_coverage::accepted_reuse(
            &fail_rec,
            &evidence,
            at_midnight("20260105"),
        );
        assert_eq!(
            reuse,
            Some(Some(
                solstone_core_system::daily_coverage::AcceptedReuse::FrozenBriefing
            ))
        );
    }

    /// An entity review that stopped on a conflict after its earlier changes
    /// committed, once its automatic retry is spent.
    fn stopped_review_record(
        identity: &DailyUnitIdentity,
        evidence: &str,
        contract: &str,
        receipts: &[(&str, &str)],
    ) -> DailyUnitRecord {
        let mut record = DailyUnitRecord::new(identity.clone(), evidence, contract);
        record.status = DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        record.owner_conflict_kind = Some("alias_claimed".into());
        record.error_detail =
            Some("conflict: promotion alias was claimed after preparation".into());
        record.failure_count = 2;
        record.attempt_day = Some(identity.day.clone());
        record.lock_token = Some("stopped".into());
        let stale = json!({"stale":"packet"});
        record.packet_digest = Some(solstone_core_talent_runtime::daily_prepare::packet_digest(
            &stale,
        ));
        record.frozen_packet = Some(stale);
        record.generated_result = Some(json!({"response":"stale"}));
        record.action_plan = Some(json!({"actions":[], "no_output":false}));
        record.receipts = receipts
            .iter()
            .map(|(action, state)| {
                json!({"kind":"owner_action","action_id":action,"token":"stopped","state":state})
            })
            .collect();
        record
    }

    #[test]
    fn an_owner_retry_replaces_a_review_stopped_before_its_conflicting_write() {
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        // The same inputs, and inputs that changed since the stop.
        for evidence in ["E", "E2"] {
            let root = tempfile::tempdir().unwrap();
            let mut stopped = stopped_review_record(
                &identity,
                "E",
                "C",
                &[("0:a", "committed"), ("1:b", "committed")],
            );
            stopped.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
                evidence_revision: "earlier".into(),
                contract_digest: "C".into(),
                status: DailyUnitStatus::Committed,
                packet_digest: None,
                generated_result: None,
                receipts: vec![],
                committed_at_ms: 7,
            });
            save_daily_unit_record(root.path(), &stopped).unwrap();
            let fresh = json!({"fresh":"packet"});
            let reserve = |retry| {
                reserve_daily_attempt(
                    root.path(),
                    &identity,
                    evidence,
                    "C",
                    &fresh,
                    "20260912",
                    "owner-retry",
                    false,
                    retry,
                    9,
                    &|| Ok(()),
                )
            };
            // Without the owner's request the stop holds.
            assert!(reserve(false).is_err());
            assert_eq!(
                load_daily_unit_record(root.path(), &identity)
                    .unwrap()
                    .unwrap(),
                stopped
            );
            reserve(true).unwrap();
            let reserved = load_daily_unit_record(root.path(), &identity)
                .unwrap()
                .unwrap();
            assert_eq!(reserved.status, DailyUnitStatus::Unfinished);
            assert_eq!(reserved.evidence_revision, evidence);
            assert_eq!(reserved.frozen_packet.as_ref(), Some(&fresh));
            assert!(reserved.generated_result.is_none());
            assert!(reserved.action_plan.is_none());
            assert!(reserved.receipts.is_empty());
            assert_eq!(reserved.failure_count, 0);
            assert_eq!(reserved.lock_token.as_deref(), Some("owner-retry"));
            assert_eq!(reserved.attempt_day.as_deref(), Some("20260912"));
            assert_eq!(reserved.accepted, stopped.accepted);
        }
    }

    #[test]
    fn an_owner_retry_never_replaces_a_review_that_may_have_started_a_write() {
        let root = tempfile::tempdir().unwrap();
        let identity =
            DailyUnitIdentity::new("20260910", "entities:entities_review", Some("work".into()));
        let stopped = stopped_review_record(
            &identity,
            "E",
            "C",
            &[("0:a", "committed"), ("1:b", "started")],
        );
        save_daily_unit_record(root.path(), &stopped).unwrap();
        let error = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &json!({"fresh":"packet"}),
            "20260912",
            "owner-retry",
            false,
            true,
            9,
            &|| Ok(()),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ambiguous uncommitted owner action"),
            "{error}"
        );
        assert_eq!(
            load_daily_unit_record(root.path(), &identity)
                .unwrap()
                .unwrap(),
            stopped
        );
    }

    #[cfg(all(test, feature = "full-tests", unix))]
    #[test]
    fn an_owner_retry_of_a_stopped_review_prepares_again_and_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = "20260910";
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"},"providers":{"active":{"provider":"openai","model":"test-model"}}}"#,
        )
        .unwrap();
        solstone_core_facets::create_facet(root, "work", "Work", "", "", "", None).unwrap();
        solstone_core_facets::ensure_daily_facet_id(root, "work").unwrap();
        let talent = root.join("payload/talent");
        let apps = root.join("payload/apps");
        std::fs::create_dir_all(&talent).unwrap();
        std::fs::create_dir_all(apps.join("entities/talent")).unwrap();
        std::fs::write(
            apps.join("entities/talent/entities_review.md"),
            "{\n\"type\":\"generate\",\"max_output_tokens\":1024,\"schedule\":\"daily\",\"priority\":56,\"output\":\"json\",\"multi_facet\":true,\"hook\":{\"pre\":\"entities:entities_review\",\"post\":\"entities:entities_review\"}\n}\nprompt",
        )
        .unwrap();
        let people = ["Ali", "Alice Smith"].map(|name| solstone_core_facets::DetectedEntityInput {
            entity_type: "Person".to_owned(),
            name: name.to_owned(),
            description: "Collaborator.".to_owned(),
        });
        for detected_day in ["20260908", "20260909"] {
            solstone_core_facets::upsert_detection_segment(
                root,
                "work",
                detected_day,
                "090000_300",
                &people,
            )
            .unwrap();
        }
        // The stopped attempt committed "Ali" before Alice's alias conflicted.
        solstone_core_facets::attach_or_reactivate_entity(
            root,
            "work",
            "Person",
            "Ali",
            "Designer.",
        )
        .unwrap();
        let review = json!({"promotions":[
            {"name":"Ali", "description":"Designer.", "promote":true, "aliases":[]},
            {"name":"Alice Smith", "description":"Engineer.", "promote":true,
                "aliases":["Ali", "Ally"]},
        ], "merges":[]});
        let stub = root.join("generate-stub.sh");
        std::fs::write(
            root.join("response.json"),
            model_response(&review.to_string(), Value::Null).to_string(),
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
            sibling_rewrites: None,
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
        let config = load_talent_configs(
            &talent,
            &apps,
            None,
            TalentFilter {
                r#type: None,
                schedule: Some("daily"),
                include_disabled: false,
            },
        )
        .unwrap()
        .into_iter()
        .find(|config| config.key == "entities:entities_review")
        .unwrap();
        let identity = DailyUnitIdentity::new(day, "entities:entities_review", Some("work".into()));
        let (evidence, contract) = crate::snapshot::compute_daily_evidence_revision(
            root,
            day,
            &config,
            Some("work"),
            None,
        )
        .unwrap();
        let runtime = runtime().unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        let mut queue = |context: &ThinkContext| {
            let mut pending = Vec::new();
            queue_daily(
                context,
                &mut log,
                &runtime,
                &config,
                Some("work"),
                false,
                &mut pending,
                &mut ModeResult::default(),
            )
            .unwrap();
            pending.len()
        };

        // An unconfirmed write keeps its guard even when the owner asks.
        let started = stopped_review_record(
            &identity,
            &evidence,
            &contract,
            &[("0:a", "committed"), ("1:b", "started")],
        );
        save_daily_unit_record(root, &started).unwrap();
        assert_eq!(queue(&permit_owner_daily(&context, &identity)), 0);
        assert_eq!(
            load_daily_unit_record(root, &identity).unwrap().unwrap(),
            started
        );

        let stopped = stopped_review_record(
            &identity,
            &evidence,
            &contract,
            &[("0:a", "committed"), ("1:b", "committed")],
        );
        save_daily_unit_record(root, &stopped).unwrap();
        // An ordinary run leaves the stop where it is.
        assert_eq!(queue(&context), 0);
        assert!(worker.outcomes.lock().unwrap().is_empty());

        assert_eq!(queue(&permit_owner_daily(&context, &identity)), 1);
        let finished = load_daily_unit_record(root, &identity).unwrap().unwrap();
        assert_eq!(finished.status, DailyUnitStatus::Committed);
        assert_ne!(finished.frozen_packet, stopped.frozen_packet);
        assert!(!finished.has_uncommitted_started_receipt());
        assert!(
            finished
                .receipts
                .iter()
                .filter(|receipt| receipt["kind"] == "owner_action")
                .all(|receipt| receipt["state"] == "committed")
        );
        let attached =
            solstone_core_facets::list_scoped_facet_entities(root, "work", false, false).unwrap();
        let alice = attached
            .iter()
            .find(|entity| entity.entity_id == "alice_smith")
            .expect("the new preparation promotes Alice");
        assert_eq!(alice.identity["aka"], json!(["Ally"]));
        assert!(attached.iter().any(|entity| entity.entity_id == "ali"));
        let unit = solstone_core_system::work_retry::Unit::Daily {
            day: day.into(),
            talent: identity.name.clone(),
            facet: identity.facet.clone(),
        };
        assert!(
            !solstone_core_system::work_retry::read(root, &unit.id().unwrap())
                .unwrap()
                .unwrap()
                .held
        );
    }

    struct ClearDailyEvidenceHook;
    impl Drop for ClearDailyEvidenceHook {
        fn drop(&mut self) {
            set_before_daily_evidence(None);
        }
    }

    fn location_config(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Denver"},"daily_inputs":{"location":{"location-words-1":"20260101"}}}"#,
        )
        .unwrap();
    }

    fn stored_cutoff(root: &std::path::Path) -> String {
        let config = solstone_core_journal_config::read_journal_config(root)
            .unwrap()
            .config
            .unwrap();
        solstone_core_journal_config::daily_input_since(&config, "location", "location-words-1")
            .unwrap()
            .unwrap()
            .format("%Y%m%d")
            .to_string()
    }

    fn briefing_packet(root: &std::path::Path) -> Value {
        let prepared = solstone_core_talent_runtime::PreparedTalent {
            name: "morning_briefing".to_owned(),
            config: json!({
                "day":"20260101",
                "type":"generate",
                "max_output_tokens":1024,
                "prompt":"ordinary briefing",
                "skip_reason":"no_input",
                "hook":{"pre":"morning_briefing"}
            })
            .as_object()
            .unwrap()
            .clone(),
        };
        solstone_core_talent_runtime::daily_prepare::freeze(
            prepared,
            &solstone_core_talent_runtime::ExecutionContext {
                journal: root.to_path_buf(),
            },
        )
        .unwrap()
    }

    fn accepted_briefing(
        root: &std::path::Path,
        packet: Value,
        failure_count: u32,
    ) -> DailyUnitRecord {
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let mut record = DailyUnitRecord::new(identity, "E", "C");
        record.status = DailyUnitStatus::Unfinished;
        record.failure_count = failure_count;
        record.attempts = 2;
        record.frozen_packet = Some(packet.clone());
        record.packet_digest = Some(solstone_core_talent_runtime::daily_prepare::packet_digest(
            &packet,
        ));
        record.generated_result = Some(json!({"response":"kept","output":"kept"}));
        record.accepted = Some(solstone_core_journal_io::AcceptedDailyResult {
            evidence_revision: "E".to_owned(),
            contract_digest: "C".to_owned(),
            status: DailyUnitStatus::CommittedNoOutput,
            packet_digest: Some("a".repeat(64)),
            generated_result: Some(json!({"response":"[]","output":"[]"})),
            receipts: Vec::new(),
            committed_at_ms: 1,
        });
        save_daily_unit_record(root, &record).unwrap();
        record
    }

    fn reserve_briefing(root: &std::path::Path, packet: &Value) -> Result<(), ReserveAttemptError> {
        reserve_daily_attempt(
            root,
            &DailyUnitIdentity::new("20260101", "morning_briefing", None),
            "E",
            "C",
            packet,
            "20260102",
            "worker-token",
            false,
            false,
            9,
            &|| Ok(()),
        )
    }

    #[test]
    fn daily_run_stores_the_cutoff_before_evidence() {
        let _clear = ClearDailyEvidenceHook;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"America/Denver"}}"#,
        )
        .unwrap();
        let day = "20261012";
        std::fs::create_dir_all(root.join("chronicle").join(day)).unwrap();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let calls_hook = std::rc::Rc::clone(&calls);
        set_before_daily_evidence(Some(Box::new(move |context| {
            calls_hook.set(calls_hook.get() + 1);
            let config = solstone_core_journal_config::read_journal_config(&context.journal)
                .unwrap()
                .config
                .unwrap();
            assert!(
                solstone_core_journal_config::daily_input_since(
                    &config,
                    "location",
                    "location-words-1"
                )
                .unwrap()
                .is_some()
            );
            Err("stop".into())
        })));
        let before = chrono::Utc::now();
        let context = ThinkContext::new(
            root,
            day.to_owned(),
            root.join("chronicle").join(day),
            1_700_000_000_000,
        )
        .unwrap();
        let mut log = RunLogWriter::open(root, day, "daily");
        run(&context, &mut log, None, false, 1).unwrap();
        let after = chrono::Utc::now();
        assert!(calls.get() >= 1);
        let zone = solstone_core_journal_config::owner_zone(root);
        let early = solstone_core_system::daily_coverage::daily_input_since_day(before, &zone, 7);
        let late = solstone_core_system::daily_coverage::daily_input_since_day(after, &zone, 7);
        let stored = stored_cutoff(root);
        assert!(stored == early || stored == late, "{stored} {early} {late}");
        let mut log = RunLogWriter::open(root, day, "daily");
        run(&context, &mut log, None, false, 1).unwrap();
        assert_eq!(stored_cutoff(root), stored);
    }

    #[test]
    fn uncommitted_started_receipt_is_not_replaced() {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let packet = briefing_packet(root.path());
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let mut record = accepted_briefing(root.path(), packet, 4);
        record.identity = identity.clone();
        record.receipts.push(json!({
            "kind": "owner_action",
            "action_id": "0:action_1",
            "token": "old_token",
            "state": "started"
        }));
        let stale = {
            let mut caller = record.frozen_packet.clone().unwrap();
            caller["prepared"]["config"]["_daily_location_fingerprint"] = json!("ab".repeat(32));
            caller
        };
        record.frozen_packet = Some(stale);
        save_daily_unit_record(root.path(), &record).unwrap();
        let before = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        let caller = briefing_packet(root.path());
        assert!(reserve_briefing(root.path(), &caller).is_err());
        let stored = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(stored, before);
        assert!(stored.has_uncommitted_started_receipt());
        assert_eq!(stored.failure_count, 4);
        assert!(stored.frozen_packet.is_some());
    }

    #[test]
    fn missing_fingerprint_on_an_eligible_recipient_is_stale() {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let mut packet = briefing_packet(root.path());
        packet["prepared"]["config"]
            .as_object_mut()
            .unwrap()
            .remove("_daily_location_fingerprint");
        assert_eq!(
            solstone_core_talent_runtime::daily_location::daily_location_freshness(
                root.path(),
                "morning_briefing",
                "20260101",
                Some(&packet),
            ),
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Stale)
        );
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let seeded = accepted_briefing(root.path(), packet, 4);
        let caller = json!({"version":1,"prepared":{"config":{"_daily_location_fingerprint":"00".repeat(32)}}});
        assert!(reserve_briefing(root.path(), &caller).is_err());
        let stored = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert!(stored.frozen_packet.is_none());
        assert!(stored.packet_digest.is_none());
        assert!(stored.generated_result.is_none());
        assert!(stored.action_plan.is_none());
        assert_eq!(stored.failure_count, 4);
        assert_eq!(stored.accepted, seeded.accepted);
        assert_ne!(stored.status, DailyUnitStatus::Capped);
        assert_eq!(stored.status, DailyUnitStatus::Unfinished);
    }

    #[test]
    fn receipt_free_matching_fingerprint_resumes_stored_bytes() {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let packet = briefing_packet(root.path());
        assert_eq!(
            solstone_core_talent_runtime::daily_location::daily_location_freshness(
                root.path(),
                "morning_briefing",
                "20260101",
                Some(&packet),
            ),
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Reuse)
        );
        let seeded = accepted_briefing(root.path(), packet.clone(), 4);
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let caller = json!({"version":1,"prepared":{"name":"morning_briefing","config":{"prompt":"other","_daily_location_fingerprint":"cd".repeat(32)}}});
        reserve_briefing(root.path(), &caller).unwrap();
        let stored = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(stored.frozen_packet, Some(packet));
        assert_eq!(stored.generated_result, seeded.generated_result);
        assert_eq!(stored.accepted, seeded.accepted);
        assert_eq!(stored.failure_count, 4);
        assert_eq!(stored.status, DailyUnitStatus::Unfinished);
        assert_eq!(stored.attempts, 3);
        assert_ne!(stored.status, DailyUnitStatus::Capped);
    }

    #[test]
    fn fingerprint_change_before_dispatch_during_generation_and_before_saved_response_refuses_without_spending_failure_count()
     {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let packet = briefing_packet(root.path());
        let location = root
            .path()
            .join("chronicle/20260101/phone/120000_1/location.jsonl");
        std::fs::create_dir_all(location.parent().unwrap()).unwrap();
        std::fs::write(&location, b"changed\n").unwrap();
        let seeded = accepted_briefing(root.path(), packet.clone(), 4);
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        assert_eq!(
            solstone_core_talent_runtime::daily_location::daily_location_freshness(
                root.path(),
                "morning_briefing",
                "20260101",
                seeded.frozen_packet.as_ref(),
            ),
            Ok(solstone_core_talent_runtime::daily_location::DailyLocationFreshness::Stale)
        );
        assert!(reserve_briefing(root.path(), &packet).is_err());
        let cleared = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert!(cleared.frozen_packet.is_none());
        assert!(cleared.packet_digest.is_none());
        assert!(cleared.generated_result.is_none());
        assert!(cleared.action_plan.is_none());
        assert_eq!(cleared.failure_count, 4);
        assert_eq!(cleared.accepted, seeded.accepted);
        assert_eq!(cleared.status, DailyUnitStatus::Unfinished);
        std::fs::remove_file(&location).unwrap();
        reserve_briefing(root.path(), &packet).unwrap();
        let prepared = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(prepared.frozen_packet, Some(packet));
        assert_eq!(prepared.failure_count, 4);
        assert_eq!(prepared.status, DailyUnitStatus::Unfinished);
        assert_ne!(prepared.status, DailyUnitStatus::Capped);
        assert!(prepared.packet_digest.is_some());
    }

    #[test]
    fn stale_location_stop_does_not_clear_the_record() {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let packet = briefing_packet(root.path());
        let location = root
            .path()
            .join("chronicle/20260101/phone/120000_1/location.jsonl");
        std::fs::create_dir_all(location.parent().unwrap()).unwrap();
        std::fs::write(&location, b"changed\n").unwrap();
        let seeded = accepted_briefing(root.path(), packet.clone(), 4);
        let identity = DailyUnitIdentity::new("20260101", "morning_briefing", None);
        let error = reserve_daily_attempt(
            root.path(),
            &identity,
            "E",
            "C",
            &packet,
            "20260102",
            "worker-token",
            false,
            false,
            9,
            &|| Err("stopped".into()),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ReserveAttemptError::Stopped(message) if message == "stopped"
        ));
        let stored = load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(stored.frozen_packet, seeded.frozen_packet);
        assert_eq!(stored.packet_digest, seeded.packet_digest);
        assert_eq!(stored.generated_result, seeded.generated_result);
        assert_eq!(stored.action_plan, seeded.action_plan);
        assert_eq!(stored.failure_count, seeded.failure_count);
        assert_eq!(stored.status, seeded.status);
        assert_eq!(stored.attempts, seeded.attempts);
    }

    #[test]
    fn partially_committed_reservation_keeps_the_exact_attempt_when_location_is_unreadable() {
        let root = tempfile::tempdir().unwrap();
        location_config(root.path());
        let packet = briefing_packet(root.path());
        let mut before = accepted_briefing(root.path(), packet.clone(), 4);
        before
            .receipts
            .push(json!({"kind":"owner_action","action_id":"first","state":"committed"}));
        before.action_plan = Some(json!({"actions":["retained-plan"]}));
        save_daily_unit_record(root.path(), &before).unwrap();
        let accuracy = root.path().join("cache/location/accuracy");
        std::fs::create_dir_all(&accuracy).unwrap();
        std::fs::write(accuracy.join("installed.json"), b"{").unwrap();
        reserve_briefing(root.path(), &packet).unwrap();
        let after = load_daily_unit_record(root.path(), &before.identity)
            .unwrap()
            .unwrap();
        assert_eq!(after.frozen_packet, before.frozen_packet);
        assert_eq!(after.generated_result, before.generated_result);
        assert_eq!(after.action_plan, before.action_plan);
        assert_eq!(after.receipts, before.receipts);
        assert_eq!(after.failure_count, before.failure_count);
    }
}
