// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    run_with_clock(context, check, chrono::Utc::now)
}

pub(crate) fn run_with_clock(
    context: &CheckContext,
    check: Check,
    clock: impl FnOnce() -> chrono::DateTime<chrono::Utc>,
) -> RunnerResult {
    let config = solstone_core_journal_config::read_journal_config(&context.journal_path)
        .map_err(|error| crate::vocabulary::ExecutionError {
            kind: "ConfigLoadError".into(),
            message: error.to_string(),
        })?
        .config
        .unwrap_or_default();
    let mut now = context.now;
    let inspection =
        solstone_core_brain::inspect_brain_state_with_clock(&context.journal_path, &config, || {
            now = clock();
            now
        });
    if !matches!(inspection.status, solstone_core_brain::InspectionStatus::Ok) {
        return Ok(make_result(
            check,
            Status::Warn,
            format!(
                "unknown: {}",
                inspection
                    .error
                    .as_deref()
                    .or(inspection.projection.reason_code.as_deref())
                    .unwrap_or("brain state unavailable")
            ),
            None::<String>,
        ));
    }
    let view = solstone_core_brain::present_brain_inspection(&inspection, now);
    let detail = format!(
        "{}; state={}; reason={}; component={}; evidence_age={}",
        view.headline,
        inspection.projection.aggregate_state,
        view.reason_text,
        view.failing_component.unwrap_or_else(|| "none".into()),
        view.evidence.age_text.unwrap_or_else(|| "unknown".into())
    );
    let status = if matches!(
        inspection.projection.aggregate_state.as_str(),
        "ready" | "checking"
    ) {
        Status::Ok
    } else {
        Status::Warn
    };
    Ok(make_result(check, status, detail, None::<String>))
}
