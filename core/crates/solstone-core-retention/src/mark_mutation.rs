// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Exact-observation updates for removal proposals and durable recovery plans.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    BoundParentLock, ExistingParentLockError, JournalRoot, JsonWriteOptions,
    acquire_existing_parent_lock_bound, hold_lock, write_json,
};
#[cfg(unix)]
use solstone_core_journal_io::{
    FlatDirectory as HealthDirectory, open_flat_directory_bound as open_health,
};
#[cfg(windows)]
use solstone_core_journal_io::{
    WindowsFlatDirectory as HealthDirectory, open_windows_flat_directory_bound as open_health,
};

use crate::archive_proof::OffloadOffer;
use crate::marks::{
    Failure, Mark, MarkState, Register, StoreError, load_path, register_lock_options,
    register_path, validate,
};
#[cfg(any(unix, windows))]
use crate::review_context::MAX_REGISTER_BYTES;
use crate::review_context::{
    CheckedReviewContext, ReviewRefusal, check_marks, read_at_health, revalidate_health,
};

mod recovered;
pub use recovered::{
    RecoveredRestoredMarkMutation, RecoveredRestoredMarkRefusal,
    conditional_retire_recovered_restored,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStatus {
    Applied,
    AlreadyApplied,
    Missing,
    Stale,
    Collision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionalResult {
    pub status: MutationStatus,
    pub register: Register,
}

#[derive(Debug)]
pub enum BoundMutationRefusal {
    Unsupported,
    Review(ReviewRefusal),
    Lock(ExistingParentLockError),
    Serialize(serde_json::Error),
    #[cfg(unix)]
    Write(solstone_core_journal_io::atomic::DetailedAtomicError),
    #[cfg(windows)]
    Write(solstone_core_journal_io::atomic::BoundAtomicPublishError),
}

impl From<ReviewRefusal> for BoundMutationRefusal {
    fn from(value: ReviewRefusal) -> Self {
        Self::Review(value)
    }
}

/// Failures after publication; the candidate may no longer be the named register.
#[derive(Debug, Default)]
pub struct RegisterUncertainty {
    pub durability: Option<io::Error>,
    pub observation: Option<io::Error>,
    pub bindings: Vec<ReviewRefusal>,
    pub lock: Option<ExistingParentLockError>,
}

impl RegisterUncertainty {
    #[cfg(any(unix, windows))]
    fn confirmed(&self) -> bool {
        self.durability.is_none()
            && self.observation.is_none()
            && self.bindings.is_empty()
            && self.lock.is_none()
    }
}

#[must_use]
#[derive(Debug)]
pub enum BoundMutationOutcome {
    Unchanged(ConditionalResult),
    Published(ConditionalResult),
    PublishedUncertain {
        candidate: ConditionalResult,
        uncertainty: RegisterUncertainty,
    },
}

/// A deterministic update that can be recorded before publishing restored media.
/// Deserialization supplies data only; execution validates the entire plan again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedMarkMutation {
    expected: Mark,
    replacement: Option<Mark>,
}

fn invalid(reason: &'static str) -> StoreError {
    StoreError::Integrity { reason }
}

fn validate_one(mark: &Mark) -> Result<(), StoreError> {
    let mut register = Register::empty();
    if mark.offload.is_some() {
        register.version = 2;
    }
    register.marks.insert(mark.id.clone(), mark.clone());
    validate(&register)
}

impl PlannedMarkMutation {
    pub fn resolve(expected: &Mark) -> Result<Self, StoreError> {
        validate_one(expected)?;
        Ok(Self {
            expected: expected.clone(),
            replacement: None,
        })
    }

    pub fn failure(expected: &Mark, failure: Failure) -> Result<Self, StoreError> {
        validate_one(expected)?;
        let mut replacement = expected.clone();
        replacement.state = MarkState::Failed(failure);
        Ok(Self {
            expected: expected.clone(),
            replacement: Some(replacement),
        })
    }

    /// Keep only a sorted subset of the offered files, preserving the nonce.
    /// Legacy proposals have no per-file bytes or witness and cannot be shrunk.
    pub fn remainder(expected: &Mark, names: &[String]) -> Result<Self, StoreError> {
        validate_one(expected)?;
        if names.is_empty() {
            return Self::resolve(expected);
        }
        let names_set: BTreeSet<_> = names.iter().map(String::as_str).collect();
        if names_set.len() != names.len()
            || names
                .iter()
                .map(String::as_str)
                .ne(names_set.iter().copied())
            || names
                .iter()
                .any(|name| expected.proposal.names.binary_search(name).is_err())
        {
            return Err(invalid(
                "a remainder must be a sorted subset of the proposal",
            ));
        }
        let offer = expected
            .offload
            .as_ref()
            .ok_or_else(|| invalid("an unbound proposal has no exact remainder"))?;
        let mut proof = offer.proof.clone();
        proof
            .files
            .retain(|file| names_set.contains(file.name.as_str()));
        let bytes = proof
            .bytes()
            .map_err(|_| invalid("remainder bytes overflow"))?;
        let mut replacement = expected.clone();
        replacement.proposal.names = names.to_vec();
        replacement.proposal.bytes = bytes;
        replacement.id = crate::marks::MarkId::derive(expected.class, &expected.target, names);
        replacement.offload = Some(
            OffloadOffer::with_nonce(proof, offer.nonce.clone())
                .map_err(|_| invalid("invalid remainder archive evidence"))?,
        );
        validate_one(&replacement)?;
        Ok(Self {
            expected: expected.clone(),
            replacement: Some(replacement),
        })
    }

    pub fn expected(&self) -> &Mark {
        &self.expected
    }
    pub fn replacement(&self) -> Option<&Mark> {
        self.replacement.as_ref()
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        validate_one(&self.expected)?;
        let Some(replacement) = &self.replacement else {
            return Ok(());
        };
        validate_one(replacement)?;
        if replacement == &self.expected {
            return Ok(());
        }
        if let MarkState::Failed(failure) = &replacement.state
            && Self::failure(&self.expected, failure.clone())?
                .replacement
                .as_ref()
                == Some(replacement)
        {
            return Ok(());
        }
        if Self::remainder(&self.expected, &replacement.proposal.names)?
            .replacement
            .as_ref()
            == Some(replacement)
        {
            return Ok(());
        }
        Err(invalid(
            "a planned mutation is not an exact failure or remainder",
        ))
    }
}

fn apply(
    register: &mut Register,
    plan: &PlannedMarkMutation,
) -> Result<MutationStatus, StoreError> {
    validate(register)?;
    plan.validate()?;
    match register.marks.get(&plan.expected.id) {
        None => {
            return Ok(
                if plan.replacement.as_ref().is_some_and(|replacement| {
                    register.marks.get(&replacement.id) == Some(replacement)
                }) {
                    MutationStatus::AlreadyApplied
                } else {
                    MutationStatus::Missing
                },
            );
        }
        Some(current) if current != &plan.expected => {
            return Ok(if plan.replacement.as_ref() == Some(current) {
                MutationStatus::AlreadyApplied
            } else {
                MutationStatus::Stale
            });
        }
        Some(_) => {}
    }
    if let Some(replacement) = &plan.replacement {
        if replacement == &plan.expected {
            return Ok(MutationStatus::AlreadyApplied);
        }
        if replacement.id != plan.expected.id && register.marks.contains_key(&replacement.id) {
            return Ok(MutationStatus::Collision);
        }
    }
    register.marks.remove(&plan.expected.id);
    if let Some(replacement) = &plan.replacement {
        register
            .marks
            .insert(replacement.id.clone(), replacement.clone());
    }
    validate(register)?;
    Ok(MutationStatus::Applied)
}

/// Apply only to the complete expected Mark under the register lock.
/// Existing v2 registers remain read-only until their writer is enabled.
pub fn conditional_mutate(
    journal: &Path,
    plan: &PlannedMarkMutation,
) -> Result<ConditionalResult, StoreError> {
    plan.validate()?;
    let path = register_path(journal);
    let _lock = hold_lock(&path, register_lock_options()).map_err(StoreError::Lock)?;
    let mut register = load_path(&path)?;
    if register.version != 1 || plan.expected.offload.is_some() {
        return Err(StoreError::WriterHeld);
    }
    let status = apply(&mut register, plan)?;
    if status == MutationStatus::Applied {
        write_json(
            &path,
            &register,
            JsonWriteOptions {
                mode: Some(0o600),
                ..JsonWriteOptions::default()
            },
        )
        .map_err(StoreError::Write)?;
    }
    Ok(ConditionalResult { status, register })
}

fn begin_bound(
    root: &JournalRoot,
    health: &HealthDirectory,
) -> Result<(BoundParentLock, Register), BoundMutationRefusal> {
    let (lock, register) = begin_bound_read(root, health)?;
    if register.version != 1 {
        return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
            StoreError::WriterHeld,
        )));
    }
    Ok((lock, register))
}

