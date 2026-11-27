// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DrainOutcome, ModeResult, PendingUse, dispatch_direct, drain_with_deadline_observed,
    drain_with_failure_policy, merge_mode_result, runtime, use_log_blocking,
    use_log_failure_detail,
};
use crate::helpers;
use crate::run_log::RunLogWriter;
use jiff::Timestamp;
use serde_json::{Map, Value};
use solstone_core_facets::{
    ActivityRecordStoreError, AppendOutcome, DeclaredFacetInventory,
    append_activity_record_checked, get_activity_record, observe_declared_facet_inventory,
};
use solstone_core_format::content::strava_body::STRAVA_STREAM;
use solstone_core_journal_io::{
    AtomicWriteOptions, DEFAULT_STREAM, FileLock, LockOptions, PathOrDay, atomic_replace,
    hold_lock, iter_segments,
};
use solstone_core_system::activity_state::{
    ActivityStateMachine, SHARED_ACTIVITY_STATE, activity_state_path,
};
use solstone_core_system_health::{
    FilesystemHealthLogSource, SEGMENT_FLOOR_TALENTS, SEGMENT_NONGATING_TALENTS,
    detect_segment_change, find_segment_dir, is_floor_talent_capped, lookup_segment_progress,
    newest_segment_input_ms, read_segment_data_state, read_segment_progress, resolve_predecessor,
    segment_fully_sensed, segment_thinking_is_current,
};
use solstone_core_talent_config::{TalentConfig, TalentFilter, get_output_path};

/// Port of `thinking.py:1382-2052`, with activity-state replay performed by
/// [`replay_activity_state`] after the segment's Sense projection is durable.
#[allow(
    clippy::too_many_arguments,
    reason = "The reference keeps segment mode, timeout, live, and skip controls distinct at this boundary."
)]
pub(crate) fn run(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    refresh: bool,
    stream: Option<&str>,
    max_concurrency: i64,
    timeout: Option<Duration>,
    live: bool,
    skip_talents: &[String],
) -> Result<ModeResult, String> {
    let current = if live {
        CurrentSegment::Skip
    } else {
        CurrentSegment::Rethink
    };
    run_with(
        context,
        log,
        segment,
        refresh,
        stream,
        max_concurrency,
        timeout,
        live,
        skip_talents,
        current,
    )
}

/// What a run does with a segment whose thinking already covers its input.
///
/// Live thinking and the day's segment repair select segments independently,
/// so after a late delivery each would otherwise think again what the other
/// has just thought.  The two schedulers skip; an explicit request re-thinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CurrentSegment {
    Rethink,
    Skip,
}

/// How long a run waits for another run to finish thinking the same segment.
///
/// Longer than one segment's thinking deadline, so a waiter normally sees the
/// other run's finished work.
const SEGMENT_TURN_WAIT: Duration = Duration::from_secs(15 * 60);

/// Think one segment, then publish its saved outputs once its thinking covers
/// its input. Every way a segment is thought ends here, so a thought segment
/// is ready for its day's daily work without a separate repair.
#[allow(
    clippy::too_many_arguments,
    reason = "The reference keeps segment mode, timeout, live, and skip controls distinct at this boundary."
)]
pub(crate) fn run_with(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    refresh: bool,
    stream: Option<&str>,
    max_concurrency: i64,
    timeout: Option<Duration>,
    live: bool,
    skip_talents: &[String],
    current: CurrentSegment,
) -> Result<ModeResult, String> {
    let mut result = think(
        context,
        log,
        segment,
        refresh,
        stream,
        max_concurrency,
        timeout,
        live,
        skip_talents,
        current,
    )?;
    publish_if_thought(context, segment, stream, &mut result);
    Ok(result)
}

/// After a run that succeeded, publish the segment's saved outputs if its
/// thinking now covers its input. A failed publication is the run's failure;
/// the segment stays owed and nothing is thought again to retry it.
pub(crate) fn publish_if_thought(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    result: &mut ModeResult,
) {
    if result.failed != 0 || result.timed_out {
        return;
    }
    let published = (|| {
        let Some(segment_dir) = resolve_segment_dir(
            context.locale(),
            &context.journal,
            &context.day,
            segment,
            stream,
        )?
        else {
            return Ok(());
        };
        let stream = stream.or_else(|| named_stream(&segment_dir, &context.day));
        // A Strava tile's earlier thinking is never published again.
        if stream == Some(STRAVA_STREAM)
            || !thinking_is_current(context, segment, stream, &segment_dir)
        {
            return Ok(());
        }
        publish_saved_outputs(context, &segment_dir)
    })();
    if let Err(error) = published {
        crate::dispatch::record_followup_failure(result, "publication", &error);
    }
}

/// Index a thought segment's saved outputs and record that they are
/// published. No model is called and no output is rewritten.
pub(crate) fn publish_saved_outputs(
    context: &ThinkContext,
    segment_dir: &std::path::Path,
) -> Result<(), String> {
    if (context.cancel_checkpoint)() {
        return Err("cancelled before publication".into());
    }
    // A deleted segment publishes nothing.
    if is_deleted_segment(segment_dir) {
        return Ok(());
    }
    // Taken so a run thinking this segment does not change it underneath.
    let _turn = segment_turn(context, segment_dir);
    let before = newest_segment_input_ms(segment_dir).map_err(|error| error.to_string())?;
    if solstone_core_system::segment_publication::is_current(&context.journal, segment_dir, before)?
    {
        return Ok(());
    }
    if (context.cancel_checkpoint)() {
        return Err("cancelled before publication".into());
    }
    solstone_core_system::segment_publication::publish_checked(
        &context.journal,
        segment_dir,
        before,
        |journal, path| context.index.enqueue_path(journal, path),
        &|| context.checkpoint(),
    )?;
    if before != newest_segment_input_ms(segment_dir).map_err(|error| error.to_string())? {
        return Err("segment input changed during publication".into());
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "The reference keeps segment mode, timeout, live, and skip controls distinct at this boundary."
)]
fn think(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    refresh: bool,
    stream: Option<&str>,
    max_concurrency: i64,
    timeout: Option<Duration>,
    live: bool,
    skip_talents: &[String],
    current: CurrentSegment,
) -> Result<ModeResult, String> {
    let configs = solstone_core_talent_config::load_talent_configs_in(
        context.locale(),
        &context.talent_root,
        &context.apps_root,
        None,
        TalentFilter {
            r#type: None,
            schedule: Some("segment"),
            include_disabled: false,
        },
    )?;
    if configs.is_empty() {
        return Ok(ModeResult::default());
    }
    let by_name = configs
        .into_iter()
        .map(|config| (config.key.clone(), config))
        .collect::<BTreeMap<_, _>>();
    let Some(sense) = by_name.get("sense") else {
        log_skip(log, context, "sense", segment, "no_config", stream);
        return Ok(ModeResult {
            failed: 1,
            failed_names: vec!["sense (not_configured)".to_owned().into()],
            ..ModeResult::default()
        });
    };
    let Some(segment_dir) = resolve_segment_dir(
        context.locale(),
        &context.journal,
        &context.day,
        segment,
        stream,
    )?
    else {
        return Ok(ModeResult {
            failed: 1,
            failed_names: vec!["sense (missing_segment)".to_owned().into()],
            ..ModeResult::default()
        });
    };
    // A segment the owner deleted keeps only its tombstone. Nothing is thought
    // about it, and nothing, not even this run's turn, is written for it.
    if is_deleted_segment(&segment_dir) {
        log_skip(log, context, "*", segment, "deleted", stream);
        return Ok(ModeResult::default());
    }
    let stream = stream.or_else(|| named_stream(&segment_dir, &context.day));
    // Held to the end of the run, so a second run on this segment starts from
    // this one's finished work rather than beside it.
    let _turn = segment_turn(context, &segment_dir);
    if stream == Some(STRAVA_STREAM) {
        return end_body_data_sense(context, log, segment, stream);
    }
    let state = read_segment_data_state(
        &context.journal,
        &context.day,
        segment,
        stream,
        Timestamp::now(),
    );
    if !segment_fully_sensed(&state) {
        // A retryable failure is still owed media analysis, not empty input.
        log_skip(log, context, "sense", segment, "raw_media_pending", stream);
        return Ok(ModeResult::default());
    }
    // A live-only talent never runs from the day's repair, so the live run
    // that would be its one chance is never skipped.
    let live_only_talent = live
        && by_name
            .values()
            .any(|config| config.metadata.get("new_only").is_some_and(python_truthy));
    if current == CurrentSegment::Skip
        && !refresh
        && !live_only_talent
        && thinking_is_current(context, segment, stream, &segment_dir)
    {
        log_skip(log, context, "*", segment, "already_thought", stream);
        return Ok(ModeResult::default());
    }
    let load = sense
        .metadata
        .get("load")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    // Source-derived, not measured: thinking.py:1507-1534 starts the Sense
    // lifecycle before the no-input gate so its terminal result remains visible.
    context
        .status
        .update(segment_status(context, segment, stream, 1, 0));
    let _ = helpers::emit(
        &context.journal,
        context.now_ms,
        "started",
        segment_event(
            context,
            segment,
            stream,
            Map::from_iter([
                ("count".to_owned(), Value::from(1)),
                ("groups".to_owned(), Value::from(1)),
            ]),
        ),
    );
    // A routing retry rereads declarations against retained source, without another model call.
    let pending = solstone_core_system_health::read_pending_facet_routing(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .map_err(|error| error.render(context.locale()))?;
    if pending.malformed_line_count != 0 {
        return Err(solstone_core_cli_copy::render_for(
            context.locale(),
            "journal.cli.think.routing.log.malformed",
            &[],
        ));
    }
    let key = solstone_core_system_health::SegmentIdentity {
        stream: stream.map(str::to_owned),
        segment: segment.to_owned(),
    };
    let routing_pending = if let Some(fingerprint) =
        pending.value.get(&key).and_then(Option::as_deref)
    {
        solstone_core_system::catchup::read_raw_input_fingerprint(&context.journal, &context.day)
            .map_err(|error| error.to_string())?
            == fingerprint
    } else {
        false
    };
    if routing_pending && !refresh {
        let raw = std::fs::read(segment_dir.join("talents/sense.json"))
            .map_err(|error| error.to_string())?;
        let retained: Value = serde_json::from_slice(&raw).map_err(|error| error.to_string())?;
        let sense = retained
            .as_object()
            .filter(|_| valid_activity_sense(&retained))
            .ok_or_else(|| {
                solstone_core_cli_copy::render_for(
                    context.locale(),
                    "journal.cli.think.routing.source.invalid",
                    &[],
                )
            })?;
        write_sense_and_change(context, log, segment, stream, &segment_dir, sense)?;
        return Ok(ModeResult::default());
    }
    // Retry transiently failed / pending / thinking-off photo observations
    // while the journal still holds the original pixels, before Sense runs.
    if let Some(stream_name) = stream
        && segment_dir
            .join(solstone_core_format::content::PHOTO_OBSERVATION_FILENAME)
            .is_file()
        && solstone_core_segment::photo_observation::read_photo_observation(&segment_dir)
            .is_some_and(|obs| obs.state.is_retryable())
    {
        let _ = solstone_core_import_sources::image::retry_photo_observation(
            &context.journal,
            &context.day,
            stream_name,
            segment,
            &solstone_core_import_sources::image::SystemWireClient,
            &solstone_core_import_sources::photo_faces::SystemPhotoFaceDetector,
        );
    }
    if solstone_core_talent_runtime::check_segment_has_no_input(
        &context.journal,
        &context.day,
        segment,
        stream,
        &load,
        Timestamp::now(),
    ) {
        // Source-derived, not measured: thinking.py:1536-1584 writes a
        // schema-valid idle artifact and change record before terminalizing.
        let sense_json = empty_input_sense_output();
        let change =
            write_sense_and_change(context, log, segment, stream, &segment_dir, &sense_json)?;
        log_sense(log, context, segment, "idle", stream);
        log_change(
            log,
            context,
            segment,
            change["change_class"].as_str().unwrap_or("idle"),
            stream,
        );
        context
            .status
            .update(segment_status(context, segment, stream, 1, 0));
        let _ = helpers::emit(
            &context.journal,
            context.now_ms,
            "completed",
            segment_event(
                context,
                segment,
                stream,
                Map::from_iter([
                    ("success".to_owned(), Value::from(0)),
                    ("failed".to_owned(), Value::from(0)),
                    ("density".to_owned(), Value::String("idle".to_owned())),
                ]),
            ),
        );
        log_skip(log, context, "*", segment, "density_idle", stream);
        complete(log, context, segment, stream, ModeResult::default());
        return Ok(ModeResult::default());
    }
    if !refresh && held_aside(context, &segment_dir, segment, stream, "sense")? {
        log_skip(log, context, "sense", segment, "set_aside", stream);
        return Ok(ModeResult::default());
    }
    // A silent segment whose screens describe carried over unchanged continues
    // its predecessor's Sense: asking the model again re-reads the same screen.
    if !refresh && let Some(continued) = sense_continuation(context, segment, stream, &segment_dir)
    {
        let change =
            write_sense_and_change(context, log, segment, stream, &segment_dir, &continued)?;
        log_sense(
            log,
            context,
            segment,
            continued
                .get("density")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            stream,
        );
        let change_class = change
            .get("change_class")
            .and_then(Value::as_str)
            .unwrap_or_default();
        log_change(log, context, segment, change_class, stream);
        if change_class == "redundant" {
            log_skip(log, context, "sense", segment, "continuation", stream);
            log_skip(log, context, "*", segment, "change_redundant", stream);
            let result = ModeResult::default();
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
        // Something changed between the check and the write, so Sense runs.
    }
    let runtime = runtime()?;
    let sense_use = match dispatch_agent(
        context,
        &runtime,
        sense,
        segment,
        refresh,
        stream,
        live,
        skip_talents,
        log,
    ) {
        // The reference continues to the persisted Sense artifact when a
        // dispatch was intentionally skipped (`thinking.py:1644-1708`).
        Ok(AgentDispatch::Skipped) => None,
        Ok(AgentDispatch::Pending(item)) => Some(item),
        Err(DispatchFailure::NotClaimed { use_id }) => {
            log_request_lost(log, context, "sense", segment, stream, &use_id);
            let result = ModeResult {
                failed: 1,
                failed_names: vec!["sense (request_lost)".to_owned().into()],
                ..ModeResult::default()
            };
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
        Err(DispatchFailure::Unavailable) => {
            log_skip(log, context, "sense", segment, "send_failed", stream);
            let result = ModeResult {
                failed: 1,
                failed_names: vec!["sense (send)".to_owned().into()],
                ..ModeResult::default()
            };
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
    };
    if let Some(sense_use) = sense_use.as_ref() {
        // Source-derived, not measured: thinking.py:1648-1675 records the
        // per-talent dispatch lifecycle before draining the Sense use.
        log_dispatch(log, context, segment, stream, sense_use);
        context.status.update(segment_status_with_current(
            context,
            segment,
            stream,
            1,
            0,
            vec!["sense".to_owned()],
        ));
    }
    let mut sense_observed = Vec::new();
    let mut result = sense_use.map_or_else(ModeResult::default, |sense_use| {
        drain_with_deadline_observed(
            context,
            &runtime,
            vec![sense_use],
            timeout,
            &mut |item, outcome| {
                log_use_terminal(log, context, segment, stream, item, outcome.clone());
                sense_observed.push((item.use_id.clone(), outcome));
            },
        )
    });
    if result.failed != 0
        && let Some((use_id, outcome)) = sense_observed.first()
        && is_retryable_capacity_exhausted(&context.journal, use_id, outcome)
        && let Ok(AgentDispatch::Pending(retry_use)) = dispatch_agent(
            context,
            &runtime,
            sense,
            segment,
            refresh,
            stream,
            live,
            skip_talents,
            log,
        )
    {
        log_dispatch(log, context, segment, stream, &retry_use);
        let retry_result = drain_with_deadline_observed(
            context,
            &runtime,
            vec![retry_use],
            timeout,
            &mut |r_item, r_outcome| {
                log_use_terminal(log, context, segment, stream, r_item, r_outcome);
            },
        );
        result.timed_out |= retry_result.timed_out;
        if retry_result.failed == 0 && retry_result.success != 0 && !retry_result.timed_out {
            let prefix = "sense (";
            if let Some(pos) = result
                .failed_names
                .iter()
                .position(|n| n.source.starts_with(prefix) || n.source == "sense")
            {
                result.failed_names.remove(pos);
                result.failed = result.failed.saturating_sub(1);
            }
            result.success += retry_result.success;
            result.success_names.extend(retry_result.success_names);
        }
    }
    context.status.update(segment_status(
        context,
        segment,
        stream,
        1,
        result.success + result.failed,
    ));
    if result.failed != 0 {
        result.provider_blocked = sense_observed.iter().any(|(use_id, outcome)| {
            matches!(outcome, DrainOutcome::Fail { .. })
                && use_log_blocking(&context.journal, use_id)
        });
        complete(log, context, segment, stream, result.clone());
        return Ok(result);
    }

    let sense_path = get_output_path(
        &context.day_dir,
        "sense",
        Some(segment),
        Some("json"),
        None,
        stream,
    );
    let sense_bytes = match std::fs::read(&sense_path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let result = failed("sense (output_parse)");
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
    };
    let sense_json: Value = match serde_json::from_slice(&sense_bytes).ok() {
        Some(value) => value,
        None => {
            let result = failed("sense (output_parse)");
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
    };
    let Some(sense_object) = sense_json.as_object() else {
        let result = failed("sense (output_invalid)");
        complete(log, context, segment, stream, result.clone());
        return Ok(result);
    };
    if !sense_object.contains_key("density") || !sense_object.contains_key("content_type") {
        let result = failed("sense (output_invalid)");
        complete(log, context, segment, stream, result.clone());
        return Ok(result);
    }
    let Some(density) = sense_object.get("density").and_then(Value::as_str) else {
        return Ok(failed("sense (output_invalid)"));
    };
    let change = write_sense_and_change(context, log, segment, stream, &segment_dir, sense_object)?;
    log_sense(log, context, segment, density, stream);
    let change_class = change
        .get("change_class")
        .and_then(Value::as_str)
        .unwrap_or_default();
    log_change(log, context, segment, change_class, stream);
    if change_class == "redundant" && !refresh {
        log_skip(log, context, "*", segment, "change_redundant", stream);
        complete(log, context, segment, stream, result.clone());
        return Ok(result);
    }
    let mut total = result;
    let mut agents = if density == "idle" && !refresh {
        // An idle segment keeps its Sense projection and activity-state update
        // but dispatches no further talents.
        log_skip(log, context, "*", segment, "density_idle", stream);
        Vec::new()
    } else {
        select_agents(
            context,
            log,
            &by_name,
            sense_object,
            &segment_dir,
            segment,
            stream,
            refresh,
        )?
    };
    context.status.update(segment_status(
        context,
        segment,
        stream,
        1 + agents.len(),
        total.success + total.failed,
    ));
    let mut pending = Vec::new();
    for config in agents.drain(..) {
        match dispatch_agent(
            context,
            &runtime,
            config,
            segment,
            refresh,
            stream,
            live,
            skip_talents,
            log,
        ) {
            Ok(AgentDispatch::Skipped) => {}
            Ok(AgentDispatch::Pending(item)) => {
                // Source-derived, not measured: thinking.py:1943-1956 logs
                // every accepted selected-agent dispatch before batch draining.
                log_dispatch(log, context, segment, stream, &item);
                pending.push(item);
            }
            Err(DispatchFailure::Unavailable) => {
                log_skip(log, context, &config.key, segment, "send_failed", stream);
                if !SEGMENT_NONGATING_TALENTS.contains(&config.key.as_str()) {
                    total.failed += 1;
                    total
                        .failed_names
                        .push(format!("{} (send)", config.key).into());
                }
            }
            Err(DispatchFailure::NotClaimed { use_id }) => {
                // Source-derived, not measured: thinking.py:1951-1960 treats
                // a selected talent's unclaimed request separately from send failure.
                log_request_lost(log, context, &config.key, segment, stream, &use_id);
                if !SEGMENT_NONGATING_TALENTS.contains(&config.key.as_str()) {
                    total.failed += 1;
                    total
                        .failed_names
                        .push(format!("{} (request_lost)", config.key).into());
                }
            }
        }
        if max_concurrency != 0 && pending.len() as i64 >= max_concurrency {
            drain_selected(
                context,
                &runtime,
                &mut pending,
                timeout,
                &mut total,
                &by_name,
                segment,
                refresh,
                stream,
                live,
                skip_talents,
                log,
            );
        }
        context.status.update(segment_status(
            context,
            segment,
            stream,
            1,
            total.success + total.failed,
        ));
    }
    // Source-derived, not measured: thinking.py:1994-2010 drains the final
    // partial selection batch; zero means one unlimited final batch.
    drain_selected(
        context,
        &runtime,
        &mut pending,
        timeout,
        &mut total,
        &by_name,
        segment,
        refresh,
        stream,
        live,
        skip_talents,
        log,
    );

    complete(log, context, segment, stream, total.clone());
    Ok(total)
}

/// Whether the owner deleted this segment: its directory keeps only its
/// tombstone, whatever else a late writer left beside it.
fn is_deleted_segment(segment_dir: &std::path::Path) -> bool {
    std::fs::symlink_metadata(segment_dir.join("tombstone.json")).is_ok()
}

/// Take this segment's thinking turn, shared by every process on this journal.
///
/// A turn that cannot be taken, whether it times out or its lock file cannot
/// be made, is not fatal: the run goes ahead unserialized, as every run did
/// before turns existed.
///
/// The turn is named from the segment directory found on disk, never from
/// caller input.
fn segment_turn(context: &ThinkContext, segment_dir: &std::path::Path) -> Option<FileLock> {
    let segment = segment_dir.file_name()?.to_str()?;
    let path = context
        .day_dir
        .join("health")
        .join("segment-think")
        .join(named_stream(segment_dir, &context.day).unwrap_or("_"))
        .join(segment);
    match hold_lock(
        &path,
        LockOptions {
            timeout: SEGMENT_TURN_WAIT,
            ..LockOptions::default()
        },
    ) {
        Ok(turn) => Some(turn),
        Err(error) => {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.segment.turn",
                    [
                        ("day", &context.day),
                        ("segment", &segment),
                        (
                            "error",
                            &solstone_core_journal_io::cli_diagnostics::lock_detail(
                                context.locale(),
                                &error
                            )
                        )
                    ]
                )
            );
            None
        }
    }
}

/// Whether this segment's recorded thinking already covers its current input.
///
/// Anything unreadable answers no, so the segment is thought again.
fn thinking_is_current(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    segment_dir: &std::path::Path,
) -> bool {
    let Ok(newest_input_ms) = newest_segment_input_ms(segment_dir) else {
        return false;
    };
    let Ok(progress) = read_segment_progress(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    ) else {
        return false;
    };
    segment_thinking_is_current(
        lookup_segment_progress(&progress.value, stream.unwrap_or_default(), segment),
        newest_input_ms,
    )
}

#[allow(clippy::too_many_arguments)]
/// A held talent reuses the verified Sense output. No Sense rerun, activity
/// replay, successful sibling or whole-day work is hidden here. A held Sense
/// is the segment's first step, so retrying it thinks the segment again.
pub(crate) fn retry_talent(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    stream: &str,
    talent: &str,
) -> Result<ModeResult, crate::dispatch::RunFailure> {
    // A Strava tile is body data that no talent reads. Work held on one from
    // before ends as the retry of finished work does: accepted, its hold
    // released, with no model call.
    if stream == STRAVA_STREAM {
        if let Some((unit, _)) = &context.retry {
            solstone_core_system::work_retry::reconcile_accepted_checked(
                &context.journal,
                unit,
                &|| context.checkpoint(),
            )?;
        }
        log_skip(log, context, talent, segment, "body_data", Some(stream));
        return Ok(ModeResult::default());
    }
    if !SEGMENT_FLOOR_TALENTS.contains(&talent) {
        // Finished elsewhere, or no longer selected: nothing is left to
        // retry, so the hold is released rather than retried every pass.
        let progress = read_segment_progress(
            &FilesystemHealthLogSource::new(&context.journal),
            &context.day,
        )
        .map_err(|error| context.failure(|locale| error.render(locale)))?;
        if progress.malformed_line_count != 0 {
            return Err(context.failure(|locale| {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.think.health.progress.unreadable",
                    &[],
                )
            }));
        }
        let pending = solstone_core_system_health::segment_pending_talents(
            lookup_segment_progress(&progress.value, stream, segment),
        );
        if !pending.contains(talent) {
            if let Some((unit, _)) = &context.retry {
                solstone_core_system::work_retry::reconcile_accepted_checked(
                    &context.journal,
                    unit,
                    &|| context.checkpoint(),
                )?;
            }
            return Ok(ModeResult::default());
        }
    }
    if talent == "sense" {
        return run_with(
            context,
            log,
            segment,
            false,
            Some(stream),
            1,
            Some(crate::dispatch::DEFAULT_THINK_TIMEOUT),
            false,
            &[],
            CurrentSegment::Skip,
        )
        .map_err(Into::into);
    }
    let configs = solstone_core_talent_config::load_talent_configs_in(
        context.locale(),
        &context.talent_root,
        &context.apps_root,
        solstone_core_talent_config::read_talent_overrides_in(context.locale(), &context.journal)?
            .as_ref(),
        TalentFilter {
            r#type: None,
            schedule: Some("segment"),
            include_disabled: false,
        },
    )?;
    let config = configs.iter().find(|c| c.key == talent).ok_or_else(|| {
        context.failure(|locale| {
            solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.think.retry.talent.disabled",
                &[],
            )
        })
    })?;
    let runtime = crate::dispatch::runtime()?;
    let item = match dispatch_agent(
        context,
        &runtime,
        config,
        segment,
        false,
        Some(stream),
        false,
        &[],
        log,
    ) {
        Ok(AgentDispatch::Pending(item)) => item,
        Ok(AgentDispatch::Skipped) => {
            return Err(context.failure(|locale| {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.think.retry.talent.skipped",
                    &[],
                )
            }));
        }
        Err(_) => {
            return Err(context.failure(|locale| {
                solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.think.retry.talent.dispatch",
                    &[],
                )
            }));
        }
    };
    let result = drain_with_deadline_observed(
        context,
        &runtime,
        vec![item],
        Some(crate::dispatch::DEFAULT_THINK_TIMEOUT),
        &mut |item, outcome| log_use_terminal(log, context, segment, Some(stream), item, outcome),
    );
    if result.failed == 0 {
        let directory = find_segment_dir(&context.journal, &context.day, segment, Some(stream))
            .ok_or_else(|| {
                context.failure(|locale| {
                    solstone_core_cli_copy::render_for(
                        locale,
                        "journal.cli.think.retry.talent.segment.missing",
                        &[],
                    )
                })
            })?;
        publish_saved_outputs(context, &directory)?;
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn select_agents<'a>(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    by_name: &'a BTreeMap<String, TalentConfig>,
    sense: &Map<String, Value>,
    segment_dir: &std::path::Path,
    segment: &str,
    stream: Option<&str>,
    refresh: bool,
) -> Result<Vec<&'a TalentConfig>, String> {
    let mut selected = Vec::new();
    let source = FilesystemHealthLogSource::new(&context.journal);
    for name in SEGMENT_FLOOR_TALENTS {
        let Some(config) = by_name.get(*name) else {
            log_skip(log, context, name, segment, "no_config", stream);
            continue;
        };
        let unit = solstone_core_system::work_retry::Unit::Segment {
            day: context.day.clone(),
            stream: stream.unwrap_or(DEFAULT_STREAM).into(),
            segment: segment.into(),
            talent: (*name).into(),
            direct: segment_dir.parent() == Some(context.day_dir.as_path()),
        };
        let legacy = is_floor_talent_capped(&source, &context.day, stream, segment, name)
            .map_err(|e| e.to_string())?;
        if legacy.malformed_line_count != 0 {
            return Err("floor failure evidence is unreadable".into());
        }
        if legacy.value
            && solstone_core_system::work_retry::read(&context.journal, &unit.id()?)?.is_none()
        {
            let input = solstone_core_system::work_retry::input::current(&context.journal, &unit)?;
            solstone_core_system::work_retry::observe_held(
                &context.journal,
                &unit,
                "floor_failed",
                &input,
            )?;
        }
        if !refresh
            && solstone_core_system::work_retry::read(&context.journal, &unit.id()?)?
                .is_some_and(|r| r.held)
        {
            log_skip(log, context, name, segment, "capped", stream);
            continue;
        }
        selected.push(config);
    }
    if let Some(config) = by_name.get("entities:detection") {
        selected.push(config);
    } else {
        log_skip(
            log,
            context,
            "entities:detection",
            segment,
            "no_config",
            stream,
        );
    }
    let possible_names = solstone_core_talent_runtime::possible_names::TALENT;
    if !solstone_core_talent_runtime::possible_names::has_speech_transcript(segment_dir) {
        log_skip(
            log,
            context,
            possible_names,
            segment,
            "no_transcript",
            stream,
        );
    } else if let Some(config) = by_name.get(possible_names) {
        selected.push(config);
    } else {
        log_skip(log, context, possible_names, segment, "no_config", stream);
    }
    let recommend = sense
        .get("recommend")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if recommend.get("screen_record").is_some_and(python_truthy) {
        if let Some(config) = by_name.get("screen") {
            selected.push(config);
        } else {
            log_skip(log, context, "screen", segment, "no_config", stream);
        }
    } else {
        log_skip(log, context, "screen", segment, "not_recommended", stream);
    }
    // Every segment with voice embeddings gets its speakers resolved: the owner's voice
    // and known voiceprints need no model. Sense's recommendation decides only whether
    // the model then reads what they left unmatched (the talent's own pre-step).
    if has_audio_embeddings(segment_dir) {
        if let Some(config) = by_name.get("speaker_attribution") {
            selected.push(config);
        } else {
            log_skip(
                log,
                context,
                "speaker_attribution",
                segment,
                "no_config",
                stream,
            );
        }
    } else {
        log_skip(
            log,
            context,
            "speaker_attribution",
            segment,
            "no_audio_embeddings",
            stream,
        );
    }
    if !refresh {
        let mut kept = Vec::with_capacity(selected.len());
        for config in selected {
            let gating = !SEGMENT_FLOOR_TALENTS.contains(&config.key.as_str())
                && !SEGMENT_NONGATING_TALENTS.contains(&config.key.as_str());
            if gating && held_aside(context, segment_dir, segment, stream, &config.key)? {
                log_skip(log, context, &config.key, segment, "set_aside", stream);
            } else {
                kept.push(config);
            }
        }
        selected = kept;
    }
    Ok(selected)
}

