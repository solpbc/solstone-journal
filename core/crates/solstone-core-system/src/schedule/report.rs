// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{fs, io, path::Path};

use jiff::civil::Weekday;
use jiff::tz::TimeZone;
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

fn cli_text(locale: &str, id: &str) -> String {
    solstone_core_cli_copy::render_for(locale, id, &[])
}

impl ScheduleReport {
    /// Render the legacy-compatible schedule display without diagnostics.
    pub fn render(&self) -> String {
        self.render_in(solstone_core_cli_copy::ENGLISH)
    }

    /// Render the schedule display in `locale` without diagnostics.
    pub fn render_in(&self, locale: &str) -> String {
        if self.no_config {
            let mut empty = solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.empty",
                &[
                    (
                        "path",
                        solstone_core_cli_copy::Value::String(&self.config_path),
                    ),
                    ("line_break", solstone_core_cli_copy::Value::String("\n")),
                ],
            );
            empty.push('\n');
            return empty;
        }
        let header_name = cli_text(locale, "journal.cli.schedule.header.name");
        let header_every = cli_text(locale, "journal.cli.schedule.header.every");
        let header_last_run = cli_text(locale, "journal.cli.schedule.header.last.run");
        let header_next_due = cli_text(locale, "journal.cli.schedule.header.next.due");
        let header_cmd = cli_text(locale, "journal.cli.schedule.header.cmd");
        let disabled_tag = cli_text(locale, "journal.cli.schedule.tag.disabled");
        let name_width = self
            .rows
            .iter()
            .map(|row| row.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(header_name.chars().count())
            .max(4);
        let every_width = header_every.chars().count().max(8);
        let last_run_width = header_last_run.chars().count().max(18);
        let next_due_width = header_next_due.chars().count().max(10);
        let mut output = format!(
            "  {:<name_width$}  {:<every_width$}  {:<last_run_width$}  {:<next_due_width$}  {}\n\n",
            header_name, header_every, header_last_run, header_next_due, header_cmd,
        );
        for row in &self.rows {
            let line = if row.disabled {
                format!(
                    "  {:<name_width$}  {:<every_width$}  {:<last_run_width$}  {:<next_due_width$}  {} {}",
                    row.name, row.every, row.last_run, row.next_due, row.cmd, disabled_tag,
                )
            } else {
                format!(
                    "  {:<name_width$}  {:<every_width$}  {:<last_run_width$}  {:<next_due_width$}  {}",
                    row.name, row.every, row.last_run, row.next_due, row.cmd,
                )
            };
            output.push_str(line.trim_end());
            output.push('\n');
        }
        output.push('\n');
        output.push_str(&solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.schedule.config",
            &[(
                "path",
                solstone_core_cli_copy::Value::String(&self.config_path),
            )],
        ));
        output.push('\n');
        output
    }
}

/// Build the CLI report without mutating config or scheduler state.
pub fn build_schedule_report(
    config_path: impl AsRef<Path>,
    state_path: impl AsRef<Path>,
    now: ScheduleNow,
) -> ScheduleReport {
    build_schedule_report_in(
        solstone_core_cli_copy::ENGLISH,
        config_path,
        state_path,
        now,
    )
}

/// Build the CLI report in `locale` without mutating config or scheduler state.
pub fn build_schedule_report_in(
    locale: &str,
    config_path: impl AsRef<Path>,
    state_path: impl AsRef<Path>,
    now: ScheduleNow,
) -> ScheduleReport {
    let config_path = config_path.as_ref();
    let state_path = state_path.as_ref();
    let config_path_display = config_path.display().to_string();
    let raw = match load_raw_config(locale, config_path) {
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

    let (state, state_diagnostic) = load_report_state(locale, state_path);
    let state_trusted = state_diagnostic.is_none();
    let config = metadata_from_raw(&raw);
    let mut diagnostics = state_diagnostic.into_iter().collect::<Vec<_>>();
    let mut rows = Vec::new();
    for (name, value) in raw {
        if RESERVED_METADATA_KEYS.contains(&name.as_str()) {
            continue;
        }
        let Value::Object(entry) = value else {
            diagnostics.push(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.entry.invalid",
                &[("name", solstone_core_cli_copy::Value::String(&name))],
            ));
            continue;
        };
        let disabled = !entry.get("enabled").map(json_truthy).unwrap_or(true);
        let (cmd, command) = classify_command(locale, &entry);
        if command.is_none() {
            diagnostics.push(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.cmd.invalid",
                &[("name", solstone_core_cli_copy::Value::String(&name))],
            ));
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
            format_last_run(locale, state_entry(&state, &name), &now.zone)
        } else {
            cli_text(locale, "journal.cli.schedule.cell.invalid")
        };
        let next_due = if disabled {
            cli_text(locale, "journal.cli.schedule.cell.disabled")
        } else if command.is_none() || !supported {
            "?".to_owned()
        } else if !state_trusted {
            cli_text(locale, "journal.cli.schedule.cell.invalid")
        } else {
            let entry = ScheduleEntry {
                cmd: command.expect("checked command"),
                every: raw_every.expect("supported interval").to_owned(),
                max_runtime: None,
            };
            format_next_due(locale, &entry, state_entry(&state, &name), &config, &now)
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

fn load_raw_config(locale: &str, path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let path_str = path.display().to_string();
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.config.unreadable",
                &[("path", solstone_core_cli_copy::Value::String(&path_str))],
            ));
        }
    }
    match read_json::<Value>(path, Value::Null, MalformedPolicy::Raise) {
        Ok(Value::Object(raw)) => Ok(Some(raw)),
        Ok(_) => Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.schedule.error.config.not.object",
            &[("path", solstone_core_cli_copy::Value::String(&path_str))],
        )),
        Err(ReadError::Malformed(_)) => Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.schedule.error.config.malformed",
            &[("path", solstone_core_cli_copy::Value::String(&path_str))],
        )),
        Err(ReadError::Io { .. }) => Err(solstone_core_cli_copy::render_for(
            locale,
            "journal.cli.schedule.error.config.unreadable",
            &[("path", solstone_core_cli_copy::Value::String(&path_str))],
        )),
    }
}

