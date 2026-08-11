// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `journal reprocess` command body.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_callosum::{CallosumEnvelope, CallosumOneShotSender};
use solstone_core_segment::{PathOrDay, day_path, iter_segments, touch_stream_health_marker};
use solstone_core_system::catchup::read_raw_input_fingerprint;
use solstone_core_system_health::{FilesystemSegmentSource, day_is_complete, scan_day};

const UNREACHABLE_MESSAGE: &str = "supervisor not reachable - start it (journal start), then retry";
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
enum Flavor {
    ProcessNow,
    FromScratch,
    MarkUpdated,
}

#[derive(Debug)]
struct ParsedArgs {
    day: String,
    through: Option<String>,
    yes: bool,
    flavor: Flavor,
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
enum DayOutcome {
    Malformed,
    PastOnly,
    NoData,
    Submitted(Flavor),
    AlreadyComplete,
    Held(f64),
    Unreachable,
    Failed(String),
}

#[derive(Debug, Default, Deserialize)]
struct CatchupState {
    #[serde(default)]
    entries: HashMap<String, Value>,
}

#[derive(Debug)]
struct CatchupEntry {
    active: Option<Value>,
    next_retry_at: Option<Value>,
    fingerprint: Option<String>,
}

/// Run with the real socket transport and the host's IANA local zone.
pub fn run_cli(args: &[String], journal_path: &Path) -> CliRun {
    // Failure to identify a display zone must not prevent reprocessing.
    let zone = iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| Tz::from_str(&name).ok())
        .unwrap_or(chrono_tz::UTC);
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
            return success(range_plan(&days));
        }
        return run_from_scratch_range(journal_path, &days, now, zone, &mut transport);
    }

    render_day_outcome(
        &parsed.day,
        reprocess_day(
            journal_path,
            &parsed.day,
            parsed.flavor,
            now,
            zone,
            &mut transport,
        ),
        now,
        zone,
    )
}

fn parse_arguments(args: &[String]) -> Result<ParsedArgs, ParseResult> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Err(ParseResult::Help);
    }
    let mut day = None;
    let mut through = None;
    let mut yes = false;
    let mut flavor = Flavor::ProcessNow;
    let mut flavor_flag: Option<&str> = None;
    let mut unknown = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        match argument.as_str() {
            "-v" | "--verbose" | "-d" | "--debug" => {}
            "--yes" => yes = true,
            "--through" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(ParseResult::Usage(
                        "argument --through: expected one argument".to_owned(),
                    ));
                };
                through = Some(value.clone());
            }
            "--from-scratch" | "--mark-updated" => {
                if let Some(previous) = flavor_flag {
                    return Err(ParseResult::Usage(format!(
                        "argument {argument}: not allowed with argument {previous}"
                    )));
                }
                flavor_flag = Some(argument);
                flavor = if argument == "--from-scratch" {
                    Flavor::FromScratch
                } else {
                    Flavor::MarkUpdated
                };
            }
            _ if argument.starts_with('-') => unknown.push(argument.clone()),
            _ if day.is_none() => day = Some(argument.clone()),
            _ => unknown.push(argument.clone()),
        }
        index += 1;
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

fn reprocess_day<F>(
    journal: &Path,
    day: &str,
    flavor: Flavor,
    now: DateTime<Utc>,
    zone: Tz,
    transport: &mut F,
) -> DayOutcome
where
    F: FnMut(&CallosumEnvelope) -> bool,
{
    let Some(parsed) = parse_day(journal, day) else {
        return DayOutcome::Malformed;
    };
    if parsed >= now.with_timezone(&zone).date_naive() {
        return DayOutcome::PastOnly;
    }
    let Ok(day_directory) = day_path(journal, Some(day), false) else {
        return DayOutcome::Malformed;
    };
    let has_data = day_directory.is_dir()
        && iter_segments(journal, PathOrDay::Day(day)).is_ok_and(|segments| !segments.is_empty());
    if !has_data {
        return DayOutcome::NoData;
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
        Ok(true) => return DayOutcome::AlreadyComplete,
        Ok(false) => {}
        Err(error) => return DayOutcome::Failed(error.to_string()),
    }
    if let Some(retry_at) = read_drain_hold_retry_at(journal, day, now) {
        return DayOutcome::Held(retry_at);
    }
    if transport(&drain_envelope(day)) {
        DayOutcome::Submitted(flavor)
    } else {
        DayOutcome::Unreachable
    }
}

