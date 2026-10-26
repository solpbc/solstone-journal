// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Private retained restore execution. Every failure keeps its original owner;
//! saved observations never reconstruct a stage or authorize cleanup.

use std::ffi::OsStr;
use std::sync::Arc;

use solstone_core_backup::archive_readback::{
    ExactArchiveFile, OwnedReadOperation, ReadOperation, ReadbackFailure,
};
#[cfg(all(test, feature = "full-tests"))]
use solstone_core_journal_io::activity_obligations::observe_with_carrier;
use solstone_core_journal_io::media::{
    Cleanup, Confirmation, Durability, MediaObservation, MediaStage, OwnedMediaFailure,
    OwnedMediaTarget, PreparedMedia, PublicationEffect, PublicationReceipt, RootActivityCarrier,
};
use solstone_core_journal_io::{JournalRoot, LockOptions};
use solstone_core_retention::archive_catalog::{
    ArchiveWitness, LegacyRestoreBinding, RestoreDisposition, RestoreOperation, SegmentCatalog,
    read_complete_catalog,
};
#[cfg(all(test, feature = "full-tests", any(unix, windows)))]
use solstone_core_retention::mark_mutation::conditional_mutate_at_root;
use solstone_core_retention::mark_mutation::{
    BoundMutationOutcome, MutationStatus, RestoredMarkMutation, conditional_retire_restored,
};
use solstone_core_retention::{Mark, Register, RemovalClass, review_context};
use solstone_core_segment::output_attempt::{OutputClaims, SegmentCommit};

use super::{RestoreIntentStore, StoreAdmissionFailure, StoreProblem, StoreWriteReceipt};
use crate::ledger::writer::{
    LedgerConfirmation, LedgerSync, LedgerWriteReceipt, LedgerWriteRefusal,
    append_legacy_restore_operation, append_restore_operation,
};
use crate::restore::owned::RestoreRegisterRefusal;
use crate::restore::owned::read::RestoreRead;
use crate::restore_prepared::PreparedRestore;
use solstone_core_backup_runtime::engine::QualifiedArchiveRecoveryBinding;
use solstone_core_retention::restore_intent::{
    AttemptEffects, CurrentContent, Obligation, PublicationDecision, RESTORE_DOWNLOAD_LEAF,
    RecoveryDecision, RegisterDecision, RepairHold, RestoreIntent, StageEvidence,
    content_observation, inspect_recovery, selected_name,
};

#[derive(Debug)]
pub(super) enum Hold {
    Admission(String),
    Repair(RepairHold),
    Obligations,
    Store(StoreProblem),
    StorePublication,
    Marker,
    Ledger,
    Register,
    Retirement,
}

mod observation;
use observation::observe_restore_obligations;

enum Admission<'operation, 'clock> {
    Readback(&'operation ReadOperation<'clock>),
    Recovery(
        &'operation OwnedReadOperation,
        &'operation QualifiedArchiveRecoveryBinding,
    ),
    #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
    LocalTest(&'operation std::cell::Cell<bool>),
}

impl<'operation, 'clock> Admission<'operation, 'clock> {
    fn for_read(
        operation: &'operation impl RestoreRead<'clock>,
        binding: Option<&'operation QualifiedArchiveRecoveryBinding>,
    ) -> Self {
        match (binding, operation.as_owned()) {
            (Some(binding), Some(operation)) => Self::Recovery(operation, binding),
            _ => Self::Readback(operation.as_read()),
        }
    }

    fn operation(&self) -> Option<&ReadOperation<'_>> {
        match self {
            Self::Readback(operation) => Some(operation),
            Self::Recovery(operation, _) => Some(operation.as_read()),
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            Self::LocalTest(_) => None,
        }
    }

    fn attach(&self, intent: &mut RestoreIntent) -> Result<(), Hold> {
        if let Self::Recovery(operation, binding) = self {
            binding
                .revalidate_for_read(operation)
                .map_err(|error| Hold::Admission(error.to_string()))?;
            // Only this fresh, unpersisted initial value reaches attachment.
            if intent.recovery_binding.is_some()
                || intent.outcome.is_some()
                || intent.confirmed.is_some()
                || intent.marker != Obligation::Pending
                || intent.event_recorded != Obligation::Pending
                || intent.cleanup != Obligation::Pending
                || intent.event.files.len() != 1
                || !intent
                    .event
                    .files
                    .first()
                    .is_some_and(|file| file.disposition == RestoreDisposition::Unattempted)
            {
                return Err(Hold::Repair(RepairHold::InvalidIntent));
            }
            intent.recovery_binding = Some(binding.data().clone());
            intent.validate().map_err(Hold::Repair)?;
        }
        Ok(())
    }

    fn check(&self) -> Result<(), Hold> {
        match self {
            Self::Recovery(operation, binding) => {
                binding
                    .revalidate_for_read(operation)
                    .map_err(|error| Hold::Admission(error.to_string()))?;
                operation
                    .as_read()
                    .revalidate_admission()
                    .map_err(|error| Hold::Admission(error.to_string()))
            }
            Self::Readback(operation) => operation
                .revalidate_admission()
                .map_err(|error| Hold::Admission(error.to_string())),
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            Self::LocalTest(allowed) if allowed.get() => Ok(()),
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            Self::LocalTest(_) => Err(Hold::Admission("test admission stopped".into())),
        }
    }
}

struct Owner<'operation, 'clock> {
    carrier: Arc<RootActivityCarrier>,
    destination: PreparedRestore,
    admission: Admission<'operation, 'clock>,
    media: Option<PreparedMedia>,
}

impl Owner<'_, '_> {
    fn root(&self) -> &Arc<JournalRoot> {
        self.carrier.journal_root()
    }

    fn native_check(&self) -> Result<(), Hold> {
        if !Arc::ptr_eq(self.root(), self.destination.journal_root()) {
            return Err(Hold::Repair(RepairHold::DestinationChanged));
        }
        if let Some(operation) = self.admission.operation()
            && (!Arc::ptr_eq(operation.journal_root(), self.root())
                || !Arc::ptr_eq(operation.carrier(), &self.carrier))
        {
            return Err(Hold::Repair(RepairHold::DestinationChanged));
        }
        self.carrier
            .revalidate()
            .map_err(|error| Hold::Admission(error.to_string()))?;
        self.destination
            .revalidate()
            .map_err(|error| Hold::Repair(error.reason))
    }

