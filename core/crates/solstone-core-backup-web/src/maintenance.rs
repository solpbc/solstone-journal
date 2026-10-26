// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Callable planned offload through the route's original operation ownership.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use solstone_core_backup::archive_admission::ArchiveConfiguration;
use solstone_core_backup_runtime::resolve_restic_member;
use solstone_core_journal_io::JournalRoot;

use crate::{BackupWebDeps, archive_worker, operation};

/// The current run's disposition; stored previous success is not consulted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlannedOffloadDisposition {
    Skipped,
    Completed,
    Refused,
    Failed,
    Held,
}

/// Keeps the operation domain, including any original unresolved native owners.
/// Borrowed report fields cannot release that custody or complete an operation.
#[must_use = "retain the whole offload outcome through its receiving boundary"]
pub struct PlannedOffloadOutcome {
    disposition: PlannedOffloadDisposition,
    generation: Option<u64>,
    reason: Option<&'static str>,
    operation: Option<operation::Operation>,
    _operations: operation::SharedOperationSlot,
    _root: Arc<JournalRoot>,
}

impl PlannedOffloadOutcome {
    pub fn disposition(&self) -> PlannedOffloadDisposition {
        self.disposition
    }

    pub fn exit_code(&self) -> i32 {
        i32::from(!matches!(
            self.disposition,
            PlannedOffloadDisposition::Skipped | PlannedOffloadDisposition::Completed
        ))
    }

    pub fn generation(&self) -> Option<u64> {
        self.generation
    }

    pub fn reason(&self) -> Option<&str> {
        if let Some(reason) = self.reason {
            return Some(reason);
        }
        self.operation
            .as_ref()
            .and_then(|operation| operation.reason_code.as_deref())
    }

    pub fn report(&self) -> Option<&serde_json::Value> {
        self.operation
            .as_ref()
            .and_then(|operation| operation.report.as_ref())
    }
}

