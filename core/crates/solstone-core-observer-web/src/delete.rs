// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Path as RoutePath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use solstone_core_convey_http::envelope::error_envelope;
use solstone_core_indexer_store::RetentionIndex;
use solstone_core_journal_io::Removed;
use solstone_core_observer::remove_history_rows_for_stream;
use solstone_core_retention::door;
use solstone_core_retention::receipt::{Outcome, Target};
use solstone_core_retention::tombstone::RemovalReason;
use solstone_core_segment::delete_stream_record;

use crate::auth::{AuthError, authorize};
use crate::not_confirmed::collect_not_confirmed;
use crate::receipt::{Issue, Receipt, ReceiptTarget, Removed as RemovedCounts, owner_issue};
use crate::select::{Selected, select_location_targets};

pub(crate) async fn delete_source(
    State(journal): State<PathBuf>,
    RoutePath(stream): RoutePath<String>,
    headers: HeaderMap,
) -> Response {
    match authorize(&journal, &headers) {
        Ok(_) => {}
        Err(AuthError::Required) => {
            return error_envelope(
                "auth_required",
                "I need an observer key.",
                "Observer handle missing or invalid.",
                StatusCode::UNAUTHORIZED,
            )
            .into_response();
        }
        Err(AuthError::InvalidKey) => {
            return error_envelope(
                "auth_key_invalid",
                "I need an observer key.",
                "Observer handle missing or invalid.",
                StatusCode::UNAUTHORIZED,
            )
            .into_response();
        }
        Err(AuthError::Revoked) => {
            return error_envelope(
                "pl_revoked",
                "I couldn't use that observer because it was revoked.",
                "observer revoked",
                StatusCode::FORBIDDEN,
            )
            .into_response();
        }
        Err(AuthError::FeatureUnavailable) => {
            return error_envelope(
                "feature_unavailable",
                "I couldn't use that observer.",
                "observer is disabled or does not own this source",
                StatusCode::FORBIDDEN,
            )
            .into_response();
        }
    }
    if stream != "location" {
        return error_envelope(
            "invalid_segment_or_stream",
            "I couldn't use that segment or stream.",
            "Only known source streams can be deleted.",
            StatusCode::BAD_REQUEST,
        )
        .into_response();
    }

    Json(erase_location(&journal)).into_response()
}

fn erase_location(journal: &Path) -> Receipt {
    let scan = select_location_targets(journal);
    let scan_complete = scan.complete;
    let selected = scan.targets;
    let occupied_streams: BTreeSet<String> = selected
        .iter()
        .map(|item| item.target.stream.clone())
        .collect();
    let attempted: Vec<Target> = selected.iter().map(|item| item.target.clone()).collect();
    let deleted_at = Utc::now().to_rfc3339();

    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    // The door stamps one `cid` onto every tombstone in a call. Group by the
    // segment's `device.json` cid (else `"unknown"`) so each call still takes
    // a SET while criterion 8's per-segment identity is preserved.
    for (cid, targets) in groups_by_cid(&selected) {
        let part = door::remove_segments(
            journal,
            &targets,
            &deleted_at,
            RemovalReason::OwnerSegmentDelete,
            &cid,
        );
        outcome.targets.extend(part.targets);
    }

    let mut not_removed: Vec<Issue> = outcome
        .targets
        .iter()
        .flat_map(|target| target.not_removed.iter().map(owner_issue))
        .collect();
    not_removed.extend(outcome.targets.iter().filter_map(|target| {
        target.post_commit_failure.as_ref().map(|failure| Issue {
            what: failure.entry.clone(),
            plain_reason: failure.reason.clone(),
        })
    }));

    let completed: Vec<&Selected> = selected
        .iter()
        .filter(|item| {
            outcome
                .targets
                .iter()
                .any(|row| row.target == item.target && row.not_removed.is_empty())
        })
        .collect();
    let segments = completed.len() as u64;
    let mixed_segments = completed.iter().filter(|item| item.mixed).count() as u64;
    let days = completed
        .iter()
        .map(|item| item.target.day.as_str())
        .collect::<BTreeSet<_>>()
        .len() as u64;
    // originals, segments, and tombstones are equal by construction: one
    // completed target is one segment that went, one tombstone that remains
    // on disk, and one unit the client decodes as `originals`.
    let originals = segments;
    let tombstones = segments;
    let any_failed = outcome.has_failures() || outcome.halted.is_some();

    let index = RetentionIndex::new(journal);
    let index_chunks = match door::notify_index(&index, &outcome) {
        Ok(counts) => counts.chunks,
        Err(_) => {
            not_removed.push(Issue {
                what: "search index".to_owned(),
                plain_reason: "the search index could not be updated".to_owned(),
            });
            0
        }
    };

    let history_rows = if any_failed {
        0
    } else {
        let history = remove_history_rows_for_stream(journal, "location");
        for failure in history.failures {
            not_removed.push(Issue {
                what: "observer history".to_owned(),
                plain_reason: failure.reason,
            });
        }
        history.removed as u64
    };

    let stream_identity =
        unlink_location_stream(journal, any_failed, scan_complete, &mut not_removed);

    let not_confirmed = collect_not_confirmed(journal, &attempted, &occupied_streams);

    Receipt {
        target: ReceiptTarget {
            journal: journal.display().to_string(),
            stream: "location".to_owned(),
        },
        removed: RemovedCounts {
            days,
            history_rows,
            index_chunks,
            mixed_segments,
            originals,
            segments,
            stream_identity,
            tombstones,
        },
        not_confirmed,
        not_removed,
        backup_hosted: "not confirmed",
    }
}

fn groups_by_cid(selected: &[Selected]) -> Vec<(String, Vec<Target>)> {
    let mut groups: Vec<(String, Vec<Target>)> = Vec::new();
    for item in selected {
        if let Some((_, targets)) = groups.iter_mut().find(|(cid, _)| *cid == item.cid) {
            targets.push(item.target.clone());
        } else {
            groups.push((item.cid.clone(), vec![item.target.clone()]));
        }
    }
    groups
}

fn unlink_location_stream(
    journal: &Path,
    any_failed: bool,
    scan_complete: bool,
    not_removed: &mut Vec<Issue>,
) -> u64 {
    if any_failed {
        return 0;
    }
    if !scan_complete {
        not_removed.push(incomplete_scan_issue());
        return 0;
    }
    let remaining = select_location_targets(journal);
    if !remaining.complete {
        not_removed.push(incomplete_scan_issue());
        return 0;
    }
    if !remaining.targets.is_empty() {
        return 0;
    }
    match delete_stream_record(journal, "location") {
        Ok(Removed::Unlinked) => 1,
        Ok(Removed::AlreadyAbsent) => 0,
        Err(_) => {
            not_removed.push(Issue {
                what: "location stream state".to_owned(),
                plain_reason: "the stream record could not be removed".to_owned(),
            });
            0
        }
    }
}

fn incomplete_scan_issue() -> Issue {
    Issue {
        what: "chronicle listing".to_owned(),
        plain_reason: "the journal could not be listed completely, so remaining \
                       location data could not be ruled out"
            .to_owned(),
    }
}
