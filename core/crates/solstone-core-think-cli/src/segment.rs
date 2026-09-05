// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_facets::{AppendOutcome, append_activity_record};
use solstone_core_journal_io::{AtomicWriteOptions, DEFAULT_STREAM, atomic_replace};
use solstone_core_system::activity_state::ActivityStateMachine;
use solstone_core_system_health::{
    DataState, FilesystemHealthLogSource, SEGMENT_FLOOR_TALENTS, detect_segment_change,
    find_segment_dir, is_floor_talent_capped, read_segment_data_state, resolve_predecessor,
};
use solstone_core_talent_config::{
    TalentConfig, TalentFilter, get_output_path, load_talent_configs,
};
use solstone_core_timeline::{
    AttemptOutcome, AttemptStateV1, SegmentSelectorV1, TimelineError, TimelineLockRequest,
    TimelineLockSubject, acquire_timeline_locks, publish_continuation_summary,
    resolve_segment_binding, segment_directory,
};

use crate::context::{DispatchFailure, ThinkContext};
use crate::dispatch::{
    DrainOutcome, ModeResult, PendingUse, dispatch_direct, drain_with_deadline_observed,
    merge_mode_result, runtime,
};
use crate::helpers;
use crate::run_log::RunLogWriter;

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
    let configs = load_talent_configs(
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
            failed_names: vec!["sense (not_configured)".to_owned()],
            ..ModeResult::default()
        });
    };
    let binding = match resolve_segment_binding(
        &context.journal,
        &SegmentSelectorV1 {
            day: context.day.clone(),
            segment: segment.to_owned(),
            stream: stream.map(ToOwned::to_owned),
        },
    ) {
        Ok(binding) => binding,
        Err(TimelineError::SegmentNotFound { .. }) => {
            return Ok(ModeResult {
                failed: 1,
                failed_names: vec!["sense (missing_segment)".to_owned()],
                ..ModeResult::default()
            });
        }
        Err(error) => return Err(error.to_string()),
    };
    let segment_dir =
        segment_directory(&context.journal, &binding).map_err(|error| error.to_string())?;
    let stream = match (stream, binding.stream.as_str()) {
        (Some(_), resolved) => Some(resolved),
        (None, DEFAULT_STREAM) => None,
        (None, resolved) => Some(resolved),
    };
    let state =
        read_segment_data_state(&context.journal, &context.day, segment, stream, Utc::now());
    let in_flight = state.0.values().any(|value| {
        value == DataState::Pending.as_str() || value == DataState::Analyzing.as_str()
    });
    if in_flight {
        // Source-derived, not measured: thinking.py:1485-1499 records this raw-media gate.
        log_skip(log, context, "sense", segment, "raw_media_pending", stream);
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
    if solstone_core_talent_runtime::check_segment_has_no_input(
        &context.journal,
        &context.day,
        segment,
        stream,
        &load,
        Utc::now(),
    ) {
        // Source-derived, not measured: thinking.py:1536-1584 writes a
        // schema-valid idle artifact and change record before terminalizing.
        let sense_json = empty_input_sense_output();
        let change = write_sense_and_change(context, &binding, &segment_dir, &sense_json)?;
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
                failed_names: vec!["sense (request_lost)".to_owned()],
                ..ModeResult::default()
            };
            complete(log, context, segment, stream, result.clone());
            return Ok(result);
        }
        Err(DispatchFailure::Unavailable) => {
            log_skip(log, context, "sense", segment, "send_failed", stream);
            let result = ModeResult {
                failed: 1,
                failed_names: vec!["sense (send)".to_owned()],
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
    let result = sense_use.map_or_else(ModeResult::default, |sense_use| {
        drain_with_deadline_observed(
            context,
            &runtime,
            vec![sense_use],
            timeout,
            &mut |item, outcome| {
                log_use_terminal(log, context, segment, stream, item, outcome);
            },
        )
    });
    context.status.update(segment_status(
        context,
        segment,
        stream,
        1,
        result.success + result.failed,
    ));
    if result.failed != 0 {
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
    let sense_json: Value = match std::fs::read_to_string(&sense_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
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
    let change = write_sense_and_change(context, &binding, &segment_dir, sense_object)?;
    log_sense(log, context, segment, density, stream);
    let change_class = change
        .get("change_class")
        .and_then(Value::as_str)
        .unwrap_or_default();
    log_change(log, context, segment, change_class, stream);
    if change_class == "redundant" && !refresh {
        if let Some(previous) = change
            .pointer("/predecessor/segment")
            .and_then(Value::as_str)
        {
            let attempt = AttemptStateV1 {
                attempt_id: format!(
                    "continuation-{}-{}-{segment}",
                    std::process::id(),
                    context.now_ms
                ),
                input_digest: String::new(),
                started_at_ms: context.now_ms,
                finished_at_ms: None,
                outcome: AttemptOutcome::Running,
                detail: String::new(),
            };
            publish_continuation_summary(
                &context.journal,
                binding.clone(),
                previous.to_owned(),
                context.now_ms,
                attempt,
            )
            .map_err(|error| error.to_string())?;
        }
        log_skip(log, context, "*", segment, "change_redundant", stream);
        complete(log, context, segment, stream, result.clone());
        return Ok(result);
    }
    let timeline_only = density == "idle" && !refresh;
    if timeline_only {
        log_skip(
            log,
            context,
            "non_timeline_talents",
            segment,
            "density_idle",
            stream,
        );
    }
    let mut total = result;
    let mut agents = select_agents(
        context,
        log,
        &by_name,
        sense_object,
        &segment_dir,
        segment,
        stream,
        refresh,
        timeline_only,
    )?;
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
                total.failed += 1;
                total.failed_names.push(format!("{} (send)", config.key));
            }
            Err(DispatchFailure::NotClaimed { use_id }) => {
                // Source-derived, not measured: thinking.py:1951-1960 treats
                // a selected talent's unclaimed request separately from send failure.
                log_request_lost(log, context, &config.key, segment, stream, &use_id);
                total.failed += 1;
                total
                    .failed_names
                    .push(format!("{} (request_lost)", config.key));
            }
        }
        if max_concurrency != 0 && pending.len() as i64 >= max_concurrency {
            drain_selected(
                context,
                &runtime,
                &mut pending,
                timeout,
                &mut total,
                &mut |item, outcome| {
                    log_use_terminal(log, context, segment, stream, item, outcome);
                },
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
        &mut |item, outcome| {
            log_use_terminal(log, context, segment, stream, item, outcome);
        },
    );
    complete(log, context, segment, stream, total.clone());
    Ok(total)
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
    timeline_only: bool,
) -> Result<Vec<&'a TalentConfig>, String> {
    let mut selected = Vec::new();
    if timeline_only {
        if let Some(config) = by_name.get("timeline:segment_summary") {
            selected.push(config);
        } else {
            log_skip(
                log,
                context,
                "timeline:segment_summary",
                segment,
                "no_config",
                stream,
            );
        }
        return Ok(selected);
    }
    let source = FilesystemHealthLogSource::new(&context.journal);
    for name in SEGMENT_FLOOR_TALENTS {
        let Some(config) = by_name.get(*name) else {
            log_skip(log, context, name, segment, "no_config", stream);
            continue;
        };
        if !refresh
            && is_floor_talent_capped(&source, &context.day, stream, segment, name)
                .map_err(|error| error.to_string())?
                .value
        {
            // Source-derived, not measured: thinking.py:1824-1838 skips a
            // repeatedly failed floor talent unless an explicit refresh retries it.
            log_skip(log, context, name, segment, "capped", stream);
            continue;
        }
        selected.push(config);
    }
    for name in ["timeline:segment_summary", "entities:detection"] {
        if let Some(config) = by_name.get(name) {
            selected.push(config);
        } else {
            log_skip(log, context, name, segment, "no_config", stream);
        }
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
    let speakers_recommended = recommend
        .get("speaker_attribution")
        .is_some_and(python_truthy);
    if speakers_recommended && has_audio_embeddings(segment_dir) {
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
        // Source-derived, not measured: thinking.py:1905-1930 keeps both the
        // non-recommended and no-audio-embeddings branches distinguishable.
        log_skip(
            log,
            context,
            "speaker_attribution",
            segment,
            "not_recommended",
            stream,
        );
    }
    Ok(selected)
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

fn drain_selected(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: &mut Vec<PendingUse>,
    timeout: Option<Duration>,
    total: &mut ModeResult,
    observer: &mut dyn FnMut(&PendingUse, DrainOutcome),
) {
    if pending.is_empty() {
        return;
    }
    merge(
        total,
        drain_with_deadline_observed(context, runtime, std::mem::take(pending), timeout, observer),
    );
}

enum AgentDispatch {
    Skipped,
    Pending(PendingUse),
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
    let generate = config.metadata.get("type").and_then(Value::as_str) == Some("generate");
    let mut request = Map::from_iter([
        ("day".to_owned(), Value::String(context.day.clone())),
        ("segment".to_owned(), Value::String(segment.to_owned())),
        ("schedule".to_owned(), Value::String("segment".to_owned())),
    ]);
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
        if refresh {
            request.insert("refresh".to_owned(), Value::Bool(true));
        }
    } else if let Some(output) = config.metadata.get("output") {
        request.insert("output".to_owned(), output.clone());
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
                    let Some((segment, stream)) =
                        queue.lock().expect("repair queue lock").pop_front()
                    else {
                        break;
                    };
                    match run(
                        context,
                        &mut log,
                        &segment,
                        refresh,
                        stream.as_deref(),
                        max_concurrency,
                        timeout,
                        false,
                        skip_talents,
                    ) {
                        Ok(result) => {
                            merge(&mut aggregate.lock().expect("repair result lock"), result)
                        }
                        Err(_) => {
                            // Source-derived, not measured: thinking.py:677-682
                            // folds one worker exception into that segment's failure
                            // while the remaining repairs continue.
                            let mut aggregate = aggregate.lock().expect("repair result lock");
                            aggregate.failed += 1;
                            aggregate
                                .failed_names
                                .push(format!("{segment} (exception)"));
                        }
                    }
                }
            });
        }
    });
    Ok(aggregate.lock().expect("repair result lock").clone())
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
) -> Result<ModeResult, String> {
    run_repair_batch(
        context,
        log,
        segments.clone(),
        refresh,
        max_concurrency,
        segment_workers,
        timeout,
        skip_talents,
    )
    .map(|mut result| {
        if let Err(error) = replay_activity_state(
            context,
            log,
            &segments,
            refresh,
            max_concurrency,
            no_activity_prompts,
            false,
        ) {
            crate::dispatch::record_followup_failure(&mut result, "activity replay", &error);
        }
        result
    })
}

