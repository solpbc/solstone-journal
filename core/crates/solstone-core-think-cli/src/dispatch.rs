// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
use serde_json::{Map, Value};
use solstone_core_cortex_client::{CortexRequest, TimedOutUse, UseEndState, read_use_events};
use solstone_core_talent_config::{TalentConfig, get_output_path};

use crate::context::{DispatchFailure, ThinkContext};

/// Daily, weekly, and cadence all use the reference's 610-second batch deadline.
pub(crate) const DEFAULT_THINK_TIMEOUT: Duration = Duration::from_secs(610);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModeResult {
    pub(crate) success: usize,
    pub(crate) failed: usize,
    pub(crate) timed_out: bool,
    pub(crate) failed_names: Vec<String>,
    pub(crate) success_names: Vec<String>,
    pub(crate) skipped: usize,
    pub(crate) skipped_names: Vec<String>,
    pub(crate) applicable_units: BTreeSet<(String, Option<String>)>,
    /// Daily units that are terminal either from this invocation or a
    /// previously folded terminal record.
    pub(crate) terminal_units: BTreeSet<(String, Option<String>)>,
    /// Subset of [`Self::terminal_units`] whose deterministic failure cap
    /// made them terminal for daily-completion purposes.
    pub(crate) capped_units: BTreeSet<(String, Option<String>)>,
}

pub(crate) fn item_label(name: &str, facet: Option<&str>) -> String {
    facet.map_or_else(|| name.to_owned(), |facet| format!("{name}/{facet}"))
}

pub(crate) fn merge_mode_result(into: &mut ModeResult, from: ModeResult) {
    into.success += from.success;
    into.failed += from.failed;
    into.timed_out |= from.timed_out;
    into.failed_names.extend(from.failed_names);
    into.success_names.extend(from.success_names);
    into.skipped += from.skipped;
    into.skipped_names.extend(from.skipped_names);
    into.applicable_units.extend(from.applicable_units);
    into.terminal_units.extend(from.terminal_units);
    into.capped_units.extend(from.capped_units);
}

pub(crate) fn record_followup_failure(result: &mut ModeResult, label: &str, error: &str) {
    result.failed += 1;
    result.failed_names.push(format!("{label} ({error})"));
}

fn use_log_error(journal: &Path, use_id: &str) -> Option<String> {
    let events = read_use_events(journal, use_id).ok()?;
    events.iter().rev().find_map(|event| {
        (event.get("event").and_then(Value::as_str) == Some("error")
            && event
                .get("terminal")
                .and_then(Value::as_bool)
                .unwrap_or(true))
        .then(|| {
            event
                .get("reason_code")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    event
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
        })?
    })
}

pub(crate) fn timeout_cause(timeout: &TimedOutUse) -> &'static str {
    match timeout {
        TimedOutUse::LostAtDeadline { .. } => "lost",
        TimedOutUse::GenuineTimeout { .. } => "timeout",
    }
}

/// The durable use log is the authority on what a talent actually did.
///
/// A failed wait tells us nothing about any individual use, so attributing the
/// wait's failure to every pending one records work that already finished as
/// `wait_failed`. Measured on the owner's journal 2026-09-03: 17 of 327
/// `talent.fail` records were provably false, their outputs present on disk and
/// their own logs ending in `finish` — many a legitimate `no_input` skip.
fn use_log_terminal(journal: &Path, use_id: &str) -> Option<String> {
    let events = read_use_events(journal, use_id).ok()?;
    events.iter().rev().find_map(|event| {
        let name = event.get("event").and_then(Value::as_str)?;
        let terminal = event
            .get("terminal")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        (matches!(name, "finish" | "error") && terminal).then(|| name.to_owned())
    })
}

pub(crate) fn failure_cause(journal: &Path, use_id: &str, fallback: &str) -> String {
    if let Some(error) = use_log_error(journal, use_id) {
        return error;
    }
    fallback.to_owned()
}

pub(crate) fn named_failure(label: &str, cause: &str) -> String {
    format!("{label} ({cause})")
}

pub(crate) struct PendingUse {
    pub(crate) use_id: String,
    pub(crate) name: String,
    pub(crate) facet: Option<String>,
    pub(crate) output_path: Option<PathBuf>,
    pub(crate) index_output: bool,
}

