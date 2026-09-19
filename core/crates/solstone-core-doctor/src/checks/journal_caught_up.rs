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
            if capped == 0 {
                Ok(make_result(
                    check,
                    Status::Ok,
                    "caught up".to_owned(),
                    None::<String>,
                ))
            } else {
                Ok(make_result(
                    check,
                    Status::Warn,
                    format!("caught up; {capped} day(s) completed with capped daily unit(s)"),
                    None::<String>,
                ))
            }
        }
        Ok(view) => {
            let mut detail = format!(
                "{} day(s) pending, {} day(s) stuck",
                view.pending_days, view.stuck_days
            );
            if let Some(day) = view.oldest_pending_day {
                detail.push_str(&format!("; oldest outstanding {day}"));
            }
            let review_unit = view
                .days
                .iter()
                .flat_map(|day| {
                    day.why.iter().filter_map(move |unit| {
                        if unit.name != "entities:entities_review" {
                            return None;
                        }
                        let severity = match unit.lifecycle_state.as_deref() {
                            Some("ambiguous_started") => 3u8,
                            Some("exhausted") => 2,
                            Some("retrying") => 1,
                            _ => return None,
                        };
                        Some((severity, day.day.as_str(), unit))
                    })
                })
                .max_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(left.1)));
            let location = |day: &str, facet: Option<&str>| match facet
                .map(str::trim)
                .filter(|facet| !facet.is_empty())
            {
                Some(facet) => format!("{day}/{facet}"),
                None => day.to_owned(),
            };
            let fix = match review_unit {
                Some((_, day, unit))
                    if unit.lifecycle_state.as_deref() == Some("ambiguous_started") =>
                {
                    let loc = location(day, unit.facet.as_deref());
                    format!(
                        "entities:entities_review may have started an entity review change but did not confirm completion on {loc}; resolve the in-progress change before reprocessing"
                    )
                }
                Some((_, day, unit)) if unit.lifecycle_state.as_deref() == Some("exhausted") => {
                    let loc = location(day, unit.facet.as_deref());
                    let kind = unit.owner_conflict_kind.as_deref().unwrap_or("unknown");
                    format!(
                        "entities:entities_review stopped on {loc} with reason {kind} after its automatic retry; run journal reprocess {day} --from-scratch"
                    )
                }
                Some((_, day, unit)) if unit.lifecycle_state.as_deref() == Some("retrying") => {
                    let loc = location(day, unit.facet.as_deref());
                    let kind = unit.owner_conflict_kind.as_deref().unwrap_or("unknown");
                    format!(
                        "entities:entities_review stopped on {loc} with reason {kind}; it will retry automatically on the next run"
                    )
                }
                _ => "solstone catches up on its own; reprocess a day from the health surface to prioritize it".to_owned(),
            };
            Ok(make_result(check, Status::Warn, detail, Some(fix)))
        }
    }
}
