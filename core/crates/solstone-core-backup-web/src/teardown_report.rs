// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Borrow actual teardown observations without serializing native owners or secrets.

use serde_json::{Value, json};
use solstone_core_backup::archive_admission::AdmissionError;
use solstone_core_backup::teardown_configuration::TeardownConfigurationRefusal;
use solstone_core_backup_runtime::teardown::owned::{
    RemoteTeardownPhase, TeardownProtocolRefusal, TeardownRemoteRefusal,
};
use solstone_core_backup_runtime::teardown_admission::{
    TeardownAdmissionRefusal, TeardownCarrierRefusal, TeardownClearRefusal, TeardownReaderRefusal,
};
use solstone_core_journal_config_write::{
    BoundConfigConfirmationRefusal, BoundConfigRefusal, DirectoryFlush,
};
use solstone_core_journal_io::media::ActivityCarrierRetirementEffect;
use solstone_core_offload::teardown::{OwnedTeardownOutcome, TeardownRunPhase, TeardownRunRefusal};

pub(crate) fn outcome(owner: &OwnedTeardownOutcome) -> Value {
    #[cfg(any(unix, windows))]
    let binding = owner.binding().map(binding_facts);
    #[cfg(not(any(unix, windows)))]
    let binding: Option<Value> = None;
    #[cfg(any(unix, windows))]
    let binding_failure = owner
        .binding_failure()
        .map(|failure| binding_refusal(failure.refusal()));
    #[cfg(not(any(unix, windows)))]
    let binding_failure: Option<Value> = None;
    json!({
        "state":match owner { OwnedTeardownOutcome::Finished(_) => "finished", OwnedTeardownOutcome::Held(_) => "held", OwnedTeardownOutcome::Unknown(_) => "unknown" },
        "phase":phase(owner.phase()),
        "refusal":owner.refusal().map(run_refusal),
        "remote":owner.remote().map(|remote| json!({
            "phase":match remote.phase() {
                RemoteTeardownPhase::NotStarted => "not_started", RemoteTeardownPhase::Broker => "broker",
                RemoteTeardownPhase::Inventory => "inventory", RemoteTeardownPhase::Deleting => "deleting",
                RemoteTeardownPhase::Aborting => "aborting", RemoteTeardownPhase::Confirming => "confirming",
                RemoteTeardownPhase::Wiped => "wiped", RemoteTeardownPhase::BindingSuperseded => "binding_superseded",
                RemoteTeardownPhase::SnapshotsPruned => "snapshots_pruned",
            },
            "broker_attempted":remote.broker_attempted(),
            "delete_attempts":remote.delete_attempts(), "delete_confirmations":remote.delete_confirmations(),
            "abort_attempts":remote.abort_attempts(), "abort_confirmations":remote.abort_confirmations(),
            "objects_empty":remote.objects_empty(), "uploads_empty":remote.uploads_empty(),
            "storage_retention":remote.current_storage_retention().map(|retention| format!("{retention:?}")),
            "admitted_snapshot_count":remote.admitted_snapshot_count(),
            "previous_confirmed":remote.previous(),
        })),
        "attempts":{"clear":owner.attempts().clear(), "binding":owner.attempts().binding(), "retirement":owner.attempts().retirement()},
        "publication":owner.publication().map(|receipt| json!({
            "effect":"published",
            "directory_flush":match receipt.directory_flush {
                DirectoryFlush::Synced => "synced", DirectoryFlush::Failed => "failed", DirectoryFlush::Unavailable => "unavailable",
            },
            "durability_uncertainty":receipt.durability_uncertainty.as_ref().map(io_kind),
            "observation_uncertainty":receipt.observation_uncertainty.as_ref().map(io_kind),
            "confirmation_refusal":receipt.confirmation_refusal.as_ref().map(|failure| match failure {
                BoundConfigConfirmationRefusal::Admission(_) => json!({"kind":"admission"}),
                BoundConfigConfirmationRefusal::Read(error) => json!({"kind":"read", "error":io_kind(error)}),
                BoundConfigConfirmationRefusal::Different => json!({"kind":"different"}),
            }),
        })),
        "binding":binding,
        "retirement":owner.retirement().map(|receipt| json!({
            "effect":match receipt.effect() { ActivityCarrierRetirementEffect::NotAttempted => "not_attempted", ActivityCarrierRetirementEffect::Attempted => "attempted", ActivityCarrierRetirementEffect::Removed => "removed" },
            "root_durability":receipt.root_durability(), "confirmation":receipt.confirmation(),
            "failures":receipt.failures().iter().map(io_kind).collect::<Vec<_>>(),
        })),
        "preparation_failure":owner.preparation_failure().map(|failure| json!({
            "refusal":match failure.refusal() { TeardownCarrierRefusal::Admission(reason) => admission(reason), TeardownCarrierRefusal::Preparation => json!({"kind":"preparation"}) },
            "receipt":failure.preparation().map(|receipt| json!({"stage_durability":receipt.stage_durability(), "root_durability":receipt.root_durability(), "confirmation":receipt.confirmation()})),
            "error":failure.preparation_error().map(io_kind),
        })),
        "clear_failure":owner.clear_failure().map(|failure| json!({
            "attempted":failure.attempted(),
            "refusal":match failure.refusal() {
                TeardownClearRefusal::Admission(reason) => admission(*reason),
                TeardownClearRefusal::Clear(reason) => config_refusal(reason),
                TeardownClearRefusal::Unchanged => json!({"kind":"unchanged"}),
                TeardownClearRefusal::PublicationUncertain => json!({"kind":"publication_uncertain"}),
            },
        })),
        "binding_failure":binding_failure,
    })
}

