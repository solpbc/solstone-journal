// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Borrow observed removal facts while their executor retains all resources.
//! Serialization performs no IO and grants no completion or cleanup authority.

use std::io;

use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use solstone_core_journal_io::activity_obligations::{
    RecoveryObservationError, RecoveryObservationFailure,
};
use solstone_core_journal_io::media::{
    ActivityCarrierPreparationReceipt, ActivityCarrierRetirementEffect,
    ActivityCarrierRetirementReceipt, RemovalPhase, RemovalReceipt,
};
use solstone_core_journal_io::{ExistingParentLockError, FlatDirectoryError};
use solstone_core_retention::mark_mutation::{BoundMutationOutcome, BoundMutationRefusal};
use solstone_core_retention::remove_marked::{
    ArchiveReadState, OwnedReviewedArchiveRemoval, OwnedReviewedArchiveRun, OwnedReviewedRemoval,
    ReviewedArchiveFinishRefusal, ReviewedArchiveRemoval, ReviewedArchiveReport,
    ReviewedFileRemoval, ReviewedRemoval, ReviewedRemovalFileReport, ReviewedRemovalFinishRefusal,
    ReviewedRemovalRefusal, ReviewedRemovalReport,
};
use solstone_core_retention::review_context::ReviewRefusal;

/// Borrow the ordinary result, including its carrier and partial effects.
///
/// A borrowing helper compiles without constructing an executor:
/// ```
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::ordinary;
/// use solstone_core_retention::remove_marked::ReviewedRemoval;
/// fn view(owner: &ReviewedRemoval) -> impl Serialize + '_ { ordinary(owner) }
/// ```
/// The same view cannot escape a local owner:
/// ```compile_fail,E0515
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::ordinary;
/// use solstone_core_retention::remove_marked::ReviewedRemoval;
/// fn escape<'a>(owner: ReviewedRemoval) -> impl Serialize + 'a { ordinary(&owner) }
/// ```
pub fn ordinary(owner: &ReviewedRemoval) -> impl Serialize + '_ {
    OrdinaryView(owner)
}

/// Borrow a local hold and its independent finish and retirement observations.
///
/// ```
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::held_ordinary;
/// use solstone_core_retention::remove_marked::OwnedReviewedRemoval;
/// fn view(owner: &OwnedReviewedRemoval) -> impl Serialize + '_ { held_ordinary(owner) }
/// ```
/// ```compile_fail,E0515
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::held_ordinary;
/// use solstone_core_retention::remove_marked::OwnedReviewedRemoval;
/// fn escape<'a>(owner: OwnedReviewedRemoval) -> impl Serialize + 'a { held_ordinary(&owner) }
/// ```
pub fn held_ordinary(owner: &OwnedReviewedRemoval) -> impl Serialize + '_ {
    HeldOrdinaryView(owner)
}

/// Borrow the actual local settlement receipts without constructing authority.
///
/// ```
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::settled_ordinary;
/// use solstone_core_retention::remove_marked::ReviewedRemovalReport;
/// fn view(report: &ReviewedRemovalReport) -> impl Serialize + '_ { settled_ordinary(report) }
/// ```
/// ```compile_fail,E0515
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::settled_ordinary;
/// use solstone_core_retention::remove_marked::ReviewedRemovalReport;
/// fn escape<'a>(report: ReviewedRemovalReport) -> impl Serialize + 'a { settled_ordinary(&report) }
/// ```
pub fn settled_ordinary(report: &ReviewedRemovalReport) -> impl Serialize + '_ {
    SettledOrdinaryView(report)
}

