// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_backup::archive_readback::ReadbackError;
use solstone_core_backup_runtime::ArchiveError;
use solstone_core_cli_copy as copy;

pub(super) fn stopped_archive(error: ArchiveError) -> String {
    match error {
        ArchiveError::NotConfigured => {
            copy::text("journal.cli.offload.stopped.archive.not.configured")
        }
        ArchiveError::ConfigurationUnavailable => {
            copy::text("journal.cli.offload.stopped.archive.configuration.unavailable")
        }
        ArchiveError::ConfigurationChanged => {
            copy::text("journal.cli.offload.stopped.archive.configuration.changed")
        }
        ArchiveError::JournalIdentityChanged => {
            copy::text("journal.cli.offload.stopped.archive.journal.identity.changed")
        }
        ArchiveError::ToolUnavailable => {
            copy::text("journal.cli.offload.stopped.archive.tool.unavailable")
        }
        ArchiveError::InvalidCredentialExpiry => {
            copy::text("journal.cli.offload.stopped.archive.invalid.credential.expiry")
        }
        ArchiveError::CredentialsExpired => {
            copy::text("journal.cli.offload.stopped.archive.credentials.expired")
        }
        ArchiveError::DeadlineExceeded => {
            copy::text("journal.cli.offload.stopped.archive.deadline.exceeded")
        }
        ArchiveError::ClockChanged => {
            copy::text("journal.cli.offload.stopped.archive.clock.changed")
        }
        ArchiveError::Cancelled => copy::text("journal.cli.offload.stopped.archive.cancelled"),
        ArchiveError::RepositoryIdentityInvalid => {
            copy::text("journal.cli.offload.stopped.archive.repository.identity.invalid")
        }
        ArchiveError::RepositoryIdentityChanged => {
            copy::text("journal.cli.offload.stopped.archive.repository.identity.changed")
        }
        ArchiveError::InvalidSelection => {
            copy::text("journal.cli.offload.stopped.archive.invalid.selection")
        }
        ArchiveError::SourceUnavailable => {
            copy::text("journal.cli.offload.stopped.archive.source.unavailable")
        }
        ArchiveError::SnapshotIdentityInvalid => {
            copy::text("journal.cli.offload.stopped.archive.snapshot.identity.invalid")
        }
        ArchiveError::CredentialsUnavailable { reason } | ArchiveError::ToolFailed { reason } => {
            copy::display(
                "journal.cli.offload.stopped.tool.reason",
                [("reason", &(reason))],
            )
        }
        ArchiveError::NativeStart { status, .. } => copy::display(
            "journal.cli.offload.stopped.native.start",
            [("status", &(status))],
        ),
        ArchiveError::Readback(error) => stopped_readback(error),
    }
}

pub(super) fn stopped_readback(error: ReadbackError) -> String {
    match error {
        ReadbackError::Unsupported => {
            copy::text("journal.cli.offload.stopped.readback.unsupported")
        }
        ReadbackError::NotConfigured => {
            copy::text("journal.cli.offload.stopped.readback.not.configured")
        }
        ReadbackError::ToolUnavailable => {
            copy::text("journal.cli.offload.stopped.readback.tool.unavailable")
        }
        ReadbackError::ToolChanged => {
            copy::text("journal.cli.offload.stopped.readback.tool.changed")
        }
        ReadbackError::UnsupportedToolVersion => {
            copy::text("journal.cli.offload.stopped.readback.unsupported.tool.version")
        }
        ReadbackError::UnsupportedBackend => {
            copy::text("journal.cli.offload.stopped.readback.unsupported.backend")
        }
        ReadbackError::CredentialsUnavailable => {
            copy::text("journal.cli.offload.stopped.readback.credentials.unavailable")
        }
        ReadbackError::InvalidSelection => {
            copy::text("journal.cli.offload.stopped.readback.invalid.selection")
        }
        ReadbackError::RepositoryIdentityChanged => {
            copy::text("journal.cli.offload.stopped.archive.repository.identity.changed")
        }
        ReadbackError::InvalidRepositoryIdentity => {
            copy::text("journal.cli.offload.stopped.archive.repository.identity.invalid")
        }
        ReadbackError::SnapshotNodeUnavailable => {
            copy::text("journal.cli.offload.stopped.readback.snapshot.node.unavailable")
        }
        ReadbackError::IncompleteDiscovery | ReadbackError::IncompleteDiscoveryAt(_) => {
            copy::text("journal.cli.offload.stopped.readback.incomplete.discovery")
        }
        ReadbackError::ToolFailed => copy::text("journal.cli.offload.stopped.readback.tool.failed"),
        ReadbackError::NativeFailure => {
            copy::text("journal.cli.offload.stopped.readback.native.failure")
        }
        ReadbackError::TargetRootChanged => {
            copy::text("journal.cli.offload.stopped.readback.target.root.changed")
        }
        ReadbackError::Admission(error) => stopped_archive(error.into()),
    }
}