/// Replay durable Sense output through the activity-state tail.
///
/// Source-derived, not measured: `thinking.py:379-435` persists the state
/// machine, appends ended activity records, and runs their eligible prompts;
/// `thinking.py:594-634` performs the same replay after a segment batch.
/// Think owns this write as the native port of `solstone.think.thinking`; the
/// system crate supplies only the deterministic state machine and the facets
/// crate owns append-only activity-record publication.
pub(crate) fn replay_activity_state(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segments: &[(String, Option<String>)],
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
    hydrate_existing: bool,
) -> Result<(), String> {
    let mut ordered = segments.to_vec();
    ordered.sort();
    let mut machines = BTreeMap::new();
    if hydrate_existing {
        machines.insert(None, ActivityStateMachine::hydrate(Some(&context.journal)));
    }
    for (segment, stream) in ordered {
        let Some(segment_dir) =
            find_segment_dir(&context.journal, &context.day, &segment, stream.as_deref())
        else {
            continue;
        };
        let sense_path = segment_dir.join("talents/sense.json");
        let Ok(bytes) = std::fs::read(&sense_path) else {
            continue;
        };
        let Ok(sense) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if !valid_activity_sense(&sense) {
            continue;
        }
        let machine = if hydrate_existing {
            machines
                .get_mut(&None)
                .expect("direct replay machine exists")
        } else {
            machines.entry(stream.clone()).or_default()
        };
        // Source-derived, not measured: thinking.py:394-405 captures this
        // routing day before update closes a carried-over activity.
        let routing_day = machine
            .last_segment_day()
            .unwrap_or(&context.day)
            .to_owned();
        let changes = machine.update(&sense, &segment, &context.day, None, context.now_ms);
        if hydrate_existing {
            // Source-derived, not measured: thinking.py:408-411 deliberately
            // logs and continues when snapshot persistence fails; ended records
            // and their prompts must still be published.
            if let Err(error) = persist_activity_state(context, machine) {
                log::debug!("failed to write activity state snapshot: {error}");
            }
        }
        persist_ended_activities(
            context,
            log,
            &segment,
            &routing_day,
            changes,
            &machine.completed_activities(),
            refresh,
            max_concurrency,
            skip_activity_prompts,
        )?;
    }
    if !hydrate_existing {
        flush_replay_machines(
            context,
            log,
            machines,
            refresh,
            max_concurrency,
            skip_activity_prompts,
        )?;
    }
    Ok(())
}