/// Borrow the archive result and its original operation lifetime.
///
/// ```
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::archive;
/// use solstone_core_retention::remove_marked::ReviewedArchiveRemoval;
/// fn view<'a, 'op: 'a>(owner: &'a ReviewedArchiveRemoval<'op>) -> impl Serialize + 'a {
///     archive(owner)
/// }
/// ```
/// ```compile_fail,E0515
/// use serde::Serialize;
/// use solstone_core_backup_web::removal_report::archive;
/// use solstone_core_retention::remove_marked::ReviewedArchiveRemoval;
/// fn escape<'a, 'op: 'a>(owner: ReviewedArchiveRemoval<'op>) -> impl Serialize + 'a {
///     archive(&owner)
/// }
/// ```
pub fn archive<'a, 'op>(owner: &'a ReviewedArchiveRemoval<'op>) -> impl Serialize + 'a {
    ArchiveView(owner)
}

/// Borrow the actual typed active result without moving its original owners.
pub fn owned_archive(owner: &OwnedReviewedArchiveRun) -> impl Serialize + '_ {
    OwnedArchiveView(owner)
}

/// Borrow the closed hold, including independent consuming finish refusal.
pub fn held_archive(owner: &OwnedReviewedArchiveRemoval) -> impl Serialize + '_ {
    HeldArchiveView(owner)
}

/// Borrow resource-free facts kept beside the original runtime finish result.
pub fn settled_archive(report: &ReviewedArchiveReport) -> impl Serialize + '_ {
    SettledArchiveView(report)
}

struct OwnedArchiveView<'a>(&'a OwnedReviewedArchiveRun);
struct HeldArchiveView<'a>(&'a OwnedReviewedArchiveRemoval);
struct SettledArchiveView<'a>(&'a ReviewedArchiveReport);

struct OrdinaryView<'a>(&'a ReviewedRemoval);
struct HeldOrdinaryView<'a>(&'a OwnedReviewedRemoval);
struct SettledOrdinaryView<'a>(&'a ReviewedRemovalReport);
struct ArchiveView<'a, 'op>(&'a ReviewedArchiveRemoval<'op>);

#[derive(Serialize)]
struct Common<'a> {
    outcome: &'a solstone_core_retention::Outcome,
    refusal: Option<Value>,
    attempts: Vec<Value>,
    stage_failures: Vec<Value>,
    mutation: Option<Value>,
}

fn common<'a>(
    outcome: &'a solstone_core_retention::Outcome,
    refusal: Option<&ReviewedRemovalRefusal>,
    attempts: &[ReviewedFileRemoval],
    stages: &[solstone_core_journal_io::media::OwnedMediaFailure<
        Option<solstone_core_journal_io::media::MediaStage>,
    >],
    mutation: Option<&Result<BoundMutationOutcome, BoundMutationRefusal>>,
) -> Common<'a> {
    Common {
        outcome,
        refusal: refusal.map(refusal_facts),
        attempts: attempts.iter().map(attempt_facts).collect(),
        stage_failures: stages
            .iter()
            .map(|failure| {
                json!({
                    "error": io_kind(&failure.error), "retained_stage": failure.owned.is_some(),
                })
            })
            .collect(),
        mutation: mutation.map(mutation_facts),
    }
}

impl Serialize for OrdinaryView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct View<'a> {
            #[serde(flatten)]
            common: Common<'a>,
            carrier_preparation: Option<Value>,
            carrier_failure: Option<Value>,
        }
        let owner = self.0;
        View {
            common: common(
                owner.outcome(),
                owner.refusal(),
                owner.attempts(),
                owner.stage_failures(),
                owner.mutation(),
            ),
            carrier_preparation: owner.carrier_preparation().map(carrier_facts),
            carrier_failure: owner.carrier_failure().map(|failure| {
                json!({
                    "receipt": carrier_facts(failure.receipt()),
                    "error": io_kind(failure.error()),
                    "retained_stage": failure.diagnostic_path().is_some(),
                })
            }),
        }
        .serialize(serializer)
    }
}

