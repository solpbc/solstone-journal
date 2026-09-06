// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only Thinking routes retained from the Python Sol application.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Path as AxumPath, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_facets::{list_declared_facet_names, read_facet_declaration};
use solstone_core_facets_web::date_nav_index;
use solstone_core_journal_config::read_journal_config;
use solstone_core_journal_io::cortex_use::{CortexUseCandidateRead, parse_cortex_use_request};
use solstone_core_journal_io::paths::resolve_journal_path;
use solstone_core_system::catchup::updated_days;
use solstone_core_talent_cli::compose_talent;
use solstone_core_talent_config::{
    TalentConfig, TalentFilter, get_output_path, load_talent_configs,
};

use crate::JournalRoot;

#[derive(Clone)]
pub(crate) struct TalentRoots {
    talent_root: PathBuf,
    apps_root: PathBuf,
    templates_dir: PathBuf,
}

impl TalentRoots {
    pub(crate) fn production() -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("could not inspect current executable: {error}"))?;
        let directory = executable.parent().ok_or_else(|| {
            format!(
                "could not locate packaged talent roots from the current executable {}",
                executable.display()
            )
        })?;
        Self::from_executable_dir(directory)
    }

    pub(crate) fn from_executable_dir(directory: &Path) -> Result<Self, String> {
        solstone_core_journal::resolve_installation_root_from_executable_dir(directory)
            .map(Self::from_root)
            .ok_or_else(|| solstone_core_journal::describe_installation_root_miss(directory))
    }

    fn from_root(root: PathBuf) -> Self {
        Self {
            talent_root: root.join("solstone/talent"),
            apps_root: root.join("solstone/apps"),
            templates_dir: root.join("solstone/think/templates"),
        }
    }

    #[cfg(test)]
    fn explicit(talent_root: PathBuf, apps_root: PathBuf, templates_dir: PathBuf) -> Self {
        Self {
            talent_root,
            apps_root,
            templates_dir,
        }
    }
}

pub(crate) async fn api_talents_day(
    AxumPath(day): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(roots): Extension<Arc<TalentRoots>>,
) -> Response {
    if !day_key(&day) {
        return error(
            "invalid_day",
            "that day couldn't be used.",
            "Invalid day format",
            StatusCode::BAD_REQUEST,
        );
    }
    let facet = query.get("facet").cloned();
    let overrides = match journal_config(&journal.0) {
        Ok(config) => config
            .get("talent_overrides")
            .and_then(Value::as_object)
            .cloned(),
        Err(detail) => return talent_failure(detail),
    };
    let configs = match load_talent_configs(
        &roots.talent_root,
        &roots.apps_root,
        overrides.as_ref(),
        TalentFilter {
            r#type: None,
            schedule: None,
            include_disabled: true,
        },
    ) {
        Ok(configs) => configs,
        Err(detail) => return talent_failure(detail),
    };
    Json(json!({
        "uses": uses_for_day(&journal.0, &day, facet.as_deref()),
        "talents": talent_metadata(configs),
        "facets": facets(&journal.0),
    }))
    .into_response()
}

