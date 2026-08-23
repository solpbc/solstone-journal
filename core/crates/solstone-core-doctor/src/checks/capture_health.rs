// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_observer::{DeliveryAssessment, DeliveryInspection, OwnerState, RegistryState};

const HOUR_MS: i64 = 3_600_000;

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
            "rollup=unknown; device list unavailable",
            None::<String>,
        )
    } else if inspection.assessed.is_empty() {
        make_result(
            check,
            Status::Skip,
            "rollup=no_senders; the solstone app hasn't added anything to your journal yet",
            None::<String>,
        )
    } else if inspection
        .assessed
        .iter()
        .all(|row| row.state == OwnerState::Active)
    {
        make_result(
            check,
            Status::Ok,
            "rollup=active; the solstone app on every device that has added to your journal is current",
            None::<String>,
        )
    } else {
        let clauses: Vec<String> = inspection
            .assessed
            .iter()
            .filter(|row| row.state != OwnerState::Active)
            .map(capture_clause)
            .collect();
        let detail = format!("rollup=attention; {}", common::join_capped(&clauses, ", "));
        make_result(
            check,
            Status::Warn,
            truncate(&detail, 400),
            Some("open /app/health to inspect each device"),
        )
    };
    result.observer_delivery = Some(facts);
    result
}

fn capture_clause(row: &DeliveryAssessment) -> String {
    let base = match row.last_segment_received_age_ms {
        Some(age) => format!(
            "the solstone app on {} last added {}h ago",
            row.name,
            age / HOUR_MS
        ),
        None => format!("the solstone app on {} is having trouble adding", row.name),
    };
    if matches!(row.state, OwnerState::Stale | OwnerState::Offline) {
        format!("{base}; {}", common::delivery_reach_clause(row.reach))
    } else {
        base
    }
}
