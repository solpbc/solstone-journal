// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `journal reprocess` command body.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde_json::{Map, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_segment::{PathOrDay, day_path, iter_segments, touch_stream_health_marker};
use solstone_core_system_health::{FilesystemSegmentSource, day_is_complete, scan_day};

// ⛔ `journal up`, not `journal start`. `journal start` runs the supervisor
// in the FOREGROUND (SKILL.md: "starts the supervisor runtime only"), so an
// owner who follows it gets a process tied to that terminal and loses intake
// when they close it. `journal up` is the alias for `journal service start`.
const UNREACHABLE_MESSAGE: &str =
    "supervisor not reachable - start it (solstone journal up), then retry";
const THROUGH_REQUIRES_FROM_SCRATCH: &str = "--through requires --from-scratch";
const THROUGH_BEFORE_START: &str = "--through must be on or after the start day";
const HELP_FIXTURE: &str =
    include_str!("../../../fixtures/journal-storage-ops-reference-grammar.txt");

/// The observable result of a library-hosted CLI invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    ProcessNow,
    FromScratch,
    MarkUpdated,
    /// Report what is owed and why; submit nothing.
    Owed,
}

#[derive(Debug)]
struct ParsedArgs {
    day: String,
    through: Option<String>,
    yes: bool,
    flavor: Flavor,
    unit: Option<String>,
    facet: Option<String>,
}

/// Range facts deliberately preserve their distinct sources: iter-segments is
/// the data gate, while scan-day supplies only the displayed segment count.
#[derive(Debug)]
struct RangeDay {
    day: String,
    has_iter_segments_data: bool,
    scan_day_segment_count: usize,
}

#[derive(Debug)]
pub enum DayOutcome {
    Malformed,
    PastOnly,
    NoData,
    Submitted(Flavor),
    AlreadyComplete,
    CurrentDegraded,
    NoThinkingEngine,
    Unreachable,
    Failed(String),
}

/// Run with the real socket transport and the journal's zone.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    let zone = solstone_core_journal_config::owner_zone(journal_path);
    run_cli_with(args, journal_path, Utc::now(), zone, |envelope| {
        send_envelope(journal_path, envelope)
    })
}

/// Run with explicit time, zone, and transport seams.
pub fn run_cli_with<F>(
    args: &[String],
    journal_path: &Path,
    now: DateTime<Utc>,
    zone: Tz,
    mut transport: F,
) -> CliRun
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let parsed = match parse_arguments(args) {
        Ok(parsed) => parsed,
        Err(ParseResult::Help) => return success(reprocess_help()),
        Err(ParseResult::Usage(message)) => return usage_error(&message),
    };

    if let Some(unit_name) = parsed.unit.as_deref() {
        let Some(day_date) = parse_day(journal_path, &parsed.day) else {
            return failure("expected day in YYYYMMDD format");
        };
        let today = now.with_timezone(&zone).date_naive();
        if day_date >= today {
            return failure("reprocess is past-only (cannot reprocess today or a future day)");
        }
        let today_str = today.format("%Y%m%d").to_string();
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(
            &parsed.day,
            unit_name,
            parsed.facet.clone(),
        );
        match solstone_core_journal_io::reset_daily_unit_for_reprocess(
            journal_path,
            &identity,
            &today_str,
            now.timestamp_millis(),
        ) {
            Ok(()) => {
                let line = match parsed.facet.as_deref() {
                    Some(facet) if !facet.is_empty() => {
                        format!(
                            "{unit_name} ({facet}) on {} was reset for the next eligible run\n",
                            parsed.day
                        )
                    }
                    _ => format!(
                        "{unit_name} on {} was reset for the next eligible run\n",
                        parsed.day
                    ),
                };
                return success(line);
            }
            Err(error) => {
                return failure(&format!("reprocess unit failed: {error}"));
            }
        }
    }

    if parsed.flavor == Flavor::Owed {
        return owed_report(
            journal_path,
            &parsed.day,
            parsed.through.as_deref(),
            now,
            zone,
        );
    }
    if let Some(through_raw) = parsed.through.as_deref() {
        if parsed.flavor != Flavor::FromScratch {
            return failure(THROUGH_REQUIRES_FROM_SCRATCH);
        }
        let Some(start) = parse_day(journal_path, &parsed.day) else {
            return failure("expected day in YYYYMMDD format");
        };
        let Some(through) = parse_day(journal_path, through_raw) else {
            return failure("expected day in YYYYMMDD format");
        };
        let today = now.with_timezone(&zone).date_naive();
        if start >= today || through >= today {
            return failure("reprocess is past-only (cannot reprocess today or a future day)");
        }
        if through < start {
            return failure(THROUGH_BEFORE_START);
        }
        let days = enumerate_range_days(journal_path, start, through, now);
        if data_days(&days).is_empty() {
            return failure(&format!(
                "no data for days {} through {through_raw}",
                parsed.day
            ));
        }
        if !parsed.yes {
            if solstone_core_system::no_thinking_engine_chosen(journal_path) {
                return render_day_outcome(&parsed.day, DayOutcome::NoThinkingEngine);
            }
            return success(range_plan(&days));
        }
        return run_from_scratch_range(journal_path, &days, now, zone, &mut transport);
    }

    render_day_outcome(
        &parsed.day,
        reprocess_day_with(
            journal_path,
            &parsed.day,
            parsed.flavor,
            now,
            zone,
            transport,
        ),
    )
}