/// The retry identity of one talent of this segment.
fn segment_talent_unit(
    context: &ThinkContext,
    segment_dir: &std::path::Path,
    segment: &str,
    stream: Option<&str>,
    talent: &str,
) -> solstone_core_system::work_retry::Unit {
    solstone_core_system::work_retry::Unit::Segment {
        day: context.day.clone(),
        stream: stream.unwrap_or(DEFAULT_STREAM).into(),
        segment: segment.into(),
        talent: talent.into(),
        direct: segment_dir.parent() == Some(context.day_dir.as_path()),
    }
}

/// A talent set aside after failing again and again on the same input waits
/// for its own retry: an ordinary run neither repeats nor abandons it.
fn held_aside(
    context: &ThinkContext,
    segment_dir: &std::path::Path,
    segment: &str,
    stream: Option<&str>,
    talent: &str,
) -> Result<bool, String> {
    let unit = segment_talent_unit(context, segment_dir, segment, stream, talent);
    if context
        .retry
        .as_ref()
        .is_some_and(|(selected, _)| selected == &unit)
    {
        return Ok(false);
    }
    Ok(
        solstone_core_system::work_retry::read(&context.journal, &unit.id()?)?
            .is_some_and(|record| record.held),
    )
}

fn has_audio_embeddings(segment_dir: &std::path::Path) -> bool {
    std::fs::read_dir(segment_dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let path = entry.path();
            path.extension().and_then(|extension| extension.to_str()) == Some("npz")
                && path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .is_some_and(|stem| stem == "audio" || stem.ends_with("_audio"))
        })
    })
}

fn is_retryable_capacity_exhausted(
    journal: &std::path::Path,
    use_id: &str,
    outcome: &DrainOutcome,
) -> bool {
    if !matches!(outcome, DrainOutcome::Fail { state, .. } if *state == "error") {
        return false;
    }
    let Ok(events) = solstone_core_cortex_client::read_use_events(journal, use_id) else {
        return false;
    };
    let Some(event) = events
        .iter()
        .rev()
        .find(|event| event.get("terminal").and_then(Value::as_bool) == Some(true))
    else {
        return false;
    };
    event.get("event").and_then(Value::as_str) == Some("error")
        && event.get("reason_code").and_then(Value::as_str) == Some("local_capacity_exhausted")
        && event.get("retryable").and_then(Value::as_bool) == Some(true)
}

#[allow(clippy::too_many_arguments)]
fn drain_selected(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: &mut Vec<PendingUse>,
    timeout: Option<Duration>,
    total: &mut ModeResult,
    by_name: &BTreeMap<String, TalentConfig>,
    segment: &str,
    refresh: bool,
    stream: Option<&str>,
    live: bool,
    skip_talents: &[String],
    log: &mut RunLogWriter,
) {
    if pending.is_empty() {
        return;
    }
    let mut observed_outcomes = Vec::new();
    let batch_result = drain_with_failure_policy(
        context,
        runtime,
        std::mem::take(pending),
        timeout,
        &mut |item, outcome| {
            log_use_terminal(log, context, segment, stream, item, outcome.clone());
            let label = crate::dispatch::item_label(&item.name, item.facet.as_deref());
            observed_outcomes.push((label, item.name.clone(), item.use_id.clone(), outcome));
        },
        &|item| !SEGMENT_NONGATING_TALENTS.contains(&item.name.as_str()),
    );
    let mut retried_successes = Vec::new();
    let mut retried_failures_removed = Vec::new();
    let mut blocked = Vec::new();
    for (label, name, use_id, outcome) in observed_outcomes {
        if matches!(outcome, DrainOutcome::Fail { .. })
            && !SEGMENT_NONGATING_TALENTS.contains(&name.as_str())
        {
            blocked.push((label.clone(), use_id.clone()));
        }
        if matches!(outcome, DrainOutcome::Fail { .. })
            && is_retryable_capacity_exhausted(&context.journal, &use_id, &outcome)
            && let Some(config) = by_name.get(&name)
            && let Ok(AgentDispatch::Pending(retry_use)) = dispatch_agent(
                context,
                runtime,
                config,
                segment,
                refresh,
                stream,
                live,
                skip_talents,
                log,
            )
        {
            log_dispatch(log, context, segment, stream, &retry_use);
            let retry_result = drain_with_deadline_observed(
                context,
                runtime,
                vec![retry_use],
                timeout,
                &mut |r_item, r_outcome| {
                    log_use_terminal(log, context, segment, stream, r_item, r_outcome);
                },
            );
            total.timed_out |= retry_result.timed_out;
            if retry_result.failed == 0 && retry_result.success != 0 && !retry_result.timed_out {
                retried_failures_removed.push(label.clone());
                retried_successes.push(label);
            }
        }
    }
    let mut final_result = batch_result;
    final_result.provider_blocked |= blocked.iter().any(|(label, use_id)| {
        !retried_failures_removed.contains(label) && use_log_blocking(&context.journal, use_id)
    });
    for failed_label in retried_failures_removed {
        let prefix = format!("{failed_label} (");
        if let Some(pos) = final_result
            .failed_names
            .iter()
            .position(|n| n.source.starts_with(&prefix) || n.source == failed_label)
        {
            final_result.failed_names.remove(pos);
            final_result.failed = final_result.failed.saturating_sub(1);
        }
    }
    for success_label in retried_successes {
        final_result.success += 1;
        final_result.success_names.push(success_label);
    }
    merge_mode_result(total, final_result);
}

enum AgentDispatch {
    Skipped,
    Pending(PendingUse),
}

/// An admitted from-scratch request is one owner cause, even when dispatch
/// retries internally. Its durable id prevents those retries minting a budget.
fn floor_retry_attempt(
    context: &ThinkContext,
    unit: &solstone_core_system::work_retry::Unit,
    refresh: bool,
) -> Result<Option<solstone_core_system::work_retry::Attempt>, String> {
    use solstone_core_system::{work_intent, work_retry};
    if let Some((selected, attempt)) = &context.retry
        && selected == unit
    {
        return Ok(Some(attempt.clone()));
    }
    if refresh
        && let Some(intent) = work_intent::read(&context.journal)?.get(&context.day)
        && intent.from_scratch
        && intent.state == work_intent::IntentState::Claimed
        && work_retry::read(&context.journal, &unit.id()?)?.is_some_and(|r| r.held)
    {
        work_retry::request_with_id(&context.journal, unit, &intent.id)?;
        let input = work_retry::input::current(&context.journal, unit)?;
        return Ok(work_retry::read(&context.journal, &unit.id()?)?
            .and_then(|r| r.permission(&input, work_retry::DECLARED_FIXES)));
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn dispatch_agent(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    config: &TalentConfig,
    segment: &str,
    refresh: bool,
    stream: Option<&str>,
    live: bool,
    skip_talents: &[String],
    log: &mut RunLogWriter,
) -> Result<AgentDispatch, DispatchFailure> {
    if skip_talents.iter().any(|name| name == &config.key) {
        // Source-derived, not measured: thinking.py:1412-1421 skips names
        // requested through `--skip-talents` without dispatching them.
        log_skip(
            log,
            context,
            &config.key,
            segment,
            "skip_talents_flag",
            stream,
        );
        return Ok(AgentDispatch::Skipped);
    }
    if config.metadata.get("new_only").is_some_and(python_truthy) && !live {
        // Source-derived, not measured: thinking.py:1423-1433 gates raw
        // Python-truthy `new_only` values on the live current-segment run.
        log_skip(
            log,
            context,
            &config.key,
            segment,
            "new_only_historical",
            stream,
        );
        return Ok(AgentDispatch::Skipped);
    }
    // A held talent's own retry carries its permission to the writer.
    let retry_target = context.retry.as_ref().is_some_and(|(selected, _)| {
        matches!(
            selected,
            solstone_core_system::work_retry::Unit::Segment { segment: held, talent, .. }
                if held == segment && talent == &config.key
        )
    });
    let retry_unit = if SEGMENT_FLOOR_TALENTS.contains(&config.key.as_str()) || retry_target {
        let directory = find_segment_dir(&context.journal, &context.day, segment, stream)
            .ok_or(DispatchFailure::Unavailable)?;
        Some(segment_talent_unit(
            context,
            &directory,
            segment,
            stream,
            &config.key,
        ))
    } else {
        None
    };
    let generate = config.metadata.get("type").and_then(Value::as_str) == Some("generate");
    let mut request = Map::from_iter([
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("schedule".to_owned(), Value::String("segment".to_owned())),
    ]);
    if let Some(unit) = &retry_unit {
        request.insert(
            "_work_unit".into(),
            serde_json::to_value(unit).map_err(|_| DispatchFailure::Unavailable)?,
        );
        if let Some(attempt) =
            floor_retry_attempt(context, unit, refresh).map_err(|_| DispatchFailure::Unavailable)?
        {
            request.insert(
                "_work_retry".into(),
                serde_json::to_value(attempt).map_err(|_| DispatchFailure::Unavailable)?,
            );
        }
    }
    let output_format = if generate {
        let format = config
            .metadata
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("md");
        request.insert("output".to_owned(), Value::String(format.to_owned()));
        if refresh {
            request.insert("refresh".to_owned(), Value::Bool(true));
        }
        Some(format)
    } else if let Some(output) = config.metadata.get("output") {
        request.insert("output".to_owned(), output.clone());
        output.as_str()
    } else {
        None
    };
    if let Some(format) = output_format {
        let path = get_output_path(
            &context.day_dir,
            &config.key,
            Some(segment),
            Some(format),
            None,
            stream,
        );
        request.insert(
            "output_path".to_owned(),
            Value::String(path.to_string_lossy().to_string()),
        );
    }
    if let Some(stream) = stream {
        request.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    // Source-derived, not measured: the flat cortex request keeps an explicit
    // talent provider/model override when frontmatter supplies either field.
    for field in ["provider", "model"] {
        if let Some(value) = config.metadata.get(field) {
            request.insert(field.to_owned(), value.clone());
        }
    }
    let mut env = Map::from_iter([
        ("SOL_DAY".to_owned(), Value::String(context.day.clone())),
        ("SOL_SEGMENT".to_owned(), Value::String(segment.to_owned())),
    ]);
    if let Some(stream) = stream {
        env.insert("SOL_STREAM".to_owned(), Value::String(stream.to_owned()));
    }
    request.insert("env".to_owned(), Value::Object(env));
    dispatch_direct(
        context,
        runtime,
        &config.key,
        if generate {
            String::new()
        } else {
            format!("Running scheduled task for {}.", context.day)
        },
        request,
        None,
    )
    .map(AgentDispatch::Pending)
}

/// Run selected repairs with the reference's bounded segment-level worker pool.
/// Workers share one invocation-owned log; the shared cortex allocator remains
/// locked, so equal-millisecond concurrent dispatches still receive unique use ids.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_repair_batch(
    context: &ThinkContext,
    log: &RunLogWriter,
    segments: Vec<(String, Option<String>)>,
    refresh: bool,
    max_concurrency: i64,
    segment_workers: usize,
    timeout: Option<Duration>,
    skip_talents: Vec<String>,
    current: CurrentSegment,
) -> Result<ModeResult, String> {
    if segments.is_empty() {
        return Ok(ModeResult::default());
    }
    let workers = segment_workers.clamp(1, segments.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(segments)));
    let aggregate = Arc::new(Mutex::new(ModeResult::default()));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let queue = Arc::clone(&queue);
            let aggregate = Arc::clone(&aggregate);
            let skip_talents = &skip_talents;
            let mut log = log.clone_for_shared_writes();
            scope.spawn(move || {
                loop {
                    // A provider that refused as blocking is unusable for now:
                    // the rest of the batch waits for the next run, which the
                    // task queue spaces out while thinking keeps failing.
                    if aggregate
                        .lock()
                        .expect("repair result lock")
                        .provider_blocked
                    {
                        break;
                    }
                    let Some((segment, stream)) =
                        queue.lock().expect("repair queue lock").pop_front()
                    else {
                        break;
                    };
                    match run_with(
                        context,
                        &mut log,
                        &segment,
                        refresh,
                        stream.as_deref(),
                        max_concurrency,
                        timeout,
                        false,
                        skip_talents,
                        current,
                    ) {
                        Ok(result) => {
                            merge(&mut aggregate.lock().expect("repair result lock"), result)
                        }
                        Err(error) => {
                            // Source-derived, not measured: thinking.py:677-682
                            // folds one worker exception into that segment's failure
                            // while the remaining repairs continue.
                            let mut aggregate = aggregate.lock().expect("repair result lock");
                            aggregate.failed += 1;
                            aggregate
                                .failed_names
                                .push(format!("{segment} ({error})").into());
                        }
                    }
                }
            });
        }
    });
    let left = queue.lock().expect("repair queue lock").len();
    let result = aggregate.lock().expect("repair result lock").clone();
    if result.provider_blocked && left != 0 {
        log::warn!("{}", provider_wait_warning(context.terminal_locale, left));
    }
    Ok(result)
}

fn provider_wait_warning(terminal_locale: Option<&str>, left: usize) -> String {
    use solstone_core_cli_copy::{ENGLISH, Value, render_for};

    match terminal_locale {
        Some(locale) => render_for(
            locale,
            "journal.cli.think.log.provider",
            &[("left", Value::Cardinal(left as i64))],
        ),
        None => render_for(
            ENGLISH,
            "journal.cli.think.log.provider.record",
            &[("left", Value::String(&left.to_string()))],
        ),
    }
}

#[cfg(test)]
#[test]
fn provider_warning_preserves_supervised_log_grammar() {
    use solstone_core_cli_copy::ENGLISH;
    assert_eq!(
        provider_wait_warning(None, 1),
        "thinking provider is unavailable; 1 segments wait for the next run"
    );
    assert_eq!(
        provider_wait_warning(Some(ENGLISH), 1),
        "thinking provider is unavailable; 1 segment waits for the next run"
    );
    assert_eq!(
        provider_wait_warning(None, 2),
        provider_wait_warning(Some(ENGLISH), 2)
    );
}

/// Run the maintained segment-batch operation through its activity-state tail.
///
/// Both the direct `--segments` surface and whole-day catchup use this seam so
/// durable Sense output is replayed exactly once after every completed repair
/// batch.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_repair_batch_with_activity(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segments: Vec<(String, Option<String>)>,
    refresh: bool,
    max_concurrency: i64,
    segment_workers: usize,
    timeout: Option<Duration>,
    skip_talents: Vec<String>,
    no_activity_prompts: bool,
    current: CurrentSegment,
) -> Result<ModeResult, String> {
    let mut selected = std::collections::BTreeSet::new();
    for (segment, stream) in &segments {
        if let Some(dir) = resolve_segment_dir(
            context.locale(),
            &context.journal,
            &context.day,
            segment,
            stream.as_deref(),
        )? {
            selected.insert((
                segment.clone(),
                Some(
                    named_stream(&dir, &context.day)
                        .unwrap_or(DEFAULT_STREAM)
                        .to_owned(),
                ),
            ));
        }
    }
    let replay_segments = stream_day_segments(context, &selected)?;
    run_repair_batch(
        context,
        log,
        segments,
        refresh,
        max_concurrency,
        segment_workers,
        timeout,
        skip_talents,
        current,
    )
    .map(|mut result| {
        if let Err(error) = replay_activity_state_selected(
            context,
            log,
            &replay_segments,
            refresh,
            max_concurrency,
            no_activity_prompts,
            false,
            Some(&selected),
        ) {
            crate::dispatch::record_followup_failure(&mut result, "activity replay", &error);
        }
        let streams = selected
            .iter()
            .filter_map(|(_, stream)| stream.clone())
            .collect::<BTreeSet<_>>();
        for stream in streams {
            if crate::settle::defers(Some(&stream), refresh)
                && let Err(error) = crate::settle::check(
                    context,
                    log,
                    &stream,
                    false,
                    no_activity_prompts,
                    max_concurrency,
                )
            {
                crate::dispatch::record_followup_failure(&mut result, "activity settle", &error);
            }
        }
        result
    })
}

