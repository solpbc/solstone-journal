// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};

/// Newest chronicle days read.
const RECENT_DAYS: usize = 2;
/// Unrouted share of active segments above which the check warns. Healthy routing
/// files every active segment under a facet; 2026-09-20's regression reached 84%.
const WARN_PERCENT: usize = 10;

/// Active segments whose Sense output routed them to no facet. The owner's activity
/// lists are built from those routes, so an unrouted segment is missing from them.
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let chronicle = context.journal_path.join("chronicle");
    let mut days = fs::read_dir(&chronicle)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.len() == 8 && name.bytes().all(|byte| byte.is_ascii_digit()))
        .collect::<Vec<_>>();
    days.sort();
    let days = &days[days.len().saturating_sub(RECENT_DAYS)..];
    let (mut active, mut unrouted) = (0usize, 0usize);
    for day in days {
        for (is_active, routed) in segment_routes(&chronicle.join(day)) {
            if is_active {
                active += 1;
                unrouted += usize::from(!routed);
            }
        }
    }
    if active == 0 {
        return Ok(make_result(
            check,
            Status::Skip,
            "no active segments in the newest days",
            None::<String>,
        ));
    }
    let since = days.first().map(String::as_str).unwrap_or_default();
    if unrouted * 100 > active * WARN_PERCENT {
        Ok(make_result(
            check,
            Status::Warn,
            format!(
                "{unrouted} of {active} active segments since {since} were filed under no facet"
            ),
            Some(
                "those segments are missing from the activity lists; check the facet declarations with `journal facet doctor` and recent sense runs with `journal talent logs`",
            ),
        ))
    } else {
        Ok(make_result(
            check,
            Status::Ok,
            format!(
                "{} of {active} active segments since {since} filed under a facet",
                active - unrouted
            ),
            None::<String>,
        ))
    }
}

/// `(active, routed)` for every sensed segment under one day directory.
fn segment_routes(day: &Path) -> Vec<(bool, bool)> {
    let mut routes = Vec::new();
    for stream in fs::read_dir(day)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        for segment in fs::read_dir(stream.path())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let talents = segment.path().join("talents");
            let Some(sense) = read_json(&talents.join("sense.json")) else {
                continue;
            };
            let is_active = sense.get("density").and_then(Value::as_str) == Some("active");
            let routed = read_json(&talents.join("facets.json"))
                .and_then(|facets| facets.as_array().map(|array| !array.is_empty()))
                .unwrap_or(false);
            routes.push((is_active, routed));
        }
    }
    routes
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
