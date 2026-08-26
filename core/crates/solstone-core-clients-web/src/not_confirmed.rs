// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::Path;

use solstone_core_facets::list_facet_directories;
use solstone_core_journal_io::{contained_path, path_lexists};
use solstone_core_observer::has_history_for_stream;
use solstone_core_retention::receipt::Target;

use crate::receipt::Issue;
use crate::select::days_holding_tombstones;

const DERIVED_REASON: &str =
    "this was derived from data in the erased segments and was not removed";

pub(crate) fn collect_not_confirmed(
    journal: &Path,
    attempted: &[Target],
    occupied_streams: &BTreeSet<String>,
) -> Vec<Issue> {
    let mut days: BTreeSet<String> = attempted.iter().map(|target| target.day.clone()).collect();
    days.extend(days_holding_tombstones(journal));

    let mut issues = Vec::new();
    let facets = list_facet_directories(journal).unwrap_or_default();
    for day in &days {
        for facet in &facets {
            if artifact_exists(journal, &format!("facets/{facet}/entities/{day}.jsonl")) {
                issues.push(Issue {
                    what: format!("{facet} {day}: people and topics"),
                    plain_reason: DERIVED_REASON.to_owned(),
                });
            }
            if artifact_exists(journal, &format!("facets/{facet}/logs/{day}.jsonl")) {
                issues.push(Issue {
                    what: format!("{facet} {day}: activity summary"),
                    plain_reason: DERIVED_REASON.to_owned(),
                });
            }
            if artifact_exists(journal, &format!("facets/{facet}/news/{day}.md")) {
                issues.push(Issue {
                    what: format!("{facet} {day}: news"),
                    plain_reason: DERIVED_REASON.to_owned(),
                });
            }
        }
    }
    for stream in occupied_streams {
        if stream == "location" {
            continue;
        }
        if has_history_for_stream(journal, stream) {
            issues.push(Issue {
                what: format!("{stream}: import history"),
                plain_reason: "import history for this stream was not removed".to_owned(),
            });
        }
    }
    issues.sort_by(|left, right| left.what.cmp(&right.what));
    issues.dedup_by(|left, right| left.what == right.what);
    issues
}

fn artifact_exists(journal: &Path, rel: &str) -> bool {
    contained_path(journal, rel)
        .ok()
        .is_some_and(|path| path_lexists(&path).unwrap_or(false) && path.is_file())
}