impl Serialize for HeldOrdinaryView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct View<'a> {
            #[serde(flatten)]
            ordinary: OrdinaryView<'a>,
            finish_refusal: Option<Value>,
            retirement: Option<Value>,
        }
        let owner = self.0;
        View {
            ordinary: OrdinaryView(owner.removal()),
            finish_refusal: owner.finish_refusal().map(|reason| match reason {
                ReviewedRemovalFinishRefusal::IncompleteSelection => {
                    json!({"kind":"incomplete_selection"})
                }
                ReviewedRemovalFinishRefusal::Native(reason) => {
                    json!({"kind":"native", "detail":finish_refusal_facts(reason)})
                }
                ReviewedRemovalFinishRefusal::Retirement => json!({"kind":"retirement"}),
            }),
            retirement: owner.retirement().map(retirement_facts),
        }
        .serialize(serializer)
    }
}

impl Serialize for SettledOrdinaryView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct View<'a> {
            #[serde(flatten)]
            common: Common<'a>,
            finish_refusal: Option<Value>,
            retirement: Option<Value>,
        }
        let report = self.0;
        View {
            common: Common {
                outcome: report.outcome(),
                refusal: report.refusal().map(refusal_facts),
                attempts: report
                    .attempts()
                    .iter()
                    .map(settled_attempt_facts)
                    .collect(),
                stage_failures: Vec::new(),
                mutation: report.mutation().map(mutation_facts),
            },
            finish_refusal: None,
            retirement: report.retirement().map(retirement_facts),
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
struct ArchiveFacts<'a> {
    #[serde(flatten)]
    common: Common<'a>,
    approval_readback_error: Option<String>,
    approval_cleanup: Option<solstone_core_journal_io::media::Cleanup>,
    approval_native_error: Option<String>,
    archive_reads: Vec<Value>,
    finish_refusal: Option<Value>,
}

fn read_facts(read: ArchiveReadState<'_>) -> Value {
    match read {
        ArchiveReadState::Stage(stage) => {
            json!({"state":"stage", "retained_stage":stage.is_some()})
        }
        ArchiveReadState::Target(_) => json!({"state":"target"}),
        ArchiveReadState::Prepared { file, cleanup, .. } => json!({
            "state":"prepared", "name":file.name, "bytes":file.bytes, "cleanup":cleanup,
        }),
        ArchiveReadState::Readback {
            file,
            failure,
            cleanup,
            ..
        } => json!({
            "state":"readback", "name":file.name, "bytes":file.bytes,
            "reason":failure.reason().to_string(), "cleanup":cleanup,
        }),
    }
}

macro_rules! archive_view {
    ($view:ty, $finish:expr) => {
        impl Serialize for $view {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let owner = self.0;
                ArchiveFacts {
                    common: common(
                        owner.outcome(),
                        owner.refusal(),
                        owner.attempts(),
                        owner.stage_failures(),
                        owner.mutation(),
                    ),
                    approval_readback_error: owner
                        .approval_readback_error()
                        .map(|error| error.to_string()),
                    approval_cleanup: owner.approval_cleanup(),
                    approval_native_error: owner.approval_native_error().map(io_kind),
                    archive_reads: owner.archive_reads().map(read_facts).collect(),
                    finish_refusal: $finish(owner),
                }
                .serialize(serializer)
            }
        }
    };
}
archive_view!(ArchiveView<'_, '_>, |_| None);
archive_view!(OwnedArchiveView<'_>, |_| None);
archive_view!(
    HeldArchiveView<'_>,
    |owner: &OwnedReviewedArchiveRemoval| owner.finish_refusal().map(finish_refusal_facts)
);

impl Serialize for SettledArchiveView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let report = self.0;
        ArchiveFacts {
            common: Common {
                outcome: report.outcome(),
                refusal: None,
                attempts: report
                    .attempts()
                    .iter()
                    .map(settled_attempt_facts)
                    .collect(),
                stage_failures: Vec::new(),
                mutation: report.mutation().map(mutation_facts),
            },
            approval_readback_error: None,
            approval_cleanup: None,
            approval_native_error: None,
            archive_reads: report
                .archive_reads()
                .iter()
                .map(|read| {
                    json!({
                        "state":"confirmed", "name":read.file().name, "bytes":read.file().bytes,
                        "cleanup":read.cleanup(),
                    })
                })
                .collect(),
            finish_refusal: None,
        }
        .serialize(serializer)
    }
}

