// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local support diagnostics, with the same intentionally narrow redaction as Python.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use regex::Regex;
use serde_json::{Map, Value, json};
use solstone_core_journal_io::{
    JournalRoot,
    operational_log::{catalog_oplogs, open_oplog_catalog_entry},
};

const UNKNOWN_HEADLINE: &str = "thinking status unavailable";
const RECENCY_WINDOW: Duration = Duration::hours(168);

static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[A-Za-z0-9_-]*(?:api[_-]?key|access[_-]?token|token|secret|password|passwd|pwd)[A-Za-z0-9_-]*\s*[=:]\s*[^\s;]+")
        .expect("fixed regex")
});
static ENV_SECRET_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z0-9_]*(?:API_KEY|ACCESS_TOKEN|TOKEN|SECRET|PASSWORD)[A-Z0-9_]*\b")
        .expect("fixed regex")
});
// The `sk-ant-` branch is intentionally shadowed by leftmost-first `sk-`, as in Python.
static SECRET_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]+|sk-ant-[A-Za-z0-9_-]+|AIza[A-Za-z0-9_-]+)\b")
        .expect("fixed regex")
});
static WINDOWS_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z]:\\[^\s;]+").expect("fixed regex"));

/// Injected platform facts so tests never observe the build host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    pub system: String,
    pub release: String,
    pub machine: String,
}

/// Return the redacted, character-bounded text used by support diagnostics.
pub fn bounded_redacted_text(value: Option<&str>, limit: usize) -> Option<String> {
    let value = value?;
    let mut clean = value.split_whitespace().collect::<Vec<_>>().join(" ");
    clean = SECRET_ASSIGNMENT
        .replace_all(&clean, "<secret>")
        .into_owned();
    clean = ENV_SECRET_NAME.replace_all(&clean, "<secret>").into_owned();
    clean = SECRET_VALUE.replace_all(&clean, "<secret>").into_owned();
    clean = WINDOWS_PATH.replace_all(&clean, "<path>").into_owned();
    clean = redact_posix_paths(&clean);
    clean = clean.replace("Traceback (most recent call last):", "traceback redacted");
    if clean.chars().count() <= limit {
        return Some(clean);
    }
    Some(
        clean
            .chars()
            .take(limit.saturating_sub(1))
            .collect::<String>()
            + "…",
    )
}

fn redact_posix_paths(value: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    for (index, character) in value.char_indices() {
        if index < cursor {
            continue;
        }
        if character != '/' {
            continue;
        }
        let previous = value[..index].chars().next_back();
        // Faithful hand-roll of Python `(?<!\w)/(?:[^\s;]+)`: Unicode word
        // lookbehind is `prev.is_alphanumeric() || prev == '_'`, which regex cannot express.
        if previous.is_some_and(|item| item.is_alphanumeric() || item == '_') {
            continue;
        }
        let end = value[index..]
            .char_indices()
            .skip(1)
            .find_map(|(offset, item)| {
                (item.is_whitespace() || item == ';').then_some(index + offset)
            })
            .unwrap_or(value.len());
        if end == index + 1 {
            continue;
        }
        output.push_str(&value[cursor..index]);
        output.push_str("<path>");
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

/// Substring-based secret-key predicate; this intentionally includes `api_key_id`.
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_lowercase();
    ["key", "token", "secret", "password"]
        .iter()
        .any(|needle| key.contains(needle))
}

/// Recursively redact secret-keyed object values without descending into them.
pub fn strip_secrets(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if is_secret_key(key) {
                            json!("***")
                        } else {
                            strip_secrets(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(strip_secrets).collect()),
        _ => value.clone(),
    }
}

/// Version is intentionally unredacted: it has no input value to redact.
pub fn collect_version() -> Option<String> {
    Some(env!("CARGO_PKG_VERSION").to_owned())
}

/// Revision is intentionally unredacted: it has no input value to redact.
/// Return the revision captured at build time, or `None` for a non-git build.
pub fn collect_revision() -> Option<String> {
    option_env!("SOLSTONE_SUPPORT_PORTAL_REVISION").map(ToOwned::to_owned)
}

/// Platform is intentionally unredacted: it has no input value to redact.
pub fn collect_platform(platform: PlatformInfo) -> Value {
    // Python includes `python`; this Rust-native wire drops it because no Python runtime is owned.
    json!({"system": platform.system, "release": platform.release, "machine": platform.machine})
}

pub fn native_platform() -> PlatformInfo {
    let system = match std::env::consts::OS {
        "linux" => "Linux".to_owned(),
        "macos" => "Darwin".to_owned(),
        "windows" => "Windows".to_owned(),
        other => other.to_owned(),
    };
    // Python platform.release() has no error surface.  Keep the Rust collector infallible.
    let release = nix::sys::utsname::uname()
        .map(|value| value.release().to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_owned());
    PlatformInfo {
        system,
        release,
        machine: std::env::consts::ARCH.to_owned(),
    }
}

/// Services are intentionally unredacted: their status values are not diagnostic text.
pub fn collect_services(journal_root: &Path) -> Value {
    collect_services_with_probe(journal_root, signal_zero)
}

fn collect_services_with_probe(
    journal_root: &Path,
    probe: impl Fn(i32) -> Result<(), Errno>,
) -> Value {
    let health = journal_root.join("health");
    let Ok(entries) = fs::read_dir(health) else {
        return json!({});
    };
    let mut statuses = Map::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "pid") {
            continue;
        }
        let service = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let status = service_status(fs::read_to_string(&path).ok().as_deref(), &probe);
        statuses.insert(service.to_owned(), Value::String(status.to_owned()));
    }
    Value::Object(statuses)
}

