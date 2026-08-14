// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native runtime primitives for journal backup.

pub mod destination;
pub mod hosted_runtime;
pub mod install;
pub mod rclone_install;
pub mod readiness;
pub mod repo;
pub mod runner;

pub use destination::{DestinationStatus, validate_destination};
pub use hosted_runtime::{
    BROKER_TIMEOUT_SECONDS, HostedCredentials, HostedCredsUnavailable, HostedResticSession,
    HttpRequest, HttpResponse, HttpTransport, UreqHttpTransport, fetch_hosted_credentials,
    hosted_append_only_session, hosted_session, operated_destination, operated_repository,
};
pub use install::{RESTIC_LICENSE_TEXT, ensure_restic};
pub use rclone_install::ensure_rclone;
pub use readiness::{
    ARCH_ALIASES, LINUX_TOOL_DIR, MAC_TOOL_DIR, RESTIC_BUNDLE_ENV, RESTIC_BZ2_SHA256,
    RESTIC_SCHEMA_VERSION, RESTIC_VERSION, check_restic_ready, select_restic_asset,
};
pub use repo::{
    ResticKeyError, add_recovery_key, capture_current_key_id, init_repository, remove_key,
};
pub use runner::{
    ResticResult, SystemToolRunner, ToolOutput, ToolRequest, ToolRunner, reason_for_returncode,
    run_restic, select_summary,
};