fn finish_refusal_facts(reason: &ReviewedArchiveFinishRefusal) -> Value {
    use ReviewedArchiveFinishRefusal as Refusal;
    match reason {
        Refusal::IncompleteSelection => json!({"kind":"incomplete_selection"}),
        Refusal::ActorResources => json!({"kind":"actor_resources"}),
        Refusal::Readback(error) => json!({"kind":"readback", "reason":error.to_string()}),
        Refusal::NativeCleanup(cleanup) => json!({"kind":"native_cleanup", "cleanup":cleanup}),
        Refusal::Changed(error) => json!({"kind":"changed", "error":io_kind(error)}),
        Refusal::Census(error) => census_facts(error),
        Refusal::Obligations(entries) => json!({"kind":"obligations", "count":entries.len()}),
    }
}

/// Which entry the census could not observe, and how, as a journal-relative path.
fn census_facts(error: &RecoveryObservationError) -> Value {
    use RecoveryObservationFailure as Failure;
    let failure = match error.failure {
        Failure::Io => "io",
        Failure::Changed => "changed",
        Failure::DirectoryAdded => "directory_added",
        Failure::NotDirectory => "not_directory",
        Failure::Budget(_) => "budget",
    };
    let path = error
        .relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    json!({
        "kind":"census", "failure":failure, "path":path,
        "error":error.source.as_ref().map(io_kind),
        // Invalid-data refusals are fixed strings that tell a changed listing
        // from a refused name; other messages can carry absolute paths.
        "detail":error.source.as_ref()
            .filter(|source| source.kind() == io::ErrorKind::InvalidData)
            .map(ToString::to_string),
    })
}

fn io_kind(error: &io::Error) -> String {
    error.kind().to_string()
}

fn carrier_facts(receipt: &ActivityCarrierPreparationReceipt) -> Value {
    json!({
        "stage_durability":receipt.stage_durability(),
        "root_durability":receipt.root_durability(),
        "confirmation":receipt.confirmation(),
    })
}

fn retirement_facts(receipt: &ActivityCarrierRetirementReceipt) -> Value {
    json!({
        "effect":match receipt.effect() {
            ActivityCarrierRetirementEffect::NotAttempted => "not_attempted",
            ActivityCarrierRetirementEffect::Attempted => "attempted",
            ActivityCarrierRetirementEffect::Removed => "removed",
        },
        "root_durability":receipt.root_durability(),
        "confirmation":receipt.confirmation(),
        "failures":receipt.failures().iter().map(io_kind).collect::<Vec<_>>(),
    })
}

macro_rules! attempt_view {
    ($name:ident, $ty:ty, $receipt:expr) => {
        fn $name(attempt: &$ty) -> Value {
            #[cfg(unix)]
            let append = attempt.append().map(append_facts);
            #[cfg(not(unix))]
            let append: Option<Value> = None;
            #[cfg(unix)]
            let marker = attempt.marker().map(marker_facts);
            #[cfg(not(unix))]
            let marker: Option<Value> = None;
            json!({
                "relative":attempt.relative(), "class":attempt.class(),
                "native":removal_facts($receipt(attempt)),
                "append":append, "marker":marker,
                "binding_error":attempt.binding_error().map(io_kind),
            })
        }
    };
}
fn active_receipt(attempt: &ReviewedFileRemoval) -> &RemovalReceipt {
    attempt.native().receipt()
}
attempt_view!(attempt_facts, ReviewedFileRemoval, active_receipt);
fn settled_receipt(attempt: &ReviewedRemovalFileReport) -> &RemovalReceipt {
    attempt.native()
}
attempt_view!(
    settled_attempt_facts,
    ReviewedRemovalFileReport,
    settled_receipt
);

