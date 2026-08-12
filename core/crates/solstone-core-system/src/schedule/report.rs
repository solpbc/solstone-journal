// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, io, path::Path};

use serde_json::{Map, Value};
use solstone_core_journal_io::{MalformedPolicy, ReadError, read_json};

use super::config::{RESERVED_METADATA_KEYS, is_interval, json_truthy, metadata_from_raw};
use super::due::{
    compute_next_run, effective_every, local_from_epoch, parse_daily_time, state_entry,
};
use super::{ScheduleConfig, ScheduleEntry, ScheduleNow};

/// One rendered schedule table row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleReportRow {
    pub name: String,
    pub every: String,
    pub last_run: String,
    pub next_due: String,
    pub cmd: String,
    pub disabled: bool,
}

/// Read-only projection of the raw schedule configuration and completion state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleReport {
    pub rows: Vec<ScheduleReportRow>,
    pub diagnostics: Vec<String>,
    pub exit_code: u8,
    config_path: String,
    no_config: bool,
}

impl ScheduleReport {
    /// Render the legacy-compatible schedule display without diagnostics.
    pub fn render(&self) -> String {
        if self.no_config {
            return format!(
                "No schedules configured.\n\nAdd schedules to: {}\n",
                self.config_path
            );
        }
        let name_width = self
            .rows
            .iter()
            .map(|row| row.name.len())
            .max()
            .unwrap_or(0)
            .max(4);
        let mut output = format!(
            "  {:<name_width$}  {:<8}  {:<18}  {:<10}  CMD\n\n",
            "NAME", "EVERY", "LAST RUN", "NEXT DUE",
        );
        for row in &self.rows {
            let tags = if row.disabled { " [disabled]" } else { "" };
            let line = format!(
                "  {:<name_width$}  {:<8}  {:<18}  {:<10}  {}{}",
                row.name, row.every, row.last_run, row.next_due, row.cmd, tags,
            );
            output.push_str(line.trim_end());
            output.push('\n');
        }
        output.push('\n');
        output.push_str(&format!("Config: {}\n", self.config_path));
        output
    }
}

/// Build the CLI report without mutating config or scheduler state.
pub fn build_schedule_report(
    config_path: impl AsRef<Path>,
    state_path: impl AsRef<Path>,
    now: ScheduleNow,
) -> ScheduleReport {
    let config_path = config_path.as_ref();
    let state_path = state_path.as_ref();
    let config_path_display = config_path.display().to_string();
    let raw = match load_raw_config(config_path) {
        Ok(None) => {
            return ScheduleReport {
                rows: Vec::new(),
                diagnostics: Vec::new(),
                exit_code: 0,
                config_path: config_path_display,
                no_config: true,
            };
        }
        Ok(Some(raw)) => raw,
        Err(diagnostic) => {
            return ScheduleReport {
                rows: Vec::new(),
                diagnostics: vec![diagnostic],
                exit_code: 1,
                config_path: config_path_display,
                no_config: false,
            };
        }
    };
    if raw
        .keys()
        .all(|name| RESERVED_METADATA_KEYS.contains(&name.as_str()))
    {
        return ScheduleReport {
            rows: Vec::new(),
            diagnostics: Vec::new(),
            exit_code: 0,
            config_path: config_path_display,
            no_config: true,
        };
    }

    let (state, state_diagnostic) = load_report_state(state_path);
    let state_trusted = state_diagnostic.is_none();
    let config = metadata_from_raw(&raw);
    let mut diagnostics = state_diagnostic.into_iter().collect::<Vec<_>>();
    let mut rows = Vec::new();
    for (name, value) in raw {
        if RESERVED_METADATA_KEYS.contains(&name.as_str()) {
            continue;
        }
        let Value::Object(entry) = value else {
            diagnostics.push(format!("schedule '{name}' is not a valid entry"));
            continue;
        };
        let disabled = !entry.get("enabled").map(json_truthy).unwrap_or(true);
        let (cmd, command) = classify_command(&entry);
        if command.is_none() {
            diagnostics.push(format!("schedule '{name}' has invalid cmd"));
        }
        let raw_every = entry.get("every").and_then(Value::as_str);
        let supported = raw_every.is_some_and(is_interval);
        let every = raw_every
            .map(|value| {
                if supported {
                    effective_every(value)
                } else {
                    value.to_owned()
                }
            })
            .unwrap_or_else(|| "?".to_owned());
        let last_run = if state_trusted {
            format_last_run(state_entry(&state, &name))
        } else {
            "invalid".to_owned()
        };
        let next_due = if disabled {
            "disabled".to_owned()
        } else if command.is_none() || !supported {
            "?".to_owned()
        } else if !state_trusted {
            "invalid".to_owned()
        } else {
            let entry = ScheduleEntry {
                cmd: command.expect("checked command"),
                every: raw_every.expect("supported interval").to_owned(),
                max_runtime: None,
            };
            format_next_due(&entry, state_entry(&state, &name), &config, now)
        };
        rows.push(ScheduleReportRow {
            name,
            every,
            last_run,
            next_due,
            cmd,
            disabled,
        });
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    ScheduleReport {
        rows,
        exit_code: (!diagnostics.is_empty()).into(),
        diagnostics,
        config_path: config_path_display,
        no_config: false,
    }
}

fn load_raw_config(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(format!(
                "schedules config at {} is unreadable",
                path.display()
            ));
        }
    }
    match read_json::<Value>(path, Value::Null, MalformedPolicy::Raise) {
        Ok(Value::Object(raw)) => Ok(Some(raw)),
        Ok(_) => Err(format!(
            "schedules config at {} must be a JSON object",
            path.display()
        )),
        Err(ReadError::Malformed(_)) => Err(format!(
            "schedules config at {} is malformed",
            path.display()
        )),
        Err(ReadError::Io { .. }) => Err(format!(
            "schedules config at {} is unreadable",
            path.display()
        )),
    }
}

