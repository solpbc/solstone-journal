// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Borrow offload facts without IO, eligibility calculations or owner extraction.

use serde_json::{Value, json};
use solstone_core_backup_runtime::{
    ArchiveFailure, ArchiveRemoteEffects, ArchiveSnapshot, OwnedArchiveAdmissionFailure,
};
use solstone_core_offload::{
    measurement::{ProposedRelief, RawMediaPopulation},
    run::*,
};
use solstone_core_retention::{
    archive_offer::{
        ObservedOffloadOffers, OffloadOfferRefusal, OffloadReconciliationDisposition,
        OffloadReconciliationGroup, OwnedOffloadOffersHold, SettledOffloadOfferReport,
    },
    mark_mutation::BoundMutationOutcome,
    remove_marked::{ArchiveReadState, ReviewedArchiveReadReport},
};

pub(crate) fn admission_failure(failure: &OwnedArchiveAdmissionFailure) -> Value {
    json!({"state":"refused","reason":failure.error().to_string(),
        "broker_requested":failure.effects().broker_requested,
        "write_repository_requested":failure.effects().write_repository_requested,
        "read_repository_requested":failure.effects().read_repository_requested})
}

pub(crate) fn selected_inputs(input: &OwnedOffloadInputs) -> Value {
    json!({"observed":observed(input.observed())})
}
pub(crate) fn selected_input_failure(failure: &OwnedOffloadInputFailure) -> Value {
    json!({"reason":creation_reason(failure.reason()),"observed":failure.observed().map(observed)})
}
pub(crate) fn planned_inputs(input: &OwnedOffloadPlanInputs) -> Value {
    json!({"planning":planning(input.planning())})
}
pub(crate) fn planned_input_failure(failure: &OwnedOffloadPlanInputFailure) -> Value {
    json!({"reason":creation_reason(failure.reason()),"planning":planning(failure.planning())})
}

fn selection(value: &solstone_core_retention::archive_offer::OffloadOfferSelection) -> Value {
    json!({"target":value.target,"layout":value.layout,"names":value.names})
}

fn observed(offers: &ObservedOffloadOffers) -> Value {
    json!({"groups":offers.groups().iter().map(|group| json!({
        "selection":selection(group.selection()), "expected_mark":group.expected_mark(),
        "files":group.files().iter().map(|file| json!({"name":file.name(),"observation":{"identity":file.observation().identity,"digest":file.observation().digest},"processing":file.processing()})).collect::<Vec<_>>()
    })).collect::<Vec<_>>()})
}

fn archives(results: &[Result<ArchiveSnapshot, ArchiveFailure>]) -> Value {
    json!(results.iter().map(|result| match result {
        Ok(snapshot) => json!({"repository_id":snapshot.repository_id,"snapshot_id":snapshot.snapshot_id}),
        Err(failure) => json!({"reason":failure.error.to_string(),"diagnostic":failure.diagnostic,"native_start":match failure.error {
            solstone_core_backup_runtime::ArchiveError::NativeStart { status, stage: Some(stage) } => Some(json!({"status":status,"refusal_stage":stage.as_str()})),
            _ => None,
        },"remote_effects":match &failure.remote_effects {
            ArchiveRemoteEffects::NotAttempted => json!({"state":"not_attempted"}),
            ArchiveRemoteEffects::Attempted { returncode, observed_snapshot_id } => json!({"state":"attempted","returncode":returncode,"observed_snapshot_id":observed_snapshot_id}),
        }}),
    }).collect::<Vec<_>>())
}

fn ledger<'a>(writes: impl Iterator<Item = OffloadLedgerWrite<'a>>) -> Value {
    let sync = |value| match value {
        OffloadLedgerSync::Confirmed => "confirmed",
        OffloadLedgerSync::Uncertain => "uncertain",
    };
    json!(writes.map(|write| match write {
        OffloadLedgerWrite::Recorded { effect, durability, confirmation, failures } => {
            let (effect, bytes) = match effect {
                OffloadLedgerEffect::AlreadyRecorded => ("already_recorded", None),
                OffloadLedgerEffect::Appended { bytes } => ("appended", Some(bytes)),
                OffloadLedgerEffect::Attempted { written } => ("attempted", written),
            };
            json!({"effect":effect,"bytes":bytes,"durability":durability.map(sync),"confirmation":sync(confirmation),"failures":failures.iter().map(|_| json!({"detail_available":true})).collect::<Vec<_>>()})
        }
        OffloadLedgerWrite::Refused { reason, detail } => json!({"effect":"refused","reason":reason,"detail_available":detail.is_some()}),
    }).collect::<Vec<_>>())
}