/// Mirrors `thinking.py:2058`: daily, weekly, and cadence use this persistence
/// shaping helper; segment, activity, and flush deliberately set output directly.
pub(crate) fn apply_output_persistence(
    config: &TalentConfig,
    request: &mut Map<String, Value>,
    force: bool,
) {
    if config.metadata.get("accumulate") == Some(&Value::Bool(true)) {
        return;
    }
    let declared_output = config.metadata.get("output").and_then(Value::as_str);
    let is_generate = config.metadata.get("type").and_then(Value::as_str) == Some("generate");
    if is_generate || declared_output.is_some() {
        request.insert(
            "output".to_owned(),
            Value::String(declared_output.unwrap_or("md").to_owned()),
        );
        if force {
            request.insert("refresh".to_owned(), Value::Bool(true));
        }
    }
}

pub(crate) fn grouped(configs: Vec<TalentConfig>) -> BTreeMap<i64, Vec<TalentConfig>> {
    let mut groups = BTreeMap::new();
    for config in configs {
        groups
            .entry(
                config
                    .metadata
                    .get("priority")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            )
            .or_insert_with(Vec::new)
            .push(config);
    }
    for configs in groups.values_mut() {
        configs.sort_by(|left, right| left.key.cmp(&right.key));
    }
    groups
}

/// Python's `fnmatch.fnmatch` equivalent for `exclude_streams` metadata.
pub(crate) fn excluded(config: &TalentConfig, stream: Option<&str>) -> bool {
    let Some(stream) = stream else {
        return false;
    };
    config
        .metadata
        .get("exclude_streams")
        .and_then(Value::as_array)
        .is_some_and(|patterns| {
            patterns.iter().filter_map(Value::as_str).any(|pattern| {
                glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(stream))
            })
        })
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, String> {
    // Wait talks to Callosum over a Unix socket. Time-only is enough for the
    // claim poll, and then UnixStream::connect panics: "IO is disabled".
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| error.to_string())
}

pub(crate) fn dispatch(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    config: &TalentConfig,
    schedule: &str,
    facet: Option<&str>,
    force: bool,
    extra: Map<String, Value>,
) -> Result<PendingUse, DispatchFailure> {
    let mut request = extra;
    request
        .entry("day".to_owned())
        .or_insert_with(|| Value::String(context.day.clone()));
    request.insert("schedule".to_owned(), Value::String(schedule.to_owned()));
    let mut env = Map::from_iter([("SOL_DAY".to_owned(), Value::String(context.day.clone()))]);
    if let Some(facet) = facet {
        request.insert("facet".to_owned(), Value::String(facet.to_owned()));
        env.insert("SOL_FACET".to_owned(), Value::String(facet.to_owned()));
    }
    request.insert("env".to_owned(), Value::Object(env));
    apply_output_persistence(config, &mut request, force);
    if schedule == "weekly" && config.key == "weekly_reflection" {
        let day = NaiveDate::parse_from_str(&context.day, "%Y%m%d").expect("validated day");
        let week_start =
            day - ChronoDuration::days(i64::from(day.weekday().num_days_from_sunday()));
        let week_start = week_start.format("%Y%m%d").to_string();
        request.insert("day".to_owned(), Value::String(week_start.clone()));
        request.insert("output".to_owned(), Value::String("md".to_owned()));
        request.insert(
            "output_path".to_owned(),
            Value::String(
                context
                    .journal
                    .join("reflections/weekly")
                    .join(format!("{week_start}.md"))
                    .display()
                    .to_string(),
            ),
        );
        if let Some(env) = request.get_mut("env").and_then(Value::as_object_mut) {
            env.insert("SOL_DAY".to_owned(), Value::String(week_start));
        }
    }
    if let Some(format) = request.get("output").and_then(Value::as_str)
        && !request.contains_key("output_path")
    {
        request.insert(
            "output_path".to_owned(),
            Value::String(
                get_output_path(
                    &context.day_dir,
                    &config.key,
                    None,
                    Some(format),
                    facet,
                    None,
                )
                .display()
                .to_string(),
            ),
        );
    }
    let prompt = if config.metadata.get("type").and_then(Value::as_str) == Some("generate") {
        String::new()
    } else if schedule == "weekly" && config.key == "weekly_reflection" && facet.is_none() {
        // Source-derived, not measured: thinking.py:2606 and 2834 include
        // the weekly reflection's ISO week-start and the day's input summary.
        format!(
            "Running scheduled weekly reflection for {}: {}.",
            request
                .get("day")
                .and_then(Value::as_str)
                .and_then(iso_day)
                .unwrap_or_else(|| iso_day(&context.day).unwrap_or_else(|| context.day.clone())),
            day_input_summary(&context.day_dir),
        )
    } else if schedule == "cadence" {
        // Source-derived, not measured: thinking.py:3012-3025 gives cadence
        // cogitate talents their own prompt form, without a day summary.
        format!(
            "Running cadence task for {}.",
            iso_day(&context.day).unwrap_or_else(|| context.day.clone())
        )
    } else if let Some(facet) = facet {
        // Source-derived, not measured: thinking.py:2134/2294 and
        // 2606/2728-2730 retain the facet context and input summary; the
        // name-keyed weekly reflection uses its week-start day here too.
        format!(
            "Processing facet '{facet}' for {}: {}. Use get_facet('{facet}') to load context.",
            if schedule == "weekly" && config.key == "weekly_reflection" {
                request
                    .get("day")
                    .and_then(Value::as_str)
                    .and_then(iso_day)
                    .unwrap_or_else(|| iso_day(&context.day).unwrap_or_else(|| context.day.clone()))
            } else {
                iso_day(&context.day).unwrap_or_else(|| context.day.clone())
            },
            day_input_summary(&context.day_dir),
        )
    } else {
        // Source-derived, not measured: thinking.py:2134/2425 and
        // 2606/2836 include the day's recording summary for scheduled work.
        format!(
            "Running scheduled task for {}: {}.",
            iso_day(&context.day).unwrap_or_else(|| context.day.clone()),
            day_input_summary(&context.day_dir),
        )
    };
    let request = CortexRequest::new(prompt, config.key.clone()).with_config(request);
    let output_path = request
        .config
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let index_output = config.metadata.get("type").and_then(Value::as_str) == Some("generate")
        && config.metadata.get("output").and_then(Value::as_str) != Some("json");
    let use_id = context.cortex.dispatch(runtime, &request)?;
    Ok(PendingUse {
        use_id,
        name: config.key.clone(),
        facet: facet.map(ToOwned::to_owned),
        output_path,
        index_output,
    })
}