fn begin_bound_read(
    root: &JournalRoot,
    health: &HealthDirectory,
) -> Result<(BoundParentLock, Register), BoundMutationRefusal> {
    let (lock, register) = begin_bound_optional(root, health)?;
    let register = register.ok_or_else(|| {
        BoundMutationRefusal::Review(ReviewRefusal::Register(invalid(
            "the retained removal register disappeared",
        )))
    })?;
    Ok((lock, register))
}

fn begin_bound_optional(
    root: &JournalRoot,
    health: &HealthDirectory,
) -> Result<(BoundParentLock, Option<Register>), BoundMutationRefusal> {
    revalidate_health(root, health)?;
    let options = register_lock_options();
    let lock = acquire_existing_parent_lock_bound(
        health,
        OsStr::new("retention-marks.json.lock"),
        options.timeout,
        options.poll_interval,
    )
    .map_err(BoundMutationRefusal::Lock)?;
    revalidate_health(root, health)?;
    let register = read_at_health(health)?;
    lock.revalidate_bound(health, OsStr::new("retention-marks.json.lock"))
        .map_err(BoundMutationRefusal::Lock)?;
    Ok((lock, register))
}

/// Withdraw only the strict subsets minted by retained kept-empty inspection.
#[cfg(any(unix, windows))]
pub(crate) fn reconcile_empty_observed(
    observation: &crate::archive_offer::reconciliation::unix::ReconciliationObservation<'_, '_>,
) -> Result<
    (
        BoundMutationOutcome,
        Option<crate::archive_offer::OffloadOfferRefusal>,
    ),
    crate::archive_offer::reconciliation::unix::ReconciliationWriteRefusal,
> {
    use crate::archive_offer::reconciliation::unix::ReconciliationWriteRefusal;
    let root = observation.journal_root();
    observation
        .revalidate()
        .map_err(ReconciliationWriteRefusal::Observation)?;
    let health = open_health(root, OsStr::new("health"), root.canonical_path())
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Read(error)))?
        .ok_or(BoundMutationRefusal::Review(ReviewRefusal::RootChanged))?;
    let (lock, mut register) = begin_bound_read(root, &health)?;
    observation
        .revalidate()
        .map_err(ReconciliationWriteRefusal::Observation)?;
    for expected in observation.expected() {
        if register.marks.get(&expected.id) != Some(expected) {
            return Err(BoundMutationRefusal::Review(ReviewRefusal::Stale {
                id: expected.id.clone(),
            })
            .into());
        }
    }
    if observation.plans().is_empty() {
        return Err(BoundMutationRefusal::Review(ReviewRefusal::Empty).into());
    }
    for plan in observation.plans() {
        let status = apply(&mut register, plan)
            .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
        if status != MutationStatus::Applied {
            return Err(BoundMutationRefusal::Review(ReviewRefusal::Stale {
                id: plan.expected().id.clone(),
            })
            .into());
        }
    }
    observation
        .revalidate()
        .map_err(ReconciliationWriteRefusal::Observation)?;
    let outcome = publish_bound(
        root,
        &health,
        &lock,
        ConditionalResult {
            status: MutationStatus::Applied,
            register,
        },
    )?;
    let changed = observation.revalidate().err();
    Ok((outcome, changed))
}

/// Private final publication phase after owning archive verification. No WAN
/// or bookkeeping runs while this retained register exclusion is held.
#[cfg(any(unix, windows))]
pub(crate) struct OfferMarkGuard<'a> {
    root: &'a JournalRoot,
    health: HealthDirectory,
    lock: BoundParentLock,
    result: ConditionalResult,
}

