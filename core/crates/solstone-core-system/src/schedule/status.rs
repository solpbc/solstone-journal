// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// Scheduler status projection for an enabled entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleStatus {
    pub name: String,
    pub every: String,
    pub last_run: Option<f64>,
    pub due: bool,
    pub next_run: i64,
    pub daily_time: Option<String>,
    pub weekly_day: Option<String>,
    pub weekly_time: Option<String>,
}
