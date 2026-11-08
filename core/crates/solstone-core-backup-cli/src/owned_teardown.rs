// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use solstone_core_backup::archive_admission::ArchiveCancellation;
use solstone_core_backup_runtime::teardown::owned::{StorageKeeps, StorageRetention};
use solstone_core_backup_runtime::teardown_admission::{
    TeardownAdmissionRefusal, TeardownCarrierRefusal, TeardownReaderRefusal,
};
use solstone_core_offload::teardown::{
    OwnedTeardownOutcome, TeardownRunRefusal, run_owned_teardown, storage_retention_sentence,
};

use super::{CliRun, ProductionClock};

pub(super) const PRELAUNCH_HELD: &str =
    "turning off and deleting your backup isn't available here. no backup changes were made.";
pub(super) const BACKUP_ONLY_HELD: &str = "your backup holds the only copy of some of your media, so no backup changes were made. restore everything from the backup page first, then try again.";
pub(super) const DESTINATION_UNSUPPORTED: &str = "your backup's settings are incomplete, so your journal can't reach it, and no backup changes were made.";
pub(super) const TOOL_UNAVAILABLE: &str =
    "the backup program that comes with your journal is missing, so no backup changes were made.";
pub(super) const BACKUP_ONLY_UNPLACED: &str = "your backup holds the only copy of some older media that can't be matched to a recording on this computer, so it can't be restored and no backup changes were made. if your journal was moved or copied, bring back its missing days, then try again.";
pub(super) const OFFLOAD_RECORD_UNAVAILABLE: &str = "the record of what media offload moved into your backup is missing or can't be read, so no backup changes were made. if your journal was moved or copied, bring back its health folder, then try again.";
pub(super) const STORAGE_UNREACHED: &str = "turning off and deleting your backup stopped before anything was deleted, so it's safe to try again.";
pub(super) const NOT_STARTED: &str =
    "turning off and deleting your backup couldn't start. no backup changes were made.";

/// The complete native outcome remains alive while output is borrowed.
#[must_use = "Retain the whole teardown result through printing and exit"]
pub struct OwnedTeardownCliRun {
    outcome: OwnedTeardownOutcome,
    output: CliRun,
}

impl std::fmt::Debug for OwnedTeardownCliRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedTeardownCliRun")
            .field("phase", &self.outcome.phase())
            .field("exit_code", &self.output.exit_code)
            .finish_non_exhaustive()
    }
}

impl OwnedTeardownCliRun {
    fn receive(outcome: OwnedTeardownOutcome) -> Self {
        let output = render(&outcome);
        Self { outcome, output }
    }

    pub fn outcome(&self) -> &OwnedTeardownOutcome {
        &self.outcome
    }

    pub fn output(&self) -> (&str, &str, i32) {
        (
            &self.output.stdout,
            &self.output.stderr,
            self.output.exit_code,
        )
    }
}

pub(super) fn run(journal: &Path) -> OwnedTeardownCliRun {
    let outcome = run_owned_teardown(
        journal,
        Arc::new(ProductionClock),
        ArchiveCancellation::new(),
        env!("CARGO_PKG_VERSION"),
    );
    solstone_core_offload::teardown::record_backup_off(
        journal,
        &outcome,
        solstone_core_removals::Surface::Cli,
    );
    OwnedTeardownCliRun::receive(outcome)
}

/// Coverage refusals the owner can act on, with their reason and message.
fn coverage_refusal(outcome: &OwnedTeardownOutcome) -> Option<(&'static str, &'static str)> {
    let failure = outcome.preparation_failure()?;
    let TeardownCarrierRefusal::Admission(TeardownAdmissionRefusal::Reader(reader)) =
        failure.refusal()
    else {
        return None;
    };
    match reader {
        TeardownReaderRefusal::BackupOnlyHeld => Some(("backup_only_held", BACKUP_ONLY_HELD)),
        TeardownReaderRefusal::BackupOnlyUnplaced => {
            Some(("backup_only_unplaced", BACKUP_ONLY_UNPLACED))
        }
        TeardownReaderRefusal::CatalogUnavailable => {
            Some(("offload_record_unavailable", OFFLOAD_RECORD_UNAVAILABLE))
        }
        TeardownReaderRefusal::RecoveryUnavailable | TeardownReaderRefusal::RecoveryHeld => None,
    }
}