#[cfg(any(unix, windows))]
impl<'a> OfferMarkGuard<'a> {
    pub(crate) fn begin(
        root: &'a JournalRoot,
        selected: &[(Option<&Mark>, &Mark)],
    ) -> Result<Self, BoundMutationRefusal> {
        let health = open_health(root, OsStr::new("health"), root.canonical_path())
            .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Read(error)))?
            .ok_or(BoundMutationRefusal::Review(ReviewRefusal::RootChanged))?;
        let (lock, register) = begin_bound_optional(root, &health)?;
        let mut register = register.unwrap_or_else(Register::empty);
        let mut ids = BTreeSet::new();
        if selected.is_empty() {
            return Err(BoundMutationRefusal::Review(ReviewRefusal::Empty));
        }
        let mut changed = register.version != 2;
        for (expected, candidate) in selected {
            if !ids.insert(&candidate.id)
                || candidate.offload.is_none()
                || candidate.class != crate::marks::RemovalClass::OffloadRawRelease
                || !matches!(candidate.state, MarkState::Marked)
            {
                return Err(BoundMutationRefusal::Review(ReviewRefusal::Duplicate));
            }
            if register.marks.get(&candidate.id) != *expected {
                return Err(BoundMutationRefusal::Review(ReviewRefusal::Stale {
                    id: candidate.id.clone(),
                }));
            }
            changed |= register.marks.get(&candidate.id) != Some(candidate);
        }
        register.version = 2;
        for (_, candidate) in selected {
            register
                .marks
                .insert(candidate.id.clone(), (*candidate).clone());
        }
        validate(&register)
            .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
        let status = if changed {
            MutationStatus::Applied
        } else {
            MutationStatus::AlreadyApplied
        };
        Ok(Self {
            root,
            health,
            lock,
            result: ConditionalResult { status, register },
        })
    }

    pub(crate) fn publish(self) -> Result<BoundMutationOutcome, BoundMutationRefusal> {
        if self.result.status == MutationStatus::Applied {
            publish_bound(self.root, &self.health, &self.lock, self.result)
        } else {
            revalidate_health(self.root, &self.health)?;
            self.lock
                .revalidate_bound(&self.health, OsStr::new("retention-marks.json.lock"))
                .map_err(BoundMutationRefusal::Lock)?;
            Ok(BoundMutationOutcome::Unchanged(self.result))
        }
    }
}

#[cfg(any(unix, windows))]
fn publish_bound(
    root: &JournalRoot,
    health: &HealthDirectory,
    lock: &BoundParentLock,
    result: ConditionalResult,
) -> Result<BoundMutationOutcome, BoundMutationRefusal> {
    validate(&result.register)
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
    let bytes =
        serde_json::to_vec_pretty(&result.register).map_err(BoundMutationRefusal::Serialize)?;
    if bytes.len() > MAX_REGISTER_BYTES {
        return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
            invalid("the replacement removal register exceeds the read bound"),
        )));
    }
    revalidate_health(root, health)?;
    lock.revalidate_bound(health, OsStr::new("retention-marks.json.lock"))
        .map_err(BoundMutationRefusal::Lock)?;
    #[cfg(unix)]
    let mut uncertainty = {
        use solstone_core_journal_io::BoundAtomicOutcome;
        match solstone_core_journal_io::atomic_replace_bound(
            health,
            OsStr::new("retention-marks.json"),
            &bytes,
            0o600,
        )
        .map_err(BoundMutationRefusal::Write)?
        {
            BoundAtomicOutcome::Published { .. } => RegisterUncertainty::default(),
            BoundAtomicOutcome::PublishedDurabilityUncertain { source, .. } => {
                RegisterUncertainty {
                    durability: Some(source),
                    ..RegisterUncertainty::default()
                }
            }
            BoundAtomicOutcome::PublishedObservationUncertain {
                source,
                durability_source,
                ..
            } => RegisterUncertainty {
                durability: durability_source,
                observation: Some(source),
                ..RegisterUncertainty::default()
            },
        }
    };
    #[cfg(windows)]
    let mut uncertainty = {
        use solstone_core_journal_io::atomic::DetailedAtomicOutcome;
        match solstone_core_journal_io::atomic_replace_detailed_bound(
            health,
            lock,
            OsStr::new("retention-marks.json"),
            &bytes,
            0o600,
        )
        .map_err(BoundMutationRefusal::Write)?
        {
            DetailedAtomicOutcome::Published => RegisterUncertainty::default(),
            DetailedAtomicOutcome::PublishedDurabilityUncertain { source } => RegisterUncertainty {
                durability: Some(source),
                ..RegisterUncertainty::default()
            },
            DetailedAtomicOutcome::PublishedParentPathRaced { sync_error } => RegisterUncertainty {
                durability: sync_error,
                observation: Some(io::Error::other("published parent path raced")),
                ..RegisterUncertainty::default()
            },
            DetailedAtomicOutcome::PublishedParentPathUnverified {
                observation,
                sync_error,
            } => RegisterUncertainty {
                durability: sync_error,
                observation: Some(observation),
                ..RegisterUncertainty::default()
            },
        }
    };
    if let Err(error) = revalidate_health(root, health) {
        uncertainty.bindings.push(error);
    }
    uncertainty.lock = lock
        .revalidate_bound(health, OsStr::new("retention-marks.json.lock"))
        .err();
    Ok(if uncertainty.confirmed() {
        BoundMutationOutcome::Published(result)
    } else {
        BoundMutationOutcome::PublishedUncertain {
            candidate: result,
            uncertainty,
        }
    })
}

