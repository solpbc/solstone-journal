// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::context::CheckContext;
use crate::vocabulary::{Check, RunnerResult, Status, make_result};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system::queue_hold_store::{
    HoldPlatform, classify_task_queue_holds, current_boot_identity,
};

pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    if !context.journal_path.is_dir() {
        return Ok(make_result(
            check,
            Status::Skip,
            "no local journal",
            None::<String>,
        ));
    }
    let processes = SystemProcessInstanceSource;
    let current_boot = current_boot_identity();
    let platform = match context.platform {
        crate::vocabulary::Platform::Windows => HoldPlatform::Windows,
        crate::vocabulary::Platform::Darwin => HoldPlatform::Macos,
        crate::vocabulary::Platform::Linux => HoldPlatform::Linux,
    };
    let findings = classify_task_queue_holds(
        &context.journal_path,
        &processes,
        current_boot.as_deref(),
        platform,
    );
    if findings.is_empty() {
        return Ok(make_result(
            check,
            Status::Ok,
            "no held tasks",
            None::<String>,
        ));
    }
    let has_fail = findings.iter().any(|f| f.is_fail);
    let has_warn = findings.iter().any(|f| f.is_warn);
    let status = if has_fail {
        Status::Fail
    } else if has_warn {
        Status::Warn
    } else {
        Status::Ok
    };
    let details: Vec<String> = findings
        .iter()
        .map(|f| {
            let rel = f
                .path
                .strip_prefix(&context.journal_path)
                .unwrap_or(&f.path)
                .display();
            format!("{rel}: {}", f.detail)
        })
        .collect();
    let actions: Vec<&str> = findings
        .iter()
        .filter_map(|f| f.action.as_deref())
        .collect();
    let action_str = if actions.is_empty() {
        None
    } else {
        Some(actions.join("; "))
    };
    Ok(make_result(check, status, details.join("\n"), action_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::test_support::{check, context};
    use crate::vocabulary::Severity;
    use std::fs;

    #[test]
    fn clean_journal_reports_ok() {
        let context = context();
        fs::create_dir_all(&context.journal_path).unwrap();
        let result = run(&context, check("task_queue_holds", Severity::Advisory)).unwrap();
        assert_eq!(result.status, Status::Ok);
        assert_eq!(result.detail, "no held tasks");
    }

    #[test]
    fn corrupted_record_reports_fail() {
        let context = context();
        fs::create_dir_all(&context.journal_path).unwrap();
        let scope_name = solstone_core_system::queue_hold_store::format_scope_dir_name(
            None,
            &solstone_core_system::process::ProcessInstance {
                pid: 100,
                birth: solstone_core_system::process::ProcessBirth::unknown(),
            },
        );
        let scope = solstone_core_system::queue_hold_store::scope_directory(
            &context.journal_path,
            &scope_name,
        );
        fs::create_dir_all(&scope).unwrap();
        let rec_path = solstone_core_system::queue_hold_store::partition_record_path(
            &context.journal_path,
            &scope_name,
            &solstone_core_system::partition::Partition::new("svc"),
        );
        fs::write(&rec_path, b"invalid").unwrap();

        let result = run(&context, check("task_queue_holds", Severity::Advisory)).unwrap();
        assert_eq!(result.status, Status::Fail);
        assert!(result.detail.contains("cannot parse record"));
    }
}
