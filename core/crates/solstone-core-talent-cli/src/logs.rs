// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::Value;

use crate::CliRun;
use crate::args::LogsOptions;
use crate::runs;
use solstone_core_talent_config::TalentConfig;

#[derive(Default)]
struct RunStats {
    event_count: usize,
    tool_count: usize,
    model: Option<String>,
    usage: Option<Value>,
    request: Option<Value>,
}

pub(crate) fn run_logs(
    journal_root: &Path,
    options: &LogsOptions,
    now: SystemTime,
    stdout_is_tty: bool,
    load_configs: &mut dyn FnMut() -> Result<Vec<TalentConfig>, String>,
) -> CliRun {
    let talents_dir = journal_root.join("talents");
    if !talents_dir.is_dir() {
        return success(String::new());
    }
    if let Some(day) = &options.day
        && !valid_day(day)
    {
        return failure(format!("Invalid --day format: {day}. Expected YYYYMMDD.\n"));
    }

    let count = resolved_count(options);
    let mut records = match collect_records(&talents_dir, options, count, load_configs) {
        Ok(records) => records,
        Err(error) => return failure(format!("{error}\n")),
    };
    if records.is_empty() {
        return success(String::new());
    }
    records.sort_by_key(|record| std::cmp::Reverse(timestamp(record)));
    truncate_python_prefix(&mut records, count);

    if options.summary {
        return success(render_summary(&records));
    }
    success(render_table(
        &talents_dir,
        journal_root,
        &records,
        now,
        stdout_is_tty,
    ))
}

fn collect_records(
    talents_dir: &Path,
    options: &LogsOptions,
    count: i64,
    load_configs: &mut dyn FnMut() -> Result<Vec<TalentConfig>, String>,
) -> Result<Vec<Value>, String> {
    let day_files = day_files(talents_dir, options.day.as_deref())?;
    let mut records = Vec::new();
    let mut schedule_lookup: Option<HashMap<String, Option<String>>> = None;

    for day_file in day_files {
        let text = fs::read_to_string(&day_file)
            .map_err(|error| format!("failed to read {}: {error}", day_file.display()))?;
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(object) = record.as_object() else {
                continue;
            };
            if options
                .agent
                .as_deref()
                .is_some_and(|agent| object.get("name").and_then(Value::as_str) != Some(agent))
            {
                continue;
            }
            if options.errors && object.get("status").and_then(Value::as_str) != Some("error") {
                continue;
            }
            if options.daily {
                let mut schedule = object.get("schedule").and_then(Value::as_str);
                if schedule.is_none() {
                    if schedule_lookup.is_none() {
                        let configs = load_configs()?;
                        schedule_lookup = Some(
                            configs
                                .into_iter()
                                .map(|config| {
                                    (
                                        config.key,
                                        config
                                            .metadata
                                            .get("schedule")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                    )
                                })
                                .collect(),
                        );
                    }
                    schedule = schedule_lookup
                        .as_ref()
                        .and_then(|lookup| lookup.get(object.get("name").and_then(Value::as_str)?))
                        .and_then(|schedule| schedule.as_deref());
                }
                if schedule != Some("daily") {
                    continue;
                }
            }
            records.push(record);
        }
        if records.len() as i64 >= count {
            break;
        }
    }
    Ok(records)
}

fn day_files(talents_dir: &Path, day: Option<&str>) -> Result<Vec<PathBuf>, String> {
    if let Some(day) = day {
        let path = talents_dir.join(format!("{day}.jsonl"));
        return Ok(path.is_file().then_some(path).into_iter().collect());
    }
    let mut files = fs::read_dir(talents_dir)
        .map_err(|error| format!("failed to read {}: {error}", talents_dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() == 14
                        && name.ends_with(".jsonl")
                        && name[..8].bytes().all(|byte| byte.is_ascii_digit())
                })
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    Ok(files)
}

fn valid_day(day: &str) -> bool {
    day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit())
}

fn resolved_count(options: &LogsOptions) -> i64 {
    options.count.unwrap_or(if options.daily { 50 } else { 20 })
}

fn truncate_python_prefix(records: &mut Vec<Value>, count: i64) {
    let end = if count >= 0 {
        usize::try_from(count)
            .unwrap_or(usize::MAX)
            .min(records.len())
    } else {
        records
            .len()
            .saturating_sub(usize::try_from(count.unsigned_abs()).unwrap_or(usize::MAX))
    };
    records.truncate(end);
}

