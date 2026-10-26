// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native runtime primitives for journal backup.
//!
//! Resolver fault-injection hooks are not part of a default build:
//!
//! ```compile_fail,E0432
//! use solstone_core_backup_runtime::install_backup_journal_resolved_hook;
//! ```
//!
//! ```compile_fail,E0432
//! use solstone_core_backup_runtime::reset_backup_journal_resolved_hook;
//! ```
//!
//! ```compile_fail,E0432
//! use solstone_core_backup_runtime::backup_journal_resolved_hook_armed;
//! ```

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "android",
    windows
))]
mod controlled_http;
pub mod destination;
pub mod destination_admission;
pub mod engine;
pub mod hosted_runtime;
#[cfg(target_os = "ios")]
pub mod ios_callable;
#[cfg(any(target_os = "ios", test))]
mod ios_linked_command;
pub mod repo;
pub mod resolve;
pub mod restore;
mod restore_catalog;
pub mod rotation;
pub mod runner;
pub mod s3_wipe;
pub mod schedule;
pub mod teardown;
pub mod teardown_admission;
pub use engine::backup_enabled;
pub use schedule::effective_backup_schedule;
#[cfg(windows)]
pub mod windows_cleanup;
#[cfg(windows)]
pub mod windows_tool;
#[cfg(windows)]
pub use windows_tool::verify_package_and_get_tool;

pub use destination::{DestinationStatus, validate_destination};
#[cfg(any(test, feature = "test-hooks", feature = "test-support"))]
pub use destination_admission::with_forced_admission_failure;
pub use destination_admission::{
    AdmittedDestination, DestinationAdmissionError, admit_restore_destination,
};
pub use engine::{
    ARCHIVE_TAG, AdmittedArchive, AdmittedCapability, ArchiveAdmissionEffects,
    ArchiveAdmissionFailure, ArchiveCheckResult, ArchiveError, ArchiveFailure, ArchiveFileVerdict,
    ArchiveRemoteEffects, ArchiveSnapshot, BackupResult, BackupServices, Clock, ClosedToolError,
    DirectReadFinish, DirectReadFinishOwner, DirectReadFinishState, InstalledArchiveRead,
    JournalMaintenance, JournalMaintenanceError, NativeJournalMaintenance, NativeRestoreRecorder,
    OwnedAdmittedArchive, OwnedAdmittedArchiveHold, OwnedArchiveAdmissionFailure,
    OwnedDirectReadFinish, OwnedInstalledArchiveCreator, OwnedInstalledArchiveRead, PruneResult,
    RestoreRecorder, UnreadableSources, VerificationResult, admit_archive,
    admit_archive_with_budget, admit_prepared_archive,
    admit_prepared_archive_current_read_installed, admit_prepared_archive_owned_creator_installed,
    admit_prepared_archive_owned_current_read_installed,
    admit_prepared_archive_owned_read_installed, admit_prepared_archive_read,
    admit_prepared_archive_read_installed, check_archive_snapshot_files, prepare,
    run_archive_backup, run_prune, run_verification,
};

// The legacy direct entry points: production goes through admission, so only
// test builds can reach these.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use engine::{
    backup_journal_resolved_hook_armed, backup_path_resolution_attempts,
    backup_record_failure_hook_armed, backup_record_failure_hook_consumed_target,
    backup_tool_resolution_started_hook_armed, install_backup_journal_resolved_hook,
    install_backup_record_failure_hook, install_backup_tool_resolution_started_hook,
    reset_backup_journal_resolved_hook, reset_backup_path_resolution_attempts,
    reset_backup_record_failure_hook, reset_backup_tool_resolution_started_hook,
};
#[cfg(any(feature = "test-hooks", feature = "test-support"))]
pub use engine::{record_backup_error, run_backup};
pub use hosted_runtime::{
    BROKER_TIMEOUT_SECONDS, HostedCredentials, HostedCredsUnavailable, HostedResticSession,
    HttpRequest, HttpResponse, HttpTransport, UreqHttpTransport, fetch_hosted_credentials,
    hosted_append_only_session, hosted_session, operated_destination, operated_repository,
};
pub use repo::{
    KeyLabelOutcome, NEUTRAL_KEY_LABEL, ResticKeyError, add_recovery_key, capture_current_key_id,
    init_repository, log_key_label_outcome, neutralize_key_labels, remove_key,
};
pub use resolve::{
    ResolvedResticInvocation, ResolvedTools, resolve_operational_tools, resolve_rclone_member,
    resolve_restic_member, resolve_tools,
};
pub use restore::{RestoreDraft, RestoreOutcome, publish_restore_outcome, restore_journal};
pub use rotation::{RotationResult, rotate_recovery_key};
#[cfg(all(unix, feature = "test-hooks"))]
pub use rotation::{arm_rotation_parent_sync_fault, rotation_parent_sync_fault_observation};
pub use runner::{
    PassedHandle, ResticInvocation, ResticResult, SystemToolRunner, ToolOutput, ToolRequest,
    ToolRunner, reason_for_returncode, run_restic, select_summary,
};
pub use s3_wipe::{DELETE_OBJECT_BATCH_SIZE, S3Credentials, WipeResult};
pub use teardown_admission::{
    CarrierTeardownAdmission, ClearedTeardownAdmission, TeardownAdmission,
    TeardownAdmissionRefusal, TeardownCarrierFailure, TeardownCarrierRefusal, TeardownClearFailure,
    TeardownClearRefusal, TeardownReaderRefusal, admit_teardown,
};

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support;

mod restic_paths;
pub use restic_paths::{restic_filesystem_path, restic_tree_path};
