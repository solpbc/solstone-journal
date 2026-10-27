// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whole-journal restore and its sealed attempt record.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_backup::{
    Destination, assemble_backend_env, get_backup_config, load_hosted_binding, parse_recovery_key,
    save_hosted_binding, set_destination, set_recovery_key, set_recovery_key_confirmed,
};

use crate::engine::{BackupServices, Clock, RestoreRecorder};
use crate::restore_catalog::{CatalogError, select_journal_snapshot, snapshot_with_id};
use crate::runner::{ResticResult, reason_for_returncode, run_restic, select_summary};

pub const RESTORE_LIST_TIMEOUT_SECONDS: u64 = 5 * 60;
pub const RESTORE_TIMEOUT_SECONDS: u64 = 48 * 60 * 60;
pub const RESTORE_CHECK_TIMEOUT_SECONDS: u64 = 6 * 60 * 60;

pub const RESTORE_REASON_INVALID_KEY: &str = "invalid_key";
pub const RESTORE_REASON_RESTIC_UNAVAILABLE: &str = "restic_unavailable";
pub const RESTORE_REASON_DESTINATION_INVALID: &str = "destination_invalid";
pub const RESTORE_REASON_DESTINATION_ADMISSION_FAILED: &str = "destination_admission_failed";
pub const RESTORE_REASON_SNAPSHOT_LIST_IO_FAILED: &str = "snapshot_list_io_failed";
pub const RESTORE_REASON_SNAPSHOT_LIST_FAILED: &str = "snapshot_list_failed";
pub const RESTORE_REASON_SNAPSHOT_CATALOG_INVALID: &str = "snapshot_catalog_invalid";
pub const RESTORE_REASON_JOURNAL_SNAPSHOT_NOT_FOUND: &str = "journal_snapshot_not_found";
pub const RESTORE_REASON_SNAPSHOT_SELECTION_AMBIGUOUS: &str = "snapshot_selection_ambiguous";
pub const RESTORE_REASON_RESTORE_IO_FAILED: &str = "restore_io_failed";
pub const RESTORE_REASON_RESTORE_FAILED: &str = "restore_failed";
pub const RESTORE_REASON_RESTORE_SUMMARY_MISSING: &str = "restore_summary_missing";
pub const RESTORE_REASON_INTEGRITY_CHECK_IO_FAILED: &str = "integrity_check_io_failed";
pub const RESTORE_REASON_INTEGRITY_UNVERIFIED: &str = "integrity_unverified";
pub const RESTORE_REASON_INTEGRITY_FAILED: &str = "integrity_failed";
pub const RESTORE_REASON_BODY_REBUILD_FAILED: &str = "body_rebuild_failed";
pub const RESTORE_REASON_DESTINATION_PUBLISH_FAILED: &str = "destination_publish_failed";
pub const RESTORE_REASON_RECOVERY_KEY_PUBLISH_FAILED: &str = "recovery_key_publish_failed";
pub const RESTORE_REASON_RECOVERY_CONFIRMATION_PUBLISH_FAILED: &str =
    "recovery_confirmation_publish_failed";
pub const RESTORE_REASON_FULL_SCAN_FAILED: &str = "full_scan_failed";
pub const RESTORE_REASON_RESTORE_RECORD_FAILED: &str = "restore_record_failed";
pub const RESTORE_REASON_RECOVERY_KEY_REVOCATION_PENDING: &str = "recovery_key_revocation_pending";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreDraft {
    pub status: String,
    pub reason_code: Option<String>,
    pub integrity_ok: bool,
    pub resumable: bool,
    pub files_expected: Option<u64>,
    pub files_restored: Option<u64>,
    pub bytes_expected: Option<u64>,
    pub bytes_restored: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::manual_non_exhaustive)] // Private ZST deliberately prevents external literals.
pub struct RestoreOutcome {
    pub status: String,
    pub reason_code: Option<String>,
    pub recording_failure: Option<String>,
    pub integrity_ok: bool,
    pub resumable: bool,
    pub files_expected: Option<u64>,
    pub files_restored: Option<u64>,
    pub bytes_expected: Option<u64>,
    pub bytes_restored: Option<u64>,
    _sealed: (),
}

/// Record exactly one whole-journal restore attempt, then seal the result.
pub fn publish_restore_outcome(
    journal: &Path,
    clock: &dyn Clock,
    recorder: &dyn RestoreRecorder,
    draft: RestoreDraft,
) -> RestoreOutcome {
    let record_result = recorder.record(
        journal,
        &draft.status,
        json!(clock.now_unix()),
        option_string_value(draft.reason_code.as_deref()),
        "journal",
        Value::Null,
        json!(0),
        json!(0),
        option_number_value(draft.files_expected),
        option_number_value(draft.files_restored),
        option_number_value(draft.bytes_expected),
        option_number_value(draft.bytes_restored),
    );
    let recording_failure = record_result
        .err()
        .map(|_| RESTORE_REASON_RESTORE_RECORD_FAILED.to_owned());
    let (status, reason_code) = match recording_failure.as_deref() {
        None => (draft.status, draft.reason_code),
        Some(_) if draft.status == "ok" => (
            "error".to_owned(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED.to_owned()),
        ),
        Some(_) => ("error".to_owned(), draft.reason_code),
    };
    RestoreOutcome {
        status,
        reason_code,
        recording_failure,
        integrity_ok: draft.integrity_ok,
        resumable: draft.resumable,
        files_expected: draft.files_expected,
        files_restored: draft.files_restored,
        bytes_expected: draft.bytes_expected,
        bytes_restored: draft.bytes_restored,
        _sealed: (),
    }
}

fn option_string_value(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_owned()))
}

fn option_number_value(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

#[derive(Clone, Copy, Default)]
struct RestoreCounters {
    files_expected: Option<u64>,
    files_restored: Option<u64>,
    bytes_expected: Option<u64>,
    bytes_restored: Option<u64>,
}

impl RestoreCounters {
    fn into_draft(
        self,
        status: &str,
        reason_code: Option<String>,
        integrity_ok: bool,
        resumable: bool,
    ) -> RestoreDraft {
        RestoreDraft {
            status: status.to_owned(),
            reason_code,
            integrity_ok,
            resumable,
            files_expected: self.files_expected,
            files_restored: self.files_restored,
            bytes_expected: self.bytes_expected,
            bytes_restored: self.bytes_restored,
        }
    }
}

fn error_draft(reason_code: &str) -> RestoreDraft {
    RestoreCounters::default().into_draft("error", Some(reason_code.to_owned()), false, false)
}

fn backend(destination: &Destination) -> Result<BTreeMap<String, Option<String>>, &'static str> {
    assemble_backend_env(destination)
        .map(|env| {
            env.into_iter()
                .map(|(key, value)| (key, value.as_str().map(str::to_owned)))
                .collect()
        })
        .map_err(|_| RESTORE_REASON_DESTINATION_INVALID)
}

fn unrecorded_outcome(draft: RestoreDraft) -> RestoreOutcome {
    RestoreOutcome {
        status: draft.status,
        reason_code: draft.reason_code,
        recording_failure: None,
        integrity_ok: draft.integrity_ok,
        resumable: draft.resumable,
        files_expected: draft.files_expected,
        files_restored: draft.files_restored,
        bytes_expected: draft.bytes_expected,
        bytes_restored: draft.bytes_restored,
        _sealed: (),
    }
}

#[allow(clippy::too_many_arguments)] // Mirrors the shared restic invocation boundary.
fn restic(
    services: &BackupServices<'_>,
    admitted: &crate::destination_admission::AdmittedDestination,
    args: Vec<String>,
    destination: &Destination,
    key: &str,
    env: &BTreeMap<String, Option<String>>,
    json_output: bool,
    timeout: u64,
    runner_error_reason: &'static str,
) -> Result<ResticResult, &'static str> {
    admitted
        .revalidate_for_target()
        .map_err(|_| RESTORE_REASON_DESTINATION_ADMISSION_FAILED)?;
    let restic_path = services
        .restic_invocation()
        .map_err(|_| RESTORE_REASON_RESTIC_UNAVAILABLE)?;
    run_restic(
        services.runner,
        &args,
        &destination.repository,
        key,
        restic_path,
        Some(env),
        json_output,
        None,
        Some(Duration::from_secs(timeout)),
        &{
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                [admitted.child_lock()]
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                []
            }
        },
    )
    .map_err(|error| {
        log::warn!(
            "restore step {} refused: {error:?}",
            args.first().map_or("", String::as_str)
        );
        runner_error_reason
    })
}

fn recognized_returncode_reason(returncode: i32) -> Option<&'static str> {
    match returncode {
        3 | 10 | 11 | 12 | 124 => Some(reason_for_returncode(returncode)),
        _ => None,
    }
}