fn parse_arguments(args: &[String]) -> Result<ParsedArgs, ParseResult> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Err(ParseResult::Help);
    }
    let mut day = None;
    let mut through = None;
    let mut through_flag = false;
    let mut yes = false;
    let mut flavor = Flavor::ProcessNow;
    let mut flavor_flag: Option<&str> = None;
    let mut unit = None;
    let mut facet = None;
    let mut unknown = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-v" | "--verbose" | "-d" | "--debug" => {}
            "--yes" => {
                if unit.is_some() {
                    return Err(ParseResult::Usage(
                        "argument --yes: not allowed with argument --unit".to_owned(),
                    ));
                }
                yes = true;
            }
            "--through" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(
                        "argument --through: expected one argument".to_owned(),
                    ));
                };
                if unit.is_some() {
                    return Err(ParseResult::Usage(
                        "argument --through: not allowed with argument --unit".to_owned(),
                    ));
                }
                through_flag = true;
                through = Some(value.clone());
            }
            "--unit" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(
                        "argument --unit: expected one argument".to_owned(),
                    ));
                };
                if through_flag {
                    return Err(ParseResult::Usage(
                        "argument --unit: not allowed with argument --through".to_owned(),
                    ));
                }
                if yes {
                    return Err(ParseResult::Usage(
                        "argument --unit: not allowed with argument --yes".to_owned(),
                    ));
                }
                if let Some(previous) = flavor_flag {
                    return Err(ParseResult::Usage(format!(
                        "argument --unit: not allowed with argument {previous}"
                    )));
                }
                unit = Some(value.clone());
            }
            "--facet" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(
                        "argument --facet: expected one argument".to_owned(),
                    ));
                };
                facet = Some(value.clone());
            }
            "--from-scratch" | "--mark-updated" | "--owed" => {
                if unit.is_some() {
                    return Err(ParseResult::Usage(format!(
                        "argument {argument}: not allowed with argument --unit"
                    )));
                }
                if let Some(previous) = flavor_flag {
                    return Err(ParseResult::Usage(format!(
                        "argument {argument}: not allowed with argument {previous}"
                    )));
                }
                flavor_flag = Some(argument);
                flavor = match argument.as_str() {
                    "--from-scratch" => Flavor::FromScratch,
                    "--mark-updated" => Flavor::MarkUpdated,
                    _ => Flavor::Owed,
                };
            }
            _ if argument.starts_with('-') => unknown.push(argument.clone()),
            _ if day.is_none() => day = Some(argument.clone()),
            _ => unknown.push(argument.clone()),
        }
        index += 1;
    }
    if facet.is_some() && unit.is_none() {
        return Err(ParseResult::Usage(
            "argument --facet: requires --unit".to_owned(),
        ));
    }
    let Some(day) = day else {
        return Err(ParseResult::Usage(
            "the following arguments are required: day".to_owned(),
        ));
    };
    if unknown.is_empty() {
        Ok(ParsedArgs {
            day,
            through,
            yes,
            flavor,
            unit,
            facet,
        })
    } else {
        Err(ParseResult::Usage(format!(
            "unrecognized arguments: {}",
            unknown.join(" ")
        )))
    }
}

enum ParseResult {
    Help,
    Usage(String),
}

/// Reprocess one day using the current time and the journal's zone.
pub fn reprocess_day(journal_path: &Path, day: &str, flavor: Flavor) -> DayOutcome {
    let zone = solstone_core_journal_config::owner_zone(journal_path);
    reprocess_day_with(journal_path, day, flavor, Utc::now(), zone, |envelope| {
        send_envelope(journal_path, envelope)
    })
}

/// Reprocess one day with explicit time, zone, and transport seams.
pub fn reprocess_day_with<F>(
    journal: &Path,
    day: &str,
    flavor: Flavor,
    now: DateTime<Utc>,
    zone: Tz,
    mut transport: F,
) -> DayOutcome
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    if flavor == Flavor::Owed {
        return DayOutcome::Failed("--owed only reports; use run_cli".to_owned());
    }
    let Some(parsed) = parse_day(journal, day) else {
        return DayOutcome::Malformed;
    };
    if parsed >= now.with_timezone(&zone).date_naive() {
        return DayOutcome::PastOnly;
    }
    let Ok(day_directory) = day_path(journal, day, false) else {
        return DayOutcome::Malformed;
    };
    let has_data = day_directory.is_dir()
        && iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| !segments.is_empty());
    if !has_data {
        return DayOutcome::NoData;
    }
    if solstone_core_system::no_thinking_engine_chosen(journal) {
        return DayOutcome::NoThinkingEngine;
    }
    if flavor == Flavor::FromScratch {
        return if transport(&request_envelope(day)) {
            DayOutcome::Submitted(flavor)
        } else {
            DayOutcome::Unreachable
        };
    }
    if flavor == Flavor::MarkUpdated {
        // Persist the requeue intent before a socket failure can lose it.
        if let Err(error) = touch_stream_health_marker(journal, day) {
            return DayOutcome::Failed(error.to_string());
        }
        return if transport(&drain_envelope(day)) {
            DayOutcome::Submitted(flavor)
        } else {
            DayOutcome::Unreachable
        };
    }
    match day_is_complete(journal, day) {
        Ok(true) => match solstone_core_system::daily_coverage::read_daily_coverage(journal, day) {
            Ok(coverage) => match coverage.state {
                solstone_core_system::daily_coverage::CoverageState::Current => {
                    return DayOutcome::AlreadyComplete;
                }
                solstone_core_system::daily_coverage::CoverageState::CurrentDegraded => {
                    return DayOutcome::CurrentDegraded;
                }
                _ => {}
            },
            Err(error) => return DayOutcome::Failed(error),
        },
        Ok(false) => {}
        Err(error) => return DayOutcome::Failed(error.to_string()),
    }
    if transport(&drain_envelope(day)) {
        DayOutcome::Submitted(flavor)
    } else {
        DayOutcome::Unreachable
    }
}

fn render_day_outcome(day: &str, outcome: DayOutcome) -> CliRun {
    match outcome {
        DayOutcome::Malformed => failure("expected day in YYYYMMDD format"),
        DayOutcome::PastOnly => {
            failure("reprocess is past-only (cannot reprocess today or a future day)")
        }
        DayOutcome::NoData => failure(&format!("no data for day {day}")),
        DayOutcome::Submitted(Flavor::FromScratch) => {
            success(format!("reprocess (from-scratch) submitted for {day}\n"))
        }
        DayOutcome::Submitted(Flavor::MarkUpdated) => {
            success(format!("reprocess (mark-updated) submitted for {day}\n"))
        }
        DayOutcome::Submitted(Flavor::ProcessNow) => {
            success(format!("reprocess (process-now) submitted for {day}\n"))
        }
        DayOutcome::Submitted(Flavor::Owed) => {
            failure("--owed only reports; nothing was submitted")
        }
        DayOutcome::AlreadyComplete => success(format!(
            "day {day} already complete; use --from-scratch to force a full re-run\n"
        )),
        DayOutcome::CurrentDegraded => success(format!(
            "day {day}: some daily processing is still unresolved. use --from-scratch to retry.\n"
        )),
        DayOutcome::NoThinkingEngine => {
            failure("no model is chosen yet. choose one in thinking, then retry")
        }
        DayOutcome::Unreachable => failure(UNREACHABLE_MESSAGE),
        DayOutcome::Failed(error) => failure(&format!("reprocess failed: {error}")),
    }
}