fn phase(value: TeardownRunPhase) -> &'static str {
    match value {
        TeardownRunPhase::Prelaunch => "prelaunch",
        TeardownRunPhase::Admission => "admission",
        TeardownRunPhase::Preparation => "preparation",
        TeardownRunPhase::Remote => "remote",
        TeardownRunPhase::Clear => "clear",
        TeardownRunPhase::Binding => "binding",
        TeardownRunPhase::FinishAdmission => "finish_admission",
        TeardownRunPhase::CarrierTransfer => "carrier_transfer",
        TeardownRunPhase::Retirement => "retirement",
        TeardownRunPhase::FinalChecks => "final_checks",
        TeardownRunPhase::Finished => "finished",
    }
}

fn run_refusal(value: &TeardownRunRefusal) -> Value {
    match value {
        TeardownRunRefusal::UnsupportedPlatform => json!({"kind":"unsupported_platform"}),
        TeardownRunRefusal::UnsupportedDestination => json!({"kind":"unsupported_destination"}),
        TeardownRunRefusal::ToolUnavailable => json!({"kind":"tool_unavailable"}),
        TeardownRunRefusal::Root(_) => json!({"kind":"root"}),
        TeardownRunRefusal::Budget(reason) => json!({"kind":"budget", "reason":budget(*reason)}),
        TeardownRunRefusal::Admission(reason) => admission(*reason),
        TeardownRunRefusal::CarrierPreparation => json!({"kind":"carrier_preparation"}),
        TeardownRunRefusal::Remote(reason) => {
            json!({"kind":"remote", "reason":remote_refusal(reason)})
        }
        TeardownRunRefusal::Clear => json!({"kind":"clear"}),
        TeardownRunRefusal::Binding => json!({"kind":"binding"}),
        #[cfg(any(unix, windows))]
        TeardownRunRefusal::Finish(reason) => {
            json!({"kind":"finish", "reason":binding_refusal(reason)})
        }
        TeardownRunRefusal::CarrierShared => json!({"kind":"carrier_shared"}),
        TeardownRunRefusal::RetirementUncertain => json!({"kind":"retirement_uncertain"}),
    }
}

fn budget(value: AdmissionError) -> &'static str {
    value.code()
}

fn configuration(value: TeardownConfigurationRefusal) -> &'static str {
    match value {
        TeardownConfigurationRefusal::Unsupported => "unsupported",
        TeardownConfigurationRefusal::RootChanged => "root_changed",
        TeardownConfigurationRefusal::ConfigurationUnavailable => "configuration_unavailable",
        TeardownConfigurationRefusal::ConfigurationChanged => "configuration_changed",
        TeardownConfigurationRefusal::KeysUnavailable => "keys_unavailable",
        TeardownConfigurationRefusal::BindingUnavailable => "binding_unavailable",
        TeardownConfigurationRefusal::BindingChanged => "binding_changed",
    }
}