pub(crate) async fn api_agent_run(
    AxumPath(use_id): AxumPath<String>,
    Extension(journal): Extension<Arc<JournalRoot>>,
) -> Response {
    let talents = journal.0.join("talents");
    let Some((path, active)) = find_run_file(&talents, &use_id) else {
        return error(
            "talent_not_found",
            "that talent run couldn't be found.",
            format!("talent run {use_id} not found"),
            StatusCode::NOT_FOUND,
        );
    };
    if active {
        return error(
            "talent_run_pending",
            "that talent run is still running.",
            "",
            StatusCode::ACCEPTED,
        );
    }
    match read_run(&path, &journal.0, &use_id) {
        Ok(run) => Json(run).into_response(),
        Err(RunError::Malformed) => error(
            "talent_run_malformed",
            "that talent run couldn't be read.",
            format!("talent run {use_id} is malformed"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        Err(RunError::Operation(detail)) => talent_failure(detail),
    }
}

pub(crate) async fn api_output_file(
    AxumPath((day, path)): AxumPath<(String, String)>,
    Extension(journal): Extension<Arc<JournalRoot>>,
) -> Response {
    if !day_key(&day) {
        return error(
            "invalid_day",
            "that day couldn't be used.",
            "Invalid day format",
            StatusCode::BAD_REQUEST,
        );
    }
    let canonical_root = match fs::canonicalize(&journal.0) {
        Ok(root) => root,
        Err(error) => return talent_failure(error.to_string()),
    };
    let candidate = if path.starts_with("facets/") {
        journal.0.join(&path)
    } else {
        match resolve_journal_path(&journal.0, &format!("{day}/{path}")) {
            Ok(path) => path,
            Err(_) => return invalid_path(),
        }
    };
    let resolved = match fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return file_not_found(),
        Err(error) => return file_read_failed(error.to_string()),
    };
    // Output fetch deliberately resolves symlinks: in-journal cross-day links are
    // readable, but any link escaping the journal is refused before its contents leak.
    if !resolved.starts_with(&canonical_root) {
        return invalid_path();
    }
    if !resolved.is_file() {
        return file_not_found();
    }
    let content = match fs::read_to_string(&resolved) {
        Ok(content) => content,
        Err(_) => return file_read_failed("Could not read file"),
    };
    let format = if resolved
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        "json"
    } else {
        "md"
    };
    Json(json!({"content": content, "format": format, "filename": resolved.file_name().and_then(|name| name.to_str()).unwrap_or_default()})).into_response()
}

pub(crate) async fn api_preview_prompt(
    AxumPath(name): AxumPath<String>,
    Extension(journal): Extension<Arc<JournalRoot>>,
    Extension(roots): Extension<Arc<TalentRoots>>,
) -> Response {
    let overrides = match journal_config(&journal.0) {
        Ok(config) => config
            .get("talent_overrides")
            .and_then(Value::as_object)
            .cloned(),
        Err(detail) => return talent_failure(detail),
    };
    let config = match load_talent_configs(
        &roots.talent_root,
        &roots.apps_root,
        overrides.as_ref(),
        TalentFilter {
            r#type: None,
            schedule: None,
            include_disabled: true,
        },
    ) {
        Ok(configs) => configs.into_iter().find(|config| config.key == name),
        Err(detail) => return talent_failure(detail),
    };
    let Some(config) = config else {
        return error(
            "talent_not_found",
            "that talent run couldn't be found.",
            format!("Talent '{name}' not found"),
            StatusCode::NOT_FOUND,
        );
    };
    let composed = match compose_talent(&config, &journal.0, &roots.templates_dir, None) {
        Ok(composed) => composed,
        Err(detail) => return talent_failure(detail),
    };
    let system_instruction = string_value(&composed, "system_instruction");
    let extra_context = string_value(&composed, "extra_context");
    let user_instruction = string_value(&composed, "user_instruction");
    let mut sections = Vec::new();
    if !system_instruction.is_empty() {
        sections.push(format!("## System Instruction\n\n{system_instruction}"));
    }
    if !extra_context.is_empty() {
        sections.push(format!("## Context\n\n{extra_context}"));
    }
    if !user_instruction.is_empty() {
        sections.push(format!("## Instructions\n\n{user_instruction}"));
    }
    Json(json!({
        "name": name,
        "title": composed
            .get("title")
            .cloned()
            .unwrap_or_else(|| Value::String(config.key.clone())),
        "full_prompt": sections.join("\n\n"),
        "multi_facet": composed.get("multi_facet").and_then(Value::as_bool).unwrap_or(false),
    }))
    .into_response()
}

pub(crate) async fn api_index(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let counts = talent_use_counts(&journal.0, None)
        .into_iter()
        .map(|(day, facets)| (day, facets.values().sum()))
        .collect::<BTreeMap<_, usize>>();
    Json(date_nav_index(&counts)).into_response()
}

pub(crate) async fn api_stats(
    AxumPath(month): AxumPath<String>,
    Extension(journal): Extension<Arc<JournalRoot>>,
) -> Response {
    if !month_key(&month) {
        return error(
            "invalid_month",
            "that month couldn't be used.",
            "Invalid month format, expected YYYYMM",
            StatusCode::BAD_REQUEST,
        );
    }
    Json(json!(talent_use_counts(&journal.0, Some(&month)))).into_response()
}