/// restic's restore summary counts every node it restores: directories,
/// symlinks and other special files as well as regular files, in both
/// `total_files` and `files_restored`. A regular file already in place with
/// matching content is counted in `files_skipped` and in neither of the others,
/// just as its bytes are left out of `total_bytes` and `bytes_restored`.
/// The file counters are therefore derived from the snapshot's regular-file
/// count, and are unknown when the snapshot does not carry one.
fn restore_counters(
    parsed: Option<&Value>,
    snapshot_regular_files: Option<u64>,
) -> Option<RestoreCounters> {
    let summary = select_summary(parsed?)?;
    let nodes_expected = summary_counter(summary, "total_files")?;
    let nodes_restored = summary_counter(summary, "files_restored")?;
    let files_skipped = summary_counter(summary, "files_skipped")?;
    let bytes_expected = summary_counter(summary, "total_bytes")?;
    let bytes_restored = summary_counter(summary, "bytes_restored")?;
    let files_expected = snapshot_regular_files.and_then(|files| files.checked_sub(files_skipped));
    let other_nodes = files_expected.and_then(|files| nodes_expected.checked_sub(files));
    let files_restored = other_nodes.and_then(|other| nodes_restored.checked_sub(other));
    Some(RestoreCounters {
        files_expected: files_restored.and(files_expected),
        files_restored,
        bytes_expected: Some(bytes_expected),
        bytes_restored: Some(bytes_restored),
    })
}

fn summary_counter(summary: &Map<String, Value>, field: &str) -> Option<u64> {
    // Restic omits zero-valued summary counters, but a present counter must be an unsigned integer.
    match summary.get(field) {
        None => Some(0),
        Some(value) => value.as_u64(),
    }
}

fn finish(
    journal: &Path,
    clock: &dyn Clock,
    recorder: &dyn RestoreRecorder,
    draft: RestoreDraft,
) -> RestoreOutcome {
    publish_restore_outcome(journal, clock, recorder, draft)
}

/// Refuse restore before it can overwrite an unresolved recovery-key transaction.
pub fn restore_pending_rotation_refusal(journal: &Path) -> Option<&'static str> {
    match get_backup_config(journal) {
        Ok(backup) if backup.contains_key("pending_key_revocation") => {
            Some(RESTORE_REASON_RECOVERY_KEY_REVOCATION_PENDING)
        }
        Ok(_) => None,
        Err(_) => Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED),
    }
}

/// Files at the restored journal's root that name the resident running it on
/// this machine. Backups omit them, but a snapshot taken before they were
/// omitted still carries another machine's copies; restoring those over a live
/// resident leaves its service unable to recognise or stop it. Each pattern is
/// rooted at the restored subtree, so owner files with the same name elsewhere
/// in the journal still restore.
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) const RESTORE_KEEPS_LOCAL: [&str; 2] = [
    "/health/supervisor.process_instance",
    "/health/callosum.pipe-*",
];

/// The restic argv that restores one journal snapshot's tree onto `target`.
pub(crate) fn restore_args(snapshot_id: &str, tree_path: &str, target: &str) -> Vec<String> {
    #[cfg_attr(target_os = "ios", allow(unused_mut))]
    let mut args = vec![
        "restore".to_owned(),
        format!("{snapshot_id}:{tree_path}"),
        "--target".to_owned(),
        target.to_owned(),
    ];
    // These name a desktop journal's own resident; iOS runs none, reads neither
    // file, and its linked restore takes no exclusions, so it emits none.
    #[cfg(not(target_os = "ios"))]
    {
        for pattern in RESTORE_KEEPS_LOCAL {
            args.extend(["--exclude".to_owned(), pattern.to_owned()]);
        }
        // The obligation is local runtime state. Excluding it here, including
        // from a snapshot taken before the backup omit existed, keeps restore
        // from importing, overwriting, or erasing it. iOS emits no exclude
        // arguments; its rooted restore filter carries the same path.
        args.extend([
            "--exclude".to_owned(),
            format!(
                "/{}",
                solstone_core_journal_io::whole_journal_restore::WHOLE_JOURNAL_RESTORE_FILE
            ),
            "--exclude".to_owned(),
            "/health/!solstone-claim-00000000-*".to_owned(),
        ]);
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    args.extend(["--exclude".to_owned(), "backup-activity.lock".to_owned()]);
    args
}

#[cfg(target_os = "ios")]
struct IosTargetRunner<'a> {
    inner: &'a dyn crate::runner::ToolRunner,
    target: &'a crate::ios_callable::RestoreTarget,
}

#[cfg(target_os = "ios")]
impl crate::runner::ToolRunner for IosTargetRunner<'_> {
    fn run(
        &self,
        request: &crate::runner::ToolRequest<'_>,
    ) -> std::io::Result<crate::runner::ToolOutput> {
        self.inner.run(request)
    }

    fn run_cancellable(
        &self,
        request: &crate::runner::ToolRequest<'_>,
        cancellation: &solstone_core_backup::archive_admission::ArchiveCancellation,
    ) -> std::io::Result<crate::runner::ToolOutput> {
        self.inner.run_cancellable(request, cancellation)
    }

    fn cancellation(
        &self,
    ) -> Option<&solstone_core_backup::archive_admission::ArchiveCancellation> {
        self.inner.cancellation()
    }

    fn ios_journal_root(&self) -> Option<&std::sync::Arc<solstone_core_journal_io::JournalRoot>> {
        self.inner.ios_journal_root()
    }

    fn ios_restore_target(&self) -> Option<&crate::ios_callable::RestoreTarget> {
        Some(self.target)
    }
}

/// Repository identity for one mobile whole-journal restore.
///
/// Hosted recovery stores the account, instance, bucket, and prefix. The
/// endpoint and token stay on the destination and can be corrected later.
/// A BYO restore stores the destination repository and backend, not its credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MobileRestoreIdentity {
    Hosted {
        account_id: String,
        instance_id: String,
        bucket: String,
        prefix: String,
    },
    Byo,
}

/// The snapshot and subtree a mobile host recorded or is asking to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobileSelection {
    pub snapshot_id: String,
    pub subtree: String,
}

struct MobileRestore<'a> {
    root: Arc<solstone_core_journal_io::JournalRoot>,
    identity: MobileRestoreIdentity,
    selection: Option<MobileSelection>,
    drain: &'a mut dyn FnMut() -> Result<(), String>,
}

enum RestorePhase {
    Desktop,
    First {
        identity: MobileRestoreIdentity,
        selection: Option<MobileSelection>,
    },
    Adopted {
        record: solstone_core_journal_io::whole_journal_restore::WholeJournalRestore,
    },
}

fn source_from(
    identity: &MobileRestoreIdentity,
    destination: &Destination,
) -> solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource {
    use solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource;
    match identity {
        MobileRestoreIdentity::Hosted {
            account_id,
            instance_id,
            bucket,
            prefix,
        } => WholeJournalRestoreSource::Hosted {
            account_id: account_id.clone(),
            instance_id: instance_id.clone(),
            bucket: bucket.clone(),
            prefix: prefix.clone(),
        },
        MobileRestoreIdentity::Byo => WholeJournalRestoreSource::Byo {
            repository: destination.repository.clone(),
            backend: destination.backend.clone(),
        },
    }
}

fn source_matches(
    source: &solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource,
    identity: &MobileRestoreIdentity,
    destination: &Destination,
) -> bool {
    use solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource;
    match (source, identity) {
        (
            WholeJournalRestoreSource::Hosted {
                account_id,
                instance_id,
                bucket,
                prefix,
            },
            MobileRestoreIdentity::Hosted {
                account_id: requested_account,
                instance_id: requested_instance,
                bucket: requested_bucket,
                prefix: requested_prefix,
            },
        ) => {
            account_id == requested_account
                && instance_id == requested_instance
                && bucket == requested_bucket
                && prefix == requested_prefix
        }
        (
            WholeJournalRestoreSource::Byo {
                repository,
                backend,
            },
            MobileRestoreIdentity::Byo,
        ) => repository == &destination.repository && backend == &destination.backend,
        _ => false,
    }
}

#[allow(clippy::result_large_err)]
fn classify_restore(
    journal: &Path,
    destination: &Destination,
    mobile: Option<&MobileRestore<'_>>,
) -> Result<RestorePhase, RestoreOutcome> {
    use solstone_core_journal_io::whole_journal_restore::{
        WholeJournalRestoreReadiness, read_whole_journal_restore,
    };
    let refuse = || unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
    match mobile {
        None => match read_whole_journal_restore(journal) {
            WholeJournalRestoreReadiness::Absent => Ok(RestorePhase::Desktop),
            WholeJournalRestoreReadiness::Pending(_)
            | WholeJournalRestoreReadiness::Unavailable => Err(refuse()),
        },
        Some(request) => match read_whole_journal_restore(journal) {
            WholeJournalRestoreReadiness::Unavailable => Err(refuse()),
            WholeJournalRestoreReadiness::Absent => Ok(RestorePhase::First {
                identity: request.identity.clone(),
                selection: request.selection.clone(),
            }),
            WholeJournalRestoreReadiness::Pending(record) => {
                if request.selection.as_ref().is_none_or(|selection| {
                    record.snapshot_id != selection.snapshot_id
                        || record.subtree != selection.subtree
                }) || !source_matches(&record.source, &request.identity, destination)
                {
                    Err(refuse())
                } else {
                    Ok(RestorePhase::Adopted { record: *record })
                }
            }
        },
    }
}

