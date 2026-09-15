// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Days the statistics scan could not read.
//!
//! The scan records damage per day and keeps going, so without a check the
//! record is a field in a document nobody opens. `segment_fold_failed_days`
//! is exactly that today — written since the schema had the field, read by
//! nothing — so this check covers both degradations rather than adding a
//! second unread one.

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use serde_json::Value;

const SHOWN: usize = 5;
/// Matches the freshness bound the home health glance already applies to this
/// same document, rather than inventing a second one.
const MAX_AGE_HOURS: i64 = 36;

fn day_names(value: Option<&Value>, key: Option<&str>) -> Vec<String> {
    let Some(Value::Array(rows)) = value else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| match key {
            Some(key) => row.get(key).and_then(Value::as_str).map(str::to_owned),
            None => row.as_str().map(str::to_owned),
        })
        .collect()
}

/// The first recorded cause, so the owner is handed the file to look at rather
/// than sent somewhere else to find it.
fn first_cause(value: Option<&Value>) -> Option<String> {
    let Some(Value::Array(rows)) = value else {
        return None;
    };
    rows.first()
        .and_then(|row| row.get("cause"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn listed(days: &[String]) -> String {
    let shown = days
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if days.len() > SHOWN {
        format!("{shown}, and {} more", days.len() - SHOWN)
    } else {
        shown
    }
}

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let path = context.journal_path.join("stats.json");
    // ⛔ An absent or unparseable document is not a clean bill of health: the
    // check could not look. Unknown is not no.
    let Ok(bytes) = std::fs::read(&path) else {
        // ⛔ Do not say "never written": a read also fails on a permission or
        // I/O error, and this arm cannot tell those apart.
        return Ok(make_result(
            check,
            Status::Skip,
            "couldn't check — no statistics to read",
            None::<String>,
        ));
    };
    let Ok(document) = serde_json::from_slice::<Value>(&bytes) else {
        // The read succeeded; the parse did not.
        return Ok(make_result(
            check,
            Status::Skip,
            "couldn't check — the statistics aren't readable",
            None::<String>,
        ));
    };
    // Stale is a third kind of cannot-tell.  A document from last week parses
    // cleanly and would otherwise report a confident ok about days it never saw.
    if let Some(age) = document
        .get("generated_at")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|generated| {
            context
                .now
                .signed_duration_since(generated.with_timezone(&chrono::Utc))
        })
        && age.num_hours() > MAX_AGE_HOURS
    {
        return Ok(make_result(
            check,
            Status::Skip,
            format!(
                "couldn't check — the statistics are {} hours old",
                age.num_hours()
            ),
            None::<String>,
        ));
    }

    let unreadable = day_names(document.get("evidence_unreadable_days"), Some("day"));
    let fold_failed = day_names(document.get("segment_fold_failed_days"), None);
    if unreadable.is_empty() && fold_failed.is_empty() {
        // ⛔ Claim only what was checked.  A day whose individual files fail to
        // read is skipped further upstream and counted as zero, so this is not
        // a statement that every byte is intact.
        return Ok(make_result(
            check,
            Status::Ok,
            "no unreadable days recorded",
            None::<String>,
        ));
    }

    let mut parts = Vec::new();
    if !unreadable.is_empty() {
        // Carry the cause.  ⛔ Do not send the owner elsewhere for a reason this
        // line is already holding: the verbose flag unfilters ok and skipped
        // rows, it does not surface a per-day reason.
        let mut line = format!(
            "{} day(s) hold a file the journal cannot read ({})",
            unreadable.len(),
            listed(&unreadable)
        );
        if let Some(cause) = first_cause(document.get("evidence_unreadable_days")) {
            line.push_str(&format!(": {cause}"));
        }
        parts.push(line);
    }
    if !fold_failed.is_empty() {
        parts.push(format!(
            "{} day(s) are missing part of their record ({})",
            fold_failed.len(),
            listed(&fold_failed)
        ));
    }
    Ok(make_result(
        check,
        Status::Warn,
        parts.join("; "),
        Some("repair or remove the file named above, then run journal reprocess <day>"),
    ))
}