/// The remote leg stopped before any delete, and no local phase ran.
pub(super) fn nothing_deleted(outcome: &OwnedTeardownOutcome) -> bool {
    let attempts = outcome.attempts();
    matches!(outcome.refusal(), Some(TeardownRunRefusal::Remote(_)))
        && outcome
            .remote()
            .is_some_and(|remote| remote.delete_attempts() == 0 && remote.abort_attempts() == 0)
        && !attempts.clear()
        && !attempts.binding()
        && !attempts.retirement()
}

fn retention_code(retention: StorageRetention) -> &'static str {
    match retention {
        StorageRetention::Removes => "removes",
        StorageRetention::Keeps(StorageKeeps::Versions) => "keeps_versions",
        StorageRetention::Keeps(StorageKeeps::ObjectLock) => "keeps_object_lock",
        StorageRetention::Keeps(StorageKeeps::FileLock) => "keeps_file_lock",
        StorageRetention::Keeps(StorageKeeps::HiddenFiles) => "keeps_hidden_files",
        StorageRetention::Unknown => "unknown",
    }
}

fn reason(outcome: &OwnedTeardownOutcome) -> &'static str {
    if let Some((reason, _)) = coverage_refusal(outcome) {
        return reason;
    }
    match outcome.refusal() {
        Some(TeardownRunRefusal::UnsupportedPlatform) => "platform_unavailable",
        Some(TeardownRunRefusal::UnsupportedDestination) => "destination_unavailable",
        Some(TeardownRunRefusal::ToolUnavailable) => "restic_unavailable",
        Some(TeardownRunRefusal::Root(_)) => "journal_unavailable",
        Some(TeardownRunRefusal::Budget(_)) => "deadline_unavailable",
        Some(TeardownRunRefusal::Admission(_)) => "admission_refused",
        Some(TeardownRunRefusal::CarrierPreparation) => "preparation_refused",
        Some(TeardownRunRefusal::Remote(_)) => "remote_refused",
        Some(TeardownRunRefusal::Clear) => "configuration_refused",
        Some(TeardownRunRefusal::Binding) => "binding_refused",
        #[cfg(any(unix, windows))]
        Some(TeardownRunRefusal::Finish(_)) => "finish_refused",
        Some(TeardownRunRefusal::CarrierShared) => "carrier_shared",
        Some(TeardownRunRefusal::RetirementUncertain) => "retirement_uncertain",
        None => "none",
    }
}