    fn effect_check(&self, commit: &SegmentCommit<'_>) -> Result<(), Hold> {
        self.native_check()?;
        commit
            .revalidate()
            .map_err(|error| Hold::Admission(error.to_string()))?;
        self.admission.check()
    }
}

enum StagingEvidence {
    None,
    Stage(OwnedMediaFailure<Option<MediaStage>>),
    Target(OwnedMediaFailure<MediaStage>),
    Unconsumed(MediaStage),
    Readback {
        failure: ReadbackFailure,
        cleanup: Cleanup,
    },
    Prepared(PreparedMedia),
    ReadyTarget(OwnedMediaTarget),
}

#[must_use]
pub(crate) struct StagingFailure {
    carrier: Arc<RootActivityCarrier>,
    destination: PreparedRestore,
    evidence: StagingEvidence,
    hold: Hold,
}

/// Minted only after an actual retained stage and exact neutral readback.
pub(crate) struct StagedRestore<'operation, 'clock> {
    owner: Owner<'operation, 'clock>,
    witness: ArchiveWitness,
    stage: StageEvidence,
    legacy_binding: Option<LegacyRestoreBinding>,
}

fn catalog(
    owner: &Owner<'_, '_>,
    legacy_binding: Option<&LegacyRestoreBinding>,
    operation: Option<&RestoreOperation>,
) -> Result<SegmentCatalog, Hold> {
    owner.native_check()?;
    let catalog = read_complete_catalog(owner.root())
        .map_err(|_| Hold::Repair(RepairHold::DegradedCatalog))?;
    if catalog.degraded() {
        return Err(Hold::Repair(RepairHold::DegradedCatalog));
    }
    let locator = owner.destination.locator();
    if let Some(binding) = legacy_binding {
        binding
            .validate()
            .map_err(|_| Hold::Repair(RepairHold::InvalidIntent))?;
        if binding.target != locator.target
            || binding.layout != locator.layout
            || catalog.restore_binding(&binding.adoption_id) != Some(binding)
        {
            return Err(Hold::Repair(RepairHold::CatalogChanged));
        }
        let mut manifests = catalog
            .days
            .iter()
            .flat_map(|day| &day.segments)
            .flat_map(|segment| &segment.history)
            .filter(|manifest| manifest.manifest_id == binding.legacy_manifest.manifest_id);
        if manifests.next() != Some(&binding.legacy_manifest) || manifests.next().is_some() {
            return Err(Hold::Repair(RepairHold::CatalogChanged));
        }
        for original in &binding.legacy_manifest.files {
            let mut witnesses = catalog
                .days
                .iter()
                .flat_map(|day| &day.segments)
                .flat_map(|segment| &segment.history)
                .flat_map(|manifest| &manifest.files)
                .filter(|witness| witness.witness_id == original.witness_id);
            if witnesses.next() != Some(original) || witnesses.next().is_some() {
                return Err(Hold::Repair(RepairHold::CatalogChanged));
            }
        }
        if let Some(expected) = operation {
            let mut operations = catalog
                .days
                .iter()
                .flat_map(|day| &day.segments)
                .flat_map(|segment| segment.restores.iter().map(move |event| (segment, event)))
                .filter(|(_, event)| event.operation_id == expected.operation_id);
            if operations.next().is_some_and(|(segment, event)| {
                event != expected
                    || segment.day != locator.target.day
                    || segment.stream != locator.target.stream
                    || segment.segment != locator.segment
            }) || operations.next().is_some()
            {
                return Err(Hold::Repair(RepairHold::OperationCollision));
            }
        }
    }
    catalog
        .days
        .into_iter()
        .flat_map(|day| day.segments)
        .find(|segment| {
            segment.day == locator.target.day
                && segment.stream == locator.target.stream
                && segment.segment == locator.segment
        })
        .ok_or(Hold::Repair(RepairHold::CatalogChanged))
}

fn current_mark<'a>(
    owner: &Owner<'_, '_>,
    witness: &ArchiveWitness,
    register: &'a Register,
) -> Result<Option<&'a Mark>, Hold> {
    related_mark(
        &owner.destination.locator().target,
        &witness.file.name,
        register,
    )
    .map_err(Hold::Repair)
}

pub(super) fn related_mark<'a>(
    target: &solstone_core_retention::Target,
    name: &str,
    register: &'a Register,
) -> Result<Option<&'a Mark>, RepairHold> {
    let mut matches = register.marks.values().filter(|mark| {
        mark.class == RemovalClass::OffloadRawRelease
            && &mark.target == target
            && mark.proposal.names.iter().any(|current| current == name)
    });
    let selected = matches.next();
    if matches.next().is_some() {
        return Err(RepairHold::MarkChanged);
    }
    Ok(selected)
}

fn current_content(owner: &Owner<'_, '_>, intent: &RestoreIntent) -> Result<CurrentContent, Hold> {
    use solstone_core_journal_io::journal_root::JournalEntryKind;
    let live = owner.destination.live();
    let before = live
        .entries(131_072)
        .map_err(|_| Hold::Repair(RepairHold::UncertainContent))?
        .ok_or(Hold::Repair(RepairHold::UncertainContent))?
        .into_iter()
        .find(|entry| entry.name == selected_name(intent));
    let content = match &before {
        Some(entry) if entry.kind != JournalEntryKind::RegularFile => CurrentContent::Conflict,
        _ => {
            let observation = live.observe_file(selected_name(intent));
            if let Ok(Some(observed)) = &observation
                && before.as_ref().map(|entry| entry.identity) != Some(observed.identity)
            {
                return Err(Hold::Repair(RepairHold::UncertainContent));
            }
            content_observation(intent, observation)
        }
    };
    let after = live
        .entries(131_072)
        .map_err(|_| Hold::Repair(RepairHold::UncertainContent))?
        .ok_or(Hold::Repair(RepairHold::UncertainContent))?
        .into_iter()
        .find(|entry| entry.name == selected_name(intent));
    owner.native_check()?;
    if before != after {
        return Err(Hold::Repair(RepairHold::UncertainContent));
    }
    Ok(content)
}

impl<'operation, 'clock> StagedRestore<'operation, 'clock> {
    pub(crate) fn read(
        destination: PreparedRestore,
        operation: &'operation mut impl RestoreRead<'clock>,
        witness: ArchiveWitness,
        required_capacity: u64,
    ) -> Result<Self, Box<StagingFailure>> {
        Self::read_source(
            destination,
            operation,
            witness,
            required_capacity,
            None,
            None,
        )
    }

