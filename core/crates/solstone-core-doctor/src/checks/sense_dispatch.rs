// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::vocabulary::ReportForm;
use crate::{
    checks::service_status::{self, Unavailable},
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use serde_json::Value;
use solstone_core_cli_copy as copy;

const MAX_REPORTED_ERRORS: u64 = 99;

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    match service_status::fetch_observe_status(context) {
        Ok(status) => evaluate(check, &status, context.report_form),
        Err(error) => unavailable(check, error),
    }
}

#[cfg(test)]
pub(crate) fn from_status(check: Check, status: &Value) -> RunnerResult {
    evaluate(check, status, ReportForm::Human)
}

fn evaluate(check: Check, status: &Value, form: ReportForm) -> RunnerResult {
    let Some(count) = status.get("recent_error_count").and_then(Value::as_u64) else {
        return Ok(make_result(
            check,
            Status::Warn,
            copy::text("journal.cli.doctor.sense.dispatch.count.invalid"),
            None::<String>,
        ));
    };
    if count > MAX_REPORTED_ERRORS {
        return Ok(make_result(
            check,
            Status::Warn,
            copy::text("journal.cli.doctor.sense.dispatch.count.out.of.range"),
            None::<String>,
        ));
    }
    if count == 0 {
        return Ok(make_result(
            check,
            Status::Ok,
            copy::text("journal.cli.doctor.sense.dispatch.clear"),
            None::<String>,
        ));
    }

    let reason = status
        .get("last_error_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty());
    let detail = match (count == MAX_REPORTED_ERRORS, reason) {
        (true, Some(reason)) => copy::display(
            "journal.cli.doctor.sense.dispatch.capped.reason",
            [("reason", &reason)],
        ),
        (true, None) => copy::text("journal.cli.doctor.sense.dispatch.capped.unknown"),
        (false, Some(reason)) => form.counted(
            "journal.cli.doctor.sense.dispatch.errors.reason",
            &[("quantity", count)],
            &[("count", &count), ("reason", &reason)],
        ),
        (false, None) => form.counted(
            "journal.cli.doctor.sense.dispatch.errors.unknown",
            &[("quantity", count)],
            &[("count", &count)],
        ),
    };
    Ok(make_result(check, Status::Warn, detail, None::<String>))
}

fn unavailable(check: Check, error: Unavailable) -> RunnerResult {
    Ok(make_result(
        check,
        Status::Warn,
        copy::display(
            "journal.cli.doctor.sense.dispatch.unreachable",
            [("cause", &error.key())],
        ),
        None::<String>,
    ))
}

#[cfg(test)]
mod tests {
    use super::from_status;
    use crate::vocabulary::{Check, Platform, Severity, Status};
    use serde_json::json;

    fn check() -> Check {
        Check {
            name: "sense_dispatch",
            severity: Severity::Advisory,
            platforms: &[Platform::Linux],
        }
    }

    #[test]
    fn zero_errors_is_healthy_even_when_the_previous_reason_remains() {
        let result = from_status(
            check(),
            &json!({"recent_error_count": 0, "last_error_reason": "old failure"}),
        )
        .unwrap();
        assert_eq!(result.status, Status::Ok);
        assert!(
            result
                .detail
                .contains("since the last successful handler sync")
        );
    }

    #[test]
    fn positive_count_warns_and_labels_reason_as_the_latest_reason() {
        let result = from_status(
            check(),
            &json!({"recent_error_count": 2, "last_error_reason": "lease_timeout"}),
        )
        .unwrap();
        assert_eq!(result.status, Status::Warn);
        assert!(result.detail.contains("2 Sense dispatch errors"));
        assert!(result.detail.contains("latest reason: lease_timeout"));
    }

    #[test]
    fn capped_count_is_reported_as_at_least_99() {
        let result = from_status(
            check(),
            &json!({"recent_error_count": 99, "last_error_reason": "pool unavailable"}),
        )
        .unwrap();
        assert_eq!(result.status, Status::Warn);
        assert!(result.detail.contains("at least 99"));
    }

    #[test]
    fn incomplete_and_out_of_range_counts_warn() {
        for status in [
            json!({}),
            json!({"recent_error_count": -1}),
            json!({"recent_error_count": "1"}),
            json!({"recent_error_count": 100}),
        ] {
            assert_eq!(from_status(check(), &status).unwrap().status, Status::Warn);
        }
    }

    #[test]
    fn positive_count_without_a_latest_reason_still_warns() {
        for status in [
            json!({"recent_error_count": 1}),
            json!({"recent_error_count": 1, "last_error_reason": null}),
            json!({"recent_error_count": 1, "last_error_reason": "  "}),
        ] {
            let result = from_status(check(), &status).unwrap();
            assert_eq!(result.status, Status::Warn);
            assert!(result.detail.contains("latest reason unavailable"));
        }
    }
}