pub(crate) async fn api_badge_count(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let count = uses_for_day(&journal.0, &today, None)
        .iter()
        .filter(|use_info| use_info.get("failed").and_then(Value::as_bool) == Some(true))
        .count();
    Json(json!({"count": count})).into_response()
}

pub(crate) async fn api_updated_days(Extension(journal): Extension<Arc<JournalRoot>>) -> Response {
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let exclude = BTreeSet::from([today]);
    match updated_days(&journal.0, &exclude) {
        Ok(days) => Json(json!(days)).into_response(),
        Err(_) => talent_failure("Unable to load updated days"),
    }
}

fn journal_config(journal: &Path) -> Result<Map<String, Value>, String> {
    read_journal_config(journal)
        .map_err(|error| error.to_string())
        .map(|read| read.config.unwrap_or_default())
}

fn talent_metadata(configs: Vec<TalentConfig>) -> BTreeMap<String, Value> {
    configs
        .into_iter()
        .map(|config| {
            let metadata = config.metadata;
            let key = config.key;
            let value = json!({
                "title": metadata
                    .get("title")
                    .cloned()
                    .unwrap_or_else(|| Value::String(key.clone())),
                "description": metadata.get("description").cloned().unwrap_or(Value::Null),
                "color": metadata
                    .get("color")
                    .cloned()
                    .unwrap_or_else(|| Value::String("#6c757d".to_owned())),
                "source": metadata
                    .get("source")
                    .cloned()
                    .unwrap_or_else(|| Value::String("system".to_owned())),
                "app": metadata.get("app").cloned().unwrap_or(Value::Null),
                "schedule": metadata.get("schedule").cloned().unwrap_or(Value::Null),
                "type": metadata.get("type").cloned().unwrap_or(Value::Null),
                "output_format": metadata.get("output").cloned().unwrap_or(Value::Null),
                "multi_facet": metadata.get("multi_facet").and_then(Value::as_bool).unwrap_or(false),
            });
            (key, value)
        })
        .collect()
}

fn facets(journal: &Path) -> BTreeMap<String, Value> {
    list_declared_facet_names(journal)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|name| {
            read_facet_declaration(journal, &name)
                .ok()
                .flatten()
                .map(|facet| {
                    let title = facet
                        .value()
                        .get("title")
                        .cloned()
                        .unwrap_or_else(|| Value::String(name.clone()));
                    (name, json!({"title": title, "color": facet.color}))
                })
        })
        .collect()
}

fn uses_for_day(journal: &Path, day: &str, facet_filter: Option<&str>) -> Vec<Value> {
    let talents = journal.join("talents");
    let mut uses = read_day_index(&talents.join(format!("{day}.jsonl")), facet_filter);
    let Ok(entries) = fs::read_dir(&talents) else {
        return uses;
    };
    let mut active = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    active.sort();
    for directory in active {
        let Ok(files) = fs::read_dir(directory) else {
            continue;
        };
        for file in files.flatten().map(|entry| entry.path()) {
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !name.ends_with("_active.jsonl") || name.contains("_pending") || !file.is_file() {
                continue;
            }
            let Some((logical_day, use_info)) = active_use(&file, journal) else {
                continue;
            };
            if logical_day == day
                && facet_filter.is_none_or(|facet| {
                    use_info.get("facet").and_then(Value::as_str) == Some(facet)
                })
            {
                uses.push(use_info);
            }
        }
    }
    uses.sort_by_key(|value| {
        std::cmp::Reverse(
            value
                .get("start")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        )
    });
    uses
}

