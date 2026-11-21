// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_cli_copy as copy;
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
            copy::text("journal.cli.doctor.client.delivery.stall.list.unavailable"),
            None::<String>,
        )
    } else if common::activity_unavailable(&inspection) {
        make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.client.delivery.stall.activity.unavailable"),
            None::<String>,
        )
    } else if common::assessed_capture_rows(&inspection).is_none_or(|rows| rows.is_empty()) {
        make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.client.delivery.stall.no.senders"),
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
                copy::text("journal.cli.doctor.client.delivery.stall.quiet"),
                None::<String>,
            )
        } else {
            let clauses: Vec<String> = stalled.iter().map(|row| stall_clause(row)).collect();
            make_result(
                check,
                Status::Warn,
                truncate(&common::join_capped(&clauses, " | "), 400),
                Some(copy::text(
                    "journal.cli.doctor.client.delivery.stall.inspect",
                )),
            )
        }
    };
    result.client_delivery = Some(facts);
    result
}

fn stall_clause(row: &ClientAssessment) -> String {
    let names = common::needs_attention_source_names(row);
    if names.is_empty() {
        copy::display(
            "journal.cli.doctor.delivery.rejected",
            [("client", &(common::client_name(row)))],
        )
    } else {
        crate::vocabulary::ReportForm::Human.counted(
            "journal.cli.doctor.delivery.rejected.sources",
            &[("quantity", names.len() as u64)],
            &[
                ("client", &(common::client_name(row))),
                ("sources", &(names.join(", "))),
            ],
        )
    }
}
