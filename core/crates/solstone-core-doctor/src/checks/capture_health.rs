// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    checks::common,
    context::CheckContext,
    vocabulary::{Check, CheckResult, RunnerResult, Status, make_result, truncate},
};
use solstone_core_sol_link::client_status::{
    ClientAssessment, ClientCaptureState, ClientInspection, ConnectionFreshness,
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
            "rollup=unknown; device list unavailable",
            None::<String>,
        )
    } else if common::activity_unavailable(&inspection) {
        make_result(
            check,
            Status::Skip,
            "rollup=unknown; device activity unavailable",
            None::<String>,
        )
    } else if common::assessed_capture_rows(&inspection).is_none_or(|rows| rows.is_empty()) {
        make_result(
            check,
            Status::Skip,
            "rollup=no_senders; the solstone app hasn't added anything to your journal yet",
            None::<String>,
        )
    } else {
        let rows = common::assessed_capture_rows(&inspection).expect("available assessment rows");
        let clauses: Vec<String> = rows
            .iter()
            .filter_map(|row| {
                if row.capture_state != ClientCaptureState::Active {
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
                "rollup=active; the solstone app on every device that has added to your journal is current",
                None::<String>,
            )
        } else {
            let detail = format!("rollup=attention; {}", common::join_capped(&clauses, ", "));
            make_result(
                check,
                Status::Warn,
                truncate(&detail, 400),
                Some("open /app/health to inspect each device"),
            )
        }
    };
    result.client_delivery = Some(facts);
    result
}

fn capture_clause(row: &ClientAssessment) -> String {
    let base = match row.capture_elapsed_ms {
        Some(age) => format!(
            "the solstone app on {} last added {}h ago",
            common::client_name(row),
            age / HOUR_MS
        ),
        None => format!(
            "the solstone app on {} is having trouble adding",
            common::client_name(row)
        ),
    };
    let names = common::needs_attention_source_names(row);
    let base = common::with_source_attention(base, &names);
    if matches!(
        row.capture_state,
        ClientCaptureState::Stale | ClientCaptureState::Offline
    ) {
        let reach = match row.connection {
            ConnectionFreshness::Known { reach, .. } => reach,
            ConnectionFreshness::Unknown => return base,
        };
        format!("{base}; {}", common::delivery_reach_clause(reach))
    } else {
        base
    }
}

fn source_attention_clause(row: &ClientAssessment) -> Option<String> {
    let names = common::needs_attention_source_names(row);
    let sources = common::format_attention_sources(&names)?;
    Some(format!(
        "the solstone app on {} is having trouble adding {sources}",
        common::client_name(row)
    ))
}