fn service_status(
    pid_text: Option<&str>,
    probe: &impl Fn(i32) -> Result<(), Errno>,
) -> &'static str {
    let Some(pid) = pid_text.and_then(|value| value.trim().parse::<i32>().ok()) else {
        return "stopped";
    };
    match probe(pid) {
        Ok(()) => "running",
        Err(Errno::ESRCH | Errno::EPERM) => "stopped",
        Err(_) => "unknown",
    }
}

fn signal_zero(pid: i32) -> Result<(), Errno> {
    kill(Pid::from_raw(pid), None)
}

/// Closed, path-free error for the support bundle's log section.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct LogCollectionError {
    kind: &'static str,
    day: Option<String>,
}

impl LogCollectionError {
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn day(&self) -> Option<&str> {
        self.day.as_deref()
    }

    fn catalog(error: &solstone_core_journal_io::operational_log::OplogCatalogError) -> Self {
        Self {
            kind: error.kind(),
            day: error.day().map(ToOwned::to_owned),
        }
    }

    fn root() -> Self {
        Self {
            kind: "oplog_catalog_root",
            day: None,
        }
    }

    fn io(day: &str) -> Self {
        Self {
            kind: "oplog_catalog_io",
            day: Some(day.to_owned()),
        }
    }

    fn timestamp(day: &str) -> Self {
        Self {
            kind: "oplog_catalog_timestamp",
            day: Some(day.to_owned()),
        }
    }

    fn json(&self) -> Value {
        let mut object = Map::new();
        object.insert("kind".to_owned(), Value::String(self.kind.to_owned()));
        if let Some(day) = &self.day {
            object.insert("day".to_owned(), Value::String(day.clone()));
        }
        Value::Object(object)
    }
}

/// Collect recent canonical operational-log errors over every intersecting local day.
pub fn collect_recent_errors(
    journal_root: &Path,
    now: DateTime<Local>,
) -> Result<Value, LogCollectionError> {
    let cutoff = now - RECENCY_WINDOW;
    let mut candidates = Vec::new();
    let mut day = cutoff.date_naive();
    while day <= now.date_naive() {
        let day_key = day.format("%Y%m%d").to_string();
        let root = JournalRoot::open(journal_root).map_err(|_| LogCollectionError::root())?;
        let snapshot =
            catalog_oplogs(root, &[day]).map_err(|error| LogCollectionError::catalog(&error))?;
        for entry in snapshot.entries() {
            let service = entry.name().source().display_slug().to_owned();
            let root = JournalRoot::open(journal_root).map_err(|_| LogCollectionError::root())?;
            let mut file = open_oplog_catalog_entry(root, entry)
                .map_err(|error| LogCollectionError::catalog(&error))?;
            file.seek(SeekFrom::Start(entry.payload_offset() as u64))
                .map_err(|_| LogCollectionError::io(&day_key))?;
            let fallback = file
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .map(DateTime::<Local>::from);
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|_| LogCollectionError::io(&day_key))?;
            let text = String::from_utf8_lossy(&bytes);
            let mut last = None;
            for line in text.lines().filter(|line| line.contains("ERROR")) {
                let (first, rest) = split_python_once(line);
                let (time, approximate, message) = match parse_local_timestamp(first)
                    .map_err(|_| LogCollectionError::timestamp(&day_key))?
                {
                    Some(time) => {
                        last = Some(time);
                        (
                            time,
                            false,
                            bounded_redacted_text(Some(rest.trim()), 500).unwrap_or_default(),
                        )
                    }
                    None => {
                        let Some(time) = last.or(fallback) else {
                            continue;
                        };
                        (
                            time,
                            true,
                            bounded_redacted_text(Some(line.trim()), 500).unwrap_or_default(),
                        )
                    }
                };
                if time < cutoff {
                    continue;
                }
                candidates.push((time, json!({"service":service,"message":message,"time":time.to_rfc3339_opts(SecondsFormat::Secs, false),"time_approximate":approximate})));
            }
        }
        day = day
            .succ_opt()
            .expect("local day has successor in the supported range");
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    Ok(Value::Array(
        candidates
            .into_iter()
            .take(10)
            .map(|(_, value)| value)
            .collect(),
    ))
}