fn iso_day(day: &str) -> Option<String> {
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .ok()
        .map(|day| day.format("%Y-%m-%d").to_string())
}

fn day_input_summary(day_dir: &std::path::Path) -> String {
    let mut segments = Vec::new();
    collect_segment_keys(day_dir, &mut segments);
    let total_seconds = segments
        .iter()
        .filter_map(|segment| segment.split_once('_'))
        .filter_map(|(_, duration)| duration.parse::<u64>().ok())
        .sum::<u64>();
    if segments.is_empty() {
        return "No recordings".to_owned();
    }
    let duration = if total_seconds < 60 {
        format!("~{total_seconds} seconds")
    } else if total_seconds < 3_600 {
        format!("~{} minutes", (total_seconds as f64 / 60.0).round() as u64)
    } else {
        format!("~{:.1} hours", total_seconds as f64 / 3_600.0)
    };
    let count = segments.len();
    if count < 5 || total_seconds < 1_800 {
        format!(
            "Light activity: {count} segment{}, {duration}",
            if count == 1 { "" } else { "s" }
        )
    } else {
        format!("{count} segments, {duration}")
    }
}

fn collect_segment_keys(directory: &std::path::Path, keys: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        if name
            .split_once('_')
            .is_some_and(|(_, duration)| duration.parse::<u64>().is_ok())
        {
            keys.push(name.to_owned());
        } else {
            collect_segment_keys(&path, keys);
        }
    }
}