fn read_day_index(path: &Path, facet_filter: Option<&str>) -> Vec<Value> {
    if !path.is_file() {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(|entry| {
            facet_filter
                .is_none_or(|facet| entry.get("facet").and_then(Value::as_str) == Some(facet))
        })
        .filter_map(|entry| {
            let id = entry
                .get("use_id")
                .or_else(|| entry.get("agent_id"))?
                .clone();
            let status = entry.get("status").and_then(Value::as_str);
            Some(json!({
                "id": id,
                "name": entry.get("name").cloned().unwrap_or(Value::Null),
                "start": entry.get("ts").cloned().unwrap_or(Value::Null),
                "status": entry.get("status").cloned().unwrap_or(Value::Null),
                "prompt": entry.get("prompt").cloned().unwrap_or(Value::Null),
                "facet": entry.get("facet").cloned().unwrap_or(Value::Null),
                "failed": matches!(status, Some("error" | "unknown")),
                "runtime_seconds": entry.get("runtime_seconds").cloned().unwrap_or(Value::Null),
                "thinking_count": entry.get("thinking_count").cloned().unwrap_or(Value::Null),
                "tool_count": entry.get("tool_count").cloned().unwrap_or(Value::Null),
                "model": entry.get("model").cloned().unwrap_or(Value::Null),
                "provider": entry.get("provider").cloned().unwrap_or(Value::Null),
                "error_message": entry.get("error_message").cloned().unwrap_or(Value::Null),
                "reason_code": entry.get("reason_code").cloned().unwrap_or(Value::Null),
                "output_file": entry.get("output_file").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect()
}

fn active_use(path: &Path, journal: &Path) -> Option<(String, Value)> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let id = path
        .file_stem()?
        .to_str()?
        .strip_suffix("_active")?
        .to_owned();
    let request = read_request(lines.next()?.trim(), path, &id).ok()?;
    let parsed = parse_events(lines);
    let output_file = output_file(&request, journal).ok().flatten();
    let day = request
        .get("day")
        .and_then(Value::as_str)
        .filter(|day| !day.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| use_id_to_day(&id));
    Some((
        day,
        json!({
            "id": id,
            "name": request.get("name").cloned().unwrap_or(Value::Null),
            "start": request_start(&request, &id),
            "status": "running",
            "prompt": request.get("prompt").cloned().unwrap_or_else(|| json!("")),
            "facet": request.get("facet").cloned().unwrap_or(Value::Null),
            "failed": false,
            "runtime_seconds": Value::Null,
            "thinking_count": parsed.thinking_count,
            "tool_count": parsed.tool_count,
            "model": parsed.model,
            "provider": request.get("provider").cloned().or(parsed.provider).unwrap_or(Value::Null),
            "error_message": Value::Null,
            "output_file": output_file,
        }),
    ))
}

fn talent_use_counts(
    journal: &Path,
    month: Option<&str>,
) -> BTreeMap<String, BTreeMap<String, usize>> {
    let talents = journal.join("talents");
    let Ok(entries) = fs::read_dir(talents) else {
        return BTreeMap::new();
    };
    let mut result: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(day) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        if !day_key(&day) || month.is_some_and(|month| !day.starts_with(month)) || !path.is_file() {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for entry in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        {
            let facet = entry
                .get("facet")
                .and_then(Value::as_str)
                .filter(|facet| !facet.is_empty())
                .unwrap_or("_none");
            *result
                .entry(day.clone())
                .or_default()
                .entry(facet.to_owned())
                .or_default() += 1;
        }
    }
    result
}

enum CandidateOutcome {
    Match,
    NonMatch,
    Unparseable,
}

fn read_run_candidate_use_id(path: &Path, use_id: &str) -> CandidateOutcome {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return CandidateOutcome::Unparseable,
    };
    let Some(line) = BufReader::new(file).lines().next() else {
        return CandidateOutcome::Unparseable;
    };
    let Ok(line) = line else {
        return CandidateOutcome::Unparseable;
    };
    let line = line.trim();
    if line.is_empty() {
        return CandidateOutcome::Unparseable;
    }
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        return CandidateOutcome::Unparseable;
    };
    match event.get("use_id").and_then(Value::as_str) {
        Some(found) if found == use_id => CandidateOutcome::Match,
        Some(_) | None => CandidateOutcome::NonMatch,
    }
}

fn find_run_file(talents: &Path, use_id: &str) -> Option<(PathBuf, bool)> {
    let mut unparseable: Option<(PathBuf, bool)> = None;
    for (suffix, active) in [(".jsonl", false), ("_active.jsonl", true)] {
        let entries = fs::read_dir(talents).ok()?;
        for directory in entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
        {
            let path = directory.join(format!("{use_id}{suffix}"));
            if !path.is_file() {
                continue;
            }
            match read_run_candidate_use_id(&path, use_id) {
                CandidateOutcome::Match => return Some((path, active)),
                CandidateOutcome::NonMatch => {}
                CandidateOutcome::Unparseable => {
                    if unparseable.is_none() {
                        unparseable = Some((path, active));
                    }
                }
            }
        }
    }
    unparseable
}

enum RunError {
    Malformed,
    Operation(String),
}

fn read_request(first: &str, path: &Path, use_id: &str) -> Result<Value, RunError> {
    let directory = path.parent().ok_or(RunError::Malformed)?;
    if !matches!(
        parse_cortex_use_request(directory, use_id, first.as_bytes()),
        CortexUseCandidateRead::Accepted(_)
    ) {
        return Err(RunError::Malformed);
    }
    serde_json::from_str(first).map_err(|_| RunError::Malformed)
}

fn request_start(request: &Value, use_id: &str) -> i64 {
    request
        .get("ts")
        .and_then(Value::as_i64)
        .filter(|ts| *ts != 0)
        .unwrap_or_else(|| use_id.parse().unwrap_or_default())
}

fn read_run(path: &Path, journal: &Path, use_id: &str) -> Result<Value, RunError> {
    let text = fs::read_to_string(path).map_err(|error| RunError::Operation(error.to_string()))?;
    let mut lines = text.lines();
    let Some(first) = lines.next().map(str::trim).filter(|line| !line.is_empty()) else {
        return Err(RunError::Malformed);
    };
    let request = read_request(first, path, use_id)?;
    let parsed = parse_events(lines);
    let output_file = output_file(&request, journal).map_err(RunError::Operation)?;
    let start = request_start(&request, use_id);
    let end = parsed.finish_ts.or(parsed.error_ts);
    let runtime_seconds = end
        .filter(|_| start != 0)
        .map(|end| (end - start) as f64 / 1000.0);
    let end_state = parsed.end_state.as_deref().unwrap_or("unknown");
    Ok(json!({
        "id": use_id,
        "name": request.get("name").cloned().unwrap_or(Value::Null),
        "start": start,
        "status": "completed",
        "prompt": request.get("prompt").cloned().unwrap_or_else(|| json!("")),
        "facet": request.get("facet").cloned().unwrap_or(Value::Null),
        "failed": matches!(end_state, "error" | "unknown"),
        "runtime_seconds": runtime_seconds,
        "thinking_count": parsed.thinking_count,
        "tool_count": parsed.tool_count,
        "model": parsed.model,
        "provider": request.get("provider").cloned().or(parsed.provider).unwrap_or(Value::Null),
        "error_message": parsed.error_message,
        "reason_code": parsed.reason_code,
        "output_file": output_file,
        "events": parsed.events,
        "day": request.get("day").cloned().unwrap_or_else(|| json!("")),
    }))
}

struct ParsedEvents {
    thinking_count: usize,
    tool_count: usize,
    model: Value,
    provider: Option<Value>,
    usage: Option<Value>,
    finish_ts: Option<i64>,
    error_ts: Option<i64>,
    error_message: Value,
    reason_code: Value,
    end_state: Option<String>,
    events: Vec<Value>,
}

fn parse_events<'a>(lines: impl Iterator<Item = &'a str>) -> ParsedEvents {
    let mut parsed = ParsedEvents {
        thinking_count: 0,
        tool_count: 0,
        model: Value::Null,
        provider: None,
        usage: None,
        finish_ts: None,
        error_ts: None,
        error_message: Value::Null,
        reason_code: Value::Null,
        end_state: None,
        events: Vec::new(),
    };
    for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(mut event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = event
            .get("event")
            .and_then(Value::as_str)
            .map(str::to_owned);
        match kind.as_deref() {
            Some("thinking") => parsed.thinking_count += 1,
            Some("tool_start") => parsed.tool_count += 1,
            Some("start") => {
                parsed.model = event.get("model").cloned().unwrap_or(Value::Null);
                parsed.provider = event.get("provider").cloned();
            }
            Some("finish") => {
                parsed.finish_ts = event.get("ts").and_then(Value::as_i64).or(Some(0));
                parsed.usage = event.get("usage").cloned();
                parsed.end_state = Some("finish".to_owned());
            }
            Some("error") => {
                parsed.error_ts = event.get("ts").and_then(Value::as_i64).or(Some(0));
                parsed.reason_code = event.get("reason_code").cloned().unwrap_or(Value::Null);
                if let Some(message) = event
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    parsed.error_message = json!(message.chars().take(200).collect::<String>());
                }
                parsed.end_state = Some("error".to_owned());
            }
            _ => {}
        }
        if let Some(object) = event.as_object_mut() {
            object.remove("raw");
        }
        parsed.events.push(event);
    }
    parsed
}

fn output_file(request: &Value, journal: &Path) -> Result<Option<String>, String> {
    let Some(output) = request.get("output").filter(|value| truthy(value)) else {
        return Ok(None);
    };
    let path = if let Some(path) = request
        .get("output_path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
    {
        PathBuf::from(path)
    } else {
        let Some(day) = request
            .get("day")
            .and_then(Value::as_str)
            .filter(|day| !day.is_empty())
        else {
            return Ok(None);
        };
        let name = request
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        get_output_path(
            &journal.join(day),
            name,
            request.get("segment").and_then(Value::as_str),
            output.as_str(),
            request.get("facet").and_then(Value::as_str),
            request
                .get("env")
                .and_then(Value::as_object)
                .and_then(|env| env.get("SOL_STREAM"))
                .and_then(Value::as_str),
        )
    };
    if !path.exists() {
        return Ok(None);
    }
    let day_dir = request
        .get("day")
        .and_then(Value::as_str)
        .map(|day| journal.join(day));
    // Run detail intentionally stays lexical: the UI may name an in-day symlink
    // even when following it would escape the journal; fetch enforces containment.
    if let Some(day_dir) = day_dir.filter(|day_dir| path.starts_with(day_dir)) {
        return path
            .strip_prefix(day_dir)
            .map(|path| Some(path.display().to_string()))
            .map_err(|error| error.to_string());
    }
    path.strip_prefix(journal)
        .map(|path| Some(path.display().to_string()))
        .map_err(|error| error.to_string())
}

fn use_id_to_day(use_id: &str) -> String {
    use_id
        .parse::<i64>()
        .ok()
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Local)
                .format("%Y%m%d")
                .to_string()
        })
        .unwrap_or_default()
}

