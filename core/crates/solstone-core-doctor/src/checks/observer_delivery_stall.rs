// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_observer::store::reload::ReloadError;
use solstone_core_observer::{DeliveryAssessment, OwnerState};

const MINUTE_MS: i64 = 60_000;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    Ok(result_from_assessment(
        common::inspect_context(context),
        check,
    ))
}

pub(crate) fn result_from_assessment(
    assessed: Result<Vec<DeliveryAssessment>, ReloadError>,
    check: Check,
) -> CheckResult {
    let assessed = match assessed {
        Ok(assessed) => assessed,
        Err(_) => {
            return make_result(
                check,
                Status::Skip,
                "device list unavailable",
                None::<String>,
            );
        }
    };
    if assessed.is_empty() {
        return make_result(
            check,
            Status::Skip,
            "sol hasn't added anything to your journal yet",
            None::<String>,
        );
    }
    let stalled: Vec<&DeliveryAssessment> = assessed
        .iter()
        .filter(|row| matches!(row.state, OwnerState::Stale | OwnerState::Offline))
        .collect();
    if stalled.is_empty() {
        return make_result(
            check,
            Status::Ok,
            "sol on every device that has added to your journal is current",
            None::<String>,
        );
    }
    let clauses: Vec<String> = stalled.iter().map(|row| stall_clause(row)).collect();
    make_result(
        check,
        Status::Warn,
        truncate(&common::join_capped(&clauses, " | "), 400),
        Some("restart sol on that device, then confirm something new is in your journal"),
    )
}

fn stall_clause(row: &DeliveryAssessment) -> String {
    let added = row
        .last_segment_received_age_ms
        .expect("stalled device has a last-sent stamp");
    let body = format!("sol on {} last added {}m ago", row.name, added / MINUTE_MS);
    match row.last_seen_age_ms {
        Some(age) => format!("{body}; last contact {}m ago", age / MINUTE_MS),
        None => body,
    }
}