/// Every segment of the streams `selected` names on the context's day, in
/// capture order, with the selection itself.
///
/// Rebuilding activity boundaries needs the whole stream: starting at a middle
/// segment would give part of an already published activity a new identity.
fn stream_day_segments(
    context: &ThinkContext,
    selected: &std::collections::BTreeSet<(String, Option<String>)>,
) -> Result<Vec<(String, Option<String>)>, String> {
    let streams = selected
        .iter()
        .map(|(_, stream)| stream.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut segments = selected.iter().cloned().collect::<Vec<_>>();
    if !streams.is_empty() {
        for entry in iter_segments(&context.journal, PathOrDay::Day(&context.day))
            .map_err(|error| error.to_string())?
        {
            let stream = Some(stream_coordinate(entry.path(), &context.day));
            if streams.contains(&stream) {
                segments.push((entry.name().to_string_lossy().into_owned(), stream));
            }
        }
        segments.sort();
        segments.dedup();
    }
    Ok(segments)
}

/// The stream coordinate of a discovered segment directory: its named stream,
/// or the journal's name for the direct-under-day layout.
pub(crate) fn stream_coordinate(segment_dir: &std::path::Path, day: &str) -> String {
    named_stream(segment_dir, day)
        .unwrap_or(DEFAULT_STREAM)
        .to_owned()
}

/// Replay durable Sense output through the activity-state tail.
///
/// Live callers (`hydrate_existing`) advance each stream's saved state in
/// capture order (see [`advance_live_activity_state`]); a batch replay starts
/// every stream fresh and never writes state. Either way an activity that ends
/// is published, and its talent work recorded, before any talent runs: the
/// facets crate owns the append-only record, and `activity_work` owns resuming
/// talent work a stopped run left unfinished.
pub(crate) fn replay_activity_state(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segments: &[(String, Option<String>)],
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
    hydrate_existing: bool,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if hydrate_existing {
        let mut ordered = segments.to_vec();
        ordered.sort();
        errors.extend(ordered.into_iter().filter_map(|(segment, stream)| {
            advance_live_activity_state(
                context,
                log,
                &segment,
                stream.as_deref(),
                refresh,
                max_concurrency,
                skip_activity_prompts,
            )
            .err()
        }));
    } else if let Err(error) = replay_activity_state_selected(
        context,
        log,
        segments,
        refresh,
        max_concurrency,
        skip_activity_prompts,
        false,
        None,
    ) {
        errors.push(error);
    }
    // Write what has settled on each stream the replay touched.
    let streams = segments
        .iter()
        .filter_map(|(segment, stream)| {
            find_segment_dir(&context.journal, &context.day, segment, stream.as_deref())
                .map(|dir| stream_coordinate(&dir, &context.day))
        })
        .collect::<BTreeSet<_>>();
    for stream in streams {
        if crate::settle::defers(Some(&stream), refresh)
            && let Err(error) = crate::settle::check(
                context,
                log,
                &stream,
                false,
                skip_activity_prompts,
                max_concurrency,
            )
        {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// How long the live tail waits for an earlier segment of the same stream
/// that has arrived but has no Sense yet, counted from its arrival or from the
/// stream's latest Sense, whichever is newer. Deliveries land out of order (a
/// phone relays a watch's backlog newest first), and applying a newer segment
/// first would end or start activities at the wrong place; while the backlog
/// is still being thought, the wait goes on.
const LIVE_ORDER_WAIT_MS: i64 = 15 * 60 * 1000;

/// The longest one earlier segment can hold its stream back, however busy the
/// stream stays: a segment that never gets Sense must not hold it all day.
/// Past either limit the tail goes on, and the straggler is late evidence.
pub(crate) const LIVE_ORDER_HOLD_MS: i64 = 2 * 60 * 60 * 1000;

/// What the live tail does for one stream.
#[derive(Debug, PartialEq, Eq)]
enum LiveStep {
    /// Apply these segments to the saved state, in capture order.
    Advance(Vec<String>),
    /// The segment is not newer than the stream's saved one.
    Late,
}

pub(crate) fn modified_ms(path: &std::path::Path) -> Option<i64> {
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    i64::try_from(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis(),
    )
    .ok()
}

/// A stream's segments on the context's day after `after`, in capture order,
/// each with when its Sense was written (`None` while it has none).
fn stream_segments_after(
    context: &ThinkContext,
    state_stream: Option<&str>,
    after: Option<&str>,
) -> Result<Vec<(String, std::path::PathBuf, Option<i64>)>, String> {
    let mut segments = iter_segments(&context.journal, PathOrDay::Day(&context.day))
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|entry| named_stream(entry.path(), &context.day) == state_stream)
        .map(|entry| {
            let key = entry.name().to_string_lossy().into_owned();
            let sensed = modified_ms(&entry.path().join("talents/sense.json"));
            (key, entry.path().to_path_buf(), sensed)
        })
        .filter(|(key, _, _)| after.is_none_or(|after| key.as_str() > after))
        .collect::<Vec<_>>();
    segments.sort();
    Ok(segments)
}

/// Decide, from a stream's hydrated state, which of its segments the live
/// tail applies now, in capture order: every segment after the saved one up to
/// this one that has Sense, and the later ones whose Sense was written before
/// this one's (they waited for it), stopping before a segment that arrived but
/// is still being thought (when `wait`). With no saved state, that starts at
/// the beginning of the day.
fn plan_live_step(
    context: &ThinkContext,
    machine: &ActivityStateMachine,
    segment: &str,
    state_stream: Option<&str>,
    wait: bool,
) -> Result<LiveStep, String> {
    let last = machine.last_segment_day().zip(machine.last_segment_key());
    if last.is_some_and(|last| (context.day.as_str(), segment) <= last) {
        return Ok(LiveStep::Late);
    }
    let after = last.filter(|last| last.0 == context.day).map(|last| last.1);
    let segments = stream_segments_after(context, state_stream, after)?;
    let own_sense = segments
        .iter()
        .find(|(key, _, _)| key == segment)
        .and_then(|(_, _, sensed)| *sensed);
    // Another held segment's recent Sense means its backlog is still moving;
    // this segment's own Sense, just written, says nothing about that.
    let latest_sense = segments
        .iter()
        .filter(|(key, _, _)| key != segment)
        .filter_map(|(_, _, sensed)| *sensed)
        .max();
    let within = |since: i64, limit: i64| context.now_ms.saturating_sub(since) < limit;
    let mut keys = Vec::new();
    for (key, path, sensed) in segments {
        if let Some(sensed) = sensed {
            // A later segment thought first is waiting for this one; one
            // thought since will apply itself.
            if key.as_str() <= segment || own_sense.is_none_or(|own| sensed < own) {
                keys.push(key);
            }
            continue;
        }
        if key == segment {
            keys.push(key);
            continue;
        }
        let still_coming = newest_segment_input_ms(&path)
            .ok()
            .flatten()
            .is_some_and(|arrived| {
                within(arrived, LIVE_ORDER_HOLD_MS)
                    && within(
                        latest_sense.map_or(arrived, |latest| latest.max(arrived)),
                        LIVE_ORDER_WAIT_MS,
                    )
            });
        if wait && still_coming {
            break;
        }
    }
    Ok(LiveStep::Advance(keys))
}

/// The live activity tail for one thought segment.
///
/// In order, the segment (and any later ones of its stream that were waiting
/// for it) advances the saved state, after anything of the stream's previous
/// day that was still waiting. A late segment never rewinds it: it joins the
/// open activity it falls inside, and otherwise its stream's day is rebuilt in
/// capture order like a repair, which publishes a finished activity that holds
/// it only under an ID not already written. What a published activity already
/// holds stays as written.
fn advance_live_activity_state(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    stream: Option<&str>,
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<(), String> {
    let Some(segment_dir) = find_segment_dir(&context.journal, &context.day, segment, stream)
    else {
        return Ok(());
    };
    let state_stream = named_stream(&segment_dir, &context.day);
    // A workout is body data: no activity state follows Strava's tiles.
    if state_stream == Some(STRAVA_STREAM) {
        return Ok(());
    }
    let coordinate = stream_coordinate(&segment_dir, &context.day);
    let mut errors = Vec::new();
    // A new day ends the previous day's activities; first apply what of that
    // day was still waiting for an earlier segment.
    let previous_day = {
        let _turn = state_turn(context, state_stream);
        let machine = ActivityStateMachine::hydrate(Some(&context.journal), state_stream);
        match machine.last_segment_day().zip(machine.last_segment_key()) {
            Some((day, last)) if day < context.day.as_str() => {
                let day_context = activity_context(context, day)?;
                let keys = stream_segments_after(&day_context, state_stream, Some(last))?
                    .into_iter()
                    .filter(|(_, _, sensed)| sensed.is_some())
                    .map(|(key, _, _)| (key, Some(coordinate.clone())))
                    .collect::<Vec<_>>();
                (!keys.is_empty()).then_some((day_context, keys))
            }
            _ => None,
        }
    };
    if let Some((day_context, keys)) = previous_day
        && let Err(error) = replay_activity_state_selected(
            &day_context,
            log,
            &keys,
            refresh,
            max_concurrency,
            skip_activity_prompts,
            true,
            None,
        )
    {
        errors.push(error);
    }
    let step = {
        let _turn = state_turn(context, state_stream);
        let mut machine = ActivityStateMachine::hydrate(Some(&context.journal), state_stream);
        let step = plan_live_step(context, &machine, segment, state_stream, true)?;
        if step == LiveStep::Late
            && join_open_activity(
                context,
                log,
                &mut machine,
                segment,
                &segment_dir,
                &coordinate,
            )?
        {
            match persist_activity_state(context, state_stream, &machine) {
                Ok(()) => log_routing_state(context, log, segment, Some(&coordinate), false)?,
                Err(error) => errors.push(solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.activity.state.segment",
                    [("segment", &(segment)), ("error", &(error))],
                )),
            }
            log::debug!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.activity.join",
                    [("day", &context.day), ("segment", &segment)]
                )
            );
            None
        } else {
            Some(step)
        }
    };
    let applied = match step {
        None => Ok(()),
        Some(LiveStep::Advance(keys)) => replay_activity_state_selected(
            context,
            log,
            &keys
                .into_iter()
                .map(|key| (key, Some(coordinate.clone())))
                .collect::<Vec<_>>(),
            refresh,
            max_concurrency,
            skip_activity_prompts,
            true,
            None,
        ),
        Some(LiveStep::Late) => {
            log::debug!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.activity.replay",
                    [("day", &context.day), ("segment", &segment)]
                )
            );
            let selected =
                std::collections::BTreeSet::from([(segment.to_owned(), Some(coordinate.clone()))]);
            replay_activity_state_selected(
                context,
                log,
                &stream_day_segments(context, &selected)?,
                refresh,
                max_concurrency,
                skip_activity_prompts,
                false,
                Some(&selected),
            )
        }
    };
    if let Err(error) = applied {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Add a late segment to the open activity it falls inside, if any.
fn join_open_activity(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    machine: &mut ActivityStateMachine,
    segment: &str,
    segment_dir: &std::path::Path,
    coordinate: &str,
) -> Result<bool, String> {
    let Ok(solstone_core_journal_io::durability::DurableRead::Present(mut sense)) =
        solstone_core_journal_io::durability::read_json_durable::<Value>(
            solstone_core_journal_io::durability::ArtifactId::SegmentSense,
            &segment_dir.join("talents/sense.json"),
        )
    else {
        return Ok(false);
    };
    if !valid_activity_sense(&sense) {
        return Ok(false);
    }
    let inventory =
        observe_declared_facet_inventory(&context.journal).map_err(|error| error.to_string())?;
    if let Some(object) = sense.as_object_mut() {
        let (facets, unreadable) = filter_declared_facets(
            object.get("facets"),
            &inventory,
            context,
            log,
            segment,
            Some(coordinate),
            true,
        )?;
        if unreadable {
            return Ok(false);
        }
        object.insert("facets".to_owned(), facets);
    }
    Ok(machine.join_late(&sense, segment, &context.day))
}

#[allow(clippy::too_many_arguments)]
fn replay_activity_state_selected(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segments: &[(String, Option<String>)],
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
    hydrate_existing: bool,
    selected: Option<&std::collections::BTreeSet<(String, Option<String>)>>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let mut resolved_routing = Vec::new();
    let mut ordered = segments.to_vec();
    ordered.sort();
    // One machine per stream on both paths: a segment advances only its own
    // stream's activities. The live path starts each from that stream's
    // snapshot, holding the stream's turn until its state is written back; a
    // batch replay starts each fresh and never writes one.
    let mut machines = BTreeMap::new();
    let mut turns = Vec::new();
    let mut talents = Vec::new();
    let mut backfill_cache = BackfillDefaultsCache::default();
    let inventory =
        observe_declared_facet_inventory(&context.journal).map_err(|error| error.to_string())?;
    for (segment, stream) in ordered {
        let Some(segment_dir) =
            find_segment_dir(&context.journal, &context.day, &segment, stream.as_deref())
        else {
            continue;
        };
        let stream = stream.or_else(|| named_stream(&segment_dir, &context.day).map(str::to_owned));
        // The snapshot file is named from the directory the segment sits in,
        // never from the caller's stream argument, so it stays in the journal.
        let state_stream = named_stream(&segment_dir, &context.day);
        if state_stream == Some(STRAVA_STREAM) {
            continue;
        }
        let selected_coordinate =
            selected.is_none_or(|keys| keys.contains(&(segment.clone(), stream.clone())));
        let sense_path = segment_dir.join("talents/sense.json");
        let sense = match solstone_core_journal_io::durability::read_json_durable::<Value>(
            solstone_core_journal_io::durability::ArtifactId::SegmentSense,
            &sense_path,
        ) {
            Ok(solstone_core_journal_io::durability::DurableRead::Present(sense)) => sense,
            _ => continue,
        };
        if !valid_activity_sense(&sense) {
            continue;
        }
        let machine = match machines.entry(stream.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) if hydrate_existing => {
                turns.extend(state_turn(context, state_stream));
                context.checkpoint()?;
                let machine = ActivityStateMachine::hydrate(Some(&context.journal), state_stream);
                #[cfg(test)]
                state_probe::at(&context.journal, "hydrated");
                entry.insert(machine)
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ActivityStateMachine::default())
            }
        };
        // A direct re-think or late live delivery can revisit old evidence.
        // Compare under the stream's turn, before an idle input or day change
        // can end today's activities. Historical activity replay belongs to
        // the full-context repair path, which never writes live state.
        if hydrate_existing
            && machine
                .last_segment_day()
                .zip(machine.last_segment_key())
                .is_some_and(|last| (context.day.as_str(), segment.as_str()) <= last)
        {
            log::debug!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.activity.no.advance",
                    [("day", &context.day), ("segment", &segment)]
                )
            );
            continue;
        }
        // An activity that ends in the first segment of a new day belongs to
        // the day it ran on, so read that day before the update moves it.
        let routing_day = machine
            .last_segment_day()
            .unwrap_or(&context.day)
            .to_owned();
        let mut filtered_sense = sense.clone();
        let mut routing_unreadable = false;
        if let Some(obj) = filtered_sense.as_object_mut() {
            let filtered = filter_declared_facets(
                obj.get("facets"),
                &inventory,
                context,
                log,
                &segment,
                stream.as_deref(),
                selected_coordinate,
            )?;
            routing_unreadable = filtered.1;
            let merged = merge_folder_owner_facet(&segment_dir, filtered.0, &inventory);
            obj.insert("facets".to_owned(), merged);
        }
        let scope = segment_coalesce_scope(&context.journal, &segment_dir, stream.as_deref());
        let changes = machine.update_with_scope(
            &filtered_sense,
            &segment,
            &context.day,
            None,
            context.now_ms,
            &scope,
        );
        // Publish what ended before saving state that no longer holds it: a
        // run stopped in between leaves the activity open in the snapshot, so
        // the stream's next segment or flush ends it again and the append
        // finds it already there.
        // A live stream's activities are written once its evidence settles,
        // from its day's capture-order rebuild; here the days are only marked.
        let published =
            if crate::settle::defers_in(context, &routing_day, stream.as_deref(), refresh) {
                crate::settle::note(
                    context,
                    stream.as_deref().unwrap_or(DEFAULT_STREAM),
                    &[routing_day.as_str(), context.day.as_str()],
                )
            } else {
                publish_ended_activities(
                    context,
                    log,
                    &segment,
                    &routing_day,
                    state_stream,
                    changes,
                    &selected_completed(machine, stream.as_deref(), selected),
                    refresh,
                    skip_activity_prompts,
                    &mut talents,
                    &mut backfill_cache,
                )
            };
        #[cfg(test)]
        if hydrate_existing {
            state_probe::at(&context.journal, "published");
        }
        if hydrate_existing
            && let Err(error) = persist_activity_state(context, state_stream, machine)
        {
            errors.push(solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.activity.state.segment",
                [("segment", &(segment)), ("error", &(error))],
            ));
        }
        if let Err(error) = published {
            errors.push(error);
        } else if selected_coordinate && !routing_unreadable {
            resolved_routing.push((segment.clone(), stream.clone()));
        }
        if selected_coordinate && routing_unreadable {
            errors.push(format!(
                "{segment}: facet declaration is unreadable; routing remains pending"
            ));
        }
    }
    if !hydrate_existing
        && let Err(error) = flush_replay_machines(
            context,
            log,
            machines,
            selected,
            refresh,
            skip_activity_prompts,
            &mut talents,
            &mut backfill_cache,
        )
    {
        errors.push(error);
    }
    // Talents can run for minutes; no other writer waits on them.
    drop(turns);
    if let Err(error) = run_activity_talents(context, log, talents, refresh, max_concurrency) {
        errors.push(error);
    }
    if errors.is_empty() {
        for (segment, stream) in resolved_routing {
            log_routing_state(context, log, &segment, stream.as_deref(), false)?;
        }
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn selected_completed(
    machine: &ActivityStateMachine,
    stream: Option<&str>,
    selected: Option<&std::collections::BTreeSet<(String, Option<String>)>>,
) -> Vec<Value> {
    machine
        .completed_activities()
        .into_iter()
        .filter(|record| {
            selected.is_none_or(|keys| {
                record
                    .get("segments")
                    .and_then(Value::as_array)
                    .is_some_and(|segments| {
                        segments.iter().filter_map(Value::as_str).any(|segment| {
                            keys.contains(&(segment.to_owned(), stream.map(str::to_owned)))
                        })
                    })
            })
        })
        .collect()
}

/// The predecessor's Sense, marked as continued, when this segment adds
/// nothing to it: the screen is unchanged against the true predecessor, every
/// monitor's description was carried from it whole, and neither segment holds
/// speech. Anything else, including a missing or partial predecessor Sense,
/// returns `None` and Sense runs.
fn sense_continuation(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    segment_dir: &std::path::Path,
) -> Option<Map<String, Value>> {
    let stream = stream.unwrap_or(DEFAULT_STREAM);
    let predecessor = resolve_predecessor(&context.journal, &context.day, Some(stream), segment)?;
    let previous_name = predecessor.get("segment")?.as_str()?.to_owned();
    let change = detect_segment_change(
        &context.journal,
        &context.day,
        Some(stream),
        segment,
        segment_dir,
        Some(predecessor),
        "",
    );
    if change.get("change_class").and_then(Value::as_str) != Some("redundant")
        || change["sensors"]["transcript"]["present"].as_bool() != Some(false)
        || !every_screen_carried_whole(segment_dir)
    {
        return None;
    }
    let previous_dir = iter_segments(&context.journal, PathOrDay::Day(&context.day))
        .ok()?
        .into_iter()
        .find_map(|entry| {
            (entry.stream().matches(stream) && entry.name().to_str() == Some(&previous_name))
                .then(|| entry.path().to_path_buf())
        })?;
    let read = |name: &str| -> Option<Value> {
        serde_json::from_slice(&std::fs::read(previous_dir.join("talents").join(name)).ok()?).ok()
    };
    // A predecessor that heard speech shaped its Sense around it, which this
    // silent segment cannot inherit.
    if read("change.json")?["sensors"]["transcript"]["present"].as_bool() != Some(false) {
        return None;
    }
    let previous = read("sense.json")?;
    if !valid_activity_sense(&previous) || previous.get("partial_input").is_some() {
        return None;
    }
    let mut continued = previous.as_object()?.clone();
    continued.insert("continuation_of".to_owned(), Value::String(previous_name));
    Some(continued)
}

/// Every screen in the segment holds exactly one row, carried unchanged from
/// the predecessor, so describe made no model call for any of them.
fn every_screen_carried_whole(segment_dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(segment_dir) else {
        return false;
    };
    let mut screens = 0;
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                solstone_core_format::content::segment_file_offers(
                    name,
                    solstone_core_format::content::SegmentReader::Talents,
                    Default::default(),
                )
                .iter()
                .any(|offer| {
                    offer.format
                        == solstone_core_format::content::FileFormat::Raw(
                            solstone_core_format::content::RawPerceptFamily::RawScreen,
                        )
                })
            })
        {
            continue;
        }
        screens += 1;
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return false;
        };
        let rows = contents
            .lines()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<Value>)
            .collect::<Result<Vec<_>, _>>();
        let Ok([row]) = rows.as_deref() else {
            return false;
        };
        if !row.get("carried_from").is_some_and(Value::is_object) {
            return false;
        }
    }
    screens > 0
}

/// A durable Sense projection can drive activity state once it says how
/// active the segment was and what kind of work it held; nothing else is
/// required.
pub(crate) fn valid_activity_sense(sense: &Value) -> bool {
    ["density", "content_type"]
        .into_iter()
        .all(|key| sense.get(key).is_some())
}

/// Close what a batch replay leaves open where nothing more will arrive: a
/// finite import stream, or any stream of a finished day. The current day's
/// ongoing streams stay open for live thinking and the idle flush.
#[allow(clippy::too_many_arguments)]
fn flush_replay_machines(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    mut machines: BTreeMap<Option<String>, ActivityStateMachine>,
    selected: Option<&std::collections::BTreeSet<(String, Option<String>)>>,
    refresh: bool,
    skip_activity_prompts: bool,
    talents: &mut Vec<EndedActivity>,
    backfill_cache: &mut BackfillDefaultsCache,
) -> Result<(), String> {
    let mut errors = Vec::new();
    // The owner's today, not UTC's: west of UTC, UTC's date turns over in the
    // evening, and that would read the live day as finished.
    let now = Timestamp::from_millisecond(context.now_ms).unwrap_or_else(|_| Timestamp::now());
    let today = solstone_core_system::daily_coverage::local_day(&context.journal, now);
    for (stream, machine) in &mut machines {
        let Some(last_segment) = machine.last_segment_key().map(ToOwned::to_owned) else {
            continue;
        };
        let backfill_stream = stream.as_deref().is_some_and(|name| {
            name.starts_with("import.")
                || solstone_core_segment::is_backfill_stream(&context.journal, name)
        });
        if !backfill_stream && context.day >= today {
            continue;
        }
        let routing_day = machine
            .last_segment_day()
            .unwrap_or(&context.day)
            .to_owned();
        let changes = machine.close_active(&last_segment, context.now_ms);
        let published =
            if crate::settle::defers_in(context, &routing_day, stream.as_deref(), refresh) {
                crate::settle::note(
                    context,
                    stream.as_deref().unwrap_or(DEFAULT_STREAM),
                    &[routing_day.as_str()],
                )
            } else {
                publish_ended_activities(
                    context,
                    log,
                    &last_segment,
                    &routing_day,
                    stream.as_deref().filter(|stream| *stream != DEFAULT_STREAM),
                    changes,
                    &selected_completed(machine, stream.as_deref(), selected),
                    refresh,
                    skip_activity_prompts,
                    talents,
                    backfill_cache,
                )
            };
        if let Err(error) = published {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Whether a segment file holds, or is, something that was spoken.
pub(crate) fn is_spoken_audio_file(name: &str) -> bool {
    name == "audio.jsonl" || name.ends_with("_audio.jsonl") || name.starts_with("audio.")
}

/// How a segment joins its stream's activities. What arrives from an import
/// or a syncing source is one item to an activity, unless it was spoken: a
/// recording runs on like any captured stream.
pub(crate) fn segment_coalesce_scope(
    journal: &std::path::Path,
    segment_dir: &std::path::Path,
    stream: Option<&str>,
) -> solstone_core_system::activity_state::CoalesceScope {
    use solstone_core_system::activity_state::CoalesceScope;
    if !stream.is_some_and(|name| solstone_core_segment::is_backfill_stream(journal, name)) {
        return CoalesceScope::ContinuousStream;
    }
    let spoken = std::fs::read_dir(segment_dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_name().to_str().is_some_and(is_spoken_audio_file))
    });
    if spoken {
        CoalesceScope::ContinuousStream
    } else {
        CoalesceScope::DiscreteItem
    }
}

/// Whether `stream` holds the owner's email.
fn holds_email(journal: &std::path::Path, stream: &str) -> bool {
    stream.starts_with("import.email") || solstone_core_segment::is_email_stream(journal, stream)
}

/// The people one line of a written exchange names as taking part, each with
/// the address the line gives for them and how the line places them: who
/// wrote it and, in email, who it was written to and whose message it
/// answers. Copied (`cc`) recipients are bystanders and are left out.
fn written_line_people(line: &Value, email: bool) -> Vec<(String, Option<String>, &'static str)> {
    let text = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let address = |value: Option<&Value>| {
        text(value)
            .map(|s| s.to_lowercase())
            .filter(|s| s.contains('@'))
    };
    let mut people = Vec::new();
    if let Some(sender) = text(
        line.get("sender")
            .or_else(|| line.get("author"))
            .or_else(|| line.get("speaker")),
    ) {
        let from = line.get("email");
        let from = address(from).or_else(|| address(from.and_then(|v| v.get("from"))));
        people.push((
            sender,
            from,
            solstone_core_talent_runtime::participation::EXCHANGE_SENDER_CONTEXT,
        ));
    }
    if !email {
        return people;
    }
    let addresses = line
        .get("email")
        .and_then(|e| e.get("to_addresses"))
        .and_then(Value::as_array);
    for (index, label) in line
        .get("to")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(label) = text(Some(label)) else {
            continue;
        };
        let to = address(addresses.and_then(|list| list.get(index)))
            .or_else(|| address(Some(&Value::String(label.clone()))));
        people.push((
            label,
            to,
            solstone_core_talent_runtime::participation::EXCHANGE_RECIPIENT_CONTEXT,
        ));
    }
    if let Some(reply) = line.get("reply_to").filter(|v| v.is_object()) {
        let from = address(reply.get("from"));
        if let Some(name) = text(reply.get("author")).or_else(|| from.clone()) {
            people.push((
                name,
                from,
                solstone_core_talent_runtime::participation::EXCHANGE_ANSWERED_CONTEXT,
            ));
        }
    }
    people
}