fn parse_local_timestamp(value: &str) -> Result<Option<DateTime<Local>>, String> {
    if parses_aware_timestamp(value) {
        // Python compares this aware value to its naive local cutoff and raises TypeError.
        return Err("offset-aware timestamp cannot compare to naive local cutoff".to_owned());
    }
    Ok(parse_naive_timestamp(value).and_then(|value| Local.from_local_datetime(&value).single()))
}

fn split_python_once(line: &str) -> (&str, &str) {
    let line = line.trim_start();
    let Some(index) = line.find(char::is_whitespace) else {
        return (line, "");
    };
    (&line[..index], line[index..].trim_start())
}

fn parses_aware_timestamp(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value).is_ok()
        || [
            "%Y-%m-%dT%H:%M:%S%.f%z",
            "%Y-%m-%d %H:%M:%S%.f%z",
            "%Y%m%dT%H%M%S%.f%z",
        ]
        .iter()
        .any(|format| DateTime::parse_from_str(value, format).is_ok())
}

fn parse_naive_timestamp(value: &str) -> Option<NaiveDateTime> {
    [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y%m%dT%H%M%S%.f",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
    .or_else(|| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
    })
}

/// Return config data with secret-keyed values removed.
pub fn collect_config(journal_root: &Path) -> Value {
    let path = journal_root.join("config/config.json");
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .map(|value| strip_secrets(&value))
        .unwrap_or_else(|| json!({}))
}

/// Build and redact the support-surface brain health payload; this collector never propagates.
pub fn collect_brain_health(journal_root: &Path, now: DateTime<Utc>) -> Value {
    build_brain_health(journal_root, now)
        .map(|value| strip_secrets(&value))
        .unwrap_or_else(|_| brain_fallback())
}

fn build_brain_health(journal_root: &Path, now: DateTime<Utc>) -> Result<Value, String> {
    let config =
        solstone_core_thinking::read_config(journal_root).map_err(|error| error.to_string())?;
    let inspection = solstone_core_brain::inspect_brain_state(journal_root, &config, now);
    let present = solstone_core_brain::present_brain_inspection(&inspection, now);
    let projection = &inspection.projection;
    let progressing = brain_progressing(
        projection.reason_code.as_deref(),
        projection.runtime_transition_in_progress,
    );
    let snapshot = json!({
        "state": projection.aggregate_state,
        "headline": present.headline,
        "reason_code": projection.reason_code,
        "reason_text": present.reason_text,
        "failing_component": present.failing_component,
        "action": support_action(&projection.aggregate_state, projection.reason_code.as_deref(), progressing, projection.active_lane.as_deref(), present.failing_component.as_deref()),
        "identity": {"lane":projection.active_lane,"provider":projection.active_provider,"model":projection.active_model},
        "evidence": {"observed_at":present.evidence.observed_at,"age_seconds":present.evidence.age_seconds,"age_text":present.evidence.age_text},
        "components": {"generate":brain_component(inspection.record.as_ref(), "generate"),"cogitate":brain_component(inspection.record.as_ref(), "cogitate")},
        "progressing": progressing,
    });
    let lines = render_brain_health_lines(&snapshot)?;
    Ok(json!({"snapshot":snapshot,"lines":lines}))
}

fn support_action(
    state: &str,
    reason: Option<&str>,
    progressing: bool,
    lane: Option<&str>,
    component: Option<&str>,
) -> Value {
    if matches!(state, "ready" | "checking") || (state == "blocked" && progressing) {
        return Value::Null;
    }
    if matches!(state, "blocked" | "unhealthy") {
        let local_reason = matches!(
            reason,
            Some(
                "gpu_unavailable"
                    | "local_runtime_not_ready"
                    | "local_artifact_not_ready"
                    | "local_server_unhealthy"
                    | "local_runtime_state_invalid"
                    | "local_runtime_state_unavailable"
                    | "local_runtime_state_stale"
                    | "local_runtime_fingerprint_mismatch"
            )
        );
        if lane == Some("bundled")
            && (local_reason
                || (reason == Some("probe_internal_error")
                    && component == Some("lane_prerequisites")))
        {
            return json!({"href":"/app/thinking/#local-setup","label":"open local setup"});
        }
        return json!({"href":"/app/thinking/#main","label":"open thinking"});
    }
    if state == "unknown" && reason == Some("configuration_invalid") {
        return json!({"href":"/app/thinking/#main","label":"open thinking"});
    }
    if state == "unknown" {
        return json!({"href":"/app/health/#brain","label":"view health"});
    }
    Value::Null
}