fn publication(outcome: &BoundMutationOutcome) -> Value {
    match outcome {
        BoundMutationOutcome::Unchanged(result) => {
            json!({"effect":"unchanged","status":result.status})
        }
        BoundMutationOutcome::Published(result) => {
            json!({"effect":"published","status":result.status})
        }
        BoundMutationOutcome::PublishedUncertain {
            candidate,
            uncertainty,
        } => json!({
            "effect":"published_uncertain","status":candidate.status,
            "durability_error":uncertainty.durability.as_ref().map(|error| error.kind().to_string()),
            "observation_error":uncertainty.observation.as_ref().map(|error| error.kind().to_string()),
            "binding_uncertain":!uncertainty.bindings.is_empty(),"lock_uncertain":uncertainty.lock.is_some(),
        }),
    }
}
fn offer_publication(result: &Result<BoundMutationOutcome, OffloadOfferRefusal>) -> Value {
    match result {
        Ok(outcome) => publication(outcome),
        Err(error) => json!({"effect":"refused","reason":offer_reason(error)}),
    }
}
fn reads<'a>(reads: impl Iterator<Item = ArchiveReadState<'a>>) -> Value {
    json!(reads.map(|read| match read {
        ArchiveReadState::Stage(stage) => json!({"state":"stage","retained_stage":stage.is_some()}),
        ArchiveReadState::Target(_) => json!({"state":"target"}),
        ArchiveReadState::Prepared { file, target_identity, media, cleanup } => json!({"state":"prepared","name":file.name,"bytes":file.bytes,"repository_id":file.repository_id,"snapshot_id":file.snapshot_id,"archive_path":file.archive_path,"identity":target_identity,"digest":media.digest(),"cleanup":cleanup}),
        ArchiveReadState::Readback { file, target_identity, failure, cleanup } => json!({"state":"readback","name":file.name,"bytes":file.bytes,"repository_id":file.repository_id,"snapshot_id":file.snapshot_id,"archive_path":file.archive_path,"identity":target_identity,"reason":failure.reason().to_string(),"cleanup":cleanup}),
    }).collect::<Vec<_>>())
}
fn settled_reads(reads: &[ReviewedArchiveReadReport]) -> Value {
    json!(reads.iter().map(|read| json!({"name":read.file().name,"bytes":read.file().bytes,"repository_id":read.file().repository_id,"snapshot_id":read.file().snapshot_id,"archive_path":read.file().archive_path,"identity":read.target_identity(),"digest":read.digest(),"cleanup":read.cleanup()})).collect::<Vec<_>>())
}
fn offers_hold(offers: &OwnedOffloadOffersHold) -> Value {
    json!({"publication":offers.publication().map(offer_publication),"marks":offers.marks(),
        "reason":offers.reason().map(offer_reason),"archive_reads":reads(offers.archive_reads()),
        "readback_error":offers.readback_error().map(|error| error.to_string()),"cleanup":offers.cleanup(),
        "native_error":offers.native_error().map(|error| error.kind().to_string())})
}
fn offers_settled(offers: &SettledOffloadOfferReport) -> Value {
    json!({"publication":offer_publication(offers.publication()),"marks":offers.marks(),"archive_reads":settled_reads(offers.archive_reads())})
}

