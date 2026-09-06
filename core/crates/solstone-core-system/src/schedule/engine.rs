// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{NaiveDateTime, Timelike};
use serde_json::{Map, Value};

use crate::request::{ScheduledArgv, ScheduledRequest};

use super::completion::{load_runtime_state, record_completion};
use super::config::{ConfigDiagnostic, load_runtime, minute_interval, register_default_entries};
use super::due::{compute_next_run, current_marks, effective_every, is_due, state_entry};
use super::status::ScheduleStatus;
use super::{ScheduleConfig, ScheduleError, ScheduleNow, ScheduleSubmissionSink};

/// Result of one edge-triggered scheduler check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub submitted: Vec<String>,
    pub retried: Vec<String>,
    pub diagnostics: Vec<ConfigDiagnostic>,
}

/// Result of one level-triggered startup catch-up pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatchUpReport {
    pub submitted: Vec<String>,
}

/// Loaded scheduler configuration, completion state, and edge-trigger markers.
pub struct ScheduleEngine {
    config_path: PathBuf,
    state_path: PathBuf,
    config: ScheduleConfig,
    state: Map<String, Value>,
    last_minute: NaiveDateTime,
    last_hour: NaiveDateTime,
    last_daily_mark: NaiveDateTime,
    last_weekly_mark: NaiveDateTime,
    pending_retry: BTreeSet<String>,
    completion_lock: Mutex<()>,
}

impl ScheduleEngine {
    /// Load tolerant runtime configuration and state, baselining marks to `now`.
    pub fn init(
        config_path: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        now: ScheduleNow,
    ) -> Result<(Self, Vec<ConfigDiagnostic>), ScheduleError> {
        let config_path = config_path.as_ref().to_path_buf();
        let state_path = state_path.as_ref().to_path_buf();
        let loaded = load_runtime(&config_path)?;
        let state = load_runtime_state(&state_path)?;
        let (last_hour, last_daily_mark, last_weekly_mark) =
            current_marks(&loaded.config, now.local);
        let last_minute = now
            .local
            .date()
            .and_hms_opt(now.local.hour(), now.local.minute(), 0)
            .expect("valid current minute");
        Ok((
            Self {
                config_path,
                state_path,
                config: loaded.config,
                state,
                last_minute,
                last_hour,
                last_daily_mark,
                last_weekly_mark,
                pending_retry: BTreeSet::new(),
                completion_lock: Mutex::new(()),
            },
            loaded.diagnostics,
        ))
    }

    /// Strictly add missing built-ins, preserving disabled raw defaults.
    /// Returns the names that were added.
    pub fn register_defaults(&mut self) -> Result<Vec<String>, ScheduleError> {
        let added = register_default_entries(&self.config_path)?;
        if !added.is_empty() {
            self.config = load_runtime(&self.config_path)?.config;
        }
        Ok(added)
    }

    /// Retry pending coarse emissions and submit entries crossing a new boundary.
    pub fn check(
        &mut self,
        now: ScheduleNow,
        sink: &dyn ScheduleSubmissionSink,
    ) -> Result<CheckReport, ScheduleError> {
        let current_minute = now
            .local
            .date()
            .and_hms_opt(now.local.hour(), now.local.minute(), 0)
            .expect("valid current minute");
        let (current_hour, current_daily_mark, current_weekly_mark) =
            current_marks(&self.config, now.local);
        let hour_changed = current_hour != self.last_hour;
        let mut daily_changed = current_daily_mark != self.last_daily_mark;
        let mut weekly_changed = current_weekly_mark != self.last_weekly_mark;
        let minute_changed = current_minute != self.last_minute;
        if !hour_changed && !daily_changed && !weekly_changed && !minute_changed {
            return Ok(CheckReport::default());
        }

        let mut diagnostics = Vec::new();
        if hour_changed || daily_changed || weekly_changed {
            let loaded = load_runtime(&self.config_path)?;
            self.config = loaded.config;
            diagnostics = loaded.diagnostics;
        }
        self.state = load_runtime_state(&self.state_path)?;
        self.last_minute = current_minute;
        self.last_hour = current_hour;
        let (_, new_daily_mark, new_weekly_mark) = current_marks(&self.config, now.local);
        daily_changed |= new_daily_mark != self.last_daily_mark;
        weekly_changed |= new_weekly_mark != self.last_weekly_mark;
        self.last_daily_mark = new_daily_mark;
        self.last_weekly_mark = new_weekly_mark;

        let mut report = CheckReport {
            diagnostics,
            ..CheckReport::default()
        };
        let mut attempted = BTreeSet::new();
        for name in self.pending_retry.clone() {
            let Some(entry) = self.config.entries.get(&name).cloned() else {
                self.pending_retry.remove(&name);
                continue;
            };
            if !is_due(&entry, state_entry(&self.state, &name), &self.config, now) {
                self.pending_retry.remove(&name);
                continue;
            }
            attempted.insert(name.clone());
            if submit(&name, &entry, now, sink) {
                self.pending_retry.remove(&name);
                report.retried.push(name);
            }
        }

        for (name, entry) in self.config.entries.clone() {
            if attempted.contains(&name)
                || !matches_boundary(
                    &entry.every,
                    hour_changed,
                    daily_changed,
                    weekly_changed,
                    minute_changed,
                )
            {
                continue;
            }
            if !is_due(&entry, state_entry(&self.state, &name), &self.config, now) {
                continue;
            }
            if submit(&name, &entry, now, sink) {
                report.submitted.push(name);
            } else if minute_interval(&entry.every).is_none() {
                self.pending_retry.insert(name);
            }
        }
        Ok(report)
    }