fn admission(value: TeardownAdmissionRefusal) -> Value {
    match value {
        TeardownAdmissionRefusal::Unsupported => json!({"kind":"unsupported"}),
        TeardownAdmissionRefusal::ActivityBusy => json!({"kind":"activity_busy"}),
        TeardownAdmissionRefusal::ActivityUnavailable => json!({"kind":"activity_unavailable"}),
        TeardownAdmissionRefusal::CarrierUnavailable => json!({"kind":"carrier_unavailable"}),
        TeardownAdmissionRefusal::Configuration(reason) => {
            json!({"kind":"configuration", "reason":configuration(reason)})
        }
        TeardownAdmissionRefusal::Reader(reason) => json!({"kind":"reader", "reason":match reason {
            TeardownReaderRefusal::CatalogUnavailable => "catalog_unavailable", TeardownReaderRefusal::RecoveryUnavailable => "recovery_unavailable", TeardownReaderRefusal::RecoveryHeld => "recovery_held",
            TeardownReaderRefusal::BackupOnlyHeld => "backup_only_held",
            TeardownReaderRefusal::BackupOnlyUnplaced => "backup_only_unplaced",
        }}),
    }
}

fn remote_refusal(value: &TeardownRemoteRefusal) -> Value {
    match value {
        TeardownRemoteRefusal::Unsupported => json!({"kind":"unsupported"}),
        TeardownRemoteRefusal::Operation(reason) => {
            json!({"kind":"operation", "reason":budget(*reason)})
        }
        TeardownRemoteRefusal::Admission(reason) => admission(*reason),
        TeardownRemoteRefusal::BindingUnavailable => json!({"kind":"binding_unavailable"}),
        TeardownRemoteRefusal::Timestamp => json!({"kind":"timestamp"}),
        TeardownRemoteRefusal::Broker(_) => json!({"kind":"broker"}),
        TeardownRemoteRefusal::Http(reason) => json!({"kind":"http", "reason":match reason {
            solstone_core_backup_runtime::hosted_runtime::HttpError::Timeout => "timeout",
            solstone_core_backup_runtime::hosted_runtime::HttpError::Unreachable => "unreachable",
            solstone_core_backup_runtime::hosted_runtime::HttpError::Other => "other",
        }}),
        TeardownRemoteRefusal::HttpStatus(status) => json!({"kind":"http_status", "status":status}),
        TeardownRemoteRefusal::DestinationUnavailable => json!({"kind":"destination_unavailable"}),
        TeardownRemoteRefusal::Restic(reason) => json!({"kind":"restic", "reason":reason}),
        TeardownRemoteRefusal::Protocol(reason) => {
            json!({"kind":"protocol", "reason":match reason {
                TeardownProtocolRefusal::Malformed => "malformed", TeardownProtocolRefusal::Limit => "limit", TeardownProtocolRefusal::Namespace => "namespace", TeardownProtocolRefusal::Pagination => "pagination", TeardownProtocolRefusal::Acknowledgment => "acknowledgment",
            }})
        }
        TeardownRemoteRefusal::Residual => json!({"kind":"residual"}),
        TeardownRemoteRefusal::AlreadyAttempted => json!({"kind":"already_attempted"}),
    }
}

fn config_refusal(value: &BoundConfigRefusal) -> Value {
    json!({"kind":match value {
        BoundConfigRefusal::Unsupported => "unsupported", BoundConfigRefusal::Admission(_) => "admission", BoundConfigRefusal::Load(_) => "load",
        BoundConfigRefusal::Missing => "missing", BoundConfigRefusal::Conflict => "conflict", BoundConfigRefusal::Lock(_) => "lock",
        BoundConfigRefusal::Serialize(_) => "serialize", BoundConfigRefusal::TooLarge => "too_large", BoundConfigRefusal::Write(_) => "write",
    }})
}

fn io_kind(error: &std::io::Error) -> String {
    error.kind().to_string()
}

#[cfg(any(unix, windows))]
fn binding_refusal(
    value: &solstone_core_backup_runtime::teardown_admission::TeardownBindingRefusal,
) -> Value {
    use solstone_core_backup_runtime::teardown_admission::TeardownBindingRefusal as Refusal;
    match value {
        Refusal::Admission(reason) => admission(*reason),
        Refusal::Binding(reason) => binding_finalization(reason),
        Refusal::Removal => json!({"kind":"removal"}),
    }
}