pub(crate) fn creation(actor: &OwnedOffloadCreation) -> Value {
    let offers = match actor.owner() {
        OwnedOffloadCreationOwner::Published(outcome) => {
            json!({"publication":offer_publication(outcome.publication()),"marks":outcome.verified().marks(),"archive_reads":reads(outcome.verified().archive_reads())})
        }
        OwnedOffloadCreationOwner::Verified(verified) => {
            json!({"marks":verified.marks(),"archive_reads":reads(verified.archive_reads())})
        }
        OwnedOffloadCreationOwner::VerificationFailure(failure) => {
            json!({"reason":offer_reason(failure.reason()),"archive_reads":reads(failure.archive_reads())})
        }
        OwnedOffloadCreationOwner::AdmissionFailure(failure) => {
            json!({"admission":admission_failure(failure)})
        }
        OwnedOffloadCreationOwner::Admitted(_) | OwnedOffloadCreationOwner::Read(_) => Value::Null,
    };
    json!({"reason":actor.reason().map(creation_reason),"observed":actor.observed().map(observed),
        "source_copies":actor.source_copies().map(|copy| json!({"digest":copy.digest(),"identity":copy.identity()})).collect::<Vec<_>>(),
        "partial_copy":actor.partial_copy().map(partial),"native_error":actor.native_error().map(|error| error.kind().to_string()),
        "archives":archives(actor.archives()),"ledger_writes":ledger(actor.ledger_writes()),"offers":offers})
}
pub(crate) fn creation_hold(hold: &OwnedOffloadCreationHold) -> Value {
    json!({"reason":hold.reason().map(creation_reason),"finish_refusal":hold.finish_refusal().map(creation_finish),
        "observed":hold.observed().map(observed),"source_copies":hold.source_copies().map(|copy| json!({"digest":copy.digest(),"identity":copy.identity()})).collect::<Vec<_>>(),
        "partial_copy":hold.partial_copy().map(partial),"native_error":hold.native_error().map(|error| error.kind().to_string()),
        "archives":archives(hold.archives()),"ledger_writes":ledger(hold.ledger_writes()),
        "offers":hold.offers().map(offers_hold).or_else(|| hold.settled_offers().map(offers_settled)),
        "admission":hold.admission_failure().map(admission_failure),
        "source_cleanups":hold.source_cleanups().iter().map(|cleanup| json!({"index":cleanup.index,"cleanup":cleanup.cleanup})).collect::<Vec<_>>()})
}
pub(crate) fn creation_settled(report: &SettledOffloadCreationReport) -> Value {
    json!({"offers":offers_settled(report.offers()),"archives":archives(report.archives()),"ledger_writes":ledger(report.ledger_writes()),
        "source_copies":report.source_copies().iter().map(|copy| json!({"digest":copy.digest,"identity":copy.identity,"cleanup":copy.cleanup})).collect::<Vec<_>>()})
}

pub(crate) fn plan(actor: &OwnedOffloadPlan) -> Value {
    let creation = match actor.owner() {
        OwnedOffloadPlanOwner::Creation(child) => Some(creation(child)),
        _ => None,
    };
    let admission = match actor.owner() {
        OwnedOffloadPlanOwner::AdmissionFailure(failure) => Some(admission_failure(failure)),
        _ => None,
    };
    json!({"planning":planning(actor.planning()),"reason":actor.reason().map(creation_reason),"capture_failure":actor.capture_failure().map(selected_input_failure),"creation":creation,"admission":admission})
}
pub(crate) fn plan_hold(hold: &OwnedOffloadPlanHold) -> Value {
    json!({"planning":planning(hold.planning()),"reason":hold.reason().map(creation_reason),"finish_refusal":hold.finish_refusal().map(plan_finish),
        "capture_failure":hold.capture_failure().map(selected_input_failure),"creation":hold.creation().map(creation_hold).or_else(|| hold.settled_creation().map(creation_settled)),
        "admission":hold.admission_failure().map(admission_failure),"metadata_settled":hold.metadata_settled(),
        "readback_error":hold.settlement_failure().map(|failure| failure.reason().to_string())})
}
pub(crate) fn plan_settled(report: &SettledOffloadPlanReport) -> Value {
    json!({"planning":planning_report(report.planning()),"creation":report.creation().map(creation_settled)})
}

