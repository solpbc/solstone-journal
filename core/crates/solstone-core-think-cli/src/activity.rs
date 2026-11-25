// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::activity_work::ActivityWork;
use serde_json::{Map, Value};
use solstone_core_cortex_client::{
    UseEndState, UseFileStatus, get_use_end_state, read_use_events, use_file_status,
};
use solstone_core_facets::get_activity_record;
use solstone_core_journal_io::FileLock;
use solstone_core_talent_config::{TalentConfig, TalentFilter, get_output_name};
use solstone_core_talent_runtime::activity_contract;

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DEFAULT_THINK_TIMEOUT, DrainOutcome, ModeResult, PendingUse, drain_with_deadline_observed,
    grouped, merge_mode_result, runtime, use_log_failure_detail,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

/// Record an ended activity's talent work before any of it runs, so a caller
/// that stops partway leaves it to the retry drain rather than losing it. The
/// returned claim keeps the drain off the work until the caller passes it to
/// [`run_held`]. A record that selects no talent records nothing.
pub(crate) fn enqueue(
    context: &ThinkContext,
    activity_id: &str,
    facet: &str,
    refresh: bool,
) -> Result<Option<FileLock>, String> {
    let Some(record) = get_activity_record(&context.journal, facet, &context.day, activity_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Some((configs, input_hash)) =
        selected_work_with_refresh(context, facet, activity_id, &record, refresh)?
    else {
        return Ok(None);
    };
    if configs.is_empty() {
        return Ok(None);
    }
    ActivityWork::begin(
        context,
        facet,
        activity_id,
        input_hash,
        configs.iter().map(|c| c.key.clone()).collect(),
        refresh,
    )
    .map(|work| Some(work.into_claim()))
}

/// How seeding an owner-asked activity went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seeded {
    /// Its work is recorded as a full refresh.
    Recorded,
    /// Another run holds it or its earlier work is still running; ask again later.
    Busy,
    /// It cannot run again, for this reason.
    Dropped(&'static str),
}

/// Record an owner-asked activity's work as a full refresh through
/// [`enqueue`], so every selected talent runs again on its first dispatch and
/// on every retry of that work.
pub(crate) fn seed_refresh(
    context: &ThinkContext,
    activity_id: &str,
    facet: &str,
) -> Result<Seeded, String> {
    let identity = crate::activity_work::ActivityRetry {
        day: context.day.clone(),
        facet: facet.to_owned(),
        activity: activity_id.to_owned(),
    };
    if !crate::activity_work::record_replaceable(&context.journal, &identity)? {
        return Ok(Seeded::Dropped("work_record_unreadable"));
    }
    let record = match get_activity_record(&context.journal, facet, &context.day, activity_id) {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(Seeded::Dropped("activity_missing")),
        Err(_) => return Ok(Seeded::Dropped("inputs_unreadable")),
    };
    if selected_configs_with_refresh(context, facet, activity_id, &record, true)?
        .is_none_or(|configs| configs.is_empty())
    {
        return Ok(Seeded::Dropped("no_activity_talents"));
    }
    if crate::segment::compute_activity_input_hash(context, &context.day, &record).is_none() {
        return Ok(Seeded::Dropped("inputs_unreadable"));
    }
    match enqueue(context, activity_id, facet, true) {
        Ok(Some(_claim)) => Ok(Seeded::Recorded),
        Ok(None) => Ok(Seeded::Dropped("activity_missing")),
        Err(error)
            if error == crate::activity_work::CLAIMED
                || error == crate::activity_work::STILL_RUNNING =>
        {
            Ok(Seeded::Busy)
        }
        Err(error) => Err(error),
    }
}

/// The talents a stored activity record selects, and the fingerprint of the
/// input they read. A synthetic record, or one with no input span, selects
/// none and succeeds without running anything.
fn selected_work_with_refresh(
    context: &ThinkContext,
    facet: &str,
    activity_id: &str,
    record: &solstone_core_facets::ActivityRecord,
    allow_historical_synthesis: bool,
) -> Result<Option<(Vec<TalentConfig>, String)>, String> {
    let Some(configs) = selected_configs_with_refresh(
        context,
        facet,
        activity_id,
        record,
        allow_historical_synthesis,
    )?
    else {
        return Ok(None);
    };
    let input_hash = crate::segment::compute_activity_input_hash(context, &context.day, record)
        .ok_or_else(|| {
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.activity.fingerprint.failed",
                &[],
            )
        })?;
    Ok(Some((configs, input_hash)))
}