fn timestamp(record: &Value) -> i64 {
    record.get("ts").and_then(Value::as_i64).unwrap_or(0)
}

fn parse_run_stats(path: &Path) -> RunStats {
    let mut stats = RunStats::default();
    let Ok(text) = fs::read_to_string(path) else {
        return stats;
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(event) = event.as_object() else {
            continue;
        };
        let event_type = event.get("event").and_then(Value::as_str);
        if event_type == Some("request") {
            stats.request = Some(Value::Object(event.clone()));
            continue;
        }
        stats.event_count += 1;
        match event_type {
            Some("tool_start") => stats.tool_count += 1,
            Some("start") => {
                stats.model = event
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }
            Some("finish") => stats.usage = event.get("usage").cloned(),
            _ => {}
        }
    }
    stats
}

fn output_size(request: &Value, journal_root: &Path) -> Option<u64> {
    let path = output_path(request, journal_root)?;
    if path.is_file() {
        fs::metadata(path).ok().map(|metadata| metadata.len())
    } else {
        None
    }
}

fn output_path(request: &Value, journal_root: &Path) -> Option<PathBuf> {
    let output_format = request.get("output")?.as_str()?;
    if output_format.is_empty() {
        return None;
    }
    if let Some(path) = request.get("output_path").and_then(Value::as_str) {
        return Some(PathBuf::from(path));
    }
    let day = request.get("day")?.as_str()?;
    let name = request.get("name")?.as_str()?;
    let filename = format!(
        "{}.{}",
        output_name(name),
        if output_format == "json" {
            "json"
        } else {
            "md"
        }
    );
    let day_dir = journal_root.join("chronicle").join(day);
    let facet = request.get("facet").and_then(Value::as_str);
    if let Some(segment) = request.get("segment").and_then(Value::as_str) {
        let stream = request
            .get("env")
            .and_then(Value::as_object)
            .and_then(|env| env.get("SOL_STREAM"))
            .and_then(Value::as_str);
        let segment_dir = stream.map_or_else(
            || day_dir.join(segment),
            |stream| day_dir.join(stream).join(segment),
        );
        return Some(match facet {
            Some(facet) => segment_dir.join("talents").join(facet).join(filename),
            None => segment_dir.join("talents").join(filename),
        });
    }
    Some(match facet {
        Some(facet) => day_dir.join("talents").join(facet).join(filename),
        None => day_dir.join("talents").join(filename),
    })
}

fn output_name(key: &str) -> String {
    match key.split_once(':') {
        Some((app, name)) => format!("_{app}_{name}"),
        None => key.to_owned(),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        bytes.to_string()
    } else if bytes < 1_000_000 {
        format!("{:.1}K", bytes as f64 / 1_000.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1_000_000.0)
    }
}

fn format_cost(cost_usd: Option<f64>) -> String {
    let Some(cost_usd) = cost_usd else {
        return "-".to_owned();
    };
    let cents = (cost_usd * 100.0).round_ties_even() as i64;
    if cents == 0 && cost_usd > 0.0 {
        return "<1¢".to_owned();
    }
    format!("{cents}¢")
}

// genai-prices ships its price table as ~494 KB of generated Python source that a
// shipped Rust artifact cannot read or execute. A bundled snapshot would silently
// drift. The reference renders "-" when pricing is unavailable, so returning None
// preserves the output shape.
fn agent_cost_usd(model: Option<&str>, usage: Option<&Value>) -> Option<f64> {
    let _ = (model, usage);
    None
}

fn format_runtime(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        let minutes = (seconds / 60.0) as i64;
        let remainder = (seconds % 60.0) as i64;
        format!("{minutes}m {remainder:02}s")
    }
}

fn time_column(record: &Value, now: SystemTime) -> String {
    let timestamp = Local
        .timestamp_millis_opt(timestamp(record))
        .single()
        .unwrap_or_else(|| DateTime::<Utc>::UNIX_EPOCH.with_timezone(&Local));
    let today = DateTime::<Utc>::from(now)
        .with_timezone(&Local)
        .format("%Y%m%d")
        .to_string();
    let day = record
        .get("day")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| timestamp.format("%Y%m%d").to_string());
    if day == today {
        timestamp.format("%H:%M").to_string()
    } else {
        timestamp.format("%b %d %H:%M").to_string()
    }
}

