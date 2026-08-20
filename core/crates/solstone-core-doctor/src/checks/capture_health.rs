// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_observer::store::reload::ReloadError;
use solstone_core_observer::{DeliveryAssessment, OwnerState};

const HOUR_MS: i64 = 3_600_000;

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
                "rollup=unknown; device list unavailable",
                None::<String>,
            );
        }
    };
    if assessed.is_empty() {
        return make_result(
            check,
            Status::Skip,
            "rollup=no_senders; the solstone app hasn't added anything to your journal yet",
            None::<String>,
        );
    }
    if assessed.iter().all(|row| row.state == OwnerState::Active) {
        return make_result(
            check,
            Status::Ok,
            "rollup=active; the solstone app on every device that has added to your journal is current",
            None::<String>,
        );
    }
    let clauses: Vec<String> = assessed
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
}

fn capture_clause(row: &DeliveryAssessment) -> String {
    match row.last_segment_received_age_ms {
        Some(age) => format!(
            "the solstone app on {} last added {}h ago",
            row.name,
            age / HOUR_MS
        ),
        None => format!("the solstone app on {} is having trouble adding", row.name),
    }
}