fn removal_facts(receipt: &RemovalReceipt) -> Value {
    json!({
        "effect":receipt.effect, "original_name":receipt.original_name,
        "claim_confirmation":receipt.claim_confirmation,
        "file_durability":receipt.file_durability, "source_durability":receipt.source_durability,
        "claim_durability":receipt.claim_durability, "stage_parent_durability":receipt.stage_parent_durability,
        "cleanup":receipt.cleanup, "restoration_attempted":receipt.restoration_attempted,
        "failures":receipt.failures.iter().map(|failure| json!({
            "phase":match failure.phase {
                RemovalPhase::Mutation => "mutation", RemovalPhase::FileSync => "file_sync",
                RemovalPhase::SourceSync => "source_sync", RemovalPhase::ClaimSync => "claim_sync",
                RemovalPhase::StageParentSync => "stage_parent_sync", RemovalPhase::Cleanup => "cleanup",
                RemovalPhase::Completion => "completion",
            }, "error":io_kind(&failure.error),
        })).collect::<Vec<_>>(),
    })
}

#[cfg(unix)]
fn append_facts(receipt: &solstone_core_journal_io::append::BoundAppendReceipt) -> Value {
    use solstone_core_journal_io::append::{
        BoundAppendConfirmation, BoundAppendDurability, BoundAppendEffect,
    };
    let (effect, bytes) = match receipt.effect() {
        BoundAppendEffect::NotAttempted => ("not_attempted", None),
        BoundAppendEffect::Attempted { written } => ("attempted", written),
        BoundAppendEffect::Appended { bytes } => ("appended", Some(bytes)),
    };
    let durability = |value| match value {
        BoundAppendDurability::Unproven => "unproven",
        BoundAppendDurability::Synced => "synced",
    };
    json!({
        "effect":effect, "bytes":bytes,
        "file_durability":durability(receipt.file_durability()), "parent_durability":durability(receipt.parent_durability()),
        "confirmation":match receipt.confirmation() { BoundAppendConfirmation::Unverified => "unverified", BoundAppendConfirmation::Confirmed => "confirmed" },
        "failures":receipt.failures().iter().map(io_kind).collect::<Vec<_>>(),
    })
}

#[cfg(unix)]
fn marker_facts(
    result: &Result<
        solstone_core_journal_io::health_marker::BoundStreamMarkerPublication,
        solstone_core_journal_io::health_marker::BoundStreamMarkerError,
    >,
) -> Value {
    use solstone_core_journal_io::health_marker::BoundStreamMarkerError as Error;
    match result {
        Ok(publication) => json!({
            "publication":publication_facts(publication.publication()), "generation":publication.generation(),
            "binding_error":publication.binding_error().map(directory_facts),
            "lock_error":publication.lock_error().map(lock_facts),
        }),
        Err(error) => match error {
            Error::Parent(error) => json!({"error":"parent", "detail":directory_facts(error)}),
            Error::Lock(error) => json!({"error":"lock", "detail":lock_facts(error)}),
            Error::Read(error) => json!({"error":"read", "detail":directory_facts(error)}),
            Error::Malformed => json!({"error":"malformed"}),
            Error::Overflow => json!({"error":"overflow"}),
            Error::Serialize(_) => json!({"error":"serialize"}),
            Error::Publish(error) => json!({"error":"publish", "detail":write_facts(error)}),
        },
    }
}

#[cfg(unix)]
fn publication_facts(publication: &solstone_core_journal_io::atomic::BoundAtomicOutcome) -> Value {
    use solstone_core_journal_io::atomic::BoundAtomicOutcome as Publication;
    let (durability, observation) = match publication {
        Publication::Published { .. } => (None, None),
        Publication::PublishedDurabilityUncertain { source, .. } => (Some(io_kind(source)), None),
        Publication::PublishedObservationUncertain {
            source,
            durability_source,
            ..
        } => (
            durability_source.as_ref().map(io_kind),
            Some(io_kind(source)),
        ),
    };
    json!({"effect":"published", "durability_error":durability, "observation_error":observation})
}

