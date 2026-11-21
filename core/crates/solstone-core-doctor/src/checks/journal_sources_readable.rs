// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Days the statistics scan could not read.
//!
//! The scan records damage per day and keeps going, so without a check the
//! record is a field in a document nobody opens. `segment_fold_failed_days`
//! is exactly that today — written since the schema had the field, read by
//! nothing — so this check covers both degradations rather than adding a
//! second unread one.

use solstone_core_cli_copy as copy;

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
fn first_cause(value: Option<&Value>) -> Option<(Option<&str>, &str)> {
    let Some(Value::Array(rows)) = value else {
        return None;
    };
    let row = rows.first()?;
    let cause = row.get("cause").and_then(Value::as_str)?;
    let day = row.get("day").and_then(Value::as_str);
    Some((day, cause))
}

fn listed(days: &[String]) -> String {
    let shown = days
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if days.len() > SHOWN {
        copy::display(
            "journal.cli.doctor.sources.days.more",
            [("days", &shown), ("more", &(days.len() - SHOWN))],
        )
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
            copy::text("journal.cli.doctor.journal.sources.readable.stats.missing"),
            None::<String>,
        ));
    };
    let Ok(document) = serde_json::from_slice::<Value>(&bytes) else {
        // The read succeeded; the parse did not.
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.journal.sources.readable.stats.invalid"),
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
    // Stale is a third kind of cannot-tell.  A document from last week parses
    // cleanly and would otherwise report a confident ok about days it never saw.
    if let Some(age) = document
        .get("generated_at")
        .and_then(Value::as_str)
        .and_then(|value| solstone_core_time::parse_rfc3339(value).ok())
        .map(|generated| context.now.duration_since(generated.timestamp()))
        && age.as_hours() > MAX_AGE_HOURS
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
            copy::display(
                "journal.cli.doctor.sources.statistics.stale",
                [("hours", &age.as_hours())],
            ),
            Some(copy::text("journal.cli.doctor.sources.statistics.refresh")),
        ));
    }

    if document.get("evidence_unreadable_days").is_none() {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.journal.sources.readable.stats.predates"),
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
            copy::text("journal.cli.doctor.journal.sources.readable.healthy"),
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
        let line = if let Some((day, cause)) = first_cause(document.get("evidence_unreadable_days"))
        {
            let cause = truncate(cause, 400);
            named_a_file = cause.contains(std::path::MAIN_SEPARATOR);
            match day {
                Some(day) => context.report_form.counted(
                    "journal.cli.doctor.sources.unreadable.cause",
                    &[("quantity", unreadable.len() as u64)],
                    &[
                        ("count", &unreadable.len()),
                        ("days", &(listed(&unreadable))),
                        ("day", &day),
                        ("cause", &cause),
                    ],
                ),
                None => context.report_form.counted(
                    "journal.cli.doctor.sources.unreadable.undated",
                    &[("quantity", unreadable.len() as u64)],
                    &[
                        ("count", &unreadable.len()),
                        ("days", &(listed(&unreadable))),
                        ("cause", &cause),
                    ],
                ),
            }
        } else {
            context.report_form.counted(
                "journal.cli.doctor.sources.unreadable",
                &[("quantity", unreadable.len() as u64)],
                &[
                    ("count", &unreadable.len()),
                    ("days", &(listed(&unreadable))),
                ],
            )
        };
        parts.push(line);
    }
    if !fold_failed.is_empty() {
        parts.push(context.report_form.counted(
            "journal.cli.doctor.sources.fold.failed",
            &[("quantity", fold_failed.len() as u64)],
            &[
                ("count", &fold_failed.len()),
                ("days", &(listed(&fold_failed))),
            ],
        ));
    }
    // ⛔ `is_day_key` admits ANY eight ascii digits, so a folder named
    // `20260230` enumerates and reaches this list. `journal reprocess` parses
    // its argument as a calendar date and refuses with "expected day in
    // YYYYMMDD format" -- which reads as the OWNER's typo, about a day this
    // very line handed them. Do not send anyone there for a day that cannot
    // parse.
    let every_day_is_a_date = unreadable
        .iter()
        .chain(fold_failed.iter())
        .all(|day| solstone_core_time::parse_day(day).is_some());
    Ok(make_result(
        check,
        Status::Warn,
        parts.join("; "),
        Some(match (every_day_is_a_date, named_a_file) {
            (false, _) => copy::text("journal.cli.doctor.journal.sources.readable.inspect.folders"),
            (true, true) => copy::text("journal.cli.doctor.journal.sources.readable.repair.files"),
            (true, false) => copy::text("journal.cli.doctor.journal.sources.readable.reprocess"),
        }),
    ))
}
