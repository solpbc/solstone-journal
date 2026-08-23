// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_observer::{DeliveryAssessment, DeliveryInspection, OwnerState, RegistryState};

const MINUTE_MS: i64 = 60_000;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    Ok(result_from_assessment(
        common::inspect_context(context),
        check,
    ))
}

pub(crate) fn result_from_assessment(inspection: DeliveryInspection, check: Check) -> CheckResult {
    let facts = common::delivery_facts(&inspection);
    let mut result = if inspection.registry == RegistryState::RegistryUnknown {
        make_result(
            check,
            Status::Skip,
            "device list unavailable",
            None::<String>,
        )
    } else if inspection.assessed.is_empty() {
        make_result(
            check,
            Status::Skip,
            "the solstone app hasn't added anything to your journal yet",
            None::<String>,
        )
    } else {
        let stalled: Vec<&DeliveryAssessment> = inspection
            .assessed
            .iter()
            .filter(|row| matches!(row.state, OwnerState::Stale | OwnerState::Offline))
            .collect();
        if stalled.is_empty() {
            make_result(
                check,
                Status::Ok,
                "the solstone app on every device that has added to your journal is current",
                None::<String>,
            )
        } else {
            let clauses: Vec<String> = stalled.iter().map(|row| stall_clause(row)).collect();
            make_result(
                check,
                Status::Warn,
                truncate(&common::join_capped(&clauses, " | "), 400),
                Some(
                    "restart the solstone app on that device, then confirm something new is in your journal",
                ),
            )
        }
    };
    result.observer_delivery = Some(facts);
    result
}

fn stall_clause(row: &DeliveryAssessment) -> String {
    let added = row
        .last_segment_received_age_ms
        .expect("stalled device has a last-sent stamp");
    let body = format!(
        "the solstone app on {} last added {}m ago",
        row.name,
        added / MINUTE_MS
    );
    match row.last_seen_age_ms {
        Some(age) => format!("{body}; last contact {}m ago", age / MINUTE_MS),
        None => body,
    }
}