/// Every line of a segment's written exchange. Its first line is a header,
/// which names nobody.
fn written_exchange_lines(segment_dir: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(segment_dir.join("conversation_transcript.jsonl"))
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The account a synced email exchange was read from, which is the owner's
/// own address: the email source names it in the exchange's header.
fn exchange_account(lines: &[Value]) -> Option<String> {
    lines
        .first()?
        .get("email")?
        .get("account")?
        .as_str()
        .map(|account| account.trim().to_lowercase())
        .filter(|account| account.contains('@'))
}

/// Read owner email addresses from `config/email/account.json` (`account` and
/// `owner_aliases`), returning lowercased addresses containing `@`.
fn read_email_owner_aliases(journal: &std::path::Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(journal.join("config/email/account.json")) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    if let Some(account) = value.get("account").and_then(Value::as_str) {
        let lower = account.trim().to_lowercase();
        if lower.contains('@') {
            out.insert(lower);
        }
    }
    if let Some(aliases) = value.get("owner_aliases").and_then(Value::as_array) {
        for alias in aliases.iter().filter_map(Value::as_str) {
            let lower = alias.trim().to_lowercase();
            if lower.contains('@') {
                out.insert(lower);
            }
        }
    }
    out.into_iter().collect()
}

/// Pass-scoped cache for [`populate_backfill_activity_defaults_with_cache`] so
/// replaying many historical activities reads the journal entity identity map
/// and each facet's entities at most once instead of per activity.
#[derive(Default)]
struct BackfillDefaultsCache {
    loaded_owner: bool,
    owner_names: BTreeSet<String>,
    owner_emails: BTreeSet<String>,
    resolved_identities: Option<std::collections::HashMap<String, String>>,
    facet_candidates: BTreeMap<String, Vec<solstone_core_entity_matching::EntityNameCandidate>>,
}

impl BackfillDefaultsCache {
    fn ensure_owner(&mut self, journal: &std::path::Path) {
        if self.loaded_owner {
            return;
        }
        self.loaded_owner = true;
        for builtin in [
            "you",
            "me",
            "self",
            "user",
            "assistant",
            "system",
            "your agent",
        ] {
            self.owner_names.insert(builtin.to_owned());
        }
        // The name the owner gave the journal counts before the journal has
        // made an entity of them.
        if let Ok(bytes) = std::fs::read(journal.join("config/journal.json"))
            && let Ok(config) = serde_json::from_slice::<Value>(&bytes)
            && let Some(name) = config
                .get("identity")
                .and_then(|identity| identity.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
        {
            self.owner_names.insert(name.to_lowercase());
        }
        if let Ok(Some(principal)) = solstone_core_entity::read_journal_principal(journal) {
            if let Some(name) = principal.get("name").and_then(Value::as_str) {
                self.owner_names.insert(name.trim().to_lowercase());
            }
            if let Some(aka) = principal.get("aka").and_then(Value::as_array) {
                for a in aka.iter().filter_map(Value::as_str) {
                    self.owner_names.insert(a.trim().to_lowercase());
                }
            }
            if let Some(emails) = principal.get("emails").and_then(Value::as_array) {
                for e in emails.iter().filter_map(Value::as_str) {
                    self.owner_emails.insert(e.trim().to_lowercase());
                }
            }
        }
        for alias in read_email_owner_aliases(journal) {
            self.owner_names.insert(alias.clone());
            self.owner_emails.insert(alias);
        }
    }

    fn candidates_for_facet(
        &mut self,
        journal: &std::path::Path,
        facet: &str,
    ) -> &[solstone_core_entity_matching::EntityNameCandidate] {
        if !self.facet_candidates.contains_key(facet) {
            let resolved = self.resolved_identities.get_or_insert_with(|| {
                solstone_core_entity::read_identity_map(journal)
                    .map(|map| map.resolved)
                    .unwrap_or_default()
            });
            let scoped = solstone_core_facets::list_scoped_facet_entities_with_resolved_ids(
                journal, facet, true, true, resolved,
            )
            .unwrap_or_default();
            let candidates = scoped
                .iter()
                .map(|e| solstone_core_entity_matching::EntityNameCandidate {
                    id: Some(e.entity_id.clone()),
                    name: e
                        .identity
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    aka: e
                        .identity
                        .get("aka")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    emails: e
                        .identity
                        .get("emails")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_lowercase())
                        .collect(),
                })
                .collect();
            self.facet_candidates.insert(facet.to_owned(), candidates);
        }
        self.facet_candidates
            .get(facet)
            .map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
fn populate_backfill_activity_defaults(
    journal: &std::path::Path,
    day: &str,
    facet: &str,
    stream: &str,
    record: &mut Map<String, Value>,
) {
    let mut cache = BackfillDefaultsCache::default();
    populate_backfill_activity_defaults_with_cache(journal, day, facet, stream, record, &mut cache);
}

fn populate_backfill_activity_defaults_with_cache(
    journal: &std::path::Path,
    day: &str,
    facet: &str,
    stream: &str,
    record: &mut Map<String, Value>,
    cache: &mut BackfillDefaultsCache,
) {
    let Some(segments) = record.get("segments").and_then(Value::as_array).map(|arr| {
        arr.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    if segments.is_empty() {
        return;
    }

    let needs_title = record
        .get("title")
        .and_then(Value::as_str)
        .is_none_or(|t| t.trim().is_empty());
    if needs_title {
        let fallback_desc = record
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(120).collect::<String>())
            .unwrap_or_default();
        if !fallback_desc.is_empty() {
            record.insert("title".to_owned(), Value::String(fallback_desc));
        }
    }

    if record.contains_key("participation") {
        return;
    }

    cache.ensure_owner(journal);
    let _ = cache.candidates_for_facet(journal, facet);
    let owner_names = &cache.owner_names;
    let mut owner_emails = cache.owner_emails.clone();

    // The mailbox an exchange was synced from is the owner's, whether or not
    // the journal has that address on the owner's entity yet.
    let exchanges: BTreeMap<&str, (std::path::PathBuf, Vec<Value>)> = segments
        .iter()
        .filter_map(|seg| {
            let dir = find_segment_dir(journal, day, seg, Some(stream))?;
            let lines = written_exchange_lines(&dir);
            Some((seg.as_str(), (dir, lines)))
        })
        .collect();
    owner_emails.extend(
        exchanges
            .values()
            .filter_map(|(_, lines)| exchange_account(lines)),
    );

    let candidates = cache
        .facet_candidates
        .get(facet)
        .map_or(&[][..], Vec::as_slice);

    let resolve_id = |name: &str, email: Option<&str>| -> Option<String> {
        if let Some(em) = email {
            let em_low = em.trim().to_lowercase();
            if let Some(cand) = candidates
                .iter()
                .find(|c| c.emails.iter().any(|ce| ce == &em_low))
            {
                return cand.id.clone();
            }
        }
        solstone_core_entity_matching::find_matching_entity(name, candidates, 90.0)
            .and_then(|m| candidates[m.candidate_index].id.clone())
    };

    let is_owner = |name: &str, email: Option<&str>| -> bool {
        let low = name.trim().to_lowercase();
        if low.is_empty() || owner_names.contains(&low) {
            return true;
        }
        if let Some(em) = email
            && owner_emails.contains(&em.trim().to_lowercase())
        {
            return true;
        }
        false
    };

    struct ParticipantDraft {
        name: String,
        role: &'static str,
        source: &'static str,
        confidence: f64,
        context: String,
        entity_id: Option<String>,
        rank: u8,
    }

    let mut participants: BTreeMap<String, ParticipantDraft> = BTreeMap::new();
    let mut upsert = |display: String,
                      email: Option<String>,
                      role: &'static str,
                      src: &'static str,
                      conf: f64,
                      ctx: String|
     -> bool {
        if is_owner(&display, email.as_deref()) {
            return false;
        }
        let rank = match role {
            "attendee" => 5,
            "heard" => 4,
            "corresponded" => 3,
            "visible" => 2,
            _ => 1,
        };
        let key = display.trim().to_lowercase();
        if key.is_empty() {
            return false;
        }
        let eid = resolve_id(&display, email.as_deref());
        match participants.get_mut(&key) {
            Some(existing) => {
                if rank > existing.rank {
                    *existing = ParticipantDraft {
                        name: display,
                        role,
                        source: src,
                        confidence: conf,
                        context: ctx,
                        entity_id: eid.or_else(|| existing.entity_id.clone()),
                        rank,
                    };
                } else if existing.entity_id.is_none() && eid.is_some() {
                    existing.entity_id = eid;
                }
            }
            None => {
                participants.insert(
                    key,
                    ParticipantDraft {
                        name: display,
                        role,
                        source: src,
                        confidence: conf,
                        context: ctx,
                        entity_id: eid,
                        rank,
                    },
                );
            }
        }
        true
    };

    let is_email_stream = holds_email(journal, stream);
    // An assistant's side of an imported chat is not someone the owner met.
    let is_assistant_chat = ["import.chatgpt", "import.claude", "import.gemini"]
        .iter()
        .any(|prefix| stream.starts_with(prefix));

    for seg in &segments {
        let Some((seg_dir, exchange_lines)) = exchanges.get(seg.as_str()) else {
            continue;
        };
        let has_image = seg_dir.join("image_transcript.md").is_file();
        let has_screenshot = seg_dir.join("screenshot_transcript.md").is_file();
        let is_real_photo = has_image && !has_screenshot;
        let mut is_spoken = false;

        // Who corresponded is read from an exchange's own record of who wrote
        // to whom, never from the text of a note or a document: a line that
        // looks like a mail header there is something the owner kept.
        if !is_assistant_chat {
            for line in exchange_lines {
                for (name, address, placed) in written_line_people(line, is_email_stream) {
                    let _ = upsert(
                        name,
                        address,
                        "corresponded",
                        "transcript",
                        1.0,
                        placed.to_owned(),
                    );
                }
            }
        }

        let mut speaker_counts: BTreeMap<String, usize> = BTreeMap::new();
        for entry in std::fs::read_dir(seg_dir).into_iter().flatten().flatten() {
            let fname_os = entry.file_name();
            let Some(fname) = fname_os.to_str().filter(|name| is_spoken_audio_file(name)) else {
                continue;
            };
            is_spoken = true;
            if !fname.ends_with(".jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(entry.path())
                .unwrap_or_default()
                .lines()
            {
                if let Ok(val) = serde_json::from_str::<Value>(line)
                    && let Some(spk) = val
                        .get("speaker")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                {
                    *speaker_counts.entry(spk.to_owned()).or_default() += 1;
                }
            }
        }
        for (spk, count) in speaker_counts {
            let role = if count >= 2 { "attendee" } else { "heard" };
            let conf = if count >= 2 { 0.95 } else { 0.9 };
            let _ = upsert(
                spk,
                None,
                role,
                "speaker_label",
                conf,
                "Spoke in recorded conversation.".to_owned(),
            );
        }

        if let Ok(bytes) = std::fs::read(seg_dir.join("talents/sense.json"))
            && let Ok(sense) = serde_json::from_slice::<Value>(&bytes)
            && let Some(entities) = sense.get("entities").and_then(Value::as_array)
        {
            for ent in entities.iter().filter_map(Value::as_object) {
                if ent.get("type").and_then(Value::as_str) != Some("Person") {
                    continue;
                }
                let Some(name) = ent
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let level = ent.get("level").and_then(Value::as_str).unwrap_or("medium");
                let ctx = ent
                    .get("context")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if is_real_photo && level != "low" {
                    let _ = upsert(name.to_owned(), None, "visible", "screen", 0.9, ctx);
                } else if !is_spoken {
                    let src = if has_screenshot || is_real_photo {
                        "screen"
                    } else {
                        "transcript"
                    };
                    let _ = upsert(name.to_owned(), None, "mentioned", src, 0.8, ctx);
                }
            }
        }
    }

    if !participants.is_empty() {
        let rows: Vec<Value> = participants
            .into_values()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "role": p.role,
                    "source": p.source,
                    "confidence": p.confidence,
                    "context": p.context,
                    "entity_id": p.entity_id,
                })
            })
            .collect();
        record.insert("participation".to_owned(), Value::Array(rows));
    }
}

/// Fill in the activities an older day's history lacks.
///
/// Only streams that arrived as history are read: an import or a syncing
/// source. A stream captured live is never replayed here. Only thought, active
/// segments that no activity of the day holds are replayed, so every activity
/// this writes is new and every row the day already has stays as written.
pub(crate) fn ensure_historical_day_activities(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<(), String> {
    let inventory =
        observe_declared_facet_inventory(&context.journal).map_err(|error| error.to_string())?;
    let declared_facets: std::collections::BTreeSet<String> = inventory
        .enabled
        .iter()
        .chain(&inventory.muted)
        .cloned()
        .collect();
    // A row that names no stream may be an older record of any stream.
    let mut covered: std::collections::BTreeSet<(Option<String>, String)> =
        std::collections::BTreeSet::new();
    for facet in &declared_facets {
        if let Ok(rows) = solstone_core_facets::load_activity_records_readable(
            &context.journal,
            facet,
            &context.day,
            true,
        ) {
            for row in rows {
                let row_stream = row
                    .get("stream")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && *s != DEFAULT_STREAM)
                    .map(str::to_owned);
                if let Some(segs) = row.get("segments").and_then(Value::as_array) {
                    for seg in segs.iter().filter_map(Value::as_str) {
                        covered.insert((row_stream.clone(), seg.to_owned()));
                    }
                }
            }
        }
    }

    let segments =
        iter_segments(&context.journal, PathOrDay::Day(&context.day)).map_err(|e| e.to_string())?;
    let mut uncovered = Vec::new();
    for seg in segments {
        if solstone_core_segment::is_body_only_segment(&seg)
            || solstone_core_segment::is_calendar_segment(&context.journal, &seg)
            || solstone_core_segment::is_transactions_segment(&seg)
            || is_deleted_segment(seg.path())
        {
            continue;
        }
        let Some(stream) = seg
            .stream()
            .directory()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty() && *s != DEFAULT_STREAM)
            .map(str::to_owned)
        else {
            continue;
        };
        if stream == STRAVA_STREAM
            || crate::planner::is_mcp_audit_segment(Some(&stream))
            || !solstone_core_segment::is_backfill_stream(&context.journal, &stream)
        {
            continue;
        }
        let Some(seg_key) = seg.name().to_str().map(str::to_owned) else {
            continue;
        };
        if covered.contains(&(Some(stream.clone()), seg_key.clone()))
            || covered.contains(&(None, seg_key.clone()))
        {
            continue;
        }
        let sense_path = seg.path().join("talents/sense.json");
        let Ok(solstone_core_journal_io::durability::DurableRead::Present(sense)) =
            solstone_core_journal_io::durability::read_json_durable::<Value>(
                solstone_core_journal_io::durability::ArtifactId::SegmentSense,
                &sense_path,
            )
        else {
            continue;
        };
        if !valid_activity_sense(&sense)
            || sense.get("density").and_then(Value::as_str) == Some("idle")
        {
            continue;
        }
        let has_declared_active_facet =
            sense
                .get("facets")
                .and_then(Value::as_array)
                .is_some_and(|arr| {
                    arr.iter().filter_map(Value::as_object).any(|obj| {
                        obj.get("level").and_then(Value::as_str) != Some("low")
                            && obj
                                .get("facet")
                                .and_then(Value::as_str)
                                .is_some_and(|f| declared_facets.contains(f))
                    })
                });
        if has_declared_active_facet {
            uncovered.push((seg_key, Some(stream)));
        }
    }

    if !uncovered.is_empty() {
        replay_activity_state_selected(
            context,
            log,
            &uncovered,
            false,
            max_concurrency,
            skip_activity_prompts,
            false,
            None,
        )?;
    }
    // The span is the journal's whole history, not this day's work: what it
    // could not write is logged here once and does not fail the day.
    if solstone_core_segment::day_holds_only_backfill_streams(&context.journal, &context.day)
        && let Err(error) = consolidate_historical_backfill_span(&context.journal, context.now_ms)
    {
        log.log(
            "history_consolidation.failed",
            context.now_ms,
            Map::from_iter([
                ("day".to_owned(), Value::String(context.day.clone())),
                ("error".to_owned(), Value::String(error)),
            ]),
        );
    }
    Ok(())
}

/// Mailbox names that belong to a role or a machine, never to one person.
const ROLE_MAILBOXES: [&str; 22] = [
    "automated",
    "billing",
    "bounce",
    "bounces",
    "donotreply",
    "hello",
    "help",
    "info",
    "mailer",
    "marketing",
    "news",
    "newsletter",
    "noreply",
    "notification",
    "notifications",
    "postmaster",
    "receipts",
    "reply",
    "sales",
    "support",
    "team",
    "updates",
];

/// Whether a sender is a mailing, a service or a shared mailbox rather than
/// one person: by what it calls itself, or by the mailbox it writes from.
fn is_bulk_or_automated_actor(name: &str, email: Option<&str>) -> bool {
    let lower_name = name.trim().to_ascii_lowercase();
    if lower_name.is_empty()
        || matches!(
            lower_name.as_str(),
            "substack" | "no-reply" | "noreply" | "mailer-daemon" | "notifications"
        )
        || lower_name.contains("newsletter")
        || lower_name.contains("do not reply")
    {
        return true;
    }
    let Some(address) = email.map(|e| e.trim().to_ascii_lowercase()) else {
        return false;
    };
    let (mailbox, domain) = address.split_once('@').unwrap_or((address.as_str(), ""));
    domain.ends_with("substack.com")
        || mailbox.contains("mailer-daemon")
        || mailbox
            .replace("no-reply", "noreply")
            .replace("do-not-reply", "donotreply")
            .split(['.', '-', '_', '+'])
            .any(|word| ROLE_MAILBOXES.contains(&word))
}

/// "Sarah from Acme", "Sam via LinkedIn": a service signing as a person. Only
/// a person's name is read this way; a project can be "Notes from Underground".
fn signs_as_a_service(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains(" from ") || lower.contains(" via ")
}

/// What the span consolidation reads, as a cheap stamp: it runs again only
/// when one of these files changed. The date is no part of it, so a new day
/// alone redoes nothing.
fn backfill_span_fingerprint(
    journal: &std::path::Path,
    backfill_days: &[String],
    enabled_facets: &[String],
) -> Value {
    let mut max_mtime_ns = 0_u128;
    let mut total_bytes = 0_u64;
    let mut stat_path = |p: &std::path::Path| {
        if let Ok(meta) = std::fs::metadata(p) {
            total_bytes = total_bytes.saturating_add(meta.len());
            if let Ok(modified) = meta.modified()
                && let Ok(dur) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                max_mtime_ns = max_mtime_ns.max(dur.as_nanos());
            }
        }
    };
    stat_path(&journal.join("config/journal.json"));
    for facet in enabled_facets {
        let facet_dir = journal.join("facets").join(facet);
        stat_path(&facet_dir.join(solstone_core_entity::hidden_names::HIDDEN_NAMES_FILE));
        stat_path(&solstone_core_facets::facet_declaration_file(&facet_dir));
    }
    for day in backfill_days {
        stat_path(&journal.join("chronicle").join(day));
        for facet in enabled_facets {
            stat_path(
                &journal
                    .join("facets")
                    .join(facet)
                    .join("activities")
                    .join(format!("{day}.jsonl")),
            );
            stat_path(
                &journal
                    .join("facets")
                    .join(facet)
                    .join("detections/entities")
                    .join(format!("{day}.jsonl")),
            );
        }
    }
    serde_json::json!({
        "day_count": backfill_days.len(),
        "first_day": backfill_days.first(),
        "last_day": backfill_days.last(),
        "enabled_facets": enabled_facets,
        "total_bytes": total_bytes,
        "max_mtime_ns": max_mtime_ns.to_string(),
    })
}

/// A person is promoted once they took part on this many days of history. One
/// day's mail is one occasion, however many messages it held.
const SPAN_PERSON_MIN_DAYS: usize = 2;
/// A company or project is named in passing far more often than a person
/// takes part, so it needs more sightings, on more than one day.
const SPAN_ORGANIZATION_MIN_EPISODES: usize = 3;
const SPAN_ORGANIZATION_MIN_DAYS: usize = 2;
/// A tool shows up everywhere it is used; only a lasting one is kept.
const SPAN_TOOL_MIN_EPISODES: usize = 5;
const SPAN_TOOL_MIN_DAYS: usize = 3;
/// The most dated notes one consolidation gives an entity: its latest days.
const SPAN_NOTES_PER_ENTITY: usize = 24;

/// Whether a refused entity write is the store saying no (the name or its
/// place is taken, or the entity is blocked, hidden, merged or deleted), which
/// is an answer and not a failure. Every refusal is named, so a new one has to
/// be placed here before it compiles.
fn entity_write_declined(error: &solstone_core_facets::FacetEntityWriteError) -> bool {
    use solstone_core_entity::EntityWriteError as Entity;
    use solstone_core_facets::FacetEntityWriteError as Facet;
    match error {
        Facet::EntityExists { .. }
        | Facet::EntityBlocked { .. }
        | Facet::EntityNotFound { .. }
        | Facet::AkaConflict { .. }
        | Facet::IdentityMapLoser { .. }
        | Facet::RelationshipOccupied { .. } => true,
        Facet::EntityWrite(error) => match error {
            Entity::IdentityMerged { .. }
            | Entity::IdentityDeleted { .. }
            | Entity::EntityHidden { .. }
            | Entity::InvalidIdentity { .. }
            | Entity::CreateDestinationOccupied { .. } => true,
            Entity::PublicationRefused(_)
            | Entity::TrustLock(_)
            | Entity::Read(_)
            | Entity::InvalidOperationContext { .. }
            | Entity::AmbiguityRowInvalid { .. }
            | Entity::AmbiguityCountOverflow { .. }
            | Entity::ReconciliationRepairRequired { .. }
            | Entity::PreparedStageCollision { .. }
            | Entity::PreparedStage(_)
            | Entity::IdentityWrite(_)
            | Entity::VisibleEventWrite(_)
            | Entity::PreparedEventRemoval(_)
            | Entity::AmbiguityLock(_)
            | Entity::AmbiguityWrite(_)
            | Entity::AmbiguityChoiceNotFound { .. }
            | Entity::AmbiguityChoiceScopeMismatch { .. }
            | Entity::AmbiguityChoiceInvalid { .. }
            | Entity::CensusIncomplete { .. }
            | Entity::CacheWriteAfterCommit(_)
            | Entity::SearchUpdateAfterCommit(_) => false,
        },
        Facet::TrustLock(_)
        | Facet::EntityTrustLock(_)
        | Facet::FacetStore(_)
        | Facet::FacetWrite(_)
        | Facet::EntityStore(_)
        | Facet::EntityLifecycle(_)
        | Facet::MoveConflict { .. }
        | Facet::LinkFolders(_)
        | Facet::SameFacet { .. }
        | Facet::HiddenUndecidable(_)
        | Facet::MutationRefused(_)
        | Facet::Io(_) => false,
    }
}

/// The name and address of each correspondent of one activity, read from the
/// activity's own written exchanges. A segment is read once in a run, however
/// many activities and facets hold it.
fn activity_addresses(
    journal: &std::path::Path,
    day: &str,
    activity: &Map<String, Value>,
    read: &mut BTreeMap<std::path::PathBuf, Vec<(String, String)>>,
) -> Vec<(String, String)> {
    let mut addresses = Vec::new();
    let Some(stream) = activity.get("stream").and_then(Value::as_str) else {
        return addresses;
    };
    let email = holds_email(journal, stream);
    for segment in activity
        .get("segments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let Some(dir) = find_segment_dir(journal, day, segment, Some(stream)) else {
            continue;
        };
        let named = read.entry(dir).or_insert_with_key(|dir| {
            written_exchange_lines(dir)
                .iter()
                .flat_map(|line| written_line_people(line, email))
                .filter_map(|(name, address, _)| Some((name, address?)))
                .collect()
        });
        addresses.extend(named.iter().cloned());
    }
    addresses
}

/// The address an activity's exchanges give for `name`: under that name, or,
/// when a talent wrote the name another way, under the one name of the
/// exchange that is the same person.
fn correspondent_address(addresses: &[(String, String)], name: &str) -> Option<String> {
    let key = solstone_core_facets::hidden_names::hidden_name_key(name);
    if let Some((_, address)) = addresses
        .iter()
        .find(|(held, _)| solstone_core_facets::hidden_names::hidden_name_key(held) == key)
    {
        return Some(address.clone());
    }
    let close: BTreeSet<&String> = addresses
        .iter()
        .filter(|(held, _)| {
            solstone_core_talent_runtime::participation::same_correspondent(held, name)
        })
        .map(|(_, address)| address)
        .collect();
    match (close.len(), close.first()) {
        (1, Some(address)) => Some((*address).clone()),
        _ => None,
    }
}