/// Compare the complete expected row and publish beneath this retained journal.
/// Typed writers remain held; uncertainty after publication retains the candidate.
pub fn conditional_mutate_at_root(
    root: &JournalRoot,
    plan: &PlannedMarkMutation,
) -> Result<BoundMutationOutcome, BoundMutationRefusal> {
    plan.validate()
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
    root.revalidate_canonical_binding()
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Root(error)))?;
    let health = open_health(root, OsStr::new("health"), root.canonical_path())
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Read(error)))?
        .ok_or(BoundMutationRefusal::Review(ReviewRefusal::RootChanged))?;
    let (lock, mut register) = begin_bound(root, &health)?;
    if plan.expected.offload.is_some() {
        return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
            StoreError::WriterHeld,
        )));
    }
    let status = apply(&mut register, plan)
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
    let result = ConditionalResult { status, register };
    if status == MutationStatus::Applied {
        publish_bound(root, &health, &lock, result)
    } else {
        revalidate_health(root, &health)?;
        #[cfg(unix)]
        lock.revalidate_bound(&health, OsStr::new("retention-marks.json.lock"))
            .map_err(BoundMutationRefusal::Lock)?;
        Ok(BoundMutationOutcome::Unchanged(result))
    }
}

/// Borrowed observations for retiring exactly one durably restored file.
/// The owning verifier reads current native and catalog evidence independently.
pub struct RestoredMarkMutation<'a, 'clock> {
    pub read: &'a solstone_core_backup::archive_readback::ReadOperation<'clock>,
    pub operation: &'a crate::archive_catalog::RestoreOperation,
    pub confirmed: &'a solstone_core_journal_io::media::MediaObservation,
    pub plan: &'a PlannedMarkMutation,
    pub claims: &'a solstone_core_segment::output_attempt::OutputClaims,
    pub commit: &'a solstone_core_segment::output_attempt::SegmentCommit<'a>,
}

#[derive(Debug)]
pub enum RestoredMarkRefusal {
    Unsupported,
    InvalidPlan,
    Changed,
    Catalog(String),
    Readback(solstone_core_backup::archive_readback::ReadbackError),
    Io(io::Error),
    Segment(solstone_core_segment::SegmentError),
    Bound(Box<BoundMutationRefusal>),
    Intent(crate::restore_intent::RepairHold),
    Recovery(Box<solstone_core_journal_io::archive_recovery::RecoveryPopulationFailure>),
}

impl From<io::Error> for RestoredMarkRefusal {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<BoundMutationRefusal> for RestoredMarkRefusal {
    fn from(error: BoundMutationRefusal) -> Self {
        Self::Bound(Box::new(error))
    }
}

#[cfg(any(unix, windows))]
enum RestoredAdmission<'a, 'clock> {
    Live(&'a solstone_core_backup::archive_readback::ReadOperation<'clock>),
    #[cfg(target_os = "linux")]
    Recovered {
        reservation: &'a solstone_core_journal_io::archive_recovery::RecoveredArchiveReservation,
        destination: &'a crate::restore_intent::DestinationObservation,
    },
}

#[cfg(any(unix, windows))]
impl RestoredAdmission<'_, '_> {
    fn root(&self) -> &std::sync::Arc<JournalRoot> {
        match self {
            Self::Live(read) => read.journal_root(),
            #[cfg(target_os = "linux")]
            Self::Recovered { reservation, .. } => reservation.root(),
        }
    }

    fn repository_id(&self) -> &str {
        match self {
            Self::Live(read) => read.repository_id(),
            #[cfg(target_os = "linux")]
            Self::Recovered { reservation, .. } => reservation.binding().repository_id(),
        }
    }

    fn revalidate(&self) -> Result<(), RestoredMarkRefusal> {
        match self {
            Self::Live(read) => read
                .revalidate_admission()
                .map_err(RestoredMarkRefusal::Readback),
            #[cfg(target_os = "linux")]
            Self::Recovered { reservation, .. } => reservation
                .revalidate_current_population()
                .map(|_census| ())
                .map_err(RestoredMarkRefusal::Recovery),
        }
    }