fn enumerate_range_days(
    journal: &Path,
    start: NaiveDate,
    through: NaiveDate,
    now: DateTime<Utc>,
) -> Vec<RangeDay> {
    let mut days = Vec::new();
    let mut current = start;
    while current <= through {
        let day = current.format("%Y%m%d").to_string();
        let segments = iter_segments(journal, PathOrDay::Day(&day)).unwrap_or_default();
        let has_iter_segments_data =
            day_path(journal, &day, false).is_ok_and(|path| path.is_dir()) && !segments.is_empty();
        let scan_day_segment_count = if has_iter_segments_data {
            scan_day(&FilesystemSegmentSource, journal, &day, now)
                .map(|(_, _, scanned)| scanned.len())
                .unwrap_or(0)
        } else {
            0
        };
        days.push(RangeDay {
            day,
            has_iter_segments_data,
            scan_day_segment_count,
        });
        current = current
            .succ_opt()
            .expect("range day advances within chrono bounds");
    }
    days
}

fn data_days(days: &[RangeDay]) -> Vec<&RangeDay> {
    days.iter()
        .filter(|entry| entry.has_iter_segments_data)
        .collect()
}

fn range_segment_count(days: &[RangeDay]) -> usize {
    data_days(days)
        .iter()
        .map(|entry| entry.scan_day_segment_count)
        .sum()
}

fn range_plan(days: &[RangeDay]) -> String {
    let count = data_days(days).len();
    format!(
        "from-scratch reprocess plan:\n{count} days with data ({} segments) will be queued. Progress will be visible in solstone journal top or solstone journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n",
        range_segment_count(days)
    )
}

fn run_from_scratch_range<F>(
    journal: &Path,
    days: &[RangeDay],
    now: DateTime<Utc>,
    zone: Tz,
    transport: &mut F,
) -> CliRun
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let data_days = data_days(days);
    let mut queued = Vec::new();
    for entry in days {
        match reprocess_day_with(
            journal,
            &entry.day,
            Flavor::FromScratch,
            now,
            zone,
            &mut *transport,
        ) {
            DayOutcome::NoData => {}
            DayOutcome::Submitted(_) => queued.push(entry.day.clone()),
            DayOutcome::Unreachable => {
                let not_queued = data_days[queued.len()..]
                    .iter()
                    .map(|day| day.day.as_str())
                    .collect::<Vec<_>>();
                return CliRun {
                    stdout: String::new(),
                    stderr: format!(
                        "failed to queue day {} of {} ({}): {UNREACHABLE_MESSAGE}\nqueued day set: {}\nnot-queued day set: {}\n",
                        queued.len() + 1,
                        data_days.len(),
                        entry.day,
                        format_day_set(queued.iter().map(String::as_str)),
                        format_day_set(not_queued),
                    ),
                    exit_code: 1,
                };
            }
            other => return render_day_outcome(&entry.day, other),
        }
    }
    success(format!(
        "queued from-scratch reprocess for {} days ({} segments)\nprogress is visible in solstone journal top or solstone journal health\nqueued days do not survive a supervisor restart\n",
        data_days.len(),
        range_segment_count(days)
    ))
}

fn format_day_set<'a>(days: impl IntoIterator<Item = &'a str>) -> String {
    let values = days.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        "none".to_owned()
    } else {
        values.join(", ")
    }
}

/// List every daily output owed on past days in the range, and why.
///
/// Read-only: nothing is submitted.  A release burn-in runs the candidate's
/// build of this against the journal before installing it, so the number of
/// past outputs an upgrade would regenerate is stated rather than discovered.
fn owed_report(
    journal: &Path,
    start_raw: &str,
    through_raw: Option<&str>,
    now: DateTime<Utc>,
    zone: Tz,
) -> CliRun {
    let Some(start) = parse_day(journal, start_raw) else {
        return failure("expected day in YYYYMMDD format");
    };
    let through = match through_raw {
        Some(raw) => match parse_day(journal, raw) {
            Some(day) => day,
            None => return failure("expected day in YYYYMMDD format"),
        },
        None => start,
    };
    if through < start {
        return failure(THROUGH_BEFORE_START);
    }
    let today = now.with_timezone(&zone).date_naive();
    let through = through.min(today - chrono::Duration::days(1));
    let mut lines = Vec::new();
    let mut causes = std::collections::BTreeMap::<String, usize>::new();
    let (mut owed, mut days, mut unreadable) = (0usize, 0usize, 0usize);
    let mut current = start;
    while current <= through {
        let day = current.format("%Y%m%d").to_string();
        current += chrono::Duration::days(1);
        if !day_path(journal, &day, false).is_ok_and(|path| path.is_dir()) {
            continue;
        }
        match solstone_core_system::daily_coverage::read_daily_coverage(journal, &day) {
            Ok(coverage) => {
                let mut counted = false;
                for unit in coverage.units.iter().filter(|unit| unit.state.is_owed()) {
                    let cause = unit.owed_by.clone().unwrap_or_else(|| "unknown".to_owned());
                    let name = unit.identity.facet.as_deref().map_or_else(
                        || unit.identity.name.clone(),
                        |facet| format!("{}/{facet}", unit.identity.name),
                    );
                    lines.push(format!("{day}  {name}  {cause}"));
                    *causes.entry(cause).or_default() += 1;
                    owed += 1;
                    counted = true;
                }
                days += usize::from(counted);
            }
            Err(error) => {
                lines.push(format!("{day}  (unreadable: {error})"));
                unreadable += 1;
            }
        }
    }
    let summary = causes
        .iter()
        .map(|(cause, count)| format!("{count} {cause}"))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "{owed} owed output(s) on {days} past day(s){}; {unreadable} unreadable day(s)",
        if summary.is_empty() {
            String::new()
        } else {
            format!(": {summary}")
        }
    ));
    success(lines.join("\n") + "\n")
}