fn groups(groups: &[OffloadReconciliationGroup]) -> Value {
    json!(groups.iter().map(|group| json!({"selection":group.selection().map(selection),"expected":group.expected(),"disposition":match group.disposition() {
        OffloadReconciliationDisposition::Unchanged => json!({"state":"unchanged"}),
        OffloadReconciliationDisposition::Busy => json!({"state":"busy"}),
        OffloadReconciliationDisposition::Refused(error) => json!({"state":"refused","reason":offer_reason(error)}),
        OffloadReconciliationDisposition::WriteRefused(_) => json!({"state":"write_refused","detail_available":true}),
        OffloadReconciliationDisposition::Publication { outcome, observation } => json!({"state":"publication","publication":publication(outcome),"observation_refusal":observation.as_deref().map(offer_reason)}),
    }})).collect::<Vec<_>>())
}
fn planning(value: &OffloadPlanning) -> Value {
    let reconciliation = value.reconciliation().map(|result| match result {
        Ok(observed) => json!({"state":"observed","root_observation":observed.journal_root().identity().observation_id(),"register":observed.register(),"groups":groups(observed.groups())}),
        Err(failure) => json!({"state":"refused","root_observation":failure.journal_root().identity().observation_id(),"register":failure.last_confirmed_register(),"groups":groups(failure.groups()),"reason":match failure.reason() { solstone_core_retention::archive_offer::OffloadReconciliationStop::Observation(error) => offer_reason(error), solstone_core_retention::archive_offer::OffloadReconciliationStop::Publication => json!({"kind":"publication"}) }}),
    });
    planning_fields(
        value.phase(),
        reconciliation,
        value.budget_bytes(),
        value.local(),
        value.decision().map(|decision| {
            decision_fields(
                decision.root_observation(),
                decision.availability(),
                decision.eligible(),
                decision.refusals(),
            )
        }),
        value.initial_projection(),
    )
}
fn planning_report(value: &OffloadPlanningReport) -> Value {
    let reconciliation = value.reconciliation();
    planning_fields(
        value.phase(),
        Some(
            json!({"state":"observed","root_observation":reconciliation.root_observation(),"register":reconciliation.register(),"groups":groups(reconciliation.groups())}),
        ),
        value.budget_bytes(),
        value.local(),
        value.decision().map(|decision| {
            decision_fields(
                decision.root_observation().to_owned(),
                decision.availability(),
                decision.eligible(),
                decision.refusals(),
            )
        }),
        value.initial_projection(),
    )
}
fn planning_fields(
    phase: OffloadPlanningPhase,
    reconciliation: Option<Value>,
    budget: Option<u64>,
    local: Option<&RawMediaPopulation>,
    decision: Option<Value>,
    initial_projection: Option<&ProposedRelief>,
) -> Value {
    json!({"phase":match phase { OffloadPlanningPhase::Held => "held", OffloadPlanningPhase::AlreadySatisfied => "already_satisfied", OffloadPlanningPhase::Exhausted => "exhausted", OffloadPlanningPhase::OffersPublished => "offers_published" },
        "reconciliation":reconciliation,"budget_bytes":budget,
        "local":local.map(|local| json!({"complete":local.complete(),"known":local.known_usage().ok(),"total":local.usage().ok()})),
        "decision":decision,"initial_projection":initial_projection.map(|p| json!({"local":p.local,"eligible_bytes":p.eligible_bytes,"eligible_files":p.eligible_files,"projected_local_bytes":p.projected_local_bytes}))})
}
fn decision_fields<'a>(
    root: String,
    availability: CurrentOffloadAvailability,
    eligible: impl Iterator<Item = &'a EligibleOffloadFile>,
    refusals: &[CurrentOffloadRefusal],
) -> Value {
    json!({"root_observation":root,"availability":match availability { CurrentOffloadAvailability::Available => "available", CurrentOffloadAvailability::Unavailable => "unavailable" },
        "eligible":eligible.map(|file| json!({"target":file.target(),"layout":file.layout(),"name":file.name(),"bytes":file.bytes(),"revision":file.revision(),"class":file.class(),"witness_id":file.witness_id(),"mark":file.mark()})).collect::<Vec<_>>(),
        "refusals":refusals.iter().map(|refusal| json!({"mark_id":refusal.mark_id(),"reason":current_reason(refusal.reason())})).collect::<Vec<_>>()})
}

