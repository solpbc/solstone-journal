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
    vocabulary::{Check, RunnerResult, Status, make_result, truncate},
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
    // ⛔ A document written before this record existed cannot carry the field,
    // so an absent list means "this document predates the record", never
    // "there is nothing to record". Keying on the field's PRESENCE rather than
    // on a schema number states the property directly and needs no second
    // source of truth for the version. Measured on a live host: an older
    // document made this check report a confident clean while five days were
    // unreadable.
    if document.get("evidence_unreadable_days").is_none() {
        return Ok(make_result(
            check,
            Status::Skip,
            "couldn't check — the statistics predate this version",
            None::<String>,
        ));
    }
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
        // ⛔ Warn, not Skip. `output.rs` renders only Fail and Warn without
        // `--verbose`, so a Skip here is invisible on a plain `journal doctor`
        // -- and stale statistics are the one cannot-tell that means this
        // check has quietly STOPPED protecting the owner, which is the exact
        // failure class it was added for. `journal_caught_up` routes its own
        // cannot-tell the same way.
        //
        // ⚠ The action does not promise that re-running doctor refreshes
        // these; nothing an owner types does. The daily lifecycle writes them.
        return Ok(make_result(
            check,
            Status::Warn,
            format!(
                "couldn't check — the statistics are {} hours old",
                age.num_hours()
            ),
            Some("the journal refreshes these daily; check the health logs if this persists"),
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
            "the last scan found no unreadable days",
            None::<String>,
        ));
    }

    let mut parts = Vec::new();
    // ⛔ Tracks whether the message actually NAMED a file. The fold-failed
    // branch never does, and an `evidence_unreadable_days` cause can be a
    // pathless validation error, so a fix line that says "the file named
    // above" is round 1's defect moved one hop: it points at something the
    // owner cannot see.
    let mut named_a_file = false;
    if !unreadable.is_empty() {
        // Carry the cause.  ⛔ Do not send the owner elsewhere for a reason this
        // line is already holding: the verbose flag unfilters ok and skipped
        // rows, it does not surface a per-day reason.
        let mut line = format!(
            "{} day(s) hold a file that couldn't be read ({})",
            unreadable.len(),
            listed(&unreadable)
        );
        // ⚠ `cause` is an internal error string of unbounded length. Bound it
        // the way every sibling check bounds owner-visible detail.
        if let Some(cause) = first_cause(document.get("evidence_unreadable_days")) {
            line.push_str(&format!(": {}", truncate(&cause, 400)));
            named_a_file = true;
        }
        parts.push(line);
    }
    if !fold_failed.is_empty() {
        parts.push(format!(
            "{} day(s) didn't finish processing ({})",
            fold_failed.len(),
            listed(&fold_failed)
        ));
    }
    Ok(make_result(
        check,
        Status::Warn,
        parts.join("; "),
        Some(if named_a_file {
            "repair or remove the file named above, then run journal reprocess <day>"
        } else {
            "run journal reprocess <day>"
        }),
    ))
}