    pub(crate) fn read_with_recovery(
        destination: PreparedRestore,
        operation: &'operation mut impl RestoreRead<'clock>,
        witness: ArchiveWitness,
        required_capacity: u64,
        recovery: Option<&'operation QualifiedArchiveRecoveryBinding>,
    ) -> Result<Self, Box<StagingFailure>> {
        if recovery.is_none() {
            return Self::read(destination, operation, witness, required_capacity);
        }
        Self::read_source(
            destination,
            operation,
            witness,
            required_capacity,
            None,
            recovery,
        )
    }

    pub(crate) fn read_legacy(
        destination: PreparedRestore,
        operation: &'operation mut impl RestoreRead<'clock>,
        witness: ArchiveWitness,
        required_capacity: u64,
        binding: LegacyRestoreBinding,
        recovery: Option<&'operation QualifiedArchiveRecoveryBinding>,
    ) -> Result<Self, Box<StagingFailure>> {
        Self::read_source(
            destination,
            operation,
            witness,
            required_capacity,
            Some(binding),
            recovery,
        )
    }

    fn read_source(
        destination: PreparedRestore,
        operation: &'operation mut impl RestoreRead<'clock>,
        witness: ArchiveWitness,
        required_capacity: u64,
        legacy_binding: Option<LegacyRestoreBinding>,
        recovery: Option<&'operation QualifiedArchiveRecoveryBinding>,
    ) -> Result<Self, Box<StagingFailure>> {
        let carrier = Arc::clone(operation.as_read().carrier());
        let owner = Owner {
            carrier,
            destination,
            admission: Admission::for_read(operation, recovery),
            media: None,
        };
        let preflight = (|| {
            if recovery.is_some() && operation.as_owned().is_none() {
                return Err(Hold::Repair(RepairHold::UnboundWitness));
            }
            if !Arc::ptr_eq(operation.as_read().journal_root(), owner.root()) {
                return Err(Hold::Repair(RepairHold::UnboundWitness));
            }
            if let Some(binding) = &legacy_binding {
                if binding.repository_id != operation.as_read().repository_id()
                    || !binding.legacy_manifest.files.contains(&witness)
                {
                    return Err(Hold::Repair(RepairHold::UnboundWitness));
                }
            } else if witness.repository_id.as_deref() != Some(operation.as_read().repository_id())
                || witness.layout != Some(owner.destination.locator().layout)
                || witness.segment_dir.as_ref() != Some(&owner.destination.locator().target.dir)
            {
                return Err(Hold::Repair(RepairHold::UnboundWitness));
            }
            owner.native_check()?;
            owner.admission.check()?;
            let selection = catalog(&owner, legacy_binding.as_ref(), None)?;
            if !selection.active.contains(&witness) {
                return Err(Hold::Repair(RepairHold::CatalogChanged));
            }
            let coverage = observe_restore_obligations(&owner)?;
            if coverage.unresolved_entries().next().is_some() {
                return Err(Hold::Obligations);
            }
            if let Some(binding) = &legacy_binding {
                let file = binding
                    .files
                    .iter()
                    .find(|file| file.witness_id == witness.witness_id)
                    .ok_or(Hold::Repair(RepairHold::UnboundWitness))?;
                Ok(ExactArchiveFile {
                    repository_id: binding.repository_id.clone(),
                    snapshot_id: binding.snapshot_id.clone(),
                    name: file.name.clone(),
                    archive_path: file.archive_path.clone(),
                    bytes: file.bytes,
                    sha256: file.sha256.clone(),
                })
            } else {
                Ok(ExactArchiveFile {
                    repository_id: witness.repository_id.clone().unwrap_or_default(),
                    snapshot_id: witness.snapshot_id.clone(),
                    name: witness.file.name.clone(),
                    archive_path: witness.archive_path.clone().unwrap_or_default(),
                    bytes: witness.file.bytes,
                    sha256: witness.file.sha256.clone(),
                })
            }
        })();
        let fail = |owner: Owner<'_, '_>, hold, evidence| {
            Box::new(StagingFailure {
                carrier: owner.carrier,
                destination: owner.destination,
                evidence,
                hold,
            })
        };
        let exact = match preflight {
            Ok(exact) => exact,
            Err(hold) => return Err(fail(owner, hold, StagingEvidence::None)),
        };
        let stage = match MediaStage::create(
            owner.destination.stage_parent(),
            Arc::clone(owner.carrier.activity()),
        ) {
            Ok(stage) => stage,
            Err(error) => {
                return Err(fail(
                    owner,
                    Hold::Admission(error.error.to_string()),
                    StagingEvidence::Stage(error),
                ));
            }
        };
        if let Err(hold) = capacity_allows(stage.available_bytes(), required_capacity)
            .and_then(|()| owner.native_check())
            .and_then(|()| owner.admission.check())
        {
            return Err(fail(owner, hold, StagingEvidence::Unconsumed(stage)));
        }
        let observation = match stage.observation() {
            Ok(observation) => observation,
            Err(error) => {
                return Err(fail(
                    owner,
                    Hold::Admission(error.to_string()),
                    StagingEvidence::Unconsumed(stage),
                ));
            }
        };
        let target = match stage.create_named_target(OsStr::new(RESTORE_DOWNLOAD_LEAF)) {
            Ok(target) => target,
            Err(error) => {
                return Err(fail(
                    owner,
                    Hold::Admission(error.error.to_string()),
                    StagingEvidence::Target(error),
                ));
            }
        };
        if let Err(hold) = owner.native_check().and_then(|()| owner.admission.check()) {
            return Err(fail(owner, hold, StagingEvidence::ReadyTarget(target)));
        }
        // Release the immutable admission borrow only while the same operation
        // performs its owned dump; retain it again through every local effect.
        let Owner {
            carrier,
            destination,
            ..
        } = owner;
        let media = match operation.read_exact(&exact, target) {
            Ok(media) => media,
            Err(mut failure) => {
                // Nothing was published from this unpublished copy. Its cleanup
                // refuses unless the writer is known to have been reaped.
                let cleanup = failure.cleanup();
                return Err(Box::new(StagingFailure {
                    carrier,
                    destination,
                    hold: Hold::Admission(failure.to_string()),
                    evidence: StagingEvidence::Readback { failure, cleanup },
                }));
            }
        };
        let stage = StageEvidence {
            parent: observation.parent_identity,
            directory: observation.directory_identity,
            leaf: media.identity(),
            stage_name: observation.directory_name,
            leaf_name: RESTORE_DOWNLOAD_LEAF.into(),
            boot_id: None,
        };
        let owner = Owner {
            carrier,
            destination,
            admission: Admission::for_read(operation, recovery),
            media: Some(media),
        };
        if let Err(hold) = owner.native_check().and_then(|()| owner.admission.check()) {
            return Err(Box::new(StagingFailure {
                carrier: owner.carrier,
                destination: owner.destination,
                evidence: owner
                    .media
                    .map_or(StagingEvidence::None, StagingEvidence::Prepared),
                hold,
            }));
        }
        Ok(Self {
            owner,
            witness,
            stage,
            legacy_binding,
        })
    }
}