/// Deterministic span-aware entity promotion, dated observation seeding, and
/// speculative-facet aggregation across the closed days that hold only
/// imports and synced sources (`day < today` and
/// `day_holds_only_backfill_streams`): the days whose entity talents are
/// skipped once they are older than a week, so none falls between the two.
///
/// It runs only when what it reads has changed. Whatever it could not write
/// is returned once and marked on the stamp, so the same refusal is not met
/// again on every call: the span is read again when its inputs change.
pub(crate) fn consolidate_historical_backfill_span(
    journal: &std::path::Path,
    now_ms: i64,
) -> Result<(), String> {
    let zone = solstone_core_journal_config::owner_zone(journal);
    let now = jiff::Timestamp::from_millisecond(now_ms).map_err(|error| error.to_string())?;
    let today_date = zone.to_datetime(now).date();
    let today_str = solstone_core_time::day_key(today_date);

    let mut backfill_days: Vec<(String, bool)> = solstone_core_journal_io::day_dirs(journal)
        .map_err(|error| error.to_string())?
        .into_keys()
        .filter_map(|day| {
            if day.as_str() >= today_str.as_str() {
                return None;
            }
            if solstone_core_segment::day_holds_only_backfill_streams(journal, &day) {
                Some((day, true))
            } else if solstone_core_segment::day_holds_historical_backfill(journal, &day) {
                Some((day, false))
            } else {
                None
            }
        })
        .collect();
    // In day order: the listing has none, and the stamp and the notes kept
    // for an entity both depend on it.
    backfill_days.sort_by(|a, b| a.0.cmp(&b.0));
    if backfill_days.is_empty() {
        return Ok(());
    }

    let day_names: Vec<String> = backfill_days.iter().map(|(d, _)| d.clone()).collect();
    let enabled_facets = observe_declared_facet_inventory(journal)
        .map_err(|error| error.to_string())?
        .enabled;
    let stamp_path = journal.join("awareness/backfill_span_state.json");
    // Taken before anything is read, so a change that lands during the run
    // is not stamped as read.
    let mut stamp = backfill_span_fingerprint(journal, &day_names, &enabled_facets);
    if let Ok(bytes) = std::fs::read(&stamp_path)
        && let Ok(mut saved) = serde_json::from_slice::<Value>(&bytes)
    {
        if let Some(saved) = saved.as_object_mut() {
            saved.remove(SPAN_FAILED);
        }
        if saved == stamp {
            return Ok(());
        }
    }

    let failures = consolidate_span_days(
        journal,
        now_ms,
        &zone,
        today_date,
        &today_str,
        &backfill_days,
        &enabled_facets,
    );
    if !failures.is_empty() {
        stamp[SPAN_FAILED] = Value::String(failures.join("; "));
    }
    std::fs::create_dir_all(journal.join("awareness")).map_err(|error| error.to_string())?;
    solstone_core_journal_io::write_text(
        &stamp_path,
        &stamp.to_string(),
        solstone_core_journal_io::AtomicWriteOptions::default(),
    )
    .map_err(|error| error.to_string())?;
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

/// The context an older version gave a correspondent it read from header-like
/// text. Nothing writes it now.
const HEADER_SCAN_CONTEXT: &str = "Corresponded in message.";

/// The stamp field that says the last read of these inputs could not write
/// everything, and what it could not.
const SPAN_FAILED: &str = "failed";

/// Read the span and write what it earns, returning what could not be written.
fn consolidate_span_days(
    journal: &std::path::Path,
    now_ms: i64,
    zone: &jiff::tz::TimeZone,
    today_date: jiff::civil::Date,
    today_str: &str,
    backfill_days: &[(String, bool)],
    enabled_facets: &[String],
) -> Vec<String> {
    let mut failures: Vec<String> = Vec::new();
    let mut read_addresses = BTreeMap::new();
    match solstone_core_facets::aggregate_speculative_facets_with_backfill(
        journal,
        today_date,
        solstone_core_facets::FACET_CANDIDATE_MIN_SEGMENTS,
    ) {
        Ok(candidates) if candidates.is_empty() => {}
        Ok(candidates) => {
            if let Err(error) =
                solstone_core_facets::record_facet_candidates(journal, today_str, &candidates)
            {
                failures.push(format!("facet candidates: {error}"));
            }
        }
        Err(error) => failures.push(format!("facet candidates: {error}")),
    }

    let mut owner_or_blocked = solstone_core_entity_matching::ExclusionKeys::default();
    if let Ok(bytes) = std::fs::read(journal.join("config/journal.json"))
        && let Ok(cfg) = serde_json::from_slice::<Value>(&bytes)
        && let Some(ident) = cfg.get("identity").and_then(Value::as_object)
    {
        for k in ["name", "preferred"] {
            if let Some(s) = ident
                .get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
            {
                owner_or_blocked.add_candidate(
                    &solstone_core_entity_matching::EntityNameCandidate {
                        id: None,
                        name: s.trim().to_owned(),
                        aka: Vec::new(),
                        emails: Vec::new(),
                    },
                );
            }
        }
    }
    for alias in read_email_owner_aliases(journal) {
        owner_or_blocked.add_candidate(&solstone_core_entity_matching::EntityNameCandidate {
            id: None,
            name: alias.clone(),
            aka: Vec::new(),
            emails: vec![alias],
        });
    }
    if !enabled_facets.is_empty() {
        match solstone_core_entity::every_journal_entity(journal) {
            Ok(entities) => {
                for entity in entities {
                    if entity.is_principal() || entity.is_blocked() {
                        let res = entity.resolution_entity();
                        owner_or_blocked.add_candidate(
                            &solstone_core_entity_matching::EntityNameCandidate {
                                id: res.id,
                                name: res.name,
                                aka: res.aka,
                                emails: res.emails,
                            },
                        );
                    }
                }
            }
            // Without the owner and the blocked names nobody can be promoted
            // safely, so no facet is read.
            Err(error) => {
                failures.push(format!("entities: {error}"));
                return failures;
            }
        }
    }

    // Read the journal entity identity map once across all facets and promoted
    // candidates instead of rescanning entities/*/identity.json per facet or per candidate.
    let mut resolved_ids = if enabled_facets.is_empty() {
        std::collections::HashMap::new()
    } else {
        match solstone_core_entity::read_identity_map(journal) {
            Ok(map) => map.resolved,
            Err(error) => {
                failures.push(format!("entities: {error}"));
                return failures;
            }
        }
    };

    #[derive(Default)]
    struct SpanCandidate {
        entity_type: String,
        names: BTreeMap<String, usize>,
        emails: BTreeSet<String>,
        active_episodes: BTreeSet<(String, String)>,
        days: BTreeSet<String>,
        day_notes: BTreeMap<String, String>,
    }

    #[derive(Clone)]
    struct ActiveFacetTarget {
        entity_id: String,
        entity_dir: String,
        relationship_dir: String,
    }

    for facet in enabled_facets {
        let hides = match solstone_core_facets::hidden_names::facet_hidden_keys(journal, facet) {
            Ok(hides) => hides,
            Err(error) => {
                failures.push(format!("{facet}: {error}"));
                continue;
            }
        };
        let scoped = match solstone_core_facets::list_scoped_facet_entities_with_resolved_ids(
            journal,
            facet,
            true,
            true,
            &resolved_ids,
        ) {
            Ok(scoped) => scoped,
            Err(error) => {
                failures.push(format!("{facet}: {error}"));
                continue;
            }
        };
        let mut detached_or_blocked = solstone_core_entity_matching::ExclusionKeys::default();
        let mut already_attached = solstone_core_entity_matching::ExclusionKeys::default();
        let mut active_by_key: std::collections::HashMap<String, ActiveFacetTarget> =
            std::collections::HashMap::new();
        let mut active_by_rel_dir: std::collections::HashMap<String, ActiveFacetTarget> =
            std::collections::HashMap::new();
        for item in &scoped {
            let strings = |key: &str| -> Vec<String> {
                item.identity
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            };
            let item_name = item
                .identity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let item_aka = strings("aka");
            let cand = solstone_core_entity_matching::EntityNameCandidate {
                id: Some(item.entity_id.clone()),
                name: item_name.clone(),
                aka: item_aka.clone(),
                emails: strings("emails"),
            };
            if item.detached || item.blocked {
                detached_or_blocked.add_candidate(&cand);
            } else {
                already_attached.add_candidate(&cand);
                let target = ActiveFacetTarget {
                    entity_id: item.entity_id.clone(),
                    entity_dir: item.entity_dir.clone(),
                    relationship_dir: item.relationship_dir.clone(),
                };
                active_by_rel_dir.insert(item.relationship_dir.clone(), target.clone());
                active_by_key.insert(
                    solstone_core_entity_matching::normalize_resolution_query(&item.entity_id),
                    target.clone(),
                );
                if !item_name.is_empty() {
                    active_by_key.insert(
                        solstone_core_entity_matching::normalize_resolution_query(&item_name),
                        target.clone(),
                    );
                    let slug = solstone_core_entity_matching::entity_slug(&item_name);
                    if !slug.is_empty() {
                        active_by_key.entry(slug).or_insert_with(|| target.clone());
                    }
                }
                for alias in &item_aka {
                    let norm = solstone_core_entity_matching::normalize_resolution_query(alias);
                    if !norm.is_empty() {
                        active_by_key.entry(norm).or_insert_with(|| target.clone());
                    }
                }
            }
        }

        // The addresses of the attached entity that goes by exactly `name`.
        let attached_addresses = |name: &str| -> Option<BTreeSet<String>> {
            let key = solstone_core_facets::hidden_names::hidden_name_key(name);
            let strings = |item: &Value, field: &str| -> Vec<String> {
                item.get(field)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_lowercase())
                    .collect()
            };
            scoped
                .iter()
                .filter(|item| !item.detached && !item.blocked)
                .find(|item| {
                    item.identity
                        .get("name")
                        .and_then(Value::as_str)
                        .into_iter()
                        .map(str::to_owned)
                        .chain(strings(&item.identity, "aka"))
                        .any(|held| {
                            solstone_core_facets::hidden_names::hidden_name_key(&held) == key
                        })
                })
                .map(|item| strings(&item.identity, "emails").into_iter().collect())
        };

        // A person is one address where an exchange gives one, and one name
        // where none does: two senders who share a display name stay two.
        let mut groups: BTreeMap<(String, String), SpanCandidate> = BTreeMap::new();

        for (day, backfill_only) in backfill_days {
            if let Ok(activities) =
                solstone_core_facets::load_activity_records_readable(journal, facet, day, false)
            {
                for act in activities {
                    if !*backfill_only {
                        let historical = act
                            .get("stream")
                            .and_then(Value::as_str)
                            .is_some_and(|stream| {
                                solstone_core_segment::stream_arrived_as_history_on_day(
                                    journal, stream, day,
                                )
                            });
                        if !historical {
                            continue;
                        }
                    }
                    let act_id = act
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("act")
                        .to_owned();
                    let act_summary = act
                        .get("summary")
                        .or_else(|| act.get("title"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .to_owned();
                    let Some(parts) = act.get("participation").and_then(Value::as_array) else {
                        continue;
                    };
                    let addresses = activity_addresses(journal, day, &act, &mut read_addresses);
                    for part in parts.iter().filter_map(Value::as_object) {
                        let Some(name) = part
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                        else {
                            continue;
                        };
                        let role = part
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("mentioned");
                        if !matches!(role, "corresponded" | "attendee" | "heard" | "visible") {
                            continue;
                        }
                        // What an older version read from a line that looked
                        // like a mail header, in a note or a document: it was
                        // never an exchange, and counts toward nobody.
                        if part.get("context").and_then(Value::as_str) == Some(HEADER_SCAN_CONTEXT)
                        {
                            continue;
                        }
                        let name_key = solstone_core_facets::hidden_names::hidden_name_key(name);
                        let email = correspondent_address(&addresses, name);
                        if owner_or_blocked.contains(name)
                            || email
                                .as_deref()
                                .is_some_and(|em| owner_or_blocked.contains(em))
                            || hides.is_hidden(name)
                            || detached_or_blocked.contains(name)
                            || signs_as_a_service(name)
                            || is_bulk_or_automated_actor(name, email.as_deref())
                        {
                            continue;
                        }
                        let person_key = match &email {
                            Some(address) => format!("<{address}>"),
                            None => name_key,
                        };
                        let entry = groups
                            .entry(("person".to_owned(), person_key))
                            .or_insert_with(|| SpanCandidate {
                                entity_type: "Person".to_owned(),
                                ..Default::default()
                            });
                        *entry.names.entry(name.to_owned()).or_default() += 1;
                        entry.emails.extend(email);
                        entry.active_episodes.insert((day.clone(), act_id.clone()));
                        entry.days.insert(day.clone());
                        let note = part
                            .get("context")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            // What the fill-in wrote for want of a context says
                            // nothing about the person.
                            .filter(|s| {
                                !s.is_empty()
                                    && !s.starts_with("Corresponded in ")
                                    && *s != "Spoke in recorded conversation."
                            })
                            .map(str::to_owned)
                            .unwrap_or_else(|| act_summary.clone());
                        if !note.is_empty() {
                            entry.day_notes.entry(day.clone()).or_insert(note);
                        }
                    }
                }
            }

            if let Ok(detections) =
                solstone_core_facets::read_detected_entities(journal, facet, day)
            {
                for (idx, det) in detections.iter().enumerate() {
                    if !*backfill_only {
                        let historical = det
                            .get("segments")
                            .and_then(Value::as_array)
                            .is_some_and(|segs| {
                                segs.iter().filter_map(Value::as_str).any(|origin| {
                                    origin.split_once('/').is_some_and(|(stream, _)| {
                                        solstone_core_segment::stream_arrived_as_history_on_day(
                                            journal, stream, day,
                                        )
                                    })
                                })
                            });
                        if !historical {
                            continue;
                        }
                    }
                    let Some(name) = det
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    else {
                        continue;
                    };
                    let etype = det.get("type").and_then(Value::as_str).unwrap_or("Person");
                    if !matches!(etype, "Person" | "Company" | "Project" | "Tool") {
                        continue;
                    }
                    let centrality = det
                        .get("centrality")
                        .or_else(|| det.get("level"))
                        .and_then(Value::as_str)
                        .unwrap_or("medium");
                    let qualifies_episode = match etype {
                        "Person" => centrality == "high",
                        "Company" | "Project" => matches!(centrality, "high" | "medium"),
                        "Tool" => centrality == "high",
                        _ => false,
                    };
                    if owner_or_blocked.contains(name)
                        || hides.is_hidden(name)
                        || detached_or_blocked.contains(name)
                        || (etype == "Person" && signs_as_a_service(name))
                        || is_bulk_or_automated_actor(name, None)
                    {
                        continue;
                    }
                    let name_key = solstone_core_facets::hidden_names::hidden_name_key(name);
                    let entry = groups
                        .entry((etype.to_ascii_lowercase(), name_key))
                        .or_insert_with(|| SpanCandidate {
                            entity_type: etype.to_owned(),
                            ..Default::default()
                        });
                    *entry.names.entry(name.to_owned()).or_default() += 1;
                    if qualifies_episode {
                        entry
                            .active_episodes
                            .insert((day.clone(), format!("det_{idx}")));
                        entry.days.insert(day.clone());
                    }
                    if let Some(desc) = det
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        entry.day_notes.insert(day.clone(), desc.to_owned());
                    }
                }
            }
        }

        // A person seen without an address is the one addressed person of
        // that name. Where two addresses carry the name, nothing says which
        // of them it was, and it stays apart from both.
        let mut addressed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for ((kind, key), cand) in &groups {
            if kind == "person" && key.starts_with('<') {
                for name in cand.names.keys() {
                    addressed
                        .entry(solstone_core_facets::hidden_names::hidden_name_key(name))
                        .or_default()
                        .insert(key.clone());
                }
            }
        }
        for (name_key, keys) in addressed {
            if let (1, Some(key)) = (keys.len(), keys.first())
                && let Some(unaddressed) = groups.remove(&("person".to_owned(), name_key))
                && let Some(person) = groups.get_mut(&("person".to_owned(), key.clone()))
            {
                for (name, count) in unaddressed.names {
                    *person.names.entry(name).or_default() += count;
                }
                person.active_episodes.extend(unaddressed.active_episodes);
                person.days.extend(unaddressed.days);
                for (day, note) in unaddressed.day_notes {
                    person.day_notes.entry(day).or_insert(note);
                }
            }
        }

        // The addresses behind each name this pass gave an entity or added to.
        let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for cand in groups.into_values() {
            let Some(canonical_name) = cand
                .names
                .iter()
                .max_by_key(|(n, count)| (n.split_whitespace().count(), **count, n.len()))
                .map(|(n, _)| n.clone())
            else {
                continue;
            };
            // An entity is held by name, so the facet can hold one person of
            // a name. Where the one it holds, from before or from this pass,
            // writes from other addresses, this is someone else of the same
            // name: they get no entity, and nothing of theirs is added to the
            // other's.
            let name_key = solstone_core_facets::hidden_names::hidden_name_key(&canonical_name);
            let held = claimed
                .get(&name_key)
                .cloned()
                .or_else(|| attached_addresses(&canonical_name));
            if !cand.emails.is_empty()
                && held
                    .as_ref()
                    .is_some_and(|held| !held.is_empty() && held.is_disjoint(&cand.emails))
            {
                continue;
            }
            // Someone the facet already holds needs no promotion.
            let is_existing =
                already_attached.contains(&canonical_name) || claimed.contains_key(&name_key);
            let meets_promotion = match cand.entity_type.as_str() {
                "Person" => cand.days.len() >= SPAN_PERSON_MIN_DAYS,
                "Company" | "Project" => {
                    cand.active_episodes.len() >= SPAN_ORGANIZATION_MIN_EPISODES
                        && cand.days.len() >= SPAN_ORGANIZATION_MIN_DAYS
                }
                "Tool" => {
                    cand.active_episodes.len() >= SPAN_TOOL_MIN_EPISODES
                        && cand.days.len() >= SPAN_TOOL_MIN_DAYS
                }
                _ => false,
            };
            if !is_existing && !meets_promotion {
                continue;
            }
            let latest_desc = cand
                .day_notes
                .values()
                .next_back()
                .cloned()
                .unwrap_or_else(|| {
                    format!(
                        "Recurring {} across journal history.",
                        cand.entity_type.to_ascii_lowercase()
                    )
                });

            let mut created_entity_id: Option<String> = None;
            if !is_existing {
                let created = match solstone_core_facets::create_facet_entity_if_absent(
                    journal,
                    facet,
                    &cand.entity_type,
                    &canonical_name,
                    &latest_desc,
                ) {
                    Ok(solstone_core_facets::FacetEntityCreateOutcome::Created {
                        entity_id,
                        ..
                    }) => {
                        resolved_ids.insert(entity_id.clone(), entity_id.clone());
                        let target = ActiveFacetTarget {
                            entity_id: entity_id.clone(),
                            entity_dir: entity_id.clone(),
                            relationship_dir: entity_id.clone(),
                        };
                        active_by_rel_dir.insert(entity_id.clone(), target.clone());
                        active_by_key.insert(
                            solstone_core_entity_matching::normalize_resolution_query(&entity_id),
                            target.clone(),
                        );
                        active_by_key.insert(
                            solstone_core_entity_matching::normalize_resolution_query(
                                &canonical_name,
                            ),
                            target.clone(),
                        );
                        let slug = solstone_core_entity_matching::entity_slug(&canonical_name);
                        if !slug.is_empty() {
                            active_by_key.entry(slug).or_insert(target);
                        }
                        created_entity_id = Some(entity_id);
                        Ok(())
                    }
                    Ok(solstone_core_facets::FacetEntityCreateOutcome::Exists)
                    | Err(solstone_core_facets::FacetEntityWriteError::EntityExists { .. })
                        if !detached_or_blocked.contains(&canonical_name) =>
                    {
                        solstone_core_facets::attach_or_reactivate_entity(
                            journal,
                            facet,
                            &cand.entity_type,
                            &canonical_name,
                            &latest_desc,
                        )
                        .map(|attached| {
                            if let Some(eid) = attached
                                .relationship
                                .get("entity_id")
                                .or_else(|| attached.relationship.get("id"))
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                            {
                                created_entity_id = Some(eid.to_owned());
                            }
                        })
                    }
                    Ok(_) => Ok(()),
                    Err(error) => Err(error),
                };
                match created {
                    Ok(()) => {}
                    Err(error) if entity_write_declined(&error) => continue,
                    Err(error) => {
                        failures.push(format!("{facet}: {error}"));
                        continue;
                    }
                }
            }

            let norm_query =
                solstone_core_entity_matching::normalize_resolution_query(&canonical_name);
            let slug_query = solstone_core_entity_matching::entity_slug(&canonical_name);
            let resolved_target: Option<ActiveFacetTarget> = created_entity_id
                .as_deref()
                .and_then(|eid| {
                    active_by_key
                        .get(&solstone_core_entity_matching::normalize_resolution_query(eid))
                        .cloned()
                })
                .or_else(|| active_by_key.get(&norm_query).cloned())
                .or_else(|| active_by_key.get(&slug_query).cloned())
                .or_else(|| {
                    let Ok(solstone_core_facets::ObservationEntityResolution::Resolved {
                        entity_dir: rel_dir,
                    }) = solstone_core_facets::resolve_observation_entity_dir_with_resolved_ids(
                        journal,
                        facet,
                        &canonical_name,
                        &resolved_ids,
                    )
                    else {
                        return None;
                    };
                    if let Some(target) = active_by_rel_dir.get(&rel_dir) {
                        return Some(target.clone());
                    }
                    let link =
                        solstone_core_facets::read_facet_entity_link(journal, facet, &rel_dir)
                            .ok()
                            .flatten()?;
                    if link.value().get("detached") == Some(&Value::Bool(true)) {
                        return None;
                    }
                    let entity_id = link.entity_id().to_owned();
                    let entity_dir = resolved_ids
                        .get(&entity_id)
                        .cloned()
                        .unwrap_or_else(|| entity_id.clone());
                    let identity =
                        solstone_core_entity::read_entity_identity(journal, &entity_dir)
                            .ok()
                            .flatten()?;
                    if identity.value().get("blocked") == Some(&Value::Bool(true)) {
                        return None;
                    }
                    let target = ActiveFacetTarget {
                        entity_id,
                        entity_dir,
                        relationship_dir: rel_dir.clone(),
                    };
                    active_by_rel_dir.insert(rel_dir, target.clone());
                    active_by_key.insert(norm_query.clone(), target.clone());
                    Some(target)
                });
            let Some(target) = resolved_target else {
                continue;
            };
            claimed
                .entry(name_key)
                .or_insert_with(|| held.unwrap_or_default())
                .extend(cand.emails.iter().cloned());

            let mut incoming = Vec::new();
            for (day, note) in cand
                .day_notes
                .iter()
                .rev()
                .take(SPAN_NOTES_PER_ENTITY)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                if solstone_core_facets::normalize_observation_content(note).is_empty() {
                    continue;
                }
                incoming.push(solstone_core_facets::IncomingObservationRow {
                    content: note.clone(),
                    observed_at: solstone_core_entity::journal_day_start_ms(day, zone)
                        .unwrap_or(now_ms),
                    source_day: Some(day.clone()),
                    relation: None,
                });
            }
            if !incoming.is_empty() {
                use solstone_core_facets::ObservationWriteError as Refused;
                match solstone_core_facets::apply_observation_change(
                    journal,
                    facet,
                    &target.relationship_dir,
                    solstone_core_facets::ObservationChange::AppendMany {
                        rows: incoming,
                        actor: "model",
                    },
                ) {
                    // An entity merged away or deleted takes no notes.
                    Ok(_) | Err(Refused::Retired { .. } | Refused::EmptyContent) => {}
                    Err(error) => failures.push(format!("{facet}: {error}")),
                }
            }

            let canonical_slug = solstone_core_entity_matching::entity_slug(&canonical_name);
            for alias in cand.names.keys() {
                if alias != &canonical_name
                    && alias.split_whitespace().count() >= 2
                    && solstone_core_entity_matching::entity_slug(alias) != canonical_slug
                    && let Err(error) = solstone_core_facets::add_entity_aka(
                        journal,
                        facet,
                        &target.entity_id,
                        alias,
                        serde_json::json!({}),
                        |_| Ok(()),
                    )
                    && !entity_write_declined(&error)
                {
                    failures.push(format!("{facet}: {error}"));
                }
            }
            if cand.emails.is_empty() {
                continue;
            }
            let saved = (|| -> Result<(), String> {
                let _entity_trust = solstone_core_entity::hold_entity_trust_lock(journal)
                    .map_err(|error| error.to_string())?;
                let Some(snapshot) =
                    solstone_core_entity::read_entity_identity(journal, &target.entity_dir)
                        .map_err(|error| error.to_string())?
                else {
                    return Ok(());
                };
                let mut updated = snapshot.value().clone();
                let Some(obj) = updated.as_object_mut() else {
                    return Ok(());
                };
                let mut emails: BTreeSet<String> = obj
                    .get("emails")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|s| s.trim().to_lowercase())
                    .collect();
                let before_len = emails.len();
                emails.extend(cand.emails.iter().cloned());
                if emails.len() == before_len {
                    return Ok(());
                }
                obj.insert(
                    "emails".to_owned(),
                    Value::Array(emails.into_iter().map(Value::String).collect()),
                );
                use solstone_core_entity::EntityWriteError as Refused;
                match solstone_core_entity::save_entity_identity(
                    journal,
                    &target.entity_id,
                    &updated,
                    None,
                ) {
                    Ok(_)
                    | Err(
                        Refused::EntityHidden { .. }
                        | Refused::IdentityMerged { .. }
                        | Refused::IdentityDeleted { .. },
                    ) => Ok(()),
                    Err(error) => Err(error.to_string()),
                }
            })();
            if let Err(error) = saved {
                failures.push(format!("{facet}: {error}"));
            }
        }
    }

    failures
}

/// End a stream's live activities after the supervisor's idle window.
///
/// Capture that simply stops (a locked screen, a sleeping machine) produces no
/// idle segment, so nothing else ends the last activity until capture resumes
/// or the day's run replays it. The close applies only while `segment` is
/// still the last one its stream's state machine saw: a newer segment owns the
/// activity from there, and a segment not yet thought about would reopen it.
pub(crate) fn close_idle_activities(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    stream: Option<&str>,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<(), String> {
    let Some(segment_dir) = resolve_segment_dir(
        context.locale(),
        &context.journal,
        &context.day,
        segment,
        stream,
    )?
    else {
        return Ok(());
    };
    let stream = named_stream(&segment_dir, &context.day);
    let mut talents = Vec::new();
    let mut errors = Vec::new();
    // Segments the live tail left waiting for an earlier one that never got
    // its Sense are applied first, so the stream's real last segment is the
    // one this flush closes.
    let waiting = {
        let _turn = state_turn(context, stream);
        let machine = ActivityStateMachine::hydrate(Some(&context.journal), stream);
        plan_live_step(context, &machine, segment, stream, false)
    };
    let waiting = waiting.unwrap_or_else(|error| {
        errors.push(error);
        LiveStep::Late
    });
    if let LiveStep::Advance(keys) = waiting
        && let Err(error) = replay_activity_state_selected(
            context,
            log,
            &keys
                .into_iter()
                .map(|key| (key, Some(stream_coordinate(&segment_dir, &context.day))))
                .collect::<Vec<_>>(),
            false,
            max_concurrency,
            skip_activity_prompts,
            true,
            None,
        )
    {
        errors.push(error);
    }
    // Whether `segment` is still the stream's last: only then is the stream idle.
    let mut idle = false;
    'close: {
        // The flush runs in its own queue partition, so a live segment on
        // this stream may be advancing the same state right now.
        let _turn = state_turn(context, stream);
        let mut machine = ActivityStateMachine::hydrate(Some(&context.journal), stream);
        #[cfg(test)]
        state_probe::at(&context.journal, "hydrated");
        if machine.last_segment_key() != Some(segment)
            || machine.last_segment_day() != Some(context.day.as_str())
        {
            break 'close;
        }
        idle = true;
        let changes = machine.close_active(segment, context.now_ms);
        if changes.is_empty() {
            break 'close;
        }
        let day = context.day.clone();
        let published = if crate::settle::defers(stream, false) {
            crate::settle::note(context, stream.unwrap_or(DEFAULT_STREAM), &[day.as_str()])
        } else {
            let mut backfill_cache = BackfillDefaultsCache::default();
            publish_ended_activities(
                context,
                log,
                segment,
                &day,
                stream,
                changes,
                &machine.completed_activities(),
                false,
                skip_activity_prompts,
                &mut talents,
                &mut backfill_cache,
            )
        };
        if let Err(error) = published {
            errors.push(error);
        }
        #[cfg(test)]
        state_probe::at(&context.journal, "published");
        if let Err(error) = persist_activity_state(context, stream, &machine) {
            errors.push(solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.activity.state",
                [("error", &(error))],
            ));
        }
    }
    if let Err(error) = run_activity_talents(context, log, talents, false, max_concurrency) {
        errors.push(error);
    }
    if crate::settle::defers(stream, false)
        && let Err(error) = crate::settle::check(
            context,
            log,
            stream.unwrap_or(DEFAULT_STREAM),
            idle,
            skip_activity_prompts,
            max_concurrency,
        )
    {
        errors.push(error);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// How long a writer waits for a stream's activity-state turn. A holder keeps
/// it only to read, advance, publish and save that state, never while a talent
/// runs.
const STATE_TURN_WAIT: Duration = Duration::from_secs(60);

/// Take one stream's activity-state turn, shared by every process on this
/// journal. Live segment thinking and the idle flush run in different queue
/// partitions; without the turn, either could write back a snapshot the other
/// had already replaced and lose what the other started or ended.
///
/// A turn that can't be taken, whether it times out or its lock file can't
/// be made, is not fatal: the step goes ahead unserialized, as every step did
/// before turns existed. Giving up would drop this segment's update for
/// certain; going ahead loses it only if the holder is still writing.
fn state_turn(context: &ThinkContext, stream: Option<&str>) -> Option<FileLock> {
    #[cfg(test)]
    state_probe::at(&context.journal, "turn");
    match hold_lock(
        activity_state_path(&context.journal, stream),
        LockOptions {
            timeout: STATE_TURN_WAIT,
            ..LockOptions::default()
        },
    ) {
        Ok(turn) => Some(turn),
        Err(error) => {
            #[cfg(test)]
            state_probe::at(&context.journal, "unguarded");
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.activity.turn",
                    [
                        ("stream", &stream.unwrap_or(DEFAULT_STREAM)),
                        (
                            "error",
                            &solstone_core_journal_io::cli_diagnostics::lock_detail(
                                context.locale(),
                                &error
                            )
                        )
                    ]
                )
            );
            None
        }
    }
}

/// A test's view of the points where one stream's live state is read and
/// guarded, so it can run a competing writer at an exact moment.
#[cfg(test)]
pub(crate) mod state_probe {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    type Probe = Arc<dyn Fn(&Path, &str) + Send + Sync>;

    static PROBE: Mutex<Option<Probe>> = Mutex::new(None);

    pub(crate) fn install(probe: Option<Probe>) {
        *PROBE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = probe;
    }

    pub(crate) fn at(journal: &Path, point: &str) {
        let probe = PROBE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(probe) = probe {
            probe(journal, point);
        }
    }
}

fn persist_activity_state(
    context: &ThinkContext,
    stream: Option<&str>,
    machine: &ActivityStateMachine,
) -> Result<(), String> {
    context.checkpoint()?;
    let path = activity_state_path(&context.journal, stream);
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&machine.snapshot()).map_err(|error| error.to_string())?;
    context.checkpoint()?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default())
        .map_err(|error| error.to_string())?;
    if machine.adopted_shared() {
        context.checkpoint()?;
        match std::fs::remove_file(context.journal.join(SHARED_ACTIVITY_STATE)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error.to_string());
            }
            _ => {}
        }
    }
    Ok(())
}

/// A published activity whose talent work is recorded and still to run,
/// with the claim that keeps other runs off it until it does.
pub(crate) struct EndedActivity {
    day: String,
    facet: String,
    id: String,
    claim: Option<FileLock>,
}

impl EndedActivity {
    pub(crate) fn day(&self) -> &str {
        &self.day
    }
}

/// Append each ended activity's record and record its talent work, without
/// running any of it. Both writes are idempotent, so publishing an activity
/// again finds it already there.
#[allow(clippy::too_many_arguments)]
fn publish_ended_activities(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    routing_day: &str,
    stream: Option<&str>,
    changes: Vec<Value>,
    completed: &[Value],
    refresh: bool,
    skip_activity_prompts: bool,
    talents: &mut Vec<EndedActivity>,
    backfill_cache: &mut BackfillDefaultsCache,
) -> Result<(), String> {
    let mut failures = Vec::new();
    for change in changes {
        if change.get("state").and_then(Value::as_str) != Some("ended") {
            continue;
        }
        let Some(id) = change.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(facet) = change.get("facet").and_then(Value::as_str) else {
            continue;
        };
        log.log(
            "activity.detected",
            context.now_ms,
            activity_event(
                context,
                segment,
                routing_day,
                Map::from_iter([
                    ("activity".to_owned(), Value::String(id.to_owned())),
                    ("facet".to_owned(), Value::String(facet.to_owned())),
                    ("state".to_owned(), Value::String("ended".to_owned())),
                ]),
            ),
        );
        let Some(mut record) = completed.iter().rev().find_map(|record| {
            (record.get("id").and_then(Value::as_str) == Some(id)
                && record.get("facet").and_then(Value::as_str) == Some(facet))
            .then(|| record.as_object().cloned())
            .flatten()
        }) else {
            continue;
        };
        // The record names the stream its segments sit in, so whatever reads
        // them later finds this stream's and never another's with the same key.
        if let Some(stream) = stream {
            record.insert("stream".to_owned(), Value::String(stream.to_owned()));
            if solstone_core_segment::is_backfill_stream(&context.journal, stream) {
                populate_backfill_activity_defaults_with_cache(
                    &context.journal,
                    routing_day,
                    facet,
                    stream,
                    &mut record,
                    backfill_cache,
                );
            }
        }
        let written = match append_own_activity(context, facet, routing_day, &mut record) {
            Ok(OwnAppend::Written) => true,
            Ok(OwnAppend::AlreadyExists) => false,
            // Nothing was written, so there is nothing to publish or run.
            Ok(OwnAppend::Skipped) => continue,
            Err(error) => {
                log.log(
                    "activity.persist_failed",
                    context.now_ms,
                    activity_event(
                        context,
                        segment,
                        routing_day,
                        Map::from_iter([
                            ("activity".to_owned(), Value::String(id.to_owned())),
                            ("facet".to_owned(), Value::String(facet.to_owned())),
                            ("error".to_owned(), Value::String(error.to_string())),
                        ]),
                    ),
                );
                failures.push(format!("{facet}/{id}: {error}"));
                continue;
            }
        };
        if let Err(failure) = record_publication(
            context,
            log,
            segment,
            routing_day,
            facet,
            &record,
            id,
            written,
            refresh,
            skip_activity_prompts,
            talents,
        ) {
            failures.push(failure);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.activity.work.pending",
            [("failures", &(failures.join(", ")))],
        ))
    }
}