fn render(outcome: &OwnedTeardownOutcome) -> CliRun {
    let (status, exit_code) = match outcome {
        OwnedTeardownOutcome::Finished(_) => ("completed", 0),
        OwnedTeardownOutcome::Held(_) => ("held", 1),
        OwnedTeardownOutcome::Unknown(_) => ("unknown", 1),
    };
    let mut stdout = format!("teardown_status={status} reason={}\n", reason(outcome));
    if let Some(remote) = outcome.remote() {
        writeln!(stdout, "delete_attempts={} delete_confirmations={} abort_attempts={} abort_confirmations={} objects_empty={} uploads_empty={}",
            remote.delete_attempts(), remote.delete_confirmations(), remote.abort_attempts(),
            remote.abort_confirmations(), remote.objects_empty(), remote.uploads_empty()).unwrap();
        if let Some(retention) = remote.storage_retention() {
            writeln!(stdout, "storage_retention={}", retention_code(retention)).unwrap();
        }
    }
    let attempts = outcome.attempts();
    writeln!(
        stdout,
        "configuration_attempted={} binding_attempted={} retirement_attempted={}",
        attempts.clear(),
        attempts.binding(),
        attempts.retirement()
    )
    .unwrap();
    if let Some(publication) = outcome.publication() {
        writeln!(stdout, "configuration_published=true directory_flush={:?} durability_uncertain={} observation_uncertain={} confirmation_refused={}",
            publication.directory_flush, publication.durability_uncertainty.is_some(),
            publication.observation_uncertainty.is_some(), publication.confirmation_refusal.is_some()).unwrap();
    }
    #[cfg(any(unix, windows))]
    if let Some(binding) = outcome.binding() {
        writeln!(stdout, "binding_receipt_available=true removal_attempted={} removal_complete={} confirmation_refused={}",
            binding.attempted(), binding.complete(), binding.confirmation().is_err()).unwrap();
    }
    if let Some(retirement) = outcome.retirement() {
        writeln!(
            stdout,
            "retirement_effect={:?} root_durability={:?} confirmation={:?} failure_count={}",
            retirement.effect(),
            retirement.root_durability(),
            retirement.confirmation(),
            retirement.failures().len()
        )
        .unwrap();
    }
    let stderr = if exit_code == 0 {
        // Own storage was emptied, but the owner's bucket may still keep it.
        outcome
            .remote()
            .and_then(|remote| remote.storage_retention())
            .and_then(storage_retention_sentence)
            .map(|sentence| format!("{sentence}\n"))
            .unwrap_or_default()
    } else if outcome.phase() == solstone_core_offload::teardown::TeardownRunPhase::Prelaunch {
        format!("{PRELAUNCH_HELD}\n")
    } else if outcome.refused_before_effects() {
        let message = if let Some((_, message)) = coverage_refusal(outcome) {
            message
        } else if matches!(
            outcome.refusal(),
            Some(TeardownRunRefusal::UnsupportedDestination)
        ) {
            DESTINATION_UNSUPPORTED
        } else if matches!(outcome.refusal(), Some(TeardownRunRefusal::ToolUnavailable)) {
            TOOL_UNAVAILABLE
        } else {
            NOT_STARTED
        };
        format!("{message}\n")
    } else if nothing_deleted(outcome) {
        format!("{STORAGE_UNREACHED}\n")
    } else {
        "backup teardown did not finish. keep recovery files and review the recorded effects before retrying.\n".into()
    };
    CliRun {
        stdout,
        stderr,
        exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliOutcome, classify_tool_resolution, owned_teardown_options, run_cli};
    use solstone_core_offload::teardown::TeardownRunPhase;

    #[test]
    fn off_yes_retains_whole_unstarted_result_without_root_or_tools() {
        for args in [
            vec!["off".into(), "--yes".into()],
            vec![
                "--verbose".into(),
                "off".into(),
                "--yes".into(),
                "--yes".into(),
                "--".into(),
            ],
        ] {
            assert_eq!(classify_tool_resolution(&args), None);
            let run = run_cli(&args, Path::new("/does-not-exist/teardown-cli"));
            let CliOutcome::Teardown(owned) = &run else {
                panic!("whole teardown outcome required")
            };
            let native = owned.outcome();
            if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
                assert_eq!(native.phase(), TeardownRunPhase::Admission);
                assert!(matches!(
                    native.refusal(),
                    Some(TeardownRunRefusal::Root(_))
                ));
                assert!(!native.remote().unwrap().broker_attempted());
                assert!(run.output().0.contains("reason=journal_unavailable"));
                assert!(run.output().1.contains(NOT_STARTED));
            } else {
                assert_eq!(native.phase(), TeardownRunPhase::Prelaunch);
                assert!(matches!(
                    native.refusal(),
                    Some(TeardownRunRefusal::UnsupportedPlatform)
                ));
                assert!(native.remote().is_none());
                assert!(run.output().1.contains(PRELAUNCH_HELD));
            }
            assert!(native.refused_before_effects());
            assert!(native.journal_root().is_none());
            assert!(native.teardown_lease().is_none());
            assert!(native.publication().is_none());
            assert!(native.retirement().is_none());
            assert!(!native.attempts().clear());
            assert!(!native.attempts().binding());
            assert!(!native.attempts().retirement());
            assert_ne!(run.output().2, 0);
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn off_yes_names_backup_only_media_and_unsupported_destinations() {
        use solstone_core_backup::{HostedBinding, save_hosted_binding};
        use solstone_core_retention::archive_catalog::OffloadFile;
        let journal = tempfile::tempdir().unwrap();
        std::fs::create_dir(journal.path().join("config")).unwrap();
        let config = journal.path().join("config/journal.json");
        std::fs::write(&config, br#"{"backup":{"enabled":true,"mode":"operated"}}"#).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        save_hosted_binding(
            journal.path(),
            &HostedBinding {
                broker_endpoint: format!("http://{}", listener.local_addr().unwrap()),
                account_id: "account".into(),
                instance_id: "instance".into(),
                bucket: "bucket".into(),
                prefix: "owner/".into(),
                broker_token: "fixture-token".into(),
            },
        )
        .unwrap();
        solstone_core_offload::ledger::append_offload_event(
            journal.path(),
            "20260101",
            "_default",
            "010000_001",
            "legacy-snapshot",
            &[OffloadFile {
                name: "clip.webm".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            }],
            1,
        )
        .unwrap();
        let args = vec!["off".into(), "--yes".into()];
        let refuses = |reason: &str, message: &str| {
            let before = std::fs::read(&config).unwrap();
            let run = run_cli(&args, journal.path());
            let (stdout, stderr, code) = run.output();
            assert_eq!(code, 1);
            assert!(stdout.contains(&format!("reason={reason}")), "{stdout}");
            assert_eq!(stderr, format!("{message}\n"));
            assert!(matches!(
                listener.accept(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
            ));
            assert_eq!(std::fs::read(&config).unwrap(), before);
        };
        // The recording this historical media belongs to is not on this device.
        refuses("backup_only_unplaced", BACKUP_ONLY_UNPLACED);
        let segment = journal.path().join("chronicle/20260101/010000_001");
        std::fs::create_dir_all(&segment).unwrap();
        refuses("backup_only_held", BACKUP_ONLY_HELD);
        // Offload was on, but its record is gone.
        std::fs::remove_dir_all(journal.path().join("health/offload")).unwrap();
        std::fs::write(
            &config,
            br#"{"backup":{"enabled":true,"mode":"operated","offload":{"enabled":true}}}"#,
        )
        .unwrap();
        refuses("offload_record_unavailable", OFFLOAD_RECORD_UNAVAILABLE);

        std::fs::write(
            &config,
            br#"{"backup":{"enabled":true,"destination":{"repository":"s3:bucket/prefix","backend":"s3"}}}"#,
        )
        .unwrap();
        let run = run_cli(&args, journal.path());
        let (stdout, stderr, code) = run.output();
        assert_eq!(code, 1);
        assert!(
            stdout.contains("reason=destination_unavailable"),
            "{stdout}"
        );
        assert_eq!(stderr, format!("{DESTINATION_UNSUPPORTED}\n"));
    }

    #[test]
    fn off_help_unconfirmed_and_invalid_options_do_not_select_owned_execution() {
        for args in [
            vec!["off".into()],
            vec!["off".into(), "--help".into()],
            vec!["off".into(), "--wat".into()],
            vec!["off".into(), "--".into(), "--yes".into()],
        ] {
            assert!(!owned_teardown_options(&args));
            assert_eq!(classify_tool_resolution(&args), None);
        }
    }

    #[test]
    fn whole_cli_result_is_send_static() {
        fn assert_closed<T: Send + 'static>() {}
        assert_closed::<OwnedTeardownCliRun>();
        assert_closed::<CliOutcome>();
    }
}