/// Segment, activity, and flush intentionally own their output fields.  The
/// reference's `thinking.py:2058` limits `apply_output_persistence` to daily,
/// weekly, and cadence, so these callers must not use it.
pub(crate) fn dispatch_direct(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    prompt: String,
    config: Map<String, Value>,
    facet: Option<&str>,
) -> Result<PendingUse, DispatchFailure> {
    let request = CortexRequest::new(prompt, name.to_owned()).with_config(config);
    let output_path = request
        .config
        .get("output_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let index_output = request.config.contains_key("output")
        && request.config.get("output").and_then(Value::as_str) != Some("json");
    let use_id = context.cortex.dispatch(runtime, &request)?;
    Ok(PendingUse {
        use_id,
        name: name.to_owned(),
        facet: facet.map(ToOwned::to_owned),
        output_path,
        index_output,
    })
}

/// Per-use drain result visible to a mode-local observer. Fail carries the
/// canonical `state` string for a run-log terminal; accounting stays in the drain.
/// Per-use drain result visible to a mode-local observer.
///
/// `Fail` carries the canonical `state` string **and the cause that was already computed for
/// the operator-facing name**. The cause used to be re-read from the use log by each consumer,
/// which is a race: a failure recorded before that log's terminal event is flushed falls back
/// to the state word and writes `reason_code=error`, losing a reason the process already knew.
/// Observed on the owner's journal 2026-09-03 — the same failure on the same segment recorded
/// `error` at 23:19 and `context_window_exceeded` at 23:44 with identical use logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DrainOutcome {
    Finish,
    Fail {
        state: &'static str,
        cause: Option<String>,
    },
}

impl DrainOutcome {
    pub(crate) fn fail(state: &'static str, cause: impl Into<String>) -> Self {
        Self::Fail {
            state,
            cause: Some(cause.into()),
        }
    }
}

/// Shared equivalent of `_drain_priority_batch` (`thinking.py:991-1042`).
/// The client policy is `think()` and its explicit outcome deadline is 610 seconds.
pub(crate) fn drain(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
) -> ModeResult {
    drain_with_deadline(context, runtime, pending, Some(DEFAULT_THINK_TIMEOUT))
}

pub(crate) fn drain_with_deadline(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    deadline: Option<Duration>,
) -> ModeResult {
    drain_with_deadline_observed(context, runtime, pending, deadline, &mut |_, _| {})
}

pub(crate) fn drain_with_deadline_observed(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    deadline: Option<Duration>,
    observer: &mut dyn FnMut(&PendingUse, DrainOutcome),
) -> ModeResult {
    drain_with_failure_policy(context, runtime, pending, deadline, observer, &|_| true)
}

/// Keep every terminal observation while letting the caller identify enrichment
/// failures that do not block its completion contract.
pub(crate) fn drain_with_failure_policy(
    context: &ThinkContext,
    runtime: &tokio::runtime::Runtime,
    pending: Vec<PendingUse>,
    deadline: Option<Duration>,
    observer: &mut dyn FnMut(&PendingUse, DrainOutcome),
    failure_blocks: &dyn Fn(&PendingUse) -> bool,
) -> ModeResult {
    let mut result = ModeResult::default();
    if pending.is_empty() {
        return result;
    }
    let use_ids = pending
        .iter()
        .map(|item| item.use_id.clone())
        .collect::<Vec<_>>();
    match context.cortex.wait(runtime, &use_ids, deadline) {
        Ok(report) => {
            for item in pending {
                let label = item_label(&item.name, item.facet.as_deref());
                if let Some(timeout) = report
                    .timed_out
                    .iter()
                    .find(|timeout| timeout.use_id() == item.use_id)
                {
                    let cause =
                        failure_cause(&context.journal, &item.use_id, timeout_cause(timeout));
                    if failure_blocks(&item) {
                        result.failed += 1;
                        result.timed_out = true;
                        result.failed_names.push(named_failure(&label, &cause));
                    }
                    observer(&item, DrainOutcome::fail(timeout_cause(timeout), cause));
                    continue;
                }
                match report.completed.get(&item.use_id) {
                    Some(completion) if completion.end_state == UseEndState::Finish => {
                        // Source-derived, not measured: thinking.py:240-242 and
                        // 1102 require both a literal changed flag and a path.
                        maybe_rescan_output(context, &item, completion);
                        result.success += 1;
                        result.success_names.push(label);
                        observer(&item, DrainOutcome::Finish);
                    }
                    Some(completion) => {
                        let cause = failure_cause(
                            &context.journal,
                            &item.use_id,
                            completion.end_state.as_str(),
                        );
                        if failure_blocks(&item) {
                            result.failed += 1;
                            result.failed_names.push(named_failure(&label, &cause));
                        }
                        observer(
                            &item,
                            DrainOutcome::fail(completion.end_state.as_str(), cause),
                        );
                    }
                    None => {
                        let cause = failure_cause(&context.journal, &item.use_id, "unknown");
                        if failure_blocks(&item) {
                            result.failed += 1;
                            result.failed_names.push(named_failure(&label, &cause));
                        }
                        observer(&item, DrainOutcome::fail("missing_completion", cause));
                    }
                }
            }
        }
        // The wait error was previously dropped on the floor and every pending use was
        // reported as `(unavailable)` — the literal fallback for "no reason known". That
        // renders "we don't know" as if it were a cause: an operator reading
        // `documents (unavailable)` cannot tell a blocked runtime from a dead socket from a
        // read failure, and nothing else records it. Keep the error and let `failure_cause`
        // consult the same sources the completion arms already use, falling back to the
        // wait error itself rather than a word that says nothing.
        Err(error) => {
            let wait_error = format!("wait failed: {error:?}");
            for item in pending {
                let label = item_label(&item.name, item.facet.as_deref());
                // Ask each use's own log what happened rather than blaming the wait
                // on all of them. A use that already finished durably is a success
                // whatever the wait did afterwards.
                if use_log_terminal(&context.journal, &item.use_id).as_deref() == Some("finish") {
                    // Deliberately no `maybe_rescan_output` here: no completion is
                    // available to read the literal changed flag from, and this arm
                    // never rescanned. Recording the truth is strictly better than the
                    // false failure it replaces, and regresses nothing.
                    result.success += 1;
                    result.success_names.push(label);
                    observer(&item, DrainOutcome::Finish);
                    continue;
                }
                let cause = failure_cause(&context.journal, &item.use_id, &wait_error);
                if failure_blocks(&item) {
                    result.failed += 1;
                    result.failed_names.push(named_failure(&label, &cause));
                }
                observer(&item, DrainOutcome::fail("wait_failed", cause));
            }
        }
    }
    result
}