/// Log a written activity and record its talent work, without running any
/// of it. Every publisher ends here once the record store holds the activity.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_publication(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    routing_day: &str,
    facet: &str,
    record: &Map<String, Value>,
    fallback_id: &str,
    written: bool,
    refresh: bool,
    skip_activity_prompts: bool,
    talents: &mut Vec<EndedActivity>,
) -> Result<(), String> {
    context.checkpoint()?;
    // The ID it was published under: its own when another stream's
    // activity already held the one its type and first segment make.
    let published_id = record
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id)
        .to_owned();
    let id = published_id.as_str();
    log.log(
        "activity.persisted",
        context.now_ms,
        activity_event(
            context,
            segment,
            routing_day,
            Map::from_iter([
                ("activity".to_owned(), Value::String(id.to_owned())),
                ("facet".to_owned(), Value::String(facet.to_owned())),
            ]),
        ),
    );
    if skip_activity_prompts {
        log.log(
            "activity.prompts_skipped",
            context.now_ms,
            activity_event(
                context,
                segment,
                routing_day,
                Map::from_iter([
                    ("activity".to_owned(), Value::String(id.to_owned())),
                    ("facet".to_owned(), Value::String(facet.to_owned())),
                    (
                        "reason".to_owned(),
                        Value::String("--no-activity-prompts".to_owned()),
                    ),
                ]),
            ),
        );
        return Ok(());
    }
    let (changed, _) = activity_input_changed(context, routing_day, facet, id, record);
    if !(written || refresh || changed) {
        log.log(
            "activity.unchanged",
            context.now_ms,
            activity_event(
                context,
                segment,
                routing_day,
                Map::from_iter([("activity".to_owned(), Value::String(id.to_owned()))]),
            ),
        );
        return Ok(());
    }
    let enqueued = activity_context(context, routing_day)
        .and_then(|day_context| crate::activity::enqueue(&day_context, id, facet, refresh));
    match enqueued {
        Ok(claim) => {
            talents.push(EndedActivity {
                day: routing_day.to_owned(),
                facet: facet.to_owned(),
                id: id.to_owned(),
                claim,
            });
            Ok(())
        }
        Err(error) => Err(format!("{facet}/{id}: {error}")),
    }
}

/// Record talent work again for a written activity whose input has changed
/// since its talents last ran, as publishing it again would.
pub(crate) fn rethink_if_input_changed(
    context: &ThinkContext,
    routing_day: &str,
    facet: &str,
    record: &Map<String, Value>,
    talents: &mut Vec<EndedActivity>,
) -> Result<(), String> {
    let Some(id) = record.get("id").and_then(Value::as_str) else {
        return Ok(());
    };
    let (changed, _) = activity_input_changed(context, routing_day, facet, id, record);
    if !changed {
        return Ok(());
    }
    let claim = activity_context(context, routing_day)
        .and_then(|day_context| crate::activity::enqueue(&day_context, id, facet, false))
        .map_err(|error| format!("{facet}/{id}: {error}"))?;
    talents.push(EndedActivity {
        day: routing_day.to_owned(),
        facet: facet.to_owned(),
        id: id.to_owned(),
        claim,
    });
    Ok(())
}

/// What appending an ended activity did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnAppend {
    Written,
    AlreadyExists,
    /// Not appended, and not an error: the activity is in a stream whose
    /// records thinking no longer writes.
    Skipped,
}

/// Append an ended activity, giving it its own ID when another stream's
/// activity already holds the one its type and first segment make.
///
/// An activity in Strava's stream is never appended. A Strava tile is body
/// data, so its record comes from the import and never from thinking; an
/// activity left open there from before is closed with no record.
///
/// Two streams starting the same type in the same facet on segments with the
/// same key (capture clients that align segments to the clock do this) would
/// otherwise share `{type}_{since}`, and the second would be dropped as
/// already written. The second takes `{type}_{stream}_{since}` instead. A held
/// record that names no stream may be an older record of this very stream, so
/// it is taken as the same activity, as before; a record of ours with no
/// stream sits directly under the day and takes that layout's name. The choice
/// holds on every later publish, because the first record stays where it is.
pub(crate) fn append_own_activity(
    context: &ThinkContext,
    facet: &str,
    day: &str,
    record: &mut Map<String, Value>,
) -> Result<OwnAppend, ActivityRecordStoreError> {
    if record.get("stream").and_then(Value::as_str) == Some(STRAVA_STREAM) {
        return Ok(OwnAppend::Skipped);
    }
    let journal = &context.journal;
    // A run's activity can hold a segment that went to the trash while it was
    // open: it is published from the segments still in the journal, or not at
    // all. With every segment there it is published as it is.
    match solstone_core_system::activity_state::rederive(journal, day, record) {
        solstone_core_system::activity_state::Rederived::Unchanged => {}
        solstone_core_system::activity_state::Rederived::Changed(next) => *record = next,
        solstone_core_system::activity_state::Rederived::Removed => {
            return Ok(OwnAppend::Skipped);
        }
    }
    let outcome = append_activity_record_checked(journal, facet, day, record.clone(), || {
        context.checkpoint()
    })?;
    let own = |outcome: AppendOutcome| match outcome {
        AppendOutcome::Written(_) => OwnAppend::Written,
        AppendOutcome::AlreadyExists => OwnAppend::AlreadyExists,
    };
    if !matches!(outcome, AppendOutcome::AlreadyExists) {
        return Ok(own(outcome));
    }
    let outcome = own(outcome);
    let (Some(id), Some(activity)) = (
        record.get("id").and_then(Value::as_str),
        record.get("activity").and_then(Value::as_str),
    ) else {
        return Ok(outcome);
    };
    let stream = record
        .get("stream")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_STREAM);
    let Some(since) = id.strip_prefix(&format!("{activity}_")) else {
        return Ok(outcome);
    };
    let safe_name = !stream.is_empty()
        && !stream.starts_with('.')
        && stream
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !safe_name {
        return Ok(outcome);
    }
    let held_by_another_stream = get_activity_record(journal, facet, day, id)?
        .and_then(|held| {
            held.get("stream")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|held| held != stream);
    if !held_by_another_stream {
        return Ok(outcome);
    }
    let own_id = format!("{activity}_{stream}_{since}");
    record.insert("id".to_owned(), Value::String(own_id));
    append_activity_record_checked(journal, facet, day, record.clone(), || context.checkpoint())
        .map(own)
}

/// Run the talents of activities [`publish_ended_activities`] recorded. A run
/// stopped partway leaves the rest to the retry drain.
pub(crate) fn run_activity_talents(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    talents: Vec<EndedActivity>,
    refresh: bool,
    max_concurrency: i64,
) -> Result<(), String> {
    let mut failures = Vec::new();
    for EndedActivity {
        day,
        facet,
        id,
        claim,
    } in talents
    {
        let result = activity_context(context, &day).and_then(|day_context| {
            crate::activity::run_held(
                &day_context,
                log,
                &id,
                &facet,
                refresh,
                false,
                max_concurrency,
                claim,
            )
        });
        match result {
            Ok(result) if result.failed != 0 => {
                match crate::activity_work::has_persisted_disposition(
                    &context.journal,
                    &day,
                    &facet,
                    &id,
                ) {
                    Ok(true) => {}
                    Ok(false) => failures.extend(
                        result
                            .failed_names
                            .into_iter()
                            .map(|line| line.terminal.unwrap_or(line.source)),
                    ),
                    Err(error) => failures.push(format!("{facet}/{id}: {error}")),
                }
            }
            Ok(_) => {}
            Err(error) => failures.push(format!("{facet}/{id}: {error}")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(solstone_core_cli_copy::display_for(
            context.locale(),
            "journal.cli.think.activity.work.pending",
            [("failures", &(failures.join(", ")))],
        ))
    }
}

/// An activity's lifecycle event. An activity can end in a segment from the
/// next day; its events use the same source day as its record and talents,
/// and keep the triggering segment's day separately.
pub(crate) fn activity_event(
    context: &ThinkContext,
    segment: &str,
    routing_day: &str,
    fields: Map<String, Value>,
) -> Map<String, Value> {
    let mut event = segment_event(context, segment, None, fields);
    event.insert("day".to_owned(), Value::String(routing_day.to_owned()));
    event.insert("segment_day".to_owned(), Value::String(context.day.clone()));
    event
}

/// The context an activity's talents run in: the day its record is filed
/// under, with this run's boundaries and clock.
pub(crate) fn activity_context(context: &ThinkContext, day: &str) -> Result<ThinkContext, String> {
    if let Some(lease) = &context.rebuild
        && day != lease.day
    {
        solstone_core_system::work_intent::update_rebuild(
            &context.journal,
            &lease.day,
            &lease.id,
            &|| context.checkpoint(),
            |progress| {
                progress.activity_days.insert(day.to_owned());
                Ok(())
            },
        )?;
    }
    let mut day_context = context.clone();
    day_context.day = day.to_owned();
    day_context.day_dir = context.journal.join("chronicle").join(day);
    day_context.retry = None;
    Ok(day_context)
}

fn activity_input_changed(
    context: &ThinkContext,
    routing_day: &str,
    facet: &str,
    activity_id: &str,
    record: &Map<String, Value>,
) -> (bool, Option<String>) {
    let input_hash = compute_activity_input_hash(context, routing_day, record);
    let Some(input_hash) = input_hash else {
        return (true, None);
    };
    let path = activity_provenance_path(context, routing_day, facet, activity_id);
    let stored = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("input_hash")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        });
    (stored.as_deref() != Some(&input_hash), Some(input_hash))
}

pub(crate) fn compute_activity_input_hash(
    context: &ThinkContext,
    day: &str,
    record: &Map<String, Value>,
) -> Option<String> {
    solstone_core_system::work_retry::input::activity_hash(&context.journal, day, record)
}

pub(crate) fn activity_provenance_path(
    context: &ThinkContext,
    day: &str,
    facet: &str,
    activity_id: &str,
) -> std::path::PathBuf {
    solstone_core_system::activity_provenance::input_path(&context.journal, day, facet, activity_id)
}

pub(crate) fn write_activity_provenance(
    context: &ThinkContext,
    day: &str,
    facet: &str,
    activity_id: &str,
    input_hash: &str,
) -> Result<(), String> {
    context.checkpoint()?;
    let path = activity_provenance_path(context, day, facet, activity_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(&serde_json::json!({"schema_version":1,"facet":facet,"activity_id":activity_id,"input_hash":input_hash})).map_err(|error| error.to_string())?;
    context.checkpoint()?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

fn log_use_terminal(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    item: &PendingUse,
    outcome: DrainOutcome,
) {
    if let Err(error) = context.checkpoint() {
        log.remember_error(error);
        return;
    }
    let (event, state, carried_cause) = match &outcome {
        DrainOutcome::Finish => ("talent.complete", "finish", None),
        DrainOutcome::Fail { state, cause } => ("talent.fail", *state, cause.clone()),
    };
    let mut fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("name".to_owned(), Value::String(item.name.clone())),
            ("use_id".to_owned(), Value::String(item.use_id.clone())),
            ("state".to_owned(), Value::String(state.to_owned())),
        ]),
    );
    // Segment mode dropped the cause that `log_daily_terminal` already records: the
    // durable record carried only `state`, so `wait_failed` and `error` reached an
    // operator with no way to tell a blocked runtime from a dead socket. The cause is
    // known here — `failure_cause` reads the same use log the daily path consults — so
    // record it under the same `reason_code` key rather than leaving the state to stand
    // in for a reason it does not carry.
    if matches!(outcome, DrainOutcome::Finish)
        && !SEGMENT_FLOOR_TALENTS.contains(&item.name.as_str())
        && let Some(directory) = find_segment_dir(&context.journal, &context.day, segment, stream)
    {
        // A held talent that finished, by its retry or after its input
        // changed, is no longer set aside. Floor talents settle in the writer.
        let unit = segment_talent_unit(context, &directory, segment, stream, &item.name);
        if let Ok(id) = unit.id()
            && solstone_core_system::work_retry::read(&context.journal, &id)
                .is_ok_and(|record| record.is_some_and(|record| record.held))
            && let Err(error) = solstone_core_system::work_retry::reconcile_accepted_checked(
                &context.journal,
                &unit,
                &|| context.checkpoint(),
            )
        {
            log::warn!(
                "{}",
                solstone_core_cli_copy::display_for(
                    context.locale(),
                    "journal.cli.think.log.held.release",
                    [
                        ("name", &item.name),
                        ("segment", &segment),
                        ("error", &error)
                    ]
                )
            );
            log.remember_diagnostic(solstone_core_cli_copy::Diagnostic::new(
                "journal.cli.think.log.reconciliation",
                [("error", &(error))],
            ));
            return;
        }
    }
    if let Some(reason_code) = carried_cause {
        fields.insert("reason_code".to_owned(), Value::String(reason_code));
        // Same gap as daily mode's log_daily_failure: reason_code alone drops whatever
        // else the worker's terminal error carried (e.g. schema_validation's per-field
        // violations).
        if let Some(detail) = use_log_failure_detail(&context.journal, &item.use_id) {
            fields.insert("detail".to_owned(), detail);
        }
    }
    log.log_checked(event, context.event_now_ms(), fields, &|| {
        context.checkpoint()
    });
}

fn log_dispatch(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    item: &PendingUse,
) {
    let fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("name".to_owned(), Value::String(item.name.clone())),
            ("use_id".to_owned(), Value::String(item.use_id.clone())),
        ]),
    );
    let event_ms = context.event_now_ms();
    write_dispatch_event(&context.journal, log, event_ms, fields);
}

pub(super) fn write_dispatch_event(
    journal: &std::path::Path,
    log: &mut RunLogWriter,
    event_ms: i64,
    fields: Map<String, Value>,
) -> bool {
    let emitted = helpers::emit(journal, event_ms, "talent_started", fields.clone());
    log.log("talent.dispatch", event_ms, fields);
    emitted
}

fn merge(into: &mut ModeResult, from: ModeResult) {
    merge_mode_result(into, from);
}

fn log_skip(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    segment: &str,
    reason: &str,
    stream: Option<&str>,
) {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("segment".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("name".to_owned(), Value::String(name.to_owned())),
        ("reason".to_owned(), Value::String(reason.to_owned())),
    ]);
    if let Some(stream) = stream {
        fields.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    log.log("talent.skip", context.event_now_ms(), fields);
}
fn log_sense(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    segment: &str,
    density: &str,
    stream: Option<&str>,
) {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("segment".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("density".to_owned(), Value::String(density.to_owned())),
    ]);
    if let Some(stream) = stream {
        fields.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    log.log("sense.complete", context.event_now_ms(), fields);
}
fn log_change(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    segment: &str,
    change: &str,
    stream: Option<&str>,
) {
    let mut fields = Map::from_iter([
        ("mode".to_owned(), Value::String("segment".to_owned())),
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("change_class".to_owned(), Value::String(change.to_owned())),
    ]);
    if let Some(stream) = stream {
        fields.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    log.log("sense.change_detect", context.event_now_ms(), fields);
}

pub(crate) fn empty_input_sense_output() -> Map<String, Value> {
    Map::from_iter([
        ("density".to_owned(), Value::String("idle".to_owned())),
        ("content_type".to_owned(), Value::String("idle".to_owned())),
        ("activity_summary".to_owned(), Value::String(String::new())),
        ("entities".to_owned(), Value::Array(Vec::new())),
        ("facets".to_owned(), Value::Array(Vec::new())),
        ("speculative_facet".to_owned(), Value::Null),
        ("meeting_detected".to_owned(), Value::Bool(false)),
        ("speakers".to_owned(), Value::Array(Vec::new())),
        (
            "recommend".to_owned(),
            serde_json::json!({"screen_record":false,"speaker_attribution":false}),
        ),
    ])
}

fn is_you_and_opener(text: &str) -> bool {
    let mut words = text.split_whitespace();
    matches!(
        (words.next(), words.next()),
        (Some(first), Some(second))
            if first.eq_ignore_ascii_case("you") && second.eq_ignore_ascii_case("and")
    )
}

fn strip_ascii_prefix_ws<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if !text.get(..prefix.len())?.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = text.get(prefix.len()..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.trim_start())
}

fn normalize_sense_subject(text: &str) -> String {
    const REPORTED_SUBJECTS: [&str; 4] = ["the owner", "the user", "this person", "the person"];
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return text.to_owned();
    }
    if let Some(after_reported) = strip_ascii_prefix_ws(trimmed, "Reported that") {
        let targets_owner = REPORTED_SUBJECTS
            .iter()
            .any(|subject| strip_ascii_prefix_ws(after_reported, subject).is_some());
        if targets_owner {
            let normalized = solstone_core_system::activity_state::normalize_activity_description(
                after_reported,
            );
            if normalized != after_reported {
                return normalized;
            }
        }
        return text.to_owned();
    }
    if is_you_and_opener(trimmed) {
        return text.to_owned();
    }
    let normalized = solstone_core_system::activity_state::normalize_activity_description(trimmed);
    if normalized == trimmed {
        text.to_owned()
    } else {
        normalized
    }
}

fn normalize_sense_subjects(sense: &Map<String, Value>) -> Map<String, Value> {
    let mut normalized = sense.clone();
    if let Some(summary) = normalized
        .get("activity_summary")
        .and_then(Value::as_str)
        .map(normalize_sense_subject)
    {
        normalized.insert("activity_summary".to_owned(), Value::String(summary));
    }
    if let Some(Value::Array(facets)) = normalized.get_mut("facets") {
        for entry in facets {
            if let Some(object) = entry.as_object_mut()
                && let Some(activity) = object
                    .get("activity")
                    .and_then(Value::as_str)
                    .map(normalize_sense_subject)
            {
                object.insert("activity".to_owned(), Value::String(activity));
            }
        }
    }
    normalized
}

pub(crate) fn write_sense_and_change<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    segment: &std::path::Path,
    sense: &Map<String, Value>,
) -> Result<Value, String> {
    if (context.cancel_checkpoint)() {
        return Err("thinking cancelled".into());
    }
    let sense = normalize_sense_subjects(sense);
    let sense = &sense;
    // Source-derived, not measured: sense_splitter.py:13-68 writes these
    // durable projections after both actual and no-input Sense completion.
    let talents = segment.join("talents");
    std::fs::create_dir_all(&talents).map_err(|error| error.to_string())?;

    let mut published = BTreeSet::new();
    let mut assignment_published = false;
    let result: Result<Value, String> = (|| {
        let activity_path = talents.join("activity.md");
        replace_document_checked(
            context,
            &activity_path,
            &solstone_core_format::content::partial_input_document(
                solstone_core_format::stored_markdown::Document::plain(
                    sense
                        .get("activity_summary")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
                sense,
            ),
        )?;
        published.insert(activity_path.clone());

        let sense_json_path = talents.join("sense.json");
        replace_json_checked(context, &sense_json_path, &Value::Object(sense.clone()))?;
        published.insert(sense_json_path.clone());

        let inventory = observe_declared_facet_inventory(&context.journal)
            .map_err(|error| error.to_string())?;
        let (filtered_facets, _) = filter_declared_facets(
            sense.get("facets"),
            &inventory,
            context,
            log,
            segment_name,
            stream,
            true,
        )?;
        let merged_facets = merge_folder_owner_facet(segment, filtered_facets, &inventory);
        let facets_json_path = talents.join("facets.json");
        // If a prior assignment cannot be observed, conservatively refresh the
        // bounded dependency set after the new assignment has been saved.
        let previous_facets = std::fs::read(&facets_json_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let assignment_changed = previous_facets.as_ref() != Some(&merged_facets);
        replace_json_checked(context, &facets_json_path, &merged_facets)?;
        published.insert(facets_json_path.clone());
        assignment_published = assignment_changed;

        let sense_json_path = talents.join("sense.json");
        replace_json_checked(context, &sense_json_path, &Value::Object(sense.clone()))?;
        published.insert(sense_json_path.clone());

        let density_json_path = talents.join("density.json");
        let mut density = serde_json::json!({"classification":sense["density"],"timestamp":solstone_core_time::utc_rfc3339(Timestamp::now())});
        if let Some(partial) = sense.get("partial_input").filter(|v| v.is_object()) {
            density["partial_input"] = partial.clone();
        }
        replace_json_checked(context, &density_json_path, &density)?;
        published.insert(density_json_path.clone());

        if sense
            .get("entities")
            .and_then(Value::as_array)
            .is_some_and(|entities| !entities.is_empty())
        {
            use solstone_core_format::stored_markdown::{Document, Message};
            let mut document = Document::default();
            document.push_str("# ");
            document.push_message(Message::Entities);
            let mut has_entities = false;
            for entity in sense["entities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
            {
                document.push_str(if has_entities { "\n- " } else { "\n\n- " });
                let field = |key: &str| {
                    entity
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                document.push_message(Message::Entity {
                    kind: field("type"),
                    name: field("name"),
                    role: field("role"),
                    source: field("source"),
                    context: field("context"),
                });
                has_entities = true;
            }
            if has_entities {
                let sense_md_path = talents.join("sense.md");
                replace_document_checked(
                    context,
                    &sense_md_path,
                    &solstone_core_format::content::partial_input_document(document, sense),
                )?;
                published.insert(sense_md_path.clone());
            }
        }
        if sense.get("meeting_detected").is_some_and(python_truthy) {
            let speakers_json_path = talents.join("speakers.json");
            replace_json_checked(
                context,
                &speakers_json_path,
                &sense
                    .get("speakers")
                    .cloned()
                    .unwrap_or(Value::Array(Vec::new())),
            )?;
            published.insert(speakers_json_path.clone());
        }
        let stream_resolved = Some(stream.unwrap_or(DEFAULT_STREAM));
        let predecessor = resolve_predecessor(
            &context.journal,
            &context.day,
            stream_resolved,
            segment_name,
        );
        let change = detect_segment_change(
            &context.journal,
            &context.day,
            stream_resolved,
            segment_name,
            segment,
            predecessor,
            &solstone_core_time::utc_rfc3339(Timestamp::now()),
        );
        let change_json_path = segment.join("talents/change.json");
        replace_json_checked(context, &change_json_path, &change)?;
        published.insert(change_json_path.clone());
        publish_segment_topics(
            context,
            log,
            segment_name,
            stream,
            segment,
            sense,
            &mut published,
        )?;
        publish_segment_objects(context, log, segment_name, stream, segment, &mut published);
        publish_terminal_screen_text(context, log, segment_name, stream, segment, &mut published);
        Ok(change)
    })();
    if assignment_published {
        collect_segment_assigned_text(context, log, segment_name, stream, segment, &mut published);
    }
    // Attempt each saved/dependent path once, including saves preceding an
    // ordinary publication error. Indexing never replaces that original error.
    for path in published {
        if (context.cancel_checkpoint)() {
            return Err("thinking cancelled".into());
        }
        let attempt = solstone_core_indexer_store::attempt_saved_publication(
            &context.journal,
            &path,
            |j, p| context.index.enqueue_path(j, p),
        );
        log_index_enqueue(log, context, segment_name, stream, &attempt);
    }
    result
}

fn collect_segment_assigned_text<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    segment: &std::path::Path,
    paths: &mut BTreeSet<std::path::PathBuf>,
) {
    // Only immediate segment and talent files can have SegmentAssigned scope.
    // Nested facet-owned outputs retain their independent classification.
    for directory in [segment.to_path_buf(), segment.join("talents")] {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                log_dependency_discovery_failure(
                    context,
                    log,
                    segment_name,
                    stream,
                    &directory,
                    &error,
                );
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log_dependency_discovery_failure(
                        context,
                        log,
                        segment_name,
                        stream,
                        &directory,
                        &error,
                    );
                    continue;
                }
            };
            let path = entry.path();
            let Some(rel) =
                solstone_core_format::paths::relative_to_journal(&context.journal, &path)
            else {
                continue;
            };
            if solstone_core_format::content::resolve_spec(&rel).is_some_and(|spec| {
                matches!(
                    spec.disposition,
                    solstone_core_format::content::IndexDisposition::Admitted {
                        basis: solstone_core_format::content::ScopeBasis::SegmentAssigned,
                        ..
                    }
                )
            }) {
                match path.metadata() {
                    Ok(metadata) if metadata.is_file() => {
                        paths.insert(path);
                    }
                    Ok(_) => {}
                    Err(error) => log_dependency_discovery_failure(
                        context,
                        log,
                        segment_name,
                        stream,
                        &path,
                        &error,
                    ),
                }
            }
        }
    }
}

fn log_dependency_discovery_failure<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    path: &std::path::Path,
    error: &std::io::Error,
) {
    log.log(
        "index.dependencies_failed",
        context.event_now_ms(),
        segment_event(
            context,
            segment_name,
            stream,
            Map::from_iter([
                ("path".to_owned(), Value::String(path.display().to_string())),
                ("cause".to_owned(), Value::String(error.to_string())),
            ]),
        ),
    );
}