struct Record {
    store: Option<RestoreIntentStore>,
    intent: RestoreIntent,
    persisted: Option<RestoreIntent>,
    writes: Vec<StoreWriteReceipt>,
}

struct NoEffectCleanup {
    cleanup: Cleanup,
    parent_sync: Option<std::io::Result<()>>,
}

impl NoEffectCleanup {
    fn confirmed(&self) -> bool {
        self.cleanup == Cleanup::Complete
            && self.parent_sync.as_ref().is_some_and(|result| {
                if cfg!(windows) {
                    result.is_ok()
                        || result
                            .as_ref()
                            .err()
                            .is_some_and(|err| err.kind() == std::io::ErrorKind::Unsupported)
                } else {
                    result.is_ok()
                }
            })
    }
}

impl Record {
    fn persist(&mut self) -> Result<(), Hold> {
        let store = self
            .store
            .take()
            .ok_or(Hold::Store(StoreProblem::Changed))?;
        let written = if let Some(expected) = &self.persisted {
            let observed = match store.read(&expected.event.operation_id) {
                Ok(Some(observed)) if observed.intent == *expected => observed,
                result => {
                    self.store = Some(store);
                    return Err(match result {
                        Err(error) => Hold::Store(error),
                        _ => Hold::Store(StoreProblem::Changed),
                    });
                }
            };
            store.update(&observed, self.intent.clone())
        } else {
            store.create_intent(self.intent.clone())
        };
        #[cfg(any(unix, windows))]
        let confirmed = written.receipt.fully_confirmed();
        #[cfg(not(any(unix, windows)))]
        let confirmed = false;
        self.store = Some(written.owner);
        self.writes.push(written.receipt);
        if !confirmed {
            return Err(Hold::StorePublication);
        }
        self.persisted = Some(self.intent.clone());
        Ok(())
    }
}

#[must_use = "retain native effects, receipts, owner and unresolved durable intent"]
pub(crate) struct TransactionOutcome<'operation, 'clock> {
    owner: Owner<'operation, 'clock>,
    record: Option<Record>,
    store_failure: Option<StoreAdmissionFailure>,
    publication: Option<PublicationReceipt>,
    #[cfg(unix)]
    marker: Option<
        Result<
            solstone_core_journal_io::health_marker::BoundStreamMarkerPublication,
            solstone_core_journal_io::health_marker::BoundStreamMarkerError,
        >,
    >,
    #[cfg(unix)]
    marker_ancestor_sync: Vec<std::io::Result<()>>,
    #[cfg(windows)]
    marker_published: bool,
    ledger: Option<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    register: Option<Result<BoundMutationOutcome, RestoreRegisterRefusal>>,
    no_effect_cleanup: Option<NoEffectCleanup>,
    retirement: Option<retirement::RetirementFacts>,
    effects: AttemptEffects,
    hold: Option<Hold>,
}

impl<'operation, 'clock> TransactionOutcome<'operation, 'clock> {
    fn new(owner: Owner<'operation, 'clock>) -> Self {
        Self {
            owner,
            record: None,
            store_failure: None,
            publication: None,
            #[cfg(unix)]
            marker: None,
            #[cfg(unix)]
            marker_ancestor_sync: Vec::new(),
            #[cfg(windows)]
            marker_published: false,
            ledger: None,
            register: None,
            no_effect_cleanup: None,
            retirement: None,
            effects: AttemptEffects::default(),
            hold: None,
        }
    }

    fn run(mut self, execute: impl FnOnce(&mut Self) -> Result<(), Hold>) -> Self {
        if let Err(hold) = execute(&mut self) {
            self.hold = Some(hold);
        }
        self
    }

    fn record(&self) -> Result<&Record, Hold> {
        self.record
            .as_ref()
            .ok_or(Hold::Store(StoreProblem::Changed))
    }

    fn record_mut(&mut self) -> Result<&mut Record, Hold> {
        self.record
            .as_mut()
            .ok_or(Hold::Store(StoreProblem::Changed))
    }

    fn persist(&mut self) -> Result<(), Hold> {
        // Once an effect happened, cancellation/config drift cannot erase its
        // carrier. The store independently enforces native root/lock/name binding.
        self.record_mut()?.persist()
    }

    fn inspect(&self) -> Result<RecoveryDecision, Hold> {
        let intent = &self.record()?.intent;
        let current = catalog(
            &self.owner,
            intent.legacy_binding.as_ref(),
            Some(&intent.event),
        )?;
        let marks = review_context::load_at_root(self.owner.root())
            .map_err(|error| Hold::Admission(format!("{error:?}")))?;
        let content = current_content(&self.owner, intent)?;
        inspect_recovery(
            intent,
            self.owner.destination.destination(),
            &current,
            &marks,
            content,
        )
        .map_err(Hold::Repair)
    }

    fn effect_check(&self, commit: &SegmentCommit<'_>) -> Result<(), Hold> {
        self.owner.effect_check(commit)?;
        if self.record()?.intent.legacy_binding.is_some() {
            self.unchanged_mark()?;
            self.inspect()?;
        }
        Ok(())
    }
}

impl<'operation, 'clock> StagedRestore<'operation, 'clock> {
    pub(crate) fn commit(
        self,
        time: u64,
        options: LockOptions,
    ) -> TransactionOutcome<'operation, 'clock> {
        self.commit_inner(time, options, None)
    }

    pub(crate) fn commit_preserving_mark(
        self,
        time: u64,
        options: LockOptions,
        expected: Option<Mark>,
    ) -> TransactionOutcome<'operation, 'clock> {
        self.commit_inner(time, options, Some(expected))
    }

