// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let records = match common::observers(context) {
        Ok(records) => records,
        Err(_) => {
            return Ok(make_result(
                check,
                Status::Skip,
                "rollup=unknown; observer records unavailable",
                None::<String>,
            ));
        }
    };
    let records = common::enabled(records);
    if records.is_empty() {
        return Ok(make_result(
            check,
            Status::Skip,
            "rollup=no_observers; no registered observers",
            None::<String>,
        ));
    }
    let mut states = Vec::new();
    for record in &records {
        let age = record
            .last_seen()
            .map(|value| context.now.timestamp_millis() - value);
        let state = if record.ingest_rejection().is_some() {
            "degraded"
        } else if age.is_some_and(|age| age < 30_000) {
            "active"
        } else if age.is_some_and(|age| age < 120_000) {
            "stale"
        } else {
            "offline"
        };
        states.push((record.name().unwrap_or("unknown"), state));
    }
    if states.iter().any(|(_, state)| *state == "active")
        && !states.iter().any(|(_, state)| *state == "degraded")
    {
        return Ok(make_result(
            check,
            Status::Ok,
            "rollup=active; observers reaching the journal",
            None::<String>,
        ));
    }
    let rollup = if states.iter().any(|(_, state)| *state == "degraded") {
        "degraded"
    } else if states.iter().any(|(_, state)| *state == "stale") {
        "stale"
    } else {
        "offline"
    };
    let summary = states
        .iter()
        .take(3)
        .map(|(name, state)| format!("{name}={state}"))
        .chain((states.len() > 3).then(|| format!("+{} more", states.len() - 3)))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(make_result(
        check,
        Status::Warn,
        truncate(&format!("rollup={rollup}; observers: {summary}"), 400),
        Some("open /app/health to inspect observer health"),
    ))
}
