// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `journal reprocess` command body.

use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use jiff::civil::Date;
use serde_json::json;
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_cli_copy as copy;
use solstone_core_journal_config::TimeZone;
use solstone_core_segment::{PathOrDay, day_path, iter_segments, touch_stream_health_marker};
use solstone_core_system_health::{FilesystemSegmentSource, scan_day};

// ⛔ `journal up`, not `journal start`. `journal start` runs the supervisor
// in the FOREGROUND (SKILL.md: "starts the supervisor runtime only"), so an
// owner who follows it gets a process tied to that terminal and loses intake
// when they close it. `journal up` is the alias for `journal service start`.
fn waiting_message() -> String {
    copy::text("journal.cli.reprocess.waiting")
}

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
    json: bool,
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
    NoThinkingEngine,
    WaitingForSupervisor,
    Failed(String),
}

/// Run with the real socket transport and the journal's zone.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    let zone = solstone_core_journal_config::owner_zone(journal_path);
    run_cli_with(args, journal_path, Timestamp::now(), &zone, |envelope| {
        send_envelope(journal_path, envelope)
    })
}

/// Run with explicit time, zone, and transport seams.
pub fn run_cli_with<F>(
    args: &[String],
    journal_path: &Path,
    now: Timestamp,
    zone: &TimeZone,
    mut transport: F,
) -> CliRun
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let parsed = match parse_arguments(args) {
        Ok(parsed) => parsed,
        Err(ParseResult::Help) => return success(reprocess_help()),
        Err(ParseResult::Usage(message)) => return usage_error(message),
    };

    if let Some(unit_name) = parsed.unit.as_deref() {
        let Some(day_date) = parse_day(journal_path, &parsed.day) else {
            return failure(&copy::text("journal.cli.backfill.invalid.day"));
        };
        let today = now.to_zoned_datetime(zone.clone()).date();
        if day_date >= today {
            return failure(&copy::text("journal.cli.reprocess.past.only"));
        }
        let unit = solstone_core_system::work_retry::Unit::Daily {
            day: parsed.day.clone(),
            talent: unit_name.into(),
            facet: parsed.facet.clone(),
        };
        let requested = solstone_core_system::work_retry::daily::request_in(
            journal_path,
            &unit,
            copy::cli_locale(),
        );
        if let Err(error) = requested {
            return failure(&copy::strings(
                "journal.cli.reprocess.unit.failed",
                [("error", &error)],
            ));
        }
        let _ = transport(&drain_envelope(&parsed.day));
        return success(format!(
            "{}\n",
            copy::strings(
                "journal.cli.reprocess.unit.saved",
                [("unit", unit_name), ("day", &parsed.day)]
            )
        ));
    }

    if parsed.flavor == Flavor::Owed {
        return owed_report(
            journal_path,
            &parsed.day,
            parsed.through.as_deref(),
            now,
            zone,
            parsed.json,
        );
    }
    if let Some(through_raw) = parsed.through.as_deref() {
        if parsed.flavor != Flavor::FromScratch {
            return failure(&copy::text("journal.cli.reprocess.through.required"));
        }
        let Some(start) = parse_day(journal_path, &parsed.day) else {
            return failure(&copy::text("journal.cli.backfill.invalid.day"));
        };
        let Some(through) = parse_day(journal_path, through_raw) else {
            return failure(&copy::text("journal.cli.backfill.invalid.day"));
        };
        let today = now.to_zoned_datetime(zone.clone()).date();
        if start >= today || through >= today {
            return failure(&copy::text("journal.cli.reprocess.past.only"));
        }
        if through < start {
            return failure(&copy::text("journal.cli.reprocess.through.order"));
        }
        let days = enumerate_range_days(journal_path, start, through, now);
        if data_days(&days).is_empty() {
            return failure(&copy::strings(
                "journal.cli.reprocess.empty.range",
                [("day", &parsed.day), ("through", through_raw)],
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
        reprocess_day_with_locale(
            journal_path,
            &parsed.day,
            parsed.flavor,
            now,
            zone,
            copy::cli_locale(),
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
    let mut json = false;
    let mut unknown = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-v" | "--verbose" | "-d" | "--debug" => {}
            "--json" => json = true,
            "--yes" => {
                if unit.is_some() {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", "--yes"), ("previous", "--unit")],
                    )));
                }
                yes = true;
            }
            "--through" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.missing.value",
                        [("option", "--through")],
                    )));
                };
                if unit.is_some() {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", "--through"), ("previous", "--unit")],
                    )));
                }
                through_flag = true;
                through = Some(value.clone());
            }
            "--unit" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.missing.value",
                        [("option", "--unit")],
                    )));
                };
                if through_flag {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", "--unit"), ("previous", "--through")],
                    )));
                }
                if yes {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", "--unit"), ("previous", "--yes")],
                    )));
                }
                if let Some(previous) = flavor_flag {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", "--unit"), ("previous", previous)],
                    )));
                }
                unit = Some(value.clone());
            }
            "--facet" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.missing.value",
                        [("option", "--facet")],
                    )));
                };
                facet = Some(value.clone());
            }
            "--from-scratch" | "--mark-updated" | "--owed" => {
                if unit.is_some() {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", argument), ("previous", "--unit")],
                    )));
                }
                if let Some(previous) = flavor_flag {
                    return Err(ParseResult::Usage(copy::strings(
                        "journal.cli.reprocess.usage.conflict",
                        [("option", argument), ("previous", previous)],
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
        return Err(ParseResult::Usage(copy::strings(
            "journal.cli.reprocess.usage.requires",
            [("option", "--facet"), ("required", "--unit")],
        )));
    }
    if json && flavor != Flavor::Owed {
        return Err(ParseResult::Usage(copy::strings(
            "journal.cli.reprocess.usage.requires",
            [("option", "--json"), ("required", "--owed")],
        )));
    }
    let Some(day) = day else {
        return Err(ParseResult::Usage(copy::strings(
            "journal.cli.reprocess.usage.missing.day",
            [("positional", "day")],
        )));
    };
    if unknown.is_empty() {
        Ok(ParsedArgs {
            day,
            through,
            yes,
            flavor,
            unit,
            facet,
            json,
        })
    } else {
        Err(ParseResult::Usage(copy::strings(
            "journal.cli.reprocess.usage.unknown",
            [("arguments", &(unknown.join(" ")))],
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
    reprocess_day_with(
        journal_path,
        day,
        flavor,
        Timestamp::now(),
        &zone,
        |envelope| send_envelope(journal_path, envelope),
    )
}

/// Reprocess one day with explicit time, zone, and transport seams.
pub fn reprocess_day_with<F>(
    journal: &Path,
    day: &str,
    flavor: Flavor,
    now: Timestamp,
    zone: &TimeZone,
    transport: F,
) -> DayOutcome
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    reprocess_day_with_locale(journal, day, flavor, now, zone, copy::ENGLISH, transport)
}

pub fn reprocess_day_with_locale<F>(
    journal: &Path,
    day: &str,
    flavor: Flavor,
    now: Timestamp,
    zone: &TimeZone,
    locale: &str,
    mut transport: F,
) -> DayOutcome
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    if flavor == Flavor::Owed {
        return DayOutcome::Failed(copy::render_for(
            locale,
            "journal.cli.reprocess.owed.internal",
            &[],
        ));
    }
    let Some(parsed) = parse_day(journal, day) else {
        return DayOutcome::Malformed;
    };
    if parsed >= now.to_zoned_datetime(zone.clone()).date() {
        return DayOutcome::PastOnly;
    }
    let Ok(day_directory) = day_path(journal, day, false) else {
        return DayOutcome::Malformed;
    };
    let has_data = day_directory.is_dir()
        && iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| !segments.is_empty())
        && !solstone_core_segment::day_holds_nothing_the_owner_did(journal, day);
    if !has_data {
        return DayOutcome::NoData;
    }
    if solstone_core_system::no_thinking_engine_chosen(journal) {
        return DayOutcome::NoThinkingEngine;
    }
    if flavor == Flavor::FromScratch {
        if let Err(error) =
            solstone_core_system::work_intent::register_in(journal, day, true, locale)
        {
            return DayOutcome::Failed(error);
        }
        return if transport(&drain_envelope(day)) {
            DayOutcome::Submitted(flavor)
        } else {
            DayOutcome::WaitingForSupervisor
        };
    }
    if flavor == Flavor::MarkUpdated {
        // Persist the requeue intent before a socket failure can lose it.
        if let Err(error) = touch_stream_health_marker(journal, day) {
            return DayOutcome::Failed(error.to_string());
        }
        if let Err(error) =
            solstone_core_system::work_intent::register_in(journal, day, false, locale)
        {
            return DayOutcome::Failed(error);
        }
        return if transport(&drain_envelope(day)) {
            DayOutcome::Submitted(flavor)
        } else {
            DayOutcome::WaitingForSupervisor
        };
    }
    // A current daily summary does not prove that native or activity work is
    // complete. Discovery expands this durable ask against every owning source.
    if let Err(error) = solstone_core_system::work_intent::register_in(journal, day, false, locale)
    {
        return DayOutcome::Failed(error);
    }
    if transport(&drain_envelope(day)) {
        DayOutcome::Submitted(flavor)
    } else {
        DayOutcome::WaitingForSupervisor
    }
}

fn render_day_outcome(day: &str, outcome: DayOutcome) -> CliRun {
    match outcome {
        DayOutcome::Malformed => failure(&copy::text("journal.cli.backfill.invalid.day")),
        DayOutcome::PastOnly => failure(&copy::text("journal.cli.reprocess.past.only")),
        DayOutcome::NoData => failure(&copy::strings(
            "journal.cli.reprocess.no.data",
            [("day", day)],
        )),
        DayOutcome::Submitted(Flavor::FromScratch) => success(format!(
            "{}\n",
            copy::strings(
                "journal.cli.reprocess.submitted.from.scratch",
                [("day", day)]
            )
        )),
        DayOutcome::Submitted(Flavor::MarkUpdated) => success(format!(
            "{}\n",
            copy::strings(
                "journal.cli.reprocess.submitted.mark.updated",
                [("day", day)]
            )
        )),
        DayOutcome::Submitted(Flavor::ProcessNow) => success(format!(
            "{}\n",
            copy::strings(
                "journal.cli.reprocess.submitted.process.now",
                [("day", day)]
            )
        )),
        DayOutcome::Submitted(Flavor::Owed) => {
            failure(&copy::text("journal.cli.reprocess.owed.only"))
        }
        DayOutcome::NoThinkingEngine => failure(&copy::text("journal.cli.reprocess.no.engine")),
        DayOutcome::WaitingForSupervisor => success(format!("{}\n", waiting_message())),
        DayOutcome::Failed(error) => failure(&solstone_core_window_catalog::strings(
            copy::cli_locale(),
            "journal.window.health.action.reprocess.failed.cause",
            [("cause", error.as_str())],
        )),
    }
}

fn enumerate_range_days(
    journal: &Path,
    start: Date,
    through: Date,
    now: Timestamp,
) -> Vec<RangeDay> {
    let mut days = Vec::new();
    let mut current = start;
    while current <= through {
        let day = solstone_core_time::day_key(current);
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
            .tomorrow()
            .expect("range day advances within date bounds");
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
    let segments = range_segment_count(days);
    let days = data_days(days).len();
    format!(
        "{}\n",
        copy::render(
            "journal.cli.reprocess.range.plan",
            &[
                (
                    "days_quantity",
                    copy::Value::Cardinal(days.min(i32::MAX as usize) as i64)
                ),
                (
                    "segments_quantity",
                    copy::Value::Cardinal(segments.min(i32::MAX as usize) as i64)
                ),
                ("days", copy::Value::String(&days.to_string())),
                ("segments", copy::Value::String(&segments.to_string()))
            ]
        )
    )
}

fn run_from_scratch_range<F>(
    journal: &Path,
    days: &[RangeDay],
    now: Timestamp,
    zone: &TimeZone,
    transport: &mut F,
) -> CliRun
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let data_days = data_days(days);
    let mut queued = Vec::new();
    let mut waiting = false;
    for entry in days {
        match reprocess_day_with_locale(
            journal,
            &entry.day,
            Flavor::FromScratch,
            now,
            zone,
            copy::cli_locale(),
            &mut *transport,
        ) {
            DayOutcome::NoData => {}
            DayOutcome::Submitted(_) => queued.push(entry.day.clone()),
            DayOutcome::WaitingForSupervisor => {
                waiting = true;
                queued.push(entry.day.clone());
            }
            other => return render_day_outcome(&entry.day, other),
        }
    }
    let segments = range_segment_count(days);
    let days = data_days.len();
    let mut output = copy::render(
        "journal.cli.reprocess.range.saved",
        &[
            (
                "days_quantity",
                copy::Value::Cardinal(days.min(i32::MAX as usize) as i64),
            ),
            (
                "segments_quantity",
                copy::Value::Cardinal(segments.min(i32::MAX as usize) as i64),
            ),
            ("days", copy::Value::String(&days.to_string())),
            ("segments", copy::Value::String(&segments.to_string())),
        ],
    );
    output.push('\n');
    if waiting {
        output.push_str(&waiting_message());
        output.push('\n');
    }
    success(output)
}

/// List every daily output owed on past days in the range, and why.
///
/// Read-only: nothing is submitted.  A release burn-in runs the candidate's
/// build of this against the journal before installing it, so the number of
/// past outputs an upgrade would regenerate is stated rather than discovered.
/// With `json`, each day is reported as states and counts only: its markers,
/// its coverage, how many daily units it has and how many are owed, by cause.
fn owed_report(
    journal: &Path,
    start_raw: &str,
    through_raw: Option<&str>,
    now: Timestamp,
    zone: &TimeZone,
    json: bool,
) -> CliRun {
    let Some(start) = parse_day(journal, start_raw) else {
        return failure(&copy::text("journal.cli.backfill.invalid.day"));
    };
    let through = match through_raw {
        Some(raw) => match parse_day(journal, raw) {
            Some(day) => day,
            None => return failure(&copy::text("journal.cli.backfill.invalid.day")),
        },
        None => start,
    };
    if through < start {
        return failure(&copy::text("journal.cli.reprocess.through.order"));
    }
    let today = now.to_zoned_datetime(zone.clone()).date();
    let through = through.min(today.yesterday().expect("today has a yesterday"));
    let mut lines = Vec::new();
    let mut json_days = Vec::new();
    let mut causes = std::collections::BTreeMap::<String, usize>::new();
    let (mut owed, mut days, mut unreadable) = (0usize, 0usize, 0usize);
    let mut current = start;
    while current <= through {
        let day = solstone_core_time::day_key(current);
        current = current
            .tomorrow()
            .expect("owed day advances within date bounds");
        if !day_path(journal, &day, false).is_ok_and(|path| path.is_dir()) {
            continue;
        }
        let coverage = solstone_core_system::daily_coverage::read_daily_coverage_in(
            journal,
            &day,
            copy::cli_locale(),
        );
        if json {
            let markers = match solstone_core_journal_io::day_marker_pair_status(journal, &day) {
                Ok(status) if status.is_complete() => "complete",
                Ok(_) => "dirty",
                Err(_) => "unreadable",
            };
            json_days.push(match &coverage {
                Ok(coverage) => {
                    let mut owed_by = std::collections::BTreeMap::<String, usize>::new();
                    for unit in coverage.units.iter().filter(|unit| unit.state.is_owed()) {
                        *owed_by
                            .entry(unit.owed_by.clone().unwrap_or_else(|| "unknown".to_owned()))
                            .or_default() += 1;
                    }
                    json!({
                        "day": day,
                        "markers": markers,
                        "coverage": coverage.state,
                        "units": coverage.units.len(),
                        "owed": owed_by.values().sum::<usize>(),
                        "owed_by": owed_by,
                    })
                }
                Err(_) => json!({"day": day, "markers": markers, "coverage": "unreadable"}),
            });
        }
        match coverage {
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
                lines.push(copy::strings(
                    "journal.cli.reprocess.unreadable",
                    [("day", &day), ("error", &error)],
                ));
                unreadable += 1;
            }
        }
    }
    let summary = causes
        .iter()
        .map(|(cause, count)| format!("{count} {cause}"))
        .collect::<Vec<_>>()
        .join(", ");
    if json {
        return success(
            json!({
                "days": json_days,
                "owed": owed,
                "owed_days": days,
                "unreadable_days": unreadable,
            })
            .to_string()
                + "\n",
        );
    }
    lines.push(if summary.is_empty() {
        copy::render(
            "journal.cli.reprocess.owed.summary",
            &[
                (
                    "owed_quantity",
                    copy::Value::Cardinal(owed.min(i32::MAX as usize) as i64),
                ),
                (
                    "days_quantity",
                    copy::Value::Cardinal(days.min(i32::MAX as usize) as i64),
                ),
                (
                    "unreadable_quantity",
                    copy::Value::Cardinal(unreadable.min(i32::MAX as usize) as i64),
                ),
                ("owed", copy::Value::String(&owed.to_string())),
                ("days", copy::Value::String(&days.to_string())),
                ("unreadable", copy::Value::String(&unreadable.to_string())),
            ],
        )
    } else {
        copy::render(
            "journal.cli.reprocess.owed.summary.causes",
            &[
                (
                    "owed_quantity",
                    copy::Value::Cardinal(owed.min(i32::MAX as usize) as i64),
                ),
                (
                    "days_quantity",
                    copy::Value::Cardinal(days.min(i32::MAX as usize) as i64),
                ),
                (
                    "unreadable_quantity",
                    copy::Value::Cardinal(unreadable.min(i32::MAX as usize) as i64),
                ),
                ("owed", copy::Value::String(&owed.to_string())),
                ("days", copy::Value::String(&days.to_string())),
                ("unreadable", copy::Value::String(&unreadable.to_string())),
                ("summary", copy::Value::String(&summary)),
            ],
        )
    });
    success(lines.join("\n") + "\n")
}

fn parse_day(journal: &Path, day: &str) -> Option<Date> {
    day_path(journal, day, false).ok()?;
    solstone_core_time::parse_day(day)
}

fn drain_envelope(day: &str) -> CallosumEnvelope {
    CallosumEnvelope::supervisor_drain(day)
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
    format!("{}\n", copy::text("journal.cli.reprocess.help"))
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

fn usage_error(message: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("{message}\n"),
        exit_code: 2,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;

    const DAY: &str = "20260101";
    const HELP: &str = "usage: solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]\n                         [--facet FACET] [--from-scratch | --mark-updated |\n                         --owed] [--json] [-v] [-d]\n                         day\n\nSubmit a past journal day for reprocessing\n\npositional arguments:\n  day                Past day in YYYYMMDD format\n\noptions:\n  -h, --help         show this help message and exit\n  --through THROUGH  Inclusive range end in YYYYMMDD format\n  --yes\n  --unit UNIT        Request a retry of one failed daily processing step\n  --facet FACET      Facet of that unit; required when the unit has a facet\n  --from-scratch     Force a full daily re-run, preserving markers (does not\n                     flag the day as updated)\n  --mark-updated     Flag the day as having new raw data so daily processing\n                     re-queues it, then nudge a drain\n  --owed             List the daily outputs owed on the day or range and why,\n                     without submitting anything\n  --json             With --owed, report each day as states and counts: its\n                     markers, its coverage, how many daily outputs it has and\n                     how many are owed, by cause\n  -v, --verbose      Enable verbose output\n  -d, --debug        Enable debug logging\n";
    const MISSING_DAY_STDERR: &str = "usage: solstone journal reprocess [-h] [--through THROUGH] [--yes] [--unit UNIT]\n                         [--facet FACET] [--from-scratch | --mark-updated |\n                         --owed] [--json] [-v] [-d]\n                         day\nsolstone journal reprocess: error: the following arguments are required: day\n";

    fn words(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn now() -> Timestamp {
        "2026-01-03T12:00:00Z".parse().unwrap()
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
                &TimeZone::UTC,
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
            &TimeZone::UTC,
            |_| false,
        );
        assert_eq!(no_day.exit_code, 2);
        assert_eq!(no_day.stderr, MISSING_DAY_STDERR);
        let unknown = run_cli_with(
            &words(&[DAY, "--nonsense"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| false,
        );
        assert_eq!(
            unknown.stderr,
            format!(
                "{}solstone journal reprocess: error: unrecognized arguments: --nonsense\n",
                MISSING_DAY_STDERR
                    .split("solstone journal reprocess: error:")
                    .next()
                    .unwrap()
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
                &TimeZone::UTC,
                |_| false,
            );
            assert_eq!(
                result.stderr,
                format!(
                    "{}solstone journal reprocess: error: argument {second}: not allowed with argument {first}\n",
                    MISSING_DAY_STDERR
                        .split("solstone journal reprocess: error:")
                        .next()
                        .unwrap()
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
                let result = run_cli_with(&flagged, root.path(), now(), &TimeZone::UTC, |_| true);
                let expected =
                    run_cli_with(&baseline, root.path(), now(), &TimeZone::UTC, |_| true);
                assert_eq!(result.exit_code, 0, "{flag} {flavor:?}");
                assert_eq!(result.stdout, expected.stdout, "{flag} {flavor:?}");
                assert_eq!(result.stderr, expected.stderr, "{flag} {flavor:?}");
            }
        }
    }

    #[test]
    fn past_only_is_the_journals_today_not_this_computers() {
        let root = TempDir::new().unwrap();
        let now = Timestamp::now();
        let host_today = now
            .to_zoned_datetime(solstone_core_journal_config::host_zone())
            .date();
        let zone = ["Pacific/Kiritimati", "Etc/GMT+12"]
            .into_iter()
            .map(|name| solstone_core_time::zone(name).expect("zone is known"))
            .find(|zone| now.to_zoned_datetime(zone.clone()).date() != host_today)
            .expect("UTC+14 and UTC-12 never share a date");
        let owner_today = now.to_zoned_datetime(zone.clone()).date();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(
            root.path().join("config/journal.json"),
            json!({"identity": {"timezone": solstone_core_journal_config::zone_name(&zone)}})
                .to_string(),
        )
        .unwrap();
        let day = solstone_core_time::day_key;
        let owner_day = reprocess_day(root.path(), &day(owner_today), Flavor::FromScratch);
        let host_day = reprocess_day(root.path(), &day(host_today), Flavor::FromScratch);
        if Timestamp::now().to_zoned_datetime(zone.clone()).date() != owner_today {
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
        let instant: Timestamp = "2026-01-02T00:30:00Z".parse().unwrap();
        let result = run_cli_with(
            &words(&["20260101"]),
            root.path(),
            instant,
            &solstone_core_time::zone("America/Denver").unwrap(),
            |_| true,
        );
        assert_eq!(
            result.stderr,
            "reprocess is past-only (cannot reprocess today or a future day)\n"
        );
    }

    #[test]
    fn from_scratch_saves_intent_before_waking_discovery() {
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
            &TimeZone::UTC,
            |envelope| {
                sent = frame_envelope(envelope).unwrap();
                true
            },
        );
        assert_eq!(result.exit_code, 0);
        let envelope: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(envelope["event"], "drain");
        assert!(envelope.get("cmd").is_none());
        let intent = solstone_core_system::work_intent::read(root.path()).unwrap();
        assert!(intent[DAY].from_scratch);
        assert_eq!(
            intent[DAY].state,
            solstone_core_system::work_intent::IntentState::Queued
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
        let mut sent = 0;
        let result = run_cli_with(
            &words(&[DAY, "--owed"]),
            root.path(),
            now(),
            &TimeZone::UTC,
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
    fn owed_json_reports_each_day_as_states_and_counts() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n",
        )
        .unwrap();
        let result = run_cli_with(
            &words(&[DAY, "--owed", "--json"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| panic!("--owed submits nothing"),
        );
        assert_eq!(result.exit_code, 0, "{}", result.stderr);
        let report: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
        let day = &report["days"][0];
        assert_eq!(day["day"], DAY);
        assert_eq!(day["coverage"], "outstanding");
        let owed = day["owed"].as_u64().unwrap();
        assert!(owed > 0 && owed == day["units"].as_u64().unwrap());
        assert_eq!(day["owed_by"], json!({"never_made": owed}));
        assert_eq!(report["owed"], owed);
        assert_eq!(report["unreadable_days"], 0);
        // States and counts only: no unit, facet or path reaches the report.
        let keys = |value: &serde_json::Value| {
            value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(day),
            ["day", "markers", "coverage", "units", "owed", "owed_by"]
        );

        let refused = run_cli_with(
            &words(&[DAY, "--json"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| true,
        );
        assert_eq!(refused.exit_code, 2);
        assert!(refused.stderr.contains("argument --json: requires --owed"));
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
            &TimeZone::UTC,
            |_| {
                assert!(marker.is_file());
                false
            },
        );
        assert_eq!(result.exit_code, 0);
        assert!(result.stderr.is_empty());
        assert!(
            solstone_core_system::work_intent::read(root.path())
                .unwrap()
                .contains_key(DAY)
        );
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
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), &TimeZone::UTC, |_| {
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
                    serde_json::json!({"disabled":config.key != "morning_briefing"}),
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
            &TimeZone::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(matches!(result, DayOutcome::Submitted(Flavor::ProcessNow)));
        assert_eq!(calls, 1);
        let forced = reprocess_day_with(
            root.path(),
            DAY,
            Flavor::FromScratch,
            now(),
            &TimeZone::UTC,
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
            &TimeZone::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert!(matches!(
            conflict,
            DayOutcome::Submitted(Flavor::ProcessNow)
        ));
        assert_eq!(calls, 3);
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
            &TimeZone::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(
            result.stdout,
            "from-scratch reprocess plan:\n1 day with data (0 segments) will be queued. Progress will be visible in solstone journal top or solstone journal health. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.\nThis day can take hours; today's work goes first between tasks.\nre-run with --yes to proceed\n"
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
            &TimeZone::UTC,
            |_| true,
        );
        assert_eq!(single.exit_code, 1);
        assert_eq!(single.stderr, "no data for day 20251231\n");

        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "{}\n{\"start\":0,\"text\":\"fixture speech\"}\n",
        )
        .unwrap();
        let mut calls = 0;
        let range = run_cli_with(
            &words(&["20251231", "--through", DAY, "--from-scratch"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| {
                calls += 1;
                true
            },
        );
        assert_eq!(calls, 0);
        assert_eq!(
            range.stdout,
            "from-scratch reprocess plan:\n1 day with data (1 segment) will be queued. Progress will be visible in solstone journal top or solstone journal health. Requests survive a supervisor restart; a day that cannot finish from scratch finishes as an ordinary reprocess.\nThis day can take hours; today's work goes first between tasks.\nre-run with --yes to proceed\n"
        );
    }

    #[test]
    fn range_disconnected_wake_still_saves_every_data_day() {
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
            &TimeZone::UTC,
            |_| {
                calls += 1;
                calls < 3
            },
        );
        assert_eq!(calls, 3);
        assert_eq!(result.exit_code, 0);
        let intents = solstone_core_system::work_intent::read(root.path()).unwrap();
        assert_eq!(
            intents.keys().map(String::as_str).collect::<Vec<_>>(),
            ["20251230", "20251231", "20260101"]
        );
    }

    #[test]
    fn range_yes_queues_data_days_oldest_first() {
        let root = TempDir::new().unwrap();
        write_test_provider(root.path());
        let range_now: Timestamp = "2026-01-04T12:00:00Z".parse().unwrap();
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
            &TimeZone::UTC,
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
        assert_eq!(result.exit_code, 0);
        let intents = solstone_core_system::work_intent::read(root.path()).unwrap();
        assert_eq!(
            intents.keys().map(String::as_str).collect::<Vec<_>>(),
            ["20251231", "20260102"]
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
        let later_retry = now().as_second() as f64 + 10_800.0;

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
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), &TimeZone::UTC, |_| {
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
                        "next_retry_at": now().as_second() as f64 + 3_600.0,
                        "fingerprint": "matching-fingerprint",
                    },
                },
            }))
            .unwrap(),
        )
        .unwrap();
        let mut calls = 0;
        let result = run_cli_with(&words(&[DAY]), root.path(), now(), &TimeZone::UTC, |_| {
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
            &TimeZone::UTC,
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
            &TimeZone::UTC,
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
            &TimeZone::UTC,
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
                reprocess_day_with(root.path(), DAY, flavor, now(), &TimeZone::UTC, |_| {
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
                reprocess_day_with(root.path(), DAY, flavor, now(), &TimeZone::UTC, |_| {
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
            &TimeZone::UTC,
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
            &TimeZone::UTC,
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
    fn unit_reprocess_saves_intent_without_resetting_output_or_conflicts() {
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
        let identity =
            solstone_core_journal_io::DailyUnitIdentity::new(DAY, "morning_briefing", None);
        let mut record =
            solstone_core_journal_io::DailyUnitRecord::new(identity.clone(), "rev-1", "digest-1");
        record.status = solstone_core_journal_io::DailyUnitStatus::Failed;
        record.failure_count = 2;
        solstone_core_journal_io::save_daily_unit_record(root.path(), &record).unwrap();

        let mut transport_calls = 0;
        let result = run_cli_with(
            &words(&[DAY, "--unit", "morning_briefing"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| {
                transport_calls += 1;
                true
            },
        );
        assert_eq!(transport_calls, 1);
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.stdout,
            format!("retry request saved for morning_briefing on {DAY}\n")
        );

        let loaded = solstone_core_journal_io::load_daily_unit_record(root.path(), &identity)
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded.status,
            solstone_core_journal_io::DailyUnitStatus::Failed
        );
        assert_eq!(loaded.evidence_revision, "rev-1");
        assert_eq!(loaded.failure_count, 2);
        let (held, errors) = solstone_core_system::work_retry::list(root.path());
        assert!(errors.is_empty());
        assert_eq!(held.len(), 1);
        assert!(held[0].owner.is_some());

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
            &TimeZone::UTC,
            |_| {
                transport_calls += 1;
                true
            },
        );
        assert_eq!(transport_calls, 1);
        assert_eq!(result_facet.exit_code, 1);
        let loaded_facet =
            solstone_core_journal_io::load_daily_unit_record(root.path(), &facet_identity)
                .unwrap()
                .unwrap();
        assert_eq!(
            loaded_facet.status,
            solstone_core_journal_io::DailyUnitStatus::Conflicting
        );
        assert_eq!(loaded_facet.evidence_revision, "rev-2");
        assert_eq!(loaded_facet.failure_count, 1);
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
            &TimeZone::UTC,
            |_| false,
        );
        assert_eq!(no_unit.exit_code, 2);
        assert!(no_unit.stderr.contains("argument --facet: requires --unit"));

        // Mutual exclusivity with --through, --yes, --from-scratch, --mark-updated, --owed (both orders)
        let pairs = [
            (
                vec![DAY, "--unit", "morning_briefing", "--through", DAY],
                "argument --unit: not allowed with argument --through",
            ),
            (
                vec![DAY, "--through", DAY, "--unit", "morning_briefing"],
                "argument --unit: not allowed with argument --through",
            ),
            (
                vec![DAY, "--unit", "morning_briefing", "--yes"],
                "argument --unit: not allowed with argument --yes",
            ),
            (
                vec![DAY, "--yes", "--unit", "morning_briefing"],
                "argument --unit: not allowed with argument --yes",
            ),
            (
                vec![DAY, "--unit", "morning_briefing", "--from-scratch"],
                "argument --from-scratch: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--from-scratch", "--unit", "morning_briefing"],
                "argument --unit: not allowed with argument --from-scratch",
            ),
            (
                vec![DAY, "--unit", "morning_briefing", "--mark-updated"],
                "argument --mark-updated: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--mark-updated", "--unit", "morning_briefing"],
                "argument --unit: not allowed with argument --mark-updated",
            ),
            (
                vec![DAY, "--unit", "morning_briefing", "--owed"],
                "argument --owed: not allowed with argument --unit",
            ),
            (
                vec![DAY, "--owed", "--unit", "morning_briefing"],
                "argument --unit: not allowed with argument --owed",
            ),
        ];
        for (args, expected_fragment) in pairs {
            let res = run_cli_with(&words(&args), root.path(), now(), &TimeZone::UTC, |_| false);
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
            &words(&["20260103", "--unit", "morning_briefing"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| false,
        );
        assert_eq!(res_today.exit_code, 1);
        assert!(res_today.stderr.contains("reprocess is past-only"));

        // 2. Malformed day
        let res_malformed = run_cli_with(
            &words(&["not-a-day", "--unit", "morning_briefing"]),
            root.path(),
            now(),
            &TimeZone::UTC,
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
            &words(&[DAY, "--unit", "morning_briefing"]),
            root.path(),
            now(),
            &TimeZone::UTC,
            |_| false,
        );
        assert_eq!(res_not_found.exit_code, 1);
        assert!(res_not_found.stderr.contains("reprocess unit failed:"));
    }
}