fn publish_segment_topics<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    segment_dir: &std::path::Path,
    sense: &Map<String, Value>,
    published: &mut BTreeSet<std::path::PathBuf>,
) -> Result<(), String> {
    let eval = crate::topics::evaluate_topics(segment_dir, sense);

    for (unreadable_path, cause) in eval.unreadable {
        log.log(
            "sense.transcript_unreadable",
            context.event_now_ms(),
            segment_event(
                context,
                segment_name,
                stream,
                Map::from_iter([
                    (
                        "path".to_owned(),
                        Value::String(unreadable_path.display().to_string()),
                    ),
                    ("cause".to_owned(), Value::String(cause)),
                ]),
            ),
        );
    }

    log.log(
        "sense.quotes",
        context.event_now_ms(),
        segment_event(
            context,
            segment_name,
            stream,
            Map::from_iter([
                ("kept".to_owned(), Value::from(eval.kept_quotes)),
                ("dropped".to_owned(), Value::from(eval.dropped_quotes)),
            ]),
        ),
    );

    let topics_path = segment_dir.join("talents").join("topics.md");
    let markdown = eval.markdown;
    let file_exists = topics_path.exists();

    if markdown.is_some() || file_exists || sense.get("partial_input").is_some_and(Value::is_object)
    {
        let bytes = solstone_core_format::content::partial_input_document(
            markdown.unwrap_or_default(),
            sense,
        );
        use solstone_core_format::content::AdmittedCategory;
        use solstone_core_format::content::topics::{
            PROVENANCE_FILE, SENSE_INPUT_KINDS, parse_kinds, provenance,
        };
        let mut kinds = sense
            .get(SENSE_INPUT_KINDS)
            .and_then(Value::as_array)
            .and_then(|words| parse_kinds(words.iter().map(|word| word.as_str().unwrap_or(""))));
        // Verbatim quotes also depend on the speech reread by the publisher.
        if eval.kept_quotes > 0
            && let Some(kinds) = &mut kinds
        {
            kinds.insert(AdmittedCategory::Transcripts);
        }
        let publish = replace_json_checked(
            context,
            &topics_path.with_file_name(PROVENANCE_FILE),
            &provenance(&bytes, kinds.as_ref()),
        )
        .and_then(|()| replace_document_checked(context, &topics_path, &bytes));
        match publish {
            Ok(()) => {
                published.insert(topics_path);
            }
            Err(cause) => {
                log.log(
                    "sense.topics_failed",
                    context.event_now_ms(),
                    segment_event(
                        context,
                        segment_name,
                        stream,
                        Map::from_iter([
                            (
                                "path".to_owned(),
                                Value::String(topics_path.display().to_string()),
                            ),
                            ("cause".to_owned(), Value::String(cause.clone())),
                        ]),
                    ),
                );
                return Err(cause);
            }
        }
    }
    Ok(())
}

/// Publish `talents/objects.md`, emptying a previous one when nothing
/// qualifies now. Object detection is enrichment: a failed write is logged and
/// never fails the segment.
fn publish_segment_objects<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    segment_dir: &std::path::Path,
    published: &mut BTreeSet<std::path::PathBuf>,
) {
    let objects_path = segment_dir.join("talents").join("objects.md");
    let markdown = crate::objects::segment_objects_markdown(segment_dir);
    if markdown.is_none() && !objects_path.exists() {
        return;
    }
    match replace_document_checked(context, &objects_path, &markdown.unwrap_or_default()) {
        Ok(()) => {
            published.insert(objects_path);
        }
        Err(cause) => log.log(
            "sense.objects_failed",
            context.event_now_ms(),
            segment_event(
                context,
                segment_name,
                stream,
                Map::from_iter([
                    (
                        "path".to_owned(),
                        Value::String(objects_path.display().to_string()),
                    ),
                    ("cause".to_owned(), Value::String(cause)),
                ]),
            ),
        ),
    }
}

/// Publish a terminal-only segment's `talents/screen_text.md`, emptying a
/// previous one when its panes show nothing now. A segment with screen frames
/// is describe's, and is left alone. Like objects, this is enrichment: a failed
/// write is logged and never fails the segment.
fn publish_terminal_screen_text<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment_name: &str,
    stream: Option<&str>,
    segment_dir: &std::path::Path,
    published: &mut BTreeSet<std::path::PathBuf>,
) {
    use crate::terminal_screen_text::{TerminalScreenText, segment_terminal_screen_text};
    let TerminalScreenText::Record(markdown) = segment_terminal_screen_text(segment_dir) else {
        return;
    };
    let path = segment_dir.join("talents").join("screen_text.md");
    if markdown.is_none() && !path.exists() {
        return;
    }
    match replace_document_checked(context, &path, &markdown.unwrap_or_default()) {
        Ok(()) => {
            published.insert(path);
        }
        Err(cause) => log.log(
            "sense.screen_text_failed",
            context.event_now_ms(),
            segment_event(
                context,
                segment_name,
                stream,
                Map::from_iter([
                    ("path".to_owned(), Value::String(path.display().to_string())),
                    ("cause".to_owned(), Value::String(cause)),
                ]),
            ),
        ),
    }
}

fn log_index_enqueue<W: std::io::Write>(
    log: &mut RunLogWriter<W>,
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    attempt: &solstone_core_indexer_store::SavedPublicationAttempt,
) {
    let fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("path".to_owned(), Value::String(attempt.path.clone())),
            (
                "outcome".to_owned(),
                Value::String(attempt.outcome.as_str().to_owned()),
            ),
            (
                "warnings".to_owned(),
                Value::Array(
                    attempt
                        .warnings
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            (
                "cause".to_owned(),
                attempt
                    .cause
                    .as_ref()
                    .map_or(Value::Null, |c| Value::String(c.clone())),
            ),
        ]),
    );
    log.log("index.enqueue", context.event_now_ms(), fields);
}

/// Locate the requested segment. With a stream the lookup is exact; without one
/// the request must name exactly one segment, because two streams sharing a
/// basename is a refusal, not a guess. A body window is never one of them: a
/// request that names no stream means the owner's capture.
fn resolve_segment_dir(
    locale: &str,
    journal: &std::path::Path,
    day: &str,
    segment: &str,
    stream: Option<&str>,
) -> Result<Option<std::path::PathBuf>, String> {
    if stream.is_some() {
        return Ok(find_segment_dir(journal, day, segment, stream));
    }
    let matches = iter_segments(journal, PathOrDay::Day(day))
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|entry| {
            entry.name().to_str() == Some(segment)
                && !solstone_core_segment::is_body_only_segment(entry)
        })
        .map(|entry| entry.path().to_path_buf())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        _ => Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.think.segment.ambiguous",
            &[
                ("day", solstone_core_cli_copy::Value::String(day)),
                ("segment", solstone_core_cli_copy::Value::String(segment)),
                (
                    "count",
                    solstone_core_cli_copy::Value::Cardinal(matches.len() as i64),
                ),
            ],
        )),
    }
}

/// The named stream a discovered segment directory sits under, or `None` for
/// the direct-under-day layout.
pub(crate) fn named_stream<'a>(segment_dir: &'a std::path::Path, day: &str) -> Option<&'a str> {
    let parent = segment_dir.parent()?.file_name()?.to_str()?;
    (parent != day).then_some(parent)
}

fn replace_document_checked(
    context: &ThinkContext,
    path: &std::path::Path,
    document: &solstone_core_format::stored_markdown::Document,
) -> Result<(), String> {
    let checkpoint = || context.checkpoint();
    let companion = path.with_extension("md.display");
    let _publication = solstone_core_journal_io::hold_lock_checked(
        &companion,
        LockOptions::default(),
        &checkpoint,
    )
    .map_err(|e| e.to_string())?;
    // Equal English can have different origins (a literal name and a numbered
    // speaker can both say "Speaker 2"). Retire the old identity before writing.
    // A non-file at the optional companion path is never a readable receipt.
    // Preserve it and still publish the primary Markdown.
    if std::fs::symlink_metadata(&companion).is_ok_and(|entry| !entry.file_type().is_file()) {
        return replace_text_checked(context, path, document);
    }
    match std::fs::remove_file(&companion) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    replace_text_checked(context, path, document)?;
    if let Err(error) = document.save_display_receipt(path) {
        log::warn!(
            "{}",
            solstone_core_cli_copy::display_for(
                context.locale(),
                "journal.cli.think.log.display.metadata",
                [("path", &path.display()), ("error", &error)]
            )
        );
    }
    Ok(())
}

fn replace_text_checked(
    context: &ThinkContext,
    path: &std::path::Path,
    text: &str,
) -> Result<(), String> {
    if (context.cancel_checkpoint)() {
        return Err("thinking cancelled".into());
    }
    replace_text(path, text)
}
fn replace_json_checked(
    context: &ThinkContext,
    path: &std::path::Path,
    value: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if (context.cancel_checkpoint)() {
        return Err("thinking cancelled".into());
    }
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|e| e.to_string())
}

fn replace_text(path: &std::path::Path, text: &str) -> Result<(), String> {
    atomic_replace(path, text.as_bytes(), AtomicWriteOptions::default())
        .map_err(|error| error.to_string())
}

fn segment_event(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    mut fields: Map<String, Value>,
) -> Map<String, Value> {
    fields.insert("mode".to_owned(), Value::String("segment".to_owned()));
    fields.insert("day".to_owned(), Value::String(context.day.clone()));
    fields.insert("segment".to_owned(), Value::String(segment.to_owned()));
    if let Some(stream) = stream {
        fields.insert("stream".to_owned(), Value::String(stream.to_owned()));
    }
    fields
}

fn segment_status(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    total: usize,
    completed: usize,
) -> Map<String, Value> {
    segment_status_with_current(context, segment, stream, total, completed, Vec::new())
}

fn segment_status_with_current(
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    total: usize,
    completed: usize,
    current_agents: Vec<String>,
) -> Map<String, Value> {
    segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("agents_total".to_owned(), Value::from(total)),
            ("agents_completed".to_owned(), Value::from(completed)),
            (
                "current_agents".to_owned(),
                Value::Array(current_agents.into_iter().map(Value::String).collect()),
            ),
        ]),
    )
}

fn log_request_lost(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    name: &str,
    segment: &str,
    stream: Option<&str>,
    use_id: &str,
) {
    let mut fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("name".to_owned(), Value::String(name.to_owned())),
            ("use_id".to_owned(), Value::String(use_id.to_owned())),
            ("state".to_owned(), Value::String("request_lost".to_owned())),
        ]),
    );
    log.log(
        "talent.fail",
        context.event_now_ms(),
        std::mem::take(&mut fields),
    );
}

/// A Strava tile is body data, so Sense never reads it: nothing is written under
/// its talents and no model is called. Such a run can still arrive, queued
/// before tiles counted as body data. A routing retry left pending for the tile
/// ends here as resolved, the way a routing retry ends one, so its day can read
/// complete, and the run ends as an idle one does, with no outputs.
fn end_body_data_sense(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    stream: Option<&str>,
) -> Result<ModeResult, String> {
    let pending = solstone_core_system_health::read_pending_facet_routing(
        &FilesystemHealthLogSource::new(&context.journal),
        &context.day,
    )
    .map_err(|error| error.to_string())?;
    let key = solstone_core_system_health::SegmentIdentity {
        stream: stream.map(str::to_owned),
        segment: segment.to_owned(),
    };
    if pending.value.contains_key(&key) {
        log_routing_state(context, log, segment, stream, false)?;
    }
    log_skip(log, context, "sense", segment, "body_data", stream);
    complete(log, context, segment, stream, ModeResult::default());
    Ok(ModeResult::default())
}

fn complete(
    log: &mut RunLogWriter,
    context: &ThinkContext,
    segment: &str,
    stream: Option<&str>,
    result: ModeResult,
) {
    context.status.update(segment_status(
        context,
        segment,
        stream,
        1,
        result.success + result.failed,
    ));
    let fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([
            ("success".to_owned(), Value::from(result.success)),
            ("failed".to_owned(), Value::from(result.failed)),
            (
                "failed_names".to_owned(),
                Value::Array(
                    result
                        .failed_names
                        .into_iter()
                        .map(|line| Value::String(line.source))
                        .collect(),
                ),
            ),
        ]),
    );
    log.log("completed", context.now_ms, fields.clone());
    let _ = helpers::emit(&context.journal, context.now_ms, "completed", fields);
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

pub(crate) fn filter_declared_facets<W: std::io::Write>(
    facets_value: Option<&Value>,
    inventory: &DeclaredFacetInventory,
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment: &str,
    stream: Option<&str>,
    record_disposition: bool,
) -> Result<(Value, bool), String> {
    let Some(array) = facets_value.and_then(Value::as_array) else {
        return Ok((Value::Array(Vec::new()), false));
    };
    let mut accepted = Vec::new();
    let mut unreadable = false;
    for entry in array {
        let facet = entry
            .get("facet")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if inventory
            .enabled
            .iter()
            .chain(&inventory.muted)
            .any(|slug| slug == facet)
        {
            accepted.push(entry.clone());
            continue;
        }
        let reason = if inventory.unreadable.iter().any(|slug| slug == facet) {
            unreadable = true;
            "declaration_unreadable"
        } else if inventory.malformed.iter().any(|slug| slug == facet) {
            "declaration_malformed"
        } else {
            "destination_missing"
        };
        if record_disposition {
            log.log(
                "facet.routing_rejected",
                context.event_now_ms(),
                segment_event(
                    context,
                    segment,
                    stream,
                    Map::from_iter([
                        ("facet".to_owned(), Value::String(facet.to_owned())),
                        ("reason_code".to_owned(), Value::String(reason.to_owned())),
                        (
                            "next_action".to_owned(),
                            Value::String(solstone_core_cli_copy::render_for(
                                solstone_core_cli_copy::ENGLISH,
                                "journal.cli.think.activity.routing",
                                &[],
                            )),
                        ),
                    ]),
                ),
            );
        }
    }
    if record_disposition && unreadable {
        log_routing_state(context, log, segment, stream, true)?;
    }
    if record_disposition {
        log.finish()?;
    }
    Ok((Value::Array(accepted), unreadable))
}

/// Union a watched folder's owner-chosen facet (when still declared) into a
/// segment's filtered facets list. Content processing may add other facets to a
/// folder segment and never removes the owner's.
pub(crate) fn merge_folder_owner_facet(
    segment_dir: &std::path::Path,
    facets_value: Value,
    inventory: &DeclaredFacetInventory,
) -> Value {
    let activity_path = segment_dir
        .join("talents")
        .join(solstone_core_format::folder::FOLDER_ACTIVITY_FILE);
    let Some(owner_facet) = std::fs::read(&activity_path)
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<solstone_core_format::folder::StoredFolderActivity>(&bytes)
                .ok()
        })
        .and_then(|activity| activity.owner_facet)
        .filter(|slug| {
            inventory
                .enabled
                .iter()
                .chain(&inventory.muted)
                .any(|declared| declared == slug)
        })
    else {
        return facets_value;
    };
    let mut rows = match facets_value {
        Value::Array(rows) => rows,
        _ => Vec::new(),
    };
    let already_present = rows
        .iter()
        .any(|row| row.get("facet").and_then(Value::as_str) == Some(owner_facet.as_str()));
    if !already_present {
        rows.push(serde_json::json!({
            "facet": owner_facet,
            "level": "high",
        }));
    }
    Value::Array(rows)
}

fn log_routing_state<W: std::io::Write>(
    context: &ThinkContext,
    log: &mut RunLogWriter<W>,
    segment: &str,
    stream: Option<&str>,
    pending: bool,
) -> Result<(), String> {
    let mut fields = segment_event(
        context,
        segment,
        stream,
        Map::from_iter([(
            "reason_code".to_owned(),
            Value::String(
                if pending {
                    "declaration_unreadable"
                } else {
                    "routing_resolved"
                }
                .to_owned(),
            ),
        )]),
    );
    if pending {
        let fingerprint = solstone_core_system::catchup::read_raw_input_fingerprint(
            &context.journal,
            &context.day,
        )
        .map_err(|error| error.to_string())?;
        fields.insert("input_fingerprint".to_owned(), Value::String(fingerprint));
        // Direct repair of a previously certified day also needs a retry carrier.
        // Preserve an existing repair's active claim and backoff.
        if !solstone_core_system_health::read_segment_repair_attempted(
            &context.journal,
            &context.day,
        ) {
            use solstone_core_system::catchup::{
                SegmentRepairOutcome, record_segment_repair_attempt, record_segment_repair_outcome,
            };
            let now = context.event_now_ms() as f64 / 1000.0;
            record_segment_repair_attempt(&context.journal, &context.day, now);
            record_segment_repair_outcome(
                &context.journal,
                &context.day,
                SegmentRepairOutcome {
                    success: false,
                    timed_out: false,
                    timeout_seconds: None,
                    ended_at: now,
                    cleared: Some(0),
                    remaining: None,
                },
            );
        }
    }
    log.log(
        if pending {
            "facet.routing_pending"
        } else {
            "facet.routing_resolved"
        },
        context.event_now_ms(),
        fields,
    );
    log.finish()
}

fn failed(name: &str) -> ModeResult {
    ModeResult {
        failed: 1,
        failed_names: vec![name.to_owned().into()],
        ..ModeResult::default()
    }
}

#[cfg(test)]
mod activity_date_tests {
    use super::*;
    use serde_json::json;
    use solstone_core_journal_io::{JournalRoot, operational_log::fold_oplogs};

    #[test]
    fn midnight_activity_persistence_and_lifecycle_events_share_the_source_day() {
        let root = tempfile::tempdir().unwrap();
        let current = "20260907";
        let context = ThinkContext::new(
            root.path(),
            current.to_owned(),
            root.path().join("chronicle").join(current),
            1788758400000,
        )
        .unwrap();
        solstone_core_facets::create_facet(root.path(), "personal", "Personal", "", "", "", None)
            .unwrap();
        let mut log = RunLogWriter::open(root.path(), current, "segment");
        let activity = json!({"id":"terminal_233333_304", "facet":"personal", "source":"cogitate", "state":"ended"});
        let mut talents = Vec::new();
        let mut backfill_cache = BackfillDefaultsCache::default();
        publish_ended_activities(
            &context,
            &mut log,
            "000001_300",
            "20260906",
            None,
            vec![activity.clone()],
            &[activity],
            false,
            true,
            &mut talents,
            &mut backfill_cache,
        )
        .unwrap();
        assert!(talents.is_empty());
        log.finish().unwrap();
        drop(log);
        assert!(
            root.path()
                .join("facets/personal/activities/20260906.jsonl")
                .is_file()
        );
        assert!(
            !root
                .path()
                .join("facets/personal/activities/20260907.jsonl")
                .exists()
        );
        let rows = fold_oplogs(
            JournalRoot::open(root.path()).unwrap(),
            &[jiff::civil::date(2026, 9, 7)],
            |rows: &mut Vec<Value>, _, input| {
                let mut text = String::new();
                input.read_to_string(&mut text)?;
                rows.extend(
                    text.lines()
                        .map(|line| serde_json::from_str::<Value>(line).unwrap()),
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert_eq!(row["day"], "20260906");
            assert_eq!(row["segment_day"], current);
            assert_eq!(row["activity"], "terminal_233333_304");
        }
    }
}

#[cfg(test)]
#[test]
fn from_scratch_floor_retry_consumes_one_durable_owner_cause() {
    use solstone_core_system::{work_intent, work_retry};
    let root = tempfile::tempdir().unwrap();
    let day = "20260101";
    let unit = work_retry::Unit::Segment {
        day: day.into(),
        stream: "desktop".into(),
        segment: "120000_300".into(),
        talent: "documents".into(),
        direct: false,
    };
    let directory = unit.segment_directory(root.path()).unwrap();
    std::fs::create_dir_all(directory.join("talents")).unwrap();
    let sense = directory.join("talents/sense.json");
    std::fs::write(&sense, b"{}").unwrap();
    let context = ThinkContext::new(
        root.path(),
        day.into(),
        root.path().join("chronicle").join(day),
        1,
    )
    .unwrap();
    let input = work_retry::input::current(root.path(), &unit).unwrap();
    work_retry::observe_held(root.path(), &unit, "floor_failed", &input).unwrap();
    assert!(
        floor_retry_attempt(&context, &unit, true)
            .unwrap()
            .is_none()
    );
    for _ in 0..2 {
        let intent = work_intent::register(root.path(), day, true).unwrap();
        let _lease = work_intent::claim_from_scratch(root.path(), day, &intent.id).unwrap();
        assert!(
            floor_retry_attempt(&context, &unit, false)
                .unwrap()
                .is_none()
        );
        let attempt = floor_retry_attempt(&context, &unit, true).unwrap().unwrap();
        assert_eq!(attempt.cause, format!("owner:{}", intent.id));
        work_retry::consume(root.path(), &unit, &attempt, "floor_failed", &input, &[]).unwrap();
        work_retry::finish(root.path(), &unit, &attempt, Some("floor_failed")).unwrap();
        assert!(
            floor_retry_attempt(&context, &unit, true)
                .unwrap()
                .is_none()
        );
        work_intent::finish_from_scratch(root.path(), day, &intent.id, false).unwrap();
    }
}

#[cfg(test)]
#[test]
fn a_body_window_never_answers_a_segment_with_no_stream() {
    use solstone_core_segment::body_twins::{BODY_STREAM, Beside, lay};
    for beside in [
        Beside::Nothing,
        Beside::BodyWindow,
        Beside::DeletedBodyWindow,
        Beside::RetiredCard,
    ] {
        let root = tempfile::tempdir().unwrap();
        let twins = lay(root.path(), "20261010", "090000_300", &[], beside);
        let found = resolve_segment_dir(
            solstone_core_cli_copy::ENGLISH,
            root.path(),
            "20261010",
            "090000_300",
            None,
        );
        if beside == Beside::RetiredCard {
            // An older card isn't a body window: HEAD's answer, a refusal.
            assert!(found.is_err(), "{beside:?}: {found:?}");
        } else {
            assert_eq!(found, Ok(Some(twins.capture.clone())), "{beside:?}");
        }
        // A direct capture isn't refused or answered by a body window.
        std::fs::remove_dir_all(twins.capture.parent().unwrap()).unwrap();
        let direct = root.path().join("chronicle/20261010/090000_300");
        std::fs::create_dir_all(&direct).unwrap();
        let found = resolve_segment_dir(
            solstone_core_cli_copy::ENGLISH,
            root.path(),
            "20261010",
            "090000_300",
            None,
        );
        if beside == Beside::RetiredCard {
            assert!(found.is_err(), "{beside:?}: {found:?}");
        } else {
            assert_eq!(found, Ok(Some(direct.clone())), "{beside:?}");
        }
        // With no capture at all, a body window doesn't stand in.
        std::fs::remove_dir_all(&direct).unwrap();
        let found = resolve_segment_dir(
            solstone_core_cli_copy::ENGLISH,
            root.path(),
            "20261010",
            "090000_300",
            None,
        );
        match beside {
            Beside::RetiredCard => assert_eq!(found, Ok(twins.neighbour.clone()), "{beside:?}"),
            _ => assert_eq!(found, Ok(None), "{beside:?}"),
        }
        // Naming the body stream still reaches it.
        let named = resolve_segment_dir(
            solstone_core_cli_copy::ENGLISH,
            root.path(),
            "20261010",
            "090000_300",
            Some(BODY_STREAM),
        );
        assert_eq!(named, Ok(twins.neighbour.clone()), "{beside:?}");
    }
}

#[cfg(test)]
#[test]
fn a_held_talent_runs_only_in_its_own_retry() {
    use solstone_core_system::work_retry;
    let root = tempfile::tempdir().unwrap();
    let day = "20260101";
    let (segment, stream) = ("120000_300", Some("notes"));
    let directory = root
        .path()
        .join("chronicle")
        .join(day)
        .join("notes")
        .join(segment);
    std::fs::create_dir_all(directory.join("talents")).unwrap();
    std::fs::write(directory.join("talents/sense.json"), b"{}").unwrap();
    let mut context = ThinkContext::new(
        root.path(),
        day.into(),
        root.path().join("chronicle").join(day),
        1,
    )
    .unwrap();
    let screen = segment_talent_unit(&context, &directory, segment, stream, "screen");
    assert!(!held_aside(&context, &directory, segment, stream, "screen").unwrap());
    let input = work_retry::input::current(root.path(), &screen).unwrap();
    work_retry::observe_held(root.path(), &screen, "provider_request_rejected", &input).unwrap();
    // An ordinary run leaves it waiting, even once a cause to retry exists.
    assert!(held_aside(&context, &directory, segment, stream, "screen").unwrap());
    work_retry::request(root.path(), &screen).unwrap();
    assert!(held_aside(&context, &directory, segment, stream, "screen").unwrap());
    let attempt = work_retry::read(root.path(), &screen.id().unwrap())
        .unwrap()
        .unwrap()
        .permission(&input, work_retry::DECLARED_FIXES)
        .unwrap();
    context.retry = Some((screen.clone(), attempt.clone()));
    assert!(!held_aside(&context, &directory, segment, stream, "screen").unwrap());
    assert_eq!(
        floor_retry_attempt(&context, &screen, false).unwrap(),
        Some(attempt)
    );
    // Its siblings dispatch as usual inside that retry.
    let documents = segment_talent_unit(&context, &directory, segment, stream, "documents");
    assert!(
        floor_retry_attempt(&context, &documents, false)
            .unwrap()
            .is_none()
    );
}

#[cfg(test)]
const SPAN_NOW: i64 = 1_786_615_200_000;

/// One email of the owner's, written to each `(name, address)`, the way the
/// email source lands it.
#[cfg(test)]
fn span_exchange(segment_dir: &std::path::Path, to: &[(&str, &str)]) {
    std::fs::create_dir_all(segment_dir).unwrap();
    let (names, addresses): (Vec<&str>, Vec<&str>) = to.iter().copied().unzip();
    let line = serde_json::json!({
        "start": "00:00:00", "speaker": "You", "is_owner": true, "to": names, "text": "Body",
        "email": {"provider": "gmail", "to_addresses": addresses},
    });
    std::fs::write(
        segment_dir.join("conversation_transcript.jsonl"),
        format!("{}\n{line}\n", serde_json::json!({"source": "email"})),
    )
    .unwrap();
}

#[cfg(test)]
#[test]
fn consolidate_historical_backfill_span_promotes_multi_episode_entities_and_seeds_dated_observations()
 {
    let root = tempfile::tempdir().unwrap();
    let journal = root.path();
    std::fs::create_dir_all(journal.join("config")).unwrap();
    std::fs::write(
        journal.join("config/journal.json"),
        r#"{"identity":{"name":"Jordan Hale","preferred":"Jordan","timezone":"UTC"}}"#,
    )
    .unwrap();
    solstone_core_facets::create_facet(journal, "personal", "Personal", "Life", "", "", None)
        .unwrap();
    solstone_core_facets::create_facet(journal, "work", "Work", "Engineering", "", "", None)
        .unwrap();

    // Seed Samira Patel in `personal` across 2019 so `work` exercises cross-facet `Exists` attachment.
    for (day, act_id, summary) in [
        (
            "20190412",
            "act_p1",
            "Trail run and coffee catch-up with Samira Patel.",
        ),
        (
            "20191103",
            "act_p2",
            "Thanksgiving travel planning with Samira Patel.",
        ),
    ] {
        let seg = journal
            .join("chronicle")
            .join(day)
            .join("import.email/090000_0");
        std::fs::create_dir_all(&seg).unwrap();
        span_exchange(&seg, &[("Samira Patel", "samira.personal@example.com")]);
        let act_dir = journal.join("facets/personal/activities");
        std::fs::create_dir_all(&act_dir).unwrap();
        std::fs::write(
            act_dir.join(format!("{day}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "id": act_id,
                    "facet": "personal",
                    "activity": "email",
                    "stream": "import.email",
                    "segments": ["090000_0"],
                    "summary": summary,
                    "participation": [
                        {"name": "Samira Patel", "role": "corresponded", "source": "transcript", "confidence": 1.0, "context": summary}
                    ]
                })
            ),
        )
        .unwrap();
    }

    // Also include a 2020 deterministic transaction segment and verify 0-LLM boundary.
    let tx_seg = journal.join("chronicle/20200214/import.email/120000_0");
    std::fs::create_dir_all(&tx_seg).unwrap();
    std::fs::write(
        tx_seg.join("transaction_transcript.md"),
        "Order #4821 receipt: $42.00\n",
    )
    .unwrap();
    assert!(solstone_core_segment::day_holds_nothing_the_owner_did(
        journal, "20200214"
    ));

    for (day, act_id, summary, extra_person) in [
        (
            "20210510",
            "act_1",
            "Reviewed storage lease PR with Samira Patel.",
            ("Newsletter Bot", "noreply@substack.com"),
        ),
        (
            "20210819",
            "act_2",
            "Planned v1.2 chaos benchmark with Samira Patel.",
            ("One Off Bystander", "bystander@example.com"),
        ),
    ] {
        let seg = journal
            .join("chronicle")
            .join(day)
            .join("import.email/100000_0");
        std::fs::create_dir_all(&seg).unwrap();
        span_exchange(
            &seg,
            &[("Samira Patel", "samira@acme.example"), extra_person],
        );

        let act_dir = journal.join("facets/work/activities");
        std::fs::create_dir_all(&act_dir).unwrap();
        std::fs::write(
            act_dir.join(format!("{day}.jsonl")),
            format!(
                "{}\n",
                serde_json::json!({
                    "id": act_id,
                    "facet": "work",
                    "activity": "email",
                    "stream": "import.email",
                    "segments": ["100000_0"],
                    "summary": summary,
                    "participation": [
                        {"name": "Samira Patel", "role": "corresponded", "source": "transcript", "confidence": 1.0, "context": summary},
                        {"name": extra_person.0, "role": "mentioned", "source": "transcript", "confidence": 0.8, "context": "CC bystander"}
                    ]
                })
            ),
        )
        .unwrap();
    }

    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();

    let scoped = solstone_core_facets::list_scoped_facet_entities(journal, "work", false, false)
        .expect("scoped entities");
    let names: Vec<&str> = scoped
        .iter()
        .filter_map(|e| e.identity.get("name").and_then(Value::as_str))
        .collect();
    assert_eq!(names, vec!["Samira Patel"]);
    assert_eq!(
        scoped[0].identity["emails"],
        serde_json::json!(["samira.personal@example.com", "samira@acme.example"])
    );

    let scoped_personal =
        solstone_core_facets::list_scoped_facet_entities(journal, "personal", false, false)
            .expect("personal entities");
    assert_eq!(scoped_personal.len(), 1);
    assert_eq!(scoped_personal[0].entity_id, scoped[0].entity_id);

    // Re-running consolidation is idempotent and hits the watermark without duplicating observations.
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();

    let obs = solstone_core_facets::read_live_observations(
        journal,
        "work",
        &scoped[0].entity_dir,
        solstone_core_facets::ObservationReadQuery::default(),
    )
    .expect("observations");
    assert_eq!(obs.items.len(), 2);
    assert_eq!(obs.items[0].source_day.as_deref(), Some("20210819"));
    assert_eq!(obs.items[1].source_day.as_deref(), Some("20210510"));
}

#[cfg(test)]
fn span_journal() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("config")).unwrap();
    std::fs::write(
        root.path().join("config/journal.json"),
        r#"{"identity":{"name":"Jordan Hale","preferred":"Jordan","timezone":"UTC"}}"#,
    )
    .unwrap();
    solstone_core_facets::create_facet(root.path(), "work", "work", "", "", "", None).unwrap();
    root
}