    fn check_destination(
        &self,
        parent: &solstone_core_journal_io::media::MediaDirectory,
        directory: &solstone_core_journal_io::media::MediaDirectory,
    ) -> Result<(), RestoredMarkRefusal> {
        #[cfg(not(target_os = "linux"))]
        let _directories = (parent, directory);
        match self {
            Self::Live(_) => Ok(()),
            #[cfg(target_os = "linux")]
            Self::Recovered {
                reservation,
                destination,
            } => {
                if destination.root != reservation.root().identity().observation_id()
                    || destination.parent != parent.identity()
                    || destination.segment != directory.identity()
                {
                    return Err(RestoredMarkRefusal::Changed);
                }
                Ok(())
            }
        }
    }
}

#[cfg(any(unix, windows))]
struct RestoredFacts<'a, 'clock> {
    admission: RestoredAdmission<'a, 'clock>,
    operation: &'a crate::archive_catalog::RestoreOperation,
    confirmed: &'a solstone_core_journal_io::media::MediaObservation,
    plan: &'a PlannedMarkMutation,
    claims: &'a solstone_core_segment::output_attempt::OutputClaims,
    commit: &'a solstone_core_segment::output_attempt::SegmentCommit<'a>,
}

// Keep the actual exclusion and descendant handles through a recovered postcheck.
#[cfg(any(unix, windows))]
struct RestoredMutationGuard {
    _directory: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    _parent: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    _owner: solstone_core_segment::RetainedOwnerDeletion,
    _health: HealthDirectory,
    _lock: BoundParentLock,
}

/// Retire only the deterministic remainder after exact durable restore evidence.
/// Generic typed mutations remain held. This does not retire Native resources.
pub fn conditional_retire_restored(
    input: RestoredMarkMutation<'_, '_>,
) -> Result<BoundMutationOutcome, RestoredMarkRefusal> {
    #[cfg(not(any(unix, windows)))]
    {
        let _input = input;
        Err(RestoredMarkRefusal::Unsupported)
    }
    #[cfg(any(unix, windows))]
    {
        let facts = RestoredFacts {
            admission: RestoredAdmission::Live(input.read),
            operation: input.operation,
            confirmed: input.confirmed,
            plan: input.plan,
            claims: input.claims,
            commit: input.commit,
        };
        mutate_restored(&facts).map(|(outcome, _guard)| outcome)
    }
}

#[cfg(any(unix, windows))]
fn mutate_restored(
    input: &RestoredFacts<'_, '_>,
) -> Result<(BoundMutationOutcome, RestoredMutationGuard), RestoredMarkRefusal> {
    use solstone_core_journal_io::media::MediaDirectory;
    use solstone_core_segment::RetainedOwnerDeletion;
    use std::sync::Arc;

    input
        .plan
        .validate()
        .map_err(|_| RestoredMarkRefusal::InvalidPlan)?;
    let expected = input.plan.expected();
    let offer = expected
        .offload
        .as_ref()
        .filter(|_| expected.class == crate::marks::RemovalClass::OffloadRawRelease)
        .ok_or(RestoredMarkRefusal::InvalidPlan)?;
    let restored = match input.operation.files.as_slice() {
        [file]
            if matches!(
                file.disposition,
                crate::archive_catalog::RestoreDisposition::Published
                    | crate::archive_catalog::RestoreDisposition::AlreadyPresent
            ) =>
        {
            file
        }
        _ => return Err(RestoredMarkRefusal::InvalidPlan),
    };
    let file = offer
        .proof
        .files
        .iter()
        .find(|file| file.name == restored.name && file.witness_id == restored.witness_id)
        .ok_or(RestoredMarkRefusal::InvalidPlan)?;
    let remaining = expected
        .proposal
        .names
        .iter()
        .filter(|name| **name != restored.name)
        .cloned()
        .collect::<Vec<_>>();
    if input.operation.operation_id.is_empty()
        || input.admission.repository_id() != offer.proof.repository_id
        || PlannedMarkMutation::remainder(expected, &remaining)
            .map_err(|_| RestoredMarkRefusal::InvalidPlan)?
            != *input.plan
    {
        return Err(RestoredMarkRefusal::InvalidPlan);
    }
    let root = input.admission.root();
    input.admission.revalidate()?;
    let target = &offer.proof.target;
    let relative = crate::layout::segment_rel_with_layout(
        &target.day,
        &target.stream,
        &target.dir,
        offer.proof.layout,
    );
    let segment = root.canonical_path().join(&relative);
    let directory = MediaDirectory::open(Arc::clone(root), Path::new(&relative))?;
    let parent = MediaDirectory::open(
        Arc::clone(root),
        Path::new(&crate::layout::stream_rel_with_layout(
            &target.day,
            &target.stream,
            offer.proof.layout,
        )),
    )?;
    let owner = RetainedOwnerDeletion::open(
        Arc::clone(&parent),
        OsStr::new(&target.dir),
        Some(&directory),
        65_536,
    )
    .map_err(RestoredMarkRefusal::Segment)?;
    revalidate_restored(input, file, &directory, &parent, &owner, &segment)?;
    let health = open_health(root, OsStr::new("health"), root.canonical_path())
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Read(error)))?
        .ok_or(BoundMutationRefusal::Review(ReviewRefusal::RootChanged))?;
    let (lock, mut register) = begin_bound_read(root, &health)?;
    let status = apply(&mut register, input.plan)
        .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
    let result = ConditionalResult { status, register };
    revalidate_restored(input, file, &directory, &parent, &owner, &segment)?;
    let outcome = if status == MutationStatus::Applied {
        publish_bound(root, &health, &lock, result).map_err(RestoredMarkRefusal::from)?
    } else {
        revalidate_health(root, &health).map_err(BoundMutationRefusal::from)?;
        lock.revalidate_bound(&health, OsStr::new("retention-marks.json.lock"))
            .map_err(BoundMutationRefusal::Lock)?;
        BoundMutationOutcome::Unchanged(result)
    };
    Ok((
        outcome,
        RestoredMutationGuard {
            _directory: directory,
            _parent: parent,
            _owner: owner,
            _health: health,
            _lock: lock,
        },
    ))
}

#[cfg(any(unix, windows))]
fn revalidate_restored(
    input: &RestoredFacts<'_, '_>,
    file: &crate::archive_proof::ArchiveFileProof,
    directory: &solstone_core_journal_io::media::MediaDirectory,
    parent: &solstone_core_journal_io::media::MediaDirectory,
    owner: &solstone_core_segment::RetainedOwnerDeletion,
    segment: &Path,
) -> Result<(), RestoredMarkRefusal> {
    input.admission.revalidate()?;
    input.admission.check_destination(parent, directory)?;
    directory.revalidate()?;
    if input.claims.segment_identity(segment)? != directory.object_identity()
        || owner.state().map_err(RestoredMarkRefusal::Segment)?
            != solstone_core_segment::OwnerDeletionState::Live
    {
        return Err(RestoredMarkRefusal::Changed);
    }
    let expected = input.plan.expected();
    input.commit.require_outputs(
        input.claims,
        segment,
        expected.proposal.names.iter().map(|name| {
            let mut output = segment.join(name);
            output.set_extension("jsonl");
            output
        }),
    )?;
    let observed = directory
        .observe_file(OsStr::new(&file.name))?
        .ok_or(RestoredMarkRefusal::Changed)?;
    if observed != *input.confirmed
        || observed.digest.size != file.bytes
        || observed.digest.sha256 != file.sha256
    {
        return Err(RestoredMarkRefusal::Changed);
    }
    let proof = &expected
        .offload
        .as_ref()
        .ok_or(RestoredMarkRefusal::InvalidPlan)?
        .proof;
    let catalog = crate::archive_catalog::read_complete_catalog(input.admission.root())
        .map_err(RestoredMarkRefusal::Catalog)?;
    check_restored_catalog(&catalog, proof, file, input.operation)?;
    directory.revalidate()?;
    if owner.state().map_err(RestoredMarkRefusal::Segment)?
        != solstone_core_segment::OwnerDeletionState::Live
    {
        return Err(RestoredMarkRefusal::Changed);
    }
    input.admission.check_destination(parent, directory)?;
    input.admission.revalidate()
}

