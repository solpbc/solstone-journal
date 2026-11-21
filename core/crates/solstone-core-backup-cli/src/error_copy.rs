// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{copy, window};
use solstone_core_backup::BackupError;
use solstone_core_backup_runtime::ClosedToolError;
use solstone_core_journal_config_write::{ConfigLoadError, ConfigMutationError};
use solstone_core_journal_io::errors::LockError;

pub(super) fn backup_error(error: BackupError) -> String {
    match error {
        BackupError::ConfigLoad(ConfigLoadError::Corrupt { path, .. })
        | BackupError::ConfigMutation(ConfigMutationError::Load(ConfigLoadError::Corrupt {
            path,
            ..
        })) => window::strings(
            copy::cli_locale(),
            "journal.window.facets.awareness.response.detail.unreadable.settings",
            [("settings_path", path.display().to_string().as_str())],
        ),
        BackupError::ConfigMutation(ConfigMutationError::Lock(LockError::Timeout(timeout))) => {
            copy::display(
                "journal.cli.backup.config.lock.timeout",
                [
                    ("path", &timeout.path.display()),
                    ("seconds", &timeout.timeout.as_secs_f64()),
                ],
            )
        }
        BackupError::ConfigMutation(error) => error.to_string(),
        BackupError::Entropy => copy::text("journal.cli.backup.config.entropy"),
        BackupError::HostedWrite => copy::text("journal.cli.backup.config.hosted.write"),
        BackupError::HostedDelete => copy::text("journal.cli.backup.config.hosted.delete"),
        BackupError::HostedRead => copy::text("journal.cli.backup.config.hosted.read"),
        BackupError::TeardownRecoveryWrite => {
            copy::text("journal.cli.backup.config.teardown.recovery.write")
        }
        BackupError::TeardownRecoveryDelete => {
            copy::text("journal.cli.backup.config.teardown.recovery.delete")
        }
        BackupError::TeardownRecoveryRead => {
            copy::text("journal.cli.backup.config.teardown.recovery.read")
        }
        BackupError::TeardownRecoveryCorrupt => {
            copy::text("journal.cli.backup.config.teardown.recovery.corrupt")
        }
        BackupError::InvalidMode => copy::text("journal.cli.backup.config.invalid.mode"),
        BackupError::InvalidRetentionShape => {
            copy::text("journal.cli.backup.config.invalid.retention.shape")
        }
        BackupError::InvalidRetentionValue => {
            copy::text("journal.cli.backup.config.invalid.retention.value")
        }
        BackupError::InvalidOffloadShape => {
            copy::text("journal.cli.backup.config.invalid.offload.shape")
        }
        BackupError::InvalidOffloadEnabled => {
            copy::text("journal.cli.backup.config.invalid.offload.enabled")
        }
        BackupError::InvalidOffloadBytes => {
            copy::text("journal.cli.backup.config.invalid.offload.bytes")
        }
        BackupError::InvalidOffloadStatus => {
            copy::text("journal.cli.backup.config.invalid.offload.status")
        }
        BackupError::InvalidVerificationStatus => {
            copy::text("journal.cli.backup.config.invalid.verification.status")
        }
        BackupError::InvalidRestoreStatus => {
            copy::text("journal.cli.backup.config.invalid.restore.status")
        }
        BackupError::InvalidRestoreScope => {
            copy::text("journal.cli.backup.config.invalid.restore.scope")
        }
        BackupError::InvalidRestoreCounters => {
            copy::text("journal.cli.backup.config.invalid.restore.counters")
        }
        BackupError::InvalidDestinationShape => {
            copy::text("journal.cli.backup.config.invalid.destination.shape")
        }
        BackupError::StoredKeys => copy::text("journal.cli.backup.config.stored.keys"),
        BackupError::CanonicalRecoveryLength => {
            copy::text("journal.cli.backup.config.canonical.recovery.length")
        }
        BackupError::CanonicalRecoveryCharacters => {
            copy::text("journal.cli.backup.config.canonical.recovery.characters")
        }
        BackupError::RecoveryParse => copy::text("journal.cli.backup.config.recovery.parse"),
        BackupError::MissingCredential(key) => copy::display(
            "journal.cli.backup.config.missing.credential",
            [("key", &key)],
        ),
        BackupError::UnsupportedBackend(_) => {
            copy::text("journal.cli.backup.config.unsupported.backend")
        }
    }
}

pub(super) fn tool_error(error: ClosedToolError) -> String {
    use solstone_core_installed_payload::guidance;
    let id = match error.guidance() {
        guidance::RESTART_UPDATE => Some("journal.window.backup.response.tool.restart.update"),
        guidance::MANIFEST_MISSING => Some("journal.window.backup.response.tool.manifest.missing"),
        guidance::MANIFEST_INVALID => Some("journal.window.backup.response.tool.manifest.invalid"),
        guidance::UNSUPPORTED_LOCATION => {
            Some("journal.window.backup.response.tool.unsupported.location")
        }
        guidance::WRONG_PRODUCT => Some("journal.window.backup.response.tool.wrong.product"),
        guidance::WRONG_TARGET => Some("journal.window.backup.response.tool.wrong.target"),
        guidance::MEMBER_UNREADABLE => {
            Some("journal.window.backup.response.tool.member.unreadable")
        }
        guidance::UNSAFE_PATH => Some("journal.window.backup.response.tool.unsafe.path"),
        guidance::PACKAGE_MISMATCH => Some("journal.window.backup.response.tool.package.mismatch"),
        _ => None,
    };
    let guidance = id.map_or_else(
        || error.guidance().to_owned(),
        |id| window::text(copy::cli_locale(), id),
    );
    format!("{error} detail={} {guidance}", error.detail())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installed_payload::{code, guidance};

    #[test]
    fn missing_manifest_preserves_the_selected_recovery_instruction() {
        for advice in [
            guidance::MANIFEST_MISSING,
            guidance::RESTART_UPDATE,
            "raw tool detail",
        ] {
            let error = ClosedToolError::ResticUnavailable {
                detail: code::MANIFEST_MISSING.into(),
                guidance: advice.into(),
            };
            assert_eq!(
                tool_error(error),
                format!("restic_unavailable detail=manifest-missing {advice}")
            );
        }
    }
}
