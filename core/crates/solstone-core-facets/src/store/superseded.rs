// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Which records in a day file must not show, because another record already
//! stands for the same thing. Every reader that lists records asks here, so a
//! workout is listed once however far a conversion or a move has got.
//!
//! Each case turns on one durable write. A Strava line an earlier reading filed
//! stops showing when its key is in a code record's `converted` map, written
//! with the record. A moved record shows at its source until the move's log says
//! the target holds it, and from then on only at the target.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{Duration, NaiveDate};
use serde_json::Value;
use solstone_core_format::content::is_strava_reading;

use super::activity_records::{ActivityRecord, id, is_body_record};
use super::body_moves::{BodyRecordPlace, body_record_key, open_body_moves};

const STRAVA_CONVERTED_MARKER: &str = "imports/.strava-converted";
/// How far a workout's code record can be filed from a day a line of it is on.
/// A workout covers a few days at most.
const DAYS_AROUND: i64 = 7;
const CONVERTED: &[u8] = b"\"converted\"";

/// The ids of the records in `rows`, read from this day file, that must not
/// show. A file or log that can't be read hides nothing: a record shown twice
/// can be put right, and one not shown can't be seen.
#[must_use]
pub fn superseded_record_ids(
    root: &Path,
    facet: &str,
    day: &str,
    rows: &[ActivityRecord],
) -> BTreeSet<String> {
    let mut gone = folded_strava_lines(root, facet, day, rows);
    gone.extend(moved_elsewhere(root, facet, day, rows));
    gone
}

/// Whether a conversion could have left a folded line anywhere. Nothing is left
/// to convert once the marker is written, and a conversion clears it first.
#[must_use]
pub fn strava_lines_may_be_folded(root: &Path) -> bool {
    !root.join(STRAVA_CONVERTED_MARKER).exists()
}

fn folded_strava_lines(
    root: &Path,
    facet: &str,
    day: &str,
    rows: &[ActivityRecord],
) -> BTreeSet<String> {
    let mut lines = rows
        .iter()
        .filter(|row| is_strava_reading(row) && !is_body_record(row))
        .peekable();
    if lines.peek().is_none() || !strava_lines_may_be_folded(root) {
        return BTreeSet::new();
    }
    let folded = converted_keys_around(root, day);
    lines
        .map(id)
        .filter(|line| folded.contains(&format!("{day}|{facet}|{line}")))
        .map(str::to_owned)
        .collect()
}

/// Every line key a code record says it holds, on the days around `day`.
fn converted_keys_around(root: &Path, day: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return keys;
    };
    let Ok(facets) = std::fs::read_dir(root.join("facets")) else {
        return keys;
    };
    // A dot-named folder is a facet merge's scratch or leftover, never a facet.
    let folders = facets
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path().join("activities"))
        .collect::<Vec<_>>();
    for offset in -DAYS_AROUND..=DAYS_AROUND {
        let name = format!("{}.jsonl", (date + Duration::days(offset)).format("%Y%m%d"));
        for folder in &folders {
            let Ok(bytes) = std::fs::read(folder.join(&name)) else {
                continue;
            };
            for line in bytes.split(|&byte| byte == b'\n') {
                if !line.windows(CONVERTED.len()).any(|at| at == CONVERTED) {
                    continue;
                }
                let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                if !is_body_record(&record) {
                    continue;
                }
                if let Some(lines) = record
                    .get("converted")
                    .and_then(|converted| converted.get("lines"))
                    .and_then(Value::as_object)
                {
                    keys.extend(lines.keys().cloned());
                }
            }
        }
    }
    keys
}

fn moved_elsewhere(
    root: &Path,
    facet: &str,
    day: &str,
    rows: &[ActivityRecord],
) -> BTreeSet<String> {
    if !rows.iter().any(is_body_record) {
        return BTreeSet::new();
    }
    let Ok(moves) = open_body_moves(root) else {
        return BTreeSet::new();
    };
    if moves.is_empty() {
        return BTreeSet::new();
    }
    let here = BodyRecordPlace {
        day: day.to_owned(),
        facet: facet.to_owned(),
    };
    rows.iter()
        .filter(|row| is_body_record(row))
        .filter(|row| {
            let key = body_record_key(row);
            moves.iter().any(|open| {
                Some(open.key.as_str()) == key
                    && ((open.from == here && open.placed) || (open.to == here && !open.placed))
            })
        })
        .map(|row| id(row).to_owned())
        .collect()
}

#[cfg(test)]
#[path = "superseded_tests.rs"]
mod tests;
