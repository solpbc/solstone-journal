// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    run_with_clock(context, check, jiff::Timestamp::now)
}

pub(crate) fn run_with_clock(
    context: &CheckContext,
    check: Check,
    clock: impl FnOnce() -> jiff::Timestamp,
) -> RunnerResult {
    let config = match solstone_core_journal_config::read_journal_config(&context.journal_path) {
        Ok(read) => read.config.unwrap_or_default(),
        Err(error) => {
            return Err(crate::vocabulary::ExecutionError {
                kind: "ConfigLoadError".into(),
                message: error.to_string(),
                localized_message: Some(super::common::config_error(&error)),
            });
        }
    };
    let mut now = context.now;
    let inspection =
        solstone_core_brain::inspect_brain_state_with_clock(&context.journal_path, &config, || {
            now = clock();
            now
        });
    let locale = solstone_core_window_catalog::cli_locale();
    if !matches!(inspection.status, solstone_core_brain::InspectionStatus::Ok) {
        let reason = inspection.error.clone().or_else(|| {
            inspection
                .projection
                .reason_code
                .as_deref()
                .map(|code| solstone_core_brain::brain_reason_text_in(Some(code), locale))
        });
        return Ok(make_result(
            check,
            Status::Warn,
            match reason.as_deref() {
                Some(reason) => copy::strings(
                    "journal.cli.doctor.brain.unknown.reason",
                    [("reason", reason)],
                ),
                None => copy::text("journal.cli.doctor.brain.unknown"),
            },
            None::<String>,
        ));
    }
    let view = solstone_core_brain::present_brain_inspection_in(&inspection, now, locale);
    let component = view
        .failing_component
        .unwrap_or_else(|| copy::text("journal.cli.doctor.brain.no.component"));
    let age = view
        .evidence
        .age_text
        .unwrap_or_else(|| copy::text("journal.cli.doctor.brain.age.unknown"));
    let detail = copy::strings(
        "journal.cli.doctor.brain.report",
        [
            ("headline", &view.headline),
            ("state", &inspection.projection.aggregate_state),
            ("reason", &view.reason_text),
            ("component", &component),
            ("age", &age),
        ],
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