/// One email of the owner's to `name` at `address`, and one activity over it
/// that names them as corresponded.
#[cfg(test)]
fn span_email(
    journal: &std::path::Path,
    day: &str,
    seg: &str,
    act: &str,
    name: &str,
    address: &str,
) {
    use std::io::Write;
    span_exchange(
        &journal
            .join("chronicle")
            .join(day)
            .join("import.email")
            .join(seg),
        &[(name, address)],
    );
    let acts = journal.join("facets/work/activities");
    std::fs::create_dir_all(&acts).unwrap();
    let row = serde_json::json!({
        "id": act, "facet": "work", "activity": "email", "stream": "import.email",
        "segments": [seg], "summary": format!("Mail {act}."),
        "participation": [{"name": name, "role": "corresponded", "source": "transcript",
            "confidence": 1.0, "context": "Corresponded in email."}]
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(acts.join(format!("{day}.jsonl")))
        .unwrap();
    writeln!(file, "{row}").unwrap();
}

/// Each entity the facet holds: its name, type and addresses.
#[cfg(test)]
fn span_people(journal: &std::path::Path) -> Vec<(String, String, Value)> {
    solstone_core_facets::list_scoped_facet_entities(journal, "work", false, false)
        .unwrap()
        .iter()
        .map(|e| {
            (
                e.identity["name"].as_str().unwrap_or_default().to_owned(),
                e.identity["type"].as_str().unwrap_or_default().to_owned(),
                e.identity.get("emails").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// Every note the facet's entities hold, as written on disk.
#[cfg(test)]
fn span_notes(journal: &std::path::Path) -> String {
    solstone_core_facets::list_scoped_facet_entities(journal, "work", false, false)
        .unwrap()
        .iter()
        .map(|e| {
            std::fs::read_to_string(
                journal
                    .join("facets/work/entities")
                    .join(&e.entity_dir)
                    .join("observations.jsonl"),
            )
            .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
#[test]
fn a_note_that_quotes_a_mail_header_records_no_correspondent() {
    let root = span_journal();
    let journal = root.path();
    std::fs::create_dir_all(journal.join("streams")).unwrap();
    std::fs::write(
        journal.join("streams/vault.json"),
        r#"{"name":"vault","kind":"observer","host":null,"platform":null,"created_at":0,"last_day":null,"last_segment":null,"seq":1,"source":"obsidian","cid":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    )
    .unwrap();
    let seg = journal.join("chronicle/20210510/vault/100000_60");
    std::fs::create_dir_all(&seg).unwrap();
    // A synced note: the owner's own text, which pastes a fare alert.
    std::fs::write(
        seg.join("note_transcript.md"),
        "From: Skyline Air <deals@skyline.example>\nTo: Pat Lee <pat@example.org>\n\nTrip ideas, to read later.\n",
    )
    .unwrap();
    let mut record = serde_json::json!({"id":"a","title":"Trip ideas","segments":["100000_60"]})
        .as_object()
        .unwrap()
        .clone();
    populate_backfill_activity_defaults(journal, "20210510", "work", "vault", &mut record);
    assert!(!record.contains_key("participation"), "{record:?}");
}

#[cfg(test)]
#[test]
fn an_email_names_who_it_was_written_to_and_never_the_owner_or_a_copied_bystander() {
    let root = span_journal();
    let journal = root.path();
    let seg = journal.join("chronicle/20210510/import.email/100000_0");
    std::fs::create_dir_all(&seg).unwrap();
    let line = serde_json::json!({
        "speaker": "You", "is_owner": true, "text": "Body",
        "to": ["Jordan Hale", "Doe, Jane"], "cc": ["Casey Bystander"],
        "reply_to": {"author": "Sam Reyes", "from": "sam@example.com", "text": "Earlier."},
        "email": {"from": "jordan@hale.example", "to_addresses": ["jordan@hale.example", "jane@example.com"]},
    });
    std::fs::write(
        seg.join("conversation_transcript.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();
    let mut record = serde_json::json!({"id":"a","title":"Mail","segments":["100000_0"]})
        .as_object()
        .unwrap()
        .clone();
    populate_backfill_activity_defaults(journal, "20210510", "work", "import.email", &mut record);
    let rows: Vec<(&str, &str, &str)> = record["participation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["name"].as_str().unwrap(),
                p["role"].as_str().unwrap(),
                p["context"].as_str().unwrap(),
            )
        })
        .collect();
    // A "Last, First" recipient is one person: names come from the exchange's
    // own list of recipients, never from splitting a header line.
    assert_eq!(
        rows,
        vec![
            ("Doe, Jane", "corresponded", "Corresponded in email."),
            ("Sam Reyes", "corresponded", "Corresponded in email reply."),
        ]
    );
}

#[cfg(test)]
#[test]
fn two_correspondents_who_share_a_name_are_counted_and_addressed_apart() {
    let root = span_journal();
    let journal = root.path();
    let (a, b) = ("chris.a@example.com", "chris.b@example.net");
    span_email(journal, "20210510", "100000_0", "act_1", "Chris Park", a);
    span_email(journal, "20210510", "110000_0", "act_2", "Chris Park", b);
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    // One mail each is nobody seen twice.
    assert_eq!(span_people(journal), vec![]);
    // The first of them writes again on another day: that one is a person,
    // and the other's address is no part of them.
    span_email(journal, "20210517", "100000_0", "act_3", "Chris Park", a);
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let one = vec![(
        "Chris Park".to_owned(),
        "Person".to_owned(),
        serde_json::json!([a]),
    )];
    assert_eq!(span_people(journal), one);
    // Read again now that a Chris Park exists: the other sender is still not them.
    std::fs::remove_file(journal.join("awareness/backfill_span_state.json")).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(span_people(journal), one);
    // The other writes again too. The facet holds one person of a name, so
    // they get no entity, and nothing of theirs is added to the first's.
    span_email(journal, "20210524", "100000_0", "act_4", "Chris Park", b);
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(span_people(journal), one);
    assert!(!span_notes(journal).contains("act_2") && !span_notes(journal).contains("act_4"));
    // And again, now that both have earned it.
    std::fs::remove_file(journal.join("awareness/backfill_span_state.json")).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(span_people(journal), one);
    assert!(!span_notes(journal).contains("act_4"));
}

#[cfg(test)]
#[test]
fn two_recurring_correspondents_of_one_name_read_together_stay_apart() {
    let root = span_journal();
    let journal = root.path();
    let (a, b) = ("chris.a@example.com", "chris.b@example.net");
    span_email(journal, "20210510", "100000_0", "act_1", "Chris Park", a);
    span_email(journal, "20210510", "110000_0", "act_2", "Chris Park", b);
    span_email(journal, "20210517", "100000_0", "act_3", "Chris Park", a);
    span_email(journal, "20210517", "110000_0", "act_4", "Chris Park", b);
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    // Both earned it in one reading. One of them is the Chris Park the facet
    // holds; the other is not folded into them.
    assert_eq!(
        span_people(journal),
        vec![(
            "Chris Park".to_owned(),
            "Person".to_owned(),
            serde_json::json!([a]),
        )]
    );
    let notes = span_notes(journal);
    assert!(
        notes.contains("act_1") && notes.contains("act_3"),
        "{notes}"
    );
    assert!(
        !notes.contains("act_2") && !notes.contains("act_4"),
        "{notes}"
    );
}

#[cfg(test)]
#[test]
fn a_mailing_or_one_busy_day_makes_no_person() {
    let root = span_journal();
    let journal = root.path();
    // A service signing as a person, from a shared mailbox, week after week.
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Sarah from Acme",
            "hello@acme.example",
        );
        span_email(
            journal,
            day,
            "110000_0",
            &format!("{act}b"),
            "Lee Chen",
            "updates@shop.example",
        );
    }
    // A real person, but only ever on one day.
    span_email(
        journal,
        "20210601",
        "100000_0",
        "act_5",
        "Samira Patel",
        "samira@example.com",
    );
    span_email(
        journal,
        "20210601",
        "110000_0",
        "act_6",
        "Samira Patel",
        "samira@example.com",
    );
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(span_people(journal), vec![]);
    assert!(signs_as_a_service("Dana via LinkedIn"));
    assert!(!is_bulk_or_automated_actor("Notes from Underground", None));
    assert!(is_bulk_or_automated_actor(
        "Acme",
        Some("no-reply@acme.example")
    ));
    assert!(is_bulk_or_automated_actor(
        "Acme",
        Some("acme-notifications@mail.example")
    ));
    assert!(!is_bulk_or_automated_actor(
        "Samira Patel",
        Some("samira.patel@example.com")
    ));
    assert!(!is_bulk_or_automated_actor(
        "Teamo Reyes",
        Some("teamo@example.com")
    ));
}

#[cfg(test)]
#[test]
fn a_note_the_owner_removed_is_not_written_again() {
    let root = span_journal();
    let journal = root.path();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Samira Patel",
            "samira@example.com",
        );
    }
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let scoped =
        solstone_core_facets::list_scoped_facet_entities(journal, "work", false, false).unwrap();
    let live = || {
        solstone_core_facets::read_live_observations(
            journal,
            "work",
            &scoped[0].entity_dir,
            solstone_core_facets::ObservationReadQuery::default(),
        )
        .unwrap()
        .items
        .len()
    };
    assert_eq!(live(), 2);
    // Every note is retired, the way a removal leaves it on disk.
    let path = journal
        .join("facets/work/entities")
        .join(&scoped[0].entity_dir)
        .join("observations.jsonl");
    let retired = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut row: Value = serde_json::from_str(line).unwrap();
            row["retired"] = serde_json::json!({"at": 1, "by": "owner"});
            format!("{row}\n")
        })
        .collect::<String>();
    std::fs::write(&path, retired).unwrap();
    assert_eq!(live(), 0);
    // The same history read again, as after any change to it.
    std::fs::remove_file(journal.join("awareness/backfill_span_state.json")).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(live(), 0);
}

#[cfg(test)]
#[test]
fn a_note_the_owner_reworded_is_not_written_again_as_it_was() {
    let root = span_journal();
    let journal = root.path();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Samira Patel",
            "samira@example.com",
        );
    }
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let scoped =
        solstone_core_facets::list_scoped_facet_entities(journal, "work", false, false).unwrap();
    let path = journal
        .join("facets/work/entities")
        .join(&scoped[0].entity_dir)
        .join("observations.jsonl");
    // Each note reworded, the way an edit leaves it on disk: the new words,
    // and what it said before kept as its history.
    let reworded = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| {
            let mut row: Value = serde_json::from_str(line).unwrap();
            row["history"] = serde_json::json!([{
                "content": row["content"], "observed_at": row["observed_at"],
                "source_day": row["source_day"],
            }]);
            row["content"] = Value::String(format!("In my words, {}", row["id"]));
            format!("{row}\n")
        })
        .collect::<String>();
    std::fs::write(&path, reworded).unwrap();
    std::fs::remove_file(journal.join("awareness/backfill_span_state.json")).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let live = solstone_core_facets::read_live_observations(
        journal,
        "work",
        &scoped[0].entity_dir,
        solstone_core_facets::ObservationReadQuery::default(),
    )
    .unwrap()
    .items;
    assert_eq!(live.len(), 2, "{live:?}");
}

#[cfg(test)]
#[test]
fn the_span_is_read_again_when_its_inputs_change_and_not_when_the_day_does() {
    let root = span_journal();
    let journal = root.path();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Samira Patel",
            "samira@example.com",
        );
    }
    let stamp = journal.join("awareness/backfill_span_state.json");
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let first = std::fs::read(&stamp).unwrap();
    let written = std::fs::metadata(&stamp).unwrap().modified().unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW + 3 * 86_400_000).unwrap();
    assert_eq!(std::fs::read(&stamp).unwrap(), first);
    assert_eq!(
        std::fs::metadata(&stamp).unwrap().modified().unwrap(),
        written
    );

    // Hiding a name is a change to what the span may promote.
    let days = vec!["20210510".to_owned(), "20210517".to_owned()];
    let facets = vec!["work".to_owned()];
    let before = backfill_span_fingerprint(journal, &days, &facets);
    std::fs::write(
        journal.join("facets/work/hidden-names.json"),
        r#"{"names":["Samira Patel"]}"#,
    )
    .unwrap();
    assert_ne!(backfill_span_fingerprint(journal, &days, &facets), before);
}

#[cfg(test)]
#[test]
fn a_refusal_is_reported_once_and_the_span_is_tried_again_when_it_changes() {
    let root = span_journal();
    let journal = root.path();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Samira Patel",
            "samira@example.com",
        );
    }
    // What the facet hides cannot be read, so nobody in it can be promoted.
    let hides = journal.join("facets/work/hidden-names.json");
    std::fs::write(&hides, "{").unwrap();
    let stamp = journal.join("awareness/backfill_span_state.json");
    let failed = || {
        serde_json::from_slice::<Value>(&std::fs::read(&stamp).unwrap()).unwrap()["failed"].clone()
    };
    assert!(consolidate_historical_backfill_span(journal, SPAN_NOW).is_err());
    assert!(failed().is_string());
    // Nothing it reads has changed: it is not read again, nor reported again.
    let written = std::fs::metadata(&stamp).unwrap().modified().unwrap();
    assert_eq!(
        consolidate_historical_backfill_span(journal, SPAN_NOW),
        Ok(())
    );
    assert_eq!(
        std::fs::metadata(&stamp).unwrap().modified().unwrap(),
        written
    );
    assert_eq!(span_people(journal), vec![]);
    // The file is put right, which is a change to what the span reads.
    std::fs::write(&hides, r#"{"names":[]}"#).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert!(failed().is_null());
    assert_eq!(span_people(journal).len(), 1);
}

#[cfg(test)]
#[test]
fn a_correspondent_an_older_version_read_from_header_like_text_counts_toward_nobody() {
    use std::io::Write;
    let root = span_journal();
    let journal = root.path();
    let acts = journal.join("facets/work/activities");
    std::fs::create_dir_all(&acts).unwrap();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        std::fs::create_dir_all(
            journal
                .join("chronicle")
                .join(day)
                .join("import.text/100000_0"),
        )
        .unwrap();
        let row = serde_json::json!({
            "id": act, "facet": "work", "activity": "note", "stream": "import.text",
            "segments": ["100000_0"], "summary": "Trip ideas.",
            "participation": [{"name": "Skyline Air", "role": "corresponded",
                "source": "transcript", "confidence": 1.0,
                "context": "Corresponded in message."}]
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(acts.join(format!("{day}.jsonl")))
            .unwrap();
        writeln!(file, "{row}").unwrap();
    }
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    assert_eq!(span_people(journal), vec![]);
}

#[cfg(test)]
#[test]
fn a_name_a_talent_shortened_still_finds_its_address() {
    let root = span_journal();
    let journal = root.path();
    for (day, act) in [("20210510", "act_1"), ("20210517", "act_2")] {
        span_email(
            journal,
            day,
            "100000_0",
            act,
            "Christopher Adams",
            "chris@example.com",
        );
    }
    // The participation talent later wrote the name its own way on one day.
    let path = journal.join("facets/work/activities/20210517.jsonl");
    let shortened = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Christopher Adams", "Chris Adams");
    std::fs::write(&path, shortened).unwrap();
    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    // One person, seen on two days, with the address the exchanges give.
    assert_eq!(
        span_people(journal),
        vec![(
            "Christopher Adams".to_owned(),
            "Person".to_owned(),
            serde_json::json!(["chris@example.com"]),
        )]
    );
}

#[cfg(test)]
#[test]
fn someone_a_note_to_self_only_names_is_mentioned_and_never_a_correspondent() {
    let root = span_journal();
    let journal = root.path();
    let seg = journal.join("chronicle/20210510/import.email/100000_0");
    std::fs::create_dir_all(seg.join("talents")).unwrap();
    let header = serde_json::json!({"source": "email", "email": {"account": "me@hale.example"}});
    let line = serde_json::json!({
        "speaker": "You", "is_owner": true, "text": "Ask Sam Reyes about the lease.",
        "to": ["me@hale.example"],
        "email": {"from": "me@hale.example", "to_addresses": ["me@hale.example"]},
    });
    std::fs::write(
        seg.join("conversation_transcript.jsonl"),
        format!("{header}\n{line}\n"),
    )
    .unwrap();
    std::fs::write(
        seg.join("talents/sense.json"),
        serde_json::json!({"entities": [
            {"type": "Person", "name": "Sam Reyes", "level": "high", "context": "Named in the note."}
        ]})
        .to_string(),
    )
    .unwrap();
    let mut record = serde_json::json!({"id":"a","title":"Mail","segments":["100000_0"]})
        .as_object()
        .unwrap()
        .clone();
    populate_backfill_activity_defaults(journal, "20210510", "work", "import.email", &mut record);
    let rows: Vec<(&str, &str)> = record["participation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap(), p["role"].as_str().unwrap()))
        .collect();
    // No message passed between the owner and the person the text names.
    assert_eq!(rows, vec![("Sam Reyes", "mentioned")]);
}

#[cfg(test)]
#[test]
fn the_mailbox_an_email_was_synced_from_is_the_owners() {
    let root = span_journal();
    let journal = root.path();
    let seg = journal.join("chronicle/20210510/import.email/100000_0");
    std::fs::create_dir_all(&seg).unwrap();
    // A note to self: the recipient's label is the bare address, and the
    // journal has no entity for the owner yet.
    let header = serde_json::json!({"source": "email", "email": {"account": "Me@Hale.example"}});
    let line = serde_json::json!({
        "speaker": "You", "is_owner": true, "text": "Body",
        "to": ["me@hale.example", "Sam Reyes"],
        "email": {"from": "me@hale.example", "to_addresses": ["me@hale.example", "sam@example.com"]},
    });
    std::fs::write(
        seg.join("conversation_transcript.jsonl"),
        format!("{header}\n{line}\n"),
    )
    .unwrap();
    let mut record = serde_json::json!({"id":"a","title":"Mail","segments":["100000_0"]})
        .as_object()
        .unwrap()
        .clone();
    populate_backfill_activity_defaults(journal, "20210510", "work", "import.email", &mut record);
    let names: Vec<&str> = record["participation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Sam Reyes"]);
}

#[cfg(test)]
#[test]
fn email_owner_aliases_are_excluded_in_defaults_and_span_consolidation_including_mixed_days() {
    let root = span_journal();
    let journal = root.path();

    // Secondary alias persisted by the email source under config/email/account.json.
    std::fs::create_dir_all(journal.join("config/email")).unwrap();
    std::fs::write(
        journal.join("config/email/account.json"),
        serde_json::json!({
            "provider": "gmail",
            "account": "me@hale.example",
            "mailbox": "[Gmail]/All Mail",
            "owner_aliases": ["me@hale.example", "alt@old-domain.example"],
            "connected_at": "2026-11-27T00:00:00Z"
        })
        .to_string(),
    )
    .unwrap();

    // 1. populate_backfill_activity_defaults excludes secondary alias even under another label.
    let seg = journal.join("chronicle/20210510/import.email/100000_0");
    std::fs::create_dir_all(&seg).unwrap();
    let header = serde_json::json!({"source": "email", "email": {"account": "me@hale.example"}});
    let line = serde_json::json!({
        "speaker": "You", "is_owner": true, "text": "Body",
        "to": ["J. Old Alias", "Sam Reyes"],
        "email": {"from": "me@hale.example", "to_addresses": ["alt@old-domain.example", "sam@example.com"]},
    });
    std::fs::write(
        seg.join("conversation_transcript.jsonl"),
        format!("{header}\n{line}\n"),
    )
    .unwrap();
    let mut record = serde_json::json!({"id":"a","title":"Mail","segments":["100000_0"]})
        .as_object()
        .unwrap()
        .clone();
    populate_backfill_activity_defaults(journal, "20210510", "work", "import.email", &mut record);
    let names: Vec<&str> = record["participation"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Sam Reyes"]);

    // 2. Mixed closed live + historical email days: span consolidation promotes
    // recurring email correspondents on mixed days, excludes secondary owner aliases,
    // and never reads live-stream activities.
    for (day, act_id) in [("20210601", "act_m1"), ("20210602", "act_m2")] {
        // Live recording segment makes the day mixed (not backfill-only).
        let live = journal.join(format!("chronicle/{day}/default/090000_300"));
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("audio.jsonl"), "{}\n").unwrap();

        // Historical email segment on the same mixed day.
        span_email(journal, day, "140000_0", act_id, "Morgan Patel", "morgan@example.com");

        // Append both a live-stream activity (must be ignored by span consolidation)
        // and an email activity addressing the owner's secondary alias (must be excluded).
        let acts_path = journal.join(format!("facets/work/activities/{day}.jsonl"));
        let mut content = std::fs::read_to_string(&acts_path).unwrap_or_default();
        content.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "id": format!("live_{day}"),
                "stream": "default",
                "title": "Live meeting",
                "segments": ["090000_300"],
                "participation": [{"name": "Live Only Person", "role": "heard", "context": "Spoke live."}]
            })
        ));
        let alias_seg = journal.join(format!("chronicle/{day}/import.email/150000_0"));
        std::fs::create_dir_all(&alias_seg).unwrap();
        std::fs::write(
            alias_seg.join("conversation_transcript.jsonl"),
            format!(
                "{}\n{}\n",
                serde_json::json!({"source": "email", "email": {"account": "me@hale.example"}}),
                serde_json::json!({
                    "speaker": "You", "is_owner": true, "text": "Self note",
                    "to": ["J. Old Alias"],
                    "email": {"from": "me@hale.example", "to_addresses": ["alt@old-domain.example"]},
                })
            ),
        )
        .unwrap();
        content.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "id": format!("alias_{day}"),
                "stream": "import.email",
                "title": "Self alias note",
                "segments": ["150000_0"],
                "participation": [{"name": "J. Old Alias", "role": "corresponded", "context": "Note to alias."}]
            })
        ));
        std::fs::write(&acts_path, content).unwrap();
    }

    consolidate_historical_backfill_span(journal, SPAN_NOW).unwrap();
    let promoted: Vec<String> = span_people(journal)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    assert_eq!(
        promoted,
        vec!["Morgan Patel".to_owned()],
        "mixed-day email correspondent promoted while live-only person and owner alias stayed out"
    );
}