fn owner_asked_activity(journal: &std::path::Path, day: &str, facet: &str, id: &str) -> bool {
    if facet.is_empty() || id.is_empty() {
        return false;
    }
    let target = solstone_core_system::owner_ask::AskedActivity {
        facet: facet.to_owned(),
        activity: id.to_owned(),
    };
    solstone_core_system::owner_ask::read(journal).is_ok_and(|asks| {
        asks.values().any(|ask| {
            ask.day == day && (ask.to_seed.contains(&target) || ask.seeded.contains(&target))
        })
    })
}

fn historical_backfill_merits_synthesis(
    journal: &std::path::Path,
    day: &str,
    record: &solstone_core_facets::ActivityRecord,
) -> bool {
    let Some(segments) = record.get("segments").and_then(Value::as_array) else {
        return false;
    };
    let stream = record.get("stream").and_then(Value::as_str);
    let mut has_spoken_audio =
        stream.is_some_and(|s| s == "import.audio" || s.starts_with("import.audio."));
    let mut total_duration_secs = 0_u64;
    let mut turn_count = 0_usize;
    let mut exchange_like = false;
    let mut char_count = 0_usize;

    for seg in segments.iter().filter_map(Value::as_str) {
        if let Some((_, dur)) = seg.rsplit_once('_')
            && let Ok(secs) = dur.parse::<u64>()
        {
            total_duration_secs = total_duration_secs.saturating_add(secs);
        }
        let Some(seg_dir) =
            solstone_core_system_health::find_segment_dir(journal, day, seg, stream)
        else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&seg_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(fname) = name.to_str() else {
                continue;
            };
            if fname == "audio.jsonl"
                || fname.ends_with("_audio.jsonl")
                || fname == "imported_audio.jsonl"
                || fname.starts_with("audio.")
            {
                has_spoken_audio = true;
            }
            if fname == "conversation_transcript.jsonl" {
                exchange_like = true;
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    for line in text.lines().filter(|l| !l.trim().is_empty()) {
                        if let Ok(val) = serde_json::from_str::<Value>(line) {
                            if val.get("schema").is_some() {
                                continue;
                            }
                            turn_count += 1;
                            if let Some(body) = val
                                .get("content")
                                .or_else(|| val.get("text"))
                                .and_then(Value::as_str)
                            {
                                char_count += body.chars().count();
                            }
                        }
                    }
                }
            } else if (fname.ends_with("_transcript.md") || fname == "transcript.md")
                && let Ok(text) = std::fs::read_to_string(entry.path())
            {
                char_count += text.chars().count();
                let msg_turns = text
                    .lines()
                    .filter(|line| {
                        let trimmed = line.trim_start();
                        trimmed.starts_with("From:")
                            || trimmed.starts_with("### Message ")
                            || trimmed.starts_with("### Turn ")
                            || (trimmed.starts_with('[') && trimmed.contains("] "))
                    })
                    .count();
                if msg_turns > 0 {
                    exchange_like = true;
                    turn_count += msg_turns;
                }
            }
        }
    }

    if has_spoken_audio && (segments.len() >= 2 || total_duration_secs >= 180) {
        return true;
    }
    if turn_count >= 3 {
        return true;
    }
    if (exchange_like || has_spoken_audio) && char_count >= 1_500 {
        return true;
    }
    false
}