#[cfg(any(unix, windows))]
fn check_restored_catalog(
    catalog: &crate::archive_catalog::JournalCatalog,
    proof: &crate::archive_proof::OffloadProof,
    file: &crate::archive_proof::ArchiveFileProof,
    operation: &crate::archive_catalog::RestoreOperation,
) -> Result<(), RestoredMarkRefusal> {
    let segments = || catalog.days.iter().flat_map(|day| &day.segments);
    if catalog.degraded()
        || segments().any(|segment| segment.degraded())
        || catalog.days.iter().any(|day| day.degraded())
        || segments()
            .flat_map(|segment| &segment.history)
            .filter(|manifest| manifest.manifest_id == proof.manifest_id)
            .count()
            != 1
        || segments()
            .flat_map(|segment| &segment.history)
            .flat_map(|manifest| &manifest.files)
            .filter(|witness| witness.witness_id == file.witness_id)
            .count()
            != 1
        || segments()
            .flat_map(|segment| &segment.restores)
            .filter(|recorded| recorded.operation_id == operation.operation_id)
            .count()
            != 1
        || catalog.historical_witness(proof, file).is_none()
    {
        return Err(RestoredMarkRefusal::Changed);
    }
    let key = solstone_core_format::segment::segment_key(&proof.target.dir)
        .ok_or(RestoredMarkRefusal::InvalidPlan)?;
    let found = segments().any(|segment| {
        segment.day == proof.target.day
            && segment.stream == proof.target.stream
            && segment.segment == key
            && segment
                .restores
                .iter()
                .any(|recorded| recorded == operation)
    });
    if !found {
        return Err(RestoredMarkRefusal::Changed);
    }
    Ok(())
}

/// One short phase under the existing register protocol. Never spans bookkeeping.
#[cfg(any(unix, windows))]
pub(crate) struct CheckedMarkGuard<'a, 'op> {
    checked: &'a CheckedReviewContext,
    lock: BoundParentLock,
    archive: Option<crate::archive_approval::ArchiveApprovalRef<'a, 'op>>,
}

#[cfg(any(unix, windows))]
impl<'a, 'op> CheckedMarkGuard<'a, 'op> {
    pub(crate) fn acquire(checked: &'a CheckedReviewContext) -> Result<Self, BoundMutationRefusal> {
        let (lock, register) = begin_bound_read(checked.root(), checked.health())?;
        check_marks(
            &register,
            checked.marks(),
            &checked.root().identity().observation_id(),
        )?;
        Ok(Self {
            checked,
            lock,
            archive: None,
        })
    }

    pub(crate) fn acquire_archive(
        checked: &'a CheckedReviewContext,
        archive: crate::archive_approval::ArchiveApprovalRef<'a, 'op>,
    ) -> Result<Self, BoundMutationRefusal> {
        Self::check_archive(checked, archive)?;
        let (lock, register) = begin_bound_read(checked.root(), checked.health())?;
        check_marks(
            &register,
            checked.marks(),
            &checked.root().identity().observation_id(),
        )?;
        Ok(Self {
            checked,
            lock,
            archive: Some(archive),
        })
    }

    fn check_archive(
        checked: &CheckedReviewContext,
        archive: crate::archive_approval::ArchiveApprovalRef<'_, '_>,
    ) -> Result<(), BoundMutationRefusal> {
        if !std::sync::Arc::ptr_eq(checked.root_arc(), archive.journal_root())
            || checked
                .marks()
                .iter()
                .filter(|m| m.offload.is_some())
                .ne(archive.expected_marks())
            || archive.revalidate_admission().is_err()
        {
            return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
                invalid("archive approval no longer matches the checked selection"),
            )));
        }
        Ok(())
    }

    pub(crate) fn revalidate(&self) -> Result<Register, BoundMutationRefusal> {
        let root = self.checked.root();
        let health = self.checked.health();
        revalidate_health(root, health)?;
        self.lock
            .revalidate_bound(health, OsStr::new("retention-marks.json.lock"))
            .map_err(BoundMutationRefusal::Lock)?;
        let register = read_at_health(health)?
            .ok_or(BoundMutationRefusal::Review(ReviewRefusal::RootChanged))?;
        if let Some(archive) = self.archive {
            Self::check_archive(self.checked, archive)?;
        }
        check_marks(
            &register,
            self.checked.marks(),
            &root.identity().observation_id(),
        )?;
        revalidate_health(root, health)?;
        self.lock
            .revalidate_bound(health, OsStr::new("retention-marks.json.lock"))
            .map_err(BoundMutationRefusal::Lock)?;
        Ok(register)
    }

    pub(crate) fn publish(
        self,
        plans: &[PlannedMarkMutation],
    ) -> Result<BoundMutationOutcome, BoundMutationRefusal> {
        let mut register = self.revalidate()?;
        let mut changed = false;
        for plan in plans {
            if !self.checked.marks().contains(plan.expected()) {
                return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
                    invalid("mutation is outside the checked selection"),
                )));
            }
            let status = apply(&mut register, plan)
                .map_err(|error| BoundMutationRefusal::Review(ReviewRefusal::Register(error)))?;
            if status != MutationStatus::Applied {
                return Err(BoundMutationRefusal::Review(ReviewRefusal::Register(
                    invalid("the selected mutation is no longer applicable"),
                )));
            }
            changed = true;
        }
        let result = ConditionalResult {
            status: if changed {
                MutationStatus::Applied
            } else {
                MutationStatus::AlreadyApplied
            },
            register,
        };
        if changed {
            publish_bound(
                self.checked.root(),
                self.checked.health(),
                &self.lock,
                result,
            )
        } else {
            Ok(BoundMutationOutcome::Unchanged(result))
        }
    }
}