/// Run the maintained planner and owned creator synchronously. The supplied
/// checkpoint belongs to this one task; Stop remains the enclosing route lifetime.
/// Mobile admission stays closed until its native archive implementation is installed.
pub fn run_planned_offload(
    root: Arc<JournalRoot>,
    deps: &BackupWebDeps,
    checkpoint: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> PlannedOffloadOutcome {
    let outcome = |disposition, reason| PlannedOffloadOutcome {
        disposition,
        generation: None,
        reason,
        operation: None,
        _operations: Arc::clone(&deps.operations),
        _root: Arc::clone(&root),
    };
    if checkpoint().is_err() {
        return outcome(PlannedOffloadDisposition::Refused, Some("task_cancelled"));
    }
    if root.canonical_path() != deps.journal_root || root.revalidate_canonical_binding().is_err() {
        return outcome(
            PlannedOffloadDisposition::Refused,
            Some("journal_binding_changed"),
        );
    }
    if operation::lock_for_begin(&deps.operations).is_err() {
        return outcome(
            PlannedOffloadDisposition::Refused,
            Some("operation_admission_refused"),
        );
    }
    if let Ok(configuration) = ArchiveConfiguration::capture(&root)
        && !configuration.offload_enabled()
    {
        if checkpoint().is_err()
            || configuration.revalidate(&root).is_err()
            || operation::lock_for_begin(&deps.operations).is_err()
        {
            return outcome(
                PlannedOffloadDisposition::Refused,
                Some("admission_changed"),
            );
        }
        return outcome(PlannedOffloadDisposition::Skipped, Some("offload_off"));
    }
    // This is static tool selection. The original worker still owns repository,
    // credential, source, budget and actual executor admission.
    let restic = match resolve_restic_member(&deps.executable) {
        Ok(restic) => restic,
        Err(_) => {
            return outcome(
                PlannedOffloadDisposition::Refused,
                Some("archive_tools_unavailable"),
            );
        }
    };
    let begin = match archive_worker::begin_owned_for_task(
        &deps.operations,
        "offload",
        Arc::clone(&root),
        Arc::clone(&deps.clock),
        checkpoint,
    ) {
        Ok(begin) => begin,
        Err(_) => {
            return outcome(
                PlannedOffloadDisposition::Refused,
                Some("operation_admission_refused"),
            );
        }
    };
    let generation = begin.generation();
    // The reservation excludes predecessors. This receiver guard also excludes
    // a successor after handoff, until this run has captured its own result.
    let _receiver =
        match operation::admit_write(&deps.operations, operation::AdmitWrite::DuringOperation) {
            Ok(receiver) => receiver,
            Err(_) => {
                begin.cancel_before_preparation(&deps.operations);
                return outcome(
                    PlannedOffloadDisposition::Refused,
                    Some("operation_admission_refused"),
                );
            }
        };
    let Some(at) = DateTime::<Utc>::from_timestamp(deps.clock.now_unix(), 0) else {
        begin.cancel_before_preparation(&deps.operations);
        let mut result = outcome(
            PlannedOffloadDisposition::Refused,
            Some("timestamp_invalid"),
        );
        result.generation = Some(generation);
        return result;
    };
    let returned = catch_unwind(AssertUnwindSafe(|| {
        archive_worker::offload::create_planned(
            &deps.operations,
            begin,
            at.fixed_offset(),
            &restic,
        );
    }));
    let mut result = outcome(PlannedOffloadDisposition::Held, None);
    result.generation = Some(generation);
    if returned.is_err() {
        result.reason = Some("worker_unwound");
        return result;
    }
    if let Ok(state) = deps.operations.lock()
        && let Some(current) = state.current.as_ref()
        && current.generation == generation
    {
        result.operation = Some(current.view.clone());
        if !state.archive.is_busy() && !current.worker_active {
            result.disposition = if current.view.phase == "done" {
                PlannedOffloadDisposition::Completed
            } else {
                PlannedOffloadDisposition::Failed
            };
        }
    }
    if root.revalidate_canonical_binding().is_err() {
        result.disposition = PlannedOffloadDisposition::Refused;
        result.reason = Some("journal_binding_changed");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn root() -> (tempfile::TempDir, Arc<JournalRoot>, BackupWebDeps) {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(
            directory.path().join("config/journal.json"),
            r#"{"backup":{"enabled":true,"mode":"byo","offload":{"enabled":false}}}"#,
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(directory.path()).unwrap());
        let mut deps = BackupWebDeps::new(root.canonical_path().to_path_buf());
        deps.executable = "/must-not-resolve/offload-disabled".into();
        (directory, root, deps)
    }

    #[test]
    fn disabled_offload_skips_without_tool_selection_or_generation() {
        let (_directory, root, deps) = root();
        let result = run_planned_offload(root, &deps, Arc::new(|| Ok(())));
        assert_eq!(result.disposition(), PlannedOffloadDisposition::Skipped);
        assert_eq!(result.reason(), Some("offload_off"));
        assert_eq!(result.exit_code(), 0);
        assert_eq!(result.generation(), None);
        assert!(deps.operations.lock().unwrap().current.is_none());
    }

    #[test]
    fn cancelled_task_cannot_turn_a_skip_into_success_or_cancel_service() {
        let (_directory, root, deps) = root();
        let result = run_planned_offload(root, &deps, Arc::new(|| Err("task stopped".into())));
        assert_eq!(result.disposition(), PlannedOffloadDisposition::Refused);
        assert_ne!(result.exit_code(), 0);
        assert!(!deps.operations.lock().unwrap().cancellation.is_cancelled());
        assert!(deps.lifetime().admit_maintenance().is_ok());
    }

    #[test]
    fn stopped_service_refuses_even_an_otherwise_disabled_skip() {
        let (_directory, root, deps) = root();
        deps.lifetime().request_stop();
        let result = run_planned_offload(root, &deps, Arc::new(|| Ok(())));
        assert_eq!(result.disposition(), PlannedOffloadDisposition::Refused);
        assert_ne!(result.exit_code(), 0);
        assert_eq!(result.generation(), None);
    }

    #[test]
    fn changed_offload_configuration_cannot_complete_an_old_skip() {
        use std::sync::atomic::AtomicUsize;
        let (_directory, root, deps) = root();
        let calls = AtomicUsize::new(0);
        let config_path = root.canonical_path().join("config/journal.json");
        let checkpoint = Arc::new(move || {
            if calls.fetch_add(1, Ordering::AcqRel) == 1 {
                fs::write(
                    &config_path,
                    r#"{"backup":{"enabled":true,"mode":"byo","offload":{"enabled":true}}}"#,
                )
                .unwrap();
            }
            Ok(())
        });
        let result = run_planned_offload(root, &deps, checkpoint);
        assert_eq!(result.disposition(), PlannedOffloadDisposition::Refused);
        assert_eq!(result.generation(), None);
        assert!(deps.operations.lock().unwrap().current.is_none());
    }

    #[test]
    fn task_child_cancellation_leaves_service_and_successor_runnable() {
        let (_directory, root, deps) = root();
        let active = Arc::new(AtomicBool::new(true));
        let task_active = Arc::clone(&active);
        let begin = archive_worker::begin_owned_for_task(
            &deps.operations,
            "offload",
            Arc::clone(&root),
            Arc::clone(&deps.clock),
            Arc::new(move || {
                if task_active.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err("task stopped".into())
                }
            }),
        )
        .unwrap();
        let child = deps
            .operations
            .lock()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .archive_cancellation
            .as_ref()
            .unwrap()
            .clone();
        active.store(false, Ordering::Release);
        assert!(child.is_cancelled());
        assert!(!deps.operations.lock().unwrap().cancellation.is_cancelled());
        begin.cancel_before_preparation(&deps.operations);
        let next =
            archive_worker::begin_owned(&deps.operations, "offload", root, Arc::clone(&deps.clock))
                .unwrap();
        child.cancel();
        assert!(
            !deps
                .operations
                .lock()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .archive_cancellation
                .as_ref()
                .unwrap()
                .is_cancelled()
        );
        next.cancel_before_preparation(&deps.operations);
    }

    #[test]
    fn maintenance_and_archive_admission_exclude_each_other_then_reopen() {
        let (_directory, root, deps) = root();
        let maintenance = deps.lifetime().admit_maintenance().unwrap();
        assert!(deps.lifetime().admit_maintenance().is_err());
        assert!(
            archive_worker::begin_owned(
                &deps.operations,
                "offload",
                Arc::clone(&root),
                Arc::clone(&deps.clock)
            )
            .is_err()
        );
        drop(maintenance);
        let archive =
            archive_worker::begin_owned(&deps.operations, "offload", root, Arc::clone(&deps.clock))
                .unwrap();
        assert!(deps.lifetime().admit_maintenance().is_err());
        archive.cancel_before_preparation(&deps.operations);
        assert!(deps.lifetime().admit_maintenance().is_ok());
    }

    #[test]
    fn receiver_guard_excludes_successor_after_original_reservation_ends() {
        let (_directory, root, deps) = root();
        let begin = archive_worker::begin_owned(
            &deps.operations,
            "offload",
            Arc::clone(&root),
            Arc::clone(&deps.clock),
        )
        .unwrap();
        let receiver =
            operation::admit_write(&deps.operations, operation::AdmitWrite::DuringOperation)
                .unwrap();
        begin.cancel_before_preparation(&deps.operations);
        assert!(
            archive_worker::begin_owned(
                &deps.operations,
                "offload",
                Arc::clone(&root),
                Arc::clone(&deps.clock)
            )
            .is_err()
        );
        assert!(deps.lifetime().admit_maintenance().is_err());
        drop(receiver);
        let next =
            archive_worker::begin_owned(&deps.operations, "offload", root, Arc::clone(&deps.clock))
                .unwrap();
        next.cancel_before_preparation(&deps.operations);
    }

    #[test]
    fn service_stop_reaches_admitted_child_and_remains_closed() {
        let (_directory, root, deps) = root();
        let begin = archive_worker::begin_owned_for_task(
            &deps.operations,
            "offload",
            Arc::clone(&root),
            Arc::clone(&deps.clock),
            Arc::new(|| Ok(())),
        )
        .unwrap();
        let child = deps
            .operations
            .lock()
            .unwrap()
            .current
            .as_ref()
            .unwrap()
            .archive_cancellation
            .as_ref()
            .unwrap()
            .clone();
        deps.lifetime().request_stop();
        assert!(child.is_cancelled());
        begin.cancel_before_preparation(&deps.operations);
        assert!(deps.lifetime().admit_maintenance().is_err());
        assert!(
            archive_worker::begin_owned(&deps.operations, "offload", root, Arc::clone(&deps.clock))
                .is_err()
        );
    }
}