fn parse_day(journal: &Path, day: &str) -> Option<NaiveDate> {
    day_path(journal, day, false).ok()?;
    NaiveDate::parse_from_str(day, "%Y%m%d").ok()
}

fn request_envelope(day: &str) -> CallosumEnvelope {
    let mut extra = Map::new();
    extra.insert(
        "cmd".to_owned(),
        serde_json::Value::from(solstone_core_system::partition::canonical_journal_command(
            ["think", "-v", "--day", day, "--from-scratch"],
        )),
    );
    extra.insert("day".to_owned(), json!(day));
    extra.insert("queue_if_active_cmd_differs".to_owned(), json!(true));
    CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "request".to_owned(),
        ts: None,
        extra,
    }
}

fn drain_envelope(day: &str) -> CallosumEnvelope {
    let mut extra = Map::new();
    extra.insert("day".to_owned(), json!(day));
    CallosumEnvelope {
        tract: "supervisor".to_owned(),
        event: "drain".to_owned(),
        ts: None,
        extra,
    }
}

fn frame_envelope(envelope: &CallosumEnvelope) -> Option<String> {
    let mut line = serde_json::to_string(envelope).ok()?;
    line.push('\n');
    Some(line)
}

fn send_envelope(journal: &Path, envelope: &CallosumEnvelope) -> bool {
    let Some(line) = frame_envelope(envelope) else {
        return false;
    };
    CallosumOneShotSender::new(
        journal.join("health").join("callosum.sock"),
        Duration::from_secs(1),
    )
    .send_line(&line)
    .is_ok()
}

fn reprocess_help() -> String {
    let header = "=== reprocess --help\n";
    let start = HELP_FIXTURE
        .find(header)
        .expect("reprocess help fixture block exists")
        + header.len();
    let rest = &HELP_FIXTURE[start..];
    let end = rest.find("\n=== ").unwrap_or(rest.len());
    rest[..end].to_owned()
}

fn reprocess_usage() -> String {
    let header = "=== reprocess (missing day)\n";
    let start = HELP_FIXTURE
        .find(header)
        .expect("reprocess missing-day fixture block exists")
        + header.len();
    let rest = &HELP_FIXTURE[start..];
    let end = rest.find("\n=== ").unwrap_or(rest.len());
    let block = &rest[..end];
    let usage_lines: Vec<&str> = block
        .lines()
        .take_while(|line| !line.starts_with("solstone journal reprocess: error:"))
        .collect();
    format!("{}\n", usage_lines.join("\n"))
}

