// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native timeline day and master rollups.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use solstone_core_generate::{ContentPart, GenerateRequest};
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use crate::timezone::{default_rollup_day, resolve_owner_timezone};
use crate::{CliRun, RollupPicker, TimelineServices};

const EXIT_EMPTY: i32 = 66;
const ROLLUP_CONTEXT: &str = "timeline.scratch.rollup";

#[derive(Clone)]
struct SegmentRow {
    segment: String,
    hour: String,
    title: String,
    description: String,
    origin: String,
}

struct PickResult {
    picks: Vec<Value>,
    rationale: String,
}

struct BatchResult {
    key: String,
    events: Vec<Value>,
    result: Result<PickResult, String>,
}

pub(crate) fn run(
    id: &str,
    args: &[String],
    journal: &Path,
    services: &TimelineServices<'_>,
) -> CliRun {
    match id {
        "timeline:rollup-day" => match parse_day_args(args) {
            Ok(options) => rollup_day(journal, options, services),
            Err(error) => usage_error(id, &error),
        },
        "timeline:rollup-master" => match parse_master_args(args) {
            Ok(options) => rollup_master(journal, options, services),
            Err(error) => usage_error(id, &error),
        },
        _ => usage_error(id, "unrecognized routine"),
    }
}

#[derive(Default)]
struct DayOptions {
    day: Option<String>,
    top: usize,
    jobs: usize,
    force: bool,
    dry_run: bool,
}

#[derive(Default)]
struct MasterOptions {
    top: usize,
    jobs: usize,
    force: bool,
    dry_run: bool,
    months: Option<BTreeSet<String>>,
}

fn parse_day_args(args: &[String]) -> Result<DayOptions, String> {
    let mut options = DayOptions {
        top: 4,
        jobs: 5,
        ..DayOptions::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--top" => {
                options.top = parse_usize(args.get(index + 1), "--top")?;
                index += 1;
            }
            "--jobs" => {
                options.jobs = parse_usize(args.get(index + 1), "--jobs")?;
                index += 1;
            }
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            value if value.starts_with('-') => {
                return Err(format!("unrecognized arguments: {value}"));
            }
            day if options.day.is_none() && is_day(day) => options.day = Some(day.to_owned()),
            _ => return Err("argument DAY: day must be YYYYMMDD".to_owned()),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_master_args(args: &[String]) -> Result<MasterOptions, String> {
    let mut options = MasterOptions {
        top: 4,
        jobs: 5,
        ..MasterOptions::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--top" => {
                options.top = parse_usize(args.get(index + 1), "--top")?;
                index += 1;
            }
            "--jobs" => {
                options.jobs = parse_usize(args.get(index + 1), "--jobs")?;
                index += 1;
            }
            "--months" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "argument --months: expected one argument".to_owned())?;
                let months = value
                    .split(',')
                    .map(str::trim)
                    .filter(|month| !month.is_empty())
                    .map(str::to_owned)
                    .collect::<BTreeSet<_>>();
                let invalid = months
                    .iter()
                    .filter(|month| !is_month(month))
                    .cloned()
                    .collect::<Vec<_>>();
                if !invalid.is_empty() {
                    return Err(format!(
                        "--months must be comma-separated YYYYMM values: {invalid:?}"
                    ));
                }
                options.months = (!months.is_empty()).then_some(months);
                index += 1;
            }
            "--force" => options.force = true,
            "--dry-run" => options.dry_run = true,
            value => return Err(format!("unrecognized arguments: {value}")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_usize(value: Option<&String>, name: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("argument {name}: expected one argument"))?;
    value
        .parse()
        .map_err(|_| format!("argument {name}: invalid int value: '{value}'"))
}