fn day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn month_key(value: &str) -> bool {
    value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn string_value<'a>(map: &'a Map<String, Value>, key: &str) -> &'a str {
    map.get(key).and_then(Value::as_str).unwrap_or_default()
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64() != Some(0) && value.as_u64() != Some(0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn error(reason: &str, message: &str, detail: impl Into<String>, status: StatusCode) -> Response {
    error_envelope(reason, message, detail, status).into_response()
}
fn talent_failure(detail: impl Into<String>) -> Response {
    error(
        "talent_operation_failed",
        "that talent data couldn't be loaded.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}
fn invalid_path() -> Response {
    // Sol deliberately treats a rejected output path as a containment refusal
    // (403), overriding INVALID_PATH's generic 400 status from reasons.py.
    error(
        "invalid_path",
        "that path couldn't be used.",
        "Invalid path",
        StatusCode::FORBIDDEN,
    )
}
fn file_not_found() -> Response {
    error(
        "file_not_found",
        "that file couldn't be found.",
        "File not found",
        StatusCode::NOT_FOUND,
    )
}
fn file_read_failed(detail: impl Into<String>) -> Response {
    error(
        "file_read_failed",
        "that file couldn't be read.",
        detail,
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use axum::routing::get;
    use filetime::FileTime;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn preview_composes_distinct_sections_without_journal_write() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
        fs::create_dir_all(root.path().join("talent")).expect("talent root");
        fs::create_dir_all(root.path().join("apps")).expect("apps root");
        fs::create_dir_all(root.path().join("think/templates")).expect("template root");
        fs::create_dir_all(root.path().join("config")).expect("config root");
        fs::write(
            root.path().join("config/journal.json"),
            r#"{"setup":{"completed_at":1},"agent":{"name":"Agent"},"identity":{"name":"Owner"}}"#,
        )
        .expect("config");
        fs::write(
            root.path().join("talent/demo.md"),
            "{\n\"type\": \"generate\",\n\"title\": \"\",\n\"output\": \"json\",\n\"system_instruction\": \"SYSTEM\",\n\"extra_context\": \"CONTEXT\"\n}\nINSTRUCTIONS\n",
        )
        .expect("talent");
        let config = root.path().join("config/journal.json");
        let before = fs::read(&config).expect("config bytes");
        let authored = FileTime::from_unix_time(1_700_000_000, 0);
        filetime::set_file_mtime(&config, authored).expect("mtime stamps");
        let roots = TalentRoots::explicit(
            root.path().join("talent"),
            root.path().join("apps"),
            root.path().join("think/templates"),
        );
        let app = axum::Router::new()
            .route("/preview/{*name}", get(api_preview_prompt))
            .layer(Extension(Arc::new(roots)))
            .layer(Extension(Arc::new(JournalRoot(root.path().to_path_buf()))));
        let response = app
            .oneshot(
                Request::get("/preview/demo")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status();
        let body: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body"),
        )
        .expect("json");
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["title"], "");
        assert_eq!(
            body["full_prompt"],
            "## System Instruction\n\nSYSTEM\n\n## Context\n\nCONTEXT\n\n## Instructions\n\nINSTRUCTIONS"
        );
        assert_eq!(fs::read(&config).expect("config bytes"), before);
        assert_eq!(
            FileTime::from_last_modification_time(&fs::metadata(&config).expect("metadata")),
            authored
        );
    }

    #[test]
    fn talent_metadata_preserves_explicit_empty_values() {
        let config = TalentConfig {
            key: "empty".to_owned(),
            file: "talent/empty.md".to_owned(),
            metadata: Map::from_iter([
                ("title".to_owned(), json!("")),
                ("description".to_owned(), json!("")),
                ("color".to_owned(), json!("")),
                ("source".to_owned(), json!("")),
            ]),
            body: String::new(),
        };
        let metadata = talent_metadata(vec![config]);
        let empty = &metadata["empty"];
        assert_eq!(empty["title"], "");
        assert_eq!(empty["description"], "");
        assert_eq!(empty["color"], "");
        assert_eq!(empty["source"], "");
    }

    #[test]
    fn share_layout_resolves_and_fails_when_anchor_removed() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("share layout root");
        let bin = root.path().join("bin");
        let share = root.path().join("share");
        fs::create_dir_all(&bin).unwrap();
        for relative in [
            solstone_core_journal::LAYOUT_BUNDLE_ANCHOR,
            solstone_core_journal::LAYOUT_LAYOUT_ANCHOR,
            solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR,
        ] {
            let path = share.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, relative).unwrap();
        }
        let roots = TalentRoots::from_executable_dir(&bin).unwrap();
        assert_eq!(roots.talent_root, share.join("solstone/talent"));
        assert_eq!(roots.apps_root, share.join("solstone/apps"));
        fs::remove_file(share.join(solstone_core_journal::LAYOUT_TEMPLATE_ANCHOR)).unwrap();
        assert!(TalentRoots::from_executable_dir(&bin).is_err());
    }

    #[test]
    fn read_run_candidate_use_id_classifies_match_mismatch_missing_and_unparseable() {
        let root = tempfile::TempDir::new_in("/var/tmp").expect("root");
        let path = root.path().join("run.jsonl");
        fs::write(&path, r#"{"event":"request","use_id":"hit"}"#).expect("match");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::Match
        ));
        fs::write(&path, r#"{"event":"request","use_id":"other"}"#).expect("mismatch");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::NonMatch
        ));
        fs::write(&path, r#"{"event":"request"}"#).expect("missing");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::NonMatch
        ));
        fs::write(&path, "").expect("empty");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::Unparseable
        ));
        fs::write(&path, "not-json\n").expect("invalid");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::Unparseable
        ));
        fs::write(&path, [0xff, 0xfe]).expect("non-utf8");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::Unparseable
        ));
        fs::remove_file(&path).expect("vanish");
        assert!(matches!(
            read_run_candidate_use_id(&path, "hit"),
            CandidateOutcome::Unparseable
        ));
    }
}