fn render_day_outcome(day: &str, outcome: DayOutcome, now: DateTime<Utc>, zone: Tz) -> CliRun {
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
        DayOutcome::AlreadyComplete => success(format!(
            "day {day} already complete; use --from-scratch to force a full re-run\n"
        )),
        DayOutcome::Held(retry_at) => success(format!(
            "day {day} is held until {}; use --from-scratch to start it over now\n",
            format_retry_when(retry_at, now, zone)
        )),
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
        let has_iter_segments_data = day_path(journal, Some(&day), false)
            .is_ok_and(|path| path.is_dir())
            && !segments.is_empty();
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
        "from-scratch reprocess plan:\n{count} days with data ({} segments) will be queued. Progress will be visible in journal top or journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n",
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
        match reprocess_day(
            journal,
            &entry.day,
            Flavor::FromScratch,
            now,
            zone,
            transport,
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
            other => return render_day_outcome(&entry.day, other, now, zone),
        }
    }
    success(format!(
        "queued from-scratch reprocess for {} days ({} segments)\nprogress is visible in journal top or journal health\nqueued days do not survive a supervisor restart\n",
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

fn parse_day(journal: &Path, day: &str) -> Option<NaiveDate> {
    day_path(journal, Some(day), false).ok()?;
    NaiveDate::parse_from_str(day, "%Y%m%d").ok()
}

fn request_envelope(day: &str) -> CallosumEnvelope {
    let mut extra = Map::new();
    extra.insert(
        "cmd".to_owned(),
        json!(["journal", "think", "-v", "--day", day, "--from-scratch"]),
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

fn read_drain_hold_retry_at(journal: &Path, day: &str, now: DateTime<Utc>) -> Option<f64> {
    let state = std::fs::read(journal.join("health").join("catchup-state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CatchupState>(&bytes).ok())
        .unwrap_or_default();
    let records = ["daily-catchup", "segment-repair"]
        .into_iter()
        .filter_map(|kind| state.entries.get(&format!("{day}:{kind}")))
        .filter_map(catchup_entry)
        .collect::<Vec<_>>();
    // Python uses `record.get("active")`: null, false, 0, empty strings, and
    // empty containers are inactive; other values are active.
    if records
        .iter()
        .any(|record| record.active.as_ref().is_some_and(python_truthy))
    {
        return None;
    }
    let now_seconds = now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1e9;
    let candidates = records
        .iter()
        .filter_map(|record| {
            let retry_at = record.next_retry_at.as_ref()?.as_f64()?;
            (now_seconds < retry_at).then_some((record, retry_at))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }
    let fingerprint = read_raw_input_fingerprint(journal, day).ok()?;
    candidates
        .into_iter()
        .filter_map(|(record, retry_at)| {
            (record.fingerprint.as_deref() == Some(fingerprint.as_str())).then_some(retry_at)
        })
        .max_by(f64::total_cmp)
}

fn catchup_entry(value: &Value) -> Option<CatchupEntry> {
    let object = value.as_object()?;
    Some(CatchupEntry {
        active: object.get("active").cloned(),
        next_retry_at: object.get("next_retry_at").cloned(),
        fingerprint: object
            .get("fingerprint")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn format_retry_when(retry_at: f64, now: DateTime<Utc>, zone: Tz) -> String {
    let retry = DateTime::from_timestamp(retry_at as i64, 0)
        .unwrap_or(now)
        .with_timezone(&zone);
    let today = now.with_timezone(&zone).date_naive();
    let retry_day = retry.date_naive();
    let label = if retry_day == today {
        "today".to_owned()
    } else if today
        .succ_opt()
        .is_some_and(|tomorrow| retry_day == tomorrow)
    {
        "tomorrow".to_owned()
    } else {
        format!(
            "{} {}",
            retry.format("%b").to_string().to_lowercase(),
            retry.day()
        )
    };
    format!(
        "{label} at {}",
        retry
            .format("%I:%M%p")
            .to_string()
            .trim_start_matches('0')
            .to_lowercase()
    )
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
    format!("{}\n", block.lines().take(3).collect::<Vec<_>>().join("\n"))
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
        stderr: format!("{}journal reprocess: error: {message}\n", reprocess_usage()),
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
    const HELP: &str = "usage: journal reprocess [-h] [--through THROUGH] [--yes] [--from-scratch |\n                         --mark-updated] [-v] [-d]\n                         day\n\nSubmit a past journal day for reprocessing\n\npositional arguments:\n  day                Past day in YYYYMMDD format\n\noptions:\n  -h, --help         show this help message and exit\n  --through THROUGH  Inclusive range end in YYYYMMDD format\n  --yes\n  --from-scratch     Force a full daily re-run, preserving markers (does not\n                     flag the day as updated)\n  --mark-updated     Flag the day as having new raw data so daily processing\n                     re-queues it, then nudge a drain\n  -v, --verbose      Enable verbose output\n  -d, --debug        Enable debug logging\n";
    const MISSING_DAY_STDERR: &str = "usage: journal reprocess [-h] [--through THROUGH] [--yes] [--from-scratch |\n                         --mark-updated] [-v] [-d]\n                         day\njournal reprocess: error: the following arguments are required: day\n";

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
                "{}journal reprocess: error: unrecognized arguments: --nonsense\n",
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
                    "{}journal reprocess: error: argument {second}: not allowed with argument {first}\n",
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
            "{\"tract\":\"supervisor\",\"event\":\"request\",\"cmd\":[\"journal\",\"think\",\"-v\",\"--day\",\"20260101\",\"--from-scratch\"],\"day\":\"20260101\",\"queue_if_active_cmd_differs\":true}\n"
        );
    }

    #[test]
    fn mark_updated_touches_before_failed_transport() {
        let root = TempDir::new().unwrap();
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
    fn complete_day_sends_nothing() {
        let root = TempDir::new().unwrap();
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
        assert_eq!(calls, 0);
    }

    #[test]
    fn range_preview_and_divergent_scan_count_send_nothing() {
        let root = TempDir::new().unwrap();
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
            "from-scratch reprocess plan:\n1 days with data (0 segments) will be queued. Progress will be visible in journal top or journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n"
        );
    }

    #[test]
    fn empty_day_directory_is_no_data_singly_and_skipped_in_range() {
        let root = TempDir::new().unwrap();
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
            "from-scratch reprocess plan:\n1 days with data (1 segments) will be queued. Progress will be visible in journal top or journal health. Queued days do not survive a supervisor restart.\nThese days run one at a time and can take hours; today's own journal processing waits until the whole range finishes.\nre-run with --yes to proceed\n"
        );
    }

    #[test]
    fn range_partial_failure_counts_only_data_days() {
        let root = TempDir::new().unwrap();
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
            "queued from-scratch reprocess for 2 days (2 segments)\nprogress is visible in journal top or journal health\nqueued days do not survive a supervisor restart\n"
        );
    }

    #[test]
    fn catchup_hold_observes_active_retry_fingerprint_and_both_kinds() {
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "raw\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        let fingerprint = read_raw_input_fingerprint(root.path(), DAY).unwrap();
        let state_path = root.path().join("health/catchup-state.json");
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        let daily_key = format!("{DAY}:daily-catchup");
        let segment_key = format!("{DAY}:segment-repair");
        let early_retry = now().timestamp() as f64 + 3_600.0;
        let later_retry = now().timestamp() as f64 + 10_800.0;

        let write_entries = |entries: Value| {
            fs::write(
                &state_path,
                serde_json::to_vec(&json!({
                    "version": 1,
                    "entries": entries,
                }))
                .unwrap(),
            )
            .unwrap();
        };
        let held = |calls: &mut usize| {
            run_cli_with(&words(&[DAY]), root.path(), now(), chrono_tz::UTC, |_| {
                *calls += 1;
                true
            })
        };

        write_entries(json!({
            daily_key.clone(): {
                "active": {"ref": "x"},
                "next_retry_at": later_retry,
                "fingerprint": fingerprint.clone(),
            },
            segment_key.clone(): {
                "active": null,
                "next_retry_at": later_retry,
                "fingerprint": fingerprint.clone(),
            },
        }));
        let mut calls = 0;
        assert_eq!(
            held(&mut calls).stdout,
            "reprocess (process-now) submitted for 20260101\n"
        );
        assert_eq!(calls, 1);

        write_entries(json!({
            daily_key.clone(): {
                "active": null,
                "next_retry_at": early_retry,
                "fingerprint": fingerprint.clone(),
            },
            segment_key.clone(): {
                "active": null,
                "next_retry_at": later_retry,
                "fingerprint": fingerprint.clone(),
            },
        }));
        calls = 0;
        assert_eq!(
            held(&mut calls).stdout,
            "day 20260101 is held until today at 3:00pm; use --from-scratch to start it over now\n"
        );
        assert_eq!(calls, 0);

        write_entries(json!({
            daily_key.clone(): {
                "active": null,
                "next_retry_at": now().timestamp() as f64 - 1.0,
                "fingerprint": fingerprint.clone(),
            },
        }));
        calls = 0;
        assert!(
            held(&mut calls)
                .stdout
                .starts_with("reprocess (process-now) submitted")
        );
        assert_eq!(calls, 1);

        write_entries(json!({
            daily_key.clone(): {
                "active": null,
                "next_retry_at": later_retry,
                "fingerprint": "stale",
            },
        }));
        calls = 0;
        assert_eq!(held(&mut calls).exit_code, 0);
        assert_eq!(calls, 1);

        write_entries(json!({
            daily_key.clone(): {
                "active": null,
                "next_retry_at": "bad",
                "fingerprint": fingerprint.clone(),
            },
        }));
        calls = 0;
        assert_eq!(held(&mut calls).exit_code, 0);
        assert_eq!(calls, 1);

        write_entries(json!({
            daily_key: {
                "active": null,
                "next_retry_at": later_retry,
            },
        }));
        calls = 0;
        assert_eq!(held(&mut calls).exit_code, 0);
        assert_eq!(calls, 1);

        fs::write(&state_path, b"{").unwrap();
        calls = 0;
        assert_eq!(held(&mut calls).exit_code, 0);
        assert_eq!(calls, 1);
    }

    #[test]
    fn catchup_garbage_record_does_not_discard_a_valid_hold() {
        let root = TempDir::new().unwrap();
        fs::write(
            segment(root.path(), DAY, "090000_60").join("audio.jsonl"),
            "raw\n",
        )
        .unwrap();
        let health = root.path().join("chronicle").join(DAY).join("health");
        fs::create_dir_all(&health).unwrap();
        fs::write(health.join("stream.updated"), "").unwrap();
        let fingerprint = read_raw_input_fingerprint(root.path(), DAY).unwrap();
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
        assert_eq!(calls, 0);
        assert_eq!(
            result.stdout,
            "day 20260101 is held until today at 1:00pm; use --from-scratch to start it over now\n"
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
    fn retry_formatter_uses_zone_and_python_clock_style() {
        let zone = chrono_tz::America::Denver;
        let now = zone
            .with_ymd_and_hms(2026, 1, 3, 0, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let today = zone
            .with_ymd_and_hms(2026, 1, 3, 3, 15, 0)
            .unwrap()
            .timestamp() as f64;
        let tomorrow = zone
            .with_ymd_and_hms(2026, 1, 4, 12, 0, 0)
            .unwrap()
            .timestamp() as f64;
        let later = zone
            .with_ymd_and_hms(2026, 1, 5, 0, 0, 0)
            .unwrap()
            .timestamp() as f64;
        assert_eq!(format_retry_when(today, now, zone), "today at 3:15am");
        assert_eq!(
            format_retry_when(tomorrow, now, zone),
            "tomorrow at 12:00pm"
        );
        assert_eq!(format_retry_when(later, now, zone), "jan 5 at 12:00am");
    }

    #[test]
    fn invalid_calendar_day_and_drain_shape_are_distinct() {
        let root = TempDir::new().unwrap();
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
}
