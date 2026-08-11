// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
const CANT_TELL: &str = "re-run journal doctor; check the health logs if it persists";
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let source = solstone_core_system_health::FilesystemHealthLogSource::new(&context.journal_path);
    let segments = solstone_core_system_health::FilesystemSegmentSource;
    match solstone_core_system_health::read_backlog_view(
        &source,
        &segments,
        &context.journal_path,
        solstone_core_system_health::BACKLOG_DEFAULT_WINDOW,
        context.now,
    ) {
        Err(error) => Ok(make_result(
            check,
            Status::Warn,
            format!("couldn't fully determine — backlog read failed: {error}"),
            Some(CANT_TELL),
        )),
        Ok(view)
            if !view.errors.is_empty()
                || view
                    .days
                    .iter()
                    .any(|day| day.state == solstone_core_system_health::BACKLOG_STATE_UNKNOWN) =>
        {
            let unknown = view
                .days
                .iter()
                .filter(|day| day.state == solstone_core_system_health::BACKLOG_STATE_UNKNOWN)
                .count();
            Ok(make_result(
                check,
                Status::Warn,
                format!("couldn't fully determine — {unknown} day(s) unknown"),
                Some(CANT_TELL),
            ))
        }
        Ok(view) if view.pending_days == 0 && view.stuck_days == 0 => {
            let capped = view
                .days
                .iter()
                .filter(|day| day.capped_daily.is_some())
                .count();
            let detail = if capped == 0 {
                "caught up".to_owned()
            } else {
                format!("caught up; {capped} day(s) completed with capped daily unit(s)")
            };
            Ok(make_result(check, Status::Ok, detail, None::<String>))
        }
        Ok(view) => {
            let mut detail = format!(
                "{} day(s) pending, {} day(s) stuck",
                view.pending_days, view.stuck_days
            );
            if let Some(day) = view.oldest_pending_day {
                detail.push_str(&format!("; oldest outstanding {day}"));
            }
            Ok(make_result(
                check,
                Status::Warn,
                detail,
                Some(
                    "solstone catches up on its own; reprocess a day from the health surface to prioritize it",
                ),
            ))
        }
    }
}