    fn commit_inner(
        self,
        time: u64,
        options: LockOptions,
        reviewed_mark: Option<Option<Mark>>,
    ) -> TransactionOutcome<'operation, 'clock> {
        let Self {
            owner,
            witness,
            stage,
            legacy_binding,
        } = self;
        TransactionOutcome::new(owner).run(|result| {
            result.owner.native_check()?;
            result.owner.admission.check()?;
            let initial = review_context::load_at_root(result.owner.root())
                .map_err(|error| Hold::Admission(format!("{error:?}")))?;
            let mark = current_mark(&result.owner, &witness, &initial)?.cloned();
            if let Some(expected) = reviewed_mark
                && (legacy_binding.is_none() || mark != expected)
            {
                return Err(Hold::Repair(RepairHold::MarkChanged));
            }
            let mut intent = if let Some(binding) = legacy_binding {
                RestoreIntent::new_legacy(
                    binding,
                    witness,
                    result.owner.destination.destination().clone(),
                    stage,
                    mark.as_ref(),
                )
            } else {
                RestoreIntent::new(
                    result.owner.destination.locator().clone(),
                    witness,
                    result.owner.destination.destination().clone(),
                    stage,
                    mark.as_ref(),
                )
            }
            .map_err(Hold::Repair)?;
            result.owner.admission.attach(&mut intent)?;
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            tests::checkpoint(tests::Point::AfterInitialMark);
            let claims = OutputClaims::acquire(
                intent.output_paths(result.owner.root().canonical_path()),
                options,
            )
            .map_err(|_| Hold::Repair(RepairHold::ClaimsUnavailable))?;
            let commit = claims
                .commit(result.owner.destination.live().diagnostic_path())
                .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?;
            if claims
                .segment_identity(result.owner.destination.live().diagnostic_path())
                .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?
                != result.owner.destination.live().object_identity()
            {
                return Err(Hold::Repair(RepairHold::DestinationChanged));
            }
            result.owner.effect_check(&commit)?;
            let current = review_context::load_at_root(result.owner.root())
                .map_err(|error| Hold::Admission(format!("{error:?}")))?;
            if current_mark(&result.owner, &intent.witness, &current)? != mark.as_ref() {
                return Err(Hold::Repair(RepairHold::MarkChanged));
            }
            let selection = catalog(
                &result.owner,
                intent.legacy_binding.as_ref(),
                Some(&intent.event),
            )?;
            let content = current_content(&result.owner, &intent)?;
            inspect_recovery(
                &intent,
                result.owner.destination.destination(),
                &selection,
                &current,
                content,
            )
            .map_err(Hold::Repair)?;
            let store =
                match RestoreIntentStore::create(Arc::clone(result.owner.carrier.activity())) {
                    Ok(store) => store,
                    Err(failure) => {
                        result.store_failure = Some(failure);
                        return Err(Hold::StorePublication);
                    }
                };
            result.record = Some(Record {
                store: Some(store),
                intent,
                persisted: None,
                writes: Vec::new(),
            });
            result.persist()?;
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            tests::checkpoint(tests::Point::AfterInitialIntent);
            result.effect_check(&commit)?;
            if result.inspect()?.publication == PublicationDecision::HistoricalOnly {
                return Err(Hold::Repair(RepairHold::OperationCollision));
            }
            result.effect_check(&commit)?;
            let name = result.record()?.intent.witness.file.name.clone();
            let publication = result
                .owner
                .media
                .as_mut()
                .ok_or(Hold::Repair(RepairHold::InvalidIntent))?
                .publish(result.owner.destination.live(), OsStr::new(&name));
            result.effects = if publication.effect == PublicationEffect::Published {
                AttemptEffects {
                    files: 1,
                    bytes: result.record()?.intent.witness.file.bytes,
                }
            } else {
                AttemptEffects::default()
            };
            let recorded = result.record_mut()?.intent.record_outcome(&publication);
            result.publication = Some(publication);
            #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
            tests::checkpoint(tests::Point::AfterPublication);
            let recorded = recorded.map_err(|failure| Hold::Repair(failure.hold));
            // Preserve the native receipt/effect even when successor validation fails.
            recorded?;
            result.persist()?;
            result.finish(&claims, &commit, time, options, false)
        })
    }
}

impl TransactionOutcome<'_, '_> {
    fn live_matches(&self) -> Result<MediaObservation, Hold> {
        let intent = &self.record()?.intent;
        let Some(expected) = &intent.confirmed else {
            return Err(Hold::Repair(RepairHold::UncertainContent));
        };
        match self
            .owner
            .destination
            .live()
            .observe_file(selected_name(intent))
        {
            Ok(Some(current))
                if current.identity == expected.identity && current.digest == expected.digest =>
            {
                Ok(current)
            }
            _ => Err(Hold::Repair(RepairHold::UncertainContent)),
        }
    }