fn selected_configs_with_refresh(
    context: &ThinkContext,
    facet: &str,
    activity_id: &str,
    record: &solstone_core_facets::ActivityRecord,
    allow_historical_synthesis: bool,
) -> Result<Option<Vec<TalentConfig>>, String> {
    if activity_contract::is_synthetic(record) || !activity_contract::has_nonempty_span(record) {
        return Ok(None);
    }
    // A Strava activity is a reading of body data: it selects no talent.
    if crate::activity_work::is_body_data_activity(record) {
        return Ok(Some(Vec::new()));
    }
    let now = jiff::Timestamp::from_millisecond(context.now_ms)
        .unwrap_or_else(|_| jiff::Timestamp::now());
    let today = solstone_core_system::daily_coverage::local_day(&context.journal, now);
    let is_historical_backfill = context.day < today
        && record.get("source").and_then(Value::as_str) != Some("user")
        && record
            .get("stream")
            .and_then(Value::as_str)
            .is_some_and(|s| solstone_core_segment::is_backfill_stream(&context.journal, s));
    if is_historical_backfill
        && !allow_historical_synthesis
        && !owner_asked_activity(&context.journal, &context.day, facet, activity_id)
        && !historical_backfill_merits_synthesis(&context.journal, &context.day, record)
    {
        return Ok(Some(Vec::new()));
    }
    let kind = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let configs = solstone_core_talent_config::load_talent_configs_in(
        context.locale(),
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
    Ok(Some(configs))
}

fn selected_output_held(
    configs: &[TalentConfig],
    record: &solstone_core_facets::ActivityRecord,
) -> bool {
    configs.iter().any(|config| {
        let hook = config
            .metadata
            .get("hook")
            .and_then(|hook| hook.get("post"))
            .and_then(Value::as_str);
        let fields: &[&str] = match hook {
            Some("story") => &["story", "relations"],
            Some("participation") => &[
                "participation",
                "participation_confidence",
                "participation_partial_input",
            ],
            _ => &[],
        };
        if fields.is_empty() {
            record
                .keys()
                .any(|field| solstone_core_facets::activity_field_held(record, field))
        } else {
            fields
                .iter()
                .any(|field| solstone_core_facets::activity_field_held(record, field))
        }
    })
}

/// A stale completed receipt may start fresh selected work only after the
/// caller took the normal activity claim and proved no unfinished record exists.
pub(crate) fn reconcile_retained(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity: &str,
    facet: &str,
    expected: &str,
    max_concurrency: i64,
    held: FileLock,
) -> Result<ModeResult, String> {
    let record = crate::activity_work::retained_record(context, &context.day, facet, activity)?;
    // Retained provenance already exists for this activity; reconciling a stale
    // receipt after a rebuild must run its owning talents rather than letting
    // the initial-backfill skip reject the seal.
    let Some(configs) = selected_configs_with_refresh(context, facet, activity, &record, true)?
    else {
        return Err("retained activity has no eligible work".into());
    };
    if configs.is_empty() {
        return Err("retained activity has no eligible work".into());
    }
    if selected_output_held(&configs, &record) {
        return Err("retained activity output is held by owner".into());
    }
    let hash = crate::activity_work::available_input(context, &context.day, &record)?;
    if hash != expected {
        return Err("retained activity input changed during admission".into());
    }
    run_selected(
        context,
        log,
        activity,
        facet,
        false,
        false,
        max_concurrency,
        Some(held),
        record,
        configs,
        hash,
        0,
    )
}

/// Run an activity record's selected talents in priority batches, each with
/// the fixed 610-second deadline, resuming any work an earlier run recorded.
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity_id: &str,
    facet: &str,
    refresh: bool,
    reactivate: bool,
    max_concurrency: i64,
) -> Result<ModeResult, String> {
    run_held(
        context,
        log,
        activity_id,
        facet,
        refresh,
        reactivate,
        max_concurrency,
        None,
    )
    .map_err(|error| {
        if error == crate::activity_work::STILL_RUNNING {
            solstone_core_cli_copy::render_for(
                context.locale(),
                "journal.cli.think.activity.work.running",
                &[],
            )
        } else {
            error
        }
    })
}