fn rollup_day(journal: &Path, options: DayOptions, services: &TimelineServices<'_>) -> CliRun {
    let day = match options.day {
        Some(day) => day,
        None => {
            let config = match read_config(journal) {
                Ok(config) => config,
                Err(error) => return failure(error),
            };
            default_rollup_day(
                services.now,
                resolve_owner_timezone(&config, services.host_timezone),
            )
            .replace('-', "")
        }
    };
    let out_path = journal.join("chronicle").join(&day).join("timeline.json");
    if out_path.exists() && !options.force && !options.dry_run {
        return success(format!(
            "  [skip] {day}: timeline.json already exists (use --force to overwrite)"
        ));
    }
    let segments = match load_day_segments(journal, &day) {
        Ok(segments) => segments,
        Err(error) => return failure(error),
    };
    if segments.is_empty() {
        return CliRun {
            stdout: format!("  [empty] {day}: no segment timeline.json found\n"),
            stderr: String::new(),
            exit_code: EXIT_EMPTY,
        };
    }
    let by_hour = group_by_hour(&segments);
    if options.dry_run {
        return success(format!(
            "\n== {day} ==  segments: {}  hours: {}",
            segments.len(),
            by_hour.len()
        ));
    }
    let jobs = by_hour
        .iter()
        .map(|(hour, rows)| BatchResultInput {
            key: hour.clone(),
            events: rows.iter().map(segment_event).collect(),
        })
        .collect::<Vec<_>>();
    let hour_results = match pick_batch(services.picker, jobs, options.top, "hour", options.jobs) {
        Ok(results) => results,
        Err(error) => return failure(error),
    };
    let mut stdout = String::new();
    let mut hours = Map::new();
    let mut hour_picks = Vec::new();
    for result in hour_results {
        let segment_count = result.events.len();
        match result.result {
            Ok(picked) => {
                hour_picks.extend(picked.picks.iter().cloned());
                hours.insert(
                    result.key,
                    json!({
                        "segment_count": segment_count,
                        "picks": picked.picks,
                        "rationale": picked.rationale,
                    }),
                );
            }
            Err(error) => {
                stdout.push_str(&format!(
                    "    [hour-err {}h] {}\n",
                    result.key,
                    truncate(&error, 120)
                ));
                hours.insert(
                    result.key,
                    json!({
                        "segment_count": segment_count,
                        "picks": [],
                        "rationale": "",
                        "error": error,
                    }),
                );
            }
        }
    }
    let (day_top, day_rationale) = if hour_picks.is_empty() {
        (Vec::new(), "no hour picks available".to_owned())
    } else if hour_picks.len() <= options.top {
        (
            hour_picks,
            "fewer than N hour-picks; returning all".to_owned(),
        )
    } else {
        match pick_one(services.picker, &hour_picks, options.top, "day") {
            Ok(result) => (result.picks, result.rationale),
            Err(error) => {
                stdout.push_str(&format!(
                    "  [day-err {day}] day-level rollup failed: {}\n  [day-err {day}] no timeline.json written; re-run will retry this day\n",
                    truncate(&error, 160)
                ));
                return CliRun {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                };
            }
        }
    };
    let config = match read_config(journal) {
        Ok(config) => config,
        Err(error) => return failure(error),
    };
    let model = match services.model_resolver.resolve(&config) {
        Ok(model) => model,
        Err(error) => return failure(error),
    };
    let payload = json!({
        "day": day,
        "model": model,
        "generated_at": services.now.timestamp(),
        "segment_count": segments.len(),
        "hour_count": hours.len(),
        "day_top": day_top,
        "day_rationale": day_rationale,
        "hours": hours,
    });
    if let Err(error) = write_json(&out_path, &payload) {
        return failure(error);
    }
    stdout.push_str(&format!(
        "  [ok {}] → {}\n",
        payload["day"],
        out_path.display()
    ));
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

fn rollup_master(
    journal: &Path,
    options: MasterOptions,
    services: &TimelineServices<'_>,
) -> CliRun {
    let out_path = journal.join("timeline.json");
    if out_path.exists() && !options.force && !options.dry_run {
        return success(format!(
            "  [skip] {}: already exists (use --force to overwrite)",
            out_path.display()
        ));
    }
    let day_rollups = match load_day_rollups(journal) {
        Ok(rollups) => rollups,
        Err(error) => return failure(error),
    };
    if day_rollups.is_empty() {
        return CliRun {
            stdout: format!(
                "  [empty] no day-level timeline.json found under {}/chronicle/*/\n",
                journal.display()
            ),
            stderr: String::new(),
            exit_code: EXIT_EMPTY,
        };
    }
    let mut by_month = group_by_month(&day_rollups);
    if let Some(filter) = &options.months {
        by_month.retain(|month, _| filter.contains(month));
        if by_month.is_empty() {
            return success(format!(
                "  [empty] no overlap between --months {:?} and journal",
                filter.iter().collect::<Vec<_>>()
            ));
        }
    }
    if options.dry_run {
        return success(format!(
            "\n== dry run ==\ndays with day-level timeline.json: {}\nmonths covered                   : {}",
            day_rollups.len(),
            by_month.len()
        ));
    }
    let jobs = by_month
        .iter()
        .filter_map(|(month, days)| {
            let events = days
                .iter()
                .flat_map(|day| {
                    day_rollups[day]
                        .get("day_top")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(event_value)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            (!events.is_empty()).then_some(BatchResultInput {
                key: month.clone(),
                events,
            })
        })
        .collect::<Vec<_>>();
    if jobs.is_empty() {
        return success("  [empty] no month candidates found".to_owned());
    }
    let config = match read_config(journal) {
        Ok(config) => config,
        Err(error) => return failure(error),
    };
    let model = match services.model_resolver.resolve(&config) {
        Ok(model) => model,
        Err(error) => return failure(error),
    };
    let month_results = match pick_batch(
        services.picker,
        jobs.clone(),
        options.top,
        "month",
        options.jobs,
    ) {
        Ok(results) => results,
        Err(error) => jobs
            .into_iter()
            .map(|job| BatchResult {
                key: job.key,
                events: job.events,
                result: Err(error.clone()),
            })
            .collect(),
    };
    let mut months = Map::new();
    let mut year_top = Vec::new();
    let mut stdout = String::new();
    for result in month_results {
        let days = by_month.get(&result.key).cloned().unwrap_or_default();
        let (month_top, month_rationale) = match result.result {
            Ok(picked) => (picked.picks, picked.rationale),
            Err(error) => {
                stdout.push_str(&format!(
                    "  [month-err {}] {}\n",
                    result.key,
                    truncate(&error, 120)
                ));
                (Vec::new(), format!("ERROR: {}", truncate(&error, 200)))
            }
        };
        if let Some(head) = month_top.first() {
            year_top.push(json!({
                "month": result.key,
                "title": string_field(head, "title"),
                "description": string_field(head, "description"),
                "origin": string_field(head, "origin"),
            }));
        }
        let embedded_days = days
            .iter()
            .map(|day| (day.clone(), day_rollups[day].clone()))
            .collect::<Map<_, _>>();
        months.insert(
            result.key,
            json!({
                "month_top": month_top,
                "month_rationale": month_rationale,
                "day_count": days.len(),
                "days": embedded_days,
            }),
        );
    }
    let payload = json!({
        "generated_at": services.now.timestamp(),
        "model": model,
        "top_n": options.top,
        "year_top": year_top,
        "months": months,
    });
    if let Err(error) = write_json(&out_path, &payload) {
        return failure(error);
    }
    stdout.push_str(&format!("\n[ok] wrote {}\n", out_path.display()));
    CliRun {
        stdout,
        stderr: String::new(),
        exit_code: 0,
    }
}

#[derive(Clone)]
struct BatchResultInput {
    key: String,
    events: Vec<Value>,
}

fn pick_batch(
    picker: &dyn RollupPicker,
    jobs: Vec<BatchResultInput>,
    n: usize,
    scope_label: &str,
    max_concurrent: usize,
) -> Result<Vec<BatchResult>, String> {
    if max_concurrent == 0 {
        return Err("max_concurrent must be positive".to_owned());
    }
    let mut out = Vec::with_capacity(jobs.len());
    for chunk in jobs.chunks(max_concurrent) {
        let mut completed = Vec::with_capacity(chunk.len());
        std::thread::scope(|scope| {
            let handles = chunk
                .iter()
                .cloned()
                .map(|job| {
                    scope.spawn(move || BatchResult {
                        key: job.key,
                        result: pick_one(picker, &job.events, n, scope_label),
                        events: job.events,
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                completed.push(
                    handle
                        .join()
                        .map_err(|_| "timeline rollup worker panicked".to_owned()),
                );
            }
        });
        for result in completed {
            out.push(result?);
        }
    }
    Ok(out)
}

fn pick_one(
    picker: &dyn RollupPicker,
    events: &[Value],
    n: usize,
    scope_label: &str,
) -> Result<PickResult, String> {
    if events.len() <= n {
        return Ok(PickResult {
            picks: events.to_vec(),
            rationale: "fewer than N candidates; returning all".to_owned(),
        });
    }
    let request = rollup_request(events, n, scope_label);
    let response = picker.pick(&request)?;
    let payload: Value = serde_json::from_str(&response)
        .map_err(|error| format!("rollup payload parse error: {error}; response={response:?}"))?;
    let payload = payload
        .as_object()
        .ok_or_else(|| format!("rollup payload parse error: response={response:?}"))?;
    let raw_indices = payload
        .get("picks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .filter_map(|index| usize::try_from(index).ok())
        .collect::<Vec<_>>();
    if n == 0 {
        return Ok(PickResult {
            picks: Vec::new(),
            rationale: payload
                .get("rationale")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        });
    }
    let mut seen = HashSet::new();
    let mut picks = Vec::new();
    for index in raw_indices {
        if index < events.len() && seen.insert(index) {
            picks.push(events[index].clone());
        }
        if picks.len() == n {
            break;
        }
    }
    let rationale = payload
        .get("rationale")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(PickResult { picks, rationale })
}

fn rollup_request(events: &[Value], n: usize, scope_label: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: ROLLUP_CONTEXT.to_owned(),
        contents: vec![ContentPart::Text {
            text: build_user_prompt(events),
        }],
        system_instruction: Some(build_system_instruction(scope_label, n)),
        temperature: 0.3,
        max_output_tokens: 2048,
        thinking_budget: None,
        timeout_s: Some(60.0),
        json_output: true,
        json_schema: Some(build_rollup_schema(n)),
        enforce_responsiveness: false,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: None,
    }
}

fn build_rollup_schema(n: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "picks": {
                "type": "array",
                "description": format!(
                    "Zero-based indices of the most important events from the candidate list, in order of importance (most important first). Length must be exactly {n} (or fewer if input has fewer)."
                ),
                "items": {"type": "integer"},
            },
            "rationale": {
                "type": "string",
                "description": "ONE sentence, max 100 chars, naming the criterion that drove the pick. For debugging — not shown in the UI.",
            },
        },
        "required": ["picks", "rationale"],
        "additionalProperties": false,
    })
}

fn build_system_instruction(scope_label: &str, n: usize) -> String {
    format!(
        "You are picking the {n} MOST IMPORTANT events from a list of candidate events that occurred during one {scope_label} of a personal life-journal. The picked events become the headline cells in the {scope_label} view of a multi-scale timeline UI.\n\nIMPORTANT-EVENT CRITERIA, in priority order:\n  1. Consequence — decisions, shipments, milestones, externally-visible actions outweigh routine maintenance.\n  2. Specificity — concrete events outweigh generic activity descriptors. 'Trademark Filed' beats 'Email Sent'.\n  3. Diversity — when several candidates describe the same underlying thread (e.g., five 'KDE Crash' debugging segments), pick at most one. Reserve the other slots for distinct events.\n  4. Owner-relevance — events involving identifiable people, decisions, or commitments outweigh tooling housekeeping.\n\nReturn JSON: {{ \"picks\": [<indices>], \"rationale\": \"<short>\" }}.\n  - picks: zero-based indices into the input list, in importance order, length exactly {n} (or fewer if input has fewer).\n  - rationale: one sentence naming the criterion (for debugging).\n\nDo NOT rewrite titles or descriptions. Do NOT invent events. Pick from the given list only."
    )
}

fn build_user_prompt(events: &[Value]) -> String {
    let mut lines = vec!["Candidate events:\n".to_owned()];
    for (index, event) in events.iter().enumerate() {
        lines.push(format!(
            "  [{index}] {} — {}",
            string_field(event, "title"),
            string_field(event, "description")
        ));
    }
    lines.join("\n")
}

fn load_day_segments(journal: &Path, day: &str) -> Result<Vec<SegmentRow>, String> {
    let day_dir = journal.join("chronicle").join(day);
    if !day_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    find_timeline_files(&day_dir, &mut candidates);
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for timeline_path in candidates {
        if timeline_path.parent() == Some(day_dir.as_path()) {
            continue;
        }
        let Some(segment_dir) = segment_ancestor(&timeline_path) else {
            continue;
        };
        if !seen.insert(segment_dir.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&timeline_path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let object = data.as_object().ok_or_else(|| {
            format!(
                "timeline.json at {} must be a JSON object",
                timeline_path.display()
            )
        })?;
        let title = object
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        if title.is_empty() && description.is_empty() {
            continue;
        }
        let segment = segment_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        rows.push(SegmentRow {
            hour: segment[..2].to_owned(),
            segment,
            title,
            description,
            origin: object
                .get("origin")
                .and_then(Value::as_str)
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| origin_for_segment(&segment_dir)),
        });
    }
    rows.sort_by(|left, right| left.segment.cmp(&right.segment));
    Ok(rows)
}

fn find_timeline_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_timeline_files(&path, files);
        } else if path.file_name().is_some_and(|name| name == "timeline.json") {
            files.push(path);
        }
    }
}