/// An adopted retry treats an unreadable config as incomplete, not as permission
/// to materialize defaults or to ignore a later imported revocation. A first
/// attempt still refuses before any obligation is published.
fn pre_copy_refusal(journal: &Path, adopted: bool) -> Option<&'static str> {
    match restore_pending_rotation_refusal(journal) {
        Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED) if adopted => None,
        refusal => refusal,
    }
}

fn catalog_reason(error: CatalogError) -> &'static str {
    match error {
        CatalogError::Invalid => RESTORE_REASON_SNAPSHOT_CATALOG_INVALID,
        CatalogError::NotFound => RESTORE_REASON_JOURNAL_SNAPSHOT_NOT_FOUND,
        CatalogError::Ambiguous => RESTORE_REASON_SNAPSHOT_SELECTION_AMBIGUOUS,
    }
}

fn retire_obligation(
    journal: &Path,
    obligation: Option<&solstone_core_journal_io::whole_journal_restore::WholeJournalRestore>,
    mut outcome: RestoreOutcome,
) -> RestoreOutcome {
    let Some(record) = obligation else {
        return outcome;
    };
    if outcome.status != "ok" && outcome.status != "degraded" {
        return outcome;
    }
    if solstone_core_journal_io::whole_journal_restore::clear_whole_journal_restore(journal, record)
        .is_err()
    {
        outcome.status = "error".to_owned();
        outcome.reason_code = Some(RESTORE_REASON_RESTORE_RECORD_FAILED.to_owned());
    }
    outcome
}

/// Restore a BYO repository. The three config setters remain purposefully independent.
///
/// Desktop selection stays on the single latest untagged journal snapshot.
/// A pending mobile obligation refuses this entry before copy.
pub fn restore_journal(
    journal: &Path,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
    destination: Destination,
    entered_recovery_key: &str,
) -> RestoreOutcome {
    restore_journal_with(
        journal,
        services,
        recorder,
        destination,
        entered_recovery_key,
        None,
    )
}

/// Restore one explicit mobile selection into the retained target.
///
/// `drain` must return only after work already admitted on that target has
/// stopped using and publishing it. An empty obligation census is not that proof.
#[allow(clippy::too_many_arguments)]
pub fn restore_mobile_journal(
    root: Arc<solstone_core_journal_io::JournalRoot>,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
    destination: Destination,
    entered_recovery_key: &str,
    identity: MobileRestoreIdentity,
    selection: MobileSelection,
    drain: &mut dyn FnMut() -> Result<(), String>,
) -> RestoreOutcome {
    restore_journal_with(
        root.canonical_path(),
        services,
        recorder,
        destination,
        entered_recovery_key,
        Some(MobileRestore {
            root: Arc::clone(&root),
            identity,
            selection: Some(selection),
            drain,
        }),
    )
}

/// Begin a mobile restore from the latest unambiguous journal snapshot.
/// Its exact id and subtree are recorded before drain or copy; retries must
/// use that recorded selection through `restore_mobile_journal`.
pub fn restore_mobile_latest_journal(
    root: Arc<solstone_core_journal_io::JournalRoot>,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
    destination: Destination,
    entered_recovery_key: &str,
    identity: MobileRestoreIdentity,
    drain: &mut dyn FnMut() -> Result<(), String>,
) -> RestoreOutcome {
    restore_journal_with(
        root.canonical_path(),
        services,
        recorder,
        destination,
        entered_recovery_key,
        Some(MobileRestore {
            root: Arc::clone(&root),
            identity,
            selection: None,
            drain,
        }),
    )
}