/// [`run`] with the claim [`enqueue`] took for this activity, if any.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_held(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity_id: &str,
    facet: &str,
    refresh: bool,
    reactivate: bool,
    max_concurrency: i64,
    held: Option<FileLock>,
) -> Result<ModeResult, String> {
    // A facet merge copies activity rows without pointing them at the facet
    // they joined. Adopt this day's rows before reading one, so the record
    // and the request name the same facet. When adoption can't run, the
    // identity checks below refuse exactly as they would have.
    let adopted =
        solstone_core_facets::adopt_moved_activity_rows(&context.journal, facet, &context.day)
            .unwrap_or(0);
    let record = get_activity_record(&context.journal, facet, &context.day, activity_id)
        .map_err(|error| record_detail(context.locale(), &error))?;
    let Some(record) = record else {
        let message = ActivityAction::RecordMissing;
        if let Some(mut work) = ActivityWork::open_existing(context, facet, activity_id)? {
            let configs = solstone_core_talent_config::load_talent_configs_in(
                context.locale(),
                &context.talent_root,
                &context.apps_root,
                None,
                TalentFilter {
                    r#type: None,
                    schedule: Some("activity"),
                    include_disabled: true,
                },
            )?;
            let reconciled =
                reconcile_finished(context, log, &mut work, &configs, activity_id, facet)?;
            if work.is_complete() {
                work.finish(context)?;
                return Ok(reconciled);
            }
            return block_destination(
                &mut work,
                context,
                log,
                activity_id,
                facet,
                "activity_missing",
                message,
            );
        }
        log_disposition(context, log, activity_id, facet, "projection_only", message);
        return Ok(message.failure(context));
    };
    if crate::activity_work::is_body_data_activity(&record) {
        return settle_body_data(context, log, activity_id, facet, held);
    }
    if activity_contract::is_synthetic(&record) || !activity_contract::has_nonempty_span(&record) {
        return Ok(ModeResult::default());
    }
    let held = match held {
        Some(held) => held,
        None => crate::activity_work::claim_activity(context, facet, activity_id)?,
    };
    let has_existing_work = crate::activity_work::has_work_record(context, facet, activity_id)?;
    let Some((configs, input_hash)) = selected_work_with_refresh(
        context,
        facet,
        activity_id,
        &record,
        refresh || has_existing_work,
    )?
    else {
        return Ok(ModeResult::default());
    };
    run_selected(
        context,
        log,
        activity_id,
        facet,
        refresh,
        reactivate,
        max_concurrency,
        Some(held),
        record,
        configs,
        input_hash,
        adopted,
    )
}

/// A Strava activity's talents never run, whether asked by a retry, by its
/// recorded work or by the owner with `--reactivate`: any work left on it is
/// settled through each talent's completion, and the run succeeds saying so.
fn settle_body_data(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity_id: &str,
    facet: &str,
    held: Option<FileLock>,
) -> Result<ModeResult, String> {
    // This run's own claim, if it holds one, is taken again just below.
    drop(held);
    if let Some(work) = ActivityWork::open_existing(context, facet, activity_id)? {
        work.settle(context)?;
    }
    let message = ActivityAction::BodyData;
    log_disposition(
        context,
        log,
        activity_id,
        facet,
        "body_data_settled",
        message,
    );
    Ok(ModeResult {
        success_names: vec![solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.activity.settled",
            [("facet", &(facet)), ("activity", &(activity_id))],
        )],
        ..ModeResult::default()
    })
}