/// Preserve the reference's literal changed flag and existing-file gate.
pub(crate) fn maybe_rescan_output(
    context: &ThinkContext,
    item: &PendingUse,
    completion: &solstone_core_cortex_client::UseCompletion,
) {
    // Source-derived, not measured: thinking.py:240-242 requires both a
    // literal `True` finish field and an existing output path.
    if completion.finish_fields.output_changed == Some(true)
        && item.index_output
        && item.output_path.as_ref().is_some_and(|path| path.exists())
    {
        context.index.rescan_file(
            &context.journal,
            item.output_path.as_ref().expect("checked output path"),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{failure_cause, named_failure, use_log_terminal};

    fn write_blocked_local_runtime(journal: &std::path::Path) {
        let path = journal.join("health/providers/runtime/local.json");
        fs::create_dir_all(path.parent().expect("runtime directory")).expect("runtime directory");
        fs::write(
            path,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "provider": "local",
                "revision": 1,
                "phase": "artifact-not-ready",
                "reason_code": "manifest-missing",
                "detail": {},
                "desired_fingerprint_sha256": null,
                "incarnation": null,
                "generation": 0,
                "attempt": 0,
                "process": null,
                "updated_at": null,
                "display_deadline_at": null,
                "owner": null
            }))
            .expect("runtime record"),
        )
        .expect("write runtime record");
    }

    fn write_use_start(journal: &std::path::Path, use_id: &str, provider: &str) {
        let use_log = journal
            .join("talents/timeline--segment_summary")
            .join(format!("{use_id}.jsonl"));
        fs::create_dir_all(use_log.parent().expect("use log directory"))
            .expect("use log directory");
        fs::write(
            use_log,
            format!(
                "{{\"event\":\"start\",\"use_id\":\"{use_id}\",\"provider\":\"{provider}\"}}\n"
            ),
        )
        .expect("write use log");
    }

    // AC: the use log is the authority on what a talent actually did, so a wait failure
    // cannot be attributed to a use whose own log already ended in `finish`. Measured on
    // the owner's journal 2026-09-03: 17 of 327 `talent.fail` records were provably false
    // this way, every output present on disk.
    #[test]
    fn use_log_terminal_reports_the_uses_own_outcome() {
        let journal = tempfile::tempdir().expect("journal");
        let dir = journal.path().join("talents/sense");
        fs::create_dir_all(&dir).expect("use log directory");

        // nothing written yet -- the wait failure is all we know
        assert_eq!(use_log_terminal(journal.path(), "use-none"), None);

        // a use that finished, including the legitimate skip shape that dominated the
        // false failures on the owner's journal
        fs::write(
            dir.join("use-finish.jsonl"),
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-finish\"}\n",
                "{\"event\":\"finish\",\"use_id\":\"use-finish\",\"skip_reason\":\"no_input\"}\n",
            ),
        )
        .expect("write use log");
        assert_eq!(
            use_log_terminal(journal.path(), "use-finish").as_deref(),
            Some("finish")
        );

        // a use that genuinely failed stays a failure
        fs::write(
            dir.join("use-error.jsonl"),
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-error\"}\n",
                "{\"event\":\"error\",\"use_id\":\"use-error\",\"error\":\"boom\"}\n",
            ),
        )
        .expect("write use log");
        assert_eq!(
            use_log_terminal(journal.path(), "use-error").as_deref(),
            Some("error")
        );

        // a start with no terminal event yields nothing to attribute
        fs::write(
            dir.join("use-open.jsonl"),
            "{\"event\":\"start\",\"use_id\":\"use-open\"}\n",
        )
        .expect("write use log");
        assert_eq!(use_log_terminal(journal.path(), "use-open"), None);
    }

    // AC: when nothing else knows why a use failed, the caller's fallback is reported --
    // not a generic word. The wait-failure arm of `drain_with_deadline_observed` relies on
    // this to surface the real cortex error instead of the literal "unavailable", which
    // rendered "we don't know" as though it were a cause.
    #[test]
    fn failure_cause_reports_the_callers_fallback_when_nothing_else_knows() {
        let journal = tempfile::tempdir().expect("journal");
        assert_eq!(
            failure_cause(
                journal.path(),
                "1788358659849",
                "wait failed: Dispatch(Closed)"
            ),
            "wait failed: Dispatch(Closed)"
        );
        assert_eq!(
            named_failure("documents", "wait failed: Dispatch(Closed)"),
            "documents (wait failed: Dispatch(Closed))"
        );
    }

    #[test]
    fn failed_use_reason_precedes_unrelated_blocked_local_runtime() {
        let journal = tempfile::tempdir().expect("journal");
        write_blocked_local_runtime(journal.path());
        let use_log = journal
            .path()
            .join("talents/timeline--segment_summary/use-1.jsonl");
        fs::create_dir_all(use_log.parent().expect("use log directory"))
            .expect("use log directory");
        fs::write(
            use_log,
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-1\",\"provider\":\"google\"}\n",
                "{\"event\":\"error\",\"use_id\":\"use-1\",",
                "\"reason_code\":\"incomplete_json_length\",",
                "\"error\":\"generate hook failed\"}\n"
            ),
        )
        .expect("write use log");

        assert_eq!(
            failure_cause(journal.path(), "use-1", "error"),
            "incomplete_json_length"
        );
    }

    #[test]
    fn blocked_local_runtime_does_not_relabel_byo_timeout_without_terminal_error() {
        let journal = tempfile::tempdir().expect("journal");
        write_blocked_local_runtime(journal.path());
        write_use_start(journal.path(), "use-google", "google");

        assert_eq!(
            failure_cause(journal.path(), "use-google", "timeout"),
            "timeout"
        );
    }

    #[test]
    fn blocked_local_runtime_does_not_replace_provider_neutral_local_timeout() {
        let journal = tempfile::tempdir().expect("journal");
        write_blocked_local_runtime(journal.path());
        write_use_start(journal.path(), "use-local", "local");

        assert_eq!(
            failure_cause(journal.path(), "use-local", "timeout"),
            "timeout"
        );
    }

    #[test]
    fn nonterminal_use_error_does_not_replace_timeout_cause() {
        let journal = tempfile::tempdir().expect("journal");
        let use_log = journal
            .path()
            .join("talents/timeline--segment_summary/use-transient.jsonl");
        fs::create_dir_all(use_log.parent().expect("use log directory"))
            .expect("use log directory");
        fs::write(
            use_log,
            concat!(
                "{\"event\":\"start\",\"use_id\":\"use-transient\"}\n",
                "{\"event\":\"error\",\"use_id\":\"use-transient\",",
                "\"terminal\":false,\"reason_code\":\"retryable_transport\"}\n"
            ),
        )
        .expect("write use log");

        assert_eq!(
            failure_cause(journal.path(), "use-transient", "timeout"),
            "timeout"
        );
    }
}
