// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, ExecutionError, RunnerResult, Status, make_result},
};
use solstone_core_system_health::{TimelineDivergenceDiagnosis, diagnose_timeline_divergence};

const ROLLUP_FIX: &str = "journal maintenance run timeline:rollup --commit";

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let diagnosis =
        diagnose_timeline_divergence(&context.journal_path, context.now).map_err(|error| {
            ExecutionError {
                kind: "TimelineHealthError".to_owned(),
                message: error.to_string(),
            }
        })?;
    render(check, diagnosis)
}

fn render(check: Check, diagnosis: TimelineDivergenceDiagnosis) -> RunnerResult {
    match diagnosis {
        TimelineDivergenceDiagnosis::Clean => Ok(make_result(
            check,
            Status::Ok,
            "timeline artifacts current",
            None::<String>,
        )),
        TimelineDivergenceDiagnosis::NoData => Ok(make_result(
            check,
            Status::Ok,
            "no timeline artifacts yet",
            None::<String>,
        )),
        TimelineDivergenceDiagnosis::Stale { detail } => Ok(make_result(
            check,
            Status::Warn,
            format!("timeline needs refresh: {detail}"),
            Some(ROLLUP_FIX),
        )),
        TimelineDivergenceDiagnosis::Diverged { detail } => Ok(make_result(
            check,
            Status::Warn,
            format!("timeline artifacts diverged: {detail}"),
            Some(ROLLUP_FIX),
        )),
        TimelineDivergenceDiagnosis::Uncertain { detail } => Ok(make_result(
            check,
            Status::Warn,
            format!("timeline publication uncertain: {detail}"),
            Some(ROLLUP_FIX),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        checks::test_support,
        registry::{self, Battery},
        vocabulary::{Severity, Status},
    };
    use solstone_core_system_health::TimelineDivergenceDiagnosis;

    use super::{ROLLUP_FIX, render, run};

    #[test]
    fn run_mapping_uses_advisory_ok_and_warn_statuses() {
        let check = test_support::check("timeline_divergence", Severity::Advisory);
        for (diagnosis, status, fix) in [
            (TimelineDivergenceDiagnosis::Clean, Status::Ok, None),
            (TimelineDivergenceDiagnosis::NoData, Status::Ok, None),
            (
                TimelineDivergenceDiagnosis::Stale {
                    detail: "stale".to_owned(),
                },
                Status::Warn,
                Some(ROLLUP_FIX),
            ),
            (
                TimelineDivergenceDiagnosis::Diverged {
                    detail: "diverged".to_owned(),
                },
                Status::Warn,
                Some(ROLLUP_FIX),
            ),
            (
                TimelineDivergenceDiagnosis::Uncertain {
                    detail: "uncertain".to_owned(),
                },
                Status::Warn,
                Some(ROLLUP_FIX),
            ),
        ] {
            let result = render(check, diagnosis).expect("result");
            assert_eq!(result.severity, Severity::Advisory);
            assert_eq!(result.status, status);
            assert_eq!(result.fix.as_deref(), fix);
        }
    }

    #[test]
    fn run_reports_no_data_without_writing_and_registry_uses_journal_battery() {
        let context = test_support::context();
        let check = test_support::check("timeline_divergence", Severity::Advisory);
        let result = run(&context, check).expect("result");
        assert_eq!(result.status, Status::Ok);
        assert_eq!(result.detail, "no timeline artifacts yet");

        let entry = registry::lookup(Battery::Journal, "timeline_divergence").expect("entry");
        assert_eq!(entry.check.severity, Severity::Advisory);
        assert!(registry::lookup(Battery::JournalReadiness, "timeline_divergence").is_none());
    }
}