#[cfg(any(unix, windows))]
fn binding_finalization(
    value: &solstone_core_backup::teardown_configuration::BindingFinalizationRefusal,
) -> Value {
    use solstone_core_backup::teardown_configuration::BindingFinalizationRefusal as Refusal;
    match value {
        Refusal::Configuration(reason) => {
            json!({"kind":"configuration", "reason":configuration(*reason)})
        }
        Refusal::PublicationUncertain => json!({"kind":"publication_uncertain"}),
        Refusal::MissingOriginalBinding => json!({"kind":"missing_original_binding"}),
        Refusal::ParentChanged => json!({"kind":"parent_changed"}),
        Refusal::BindingChanged => json!({"kind":"binding_changed"}),
        Refusal::RemovalUnconfirmed => json!({"kind":"removal_unconfirmed"}),
        Refusal::Native(error) => json!({"kind":"native", "error":io_kind(error)}),
        Refusal::Flat(_) => json!({"kind":"flat"}),
        Refusal::Lock(_) => json!({"kind":"lock"}),
    }
}

#[cfg(any(unix, windows))]
fn binding_facts(
    receipt: &solstone_core_backup::teardown_configuration::BindingRemovalReceipt,
) -> Value {
    use solstone_core_journal_io::{
        ClaimDurability, ClaimRemovalError as Error, ClaimRemovalOutcome as Outcome,
        ClaimUnchangedReason, IdentityChangeDisposition,
    };
    json!({
        "attempted":receipt.attempted(), "complete":receipt.complete(),
        "confirmation":match receipt.confirmation() { Ok(()) => json!({"state":"confirmed"}), Err(reason) => binding_finalization(reason) },
        "native":receipt.native().map(|result| match result {
            Ok(outcome) => match outcome {
                Outcome::Removed => json!({"effect":"removed", "durability":"synced"}),
                Outcome::RemovedDurabilityUncertain => json!({"effect":"removed", "durability":"uncertain"}),
                Outcome::RemovedUnflushed => json!({"effect":"removed", "durability":"unflushed"}),
                Outcome::Unchanged {reason} => json!({"effect":"unchanged", "reason":match reason {
                    ClaimUnchangedReason::ClaimNameOccupied => "claim_name_occupied", ClaimUnchangedReason::UnsupportedNoReplace {..} => "unsupported_no_replace", ClaimUnchangedReason::RenameNotAppliedAfterReconciliation => "rename_not_applied",
                }}),
                Outcome::IdentityChanged {disposition, durability} => json!({"effect":"identity_changed", "disposition":match disposition {
                    IdentityChangeDisposition::Restored => "restored", IdentityChangeDisposition::RetainedClaim {..} => "retained_claim", IdentityChangeDisposition::UnknownLocation => "unknown_location",
                }, "durability":match durability { ClaimDurability::NotEstablished => "not_established", ClaimDurability::Synced => "synced", ClaimDurability::Uncertain => "uncertain" }}),
            },
            Err(error) => json!({"error":match error {
                Error::InvalidOriginalName {..} => "invalid_original_name", Error::ObservationNameMismatch {..} => "observation_name_mismatch", Error::AliasedClaimName {..} => "aliased_claim_name",
                Error::PostClaimInspection {..} => "post_claim_inspection", Error::Preflight {..} => "preflight", Error::Reconciliation {..} => "reconciliation", Error::ReconciliationInconclusive {..} => "reconciliation_inconclusive",
                Error::RestoreFailure {..} => "restore_failure", Error::UnlinkFailure {..} => "unlink_failure", Error::Io {..} => "io",
            }}),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_codec_redacts_error_payloads() {
        let secret = "secret-token/private/journal/key";
        let refusal = BoundConfigRefusal::Admission(Box::new(std::io::Error::other(secret)));
        assert_eq!(config_refusal(&refusal), json!({"kind":"admission"}));
        assert!(!config_refusal(&refusal).to_string().contains(secret));
        #[cfg(any(unix, windows))]
        assert_eq!(
            binding_finalization(
                &solstone_core_backup::teardown_configuration::BindingFinalizationRefusal::Native(
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, secret)
                )
            ),
            json!({"kind":"native", "error":std::io::ErrorKind::PermissionDenied.to_string()})
        );
    }
}
