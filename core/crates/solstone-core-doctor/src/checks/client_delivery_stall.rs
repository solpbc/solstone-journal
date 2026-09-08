// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_sol_link::client_status::{
    ClientAssessment, ClientCaptureState, ClientInspection,
};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    Ok(result_from_assessment(
        common::inspect_context(context),
        check,
    ))
}

pub(crate) fn result_from_assessment(inspection: ClientInspection, check: Check) -> CheckResult {
    let facts = common::delivery_facts(&inspection);
    let mut result = if common::is_ledger_unavailable(&inspection) {
        make_result(
            check,
            Status::Skip,
            "device list unavailable",
            None::<String>,
        )
    } else if common::activity_unavailable(&inspection) {
        make_result(
            check,
            Status::Skip,
            "device activity unavailable",
            None::<String>,
        )
    } else if common::assessed_capture_rows(&inspection).is_none_or(|rows| rows.is_empty()) {
        make_result(
            check,
            Status::Skip,
            "the solstone app hasn't added anything to your journal yet",
            None::<String>,
        )
    } else {
        let stalled: Vec<&ClientAssessment> = common::assessed_capture_rows(&inspection)
            .expect("available assessment rows")
            .into_iter()
            .filter(|row| {
                matches!(row.capture_state, ClientCaptureState::Degraded)
                    || !common::needs_attention_source_names(row).is_empty()
            })
            .collect();
        if stalled.is_empty() {
            make_result(
                check,
                Status::Ok,
                "no rejected uploads recorded; see devices for last delivery times",
                None::<String>,
            )
        } else {
            let clauses: Vec<String> = stalled.iter().map(|row| stall_clause(row)).collect();
            make_result(
                check,
                Status::Warn,
                truncate(&common::join_capped(&clauses, " | "), 400),
                Some("open /app/health to inspect the rejected upload"),
            )
        }
    };
    result.client_delivery = Some(facts);
    result
}

fn stall_clause(row: &ClientAssessment) -> String {
    common::with_source_attention(
        format!(
            "the journal rejected an upload from {}",
            common::client_name(row)
        ),
        &common::needs_attention_source_names(row),
    )
}