fn restore_journal_with(
    journal: &Path,
    services: &BackupServices<'_>,
    recorder: &dyn RestoreRecorder,
    destination: Destination,
    entered_recovery_key: &str,
    mut mobile: Option<MobileRestore<'_>>,
) -> RestoreOutcome {
    let admission = match mobile.as_ref() {
        Some(request) => crate::destination_admission::AdmittedDestination::admit_bound(
            Arc::clone(&request.root),
        ),
        None => crate::destination_admission::admit_restore_destination(journal),
    };
    let admitted = match admission {
        Ok(admitted) => admitted,
        Err(_) => {
            return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
        }
    };
    // iOS: the linked restore copies into exactly this admitted target, whose
    // native lease is held through every later step until `admitted` drops.
    #[cfg(target_os = "ios")]
    let targeted_runner = IosTargetRunner {
        inner: services.runner,
        target: admitted.ios_restore_target(),
    };
    #[cfg(target_os = "ios")]
    let services = &BackupServices {
        runner: &targeted_runner,
        ..*services
    };
    #[cfg(windows)]
    let admitted = std::sync::Arc::new(admitted);
    #[cfg(windows)]
    let retaining_runner = {
        let mut resources = solstone_core_system::process::BoundedHelperResources::new();
        resources.retain(admitted.clone());
        crate::windows_cleanup::RetainingToolRunner::new(services.runner, resources)
    };
    #[cfg(windows)]
    let services = &BackupServices {
        runner: &retaining_runner,
        ..*services
    };
    let journal = match admitted.revalidate_for_target() {
        Ok(_) => admitted.callback_target(),
        Err(_) => {
            return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
        }
    };
    let phase = match classify_restore(journal, &destination, mobile.as_ref()) {
        Ok(phase) => phase,
        Err(outcome) => return outcome,
    };
    if let Some(reason) = pre_copy_refusal(journal, matches!(phase, RestorePhase::Adopted { .. })) {
        return unrecorded_outcome(error_draft(reason));
    }
    let active_binding = load_hosted_binding(journal);
    let finish = |journal: &Path,
                  clock: &dyn Clock,
                  recorder: &dyn RestoreRecorder,
                  draft: RestoreDraft| {
        #[cfg(windows)]
        if retaining_runner.cleanup_pending() {
            return unrecorded_outcome(draft);
        }
        if let Some(reason) = restore_pending_rotation_refusal(journal) {
            let mut draft = draft;
            draft.status = "error".into();
            draft.reason_code = Some(reason.into());
            draft.resumable = false;
            return unrecorded_outcome(draft);
        }
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
        {
            let mut draft = draft;
            if admitted.revalidate_for_target().is_err() {
                draft.status = "error".to_owned();
                draft.reason_code = Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED.to_owned());
                draft.resumable = false;
            } else if let Some(binding) = &active_binding
                && save_hosted_binding(journal, binding).is_err()
            {
                draft.status = "error".to_owned();
                draft.reason_code = Some(RESTORE_REASON_RESTORE_IO_FAILED.to_owned());
                draft.resumable = false;
            }
            let mut outcome = crate::restore::finish(journal, clock, recorder, draft);
            if admitted.revalidate_for_target().is_err() {
                outcome.status = "error".to_owned();
                outcome.reason_code = Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED.to_owned());
                outcome.resumable = false;
            }
            outcome
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "ios")))]
        {
            if admitted.revalidate_for_target().is_err() {
                return unrecorded_outcome(error_draft(
                    RESTORE_REASON_DESTINATION_ADMISSION_FAILED,
                ));
            }
            if let Some(binding) = &active_binding
                && save_hosted_binding(journal, binding).is_err()
            {
                return crate::restore::finish(
                    journal,
                    clock,
                    recorder,
                    error_draft(RESTORE_REASON_RESTORE_IO_FAILED),
                );
            }
            crate::restore::finish(journal, clock, recorder, draft)
        }
    };
    let canonical = match parse_recovery_key(entered_recovery_key) {
        Ok(key) => key,
        Err(_) => {
            return finish(
                journal,
                services.clock,
                recorder,
                error_draft(RESTORE_REASON_INVALID_KEY),
            );
        }
    };
    let env = match backend(&destination) {
        Ok(env) => env,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    let snapshots = match restic(
        services,
        &admitted,
        vec!["snapshots".into()],
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_LIST_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_SNAPSHOT_LIST_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    if snapshots.returncode != 0 {
        let reason = recognized_returncode_reason(snapshots.returncode)
            .unwrap_or(RESTORE_REASON_SNAPSHOT_LIST_FAILED);
        return finish(journal, services.clock, recorder, error_draft(reason));
    }
    let (snapshot, tree_path) = match &phase {
        RestorePhase::Desktop
        | RestorePhase::First {
            selection: None, ..
        } => {
            let snapshot = match select_journal_snapshot(snapshots.json.as_ref()) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return finish(
                        journal,
                        services.clock,
                        recorder,
                        error_draft(catalog_reason(error)),
                    );
                }
            };
            (
                snapshot.clone(),
                crate::restic_tree_path(Path::new(&snapshot.path)),
            )
        }
        RestorePhase::First {
            selection: Some(selection),
            ..
        } => {
            let snapshot = match snapshot_with_id(snapshots.json.as_ref(), &selection.snapshot_id) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return finish(
                        journal,
                        services.clock,
                        recorder,
                        error_draft(catalog_reason(error)),
                    );
                }
            };
            (snapshot, selection.subtree.clone())
        }
        RestorePhase::Adopted { record } => {
            let snapshot = match snapshot_with_id(snapshots.json.as_ref(), &record.snapshot_id) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return finish(
                        journal,
                        services.clock,
                        recorder,
                        error_draft(catalog_reason(error)),
                    );
                }
            };
            (snapshot, record.subtree.clone())
        }
    };
    if services
        .runner
        .cancellation()
        .is_some_and(|cancellation| cancellation.is_cancelled())
    {
        return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
    }
    let target_path = match admitted.revalidate_for_target() {
        Ok(path) => path,
        Err(_) => {
            return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
        }
    };
    let mut obligation = match &phase {
        RestorePhase::Desktop => None,
        RestorePhase::Adopted { record } => Some(record.clone()),
        RestorePhase::First { identity, .. } => {
            match solstone_core_journal_io::whole_journal_restore::publish_whole_journal_restore(
                journal,
                &source_from(identity, &destination),
                &snapshot.id,
                &tree_path,
            ) {
                Ok(record) => Some(record),
                Err(error) => {
                    log::warn!("whole-journal restore obligation was not published: {error}");
                    return unrecorded_outcome(error_draft(RESTORE_REASON_RESTORE_IO_FAILED));
                }
            }
        }
    };
    if let Some(record) = obligation.as_mut() {
        if let Some(request) = mobile.as_mut()
            && let Err(error) = (request.drain)()
        {
            log::warn!("whole-journal restore waited for admitted work: {error}");
            return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
        }
        if !solstone_core_journal_io::whole_journal_restore::whole_journal_restore_matches(
            journal, record,
        ) {
            return unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED));
        }
        if matches!(phase, RestorePhase::Adopted { .. }) {
            match solstone_core_journal_io::whole_journal_restore::adopt_whole_journal_restore(
                journal, record,
            ) {
                Ok(renewed) => *record = renewed,
                Err(error) => {
                    log::warn!("whole-journal restore obligation was not adopted: {error}");
                    return unrecorded_outcome(error_draft(
                        RESTORE_REASON_DESTINATION_ADMISSION_FAILED,
                    ));
                }
            }
        }
    }
    let restored = match restic(
        services,
        &admitted,
        restore_args(&snapshot.id, &tree_path, &target_path.display().to_string()),
        &destination,
        &canonical,
        &env,
        true,
        RESTORE_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_RESTORE_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => return finish(journal, services.clock, recorder, error_draft(reason)),
    };
    if restored.returncode != 0 {
        let reason = recognized_returncode_reason(restored.returncode)
            .unwrap_or(RESTORE_REASON_RESTORE_FAILED);
        return finish(journal, services.clock, recorder, error_draft(reason));
    }
    let selected_regular_files = if cfg!(any(target_os = "linux", target_os = "android")) {
        None
    } else {
        snapshot.regular_files
    };
    let (counters, summary_missing) =
        match restore_counters(restored.json.as_ref(), selected_regular_files) {
            Some(counters) => (counters, false),
            None => (RestoreCounters::default(), true),
        };
    // Restored configuration may contain a still-unresolved transaction. Neither
    // the entered key nor ordinary restore bookkeeping may overwrite its material.
    if let Some(reason) = restore_pending_rotation_refusal(journal) {
        return unrecorded_outcome(counters.into_draft("error", Some(reason.into()), false, false));
    }
    let check = match restic(
        services,
        &admitted,
        vec!["check".into()],
        &destination,
        &canonical,
        &env,
        false,
        RESTORE_CHECK_TIMEOUT_SECONDS,
        // This opaque code intentionally covers every RunnerError variant, not only I/O.
        RESTORE_REASON_INTEGRITY_CHECK_IO_FAILED,
    ) {
        Ok(output) => output,
        Err(reason) => {
            return finish(
                journal,
                services.clock,
                recorder,
                counters.into_draft("error", Some(reason.to_owned()), false, false),
            );
        }
    };
    let (status, reason_code, integrity_ok) = if check.returncode == 0 {
        if summary_missing {
            (
                "degraded",
                Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING.to_owned()),
                true,
            )
        } else {
            ("ok", None, true)
        }
    } else if matches!(check.returncode, 11 | 124) {
        (
            "degraded",
            Some(RESTORE_REASON_INTEGRITY_UNVERIFIED.to_owned()),
            false,
        )
    } else {
        (
            "degraded",
            Some(RESTORE_REASON_INTEGRITY_FAILED.to_owned()),
            false,
        )
    };
    if obligation.is_some() && !integrity_ok {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft("error", reason_code, false, false),
        );
    }
    let admission_failed = || {
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "ios"))]
        {
            finish(
                journal,
                services.clock,
                recorder,
                counters.into_draft(
                    "error",
                    Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED.to_owned()),
                    integrity_ok,
                    false,
                ),
            )
        }
        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "ios")))]
        {
            unrecorded_outcome(error_draft(RESTORE_REASON_DESTINATION_ADMISSION_FAILED))
        }
    };
    if admitted.revalidate_for_target().is_err() {
        return admission_failed();
    }
    if services
        .journal_maintenance
        .rebuild_body_history(journal)
        .is_err()
    {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_BODY_REBUILD_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if admitted.revalidate_for_target().is_err() {
        return admission_failed();
    }
    if set_destination(journal, &destination).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_DESTINATION_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if admitted.revalidate_for_target().is_err() {
        return admission_failed();
    }
    if set_recovery_key(journal, &canonical).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_RECOVERY_KEY_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    if admitted.revalidate_for_target().is_err() {
        return admission_failed();
    }
    if set_recovery_key_confirmed(journal, true).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_RECOVERY_CONFIRMATION_PUBLISH_FAILED.to_owned()),
                integrity_ok,
                false,
            ),
        );
    }
    let resumable = get_backup_config(journal)
        .ok()
        .and_then(|config| {
            config
                .get("daily_key")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .is_some_and(|key| !key.is_empty());
    if admitted.revalidate_for_target().is_err() {
        return admission_failed();
    }
    if services.journal_maintenance.full_scan(journal).is_err() {
        return finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(
                "error",
                Some(RESTORE_REASON_FULL_SCAN_FAILED.to_owned()),
                integrity_ok,
                resumable,
            ),
        );
    }
    retire_obligation(
        journal,
        obligation.as_ref(),
        finish(
            journal,
            services.clock,
            recorder,
            counters.into_draft(status, reason_code, integrity_ok, resumable),
        ),
    )
}

#[cfg(test)]
mod tests {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod cooperation;

    use super::*;
    use crate::engine::{JournalMaintenance, JournalMaintenanceError, NativeRestoreRecorder};
    use crate::hosted_runtime::{HttpError, HttpRequest, HttpResponse, HttpTransport};
    use crate::runner::{ToolOutput, ToolRequest, ToolRunner};
    use crate::test_support::RestoreRecorderSpy;
    use solstone_core_journal_io::whole_journal_restore::{
        WholeJournalRestoreReadiness, WholeJournalRestoreSource, begin_target_use,
        publish_whole_journal_restore, read_whole_journal_restore, wait_for_target_use_to_end,
    };
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::sync::mpsc;
    use std::thread;

