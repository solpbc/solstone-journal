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

pub mod destination;
pub mod engine;
pub mod hosted_runtime;
pub mod install;
pub mod rclone_install;
pub mod readiness;
pub mod repo;
pub mod resolve;
pub mod restore;
mod restore_catalog;
pub mod rotation;
pub mod runner;
pub mod s3_wipe;
pub mod teardown;

pub use destination::{DestinationStatus, validate_destination};
pub use engine::{
    ARCHIVE_TAG, AdmittedCapability, ArchiveCheckResult, ArchiveFileVerdict, BACKUP_EXCLUDES,
    BackupResult, BackupServices, Clock, ClosedToolError, JournalMaintenance,
    JournalMaintenanceError, NativeJournalMaintenance, NativeRestoreRecorder, PruneResult,
    RestoreRecorder, VerificationResult, check_archive_snapshot_files, prepare,
    record_backup_error, run_archive_backup, run_backup, run_prune, run_verification,
};
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use engine::{
    backup_journal_resolved_hook_armed, install_backup_journal_resolved_hook,
    reset_backup_journal_resolved_hook,
};
pub use hosted_runtime::{
    BROKER_TIMEOUT_SECONDS, HostedCredentials, HostedCredsUnavailable, HostedResticSession,
    HttpRequest, HttpResponse, HttpTransport, UreqHttpTransport, fetch_hosted_credentials,
    hosted_append_only_session, hosted_session, operated_destination, operated_repository,
};
pub use install::{RESTIC_LICENSE_TEXT, ensure_restic};
pub use rclone_install::ensure_rclone;
pub use readiness::{
    ARCH_ALIASES, LINUX_TOOL_DIR, MAC_TOOL_DIR, RESTIC_BUNDLE_ENV, RESTIC_BZ2_SHA256,
    RESTIC_SCHEMA_VERSION, RESTIC_VERSION, select_restic_asset,
};
pub use repo::{
    ResticKeyError, add_recovery_key, capture_current_key_id, init_repository, remove_key,
};
pub use resolve::{ResolvedTools, ToolInstallDirs, resolve_operational_tools, resolve_tools};
pub use restore::{RestoreDraft, RestoreOutcome, publish_restore_outcome, restore_journal};
pub use rotation::{RotationResult, rotate_recovery_key};
pub use runner::{
    ResticResult, SystemToolRunner, ToolOutput, ToolRequest, ToolRunner, reason_for_returncode,
    run_restic, select_summary,
};
pub use s3_wipe::{DELETE_OBJECT_BATCH_SIZE, S3Credentials, WipeResult, wipe_prefix};
pub use teardown::{TeardownResult, teardown_backup};

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub mod test_support;