fn load_report_state(path: &Path) -> (Map<String, Value>, Option<String>) {
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Map::new(), None),
        Err(_) => {
            return (
                Map::new(),
                Some(format!(
                    "scheduler state at {} is unreadable",
                    path.display()
                )),
            );
        }
    }
    match read_json::<Value>(path, Value::Object(Map::new()), MalformedPolicy::Raise) {
        Ok(Value::Object(state)) => (state, None),
        Ok(_) => (
            Map::new(),
            Some(format!(
                "scheduler state at {} must be a JSON object",
                path.display()
            )),
        ),
        Err(ReadError::Malformed(_)) => (
            Map::new(),
            Some(format!(
                "scheduler state at {} is malformed",
                path.display()
            )),
        ),
        Err(ReadError::Io { .. }) => (
            Map::new(),
            Some(format!(
                "scheduler state at {} is unreadable",
                path.display()
            )),
        ),
    }
}

fn classify_command(entry: &Map<String, Value>) -> (String, Option<Vec<String>>) {
    match entry.get("cmd") {
        Some(Value::String(command)) => (command.clone(), Some(vec![command.clone()])),
        Some(Value::Array(command)) if command.iter().all(Value::is_string) => {
            let command = command
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (command.join(" "), Some(command))
        }
        _ => ("<invalid>".to_owned(), None),
    }
}

fn format_last_run(state: Option<&Value>) -> String {
    let Some(last_run) = state
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_run"))
    else {
        return "never".to_owned();
    };
    if last_run.is_null() {
        return "never".to_owned();
    }
    last_run
        .as_f64()
        .map(format_epoch)
        .unwrap_or_else(|| "invalid".to_owned())
}

fn format_epoch(value: f64) -> String {
    local_from_epoch(value)
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "invalid".to_owned())
}

fn format_next_due(
    entry: &ScheduleEntry,
    state: Option<&Value>,
    config: &ScheduleConfig,
    now: ScheduleNow,
) -> String {
    if super::is_due(entry, state, config, now) {
        return "now".to_owned();
    }
    let epoch_millis = compute_next_run(entry, state, config, now);
    local_from_epoch(epoch_millis as f64 / 1_000.0)
        .map(|value| match entry.every.as_str() {
            "hourly" => value.format("%H:%M").to_string(),
            "daily" => config
                .daily_time
                .as_deref()
                .and_then(parse_daily_time)
                .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
                .unwrap_or_else(|| "midnight".to_owned()),
            "weekly" => value.format("%A %H:%M").to_string(),
            _ => value.format("%H:%M").to_string(),
        })
        .unwrap_or_else(|| "invalid".to_owned())
}

#[cfg(test)]
mod tests {
    use super::format_epoch;

    #[test]
    fn non_finite_epoch_values_are_invalid() {
        assert_eq!(format_epoch(f64::NAN), "invalid");
        assert_eq!(format_epoch(f64::INFINITY), "invalid");
    }
}