/// Shared ordinary execution after its caller has admitted the actual input.
#[allow(clippy::too_many_arguments)]
fn run_selected(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity_id: &str,
    facet: &str,
    refresh: bool,
    reactivate: bool,
    max_concurrency: i64,
    held: Option<FileLock>,
    record: solstone_core_facets::ActivityRecord,
    configs: Vec<TalentConfig>,
    input_hash: String,
    adopted: usize,
) -> Result<ModeResult, String> {
    let kind = record
        .get("activity")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut work = ActivityWork::begin_held(
        context,
        facet,
        activity_id,
        input_hash,
        configs.iter().map(|c| c.key.clone()).collect(),
        refresh,
        held,
    )?;

    let reconciled = reconcile_finished(context, log, &mut work, &configs, activity_id, facet)?;
    if work.is_complete() {
        work.finish(context)?;
        return Ok(reconciled);
    }

    // An edit that arrived while Generate ran must remain held on later due
    // retries, before another model call. Accepted uses were reconciled above.
    if selected_output_held(&configs, &record) {
        return block_destination(
            &mut work,
            context,
            log,
            activity_id,
            facet,
            "owner_field_held",
            ActivityAction::OwnerFieldHeld,
        );
    }

    let stored_id = work.destination_id().map(str::to_owned).or_else(|| {
        record
            .get("destination_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });

    let destination = solstone_core_facets::observe_facet_destination(&context.journal, facet)
        .map_err(|error| error.to_string())?;
    use solstone_core_facets::DestinationObservation;
    let (facet_id, is_muted) = match destination {
        DestinationObservation::Absent => {
            return block_destination(
                &mut work,
                context,
                log,
                activity_id,
                facet,
                "destination_missing",
                ActivityAction::Missing,
            );
        }
        DestinationObservation::Malformed(_) | DestinationObservation::InvalidId { .. } => {
            return block_destination(
                &mut work,
                context,
                log,
                activity_id,
                facet,
                "declaration_malformed",
                ActivityAction::Malformed,
            );
        }
        DestinationObservation::Unreadable(_) => {
            if work.is_muted_pause() {
                work.set_muted_pause(false)?;
            }
            if work.due(context.event_now_ms()) {
                work.start_attempt(context.event_now_ms())?;
            }
            let message = if work.is_blocked() {
                ActivityAction::ReadBlocked
            } else {
                ActivityAction::ReadRetry
            };
            log_disposition(
                context,
                log,
                activity_id,
                facet,
                "declaration_unreadable",
                message,
            );
            return Ok(message.failure(context));
        }
        DestinationObservation::LegacyWithoutId { muted } => {
            if stored_id.is_some() {
                return block_destination(
                    &mut work,
                    context,
                    log,
                    activity_id,
                    facet,
                    "destination_replaced",
                    ActivityAction::Replaced,
                );
            }
            let id = solstone_core_facets::admit_activity_destination(&context.journal, facet)
                .map_err(|error| error.to_string())?;
            (id, muted)
        }
        DestinationObservation::Ready { id, muted } => (id, muted),
    };
    if stored_id.as_ref().is_some_and(|stored| *stored != facet_id) {
        return block_destination(
            &mut work,
            context,
            log,
            activity_id,
            facet,
            "destination_replaced",
            ActivityAction::Replaced,
        );
    }
    if work.destination_id().is_none() {
        work.set_destination_id(facet_id.clone())?;
    }
    if is_muted {
        work.set_muted_pause(true)?;
        let message = if work.is_blocked() {
            ActivityAction::MutedBlocked
        } else {
            ActivityAction::MutedRetry
        };
        log_disposition(
            context,
            log,
            activity_id,
            facet,
            "destination_muted",
            message,
        );
        return Ok(message.failure(context));
    }
    // An earlier version blocked this work because the row still named the
    // facet merged into this one. The row now names this facet, so that
    // block no longer holds.
    let stale_merge_block = adopted > 0
        && work
            .blocked_disposition()
            .is_some_and(|blocked| blocked.reason_code == "destination_replaced");
    if reactivate || stale_merge_block {
        work.reactivate()?;
    } else {
        if work.is_muted_pause() {
            work.set_muted_pause(false)?;
        }
        if work.is_blocked() {
            return Ok(match work.blocked_disposition() {
                Some(blocked) => failed(blocked.failure(context.terminal_locale)),
                None => ActivityAction::Blocked.failure(context),
            });
        }
    }

    if context.retry.is_none() && !work.due(context.event_now_ms()) {
        return Ok(ActivityAction::Pending.failure(context));
    }
    work.start_attempt(context.event_now_ms())?;
    let refresh = refresh || work.refreshes();
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
    let mut total = reconciled;
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
            if context.retry.as_ref().is_some_and(|(unit, _)| !matches!(unit,
                solstone_core_system::work_retry::Unit::Activity { talent, .. } if talent == &config.key)) { continue; }
            if work.parked(&config.key) && context.retry.is_none() {
                group.failed += 1;
                group.failed_names.push(
                    crate::dispatch::RunFailure::message(
                        context.terminal_locale,
                        "journal.cli.think.activity.repair.required",
                    )
                    .named(&config.key),
                );
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
                &facet_id,
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
                        .push(format!("{} (request_lost)", config.key).into());
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
                    group
                        .failed_names
                        .push(format!("{} (send)", config.key).into());
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

fn reconcile_finished(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    work: &mut ActivityWork,
    configs: &[solstone_core_talent_config::TalentConfig],
    activity_id: &str,
    facet: &str,
) -> Result<ModeResult, String> {
    let mut result = ModeResult::default();
    for config in configs {
        let Some(id) = work.use_id(&config.key).map(str::to_owned) else {
            continue;
        };
        if !work.contains(&config.key)
            || get_use_end_state(&context.journal, &id).map_err(|error| error.to_string())?
                != UseEndState::Finish
        {
            continue;
        }
        log_complete(log, context, activity_id, facet, &config.key, &id, "finish");
        log.finish()?;
        work.complete(&config.key)?;
        result.success += 1;
        result.success_names.push(config.key.clone());
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum ActivityAction {
    RecordMissing,
    BodyData,
    OwnerFieldHeld,
    Missing,
    Malformed,
    Replaced,
    ReadBlocked,
    ReadRetry,
    MutedBlocked,
    MutedRetry,
    Blocked,
    Pending,
}

impl ActivityAction {
    fn id(self) -> &'static str {
        match self {
            Self::RecordMissing => "journal.cli.think.activity.record.missing",
            Self::BodyData => "journal.cli.think.activity.body.data",
            Self::OwnerFieldHeld => "journal.cli.think.activity.owner.field.held",
            Self::Missing => "journal.cli.think.activity.missing",
            Self::Malformed => "journal.cli.think.activity.malformed",
            Self::Replaced => "journal.cli.think.activity.replaced",
            Self::ReadBlocked => "journal.cli.think.activity.read.blocked",
            Self::ReadRetry => "journal.cli.think.activity.read.retry",
            Self::MutedBlocked => "journal.cli.think.activity.muted.blocked",
            Self::MutedRetry => "journal.cli.think.activity.muted.retry",
            Self::Blocked => "journal.cli.think.activity.blocked",
            Self::Pending => "journal.cli.think.activity.pending",
        }
    }

    fn from_id(value: &str) -> Option<Self> {
        [
            Self::RecordMissing,
            Self::BodyData,
            Self::OwnerFieldHeld,
            Self::Missing,
            Self::Malformed,
            Self::Replaced,
            Self::ReadBlocked,
            Self::ReadRetry,
            Self::MutedBlocked,
            Self::MutedRetry,
            Self::Blocked,
            Self::Pending,
        ]
        .into_iter()
        .find(|action| action.id() == value)
    }

    fn text(self, locale: &str) -> String {
        solstone_core_cli_copy::render_for(locale, self.id(), &[])
    }

    fn failure(self, context: &ThinkContext) -> ModeResult {
        failed(crate::dispatch::RunFailure::projected(
            self.text(solstone_core_cli_copy::ENGLISH),
            context.terminal_locale.map(|locale| self.text(locale)),
        ))
    }
}

pub(crate) fn record_detail(
    locale: &str,
    error: &solstone_core_facets::ActivityRecordStoreError,
) -> String {
    use solstone_core_facets::ActivityRecordStoreError as E;
    use solstone_core_journal_io::cli_diagnostics::{lock_detail, path_detail, read_detail};
    match error {
        E::MissingDayFile { path } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.activity.day.missing",
            [("path", &(path.display()))],
        ),
        E::MissingRecord {
            facet,
            day,
            record_id,
        } => solstone_core_cli_copy::display_for(
            locale,
            "journal.cli.think.activity.row.missing",
            [("path", &(format!("{facet}/{day}/{record_id}")))],
        ),
        E::Path(error) => path_detail(locale, error),
        E::Read(error) => read_detail(locale, error),
        E::Lock(error) => lock_detail(locale, error),
        E::Facet(error) => solstone_core_talent_cli::preview::facet_detail(locale, error),
        _ => error.to_string(),
    }
}

impl crate::activity_work::BlockedDisposition {
    pub(crate) fn failure(&self, locale: Option<&str>) -> crate::dispatch::RunFailure {
        let terminal = locale
            .filter(|locale| *locale != solstone_core_cli_copy::ENGLISH)
            .and_then(|locale| {
                self.message_key
                    .as_deref()
                    .and_then(ActivityAction::from_id)
                    .filter(|action| action.text(solstone_core_cli_copy::ENGLISH) == self.message)
                    .map(|action| action.text(locale))
            });
        crate::dispatch::RunFailure::projected(self.message.clone(), terminal)
    }
}

fn log_disposition(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity: &str,
    facet: &str,
    reason: &str,
    message: ActivityAction,
) {
    log.log(
        "activity.destination",
        context.event_now_ms(),
        fields(
            context,
            activity,
            facet,
            Map::from_iter([
                ("reason_code".to_owned(), Value::String(reason.to_owned())),
                (
                    "next_action".to_owned(),
                    Value::String(message.text(solstone_core_cli_copy::ENGLISH)),
                ),
            ]),
        ),
    );
}

fn block_destination(
    work: &mut ActivityWork,
    context: &ThinkContext,
    log: &mut RunLogWriter,
    activity: &str,
    facet: &str,
    reason: &str,
    message: ActivityAction,
) -> Result<ModeResult, String> {
    work.block(
        reason,
        &message.text(solstone_core_cli_copy::ENGLISH),
        Some(message.id()),
    )?;
    log_disposition(context, log, activity, facet, reason, message);
    log.finish()?;
    Ok(message.failure(context))
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
    facet_id: &str,
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
        (
            "destination_id".to_owned(),
            Value::String(facet_id.to_owned()),
        ),
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
    let unit = solstone_core_system::work_retry::Unit::Activity {
        day: context.day.clone(),
        facet: facet.into(),
        activity: activity_id.into(),
        talent: config.key.clone(),
    };
    request.insert(
        "_work_unit".into(),
        serde_json::to_value(&unit).map_err(|_| DispatchFailure::Unavailable)?,
    );
    if let Some((selected, attempt)) = &context.retry {
        if selected != &unit {
            return Err(DispatchFailure::Unavailable);
        }
        request.insert(
            "_work_retry".into(),
            serde_json::to_value(attempt).map_err(|_| DispatchFailure::Unavailable)?,
        );
    }
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
        });
    }
    let request = solstone_core_cortex_client::CortexRequest::new(
        if generate {
            String::new()
        } else {
            activity_contract::untyped_activity_prompt(activity_id, kind, facet, &context.day)
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
                    // A context refusal can recover after a meaningful change,
                    // but unchanged activity catchup must not repeat it hourly.
                    // Connection interruption still never exhausts its retries.
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
                                    .is_some_and(|e| {
                                        e["retryable"] == false
                                            || e["reason_code"] == "context_window_exceeded"
                                    })
                            })
                    {
                        work.park(&item.name, cause.as_deref().unwrap_or(state), context)
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
        // Same gap as daily mode's log_daily_failure: reason_code alone drops whatever
        // else the worker's terminal error carried (e.g. schema_validation's per-field
        // violations).
        if let Some(detail) =
            use_id.and_then(|use_id| use_log_failure_detail(&context.journal, use_id))
        {
            extra.insert("detail".to_owned(), detail);
        }
    }
    let base = fields(context, activity, facet, extra);
    let event_ms = context.event_now_ms();
    log.log("talent.completed", event_ms, base.clone());
    log.log("talent.fail", event_ms, base);
}

fn merge(into: &mut ModeResult, from: ModeResult) {
    merge_mode_result(into, from);
}
fn failed(name: crate::dispatch::RunFailure) -> ModeResult {
    ModeResult {
        failed: 1,
        failed_names: vec![name],
        ..ModeResult::default()
    }
}
