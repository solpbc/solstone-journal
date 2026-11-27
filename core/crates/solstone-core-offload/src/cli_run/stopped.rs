// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use solstone_core_backup::archive_readback::ReadbackError;
use solstone_core_backup_runtime::ArchiveError;
use solstone_core_cli_copy as copy;

pub(super) fn stopped_archive(locale: &str, error: ArchiveError) -> String {
    let id = match error {
        ArchiveError::NotConfigured => "journal.cli.offload.stopped.archive.not.configured",
        ArchiveError::ConfigurationUnavailable => {
            "journal.cli.offload.stopped.archive.configuration.unavailable"
        }
        ArchiveError::ConfigurationChanged => {
            "journal.cli.offload.stopped.archive.configuration.changed"
        }
        ArchiveError::ActivityBusy => "journal.cli.offload.stopped.archive.activity.busy",
        ArchiveError::UnresolvedActivity => {
            "journal.cli.offload.stopped.archive.unresolved.activity"
        }
        ArchiveError::ActivityUnavailable => {
            "journal.cli.offload.stopped.archive.activity.unavailable"
        }
        ArchiveError::JournalIdentityChanged => {
            "journal.cli.offload.stopped.archive.journal.identity.changed"
        }
        ArchiveError::ToolUnavailable => "journal.cli.offload.stopped.archive.tool.unavailable",
        ArchiveError::InvalidCredentialExpiry => {
            "journal.cli.offload.stopped.archive.invalid.credential.expiry"
        }
        ArchiveError::CredentialsExpired => {
            "journal.cli.offload.stopped.archive.credentials.expired"
        }
        ArchiveError::DeadlineExceeded => "journal.cli.offload.stopped.archive.deadline.exceeded",
        ArchiveError::ClockChanged => "journal.cli.offload.stopped.archive.clock.changed",
        ArchiveError::Cancelled => "journal.cli.offload.stopped.archive.cancelled",
        ArchiveError::RepositoryIdentityInvalid => {
            "journal.cli.offload.stopped.archive.repository.identity.invalid"
        }
        ArchiveError::RepositoryIdentityChanged => {
            "journal.cli.offload.stopped.archive.repository.identity.changed"
        }
        ArchiveError::InvalidSelection => "journal.cli.offload.stopped.archive.invalid.selection",
        ArchiveError::SourceUnavailable => "journal.cli.offload.stopped.archive.source.unavailable",
        ArchiveError::SnapshotIdentityInvalid => {
            "journal.cli.offload.stopped.archive.snapshot.identity.invalid"
        }
        ArchiveError::CredentialsUnavailable { reason } | ArchiveError::ToolFailed { reason } => {
            return copy::display_for(
                locale,
                "journal.cli.offload.stopped.tool.reason",
                [("reason", &(reason))],
            );
        }
        ArchiveError::NativeStart { status, .. } => {
            return copy::display_for(
                locale,
                "journal.cli.offload.stopped.native.start",
                [("status", &(status))],
            );
        }
        ArchiveError::Readback(error) => return stopped_readback(locale, error),
    };
    copy::render_for(locale, id, &[])
}

pub(super) fn stopped_readback(locale: &str, error: ReadbackError) -> String {
    let id = match error {
        ReadbackError::Unsupported => "journal.cli.offload.stopped.readback.unsupported",
        ReadbackError::NotConfigured => "journal.cli.offload.stopped.readback.not.configured",
        ReadbackError::ToolUnavailable => "journal.cli.offload.stopped.readback.tool.unavailable",
        ReadbackError::ToolChanged => "journal.cli.offload.stopped.readback.tool.changed",
        ReadbackError::UnsupportedToolVersion => {
            "journal.cli.offload.stopped.readback.unsupported.tool.version"
        }
        ReadbackError::UnsupportedBackend => {
            "journal.cli.offload.stopped.readback.unsupported.backend"
        }
        ReadbackError::CredentialsUnavailable => {
            "journal.cli.offload.stopped.readback.credentials.unavailable"
        }
        ReadbackError::InvalidSelection => "journal.cli.offload.stopped.readback.invalid.selection",
        ReadbackError::RepositoryIdentityChanged => {
            "journal.cli.offload.stopped.archive.repository.identity.changed"
        }
        ReadbackError::InvalidRepositoryIdentity => {
            "journal.cli.offload.stopped.archive.repository.identity.invalid"
        }
        ReadbackError::SnapshotNodeUnavailable => {
            "journal.cli.offload.stopped.readback.snapshot.node.unavailable"
        }
        ReadbackError::IncompleteDiscovery | ReadbackError::IncompleteDiscoveryAt(_) => {
            "journal.cli.offload.stopped.readback.incomplete.discovery"
        }
        ReadbackError::ToolFailed => "journal.cli.offload.stopped.readback.tool.failed",
        ReadbackError::NativeFailure => "journal.cli.offload.stopped.readback.native.failure",
        ReadbackError::TargetRootChanged => {
            "journal.cli.offload.stopped.readback.target.root.changed"
        }
        ReadbackError::Admission(error) => return stopped_archive(locale, error.into()),
    };
    copy::render_for(locale, id, &[])
}