fn success(stdout: impl Into<String>) -> CliRun {
    CliRun {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(message: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{message}\n"),
        exit_code: 1,
    }
}

fn usage_error(message: &str) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "{}solstone journal reprocess: error: {message}\n",
            reprocess_usage()
        ),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    use super::*;

    const DAY: &str = "20260101";
    const HELP: &str = "usage: solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]\n                         [--facet FACET] [--from-scratch | --mark-updated |\n                         --owed] [-v] [-d]\n                         day\n\nSubmit a past journal day for reprocessing\n\npositional arguments:\n  day                Past day in YYYYMMDD format\n\noptions:\n  -h, --help         show this help message and exit\n  --through THROUGH  Inclusive range end in YYYYMMDD format\n  --yes\n  --unit UNIT        Reset one failed daily unit for the next eligible run\n  --facet FACET      Facet of that unit; required when the unit has a facet\n  --from-scratch     Force a full daily re-run, preserving markers (does not\n                     flag the day as updated)\n  --mark-updated     Flag the day as having new raw data so daily processing\n                     re-queues it, then nudge a drain\n  --owed             List the daily outputs owed on the day or range and why,\n                     without submitting anything\n  -v, --verbose      Enable verbose output\n  -d, --debug        Enable debug logging\n";
    const MISSING_DAY_STDERR: &str = "usage: solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]\n                         [--facet FACET] [--from-scratch | --mark-updated |\n                         --owed] [-v] [-d]\n                         day\nsolstone journal reprocess: error: the following arguments are required: day\n";

    fn words(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap()
    }

    fn segment(root: &Path, day: &str, name: &str) -> std::path::PathBuf {
        let path = root.join("chronicle").join(day).join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).unwrap();
        for entry in fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            let kind = entry.file_type().unwrap();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn write_test_provider(root: &Path) {
        let config_dir = root.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        let config_path = config_dir.join("journal.json");
        let mut map: serde_json::Map<String, serde_json::Value> = if config_path.is_file() {
            let bytes = fs::read(&config_path).unwrap();
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            serde_json::Map::new()
        };
        map.insert(
            "providers".into(),
            serde_json::json!({
                "active": {
                    "provider": "test"
                }
            }),
        );
        fs::write(config_path, serde_json::to_vec(&map).unwrap()).unwrap();
    }

    #[test]
    fn writers_reach_every_repair_and_drain_projection() {
        use solstone_core_system::catchup::{
            CatchupKind, SegmentRepairOutcome, record_daily_catchup_progress,
            record_segment_repair_attempt, record_segment_repair_outcome,
        };

        let root = TempDir::new().unwrap();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .unwrap();
        copy_tree(&repository.join("tests/fixtures/journal"), root.path());
        let day = "20250101";
        record_daily_catchup_progress(root.path(), day, 1, 2);
        record_segment_repair_attempt(root.path(), day, 1.0);
        record_segment_repair_outcome(
            root.path(),
            day,
            SegmentRepairOutcome {
                success: false,
                timed_out: true,
                timeout_seconds: Some(3.0),
                ended_at: 4.0,
                cleared: Some(1),
                remaining: Some(2),
            },
        );
        assert!(solstone_core_system_health::read_segment_repair_attempted(
            root.path(),
            day
        ));
        assert_eq!(
            solstone_core_system_health::read_segment_repair_summary(root.path(), day)
                .unwrap()
                .status,
            "progressing"
        );
        assert!(
            !solstone_core_system::catchup::day_eligible_to_drain(
                root.path(),
                day,
                CatchupKind::SegmentRepair,
                std::time::UNIX_EPOCH
            )
            .unwrap()
        );
    }

    #[test]
    fn help_is_fixture_exact() {
        assert_eq!(reprocess_help(), HELP);
        assert_eq!(
            run_cli_with(
                &words(&["--help"]),
                Path::new("."),
                now(),
                chrono_tz::UTC,
                |_| false
            )
            .stdout,
            HELP
        );
    }

    #[test]
    fn parser_errors_follow_argparse_ordering() {
        let root = TempDir::new().unwrap();
        let no_day = run_cli_with(
            &words(&["--nonsense"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(no_day.exit_code, 2);
        assert_eq!(no_day.stderr, MISSING_DAY_STDERR);
        let unknown = run_cli_with(
            &words(&[DAY, "--nonsense"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(
            unknown.stderr,
            format!(
                "{}solstone journal reprocess: error: unrecognized arguments: --nonsense\n",
                reprocess_usage()
            )
        );
        for (first, second) in [
            ("--from-scratch", "--mark-updated"),
            ("--mark-updated", "--from-scratch"),
        ] {
            let result = run_cli_with(
                &words(&[first, second]),
                root.path(),
                now(),
                chrono_tz::UTC,
                |_| false,
            );
            assert_eq!(
                result.stderr,
                format!(
                    "{}solstone journal reprocess: error: argument {second}: not allowed with argument {first}\n",
                    reprocess_usage()
                )
            );
        }
    }

    #[test]
    fn verbose_and_debug_flags_are_accepted_with_each_flavor() {
        for flag in ["-v", "--verbose", "-d", "--debug"] {
            for flavor in [None, Some("--from-scratch"), Some("--mark-updated")] {
                let root = TempDir::new().unwrap();
                write_test_provider(root.path());
                fs::write(
                    segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
                    "{}\n",
                )
                .unwrap();
                let health = root.path().join("chronicle").join(DAY).join("health");
                fs::create_dir_all(&health).unwrap();
                fs::write(health.join("stream.updated"), "").unwrap();
                let mut flagged = vec![DAY.to_owned(), flag.to_owned()];
                let mut baseline = vec![DAY.to_owned()];
                if let Some(flavor) = flavor {
                    flagged.push(flavor.to_owned());
                    baseline.push(flavor.to_owned());
                }
                let result = run_cli_with(&flagged, root.path(), now(), chrono_tz::UTC, |_| true);
                let expected =
                    run_cli_with(&baseline, root.path(), now(), chrono_tz::UTC, |_| true);
                assert_eq!(result.exit_code, 0, "{flag} {flavor:?}");
                assert_eq!(result.stdout, expected.stdout, "{flag} {flavor:?}");
                assert_eq!(result.stderr, expected.stderr, "{flag} {flavor:?}");
            }
        }
    }

    #[test]
    fn past_only_is_the_journals_today_not_this_computers() {
        let root = TempDir::new().unwrap();
        let now = Utc::now();
        let host_today = now
            .with_timezone(&solstone_core_journal_config::host_zone())
            .date_naive();
        let zone = [chrono_tz::Pacific::Kiritimati, chrono_tz::Etc::GMTPlus12]
            .into_iter()
            .find(|zone| now.with_timezone(zone).date_naive() != host_today)
            .expect("UTC+14 and UTC-12 never share a date");
        let owner_today = now.with_timezone(&zone).date_naive();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            json!({"identity": {"timezone": zone.name()}}).to_string(),
        )
        .unwrap();
        let day = |date: chrono::NaiveDate| date.format("%Y%m%d").to_string();
        let owner_day = reprocess_day(root.path(), &day(owner_today), Flavor::FromScratch);
        let host_day = reprocess_day(root.path(), &day(host_today), Flavor::FromScratch);
        if Utc::now().with_timezone(&zone).date_naive() != owner_today {
            return; // the journal's midnight passed mid-test
        }
        assert!(matches!(owner_day, DayOutcome::PastOnly), "{owner_day:?}");
        // An empty past day stops at "no data", before anything is sent.
        if host_today < owner_today {
            assert!(matches!(host_day, DayOutcome::NoData), "{host_day:?}");
        } else {
            assert!(matches!(host_day, DayOutcome::PastOnly), "{host_day:?}");
        }
    }

    #[test]
    fn past_only_uses_the_supplied_local_zone() {
        let root = TempDir::new().unwrap();
        let instant = Utc.with_ymd_and_hms(2026, 1, 2, 0, 30, 0).unwrap();
        let result = run_cli_with(
            &words(&["20260101"]),
            root.path(),
            instant,
            chrono_tz::America::Denver,
            |_| true,
        );
        assert_eq!(
            result.stderr,
            "reprocess is past-only (cannot reprocess today or a future day)\n"
        );
    }

    #[test]
    fn from_scratch_emits_literal_framed_request() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let mut sent = String::new();
        let result = run_cli_with(
            &words(&[DAY, "--from-scratch"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |envelope| {
                sent = frame_envelope(envelope).unwrap();
                true
            },
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            sent,
            "{\"tract\":\"supervisor\",\"event\":\"request\",\"cmd\":[\"solstone\",\"journal\",\"think\",\"-v\",\"--day\",\"20260101\",\"--from-scratch\"],\"day\":\"20260101\",\"queue_if_active_cmd_differs\":true}\n"
        );
    }

    /// A release burn-in states what an upgrade would regenerate before it is
    /// installed.  The report names each owed output and why, and submits
    /// nothing.
    #[test]
    fn owed_lists_each_owed_output_and_why_without_submitting() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        solstone_core_system::daily_coverage::register_daily_day(root.path(), DAY, now()).unwrap();
        let mut sent = 0;
        let result = run_cli_with(
            &words(&[DAY, "--owed"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                sent += 1;
                true
            },
        );
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        assert_eq!(sent, 0, "--owed submits nothing");
        let lines = result.stdout.lines().collect::<Vec<_>>();
        let summary = lines.last().unwrap();
        assert!(lines.len() > 1, "{}", result.stdout);
        assert!(
            lines[..lines.len() - 1]
                .iter()
                .all(|line| line.starts_with(DAY) && line.ends_with("never_made")),
            "{}",
            result.stdout
        );
        assert!(
            summary.ends_with(&format!(
                "owed output(s) on 1 past day(s): {} never_made; 0 unreadable day(s)",
                lines.len() - 1
            )),
            "{summary}"
        );
    }

    #[test]
    fn mark_updated_touches_before_failed_transport() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let marker = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("health/stream.updated");
        let result = run_cli_with(
            &words(&[DAY, "--mark-updated"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                assert!(marker.is_file());
                false
            },
        );
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.stderr, format!("{UNREACHABLE_MESSAGE}\n"));
        assert!(marker.is_file());
    }

    #[test]
    fn legacy_daily_marker_does_not_suppress_unverified_work() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        fs::write(health.join("daily.updated"), "").unwrap();
        let mut calls = 0;
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), chrono_tz::UTC, |_| {
            calls += 1;
            true
        });
        assert_eq!(result.exit_code, 0);
        assert_eq!(calls, 1);
    }

    #[test]
    fn process_now_reports_current_cap_but_conflict_remains_outstanding_and_force_retries() {
        use solstone_core_journal_io::{DailyUnitRecord, DailyUnitStatus, save_daily_unit_record};
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("note_transcript.md"),
            "# Note\nMeeting tomorrow.",
        )
        .unwrap();
        let (talent, apps) = solstone_core_system::daily_coverage::package_roots().unwrap();
        let configs =
            solstone_core_system::daily_coverage::daily_configs(root.path(), &talent, &apps)
                .unwrap();
        let overrides = configs
            .into_iter()
            .map(|config| {
                let key = match config.key.split_once(':') {
                    Some((app, name)) => format!("talent.{app}.{name}"),
                    None => format!("talent.system.{}", config.key),
                };
                (
                    key,
                    serde_json::json!({"disabled":config.key != "schedule"}),
                )
            })
            .collect::<serde_json::Map<String, serde_json::Value>>();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            serde_json::to_vec(&serde_json::json!({
                "talent_overrides": overrides,
                "providers": {
                    "active": {
                        "provider": "test"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        fs::write(health.join("daily.updated"), "").unwrap();
        let coverage =
            solstone_core_system::daily_coverage::read_daily_coverage(root.path(), DAY).unwrap();
        let unit = &coverage.units[0];
        let mut record = DailyUnitRecord::new(
            unit.identity.clone(),
            &unit.evidence_revision,
            &unit.contract_digest,
        );
        record.status = DailyUnitStatus::Capped;
        record.failure_count = 1;
        record.reason_code = Some("provider_request_rejected".into());
        save_daily_unit_record(root.path(), &record).unwrap();
        let mut calls = 0;
        let result = reprocess_day_with(
            root.path(),
            DAY,
            Flavor::ProcessNow,
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(matches!(result, DayOutcome::CurrentDegraded));
        assert_eq!(calls, 0);
        let forced = reprocess_day_with(
            root.path(),
            DAY,
            Flavor::FromScratch,
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(matches!(forced, DayOutcome::Submitted(Flavor::FromScratch)));
        record.status = DailyUnitStatus::Conflicting;
        record.reason_code = Some("daily_owner_conflict".into());
        save_daily_unit_record(root.path(), &record).unwrap();
        let conflict = reprocess_day_with(
            root.path(),
            DAY,
            Flavor::ProcessNow,
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(matches!(
            conflict,
            DayOutcome::Submitted(Flavor::ProcessNow)
        ));
        assert_eq!(calls, 2);
    }

    #[test]
    fn range_preview_and_divergent_scan_count_send_nothing() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        // This decorated name passes iter_segments but scan_day rejects its full basename.
        segment(root.path(), DAY, "x-090000_60");
        // This canonical key has no modality files, so scan_day drops it as empty.
        segment(root.path(), DAY, "100000_60");
        let mut calls = 0;
        let result = run_cli_with(
            &words(&[DAY, "--through", DAY, "--from-scratch"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(
            result.stdout,
            "from-scratch reprocess plan:\n1 days with data (0 segments) will be queued. Progress will be visible in solstone journal top or solstone journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n"
        );
    }

    #[test]
    fn empty_day_directory_is_no_data_singly_and_skipped_in_range() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::create_dir_all(root.path().join("chronicle").join("20251231")).unwrap();
        let single = run_cli_with(
            &words(&["20251231"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| true,
        );
        assert_eq!(single.exit_code, 1);
        assert_eq!(single.stderr, "no data for day 20251231\n");

        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let mut calls = 0;
        let range = run_cli_with(
            &words(&["20251231", "--through", DAY, "--from-scratch"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(
            range.stdout,
            "from-scratch reprocess plan:\n1 days with data (1 segments) will be queued. Progress will be visible in solstone journal top or solstone journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n"
        );
    }

    #[test]
    fn range_partial_failure_counts_only_data_days() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        for day in ["20251230", "20251231", "20260101"] {
            fs::write(
                segment(root.path(), day, "090000_60").join("audio.jsonl"),
                "{}\n",
            )
            .unwrap();
        }
        let mut calls = 0;
        let result = run_cli_with(
            &words(&[
                "20251229",
                "--through",
                "20260102",
                "--from-scratch",
                "--yes",
            ]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                calls < 3
            },
        );
        assert_eq!(calls, 3);
        assert_eq!(
            result.stderr,
            format!(
                "failed to queue day 3 of 3 (20260101): {UNREACHABLE_MESSAGE}\nqueued day set: 20251230, 20251231\nnot-queued day set: 20260101\n"
            )
        );
    }

    #[test]
    fn range_yes_queues_data_days_oldest_first() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        let range_now = Utc.with_ymd_and_hms(2026, 1, 4, 12, 0, 0).unwrap();
        for day in ["20251230", DAY, "20260103"] {
            fs::create_dir_all(root.path().join("chronicle").join(day)).unwrap();
        }
        for day in ["20251231", "20260102"] {
            fs::write(
                segment(root.path(), day, "090000_60").join("audio.jsonl"),
                "{}\n",
            )
            .unwrap();
        }
        let mut sent_days = Vec::new();
        let result = run_cli_with(
            &words(&[
                "20251230",
                "--through",
                "20260103",
                "--from-scratch",
                "--yes",
            ]),
            root.path(),
            range_now,
            chrono_tz::UTC,
            |envelope| {
                sent_days.push(
                    envelope.extra["day"]
                        .as_str()
                        .expect("request day")
                        .to_owned(),
                );
                true
            },
        );
        assert_eq!(sent_days, ["20251231", "20260102"]);
        assert_eq!(
            result.stdout,
            "queued from-scratch reprocess for 2 days (2 segments)\nprogress is visible in solstone journal top or solstone journal health\nqueued days do not survive a supervisor restart\n"
        );
    }

    #[test]
    fn process_now_submits_despite_matching_future_retry_for_both_kinds() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "raw\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        let fingerprint =
            solstone_core_system::catchup::read_raw_input_fingerprint(root.path(), DAY).unwrap();
        let state_path = root.path().join("health/catchup-state.json");
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        let later_retry = now().timestamp() as f64 + 10_800.0;

        // This is the former Held shape: the dirty day's raw fingerprint and
        // both catchup kinds are backoff-held until a future retry watermark.
        // `process-now` deliberately ignores that automatic backoff.
        fs::write(
            state_path,
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    format!("{DAY}:daily-catchup"): {
                        "active": null,
                        "next_retry_at": later_retry,
                        "fingerprint": fingerprint.clone(),
                    },
                    format!("{DAY}:segment-repair"): {
                        "active": null,
                        "next_retry_at": later_retry,
                        "fingerprint": fingerprint,
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();

        let mut calls = 0;
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), chrono_tz::UTC, |_| {
            calls += 1;
            true
        });
        assert_eq!(
            result.stdout,
            "reprocess (process-now) submitted for 20260101\n"
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn process_now_submits_despite_garbage_catchup_state() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "raw\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        let state_path = root.path().join("health/catchup-state.json");
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(
            state_path,
            serde_json::to_vec(&json!({
                "version": 1,
                "entries": {
                    "unrelated": ["garbage"],
                    format!("{DAY}:daily-catchup"): {
                        "active": null,
                        "next_retry_at": now().timestamp() as f64 + 3_600.0,
                        "fingerprint": "matching-fingerprint",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let mut calls = 0;
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), chrono_tz::UTC, |_| {
            calls += 1;
            true
        });
        assert_eq!(calls, 1);
        assert_eq!(
            result.stdout,
            "reprocess (process-now) submitted for 20260101\n"
        );
    }

    #[test]
    fn through_today_is_past_only() {
        let root = TempDir::new().unwrap();
        let result = run_cli_with(
            &words(&["20260101", "--through", "20260103", "--from-scratch"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| true,
        );
        assert_eq!(
            result.stderr,
            "reprocess is past-only (cannot reprocess today or a future day)\n"
        );
    }

    #[test]
    fn invalid_calendar_day_and_drain_shape_are_distinct() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        let invalid = run_cli_with(
            &words(&["20260231"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| true,
        );
        assert_eq!(invalid.stderr, "expected day in YYYYMMDD format\n");
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        let mut sent = String::new();
        let result = run_cli_with(
            &words(&[DAY]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |envelope| {
                sent = frame_envelope(envelope).unwrap();
                true
            },
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            sent,
            "{\"tract\":\"supervisor\",\"event\":\"drain\",\"day\":\"20260101\"}\n"
        );
    }

    #[test]
    fn no_thinking_engine_refuses_all_flavors_without_send() {
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "raw\n",
        )
        .unwrap();

        // 1. Missing config
        for flavor in [Flavor::ProcessNow, Flavor::FromScratch, Flavor::MarkUpdated] {
            let mut calls = 0;
            let outcome =
                reprocess_day_with(root.path(), DAY, flavor, now(), chrono_tz::UTC, |_| {
                    calls += 1;
                    true
                });
            assert!(matches!(outcome, DayOutcome::NoThinkingEngine));
            assert_eq!(calls, 0);
        }

        // Marker must not be touched
        let marker = root
            .path()
            .join("chronicle")
            .join(DAY)
            .join("health/stream.updated");
        assert!(!marker.exists());

        // 2. Empty provider string
        let config_dir = root.path().join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            serde_json::to_vec(&json!({"providers":{"active":{"provider":"   "}}})).unwrap(),
        )
        .unwrap();
        for flavor in [Flavor::ProcessNow, Flavor::FromScratch, Flavor::MarkUpdated] {
            let mut calls = 0;
            let outcome =
                reprocess_day_with(root.path(), DAY, flavor, now(), chrono_tz::UTC, |_| {
                    calls += 1;
                    true
                });
            assert!(matches!(outcome, DayOutcome::NoThinkingEngine));
            assert_eq!(calls, 0);
        }
    }

    #[test]
    fn range_preview_with_no_engine_refuses_and_does_not_preview_queued() {
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let mut calls = 0;
        let result = run_cli_with(
            &words(&[DAY, "--through", DAY, "--from-scratch"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(result.exit_code, 1);
        assert!(!result.stdout.contains("will be queued"));
        assert!(!result.stderr.contains("will be queued"));
    }

    #[test]
    fn range_execution_with_no_engine_refuses_first_day_and_sends_nothing() {
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let mut calls = 0;
        let result = run_cli_with(
            &words(&[DAY, "--through", DAY, "--from-scratch", "--yes"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(result.exit_code, 1);
        assert!(!result.stdout.contains("queued from-scratch reprocess"));
        assert!(!result.stderr.contains("queued from-scratch reprocess"));
    }

    #[test]
    fn unit_reprocess_resets_record_without_transport() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();

        // 1. Sibling unit record on the same day
        let sibling_identity = solstone_core_journal_io::DailyUnitIdentity::new(DAY, "recap", None);
        let mut sibling_record = solstone_core_journal_io::DailyUnitRecord::new(
            sibling_identity.clone(),
            "rev-sib",
            "dig-sib",
        );
        sibling_record.status = solstone_core_journal_io::DailyUnitStatus::Committed;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &sibling_record).unwrap();
        let sibling_path =
            solstone_core_journal_io::daily_unit_record_path(root.path(), &sibling_identity);
        let sibling_bytes_before = fs::read(&sibling_path).unwrap();

        // 2. Target unit record without facet
        let identity = solstone_core_journal_io::DailyUnitIdentity::new(DAY, "schedule", None);
        let mut record =
            solstone_core_journal_io::DailyUnitRecord::new(identity.clone(), "rev-1", "digest-1");
        record.status = solstone_core_journal_io::DailyUnitStatus::Failed;
        record.failure_count = 2;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();

        let mut transport_calls = 0;
        let result = run_cli_with(
            &words(&[DAY, "--unit", "schedule"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                transport_calls += 1;
                true
            },
        );
        assert_eq!(transport_calls, 0);
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.stdout,
            format!("schedule on {DAY} was reset for the next eligible run\n")
        );

        let loaded = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.status,
            solstone_core_journal_io::DailyUnitStatus::Unfinished
        );
        assert_eq!(
            loaded.evidence_revision,
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
        );
        assert_eq!(loaded.failure_count, 0);

        // Assert sibling bytes are unchanged
        let sibling_bytes_after = fs::read(&sibling_path).unwrap();
        assert_eq!(sibling_bytes_before, sibling_bytes_after);

        // 3. Target unit record with facet
        let facet_identity =
            solstone_core_journal_io::DailyUnitIdentity::new(DAY, "entities", Some("work".into()));
        let mut facet_record = solstone_core_journal_io::DailyUnitRecord::new(
            facet_identity.clone(),
            "rev-2",
            "digest-2",
        );
        facet_record.status = solstone_core_journal_io::DailyUnitStatus::Conflicting;
        facet_record.failure_count = 1;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &facet_record).unwrap();

        let result_facet = run_cli_with(
            &words(&[DAY, "--unit", "entities", "--facet", "work"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| {
                transport_calls += 1;
                true
            },
        );
        assert_eq!(transport_calls, 0);
        assert_eq!(result_facet.exit_code, 0);
        assert_eq!(
            result_facet.stdout,
            format!("entities (work) on {DAY} was reset for the next eligible run\n")
        );
        let loaded_facet =
            solstone_core_journal_io::load_daily_unit_record(root.path(), &facet_identity)
                .unwrap()
                .unwrap();
        assert_eq!(
            loaded_facet.status,
            solstone_core_journal_io::DailyUnitStatus::Unfinished
        );
        assert_eq!(
            loaded_facet.evidence_revision,
            solstone_core_journal_io::OWNER_REPROCESS_SENTINEL
        );
        assert_eq!(loaded_facet.failure_count, 0);
    }

    #[test]
    fn unit_reprocess_validates_exclusions_and_facet() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();

        // --facet requires --unit (exit 2)
        let no_unit = run_cli_with(
            &words(&[DAY, "--facet", "work"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(no_unit.exit_code, 2);
        assert!(no_unit.stderr.contains("argument --facet: requires --unit"));

        // Mutual exclusivity with --through, --yes, --from-scratch, --mark-updated, --owed (both orders)
        let pairs = [
            (
                vec![DAY, "--unit", "schedule", "--through", DAY],
                "argument --unit: not allowed with argument --through",
            ),
            (
                vec![DAY, "--through", DAY, "--unit", "schedule"],
                "argument --unit: not allowed with argument --through",
            ),
            (
                vec![DAY, "--unit", "schedule", "--yes"],
                "argument --unit: not allowed with argument --yes",
            ),
            (
                vec![DAY, "--yes", "--unit", "schedule"],
                "argument --unit: not allowed with argument --yes",
            ),
            (
                vec![DAY, "--unit", "schedule", "--from-scratch"],
                "argument --from-scratch: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--from-scratch", "--unit", "schedule"],
                "argument --unit: not allowed with argument --from-scratch",
            ),
            (
                vec![DAY, "--unit", "schedule", "--mark-updated"],
                "argument --mark-updated: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--mark-updated", "--unit", "schedule"],
                "argument --unit: not allowed with argument --mark-updated",
            ),
            (
                vec![DAY, "--unit", "schedule", "--owed"],
                "argument --owed: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--owed", "--unit", "schedule"],
                "argument --unit: not allowed with argument --owed",
            ),
        ];
        for (args, expected_fragment) in pairs {
            let res = run_cli_with(&words(&args), root.path(), now(), chrono_tz::UTC, |_| false);
            assert_eq!(res.exit_code, 2, "failed for args: {:?}", args);
            assert!(
                res.stderr.contains(expected_fragment)
                    || res.stderr.contains("not allowed with argument"),
                "stderr {:?} did not contain expected for {:?}",
                res.stderr,
                args
            );
        }
    }

    #[test]
    fn unit_reprocess_operational_refusals() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();

        // 1. Today or future day (now is 20260103)
        let res_today = run_cli_with(
            &words(&["20260103", "--unit", "schedule"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(res_today.exit_code, 1);
        assert!(res_today.stderr.contains("reprocess is past-only"));

        // 2. Malformed day
        let res_malformed = run_cli_with(
            &words(&["not-a-day", "--unit", "schedule"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(res_malformed.exit_code, 1);
        assert!(
            res_malformed
                .stderr
                .contains("expected day in YYYYMMDD format")
        );

        // 3. Record not found
        let res_not_found = run_cli_with(
            &words(&[DAY, "--unit", "schedule"]),
            root.path(),
            now(),
            chrono_tz::UTC,
            |_| false,
        );
        assert_eq!(res_not_found.exit_code, 1);
        assert!(res_not_found.stderr.contains("reprocess unit failed:"));
    }
}