fn brain_progressing(reason: Option<&str>, runtime_transition: bool) -> bool {
    reason == Some("brain_check_in_progress")
        || (reason == Some("local_runtime_not_ready") && runtime_transition)
}

fn brain_component(record: Option<&Value>, name: &str) -> Value {
    let value = record.and_then(|record| record.pointer(&format!("/evidence/{name}")));
    let reason = value
        .and_then(|item| item.get("reason_code"))
        .and_then(Value::as_str);
    json!({"status":value.and_then(|item| item.get("status")).cloned().unwrap_or(Value::Null),"reason_code":reason,"reason_text":reason.map(|item| item.replace('_', " ")).unwrap_or_else(|| "unknown".to_owned()),"observed_at":value.and_then(|item| item.get("observed_at")).cloned().unwrap_or(Value::Null)})
}

fn render_brain_health_lines(snapshot: &Value) -> Result<Vec<String>, String> {
    let headline = snapshot
        .get("headline")
        .and_then(Value::as_str)
        .ok_or("headline")?;
    let mut lines = vec!["Brain Health".to_owned(), format!("  {headline}")];
    let identity = snapshot
        .get("identity")
        .and_then(Value::as_object)
        .ok_or("identity")?;
    let lane = identity.get("lane").and_then(Value::as_str);
    let provider = identity.get("provider").and_then(Value::as_str);
    let model = identity.get("model").and_then(Value::as_str);
    let reason = snapshot
        .get("reason_text")
        .and_then(Value::as_str)
        .ok_or("reason")?;
    let component = snapshot
        .get("failing_component")
        .and_then(Value::as_str)
        .map(|item| format!(" ({item})"))
        .unwrap_or_default();
    if let (Some(lane), Some(provider), Some(model)) = (lane, provider, model) {
        if snapshot.get("state").and_then(Value::as_str) == Some("ready") {
            let age = snapshot
                .pointer("/evidence/age_text")
                .and_then(Value::as_str);
            lines.push(age.map_or_else(
                || format!("  {lane} {provider}/{model}"),
                |age| format!("  {lane} {provider}/{model}, checked {age} ago"),
            ));
        } else {
            lines.push(format!("  {lane} {provider}/{model} — {reason}{component}"));
        }
    } else if lane.is_some() || provider.is_some() || model.is_some() {
        lines.push(format!("  {reason}{component}"));
    }
    if let Some(action) = snapshot.get("action").and_then(Value::as_object)
        && let Some(label) = action.get("label").and_then(Value::as_str)
    {
        let target = action
            .get("href")
            .or_else(|| action.get("command"))
            .and_then(Value::as_str);
        lines.push(target.map_or_else(
            || format!("  → {label}"),
            |target| format!("  → {label}: {target}"),
        ));
    }
    Ok(lines)
}

fn brain_fallback() -> Value {
    json!({"snapshot":{"state":"unknown","headline":UNKNOWN_HEADLINE,"reason_code":"brain_record_unavailable"},"lines":["Brain Health",format!("  {UNKNOWN_HEADLINE}")]})
}

/// Collect every support diagnostic in fixed insertion order, omitting only a failed collector.
pub fn collect_all(
    journal_root: &Path,
    now: DateTime<Local>,
    platform: PlatformInfo,
) -> Map<String, Value> {
    let mut output = Map::new();
    // Version, revision, and platform are input-less/unredacted by reference behavior.
    output.insert(
        "version".to_owned(),
        collect_version().map_or(Value::Null, Value::String),
    );
    output.insert(
        "revision".to_owned(),
        collect_revision().map_or(Value::Null, Value::String),
    );
    output.insert("platform".to_owned(), collect_platform(platform));
    output.insert("services".to_owned(), collect_services(journal_root));
    match collect_recent_errors(journal_root, now) {
        Ok(errors) => {
            output.insert("recent_errors".to_owned(), errors);
        }
        Err(error) => {
            output.insert("log_collection_error".to_owned(), error.json());
        }
    }
    output.insert("config".to_owned(), collect_config(journal_root));
    output.insert(
        "brain_health".to_owned(),
        collect_brain_health(journal_root, now.with_timezone(&Utc)),
    );
    output
}

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod redaction_tests;