fn valid_activity_sense(sense: &Value) -> bool {
    // Source-derived, not measured: thinking.py:547-591 accepts a durable
    // replay projection with only these two required keys.
    ["density", "content_type"]
        .into_iter()
        .all(|key| sense.get(key).is_some())
}

fn flush_replay_machines(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    mut machines: BTreeMap<Option<String>, ActivityStateMachine>,
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<(), String> {
    let today = Utc
        .timestamp_millis_opt(context.now_ms)
        .single()
        .unwrap_or_else(Utc::now)
        .format("%Y%m%d")
        .to_string();
    for (stream, machine) in &mut machines {
        let Some(last_segment) = machine.last_segment_key().map(ToOwned::to_owned) else {
            continue;
        };
        let import_stream = stream
            .as_deref()
            .is_some_and(|name| name.starts_with("import."));
        if !import_stream && context.day >= today {
            continue;
        }
        // Source-derived, not measured: thinking.py:438-469 and 604-634
        // close finite import streams and completed historical days after the
        // batch, never an ongoing current-day observer stream.
        let routing_day = machine
            .last_segment_day()
            .unwrap_or(&context.day)
            .to_owned();
        let changes = machine.close_active(&last_segment, context.now_ms);
        persist_ended_activities(
            context,
            log,
            &last_segment,
            &routing_day,
            changes,
            &machine.completed_activities(),
            refresh,
            max_concurrency,
            skip_activity_prompts,
        )?;
    }
    Ok(())
}

fn persist_activity_state(
    context: &ThinkContext,
    machine: &ActivityStateMachine,
) -> Result<(), String> {
    let path = context.journal.join("awareness/activity_state.json");
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&machine.snapshot()).map_err(|error| error.to_string())?;
    atomic_replace(&path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn persist_ended_activities(
    context: &ThinkContext,
    log: &mut RunLogWriter,
    segment: &str,
    routing_day: &str,
    changes: Vec<Value>,
    completed: &[Value],
    refresh: bool,
    max_concurrency: i64,
    skip_activity_prompts: bool,
) -> Result<(), String> {
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
            segment_event(
                context,
                segment,
                None,
                Map::from_iter([
                    ("activity".to_owned(), Value::String(id.to_owned())),
                    ("facet".to_owned(), Value::String(facet.to_owned())),
                    ("state".to_owned(), Value::String("ended".to_owned())),
                ]),
            ),
        );
        let Some(record) = completed.iter().rev().find_map(|record| {
            (record.get("id").and_then(Value::as_str) == Some(id)
                && record.get("facet").and_then(Value::as_str) == Some(facet))
            .then(|| record.as_object().cloned())
            .flatten()
        }) else {
            continue;
        };
        let written = matches!(
            append_activity_record(&context.journal, facet, routing_day, record.clone())
                .map_err(|error| error.to_string())?,
            AppendOutcome::Written(_)
        );
        log.log(
            "activity.persisted",
            context.now_ms,
            segment_event(
                context,
                segment,
                None,
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
                segment_event(
                    context,
                    segment,
                    None,
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
        } else {
            let (changed, input_hash) =
                activity_input_changed(context, routing_day, facet, id, &record);
            if !(written || refresh || changed) {
                log.log(
                    "activity.unchanged",
                    context.now_ms,
                    segment_event(
                        context,
                        segment,
                        None,
                        Map::from_iter([("activity".to_owned(), Value::String(id.to_owned()))]),
                    ),
                );
                continue;
            }
            let mut prompt_context = ThinkContext::new_with_event_clock(
                &context.journal,
                routing_day.to_owned(),
                context.journal.join("chronicle").join(routing_day),
                context.now_ms,
                context.event_clock(),
            )?;
            prompt_context.talent_root = context.talent_root.clone();
            prompt_context.apps_root = context.apps_root.clone();
            prompt_context.cortex = context.cortex.clone();
            prompt_context.index = context.index.clone();
            prompt_context.status = context.status.clone();
            let result =
                crate::activity::run(&prompt_context, log, id, facet, refresh, max_concurrency)?;
            if result.failed == 0
                && let Some(input_hash) = input_hash
            {
                write_activity_provenance(context, routing_day, facet, id, &input_hash)?;
            }
        }
    }
    Ok(())
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

fn compute_activity_input_hash(
    context: &ThinkContext,
    day: &str,
    record: &Map<String, Value>,
) -> Option<String> {
    let spans = record
        .get("segments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let day_dir = context.journal.join("chronicle").join(day);
    let mut inputs = Vec::new();
    for segment in &spans {
        let mut sense = Vec::new();
        let mut entries = std::fs::read_dir(&day_dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for direct in entries {
            let segment_dir = if direct.file_name().and_then(|name| name.to_str()) == Some(segment)
                && direct.is_dir()
            {
                direct
            } else {
                let nested = direct.join(segment);
                if !nested.is_dir() {
                    continue;
                }
                nested
            };
            let path = segment_dir.join("talents/sense.json");
            let relative = path
                .strip_prefix(&context.journal)
                .ok()?
                .display()
                .to_string();
            match std::fs::read(&path) {
                Ok(bytes) => sense.push(serde_json::json!({"path":relative,"sha256":format!("{:x}", Sha256::digest(&bytes)),"size":bytes.len()})),
                Err(_) => sense.push(serde_json::json!({"path":relative,"missing":true})),
            }
        }
        if sense.is_empty() {
            sense.push(serde_json::json!({"segment":segment,"missing":true}));
        }
        inputs.push(serde_json::json!({"segment":segment,"sense":sense}));
    }
    let identity = serde_json::json!({
        "activity_id": record.get("id"), "activity": record.get("activity"),
        "facet": record.get("facet"), "segments": spans, "segment_inputs": inputs,
    });
    serde_json::to_string(&canonical_json(identity))
        .ok()
        .map(|text| format!("{:x}", Sha256::digest(text.as_bytes())))
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        primitive => primitive,
    }
}

fn activity_provenance_path(
    context: &ThinkContext,
    day: &str,
    facet: &str,
    activity_id: &str,
) -> std::path::PathBuf {
    context
        .journal
        .join("chronicle")
        .join(day)
        .join("health/talent-provenance/activity-inputs")
        .join(provenance_component(facet))
        .join(format!("{}.json", provenance_component(activity_id)))
}

/// Match `urllib.parse.quote(value, safe="._-")` used by the reference
/// provenance path, keeping an activity identifier confined to one filename.
fn provenance_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => {
                encoded.push(char::from(byte));
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    encoded
}

fn write_activity_provenance(
    context: &ThinkContext,
    day: &str,
    facet: &str,
    activity_id: &str,
    input_hash: &str,
) -> Result<(), String> {
    let path = activity_provenance_path(context, day, facet, activity_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec(&serde_json::json!({"schema_version":1,"facet":facet,"activity_id":activity_id,"input_hash":input_hash})).map_err(|error| error.to_string())?;
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
    if let Some(reason_code) = carried_cause {
        fields.insert("reason_code".to_owned(), Value::String(reason_code));
    }
    log.log(event, context.event_now_ms(), fields);
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

fn empty_input_sense_output() -> Map<String, Value> {
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
        (
            "emotional_register".to_owned(),
            Value::String("neutral".to_owned()),
        ),
    ])
}

fn write_sense_and_change(
    context: &ThinkContext,
    binding: &solstone_core_timeline::SegmentBindingV1,
    segment: &std::path::Path,
    sense: &Map<String, Value>,
) -> Result<Value, String> {
    // Source-derived, not measured: sense_splitter.py:13-68 writes these
    // durable projections after both actual and no-input Sense completion.
    let _locks = acquire_timeline_locks(
        &context.journal,
        TimelineLockRequest {
            subjects: vec![TimelineLockSubject::Segment(binding.clone())],
            ..TimelineLockRequest::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let talents = segment.join("talents");
    std::fs::create_dir_all(&talents).map_err(|error| error.to_string())?;
    replace_text(
        &talents.join("activity.md"),
        sense
            .get("activity_summary")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    replace_json(
        &talents.join("facets.json"),
        &sense
            .get("facets")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    )?;
    replace_json(&talents.join("sense.json"), &Value::Object(sense.clone()))?;
    replace_json(
        &talents.join("density.json"),
        &serde_json::json!({"classification":sense["density"],"timestamp":Utc::now().to_rfc3339()}),
    )?;
    if sense
        .get("entities")
        .and_then(Value::as_array)
        .is_some_and(|entities| !entities.is_empty())
    {
        let lines = sense
            .get("entities")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|entity| {
                format!(
                    "- {} — {} (role={}, source={}) — {}",
                    entity
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    entity
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    entity
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    entity
                        .get("source")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    entity
                        .get("context")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            replace_text(
                &talents.join("sense.md"),
                &format!("# Sense Entities\n\n{}", lines.join("\n")),
            )?;
        }
    }
    if sense.get("meeting_detected").is_some_and(python_truthy) {
        replace_json(
            &talents.join("speakers.json"),
            &sense
                .get("speakers")
                .cloned()
                .unwrap_or(Value::Array(Vec::new())),
        )?;
    }
    let stream = Some(binding.stream.as_str());
    let predecessor = resolve_predecessor(&context.journal, &binding.day, stream, &binding.segment);
    let change = detect_segment_change(
        &context.journal,
        &binding.day,
        stream,
        &binding.segment,
        segment,
        predecessor,
        &Utc::now().to_rfc3339(),
    );
    replace_json(&segment.join("talents/change.json"), &change)?;
    Ok(change)
}

fn replace_text(path: &std::path::Path, text: &str) -> Result<(), String> {
    atomic_replace(path, text.as_bytes(), AtomicWriteOptions::default())
        .map_err(|error| error.to_string())
}

fn replace_json(path: &std::path::Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
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
                Value::Array(result.failed_names.into_iter().map(Value::String).collect()),
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

fn failed(name: &str) -> ModeResult {
    ModeResult {
        failed: 1,
        failed_names: vec![name.to_owned()],
        ..ModeResult::default()
    }
}