/// Project actual register publication and independent uncertainty/refusal facts.
/// The caller retains the whole result through response serialization.
pub fn mutation_facts(result: &Result<BoundMutationOutcome, BoundMutationRefusal>) -> Value {
    match result {
        Err(error) => json!({"publication":"refused", "error":mutation_error(error)}),
        Ok(BoundMutationOutcome::Unchanged(result)) => {
            json!({"publication":"unchanged", "status":result.status})
        }
        Ok(BoundMutationOutcome::Published(result)) => {
            json!({"publication":"published", "status":result.status})
        }
        Ok(BoundMutationOutcome::PublishedUncertain {
            candidate,
            uncertainty,
        }) => json!({
            "publication":"published_uncertain", "status":candidate.status,
            "durability_error":uncertainty.durability.as_ref().map(io_kind),
            "observation_error":uncertainty.observation.as_ref().map(io_kind),
            "binding_errors":uncertainty.bindings.iter().map(review_tag).collect::<Vec<_>>(),
            "lock_error":uncertainty.lock.as_ref().map(lock_facts),
        }),
    }
}

fn mutation_error(error: &BoundMutationRefusal) -> Value {
    match error {
        BoundMutationRefusal::Unsupported => json!({"kind":"unsupported"}),
        BoundMutationRefusal::Review(error) => json!({"kind":"review", "reason":review_tag(error)}),
        BoundMutationRefusal::Lock(error) => json!({"kind":"lock", "detail":lock_facts(error)}),
        BoundMutationRefusal::Serialize(_) => json!({"kind":"serialize"}),
        #[cfg(unix)]
        BoundMutationRefusal::Write(error) => json!({"kind":"write", "detail":write_facts(error)}),
        #[cfg(windows)]
        BoundMutationRefusal::Write(error) => match error {
            solstone_core_journal_io::atomic::BoundAtomicPublishError::NamespaceChanged => {
                json!({"kind":"namespace_changed"})
            }
            solstone_core_journal_io::atomic::BoundAtomicPublishError::Atomic(error) => {
                json!({"kind":"write", "detail":write_facts(error)})
            }
        },
    }
}

#[cfg(any(unix, windows))]
fn write_facts(error: &solstone_core_journal_io::atomic::DetailedAtomicError) -> Value {
    json!({"operation":error.operation, "error":io_kind(&error.source), "orphan_stage":error.orphan_stage.is_some(), "cleanup_error":error.cleanup_error.as_ref().map(io_kind)})
}

fn review_tag(error: &ReviewRefusal) -> &'static str {
    match error {
        ReviewRefusal::Empty => "empty",
        ReviewRefusal::Duplicate => "duplicate",
        ReviewRefusal::Missing { .. } => "missing",
        ReviewRefusal::Stale { .. } => "stale",
        ReviewRefusal::Failed { .. } => "failed",
        ReviewRefusal::NotRequired { .. } => "not_required",
        ReviewRefusal::SourceRootChanged { .. } => "source_root_changed",
        ReviewRefusal::RootChanged => "root_changed",
        ReviewRefusal::Root(_) => "root",
        ReviewRefusal::Read(_) => "read",
        ReviewRefusal::Register(_) => "register",
    }
}

fn lock_facts(error: &ExistingParentLockError) -> Value {
    let (kind, source) = match error {
        ExistingParentLockError::InvalidLockPath { .. } => ("invalid_lock_path", None),
        ExistingParentLockError::MissingParent { .. } => ("missing_parent", None),
        ExistingParentLockError::UnsafeParent { .. } => ("unsafe_parent", None),
        ExistingParentLockError::UnsafeLockEntry { .. } => ("unsafe_lock_entry", None),
        ExistingParentLockError::WrongMode { .. } => ("wrong_mode", None),
        ExistingParentLockError::Io { source, .. } => ("io", Some(io_kind(source))),
        ExistingParentLockError::ParentChanged { .. } => ("parent_changed", None),
        ExistingParentLockError::NamespaceChanged { .. } => ("namespace_changed", None),
        ExistingParentLockError::Timeout(_) => ("timeout", None),
    };
    json!({"kind":kind, "error":source})
}

