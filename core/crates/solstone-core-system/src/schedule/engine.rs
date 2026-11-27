// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SUBMITTED_PERIOD_CAPACITY: usize = 512;

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde_json::{Map, Value};

use crate::request::{ScheduledArgv, ScheduledRequest};

use super::completion::{load_runtime_state, record_completion, record_completion_checked};
use super::config::{ConfigDiagnostic, load_runtime, minute_interval, register_default_entries};
use super::due::{
    compute_next_run, current_marks, effective_daily_spec, effective_every, effective_weekly_spec,
    is_due, local_from_epoch, period_key, retry_at, state_entry,
};
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
    zone: TimeZone,
    last_minute: DateTime,
    last_hour: DateTime,
    last_daily_mark: DateTime,
    last_weekly_mark: DateTime,
    pending_retry: BTreeSet<String>,
    /// The failed `last_run` each entry's retry was submitted for, so a retry
    /// still running is not submitted again on the next minute.
    failure_retried: BTreeMap<String, u64>,
    /// Period key captured when each scheduled request was submitted, keyed by
    /// request reference, so editing `daily_time` or the weekly window while a
    /// batch is still running does not misclassify in-flight completions.
    submitted_periods: Mutex<VecDeque<(String, String)>>,
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
        let last_minute = minute_mark(now.local);
        Ok((
            Self {
                config_path,
                state_path,
                config: loaded.config,
                state,
                zone: now.zone,
                last_minute,
                last_hour,
                last_daily_mark,
                last_weekly_mark,
                pending_retry: BTreeSet::new(),
                failure_retried: BTreeMap::new(),
                submitted_periods: Mutex::new(VecDeque::new()),
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
        // Owner edits take effect at the next tick, including disabling or
        // removing an interval job between hourly boundaries.
        let prev_daily_spec = effective_daily_spec(&self.config);
        let prev_weekly_spec = effective_weekly_spec(&self.config);
        let loaded = load_runtime(&self.config_path)?;
        self.config = loaded.config;
        self.zone = now.zone.clone();
        let diagnostics = loaded.diagnostics;
        self.pending_retry
            .retain(|name| self.config.entries.contains_key(name));
        let current_minute = minute_mark(now.local);
        let (current_hour, current_daily_mark, current_weekly_mark) =
            current_marks(&self.config, now.local);
        // Changing daily_time or weekly_day/weekly_time shifts the configured
        // window; it is not the wall clock crossing a boundary, so rebase the
        // mark rather than firing every entry on the edit tick.
        if effective_daily_spec(&self.config) != prev_daily_spec {
            self.last_daily_mark = current_daily_mark;
        }
        if effective_weekly_spec(&self.config) != prev_weekly_spec {
            self.last_weekly_mark = current_weekly_mark;
        }
        let hour_changed = current_hour != self.last_hour;
        let daily_changed = current_daily_mark != self.last_daily_mark;
        let weekly_changed = current_weekly_mark != self.last_weekly_mark;
        let minute_changed = current_minute != self.last_minute;
        if !hour_changed && !daily_changed && !weekly_changed && !minute_changed {
            return Ok(CheckReport {
                diagnostics,
                ..CheckReport::default()
            });
        }

        self.state = load_runtime_state(&self.state_path)?;
        self.last_minute = current_minute;
        self.last_hour = current_hour;
        self.last_daily_mark = current_daily_mark;
        self.last_weekly_mark = current_weekly_mark;

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
            if !is_due(&entry, state_entry(&self.state, &name), &self.config, &now) {
                self.pending_retry.remove(&name);
                continue;
            }
            attempted.insert(name.clone());
            if self.submit(&name, &entry, &now, sink) {
                self.pending_retry.remove(&name);
                report.retried.push(name);
            }
        }

        for (name, entry) in self.config.entries.clone() {
            if attempted.contains(&name) {
                continue;
            }
            if !matches_boundary(
                &entry.every,
                hour_changed,
                daily_changed,
                weekly_changed,
                minute_changed,
            ) {
                if minute_changed && self.failure_retry_due(&name, &entry, &now) {
                    attempted.insert(name.clone());
                    if self.submit(&name, &entry, &now, sink) {
                        self.note_failure_retry(&name);
                        report.retried.push(name);
                    }
                }
                continue;
            }
            if !is_due(&entry, state_entry(&self.state, &name), &self.config, &now) {
                continue;
            }
            if self.submit(&name, &entry, &now, sink) {
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
            if is_due(&entry, state_entry(&self.state, &name), &self.config, &now)
                && self.submit(&name, &entry, &now, sink)
            {
                self.note_failure_retry(&name);
                report.submitted.push(name);
            }
        }
        report
    }

    /// A failed run's single retry is due and not already submitted.
    fn failure_retry_due(
        &self,
        name: &str,
        entry: &super::ScheduleEntry,
        now: &ScheduleNow,
    ) -> bool {
        let state = state_entry(&self.state, name);
        retry_at(entry, state, &self.config, now).is_some_and(|at| now.local >= at)
            && self.failure_retried.get(name) != failed_run_key(state).as_ref()
    }

    /// Remember which failed run a submission retried, when it retried one.
    fn note_failure_retry(&mut self, name: &str) {
        if let Some(key) = failed_run_key(state_entry(&self.state, name)) {
            self.failure_retried.insert(name.to_owned(), key);
        }
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
                    due: is_due(entry, state, &self.config, &now),
                    next_run: compute_next_run(entry, state, &self.config, &now),
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
        let last_period = self.completion_period(name, ended_at, reference);
        record_completion(
            &self.completion_lock,
            &self.state_path,
            name,
            ended_at,
            exit_status,
            reference,
            last_period.as_deref(),
        )
    }

    /// Same publication as [`Self::record_completion`], after the completion lock
    /// is held and immediately before the state file is replaced.
    pub fn record_completion_checked(
        &self,
        name: &str,
        ended_at: f64,
        exit_status: &str,
        reference: &str,
        checkpoint: &dyn Fn() -> Result<(), String>,
    ) -> Result<(), ScheduleError> {
        let last_period = self.completion_period(name, ended_at, reference);
        record_completion_checked(
            &self.completion_lock,
            &self.state_path,
            name,
            ended_at,
            exit_status,
            reference,
            last_period.as_deref(),
            checkpoint,
        )
    }

    fn submit(
        &self,
        name: &str,
        entry: &super::ScheduleEntry,
        now: &ScheduleNow,
        sink: &dyn ScheduleSubmissionSink,
    ) -> bool {
        let reference = format!("sched:{name}:{}", now.unix_millis);
        let command =
            ScheduledArgv::from_wire(entry.cmd.clone()).expect("validated non-empty command");
        let mut request = ScheduledRequest::new(command, reference.clone(), name);
        request.max_runtime = entry.max_runtime;
        if !sink.submit(request) {
            return false;
        }
        if let Some(period) = period_key(entry, &self.config, now.local) {
            let mut periods = self
                .submitted_periods
                .lock()
                .expect("submitted period cache poisoned");
            if let Some(pos) = periods.iter().position(|(key, _)| key == &reference) {
                periods.remove(pos);
            }
            if periods.len() >= SUBMITTED_PERIOD_CAPACITY {
                periods.pop_front();
            }
            periods.push_back((reference, period));
        }
        true
    }

    fn completion_period(&self, name: &str, ended_at: f64, reference: &str) -> Option<String> {
        {
            let mut periods = self
                .submitted_periods
                .lock()
                .expect("submitted period cache poisoned");
            if let Some(pos) = periods.iter().position(|(key, _)| key == reference) {
                return periods.remove(pos).map(|(_, period)| period);
            }
        }
        let entry = self.config.entries.get(name)?;
        let local = local_from_epoch(ended_at, &self.zone)?;
        period_key(entry, &self.config, local)
    }
}

fn failed_run_key(state: Option<&Value>) -> Option<u64> {
    state?
        .as_object()?
        .get("last_run")?
        .as_f64()
        .map(f64::to_bits)
}

fn matches_boundary(every: &str, hour: bool, daily: bool, weekly: bool, minute: bool) -> bool {
    match every {
        "hourly" => hour,
        "daily" => daily,
        "weekly" => weekly,
        _ => minute_interval(every).is_some() && minute,
    }
}

/// The start of the wall-clock minute `now` falls in.
fn minute_mark(now: DateTime) -> DateTime {
    now.date().at(now.hour(), now.minute(), 0, 0)
}