    fn finish(
        &mut self,
        claims: &OutputClaims,
        commit: &SegmentCommit<'_>,
        time: u64,
        options: LockOptions,
        replay: bool,
    ) -> Result<(), Hold> {
        if self.record()?.intent.no_effect_outcome() {
            if replay {
                // Saved dispositions do not reconstruct the prepared source.
                return Err(Hold::Retirement);
            }
            return self.finish_no_effect(commit, time);
        }
        let intent = &self.record()?.intent;
        let unproven_content = intent.outcome.as_ref().is_some_and(|facts| {
            (matches!(
                facts.effect,
                PublicationEffect::Published | PublicationEffect::AlreadyPresent
            ) && facts.confirmation != Confirmation::Confirmed)
                || (facts.effect == PublicationEffect::Published
                    && facts.file_durability != Durability::Synced)
        });
        let missing_parent_sync = intent.outcome.as_ref().is_some_and(|facts| {
            matches!(
                facts.effect,
                PublicationEffect::Published | PublicationEffect::AlreadyPresent
            ) && facts.confirmation == Confirmation::Confirmed
                && (facts.effect != PublicationEffect::Published
                    || facts.file_durability == Durability::Synced)
                && facts.parent_durability != Durability::Synced
        });
        let changed_day = intent.outcome.as_ref().is_some_and(|facts| {
            matches!(
                facts.effect,
                PublicationEffect::Published | PublicationEffect::AlreadyPresent
            )
        });
        // Invalidate this invocation's observed effect even if the later catalog
        // is degraded. Replay never uses history to authorize missing media.
        if !replay && changed_day && intent.marker != Obligation::Complete {
            self.owner.effect_check(commit)?;
            self.bump_marker(options)?;
            self.record_mut()?.intent.marker = Obligation::Complete;
            self.persist()?;
        }
        if unproven_content {
            return Err(Hold::Repair(RepairHold::UncertainContent));
        }
        if missing_parent_sync && !cfg!(windows) {
            if !replay {
                return Err(Hold::Retirement);
            }
            self.live_matches()?;
            if self.owner.destination.live().sync().is_ok() {
                self.record_mut()?
                    .intent
                    .advance_parent_durability()
                    .map_err(Hold::Repair)?;
                self.persist()?;
            } else {
                return Err(Hold::Retirement);
            }
        }
        let decision = self.inspect()?;
        let successful = decision.event.files.iter().all(|file| {
            matches!(
                file.disposition,
                RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
            )
        });
        if successful {
            self.live_matches()?;
            if self.record()?.intent.marker != Obligation::Complete {
                self.effect_check(commit)?;
                self.bump_marker(options)?;
                self.record_mut()?.intent.marker = Obligation::Complete;
                self.persist()?;
            }
        } else if replay && decision.publication == PublicationDecision::Missing {
            return Err(Hold::Repair(RepairHold::PublishedContentMissing));
        }
        if self.record()?.intent.event_recorded != Obligation::Complete {
            self.effect_check(commit)?;
            if !decision.event_recorded {
                self.record_mut()?
                    .intent
                    .prepare_recovery_event(&decision)
                    .map_err(Hold::Repair)?;
                self.persist()?;
            }
            self.record_event(commit, time)?;
        }
        let decision = self.inspect()?;
        match decision.register {
            RegisterDecision::Ready(_) | RegisterDecision::AlreadyApplied => {
                let confirmed_live = self.live_matches()?;
                self.effect_check(commit)?;
                let intent = &self.record()?.intent;
                let solstone_core_retention::restore_intent::MarkRetirement::Exact { plan } =
                    &intent.retirement
                else {
                    return Err(Hold::Register);
                };
                #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
                tests::checkpoint(tests::Point::BeforeRestoredCas);
                let outcome = match &self.owner.admission {
                    Admission::Readback(read) => {
                        conditional_retire_restored(RestoredMarkMutation {
                            read,
                            operation: &intent.event,
                            confirmed: &confirmed_live,
                            plan,
                            claims,
                            commit,
                        })
                        .map_err(RestoreRegisterRefusal::Restored)
                    }
                    Admission::Recovery(read, _) => {
                        conditional_retire_restored(RestoredMarkMutation {
                            read: read.as_read(),
                            operation: &intent.event,
                            confirmed: &confirmed_live,
                            plan,
                            claims,
                            commit,
                        })
                        .map_err(RestoreRegisterRefusal::Restored)
                    }
                    #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
                    Admission::LocalTest(_) => conditional_mutate_at_root(self.owner.root(), plan)
                        .map_err(RestoreRegisterRefusal::Bound),
                };
                let confirmed = matches!(&outcome,
                    Ok(BoundMutationOutcome::Published(result) | BoundMutationOutcome::Unchanged(result))
                        if matches!(result.status, MutationStatus::Applied | MutationStatus::AlreadyApplied));
                self.register = Some(outcome);
                if !confirmed {
                    return Err(Hold::Register);
                }
                self.record_mut()?.intent.register = Obligation::Complete;
                self.persist()?;
            }
            RegisterDecision::NoMark | RegisterDecision::Complete => {}
            _ => return Err(Hold::Register),
        }
        Ok(())
    }

    fn record_event(&mut self, commit: &SegmentCommit<'_>, time: u64) -> Result<(), Hold> {
        self.effect_check(commit)?;
        let intent = &self.record()?.intent;
        let appended = match (&intent.legacy_binding, intent.legacy_operation()) {
            (Some(binding), Some(operation)) => {
                append_legacy_restore_operation(self.owner.root(), binding, &operation, time)
            }
            (None, None) => append_restore_operation(
                self.owner.root(),
                &intent.locator.target,
                intent.locator.layout,
                &intent.event,
                time,
            ),
            _ => return Err(Hold::Repair(RepairHold::InvalidIntent)),
        };
        let confirmed = appended.as_ref().is_ok_and(|receipt| receipt.settles());
        self.ledger = Some(appended);
        if !confirmed {
            return Err(Hold::Ledger);
        }
        self.record_mut()?.intent.event_recorded = Obligation::Complete;
        self.persist()
    }

    fn unchanged_mark(&self) -> Result<(), Hold> {
        use solstone_core_retention::restore_intent::MarkRetirement;
        let intent = &self.record()?.intent;
        let marks = review_context::load_at_root(self.owner.root())
            .map_err(|error| Hold::Admission(format!("{error:?}")))?;
        let selected = current_mark(&self.owner, &intent.witness, &marks)?;
        if selected != intent.retirement.expected() {
            return Err(Hold::Repair(RepairHold::MarkChanged));
        }
        if let MarkRetirement::Exact { plan } = &intent.retirement
            && plan.replacement().is_some_and(|replacement| {
                replacement.id != plan.expected().id && marks.marks.contains_key(&replacement.id)
            })
        {
            return Err(Hold::Repair(RepairHold::MarkChanged));
        }
        Ok(())
    }

    fn finish_no_effect(&mut self, commit: &SegmentCommit<'_>, time: u64) -> Result<(), Hold> {
        let receipt = self.publication.as_ref().ok_or(Hold::Retirement)?;
        let expected_facts =
            solstone_core_retention::restore_intent::PublicationFacts::from(receipt);
        if !matches!(
            receipt.effect,
            PublicationEffect::Conflict | PublicationEffect::NotPublished
        ) || receipt.identity.is_some()
            || self.effects != AttemptEffects::default()
            || !self
                .record()?
                .intent
                .outcome
                .as_ref()
                .is_some_and(|facts| facts.matches_ignoring_cleanup(&expected_facts))
        {
            return Err(Hold::Retirement);
        }
        self.effect_check(commit)?;
        self.unchanged_mark()?;
        // Inspect fresh history and current names, but keep the actual native
        // prospective event even if matching bytes appeared after the attempt.
        let decision = self.inspect()?;
        if !decision.event_recorded && self.record()?.intent.event_recorded == Obligation::Complete
        {
            return Err(Hold::Ledger);
        }
        if self.record()?.intent.event_recorded != Obligation::Complete {
            self.record_event(commit, time)?;
        }
        self.effect_check(commit)?;
        self.unchanged_mark()?;
        if !self.inspect()?.event_recorded {
            return Err(Hold::Ledger);
        }
        let cleanup = self.owner.media.as_mut().ok_or(Hold::Retirement)?.cleanup();
        self.no_effect_cleanup = Some(NoEffectCleanup {
            cleanup,
            parent_sync: None,
        });
        if cleanup != Cleanup::Complete {
            return Err(Hold::Retirement);
        }
        let parent_sync = self.owner.destination.stage_parent().sync();
        let synced = if cfg!(windows) {
            parent_sync.is_ok()
                || parent_sync
                    .as_ref()
                    .err()
                    .is_some_and(|err| err.kind() == std::io::ErrorKind::Unsupported)
        } else {
            parent_sync.is_ok()
        };
        self.no_effect_cleanup
            .as_mut()
            .ok_or(Hold::Retirement)?
            .parent_sync = Some(parent_sync);
        if !synced {
            return Err(Hold::Retirement);
        }
        #[cfg(all(test, feature = "full-tests", any(unix, windows)))]
        tests::checkpoint(tests::Point::BeforeNoEffectPersist);
        self.effect_check(commit)?;
        self.unchanged_mark()?;
        if !self.inspect()?.event_recorded {
            return Err(Hold::Ledger);
        }
        let intent = &mut self.record_mut()?.intent;
        intent.cleanup = Obligation::Complete;
        intent.marker = Obligation::NotRequired;
        intent.register = Obligation::NotRequired;
        self.persist()
    }