    /// Submit every currently due entry at most once, without changing edge markers.
    ///
    /// `fresh` names entries that did not exist before this boot. Catch-up replays
    /// work that was missed while the scheduler was down; an entry that has never
    /// been scheduled was not missed, so it starts its cadence at its next mark
    /// exactly as it would had it been added to a running scheduler.
    pub fn catch_up(
        &mut self,
        now: ScheduleNow,
        sink: &dyn ScheduleSubmissionSink,
        fresh: &BTreeSet<String>,
    ) -> CatchUpReport {
        let mut report = CatchUpReport::default();
        for (name, entry) in self.config.entries.clone() {
            if fresh.contains(&name) {
                continue;
            }
            if is_due(&entry, state_entry(&self.state, &name), &self.config, now)
                && submit(&name, &entry, now, sink)
            {
                report.submitted.push(name);
            }
        }
        report
    }

    /// Project loaded schedule state for callers that publish status.
    pub fn collect_status(&self, now: ScheduleNow) -> Vec<ScheduleStatus> {
        self.config
            .entries
            .iter()
            .map(|(name, entry)| {
                let state = state_entry(&self.state, name);
                let last_run = state
                    .and_then(Value::as_object)
                    .and_then(|entry| entry.get("last_run"))
                    .and_then(Value::as_f64);
                ScheduleStatus {
                    name: name.clone(),
                    every: effective_every(&entry.every),
                    last_run,
                    due: is_due(entry, state, &self.config, now),
                    next_run: compute_next_run(entry, state, &self.config, now),
                    daily_time: (entry.every == "daily")
                        .then(|| {
                            self.config
                                .daily_time
                                .clone()
                                .filter(|value| !value.is_empty())
                        })
                        .flatten(),
                    weekly_day: (entry.every == "weekly")
                        .then(|| self.config.weekly_day.clone())
                        .flatten(),
                    weekly_time: (entry.every == "weekly")
                        .then(|| {
                            self.config
                                .weekly_time
                                .clone()
                                .filter(|value| !value.is_empty())
                        })
                        .flatten(),
                }
            })
            .collect()
    }

    /// Persist one scheduler completion under the engine's in-process mutex.
    pub fn record_completion(
        &self,
        name: &str,
        ended_at: f64,
        exit_status: &str,
        reference: &str,
    ) -> Result<(), ScheduleError> {
        record_completion(
            &self.completion_lock,
            &self.state_path,
            name,
            ended_at,
            exit_status,
            reference,
        )
    }
}

fn matches_boundary(every: &str, hour: bool, daily: bool, weekly: bool, minute: bool) -> bool {
    match every {
        "hourly" => hour,
        "daily" => daily,
        "weekly" => weekly,
        _ => minute_interval(every).is_some() && minute,
    }
}

fn submit(
    name: &str,
    entry: &super::ScheduleEntry,
    now: ScheduleNow,
    sink: &dyn ScheduleSubmissionSink,
) -> bool {
    let command = ScheduledArgv::from_wire(entry.cmd.clone()).expect("validated non-empty command");
    let mut request =
        ScheduledRequest::new(command, format!("sched:{name}:{}", now.unix_millis), name);
    request.max_runtime = entry.max_runtime;
    sink.submit(request)
}
