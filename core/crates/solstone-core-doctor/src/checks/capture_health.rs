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

const HOUR_MS: i64 = 3_600_000;

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
            copy::text("journal.cli.doctor.capture.health.registry.unavailable"),
            None::<String>,
        )
    } else if common::activity_unavailable(&inspection) {
        make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.capture.health.data.unavailable"),
            None::<String>,
        )
    } else if common::assessed_capture_rows(&inspection).is_none_or(|rows| rows.is_empty()) {
        make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.capture.health.no.senders"),
            None::<String>,
        )
    } else {
        let rows = common::assessed_capture_rows(&inspection).expect("available assessment rows");
        let clauses: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                if row.capture_state == ClientCaptureState::Degraded {
                    Some(capture_clause(row))
                } else {
                    source_attention_clause(row)
                }
            })
            .collect();
        if clauses.is_empty() {
            make_result(
                check,
                Status::Ok,
                if rows
                    .iter()
                    .all(|row| row.capture_state == ClientCaptureState::Active)
                {
                    copy::text("journal.cli.doctor.capture.health.active")
                } else {
                    copy::text("journal.cli.doctor.capture.health.quiet")
                },
                None::<String>,
            )
        } else {
            let detail = copy::display(
                "journal.cli.doctor.capture.rollup",
                [("devices", &(common::join_capped(&clauses, ", ")))],
            );
            make_result(
                check,
                Status::Warn,
                truncate(&detail, 400),
                Some(copy::text("journal.cli.doctor.capture.health.inspect")),
            )
        }
    };
    result.client_delivery = Some(facts);
    result
}

fn capture_clause(row: &ClientAssessment) -> String {
    let names = common::needs_attention_source_names(row);
    match (row.capture_elapsed_ms, names.is_empty()) {
        (Some(age), true) => copy::display(
            "journal.cli.doctor.capture.recent",
            [
                ("client", &(common::client_name(row))),
                ("hours", &(age / HOUR_MS)),
            ],
        ),
        (Some(age), false) => crate::vocabulary::ReportForm::Human.counted(
            "journal.cli.doctor.capture.recent.sources",
            &[("quantity", names.len() as u64)],
            &[
                ("client", &(common::client_name(row))),
                ("hours", &(age / HOUR_MS)),
                ("sources", &(names.join(", "))),
            ],
        ),
        (None, true) => copy::display(
            "journal.cli.doctor.capture.trouble",
            [("client", &(common::client_name(row)))],
        ),
        (None, false) => crate::vocabulary::ReportForm::Human.counted(
            "journal.cli.doctor.capture.trouble.sources",
            &[("quantity", names.len() as u64)],
            &[
                ("client", &(common::client_name(row))),
                ("sources", &(names.join(", "))),
            ],
        ),
    }
}

fn source_attention_clause(row: &ClientAssessment) -> Option<String> {
    let names = common::needs_attention_source_names(row);
    let sources = common::format_attention_sources(&names)?;
    Some(copy::display(
        "journal.cli.doctor.capture.sources",
        [
            ("client", &(common::client_name(row))),
            ("sources", &sources),
        ],
    ))
}