fn segment_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|ancestor| {
            ancestor
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_segment)
        })
        .map(Path::to_path_buf)
}

fn origin_for_segment(segment: &Path) -> String {
    let mut after_chronicle = false;
    let mut parts = Vec::new();
    for component in segment.components() {
        let part = component.as_os_str().to_string_lossy();
        if after_chronicle {
            parts.push(part.into_owned());
        } else if part == "chronicle" {
            after_chronicle = true;
        }
    }
    if after_chronicle {
        parts.join("/")
    } else {
        segment
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
    }
}

fn group_by_hour(rows: &[SegmentRow]) -> BTreeMap<String, Vec<&SegmentRow>> {
    let mut grouped = BTreeMap::<String, Vec<&SegmentRow>>::new();
    for row in rows {
        grouped.entry(row.hour.clone()).or_default().push(row);
    }
    grouped
}

fn load_day_rollups(journal: &Path) -> Result<BTreeMap<String, Value>, String> {
    let chronicle = journal.join("chronicle");
    if !chronicle.is_dir() {
        return Ok(BTreeMap::new());
    }
    let mut days = std::fs::read_dir(&chronicle)
        .map_err(|error| error.to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_day)
        })
        .collect::<Vec<_>>();
    days.sort();
    let mut rollups = BTreeMap::new();
    for day_dir in days {
        let path = day_dir.join("timeline.json");
        if !path.is_file() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let object = value
            .as_object()
            .ok_or_else(|| format!("timeline.json at {} must be a JSON object", path.display()))?;
        if !object.get("day_top").is_some_and(is_truthy) {
            continue;
        }
        let day = day_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        rollups.insert(day, value);
    }
    Ok(rollups)
}