    const SNAPSHOT_ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Script {
        outputs: RefCell<VecDeque<ToolOutput>>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Script {
        fn new(outputs: Vec<ToolOutput>) -> Self {
            Self {
                outputs: RefCell::new(outputs.into()),
                calls: RefCell::new(vec![]),
            }
        }
    }

    impl ToolRunner for Script {
        fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
            self.calls.borrow_mut().push(
                request
                    .argv
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(self
                .outputs
                .borrow_mut()
                .pop_front()
                .expect("script output"))
        }
    }

    struct Http;
    impl HttpTransport for Http {
        fn execute(&self, _: &HttpRequest) -> Result<HttpResponse, HttpError> {
            panic!("whole-journal restore does not use hosted HTTP")
        }
    }

    struct TestClock;
    impl Clock for TestClock {
        fn now_unix(&self) -> i64 {
            50
        }

        fn iso_week(&self) -> u8 {
            1
        }
    }

    struct Maintenance;
    impl JournalMaintenance for Maintenance {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }

    #[allow(dead_code)]
    struct RecordedCall {
        status: String,
        reason: Value,
        files_expected: Value,
        files_restored: Value,
        bytes: Value,
    }

    struct Recorder {
        fail: bool,
        calls: RefCell<Vec<RecordedCall>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                fail: false,
                calls: RefCell::new(vec![]),
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                calls: RefCell::new(vec![]),
            }
        }
    }

    impl RestoreRecorder for Recorder {
        fn record(
            &self,
            _: &Path,
            status: &str,
            _: Value,
            reason: Value,
            scope: &str,
            day: Value,
            segments_selected: Value,
            segments_restored: Value,
            files_expected: Value,
            files_restored: Value,
            bytes_expected: Value,
            bytes_restored: Value,
        ) -> Result<(), solstone_core_backup::BackupError> {
            assert_eq!(scope, "journal");
            assert_eq!(day, Value::Null);
            assert_eq!(segments_selected, json!(0));
            assert_eq!(segments_restored, json!(0));
            self.calls.borrow_mut().push(RecordedCall {
                status: status.to_owned(),
                reason,
                files_expected,
                files_restored,
                bytes: json!([bytes_expected, bytes_restored]),
            });
            if self.fail {
                Err(solstone_core_backup::BackupError::InvalidRestoreStatus)
            } else {
                Ok(())
            }
        }
    }

    fn output(returncode: i32, stdout: &str) -> ToolOutput {
        ToolOutput {
            returncode,
            stdout: stdout.as_bytes().to_vec(),
            stderr: vec![],
        }
    }

    fn catalog() -> String {
        format!(
            "[{{\"id\":\"{SNAPSHOT_ID}\",\"time\":\"2026-01-01T00:00:00.000000000+00:00\",\"paths\":[\"/journal\"]}}]"
        )
    }

    /// Output captured from the pinned restic 0.19.0. The journal held two
    /// regular files, one of them in a subdirectory, and one symlink. It was
    /// backed up, restored into an empty target (`RESTORE`), then restored into
    /// that same target again (`RESTORE_AGAIN`). The `EMPTY_` pair is the same
    /// sequence for a journal with no entries.
    const CAPTURED_SNAPSHOTS: &str = include_str!("../fixtures/restic-0.19.0/snapshots.json");
    const CAPTURED_SNAPSHOT_ID: &str =
        "987b5fc44af145e44c5b8c0d0bbdf2f86c2f40192e1347be002ab1c9722a9cac";
    const CAPTURED_RESTORE: &str = include_str!("../fixtures/restic-0.19.0/restore.jsonl");
    const CAPTURED_RESTORE_AGAIN: &str =
        include_str!("../fixtures/restic-0.19.0/restore-again.jsonl");
    const CAPTURED_EMPTY_SNAPSHOTS: &str =
        include_str!("../fixtures/restic-0.19.0/empty-snapshots.json");
    const CAPTURED_EMPTY_RESTORE: &str =
        include_str!("../fixtures/restic-0.19.0/empty-restore.jsonl");

    fn destination() -> Destination {
        Destination {
            repository: "repo".into(),
            backend: "s3".into(),
            credentials: json!({"access_key_id":"access","secret_access_key":"secret"})
                .as_object()
                .expect("credentials")
                .clone(),
        }
    }

    fn services<'a>(
        runner: &'a dyn ToolRunner,
        clock: &'a dyn Clock,
        maintenance: &'a dyn JournalMaintenance,
    ) -> BackupServices<'a> {
        static HTTP: Http = Http;
        BackupServices {
            runner,
            http: &HTTP,
            clock,
            restic: Some((Path::new("/fixture/restic")).into()),
            rclone_path: None,
            version: "test",
            journal_maintenance: maintenance,
        }
    }

    fn restore_with(catalog: &str, summary: &str) -> (RestoreOutcome, RestoreRecorderSpy) {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![output(0, catalog), output(0, summary), output(0, "")]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = RestoreRecorderSpy::new();
        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        (outcome, recorder)
    }

    fn assert_recorded_counters(
        recorder: &RestoreRecorderSpy,
        status: &str,
        files_expected: Value,
        files_restored: Value,
        bytes_expected: Value,
        bytes_restored: Value,
    ) {
        let calls = recorder.calls();
        assert_eq!(calls.len(), 1);
        let record = &calls[0];
        assert_eq!(record.status, status);
        assert_eq!(record.files_expected, files_expected);
        assert_eq!(record.files_restored, files_restored);
        assert_eq!(record.bytes_expected, bytes_expected);
        assert_eq!(record.bytes_restored, bytes_restored);
    }

    fn selected_file_count(files: u64) -> Option<u64> {
        if cfg!(any(target_os = "linux", target_os = "android")) {
            None
        } else {
            Some(files)
        }
    }

    #[test]
    fn absent_restic_path_returns_existing_unavailable_reason_and_records_once() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = Recorder::new();
        let mut runtime = services(&runner, &clock, &maintenance);
        runtime.restic = None;

        let outcome = restore_journal(
            journal.path(),
            &runtime,
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTIC_UNAVAILABLE)
        );
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(recorder.calls.borrow().len(), 1);
    }

    #[test]
    fn invalid_key_records_once_without_a_runner_call() {
        let journal = tempfile::tempdir().expect("journal");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = Recorder::new();

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            "not a recovery key",
        );

        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INVALID_KEY)
        );
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(recorder.calls.borrow().len(), 1);
    }

    #[test]
    fn selects_full_snapshot_id_and_counts_only_regular_files() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = crate::test_support::ArgvResticFixture::new(
            CAPTURED_SNAPSHOTS,
            output(0, CAPTURED_RESTORE),
            output(0, ""),
        );
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = RestoreRecorderSpy::new();

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            &stored.recovery_key,
        );

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.files_expected, selected_file_count(2));
        assert_eq!(outcome.files_restored, selected_file_count(2));
        assert_eq!(outcome.bytes_expected, Some(11));
        assert_eq!(outcome.bytes_restored, Some(11));
        assert_eq!(
            runner.calls()[0],
            vec!["snapshots".to_owned(), "--json".to_owned()]
        );
        assert_eq!(
            runner.calls()[1][..2],
            [
                "restore".to_owned(),
                format!("{CAPTURED_SNAPSHOT_ID}:/tmp/journal")
            ]
        );
        for pattern in RESTORE_KEEPS_LOCAL {
            assert!(
                runner.calls()[1]
                    .windows(2)
                    .any(|pair| pair == ["--exclude", pattern]),
                "restore keeps {pattern} local"
            );
        }
        assert_recorded_counters(
            &recorder,
            "ok",
            json!(selected_file_count(2)),
            json!(selected_file_count(2)),
            json!(11),
            json!(11),
        );
        assert!(runner.refusals().is_empty());
    }

    #[test]
    fn snapshot_binding_cannot_replace_the_active_binding_on_success_or_failure() {
        struct BindingRestore<'a> {
            script: Script,
            journal: &'a Path,
        }
        impl ToolRunner for BindingRestore<'_> {
            fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
                if request.argv.first().is_some_and(|arg| arg == "restore") {
                    let mut retired = solstone_core_backup::load_hosted_binding(self.journal)
                        .expect("active binding");
                    retired.broker_token = "retired-token-from-snapshot".into();
                    solstone_core_backup::save_hosted_binding(self.journal, &retired)
                        .expect("snapshot binding");
                }
                self.script.run(request)
            }
        }
        for restore_exit in [0, 1] {
            let journal = tempfile::tempdir().expect("journal");
            let keys = solstone_core_backup::generate_and_store_keys(journal.path()).unwrap();
            let active = solstone_core_backup::HostedBinding {
                broker_endpoint: "https://services.example".into(),
                account_id: "owner".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "owner/instance/".into(),
                broker_token: "fresh-consent-token".into(),
            };
            solstone_core_backup::save_hosted_binding(journal.path(), &active).unwrap();
            let runner = BindingRestore {
                script: Script::new(vec![
                    output(0, CAPTURED_SNAPSHOTS),
                    output(restore_exit, CAPTURED_RESTORE),
                    output(0, ""),
                ]),
                journal: journal.path(),
            };
            let outcome = restore_journal(
                journal.path(),
                &services(&runner, &TestClock, &Maintenance),
                &RestoreRecorderSpy::new(),
                destination(),
                &keys.recovery_key,
            );
            assert_eq!(
                outcome.status,
                if restore_exit == 0 { "ok" } else { "error" }
            );
            assert_eq!(
                solstone_core_backup::load_hosted_binding(journal.path()),
                Some(active)
            );
        }
    }

    #[test]
    fn restoring_over_matching_files_counts_only_files_written() {
        let (outcome, recorder) = restore_with(CAPTURED_SNAPSHOTS, CAPTURED_RESTORE_AGAIN);

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert_eq!(outcome.files_expected, selected_file_count(0));
        assert_eq!(outcome.files_restored, selected_file_count(0));
        assert_eq!(outcome.bytes_expected, Some(0));
        assert_eq!(outcome.bytes_restored, Some(0));
        assert_recorded_counters(
            &recorder,
            "ok",
            json!(selected_file_count(0)),
            json!(selected_file_count(0)),
            json!(0),
            json!(0),
        );
    }

    #[test]
    fn snapshot_without_a_summary_reports_file_counts_as_unknown() {
        let (outcome, recorder) = restore_with(&catalog(), CAPTURED_RESTORE);

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert_eq!(outcome.files_expected, None);
        assert_eq!(outcome.files_restored, None);
        assert_eq!(outcome.bytes_expected, Some(11));
        assert_eq!(outcome.bytes_restored, Some(11));
        assert_recorded_counters(
            &recorder,
            "ok",
            Value::Null,
            Value::Null,
            json!(11),
            json!(11),
        );
    }

    #[test]
    fn restore_summary_short_of_the_snapshot_files_reports_file_counts_as_unknown() {
        let (outcome, recorder) = restore_with(
            CAPTURED_SNAPSHOTS,
            r#"[{"message_type":"summary","total_files":1,"files_restored":1,"total_bytes":5,"bytes_restored":5}]"#,
        );

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.files_expected, None);
        assert_eq!(outcome.files_restored, None);
        assert_eq!(outcome.bytes_expected, Some(5));
        assert_eq!(outcome.bytes_restored, Some(5));
        assert_recorded_counters(
            &recorder,
            "ok",
            Value::Null,
            Value::Null,
            json!(5),
            json!(5),
        );
    }

    #[test]
    fn summary_omits_all_zero_counters() {
        let (outcome, recorder) = restore_with(CAPTURED_EMPTY_SNAPSHOTS, CAPTURED_EMPTY_RESTORE);

        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert_eq!(outcome.files_expected, selected_file_count(0));
        assert_eq!(outcome.files_restored, selected_file_count(0));
        assert_eq!(outcome.bytes_expected, Some(0));
        assert_eq!(outcome.bytes_restored, Some(0));
        assert_recorded_counters(
            &recorder,
            "ok",
            json!(selected_file_count(0)),
            json!(selected_file_count(0)),
            json!(0),
            json!(0),
        );
    }

    #[test]
    fn malformed_summary_counter_discards_all_summary_counters() {
        let (outcome, recorder) = restore_with(
            CAPTURED_SNAPSHOTS,
            r#"[{"message_type":"summary","total_files":12.0,"files_restored":2,"total_bytes":9,"bytes_restored":8}]"#,
        );

        assert_eq!(outcome.status, "degraded");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING)
        );
        assert!(outcome.integrity_ok);
        assert_eq!(outcome.files_expected, None);
        assert_eq!(outcome.files_restored, None);
        assert_eq!(outcome.bytes_expected, None);
        assert_eq!(outcome.bytes_restored, None);
        assert_recorded_counters(
            &recorder,
            "degraded",
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Null,
        );
    }

    #[test]
    fn absent_or_non_summary_output_is_degraded_with_null_counters() {
        for output in ["[]", r#"{"message_type":"status"}"#] {
            let (outcome, recorder) = restore_with(CAPTURED_SNAPSHOTS, output);

            assert_eq!(outcome.status, "degraded");
            assert_eq!(
                outcome.reason_code.as_deref(),
                Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING)
            );
            assert_eq!(outcome.files_expected, None);
            assert_eq!(outcome.files_restored, None);
            assert_eq!(outcome.bytes_expected, None);
            assert_eq!(outcome.bytes_restored, None);
            assert_recorded_counters(
                &recorder,
                "degraded",
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            );
        }
    }

    #[test]
    fn failure_replaces_a_prior_successful_last_restore_without_stale_fields() {
        let journal = tempfile::tempdir().expect("journal");
        solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        solstone_core_backup::record_restore_result(
            journal.path(),
            "ok",
            json!(1),
            Value::Null,
            "journal",
            Value::Null,
            json!(0),
            json!(0),
            json!(7),
            json!(7),
            json!(99),
            json!(99),
        )
        .expect("prior result");
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let recorder = NativeRestoreRecorder;

        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &clock, &maintenance),
            &recorder,
            destination(),
            "invalid",
        );

        assert_eq!(outcome.status, "error");
        let last = solstone_core_backup::get_backup_config(journal.path()).expect("config");
        let last = &last["last_restore"];
        assert_eq!(last["status"], "error");
        assert_eq!(last["reason"], RESTORE_REASON_INVALID_KEY);
        assert_eq!(last["scope"], "journal");
        assert_eq!(last["day"], Value::Null);
        assert_eq!(last["segments_selected"], 0);
        assert_eq!(last["segments_restored"], 0);
        assert_eq!(last["files_expected"], Value::Null);
        assert_eq!(last["files_restored"], Value::Null);
        assert_eq!(last["bytes_expected"], Value::Null);
        assert_eq!(last["bytes_restored"], Value::Null);
    }

    #[test]
    fn recording_failure_is_a_single_attempt_and_preserves_the_runtime_outcome() {
        let clock = TestClock;
        let recorder = Recorder::failing();
        let outcome = publish_restore_outcome(
            Path::new("/journal"),
            &clock,
            &recorder,
            RestoreDraft {
                status: "degraded".into(),
                reason_code: Some(RESTORE_REASON_INTEGRITY_UNVERIFIED.into()),
                integrity_ok: false,
                resumable: true,
                files_expected: Some(3),
                files_restored: Some(2),
                bytes_expected: Some(9),
                bytes_restored: Some(8),
            },
        );

        assert_eq!(recorder.calls.borrow().len(), 1);
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INTEGRITY_UNVERIFIED)
        );
        assert_eq!(
            outcome.recording_failure.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(outcome.bytes_restored, Some(8));
    }

    #[test]
    fn recording_failure_reclassifies_an_otherwise_successful_restore() {
        let clock = TestClock;
        let recorder = Recorder::failing();
        let outcome = publish_restore_outcome(
            Path::new("/journal"),
            &clock,
            &recorder,
            RestoreDraft {
                status: "ok".into(),
                reason_code: None,
                integrity_ok: true,
                resumable: true,
                files_expected: Some(3),
                files_restored: Some(3),
                bytes_expected: Some(9),
                bytes_restored: Some(9),
            },
        );

        assert_eq!(recorder.calls.borrow().len(), 1);
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(
            outcome.recording_failure.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert_eq!(outcome.files_restored, Some(3));
    }

    #[test]
    fn restore_journal_fails_closed_when_destination_admission_fails() {
        let runner = Script::new(vec![]);
        let clock = TestClock;
        let maintenance = Maintenance;
        let services = services(&runner, &clock, &maintenance);
        let recorder = Recorder::new();

        let outcome = crate::destination_admission::with_forced_admission_failure(|| {
            restore_journal(
                Path::new("/tmp/unadmitted-destination"),
                &services,
                &recorder,
                destination(),
                "daily",
            )
        });

        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(outcome.recording_failure, None);
        assert_eq!(recorder.calls.borrow().len(), 0);
        assert_eq!(runner.calls.borrow().len(), 0);
    }

    const RESTORE_SUMMARY: &str = r#"[{"message_type":"summary","total_files":1,"files_restored":1,"total_bytes":4,"bytes_restored":4}]"#;
    const NEWER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn obligation_path(journal: &Path) -> std::path::PathBuf {
        journal.join("health/whole-journal-restore.json")
    }

    fn publish_byo(journal: &Path, snapshot: &str, subtree: &str) {
        publish_whole_journal_restore(
            journal,
            &WholeJournalRestoreSource::Byo {
                repository: "repo".into(),
                backend: "s3".into(),
            },
            snapshot,
            subtree,
        )
        .expect("obligation");
    }

    fn two_snapshot_catalog() -> String {
        format!(
            "[{{\"id\":\"{SNAPSHOT_ID}\",\"time\":\"2026-01-01T00:00:00.000000000+00:00\",\"paths\":[\"/journal\"]}},{{\"id\":\"{NEWER_ID}\",\"time\":\"2026-06-01T00:00:00.000000000+00:00\",\"paths\":[\"/newest\"]}}]"
        )
    }

    fn success_script(catalog_body: &str) -> Script {
        Script::new(vec![
            output(0, catalog_body),
            output(0, RESTORE_SUMMARY),
            output(0, ""),
        ])
    }

    fn restore_argv(runner: &Script) -> Option<Vec<String>> {
        runner
            .calls
            .borrow()
            .iter()
            .find(|argv| argv.first().is_some_and(|arg| arg == "restore"))
            .cloned()
    }

    #[allow(clippy::too_many_arguments)]
    fn mobile_with(
        journal: &Path,
        runner: &Script,
        recorder: &dyn RestoreRecorder,
        maintenance: &dyn JournalMaintenance,
        key: &str,
        snapshot: &str,
        subtree: &str,
        drain: &mut dyn FnMut() -> Result<(), String>,
    ) -> RestoreOutcome {
        let clock = TestClock;
        restore_mobile_journal(
            Arc::new(solstone_core_journal_io::JournalRoot::open(journal).unwrap()),
            &services(runner, &clock, maintenance),
            recorder,
            destination(),
            key,
            MobileRestoreIdentity::Byo,
            MobileSelection {
                snapshot_id: snapshot.into(),
                subtree: subtree.into(),
            },
            drain,
        )
    }

    struct FailingRebuild;
    impl JournalMaintenance for FailingRebuild {
        fn rebuild_body_history(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Err(JournalMaintenanceError)
        }

        fn full_scan(&self, _: &Path) -> Result<(), JournalMaintenanceError> {
            Ok(())
        }
    }

    #[test]
    fn mobile_restore_publishes_before_copy_and_clears_after_postwork() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = success_script(&catalog());
        let recorder = Recorder::new();
        let mut published_before_restore = false;
        let mut drain = || {
            published_before_restore = obligation_path(journal.path()).is_file();
            assert!(restore_argv(&runner).is_none());
            Ok(())
        };

        let outcome = mobile_with(
            journal.path(),
            &runner,
            &recorder,
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );

        assert!(published_before_restore);
        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.reason_code, None);
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
        let argv = restore_argv(&runner).expect("restore");
        assert!(
            argv.iter()
                .any(|arg| arg == &format!("{SNAPSHOT_ID}:/recorded"))
        );
        #[cfg(not(target_os = "ios"))]
        assert!(
            argv.windows(2)
                .any(|pair| pair == ["--exclude", "/health/whole-journal-restore.json"])
        );
    }

    #[test]
    fn admitted_use_finishes_before_the_restore_command() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = success_script(&catalog());
        let recorder = Recorder::new();
        let path = journal.path().to_path_buf();
        let marker = path.join("owner-was-using.txt");
        let (started_tx, started_rx) = mpsc::channel();
        let (published_tx, published_rx) = mpsc::channel();
        let marker_for_owner = marker.clone();
        let owner = thread::spawn(move || {
            let guard = begin_target_use(&path).expect("use");
            started_tx.send(()).expect("started");
            published_rx.recv().expect("published");
            fs::write(&marker_for_owner, b"using").expect("marker");
            drop(guard);
        });
        started_rx.recv().expect("owner holds the target");
        let mut drain = || {
            assert!(obligation_path(journal.path()).is_file());
            assert!(restore_argv(&runner).is_none());
            published_tx.send(()).expect("release owner");
            wait_for_target_use_to_end(journal.path());
            assert_eq!(fs::read(&marker).expect("marker"), b"using");
            assert!(restore_argv(&runner).is_none());
            Ok(())
        };

        let outcome = mobile_with(
            journal.path(),
            &runner,
            &recorder,
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );

        owner.join().expect("owner");
        assert_eq!(outcome.status, "ok");
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
        assert!(restore_argv(&runner).is_some());
    }

    #[test]
    fn wrong_key_does_not_publish_or_clear_an_obligation() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![]);
        let recorder = Recorder::new();
        let mut drain = || panic!("wrong key must not drain");
        let outcome = mobile_with(
            journal.path(),
            &runner,
            &recorder,
            &Maintenance,
            "not a recovery key",
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INVALID_KEY)
        );
        assert!(runner.calls.borrow().is_empty());
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));

        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let saved = fs::read(obligation_path(journal.path())).expect("obligation");
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(journal.path().join("config/journal.json")).unwrap())
                .unwrap();
        config["setup"] = json!({"completed_at": 1});
        fs::write(
            journal.path().join("config/journal.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let outcome = mobile_with(
            journal.path(),
            &runner,
            &recorder,
            &Maintenance,
            "not a recovery key",
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_INVALID_KEY)
        );
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), saved);
        assert_eq!(
            solstone_core_backup::get_backup_config(journal.path()).unwrap()["daily_key"],
            Value::String(stored.daily_key)
        );
    }

    #[test]
    fn desktop_pending_obligation_refuses_before_snapshot_selection() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let saved = fs::read(obligation_path(journal.path())).unwrap();
        let runner = Script::new(vec![]);
        let recorder = Recorder::new();
        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &TestClock, &Maintenance),
            &recorder,
            destination(),
            &stored.recovery_key,
        );
        assert_eq!(outcome.status, "error");
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(outcome.recording_failure, None);
        assert_eq!(recorder.calls.borrow().len(), 0);
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), saved);
    }

    #[test]
    fn desktop_without_an_obligation_still_selects_the_latest_snapshot() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = success_script(&two_snapshot_catalog());
        let outcome = restore_journal(
            journal.path(),
            &services(&runner, &TestClock, &Maintenance),
            &Recorder::new(),
            destination(),
            &stored.recovery_key,
        );
        assert_eq!(outcome.status, "ok");
        let argv = restore_argv(&runner).expect("restore");
        assert!(
            argv.iter()
                .any(|arg| arg.starts_with(&format!("{NEWER_ID}:")))
        );
        assert!(argv.iter().all(|arg| !arg.contains(SNAPSHOT_ID)));
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
    }

    #[test]
    fn mobile_retry_uses_the_recorded_snapshot_instead_of_the_newest() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let runner = success_script(&two_snapshot_catalog());
        let outcome = mobile_with(
            journal.path(),
            &runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(outcome.status, "ok");
        let argv = restore_argv(&runner).expect("restore");
        assert!(
            argv.iter()
                .any(|arg| arg == &format!("{SNAPSHOT_ID}:/recorded"))
        );
        assert!(argv.iter().all(|arg| !arg.contains(NEWER_ID)));
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
    }

    #[test]
    fn selection_or_source_mismatch_does_not_clear_or_copy() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let saved = fs::read(obligation_path(journal.path())).unwrap();
        let runner = Script::new(vec![]);
        let recorder = Recorder::new();
        let mut drain = || panic!("mismatch must not drain");
        let mismatched_selection = mobile_with(
            journal.path(),
            &runner,
            &recorder,
            &Maintenance,
            &stored.recovery_key,
            NEWER_ID,
            "/recorded",
            &mut drain,
        );
        assert_eq!(
            mismatched_selection.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(mismatched_selection.recording_failure, None);
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), saved);

        let other = Destination {
            repository: "other".into(),
            ..destination()
        };
        let mismatched_source = restore_mobile_journal(
            Arc::new(solstone_core_journal_io::JournalRoot::open(journal.path()).unwrap()),
            &services(&runner, &TestClock, &Maintenance),
            &recorder,
            other,
            &stored.recovery_key,
            MobileRestoreIdentity::Byo,
            MobileSelection {
                snapshot_id: SNAPSHOT_ID.into(),
                subtree: "/recorded".into(),
            },
            &mut drain,
        );
        assert_eq!(
            mismatched_source.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(recorder.calls.borrow().len(), 0);
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), saved);
    }

    #[test]
    fn unavailable_obligation_refuses_without_rewriting_it() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        fs::create_dir_all(journal.path().join("health")).unwrap();
        fs::write(obligation_path(journal.path()), b"{").unwrap();
        let runner = Script::new(vec![]);
        let mut drain = || panic!("unavailable obligation must not drain");
        let outcome = mobile_with(
            journal.path(),
            &runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(outcome.recording_failure, None);
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), b"{");
    }

    #[test]
    fn pretty_obligation_stays_pending_and_is_not_copied() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let parsed: serde_json::Value =
            serde_json::from_slice(&fs::read(obligation_path(journal.path())).unwrap()).unwrap();
        let pretty = serde_json::to_vec_pretty(&parsed).unwrap();
        fs::write(obligation_path(journal.path()), &pretty).unwrap();
        let runner = Script::new(vec![output(0, &catalog())]);
        let outcome = mobile_with(
            journal.path(),
            &runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(outcome.recording_failure, None);
        assert!(restore_argv(&runner).is_none());
        assert_eq!(fs::read(obligation_path(journal.path())).unwrap(), pretty);
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
    }

    #[test]
    fn unreadable_config_does_not_publish_and_an_adopted_retry_does_not_materialize_it() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let config = journal.path().join("config/journal.json");
        fs::write(&config, b"{").unwrap();
        let runner = Script::new(vec![]);
        let mut drain = || panic!("unreadable first attempt must not drain");
        let first = mobile_with(
            journal.path(),
            &runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut drain,
        );
        assert_eq!(
            first.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
        assert_eq!(fs::read(&config).unwrap(), b"{");

        fs::write(&config, br#"{"backup":{"daily_key":"present"}}"#).unwrap();
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        fs::write(&config, b"{").unwrap();
        let adopted_runner = Script::new(vec![output(0, &catalog()), output(0, RESTORE_SUMMARY)]);
        let adopted = mobile_with(
            journal.path(),
            &adopted_runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(
            adopted.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(adopted.recording_failure, None);
        assert!(restore_argv(&adopted_runner).is_some());
        assert_eq!(fs::read(&config).unwrap(), b"{");
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
    }

    #[test]
    fn drain_failure_and_copy_failure_keep_the_obligation() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        let runner = Script::new(vec![output(0, &catalog())]);
        let drained = mobile_with(
            journal.path(),
            &runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Err("still publishing".into()),
        );
        assert_eq!(
            drained.reason_code.as_deref(),
            Some(RESTORE_REASON_DESTINATION_ADMISSION_FAILED)
        );
        assert_eq!(drained.recording_failure, None);
        assert!(restore_argv(&runner).is_none());
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));

        let copy_runner = Script::new(vec![output(0, &catalog()), output(1, "")]);
        let copied = mobile_with(
            journal.path(),
            &copy_runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(copied.status, "error");
        assert_eq!(
            copied.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_FAILED)
        );
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));
    }

    #[test]
    fn a_stale_attempt_cannot_clear_the_renewed_obligation() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let WholeJournalRestoreReadiness::Pending(original) =
            read_whole_journal_restore(journal.path())
        else {
            panic!("published obligation");
        };
        let copy_runner = Script::new(vec![output(0, &catalog()), output(1, "")]);
        let copied = mobile_with(
            journal.path(),
            &copy_runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(copied.status, "error");
        let WholeJournalRestoreReadiness::Pending(renewed) =
            read_whole_journal_restore(journal.path())
        else {
            panic!("renewed obligation");
        };
        assert_ne!(renewed.attempt, original.attempt);
        assert_eq!(renewed.snapshot_id, original.snapshot_id);
        assert!(
            solstone_core_journal_io::whole_journal_restore::clear_whole_journal_restore(
                journal.path(),
                &original,
            )
            .is_err()
        );
        assert!(
            solstone_core_journal_io::whole_journal_restore::whole_journal_restore_matches(
                journal.path(),
                &renewed,
            )
        );
    }

    #[test]
    fn postwork_and_outcome_failures_do_not_clear_and_degraded_completion_does() {
        let journal = tempfile::tempdir().expect("journal");
        let stored = solstone_core_backup::generate_and_store_keys(journal.path()).expect("keys");
        publish_byo(journal.path(), SNAPSHOT_ID, "/recorded");
        let rebuild_runner = success_script(&catalog());
        let rebuild = mobile_with(
            journal.path(),
            &rebuild_runner,
            &Recorder::new(),
            &FailingRebuild,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(
            rebuild.reason_code.as_deref(),
            Some(RESTORE_REASON_BODY_REBUILD_FAILED)
        );
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));

        let record_runner = success_script(&catalog());
        let recorded = mobile_with(
            journal.path(),
            &record_runner,
            &Recorder::failing(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(recorded.status, "error");
        assert_eq!(
            recorded.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_RECORD_FAILED)
        );
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Pending(_)
        ));

        let degraded_runner =
            Script::new(vec![output(0, &catalog()), output(0, "[]"), output(0, "")]);
        let degraded = mobile_with(
            journal.path(),
            &degraded_runner,
            &Recorder::new(),
            &Maintenance,
            &stored.recovery_key,
            SNAPSHOT_ID,
            "/recorded",
            &mut || Ok(()),
        );
        assert_eq!(degraded.status, "degraded");
        assert!(degraded.integrity_ok);
        assert_eq!(
            degraded.reason_code.as_deref(),
            Some(RESTORE_REASON_RESTORE_SUMMARY_MISSING)
        );
        assert!(matches!(
            read_whole_journal_restore(journal.path()),
            WholeJournalRestoreReadiness::Absent
        ));
    }

    #[cfg(feature = "full-tests")]
    mod pending_rotation {
        use super::*;
        use solstone_core_backup::{PendingExecution, PendingKeyRevocation};
        use std::path::PathBuf;

        fn pending_configuration() -> Vec<u8> {
            let source = tempfile::tempdir().unwrap();
            solstone_core_backup::set_destination(source.path(), &destination()).unwrap();
            let keys = solstone_core_backup::generate_and_store_keys(source.path()).unwrap();
            let backup = get_backup_config(source.path()).unwrap();
            let pending = PendingKeyRevocation {
                original_key_id: "original".into(),
                candidate_binding: solstone_core_backup::candidate_binding(
                    &keys.daily_key,
                    &keys.recovery_key,
                )
                .unwrap(),
                execution: PendingExecution {
                    mode: "byo".into(),
                    repository: destination().repository,
                    backend: destination().backend,
                    credential_binding: solstone_core_backup::credential_binding(
                        &keys.daily_key,
                        &destination().credentials,
                    )
                    .unwrap(),
                },
                setup_fingerprint: solstone_core_backup::backup_setup_fingerprint(&backup, None),
            };
            assert!(
                PendingKeyRevocation::parse(&serde_json::to_value(&pending).unwrap()).is_some()
            );
            let path = source.path().join("config/journal.json");
            let mut config: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            config["backup"]["pending_key_revocation"] = serde_json::to_value(pending).unwrap();
            config["backup"]["confirmed_recovery_key"] = json!(false);
            serde_json::to_vec_pretty(&config).unwrap()
        }

        #[test]
        fn existing_pending_destination_refuses_before_restore_mutation() {
            let original = pending_configuration();
            for pending in [json!(null), json!("unresolved"), json!({"future":true}), serde_json::from_slice::<Value>(&original).unwrap()["backup"]["pending_key_revocation"].clone()] {
                let journal = tempfile::tempdir().unwrap();
                std::fs::create_dir(journal.path().join("config")).unwrap();
                let mut config: Value = serde_json::from_slice(&original).unwrap();
                config["backup"]["pending_key_revocation"] = pending;
                let before = serde_json::to_vec_pretty(&config).unwrap();
                let path = journal.path().join("config/journal.json");
                std::fs::write(&path, &before).unwrap();
                let original_note = journal.path().join("original.txt");
                std::fs::write(&original_note, b"retained original").unwrap();
                let runner = Script::new(vec![]);
                let recorder = RestoreRecorderSpy::new();
                let key = config["backup"]["recovery_key"].as_str().unwrap();
                let outcome = restore_journal(journal.path(), &services(&runner, &TestClock, &Maintenance), &recorder, destination(), key);
                assert_eq!(outcome.status, "error");
                assert_eq!(outcome.reason_code.as_deref(), Some(RESTORE_REASON_RECOVERY_KEY_REVOCATION_PENDING));
                assert!(runner.calls.borrow().is_empty());
                assert!(recorder.calls().is_empty());
                assert_eq!(std::fs::read(path).unwrap(), before);
                assert_eq!(std::fs::read(original_note).unwrap(), b"retained original");
            }
        }

        struct ImportPending {
            config: Vec<u8>,
            restore_exit: i32,
            calls: RefCell<Vec<String>>,
        }
        impl ToolRunner for ImportPending {
            fn run(&self, request: &ToolRequest<'_>) -> io::Result<ToolOutput> {
                let args: Vec<String> = request
                    .argv
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect();
                self.calls.borrow_mut().push(args[0].clone());
                match args[0].as_str() {
                    "snapshots" => Ok(output(0, &catalog())),
                    "restore" => {
                        let target = PathBuf::from(
                            &args[args.iter().position(|arg| arg == "--target").unwrap() + 1],
                        );
                        std::fs::create_dir_all(target.join("config"))?;
                        std::fs::write(target.join("config/journal.json"), &self.config)?;
                        Ok(output(self.restore_exit, CAPTURED_RESTORE))
                    }
                    other => panic!("imported pending transaction must refuse before {other}"),
                }
            }
        }

        #[test]
        fn imported_pending_material_survives_equal_and_different_restore_keys() {
            let imported = pending_configuration();
            let candidate =
                serde_json::from_slice::<Value>(&imported).unwrap()["backup"]["recovery_key"]
                    .as_str()
                    .unwrap()
                    .to_owned();
            for same_key in [false, true] {
                for restore_exit in [0, 1] {
                    let journal = tempfile::tempdir().unwrap();
                    let runner = ImportPending {
                        config: imported.clone(),
                        restore_exit,
                        calls: RefCell::new(vec![]),
                    };
                    let recorder = RestoreRecorderSpy::new();
                    let supplied = if same_key {
                        candidate.clone()
                    } else {
                        solstone_core_backup::generate_recovery_key().unwrap()
                    };
                    let outcome = restore_journal(
                        journal.path(),
                        &services(&runner, &TestClock, &Maintenance),
                        &recorder,
                        destination(),
                        &supplied,
                    );
                    assert_eq!(outcome.status, "error");
                    assert_eq!(
                        outcome.reason_code.as_deref(),
                        Some(RESTORE_REASON_RECOVERY_KEY_REVOCATION_PENDING)
                    );
                    assert!(!outcome.resumable);
                    assert!(recorder.calls().is_empty());
                    assert_eq!(*runner.calls.borrow(), ["snapshots", "restore"]);
                    let path = journal.path().join("config/journal.json");
                    assert_eq!(std::fs::read(&path).unwrap(), imported);
                }
            }
        }
    }
}