fn directory_facts(error: &FlatDirectoryError) -> Value {
    let (kind, source) = match error {
        FlatDirectoryError::InvalidRelativePath { .. } => ("invalid_relative_path", None),
        FlatDirectoryError::InvalidName { .. } => ("invalid_name", None),
        FlatDirectoryError::NotDirectory { .. } => ("not_directory", None),
        FlatDirectoryError::SymlinkRefused { .. } => ("symlink_refused", None),
        FlatDirectoryError::NotRegular { .. } => ("not_regular", None),
        FlatDirectoryError::SizeLimitExceeded { .. } => ("size_limit_exceeded", None),
        FlatDirectoryError::IdentityChanged { .. } => ("identity_changed", None),
        FlatDirectoryError::EnumerationChanged { .. } => ("enumeration_changed", None),
        FlatDirectoryError::Io { source, .. } => ("io", Some(io_kind(source))),
    };
    json!({"kind":kind, "error":source})
}

fn refusal_facts(error: &ReviewedRemovalRefusal) -> Value {
    use ReviewedRemovalRefusal as Refusal;
    let kind = match error {
        Refusal::Unsupported => "unsupported",
        Refusal::InvalidSelection => "invalid_selection",
        Refusal::Io(error) => return json!({"kind":"io", "error":io_kind(error)}),
        Refusal::Mutation(error) => {
            return json!({"kind":"mutation", "detail":mutation_error(error)});
        }
        Refusal::Activity(_) => "activity",
        Refusal::Census(_) => "census",
        Refusal::Obligations(_) => "obligations",
        Refusal::Configuration(_) => "configuration",
        #[cfg(any(unix, windows))]
        Refusal::Directory(error) => {
            return json!({"kind":"directory", "detail":directory_facts(error)});
        }
        Refusal::Changed => "changed",
        Refusal::CarrierPreparation => "carrier_preparation",
        Refusal::ArchiveApproval => "archive_approval",
        Refusal::Catalog(_) => "catalog",
        Refusal::Readback(error) => return json!({"kind":"readback", "reason":error.to_string()}),
        Refusal::OffloadFloorRequired => "offload_floor_required",
        Refusal::InsufficientSpace {
            available,
            required,
        } => {
            return json!({"kind":"insufficient_space", "available":available, "required":required});
        }
    };
    json!({"kind":kind})
}

#[cfg(all(test, feature = "full-tests"))]
mod tests;

#[cfg(test)]
mod census_tests {
    use super::*;

    #[test]
    fn a_census_finish_refusal_names_the_entry_and_how_it_moved() {
        let facts = finish_refusal_facts(&ReviewedArchiveFinishRefusal::Census(
            RecoveryObservationError {
                failure: RecoveryObservationFailure::Io,
                relative: std::path::Path::new("chronicle")
                    .join("20261001")
                    .join("import.audio"),
                source: Some(io::Error::from(io::ErrorKind::PermissionDenied)),
            },
        ));
        assert_eq!(
            facts,
            json!({
                "kind":"census", "failure":"io", "path":"chronicle/20261001/import.audio",
                "error":io::ErrorKind::PermissionDenied.to_string(), "detail":null,
            })
        );
        let facts = finish_refusal_facts(&ReviewedArchiveFinishRefusal::Census(
            RecoveryObservationError {
                failure: RecoveryObservationFailure::Io,
                relative: std::path::PathBuf::from("health"),
                source: Some(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "retained media binding changed",
                )),
            },
        ));
        assert_eq!(facts["detail"], "retained media binding changed");
        let facts = finish_refusal_facts(&ReviewedArchiveFinishRefusal::Census(
            RecoveryObservationError {
                failure: RecoveryObservationFailure::DirectoryAdded,
                relative: std::path::PathBuf::new(),
                source: None,
            },
        ));
        assert_eq!(
            facts,
            json!({"kind":"census", "failure":"directory_added", "path":"", "error":null, "detail":null})
        );
    }
}