fn partial(value: OffloadCreationNative<'_>) -> Value {
    match value {
        OffloadCreationNative::Stage(stage) => {
            json!({"state":"stage","retained_stage":stage.is_some()})
        }
        OffloadCreationNative::Target(_) => json!({"state":"target"}),
        OffloadCreationNative::Copy(_) => json!({"state":"copy"}),
    }
}
fn creation_reason(reason: &OffloadCreationRefusal) -> Value {
    match reason {
        OffloadCreationRefusal::Inputs(error) => {
            json!({"kind":"inputs","reason":offer_reason(error)})
        }
        OffloadCreationRefusal::Archive(error) => {
            json!({"kind":"archive","reason":error.to_string()})
        }
        OffloadCreationRefusal::Readback(error) => readback_reason(error),
        OffloadCreationRefusal::Native => json!({"kind":"native"}),
        OffloadCreationRefusal::Metadata => json!({"kind":"metadata"}),
        OffloadCreationRefusal::Entropy => json!({"kind":"entropy"}),
        OffloadCreationRefusal::Ledger => json!({"kind":"ledger"}),
        OffloadCreationRefusal::Verification => json!({"kind":"verification"}),
        OffloadCreationRefusal::Publication => json!({"kind":"publication"}),
        OffloadCreationRefusal::Planning(reason) => {
            json!({"kind":"planning","reason":match reason { OffloadPlanningRefusal::Disabled => json!({"kind":"disabled"}), OffloadPlanningRefusal::Reconciliation => json!({"kind":"reconciliation"}), OffloadPlanningRefusal::Local(_) => json!({"kind":"local","detail_available":true}), OffloadPlanningRefusal::Current(error) => current_reason(error), OffloadPlanningRefusal::Unavailable => json!({"kind":"unavailable"}) }})
        }
    }
}
fn readback_reason(error: &solstone_core_backup::archive_readback::ReadbackError) -> Value {
    let mut value = json!({"kind":"readback","reason":error.to_string()});
    if let Some(diagnostic) = error.discovery_diagnostic() {
        value["discovery"] = json!(diagnostic);
    }
    value
}
fn offer_reason(reason: &OffloadOfferRefusal) -> Value {
    use OffloadOfferRefusal as R;
    let kind = match reason {
        R::Unsupported => "unsupported",
        R::InvalidSelection => "invalid_selection",
        R::Changed => "changed",
        R::Kept(_) => "kept",
        R::Unproven => "unproven",
        R::Review(_) => "review",
        R::Policy(_) => "policy",
        R::Proof(_) => "proof",
        R::Approval => "approval",
        R::Metadata => "metadata",
        R::Catalog(_) => "catalog",
        R::Register(_) => "register",
        R::Admission(error) => return json!({"kind":"admission","reason":error.to_string()}),
        R::Io(error) => return json!({"kind":"io","error":error.kind().to_string()}),
        R::Readback(error) => return readback_reason(error),
        R::Cleanup(cleanup) => return json!({"kind":"cleanup","cleanup":cleanup}),
        R::InsufficientSpace {
            available,
            required,
        } => return json!({"kind":"insufficient_space","available":available,"required":required}),
    };
    json!({"kind":kind})
}
fn current_reason(reason: &CurrentOffloadRefusalReason) -> Value {
    use CurrentOffloadRefusalReason as R;
    let kind = match reason {
        R::Busy => "busy",
        R::Stale => "stale",
        R::Failed => "failed",
        R::Missing => "missing",
        R::Unbound => "unbound",
        R::WrongClass => "wrong_class",
        R::Incomplete => "incomplete",
        R::AdmissionChanged => "admission_changed",
        R::ExecutionUnavailable => "execution_unavailable",
        R::Kept { names } => return json!({"kind":"kept","names":names}),
        R::Unproven { names } => return json!({"kind":"unproven","names":names}),
        R::Prerequisite { reason } => return json!({"kind":"prerequisite","reason":reason}),
    };
    json!({"kind":kind})
}
fn creation_finish(reason: &OwnedOffloadCreationFinishRefusal) -> Value {
    use OwnedOffloadCreationFinishRefusal as R;
    let kind = match reason {
        R::IncompleteCreation => "incomplete_creation",
        R::OfferSettlement => "offer_settlement",
        R::Census(_) => "census",
        R::Obligations(_) => "obligations",
        R::Readback(error) => return readback_reason(error),
        R::Changed(error) => return json!({"kind":"changed","error":error.kind().to_string()}),
        R::Cleanup(cleanup) => return json!({"kind":"cleanup","cleanup":cleanup}),
    };
    json!({"kind":kind})
}
fn plan_finish(reason: &OwnedOffloadPlanFinishRefusal) -> Value {
    use OwnedOffloadPlanFinishRefusal as R;
    let kind = match reason {
        R::IncompletePlan => "incomplete_plan",
        R::Creation => "creation",
        R::Census(_) => "census",
        R::Obligations(_) => "obligations",
        R::Readback(error) => return readback_reason(error),
    };
    json!({"kind":kind})
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;