fn render_table(
    talents_dir: &Path,
    journal_root: &Path,
    records: &[Value],
    now: SystemTime,
    stdout_is_tty: bool,
) -> String {
    let name_width = records
        .iter()
        .filter_map(|record| record.get("name").and_then(Value::as_str))
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(10)
        .max(10);
    let mut output = String::new();
    for record in records {
        let use_id = record
            .get("use_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let run_file = runs::find_run_file(talents_dir, use_id);
        let stats = run_file.as_deref().map(parse_run_stats).unwrap_or_default();
        let model = stats
            .model
            .as_deref()
            .or_else(|| record.get("model").and_then(Value::as_str));
        let cost = agent_cost_usd(model, stats.usage.as_ref());
        let output_size = stats
            .request
            .as_ref()
            .and_then(|request| output_size(request, journal_root));
        let name = record
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let status = record
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status_symbol = if status == "completed" { '✓' } else { '✗' };
        let runtime = record
            .get("runtime_seconds")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let model = record
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let facet = record
            .get("facet")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let cost = if run_file.is_some() {
            format_cost(cost)
        } else {
            "-".to_owned()
        };
        let events = if run_file.is_some() {
            stats.event_count.to_string()
        } else {
            "-".to_owned()
        };
        let tools = if run_file.is_some() {
            stats.tool_count.to_string()
        } else {
            "-".to_owned()
        };
        let output_size = output_size.map_or_else(|| "-".to_owned(), format_bytes);
        let facet_part = if facet.is_empty() {
            String::new()
        } else {
            format!("  {facet}")
        };
        let mut line = format!(
            "{use_id:<15}{:>12}  {name:<name_width$}  {status_symbol}  {:>7}  {cost:>4}  {events:>3}  {tools:>3}  {output_size:>5}  {model}{facet_part}",
            time_column(record, now),
            format_runtime(runtime),
        );
        if stdout_is_tty && status != "completed" {
            line = format!("\x1b[31m{line}\x1b[0m");
        }
        output.push_str(&line);
        output.push('\n');
    }
    output
}

fn render_summary(records: &[Value]) -> String {
    let mut groups = BTreeMap::<String, Vec<&Value>>::new();
    for record in records {
        groups
            .entry(
                record
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
            )
            .or_default()
            .push(record);
    }
    let mut total_pass = 0usize;
    let mut total_fail = 0usize;
    let mut total_runtime = 0.0;
    let mut output = String::new();
    for (name, runs) in groups {
        let passed = runs
            .iter()
            .filter(|record| record.get("status").and_then(Value::as_str) == Some("completed"))
            .count();
        let failed = runs.len() - passed;
        let runtimes = runs
            .iter()
            .map(|record| {
                record
                    .get("runtime_seconds")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            })
            .collect::<Vec<_>>();
        let min_runtime = runtimes.iter().copied().fold(f64::INFINITY, f64::min);
        let max_runtime = runtimes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let runtime_total = runtimes.iter().sum::<f64>();
        total_pass += passed;
        total_fail += failed;
        total_runtime += runtime_total;
        let runtime = if min_runtime == max_runtime {
            format!("{min_runtime:.1}s")
        } else {
            format!("{min_runtime:.1}s–{max_runtime:.1}s")
        };
        let mut status = format!("{passed}✓");
        if failed > 0 {
            status.push_str(&format!(" {failed}✗"));
        }
        output.push_str(&format!("  {name:<20} {status:<10} {runtime}\n"));
    }
    output.push_str(&format!("  {}\n", "—".repeat(40)));
    let mut status = format!("{total_pass}✓");
    if total_fail > 0 {
        status.push_str(&format!(" {total_fail}✗"));
    }
    output.push_str(&format!(
        "  {:<20} {status:<10} {total_runtime:.1}s\n",
        "total"
    ));
    output
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(stderr: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr,
        exit_code: 1,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::time::SystemTime;

    use chrono::{Local, TimeZone, Utc};
    use serde_json::{Map, Value, json};

    use super::*;

    fn options() -> LogsOptions {
        LogsOptions::default()
    }

    fn local_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> SystemTime {
        SystemTime::from(
            Local
                .with_ymd_and_hms(year, month, day, hour, minute, 0)
                .single()
                .expect("unambiguous local time")
                .with_timezone(&Utc),
        )
    }

    fn local_timestamp(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("unambiguous local time")
            .timestamp_millis()
    }

    fn write_day(root: &Path, day: &str, records: &[Value]) {
        let talents = root.join("talents");
        fs::create_dir_all(&talents).expect("talents directory");
        let text = records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(talents.join(format!("{day}.jsonl")), format!("{text}\n")).expect("day index");
    }

    fn record(use_id: &str, name: &str, day: &str, ts: i64, status: &str, runtime: f64) -> Value {
        json!({
            "use_id": use_id,
            "name": name,
            "day": day,
            "ts": ts,
            "status": status,
            "runtime_seconds": runtime,
            "model": "m",
        })
    }

    fn run(root: &Path, options: &LogsOptions) -> CliRun {
        run_logs(root, options, SystemTime::UNIX_EPOCH, false, &mut || {
            Ok(Vec::new())
        })
    }

    #[test]
    fn early_stop_happens_before_global_sort() {
        let root = tempfile::tempdir().expect("tempdir");
        write_day(
            root.path(),
            "20260102",
            &[
                record("newer-in-file", "demo", "20260102", 20, "completed", 1.0),
                record("older-in-file", "demo", "20260102", 10, "completed", 1.0),
            ],
        );
        write_day(
            root.path(),
            "20260101",
            &[record(
                "globally-newest",
                "demo",
                "20260101",
                30,
                "completed",
                1.0,
            )],
        );
        let output = run(
            root.path(),
            &LogsOptions {
                count: Some(1),
                ..options()
            },
        );
        assert!(output.stdout.contains("newer-in-file"));
        assert!(!output.stdout.contains("globally-newest"));
    }

    #[test]
    fn run_stats_exclude_request_events() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("run.jsonl");
        fs::write(
            &path,
            "{\"event\":\"request\"}\n{\"event\":\"start\",\"model\":\"m\"}\n{\"event\":\"thinking\"}\n{\"event\":\"tool_start\"}\n",
        )
        .expect("run file");
        let stats = parse_run_stats(&path);
        assert_eq!(stats.event_count, 3);
        assert_eq!(stats.tool_count, 1);
    }

    #[test]
    fn daily_fallback_is_lazy_raw_keyed_and_memoized() {
        let root = tempfile::tempdir().expect("tempdir");
        write_day(
            root.path(),
            "20260101",
            &[
                record("one", "app:daily", "20260101", 1, "completed", 1.0),
                record("two", "app:daily", "20260101", 2, "completed", 1.0),
            ],
        );
        let loads = Cell::new(0);
        let configs = vec![TalentConfig {
            key: "app:daily".to_owned(),
            file: "ignored".to_owned(),
            metadata: Map::from_iter([(
                String::from("schedule"),
                Value::String("daily".to_owned()),
            )]),
            body: String::new(),
        }];
        let output = run_logs(
            root.path(),
            &LogsOptions {
                daily: true,
                ..options()
            },
            SystemTime::UNIX_EPOCH,
            false,
            &mut || {
                loads.set(loads.get() + 1);
                Ok(configs.clone())
            },
        );
        assert_eq!(loads.get(), 1);
        assert_eq!(output.stdout.lines().count(), 2);

        let scheduled = tempfile::tempdir().expect("tempdir");
        let mut scheduled_record = record("three", "app:daily", "20260102", 3, "completed", 1.0);
        scheduled_record["schedule"] = json!("daily");
        write_day(scheduled.path(), "20260102", &[scheduled_record]);
        let scheduled_loads = Cell::new(0);
        let scheduled_output = run_logs(
            scheduled.path(),
            &LogsOptions {
                daily: true,
                ..options()
            },
            SystemTime::UNIX_EPOCH,
            false,
            &mut || {
                scheduled_loads.set(scheduled_loads.get() + 1);
                Ok(Vec::new())
            },
        );
        assert_eq!(scheduled_loads.get(), 0);
        assert_eq!(scheduled_output.stdout.lines().count(), 1);
    }

    #[test]
    fn colour_is_tty_conditional() {
        let root = tempfile::tempdir().expect("tempdir");
        write_day(
            root.path(),
            "20260101",
            &[record("id", "demo", "20260101", 1, "error", 1.0)],
        );
        let tty = run_logs(
            root.path(),
            &options(),
            SystemTime::UNIX_EPOCH,
            true,
            &mut || Ok(Vec::new()),
        );
        let plain = run(root.path(), &options());
        assert!(tty.stdout.contains("\x1b[31m"));
        assert!(tty.stdout.contains("\x1b[0m"));
        assert!(!plain.stdout.contains('\x1b'));
    }

    #[test]
    fn cost_narrowing_and_formatter_are_deliberate() {
        assert_eq!(agent_cost_usd(Some("m"), Some(&json!({}))), None);
        assert_eq!(format_cost(None), "-");
        assert_eq!(format_cost(Some(0.001)), "<1¢");
        assert_eq!(format_cost(Some(0.005)), "<1¢");
    }

    #[test]
    fn output_paths_cover_all_forms_and_override() {
        let root = Path::new("journal");
        let request = |fields: Value| {
            Value::Object(
                Map::from_iter([
                    ("name".to_owned(), Value::String("app:demo".to_owned())),
                    ("day".to_owned(), Value::String("20260101".to_owned())),
                    ("output".to_owned(), Value::String("json".to_owned())),
                ])
                .into_iter()
                .chain(fields.as_object().unwrap().clone())
                .collect(),
            )
        };
        assert_eq!(
            output_path(&request(json!({})), root),
            Some(root.join("chronicle/20260101/talents/_app_demo.json"))
        );
        assert_eq!(
            output_path(&request(json!({"facet":"work", "output":"md"})), root),
            Some(root.join("chronicle/20260101/talents/work/_app_demo.md"))
        );
        assert_eq!(
            output_path(
                &request(json!({"segment":"s", "env":{"SOL_STREAM":"screen"}})),
                root
            ),
            Some(root.join("chronicle/20260101/screen/s/talents/_app_demo.json"))
        );
        assert_eq!(
            output_path(&request(json!({"segment":"s", "facet":"work"})), root),
            Some(root.join("chronicle/20260101/s/talents/work/_app_demo.json"))
        );
        assert_eq!(
            output_path(&request(json!({"output_path":"custom/out"})), root),
            Some(PathBuf::from("custom/out"))
        );
        assert_eq!(output_path(&request(json!({"output":""})), root), None);

        let temporary = tempfile::tempdir().expect("tempdir");
        let output = temporary.path().join("output.json");
        fs::write(&output, "bytes").expect("output file");
        assert_eq!(
            output_size(&request(json!({"output_path": output})), root),
            Some(5)
        );
    }

    #[test]
    fn time_column_uses_injected_local_today() {
        let now = local_time(2026, 8, 7, 12, 0);
        let today = record(
            "today",
            "demo",
            "20260807",
            local_timestamp(2026, 8, 7, 1, 6),
            "completed",
            1.0,
        );
        let other = record(
            "other",
            "demo",
            "20260805",
            local_timestamp(2026, 8, 6, 1, 6),
            "completed",
            1.0,
        );
        assert_eq!(time_column(&today, now), "01:06");
        assert_eq!(time_column(&other, now), "Aug 06 01:06");
    }

    #[test]
    fn table_name_width_uses_code_points() {
        let unicode_name = "é".repeat(10);
        let records = vec![
            record("unicode", &unicode_name, "19700101", 0, "completed", 1.0),
            record("ascii", "ascii", "19700101", 0, "completed", 1.0),
        ];
        let output = render_table(
            Path::new("missing"),
            Path::new("journal"),
            &records,
            SystemTime::UNIX_EPOCH,
            false,
        );
        let mut lines = output.lines();
        assert!(
            lines
                .next()
                .expect("unicode row")
                .contains(&format!("{unicode_name}  ✓"))
        );
        assert!(lines.next().expect("ascii row").contains("ascii       ✓"));
    }

    #[test]
    fn empty_states_are_silent() {
        let missing = tempfile::tempdir().expect("tempdir");
        assert_eq!(run(missing.path(), &options()), success(String::new()));
        let no_days = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(no_days.path().join("talents")).expect("talents");
        assert_eq!(run(no_days.path(), &options()), success(String::new()));
        write_day(
            no_days.path(),
            "20260101",
            &[record("id", "other", "20260101", 1, "completed", 1.0)],
        );
        assert_eq!(
            run(
                no_days.path(),
                &LogsOptions {
                    agent: Some("none".to_owned()),
                    ..options()
                }
            ),
            success(String::new())
        );
    }

    #[test]
    fn invalid_day_respects_directory_check_order() {
        let missing = tempfile::tempdir().expect("tempdir");
        let invalid_options = LogsOptions {
            day: Some("bad".to_owned()),
            ..options()
        };
        assert_eq!(
            run(missing.path(), &invalid_options),
            success(String::new())
        );
        fs::create_dir_all(missing.path().join("talents")).expect("talents");
        assert_eq!(
            run(missing.path(), &invalid_options),
            failure("Invalid --day format: bad. Expected YYYYMMDD.\n".to_owned())
        );
        for day in ["1234567", "1234567x"] {
            assert_eq!(
                run(
                    missing.path(),
                    &LogsOptions {
                        day: Some(day.to_owned()),
                        ..options()
                    }
                ),
                failure(format!("Invalid --day format: {day}. Expected YYYYMMDD.\n"))
            );
        }
    }

    #[test]
    fn defaults_are_twenty_or_fifty() {
        assert_eq!(resolved_count(&options()), 20);
        assert_eq!(
            resolved_count(&LogsOptions {
                daily: true,
                ..options()
            }),
            50
        );
    }

    #[test]
    fn summary_groups_ranges_and_failures() {
        let records = vec![
            record("one", "demo", "20260101", 1, "completed", 12.5),
            record("two", "demo", "20260101", 2, "error", 95.0),
            record("three", "solo", "20260101", 3, "other", 2.0),
        ];
        let output = render_summary(&records);
        assert!(output.contains("12.5s–95.0s"));
        assert!(output.contains("solo                 0✓ 1✗      2.0s"));
        assert!(output.contains("total                1✓ 2✗      109.5s"));
    }

    #[test]
    fn table_oracle_bytes_are_timezone_robust() {
        let root = tempfile::tempdir().expect("tempdir");
        let timestamp = local_timestamp(2026, 8, 6, 1, 6);
        let mut first = record("9001", "demo", "20260805", timestamp, "completed", 12.5);
        first["model"] = json!("m-1");
        first["cost"] = json!(0.0031);
        let mut second = record("9002", "demo", "20260805", timestamp + 1, "error", 95.0);
        second["model"] = json!("m-2");
        second["facet"] = json!("work");
        write_day(root.path(), "20260805", &[first, second]);
        let talents = root.path().join("talents/demo");
        fs::create_dir_all(&talents).expect("run directory");
        fs::write(talents.join("9001.jsonl"), "{\"event\":\"request\",\"use_id\":\"9001\"}\n{\"event\":\"start\",\"model\":\"m-1\"}\n{\"event\":\"tool_start\"}\n{\"event\":\"thinking\"}\n{\"event\":\"thinking\"}\n{\"event\":\"thinking\"}\n{\"event\":\"thinking\"}\n{\"event\":\"thinking\"}\n{\"event\":\"thinking\"}\n{\"event\":\"finish\",\"usage\":{}}\n").expect("run file");
        fs::write(talents.join("9002.jsonl"), "{\"event\":\"request\",\"use_id\":\"9002\"}\n{\"event\":\"start\",\"model\":\"m-2\"}\n{\"event\":\"error\"}\n").expect("run file");
        let output = run_logs(
            root.path(),
            &options(),
            local_time(2026, 8, 7, 12, 0),
            false,
            &mut || Ok(Vec::new()),
        );
        assert_eq!(
            output.stdout,
            "9002           Aug 06 01:06  demo        ✗   1m 35s     -    2    0      -  m-2  work\n9001           Aug 06 01:06  demo        ✓    12.5s     -    9    1      -  m-1\n"
        );
    }

    #[test]
    fn summary_oracle_bytes_match() {
        let records = vec![
            record("9001", "demo", "20260805", 1, "completed", 12.5),
            record("9002", "demo", "20260805", 2, "error", 95.0),
        ];
        assert_eq!(
            render_summary(&records),
            "  demo                 1✓ 1✗      12.5s–95.0s\n  ————————————————————————————————————————\n  total                1✓ 1✗      107.5s\n"
        );
    }
}