    #[cfg(unix)]
    fn bump_marker(&mut self, options: LockOptions) -> Result<(), Hold> {
        use solstone_core_journal_io::atomic::BoundAtomicOutcome;
        use solstone_core_journal_io::health_marker::bump_stream_marker_bound;
        use solstone_core_journal_io::media::MediaDirectory;
        use solstone_core_journal_io::{FlatDirectory, create_or_open_flat_directory_bound};
        let root = Arc::clone(self.owner.root());
        let day = &self.record()?.intent.locator.target.day;
        let chronicle_relative = std::path::Path::new("chronicle");
        let day_relative = chronicle_relative.join(day);
        let chronicle = FlatDirectory::open(&root, chronicle_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        let day_parent = FlatDirectory::open(&root, &day_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        let chronicle_guard = MediaDirectory::open(Arc::clone(&root), chronicle_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        let day_guard = MediaDirectory::open(Arc::clone(&root), &day_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        if chronicle.identity() != chronicle_guard.object_identity()
            || day_parent.identity() != day_guard.object_identity()
        {
            return Err(Hold::Marker);
        }
        self.owner.native_check()?;
        chronicle_guard.revalidate().map_err(|_| Hold::Marker)?;
        day_guard.revalidate().map_err(|_| Hold::Marker)?;
        let day_health = create_or_open_flat_directory_bound(
            &day_parent,
            OsStr::new("health"),
            0o700,
            &root.canonical_path().join(&day_relative),
        )
        .map_err(|error| Hold::Admission(error.to_string()))?;
        let named = MediaDirectory::open(Arc::clone(&root), &day_relative.join("health"))
            .map_err(|error| Hold::Admission(error.to_string()))?;
        if day_health.identity() != named.object_identity() {
            return Err(Hold::Marker);
        }
        self.owner.native_check()?;
        chronicle_guard.revalidate().map_err(|_| Hold::Marker)?;
        day_guard.revalidate().map_err(|_| Hold::Marker)?;
        named.revalidate().map_err(|_| Hold::Marker)?;
        let publication = bump_stream_marker_bound(&day_health, options);
        let confirmed = publication.as_ref().is_ok_and(|receipt| {
            matches!(receipt.publication(), BoundAtomicOutcome::Published { .. })
                && receipt.binding_error().is_none()
                && receipt.lock_error().is_none()
        });
        self.marker = Some(publication);
        // The atomic leaf syncs health. Keep actual day/chronicle/root sync
        // facts separate from the canonical parent/name checks below.
        self.marker_ancestor_sync
            .push(nix::unistd::fsync(&day_parent).map_err(std::io::Error::from));
        self.marker_ancestor_sync
            .push(nix::unistd::fsync(&chronicle).map_err(std::io::Error::from));
        self.marker_ancestor_sync
            .push(nix::unistd::fsync(root.as_ref()).map_err(std::io::Error::from));
        let bound = named.revalidate().is_ok()
            && day_guard.revalidate().is_ok()
            && chronicle_guard.revalidate().is_ok()
            && day_health.revalidate().is_ok()
            && self.owner.native_check().is_ok();
        if !confirmed || !bound || self.marker_ancestor_sync.iter().any(Result::is_err) {
            return Err(Hold::Marker);
        }
        Ok(())
    }

    #[cfg(windows)]
    fn bump_marker(&mut self, options: LockOptions) -> Result<(), Hold> {
        use solstone_core_journal_io::atomic::DetailedAtomicOutcome;
        use solstone_core_journal_io::media::MediaDirectory;
        use solstone_core_journal_io::{
            HealthMarker, acquire_existing_parent_lock_bound, atomic_replace_detailed_bound,
            create_or_open_windows_flat_directory_bound, open_windows_flat_directory_bound,
            read_windows_observed_file_bounded,
        };
        let root = Arc::clone(self.owner.root());
        let day = &self.record()?.intent.locator.target.day;
        let chronicle_relative = std::path::Path::new("chronicle");
        let day_relative = chronicle_relative.join(day);
        let chronicle_diagnostic = root.canonical_path().join("chronicle");
        let day_diagnostic = chronicle_diagnostic.join(day);
        let _health_diagnostic = day_diagnostic.join("health");
        let chronicle = open_windows_flat_directory_bound(
            root.as_ref(),
            OsStr::new("chronicle"),
            root.canonical_path(),
        )
        .map_err(|error| Hold::Admission(error.to_string()))?
        .ok_or(Hold::Marker)?;
        let day_parent =
            open_windows_flat_directory_bound(&chronicle, OsStr::new(day), &chronicle_diagnostic)
                .map_err(|error| Hold::Admission(error.to_string()))?
                .ok_or(Hold::Marker)?;
        let chronicle_guard = MediaDirectory::open(Arc::clone(&root), chronicle_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        let day_guard = MediaDirectory::open(Arc::clone(&root), &day_relative)
            .map_err(|error| Hold::Admission(error.to_string()))?;
        if chronicle.object_identity() != chronicle_guard.object_identity()
            || day_parent.object_identity() != day_guard.object_identity()
        {
            return Err(Hold::Marker);
        }
        self.owner.native_check()?;
        chronicle_guard.revalidate().map_err(|_| Hold::Marker)?;
        day_guard.revalidate().map_err(|_| Hold::Marker)?;
        let day_health = create_or_open_windows_flat_directory_bound(
            &day_parent,
            OsStr::new("health"),
            &day_diagnostic,
        )
        .map_err(|error| Hold::Admission(error.to_string()))?;
        let named = MediaDirectory::open(Arc::clone(&root), &day_relative.join("health"))
            .map_err(|error| Hold::Admission(error.to_string()))?;
        if day_health.object_identity() != named.object_identity() {
            return Err(Hold::Marker);
        }
        self.owner.native_check()?;
        chronicle_guard.revalidate().map_err(|_| Hold::Marker)?;
        day_guard.revalidate().map_err(|_| Hold::Marker)?;
        named.revalidate().map_err(|_| Hold::Marker)?;

        let lock_name = OsStr::new("stream.updated.lock");
        let lock = acquire_existing_parent_lock_bound(
            &day_health,
            lock_name,
            options.timeout,
            options.poll_interval,
        )
        .map_err(|_| Hold::Marker)?;
        day_health.revalidate_bound().map_err(|_| Hold::Marker)?;
        lock.revalidate_bound(&day_health, lock_name)
            .map_err(|_| Hold::Marker)?;

        let observed = read_windows_observed_file_bounded(
            &day_health,
            OsStr::new("stream.updated"),
            1024 * 1024,
        )
        .map_err(|_| Hold::Marker)?;
        let (generation, fingerprint) = match observed {
            None => (0, None),
            Some(observed) if observed.bytes.is_empty() => (0, None),
            Some(observed) => {
                let marker = serde_json::from_slice::<HealthMarker>(&observed.bytes)
                    .map_err(|_| Hold::Marker)?;
                if marker.version != 1 {
                    return Err(Hold::Marker);
                }
                (marker.generation, marker.fingerprint)
            }
        };
        let generation = generation.checked_add(1).ok_or(Hold::Marker)?;
        let marker = HealthMarker {
            version: 1,
            generation,
            fingerprint,
        };
        let contents = serde_json::to_vec(&marker).map_err(|_| Hold::Marker)?;
        day_health.revalidate_bound().map_err(|_| Hold::Marker)?;
        lock.revalidate_bound(&day_health, lock_name)
            .map_err(|_| Hold::Marker)?;

        let publication = atomic_replace_detailed_bound(
            &day_health,
            &lock,
            OsStr::new("stream.updated"),
            &contents,
            0o600,
        );
        let confirmed = matches!(publication, Ok(DetailedAtomicOutcome::Published));
        let bound = named.revalidate().is_ok()
            && day_guard.revalidate().is_ok()
            && chronicle_guard.revalidate().is_ok()
            && day_health.revalidate_bound().is_ok()
            && lock.revalidate_bound(&day_health, lock_name).is_ok()
            && self.owner.native_check().is_ok();
        if !confirmed || !bound {
            return Err(Hold::Marker);
        }
        self.marker_published = true;
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn bump_marker(&mut self, _: LockOptions) -> Result<(), Hold> {
        Err(Hold::Marker)
    }
}

/// Replay advances metadata only. Missing/unowned stages never mint a new attempt.
pub(super) fn replay<'operation, 'clock>(
    destination: PreparedRestore,
    operation: &'operation ReadOperation<'clock>,
    operation_id: &str,
    time: u64,
    options: LockOptions,
) -> TransactionOutcome<'operation, 'clock> {
    let owner = Owner {
        carrier: Arc::clone(operation.carrier()),
        destination,
        admission: Admission::Readback(operation),
        media: None,
    };
    replay_local(owner, operation_id, time, options)
}

fn replay_local<'operation, 'clock>(
    owner: Owner<'operation, 'clock>,
    operation_id: &str,
    time: u64,
    options: LockOptions,
) -> TransactionOutcome<'operation, 'clock> {
    TransactionOutcome::new(owner).run(|result| {
        result.owner.native_check()?;
        result.owner.admission.check()?;
        let store = match RestoreIntentStore::create(Arc::clone(result.owner.carrier.activity())) {
            Ok(store) => store,
            Err(failure) => {
                result.store_failure = Some(failure);
                return Err(Hold::StorePublication);
            }
        };
        let observed = match store.read(operation_id) {
            Ok(Some(observed)) => observed,
            value => {
                result.store_failure = Some(StoreAdmissionFailure {
                    owner: Box::new(store),
                    problem: StoreProblem::Changed,
                });
                return Err(match value {
                    Err(error) => Hold::Store(error),
                    _ => Hold::Repair(RepairHold::UnreadableIntent),
                });
            }
        };
        result.record = Some(Record {
            store: Some(store),
            intent: observed.intent.clone(),
            persisted: Some(observed.intent),
            writes: Vec::new(),
        });
        let intent = &result.record()?.intent;
        if intent.destination != *result.owner.destination.destination()
            || intent.locator != *result.owner.destination.locator()
            || matches!(result.owner.admission, Admission::Readback(operation)
                    if intent.legacy_binding.as_ref().map(|binding| binding.repository_id.as_str())
                        .or(intent.witness.repository_id.as_deref()) != Some(operation.repository_id()))
        {
            return Err(Hold::Repair(RepairHold::DestinationChanged));
        }
        let claims = OutputClaims::acquire(
            intent.output_paths(result.owner.root().canonical_path()),
            options,
        )
        .map_err(|_| Hold::Repair(RepairHold::ClaimsUnavailable))?;
        let commit = claims
            .commit(result.owner.destination.live().diagnostic_path())
            .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?;
        if claims
            .segment_identity(result.owner.destination.live().diagnostic_path())
            .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?
            != result.owner.destination.live().object_identity()
        {
            return Err(Hold::Repair(RepairHold::DestinationChanged));
        }
        result.effect_check(&commit)?;
        result.finish(&claims, &commit, time, options, true)
    })
}

#[cfg(all(test, feature = "full-tests", any(unix, windows)))]
mod tests;

pub(super) mod retirement;

mod owned;
pub(crate) use owned::{DetachedTransaction, SettledTransaction, SettlementFailure};

fn capacity_allows(available: std::io::Result<u64>, required: u64) -> Result<(), Hold> {
    match available {
        Ok(bytes) if required > 0 && bytes >= required => Ok(()),
        Ok(_) => Err(Hold::Admission("insufficient stage capacity".into())),
        Err(error) => Err(Hold::Admission(error.to_string())),
    }
}