/// Decline the entire checked selection in one retained-root transaction.
pub fn decline_reviewed(
    checked: CheckedReviewContext,
) -> Result<BoundMutationOutcome, BoundMutationRefusal> {
    let root = checked.root();
    let health = checked.health();
    let (lock, mut register) = begin_bound_read(root, health)?;
    check_marks(
        &register,
        checked.marks(),
        &root.identity().observation_id(),
    )?;
    for mark in checked.marks() {
        if register.marks.remove(&mark.id).is_none() {
            return Err(BoundMutationRefusal::Review(ReviewRefusal::Missing {
                id: mark.id.clone(),
            }));
        }
    }
    publish_bound(
        root,
        health,
        &lock,
        ConditionalResult {
            status: MutationStatus::Applied,
            register,
        },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::marks::{MarkId, Proposal, RemovalClass};
    use crate::receipt::Target;

    fn mark() -> Mark {
        let target = Target {
            day: "20261009".into(),
            stream: "camera".into(),
            dir: "120000_60_suffix".into(),
        };
        let proposal = Proposal {
            bytes: 3,
            reason: "archive".into(),
            names: vec!["a.flac".into(), "b.flac".into()],
        };
        Mark {
            id: MarkId::derive(RemovalClass::PolicyRawRelease, &target, &proposal.names),
            class: RemovalClass::PolicyRawRelease,
            target,
            marked_at: "2026-10-10T00:00:00Z".into(),
            proposal,
            state: MarkState::Marked,
            offload: None,
        }
    }

    fn bound() -> Mark {
        let mut mark = mark();
        mark.class = RemovalClass::OffloadRawRelease;
        mark.id = MarkId::derive(mark.class, &mark.target, &mark.proposal.names);
        let files = mark
            .proposal
            .names
            .iter()
            .enumerate()
            .map(|(index, name)| crate::archive_proof::ArchiveFileProof {
                witness_id: name.clone(),
                name: name.clone(),
                bytes: if index == 0 { 1 } else { 2 },
                sha256: "a".repeat(64),
                archive_path: format!("/stage/{name}"),
                processing: crate::archive_proof::AcceptedProcessingObservation {
                    record: None,
                    has_analysis_row: true,
                },
            })
            .collect();
        mark.offload = Some(
            OffloadOffer::new(crate::archive_proof::OffloadProof {
                journal_id: "native-root-v1:unix:0000000000000001:0000000000000002".into(),
                manifest_id: "manifest".into(),
                target: mark.target.clone(),
                layout: solstone_core_journal_io::paths::SegmentLayout::Named,
                repository_id: "b".repeat(64),
                snapshot_id: "c".repeat(64),
                files,
            })
            .unwrap(),
        );
        mark
    }

    #[cfg(unix)]
    fn restored_catalog_fixture() -> (
        crate::archive_catalog::JournalCatalog,
        crate::archive_proof::OffloadProof,
        crate::archive_catalog::RestoreOperation,
    ) {
        use crate::archive_catalog::{
            ArchiveManifest, ArchiveWitness, DayCatalog, JournalCatalog, OffloadFile,
            RestoreDisposition, RestoreFileV2, RestoreOperation, SegmentCatalog,
        };
        let proof = bound().offload.unwrap().proof;
        let file = &proof.files[0];
        let witness = ArchiveWitness {
            manifest_id: proof.manifest_id.clone(),
            witness_id: file.witness_id.clone(),
            snapshot_id: proof.snapshot_id.clone(),
            repository_id: Some(proof.repository_id.clone()),
            archive_path: Some(file.archive_path.clone()),
            journal_id: Some(proof.journal_id.clone()),
            layout: Some(proof.layout),
            segment_dir: Some(proof.target.dir.clone()),
            processing: Some(file.processing.clone()),
            file: OffloadFile {
                name: file.name.clone(),
                bytes: file.bytes,
                sha256: file.sha256.clone(),
            },
        };
        let operation = RestoreOperation {
            operation_id: "restore".into(),
            files: vec![RestoreFileV2 {
                witness_id: file.witness_id.clone(),
                name: file.name.clone(),
                disposition: RestoreDisposition::AlreadyPresent,
            }],
        };
        let catalog = JournalCatalog {
            days: vec![DayCatalog {
                day: proof.target.day.clone(),
                segments: vec![SegmentCatalog {
                    day: proof.target.day.clone(),
                    stream: proof.target.stream.clone(),
                    segment: solstone_core_format::segment::segment_key(&proof.target.dir).unwrap(),
                    history: vec![ArchiveManifest {
                        manifest_id: proof.manifest_id.clone(),
                        files: vec![witness],
                    }],
                    restores: vec![operation.clone()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        (catalog, proof, operation)
    }

    #[cfg(unix)]
    #[test]
    fn restored_catalog_requires_exact_history_and_successful_record() {
        let (catalog, proof, operation) = restored_catalog_fixture();
        let file = &proof.files[0];
        assert!(catalog.active_witness(&proof, file).is_none());
        assert!(catalog.historical_witness(&proof, file).is_some());
        assert!(check_restored_catalog(&catalog, &proof, file, &operation).is_ok());
        for cut in 0..5 {
            let mut changed = catalog.clone();
            match cut {
                0 => changed.days[0].segments[0].history.clear(),
                1 => changed.days[0].segments[0].restores.clear(),
                2 => {
                    changed.days[0].segments[0].restores[0].files[0].disposition =
                        crate::archive_catalog::RestoreDisposition::Conflict
                }
                3 => {
                    changed.days[0].segments[0].history[0].files[0].archive_path =
                        Some("/different/a.flac".into())
                }
                _ => changed.days[0].segments[0].history[0].files[0].processing = None,
            }
            assert!(check_restored_catalog(&changed, &proof, file, &operation).is_err());
        }
        // A newer active generation does not replace the exact historical record.
        let mut newer = catalog.clone();
        let mut witness = newer.days[0].segments[0].history[0].files[0].clone();
        witness.manifest_id = "newer".into();
        witness.witness_id = "newer-file".into();
        newer.days[0].segments[0].active.push(witness);
        assert!(check_restored_catalog(&newer, &proof, file, &operation).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn restored_catalog_refuses_cross_day_manifest_witness_and_operation_collisions() {
        let (catalog, proof, operation) = restored_catalog_fixture();
        for cut in 0..3 {
            let mut changed = catalog.clone();
            let mut other_day = catalog.days[0].clone();
            other_day.day = "20261008".into();
            other_day.segments[0].day.clone_from(&other_day.day);
            match cut {
                0 => {
                    other_day.segments[0].restores.clear();
                    other_day.segments[0].history[0].files[0].witness_id = "other-witness".into();
                }
                1 => {
                    other_day.segments[0].history[0].manifest_id = "other-manifest".into();
                    other_day.segments[0].restores.clear();
                }
                _ => other_day.segments[0].history.clear(),
            }
            changed.days.push(other_day);
            assert!(!changed.degraded());
            assert!(check_restored_catalog(&changed, &proof, &proof.files[0], &operation).is_err());
        }
    }

    #[test]
    fn changed_full_marks_survive_old_completion_and_failure() {
        let journal = tempfile::tempdir().unwrap();
        let expected = mark();
        let path = register_path(journal.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut current = expected.clone();
        current.marked_at = "2026-10-10T00:00:01Z".into();
        let mut register = Register::empty();
        register.marks.insert(current.id.clone(), current.clone());
        let bytes = serde_json::to_vec(&register).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        for plan in [
            PlannedMarkMutation::resolve(&expected).unwrap(),
            PlannedMarkMutation::failure(
                &expected,
                Failure {
                    at: "now".into(),
                    reason: "failure".into(),
                    staged: None,
                },
            )
            .unwrap(),
        ] {
            assert_eq!(
                conditional_mutate(journal.path(), &plan).unwrap().status,
                MutationStatus::Stale
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let failure = PlannedMarkMutation::failure(
            &current,
            Failure {
                at: "now".into(),
                reason: "failure".into(),
                staged: None,
            },
        )
        .unwrap();
        assert_eq!(
            conditional_mutate(journal.path(), &failure).unwrap().status,
            MutationStatus::Applied
        );
        assert_eq!(
            conditional_mutate(journal.path(), &failure).unwrap().status,
            MutationStatus::AlreadyApplied
        );
        assert_eq!(
            conditional_mutate(
                journal.path(),
                &PlannedMarkMutation::resolve(&current).unwrap()
            )
            .unwrap()
            .status,
            MutationStatus::Stale
        );
        let failed = failure.replacement().unwrap();
        assert_eq!(
            conditional_mutate(
                journal.path(),
                &PlannedMarkMutation::resolve(failed).unwrap()
            )
            .unwrap()
            .status,
            MutationStatus::Applied
        );
        assert_eq!(
            conditional_mutate(
                journal.path(),
                &PlannedMarkMutation::resolve(&current).unwrap()
            )
            .unwrap()
            .status,
            MutationStatus::Missing
        );
    }

    #[test]
    fn exact_remainder_is_durable_idempotent_and_collision_safe() {
        let expected = bound();
        let plan = PlannedMarkMutation::remainder(&expected, &["b.flac".into()]).unwrap();
        let plan: PlannedMarkMutation =
            serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
        let replacement = plan.replacement().unwrap();
        assert_eq!(replacement.proposal.bytes, 2);
        assert_eq!(
            replacement.offload.as_ref().unwrap().nonce,
            expected.offload.as_ref().unwrap().nonce
        );
        assert_ne!(
            replacement.offload.as_ref().unwrap().revision,
            expected.offload.as_ref().unwrap().revision
        );
        assert!(PlannedMarkMutation::remainder(&expected, &["z.flac".into()]).is_err());
        let mut register = Register::empty();
        register.version = 2;
        register.marks.insert(expected.id.clone(), expected.clone());
        register
            .marks
            .insert(replacement.id.clone(), replacement.clone());
        let before = register.clone();
        assert_eq!(
            apply(&mut register, &plan).unwrap(),
            MutationStatus::Collision
        );
        assert_eq!(register, before);
        register.marks.remove(&replacement.id);
        assert_eq!(
            apply(&mut register, &plan).unwrap(),
            MutationStatus::Applied
        );
        assert_eq!(
            apply(&mut register, &plan).unwrap(),
            MutationStatus::AlreadyApplied
        );
        register.marks.insert(expected.id.clone(), expected.clone());
        register
            .marks
            .get_mut(&expected.id)
            .unwrap()
            .proposal
            .reason = "newer".into();
        assert_eq!(apply(&mut register, &plan).unwrap(), MutationStatus::Stale);
    }

    #[test]
    fn bound_register_is_strict_and_all_legacy_writers_refuse() {
        use crate::marks::*;
        let journal = tempfile::tempdir().unwrap();
        let mark = bound();
        let mut register = Register::empty();
        register.version = 2;
        register.marks.insert(mark.id.clone(), mark.clone());
        let path = register_path(journal.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = serde_json::to_vec(&register).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(load(journal.path()).unwrap(), register);
        assert!(matches!(
            preflight(journal.path(), std::slice::from_ref(&mark.id)),
            Err(PreflightRefusal::ArchiveBound { .. })
        ));
        assert!(matches!(
            decline(journal.path(), &mark.id),
            Err(PreflightRefusal::Register(StoreError::WriterHeld))
        ));
        let now = jiff::Timestamp::now();
        assert!(matches!(
            upsert(
                journal.path(),
                mark.class,
                &mark.target,
                mark.proposal.clone(),
                now
            ),
            Err(StoreError::WriterHeld)
        ));
        assert!(matches!(
            reconcile(journal.path(), mark.class, &[], now),
            Err(StoreError::WriterHeld)
        ));
        assert!(matches!(
            resolve(journal.path(), &mark.id),
            Err(StoreError::WriterHeld)
        ));
        assert!(matches!(
            reconcile_recovered(journal.path()),
            Err(StoreError::WriterHeld)
        ));
        assert!(matches!(
            record_failure(
                journal.path(),
                mark.class,
                &mark.target,
                &mark.proposal.names,
                Failure {
                    at: String::new(),
                    reason: "failure".into(),
                    staged: None
                },
                now
            ),
            Err(StoreError::WriterHeld)
        ));
        assert!(matches!(
            conditional_mutate(
                journal.path(),
                &PlannedMarkMutation::resolve(&mark).unwrap()
            ),
            Err(StoreError::WriterHeld)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let mut invalid = register.clone();
        invalid.version = 1;
        assert!(validate(&invalid).is_err());
        let mut invalid = register.clone();
        invalid.marks.get_mut(&mark.id).unwrap().offload = None;
        assert!(validate(&invalid).is_err());
        let mut invalid = register.clone();
        invalid.marks.get_mut(&mark.id).unwrap().proposal.bytes = 999;
        assert!(validate(&invalid).is_err());
        let mut invalid = serde_json::to_value(&register).unwrap();
        invalid["marks"][mark.id.as_str()]["offload"]["proof"]["target"]["unknown"] = true.into();
        assert!(serde_json::from_value::<Register>(invalid).is_err());
        let mut invalid = serde_json::to_value(
            PlannedMarkMutation::remainder(&mark, &["b.flac".into()]).unwrap(),
        )
        .unwrap();
        invalid["replacement"]["proposal"]["reason"] = "changed".into();
        assert!(
            serde_json::from_value::<PlannedMarkMutation>(invalid)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