fn group_by_month(day_rollups: &BTreeMap<String, Value>) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for day in day_rollups.keys() {
        grouped
            .entry(day[..6].to_owned())
            .or_default()
            .push(day.clone());
    }
    grouped
}

fn segment_event(row: &&SegmentRow) -> Value {
    json!({
        "title": row.title,
        "description": row.description,
        "origin": row.origin,
    })
}

fn event_value(event: &Value) -> Value {
    json!({
        "title": string_field(event, "title"),
        "description": string_field(event, "description"),
        "origin": string_field(event, "origin"),
    })
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn is_segment(value: &str) -> bool {
    let bytes = value.as_bytes();
    (8..=13).contains(&bytes.len())
        && bytes.get(6) == Some(&b'_')
        && bytes[..6].iter().all(u8::is_ascii_digit)
        && bytes[7..].iter().all(u8::is_ascii_digit)
}

fn is_day(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_month(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn read_config(journal: &Path) -> Result<Map<String, Value>, String> {
    read_journal_config(journal)
        .map_err(|error| error.to_string())
        .map(|read| read.config.unwrap_or_default())
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    atomic_replace(path, &bytes, AtomicWriteOptions::default()).map_err(|error| error.to_string())
}

fn truncate(value: &str, limit: usize) -> &str {
    value.get(..limit).unwrap_or(value)
}

fn success(stdout: String) -> CliRun {
    CliRun {
        stdout: format!("{stdout}\n"),
        stderr: String::new(),
        exit_code: 0,
    }
}

fn failure(error: String) -> CliRun {
    CliRun {
        stdout: String::new(),
        stderr: format!("timeline maintenance: {error}\n"),
        exit_code: 1,
    }
}

fn usage_error(id: &str, detail: &str) -> CliRun {
    let usage = match id {
        "timeline:rollup-day" => " [DAY] [--top TOP] [--jobs JOBS] [--force] [--dry-run]",
        _ => " [--top TOP] [--jobs JOBS] [--force] [--dry-run] [--months MONTHS]",
    };
    CliRun {
        stdout: String::new(),
        stderr: format!(
            "usage: journal maintenance run {id}{usage}\njournal maintenance run {id}: error: {detail}\n"
        ),
        exit_code: 2,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    reason = "temporary journals and canned model responses are test-only"
)]
mod tests {
    use super::{pick_one, rollup_request, run};
    use crate::timezone::HostTimezoneSource;
    use crate::{GenerateModelResolver, RollupPicker, TimelineServices};
    use chrono::{TimeZone, Utc};
    use serde_json::{Map, Value, json};
    use solstone_core_generate::{ContentPart, GenerateRequest};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Host(&'static str);

    impl HostTimezoneSource for Host {
        fn usable_iana_key(&self) -> Option<String> {
            Some(self.0.to_owned())
        }
    }

    struct Picker {
        replies: Mutex<VecDeque<Result<String, String>>>,
        requests: Mutex<Vec<GenerateRequest>>,
        fail_when: Option<&'static str>,
    }

    impl Picker {
        fn canned(replies: impl IntoIterator<Item = Result<&'static str, &'static str>>) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .map(|reply| reply.map(str::to_owned).map_err(str::to_owned))
                        .collect(),
                ),
                requests: Mutex::new(Vec::new()),
                fail_when: None,
            }
        }

        fn error_when(text: &'static str) -> Self {
            Self {
                replies: Mutex::new(VecDeque::new()),
                requests: Mutex::new(Vec::new()),
                fail_when: Some(text),
            }
        }

        fn call_count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl RollupPicker for Picker {
        fn pick(&self, request: &GenerateRequest) -> Result<String, String> {
            self.requests.lock().unwrap().push(request.clone());
            let contents = match request.contents.first() {
                Some(ContentPart::Text { text }) => text,
                _ => "",
            };
            if self
                .fail_when
                .is_some_and(|needle| contents.contains(needle))
            {
                return Err(format!("canned failure for {contents}"));
            }
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("{\"picks\":[0],\"rationale\":\"canned\"}".to_owned()))
        }
    }

    struct Model {
        value: &'static str,
        calls: AtomicUsize,
    }

    impl Model {
        const fn new(value: &'static str) -> Self {
            Self {
                value,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl GenerateModelResolver for Model {
        fn resolve(&self, _config: &Map<String, Value>) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.value.to_owned())
        }
    }

    fn services<'a>(picker: &'a Picker, model: &'a Model) -> TimelineServices<'a> {
        static HOST: Host = Host("UTC");
        TimelineServices {
            now: Utc.with_ymd_and_hms(2026, 3, 2, 1, 30, 0).unwrap(),
            host_timezone: &HOST,
            picker,
            model_resolver: model,
        }
    }

    #[test]
    fn picker_request_and_response_sanitization_match_the_rollup_contract() {
        let picker = Picker::canned([Ok("{\"picks\":[2,2,9,1,0],\"rationale\":\"consequence\"}")]);
        let events = vec![
            event("Alpha", "first", "a"),
            event("Bravo", "second", "b"),
            event("Charlie", "third", "c"),
        ];
        let picked = pick_one(&picker, &events, 2, "hour").unwrap();
        assert_eq!(picked.picks, vec![events[2].clone(), events[1].clone()]);
        assert_eq!(picked.rationale, "consequence");
        let request = picker.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.context, "timeline.scratch.rollup");
        assert_eq!(request.temperature, 0.3);
        assert_eq!(request.max_output_tokens, 2048);
        assert_eq!(request.timeout_s, Some(60.0));
        assert!(request.json_output);
        assert_eq!(
            request.contents,
            vec![ContentPart::Text {
                text: "Candidate events:\n\n  [0] Alpha — first\n  [1] Bravo — second\n  [2] Charlie — third".to_owned()
            }]
        );
        assert_eq!(
            request.json_schema.unwrap(),
            json!({
                "type": "object",
                "properties": {
                    "picks": {"type": "array", "description": "Zero-based indices of the most important events from the candidate list, in order of importance (most important first). Length must be exactly 2 (or fewer if input has fewer).", "items": {"type": "integer"}},
                    "rationale": {"type": "string", "description": "ONE sentence, max 100 chars, naming the criterion that drove the pick. For debugging — not shown in the UI."}
                },
                "required": ["picks", "rationale"],
                "additionalProperties": false
            })
        );
        assert_eq!(
            rollup_request(&events, 2, "hour")
                .system_instruction
                .unwrap(),
            request.system_instruction.unwrap()
        );
    }

    #[test]
    fn day_existing_output_skips_and_force_overwrites() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        let output = journal
            .path()
            .join("chronicle")
            .join(day)
            .join("timeline.json");
        std::fs::create_dir_all(output.parent().unwrap()).unwrap();
        std::fs::write(&output, b"{\"old\":true}\n").unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let skipped = run(
            "timeline:rollup-day",
            &[day.to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(skipped.exit_code, 0);
        assert!(skipped.stdout.contains("[skip]"));
        assert_eq!(picker.call_count(), 0);
        write_segment(
            journal.path(),
            day,
            "field.audio",
            "080000_1",
            json!({"title":"New", "description":"event"}),
        );
        let forced = run(
            "timeline:rollup-day",
            &[day.to_owned(), "--force".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(forced.exit_code, 0);
        assert_eq!(read_json(&output)["day_top"][0]["title"], "New");
    }

    #[test]
    fn day_empty_and_dry_run_follow_the_reference_boundaries() {
        let journal = tempfile::tempdir().unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let empty = run(
            "timeline:rollup-day",
            &["20260301".to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(empty.exit_code, 66);
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"Dry", "description":"run"}),
        );
        let dry = run(
            "timeline:rollup-day",
            &["20260301".to_owned(), "--dry-run".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(dry.exit_code, 0);
        assert_eq!(picker.call_count(), 0);
        assert!(
            !journal
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
    }

    #[test]
    fn day_decode_failure_is_skipped_but_wrong_shape_fails_without_output() {
        let journal = tempfile::tempdir().unwrap();
        let day = "20260301";
        write_segment(
            journal.path(),
            day,
            "field.audio",
            "080000_1",
            json!({"title":"Good", "description":"row"}),
        );
        write_raw_segment(journal.path(), day, "field.audio", "090000_2", b"{not json");
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let good = run(
            "timeline:rollup-day",
            &[day.to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(good.exit_code, 0);
        assert_eq!(
            read_json(&journal.path().join("chronicle/20260301/timeline.json"))["segment_count"],
            1
        );

        let broken_day = "20260302";
        write_segment(
            journal.path(),
            broken_day,
            "field.audio",
            "080000_1",
            json!(["wrong"]),
        );
        let broken = run(
            "timeline:rollup-day",
            &[broken_day.to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(broken.exit_code, 1);
        assert!(broken.stdout.is_empty());
        assert!(
            !journal
                .path()
                .join("chronicle/20260302/timeline.json")
                .exists()
        );
    }

    #[test]
    fn day_origin_falls_back_to_path_but_embedded_origin_wins() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "first.audio",
            "080000_1",
            json!({"title":"Path", "description":"origin"}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "second.audio",
            "080100_2",
            json!({"title":"Embedded", "description":"origin", "origin":"custom-origin"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-day",
            &["20260301".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        let payload = read_json(&journal.path().join("chronicle/20260301/timeline.json"));
        assert_eq!(
            payload["day_top"][0]["origin"],
            "20260301/first.audio/080000_1"
        );
        assert_eq!(payload["day_top"][1]["origin"], "custom-origin");
    }

    #[test]
    fn day_hour_errors_write_an_error_record_and_day_errors_do_not_write() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"bad-hour", "description":"x"}),
        );
        let picker = Picker::error_when("bad-hour");
        let model = Model::new("model");
        let hour_error = run(
            "timeline:rollup-day",
            &["20260301".to_owned(), "--top".to_owned(), "0".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(hour_error.exit_code, 0);
        let payload = read_json(&journal.path().join("chronicle/20260301/timeline.json"));
        assert_eq!(payload["hours"]["08"]["picks"], json!([]));
        assert_eq!(payload["hours"]["08"]["rationale"], "");
        assert!(payload["hours"]["08"].get("error").is_some());

        let second = tempfile::tempdir().unwrap();
        write_segment(
            second.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"first", "description":"x"}),
        );
        write_segment(
            second.path(),
            "20260301",
            "field.audio",
            "080100_2",
            json!({"title":"first more", "description":"x"}),
        );
        write_segment(
            second.path(),
            "20260301",
            "field.audio",
            "090000_3",
            json!({"title":"second", "description":"y"}),
        );
        write_segment(
            second.path(),
            "20260301",
            "field.audio",
            "090100_4",
            json!({"title":"second more", "description":"y"}),
        );
        let picker = Picker::canned([
            Ok("{\"picks\":[0],\"rationale\":\"hour one\"}"),
            Ok("{\"picks\":[0],\"rationale\":\"hour two\"}"),
            Err("day failed"),
        ]);
        let model = Model::new("model");
        let day_error = run(
            "timeline:rollup-day",
            &[
                "20260301".to_owned(),
                "--top".to_owned(),
                "1".to_owned(),
                "--jobs".to_owned(),
                "1".to_owned(),
            ],
            second.path(),
            &services(&picker, &model),
        );
        assert_eq!(day_error.exit_code, 0);
        assert!(day_error.stdout.contains("[day-err 20260301]"));
        assert!(
            !second
                .path()
                .join("chronicle/20260301/timeline.json")
                .exists()
        );
    }

    #[test]
    fn day_short_circuit_resolves_model_and_invalid_day_is_usage() {
        let journal = tempfile::tempdir().unwrap();
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"One", "description":"event"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("resolved-model");
        let result = run(
            "timeline:rollup-day",
            &["20260301".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(picker.call_count(), 0);
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            read_json(&journal.path().join("chronicle/20260301/timeline.json"))["model"],
            "resolved-model"
        );
        let invalid = run(
            "timeline:rollup-day",
            &["not-a-day".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(invalid.exit_code, 2);
        assert!(invalid.stderr.contains("day must be YYYYMMDD"));
    }

    #[test]
    fn day_default_uses_the_owner_timezone_seam() {
        let journal = tempfile::tempdir().unwrap();
        write_config(
            journal.path(),
            json!({"identity": {"timezone": "Asia/Tokyo"}}),
        );
        write_segment(
            journal.path(),
            "20260301",
            "field.audio",
            "080000_1",
            json!({"title":"Owner date", "description":"wins"}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let result = run(
            "timeline:rollup-day",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            read_json(&journal.path().join("chronicle/20260301/timeline.json"))["day"],
            "20260301"
        );
    }

    #[test]
    fn master_happy_path_embeds_full_days_and_filters_empty_rollups() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("January", "first", "one")], "extra":"kept"}),
        );
        write_day_rollup(
            journal.path(),
            "20260102",
            json!({"day_top":[event("January Two", "second", "two")]}),
        );
        write_day_rollup(journal.path(), "20260201", json!({"day_top":[]}));
        let picker = Picker::canned([]);
        let model = Model::new("month-model");
        let result = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(result.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert_eq!(payload["months"]["202601"]["day_count"], 2);
        assert_eq!(
            payload["months"]["202601"]["days"]["20260101"]["extra"],
            "kept"
        );
        assert_eq!(payload["year_top"][0]["month"], "202601");
        assert!(payload["months"].get("202602").is_none());
    }

    #[test]
    fn master_existing_output_skips_and_force_replaces_it() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("January", "event", "one")]}),
        );
        std::fs::write(journal.path().join("timeline.json"), b"{\"old\":true}\n").unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let skipped = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(skipped.exit_code, 0);
        assert!(skipped.stdout.contains("[skip]"));
        assert_eq!(model.calls.load(Ordering::SeqCst), 0);
        let forced = run(
            "timeline:rollup-master",
            &["--force".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(forced.exit_code, 0);
        assert!(
            read_json(&journal.path().join("timeline.json"))
                .get("months")
                .is_some()
        );
    }

    #[test]
    fn master_empty_wrong_shape_and_no_overlap_keep_their_distinct_exits() {
        let journal = tempfile::tempdir().unwrap();
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let empty = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(empty.exit_code, 66);
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("A", "b", "c")]}),
        );
        let no_overlap = run(
            "timeline:rollup-master",
            &["--months".to_owned(), "202602".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(no_overlap.exit_code, 0);
        assert!(!journal.path().join("timeline.json").exists());
        std::fs::write(
            journal.path().join("chronicle/20260101/timeline.json"),
            b"[]",
        )
        .unwrap();
        let wrong_shape = run(
            "timeline:rollup-master",
            &[],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(wrong_shape.exit_code, 1);
        assert!(!journal.path().join("timeline.json").exists());
    }

    #[test]
    fn master_batch_failure_and_per_month_failure_publish_error_records() {
        let journal = tempfile::tempdir().unwrap();
        write_day_rollup(
            journal.path(),
            "20260101",
            json!({"day_top":[event("Jan", "x", "jan"), event("Jan Two", "x", "jan-two")]}),
        );
        write_day_rollup(
            journal.path(),
            "20260201",
            json!({"day_top":[event("Feb", "y", "feb"), event("Feb Two", "y", "feb-two")]}),
        );
        let picker = Picker::canned([]);
        let model = Model::new("model");
        let whole = run(
            "timeline:rollup-master",
            &["--jobs".to_owned(), "0".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(whole.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert!(
            payload["months"]["202601"]["month_rationale"]
                .as_str()
                .unwrap()
                .starts_with("ERROR: ")
        );
        assert_eq!(payload["months"]["202602"]["day_count"], 1);
        assert_eq!(payload["year_top"], json!([]));

        std::fs::remove_file(journal.path().join("timeline.json")).unwrap();
        let picker = Picker::error_when("Feb");
        let partial = run(
            "timeline:rollup-master",
            &["--top".to_owned(), "1".to_owned()],
            journal.path(),
            &services(&picker, &model),
        );
        assert_eq!(partial.exit_code, 0);
        let payload = read_json(&journal.path().join("timeline.json"));
        assert!(
            payload["months"]["202602"]["month_rationale"]
                .as_str()
                .unwrap()
                .starts_with("ERROR: ")
        );
        assert_eq!(payload["months"]["202601"]["month_top"][0]["title"], "Jan");
        assert_eq!(payload["year_top"].as_array().unwrap().len(), 1);
    }

    fn event(title: &str, description: &str, origin: &str) -> Value {
        json!({"title": title, "description": description, "origin": origin})
    }

    fn write_segment(journal: &Path, day: &str, stream: &str, segment: &str, value: Value) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment)
            .join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn write_raw_segment(journal: &Path, day: &str, stream: &str, segment: &str, contents: &[u8]) {
        let path = journal
            .join("chronicle")
            .join(day)
            .join(stream)
            .join(segment)
            .join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn write_day_rollup(journal: &Path, day: &str, value: Value) {
        let path = journal.join("chronicle").join(day).join("timeline.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn write_config(journal: &Path, config: Value) {
        let path = journal.join("config/journal.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config.to_string()).unwrap();
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}