fn load_report_state(locale: &str, path: &Path) -> (Map<String, Value>, Option<String>) {
    let path_str = path.display().to_string();
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Map::new(), None),
        Err(_) => {
            return (
                Map::new(),
                Some(solstone_core_cli_copy::render_for(
                    locale,
                    "journal.cli.schedule.error.state.unreadable",
                    &[("path", solstone_core_cli_copy::Value::String(&path_str))],
                )),
            );
        }
    }
    match read_json::<Value>(path, Value::Object(Map::new()), MalformedPolicy::Raise) {
        Ok(Value::Object(state)) => (state, None),
        Ok(_) => (
            Map::new(),
            Some(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.state.not.object",
                &[("path", solstone_core_cli_copy::Value::String(&path_str))],
            )),
        ),
        Err(ReadError::Malformed(_)) => (
            Map::new(),
            Some(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.state.malformed",
                &[("path", solstone_core_cli_copy::Value::String(&path_str))],
            )),
        ),
        Err(ReadError::Io { .. }) => (
            Map::new(),
            Some(solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.schedule.error.state.unreadable",
                &[("path", solstone_core_cli_copy::Value::String(&path_str))],
            )),
        ),
    }
}

fn classify_command(locale: &str, entry: &Map<String, Value>) -> (String, Option<Vec<String>>) {
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
        _ => (
            format!(
                "<{}>",
                cli_text(locale, "journal.cli.schedule.cell.invalid")
            ),
            None,
        ),
    }
}

fn format_last_run(locale: &str, state: Option<&Value>, zone: &TimeZone) -> String {
    let Some(last_run) = state
        .and_then(Value::as_object)
        .and_then(|entry| entry.get("last_run"))
    else {
        return cli_text(locale, "journal.cli.schedule.cell.never");
    };
    if last_run.is_null() {
        return cli_text(locale, "journal.cli.schedule.cell.never");
    }
    last_run
        .as_f64()
        .map(|value| format_epoch_in(locale, value, zone))
        .unwrap_or_else(|| cli_text(locale, "journal.cli.schedule.cell.invalid"))
}

#[cfg(test)]
fn format_epoch(value: f64, zone: &TimeZone) -> String {
    format_epoch_in(solstone_core_cli_copy::ENGLISH, value, zone)
}

fn format_epoch_in(locale: &str, value: f64, zone: &TimeZone) -> String {
    local_from_epoch(value, zone)
        .map(|value| value.strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| cli_text(locale, "journal.cli.schedule.cell.invalid"))
}

fn weekly_id(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "journal.cli.schedule.weekly.monday",
        Weekday::Tuesday => "journal.cli.schedule.weekly.tuesday",
        Weekday::Wednesday => "journal.cli.schedule.weekly.wednesday",
        Weekday::Thursday => "journal.cli.schedule.weekly.thursday",
        Weekday::Friday => "journal.cli.schedule.weekly.friday",
        Weekday::Saturday => "journal.cli.schedule.weekly.saturday",
        Weekday::Sunday => "journal.cli.schedule.weekly.sunday",
    }
}

fn format_next_due(
    locale: &str,
    entry: &ScheduleEntry,
    state: Option<&Value>,
    config: &ScheduleConfig,
    now: &ScheduleNow,
) -> String {
    if super::is_due(entry, state, config, now) {
        return cli_text(locale, "journal.cli.schedule.cell.now");
    }
    let epoch_millis = compute_next_run(entry, state, config, now);
    let retrying = super::due::retry_at(entry, state, config, now).is_some();
    local_from_epoch(epoch_millis as f64 / 1_000.0, &now.zone)
        .map(|value| match entry.every.as_str() {
            "hourly" => value.strftime("%H:%M").to_string(),
            "daily" if retrying => value.strftime("%H:%M").to_string(),
            "daily" => config
                .daily_time
                .as_deref()
                .and_then(parse_daily_time)
                .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
                .unwrap_or_else(|| cli_text(locale, "journal.cli.schedule.cell.midnight")),
            "weekly" => {
                let time = value.strftime("%H:%M").to_string();
                solstone_core_cli_copy::render_for(
                    locale,
                    weekly_id(value.weekday()),
                    &[("time", solstone_core_cli_copy::Value::String(&time))],
                )
            }
            _ => value.strftime("%H:%M").to_string(),
        })
        .unwrap_or_else(|| cli_text(locale, "journal.cli.schedule.cell.invalid"))
}

#[cfg(test)]
mod tests {
    use super::format_epoch;

    #[test]
    fn non_finite_epoch_values_are_invalid() {
        assert_eq!(format_epoch(f64::NAN, &jiff::tz::TimeZone::UTC), "invalid");
        assert_eq!(
            format_epoch(f64::INFINITY, &jiff::tz::TimeZone::UTC),
            "invalid"
        );
    }
}
