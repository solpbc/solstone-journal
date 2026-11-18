// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owning archive-offer creation. Returned resources still require settlement.

use jiff::Timestamp;
use jiff::civil::Date;
use solstone_core_backup::archive_activity::PreparedArchiveOperation;
use solstone_core_backup::archive_readback::{
    ReadOperation, ReadbackAdmissionError, ReadbackError,
};
use solstone_core_backup_runtime::{
    AdmittedArchive, ArchiveAdmissionFailure, ArchiveError, ArchiveFailure, ArchiveSnapshot,
    BackupServices,
};
use solstone_core_journal_io::media::{
    MediaStage, OwnedMediaFailure, OwnedMediaTarget, PreparedMedia,
};
use solstone_core_retention::archive_offer::{
    ObservedOffloadOffers, OffloadOfferFailure, OffloadOfferOutcome, OffloadOfferRefusal,
    OffloadOfferSelection, VerifiedOffloadOffers,
};

mod owned;
#[cfg(all(test, unix, feature = "full-tests"))]
pub(super) use owned::with_source_cleanup_fault;
pub use owned::{
    OffloadSourceCleanup, OffloadSourceCopyReport, OwnedOffloadCreation,
    OwnedOffloadCreationFinish, OwnedOffloadCreationFinishRefusal, OwnedOffloadCreationHold,
    OwnedOffloadCreationOwner, OwnedOffloadInputFailure, OwnedOffloadInputs,
    SettledOffloadCreation, SettledOffloadCreationReport, create_owned_offload_offers,
    prepare_owned_offload_inputs,
};
mod planner;
#[cfg(all(test, unix, feature = "full-tests"))]
pub(super) use planner::with_payload_capacity_fault;
pub use planner::{
    OffloadPlanning, OffloadPlanningPhase, OffloadPlanningRefusal, OffloadPlanningReport,
    OwnedOffloadPlan, OwnedOffloadPlanFinish, OwnedOffloadPlanFinishRefusal, OwnedOffloadPlanHold,
    OwnedOffloadPlanInputFailure, OwnedOffloadPlanInputs, OwnedOffloadPlanOwner,
    SettledOffloadPlan, SettledOffloadPlanReport, create_owned_planned_offload_offers,
    create_planned_offload_offers, prepare_owned_offload_plan,
};

use crate::ledger::writer::{
    LedgerConfirmation, LedgerSync, LedgerWriteEffect, LedgerWriteReceipt, LedgerWriteRefusal,
};

#[derive(Debug)]
pub enum OffloadCreationRefusal {
    Inputs(OffloadOfferRefusal),
    Archive(ArchiveError),
    Readback(ReadbackError),
    Native,
    Metadata,
    Entropy,
    Ledger,
    Verification,
    Publication,
    Planning(OffloadPlanningRefusal),
}

/// Borrowed original operation phase, never an extractable completion token.
pub enum OffloadCreationOwner<'a, 'clock> {
    Prepared(&'a PreparedArchiveOperation<'clock>),
    AdmissionFailure(&'a ArchiveAdmissionFailure<'clock>),
    Admitted(&'a AdmittedArchive<'clock>),
    ReadAdmissionFailure(&'a ReadbackAdmissionError<'clock>),
    Read(&'a ReadOperation<'clock>),
    VerificationFailure(&'a OffloadOfferFailure<'clock>),
    Verified(&'a VerifiedOffloadOffers<'clock>),
    Published(&'a OffloadOfferOutcome<'clock>),
}

enum Owner<'clock> {
    Prepared(Box<PreparedArchiveOperation<'clock>>),
    AdmissionFailure(Box<ArchiveAdmissionFailure<'clock>>),
    Admitted(Box<AdmittedArchive<'clock>>),
    ReadAdmissionFailure(Box<ReadbackAdmissionError<'clock>>),
    Read(Box<ReadOperation<'clock>>),
    VerificationFailure(Box<OffloadOfferFailure<'clock>>),
    Verified(Box<VerifiedOffloadOffers<'clock>>),
    Published(Box<OffloadOfferOutcome<'clock>>),
}

enum PartialCopy {
    Stage(OwnedMediaFailure<Option<MediaStage>>),
    Target(OwnedMediaFailure<MediaStage>),
    Copy(OwnedMediaFailure<OwnedMediaTarget>),
}

/// Native partial copy ownership remains in the returned creation result.
pub enum OffloadCreationNative<'a> {
    Stage(Option<&'a MediaStage>),
    Target(&'a MediaStage),
    Copy(&'a OwnedMediaTarget),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OffloadLedgerEffect {
    AlreadyRecorded,
    Appended { bytes: usize },
    Attempted { written: Option<usize> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OffloadLedgerSync {
    Confirmed,
    Uncertain,
}

/// Reporting facts do not authorize publication, cleanup or release.
pub enum OffloadLedgerWrite<'a> {
    Recorded {
        effect: OffloadLedgerEffect,
        durability: [OffloadLedgerSync; 4],
        confirmation: OffloadLedgerSync,
        failures: &'a [String],
    },
    Refused {
        reason: &'static str,
        detail: Option<&'a str>,
    },
}

#[derive(Default)]
struct Resources {
    planning: Option<OffloadPlanning>,
    refused_input_carrier:
        Option<std::sync::Arc<solstone_core_journal_io::media::RootActivityCarrier>>,
    observed: Option<ObservedOffloadOffers>,
    copies: Vec<Vec<PreparedMedia>>,
    partial: Option<PartialCopy>,
    archives: Vec<Result<ArchiveSnapshot, ArchiveFailure>>,
    ledger: Vec<Result<LedgerWriteReceipt, LedgerWriteRefusal>>,
    reason: Option<OffloadCreationRefusal>,
}

impl Resources {
    /// Nothing was copied, archived or recorded; only observations were made.
    pub(super) fn unstaged(&self) -> bool {
        self.copies.iter().all(Vec::is_empty)
            && self.partial.is_none()
            && self.archives.is_empty()
            && self.ledger.is_empty()
    }
}

/// Retains the entire original admission, copies and independent write facts.
#[must_use = "Retain all original resources until their owning work settles"]
pub struct OffloadCreation<'clock> {
    owner: Owner<'clock>,
    resources: Resources,
}

impl<'clock> OffloadCreation<'clock> {
    pub fn planning(&self) -> Option<&OffloadPlanning> {
        self.resources.planning.as_ref()
    }

    pub fn owner(&self) -> OffloadCreationOwner<'_, 'clock> {
        match &self.owner {
            Owner::Prepared(owner) => OffloadCreationOwner::Prepared(owner),
            Owner::AdmissionFailure(owner) => OffloadCreationOwner::AdmissionFailure(owner),
            Owner::Admitted(owner) => OffloadCreationOwner::Admitted(owner),
            Owner::ReadAdmissionFailure(owner) => OffloadCreationOwner::ReadAdmissionFailure(owner),
            Owner::Read(owner) => OffloadCreationOwner::Read(owner),
            Owner::VerificationFailure(owner) => OffloadCreationOwner::VerificationFailure(owner),
            Owner::Verified(owner) => OffloadCreationOwner::Verified(owner),
            Owner::Published(owner) => OffloadCreationOwner::Published(owner),
        }
    }

    pub fn reason(&self) -> Option<&OffloadCreationRefusal> {
        self.resources.reason.as_ref()
    }

    pub fn observed(&self) -> Option<&ObservedOffloadOffers> {
        match &self.owner {
            Owner::VerificationFailure(failure) => Some(failure.observed()),
            Owner::Verified(verified) => Some(verified.observed()),
            Owner::Published(outcome) => Some(outcome.verified().observed()),
            _ => self.resources.observed.as_ref(),
        }
    }

    pub fn source_copies(&self) -> impl Iterator<Item = &PreparedMedia> {
        self.resources.copies.iter().flatten()
    }

    pub fn partial_copy(&self) -> Option<OffloadCreationNative<'_>> {
        self.resources
            .partial
            .as_ref()
            .map(|partial| match partial {
                PartialCopy::Stage(failure) => OffloadCreationNative::Stage(failure.owned.as_ref()),
                PartialCopy::Target(failure) => OffloadCreationNative::Target(&failure.owned),
                PartialCopy::Copy(failure) => OffloadCreationNative::Copy(&failure.owned),
            })
    }

    pub fn native_error(&self) -> Option<&std::io::Error> {
        self.resources
            .partial
            .as_ref()
            .map(|partial| match partial {
                PartialCopy::Stage(failure) => &failure.error,
                PartialCopy::Target(failure) => &failure.error,
                PartialCopy::Copy(failure) => &failure.error,
            })
    }

    pub fn archives(&self) -> &[Result<ArchiveSnapshot, ArchiveFailure>] {
        &self.resources.archives
    }

    pub fn ledger_writes(&self) -> impl Iterator<Item = OffloadLedgerWrite<'_>> {
        ledger_writes(&self.resources.ledger)
    }
}

fn ledger_writes(
    results: &[Result<LedgerWriteReceipt, LedgerWriteRefusal>],
) -> impl Iterator<Item = OffloadLedgerWrite<'_>> {
    results.iter().map(|result| match result {
        Ok(receipt) => OffloadLedgerWrite::Recorded {
            effect: match receipt.effect() {
                LedgerWriteEffect::AlreadyRecorded => OffloadLedgerEffect::AlreadyRecorded,
                LedgerWriteEffect::Appended { bytes } => OffloadLedgerEffect::Appended { bytes },
                LedgerWriteEffect::Attempted { written } => {
                    OffloadLedgerEffect::Attempted { written }
                }
            },
            durability: receipt.durability().map(|sync| match sync {
                LedgerSync::Confirmed => OffloadLedgerSync::Confirmed,
                LedgerSync::Uncertain => OffloadLedgerSync::Uncertain,
            }),
            confirmation: match receipt.confirmation() {
                LedgerConfirmation::Confirmed => OffloadLedgerSync::Confirmed,
                LedgerConfirmation::Uncertain => OffloadLedgerSync::Uncertain,
            },
            failures: receipt.failures(),
        },
        Err(refusal) => {
            let (reason, detail) = match refusal {
                LedgerWriteRefusal::InvalidInput => ("invalid_input", None),
                LedgerWriteRefusal::RootMismatch => ("root_mismatch", None),
                LedgerWriteRefusal::Busy => ("busy", None),
                LedgerWriteRefusal::HistoryUnavailable(detail) => {
                    ("history_unavailable", Some(detail.as_str()))
                }
                LedgerWriteRefusal::Collision => ("collision", None),
                LedgerWriteRefusal::Changed => ("changed", None),
                LedgerWriteRefusal::Io(detail) => ("io", Some(detail.as_str())),
                LedgerWriteRefusal::Unsupported => ("unsupported", None),
            };
            OffloadLedgerWrite::Refused { reason, detail }
        }
    })
}

/// Create offers under one original admission. Production invocation still
/// requires the owning cooperation and settlement boundary.
pub fn create_offload_offers<'clock>(
    operation: PreparedArchiveOperation<'clock>,
    services: &BackupServices<'clock>,
    selections: &[OffloadOfferSelection],
    today: Date,
    now: Timestamp,
) -> OffloadCreation<'clock> {
    #[cfg(any(unix, windows))]
    {
        native::create(operation, services, selections, today, now)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (services, selections, today, now);
        OffloadCreation {
            owner: Owner::Prepared(Box::new(operation)),
            resources: Resources {
                reason: Some(OffloadCreationRefusal::Inputs(
                    OffloadOfferRefusal::Unsupported,
                )),
                ..Resources::default()
            },
        }
    }
}

#[cfg(any(unix, windows))]
mod native {
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsStr;
    use std::path::Path;
    use std::sync::Arc;

    use solstone_core_journal_io::{JournalRoot, media::MediaDirectory, paths::SegmentLayout};
    use solstone_core_retention::{
        archive_catalog::{
            ArchiveManifest, ArchiveWitness, JournalCatalog, codec::Event, read_complete_catalog,
        },
        archive_offer::{
            ObservedOffloadGroup, capture_offload_offers, publish_offload_offers,
            verify_offload_offers,
        },
        archive_proof::{ArchiveFileProof, OffloadProof},
        mark_mutation::BoundMutationOutcome,
        marks::{Register, RemovalClass},
        receipt::Target,
        review_context::{MAX_REGISTER_BYTES, load_at_root},
    };

    use super::*;
    use crate::ledger::writer::{append_archive_witness, confirm_archive_witness};

    pub(super) struct WitnessWrite {
        proof: OffloadProof,
        reused: bool,
    }

    pub(super) fn fail<'clock>(
        owner: Owner<'clock>,
        mut resources: Resources,
        reason: OffloadCreationRefusal,
    ) -> OffloadCreation<'clock> {
        resources.reason = Some(reason);
        if let Some(planning) = &mut resources.planning {
            planning.phase = OffloadPlanningPhase::Held;
        }
        OffloadCreation { owner, resources }
    }

    pub(super) fn inputs(error: OffloadOfferRefusal) -> OffloadCreationRefusal {
        OffloadCreationRefusal::Inputs(error)
    }

    pub(super) fn record_publication(
        resources: &mut Resources,
        publication: &Result<BoundMutationOutcome, OffloadOfferRefusal>,
    ) {
        if !matches!(
            publication,
            Ok(BoundMutationOutcome::Published(_) | BoundMutationOutcome::Unchanged(_))
        ) {
            resources.reason = Some(OffloadCreationRefusal::Publication);
        }
    }

    fn located(witness: &ArchiveWitness, selection: &OffloadOfferSelection) -> bool {
        witness.segment_dir.as_deref() == Some(selection.target.dir.as_str())
            && witness.layout == Some(selection.layout)
    }

    fn active<'a>(
        catalog: &'a JournalCatalog,
        selection: &OffloadOfferSelection,
        name: &str,
    ) -> Result<Option<&'a ArchiveWitness>, OffloadCreationRefusal> {
        let key = solstone_core_format::segment::segment_key(&selection.target.dir)
            .ok_or_else(|| inputs(OffloadOfferRefusal::InvalidSelection))?;
        let mut matching = catalog
            .days
            .iter()
            .filter(|day| day.day == selection.target.day)
            .flat_map(|day| &day.segments)
            .filter(|segment| segment.stream == selection.target.stream && segment.segment == key)
            .flat_map(|segment| &segment.active)
            .filter(|witness| witness.file.name == name && located(witness, selection));
        let result = matching.next();
        if matching.next().is_some() {
            return Err(inputs(OffloadOfferRefusal::Changed));
        }
        Ok(result)
    }

    // Partition historical coordinates only. Capture owns all eligibility and
    // expected-row observations after this planning read.
    pub(super) fn partition(
        catalog: &JournalCatalog,
        register: &Register,
        selections: &[OffloadOfferSelection],
    ) -> Result<Vec<OffloadOfferSelection>, OffloadCreationRefusal> {
        let mut result = Vec::new();
        let mut selected = BTreeSet::new();
        for selection in selections {
            if selection.names.is_empty()
                || selection.names.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(inputs(OffloadOfferRefusal::InvalidSelection));
            }
            for name in &selection.names {
                if !selected.insert((
                    &selection.target.day,
                    &selection.target.stream,
                    &selection.target.dir,
                    selection.layout == SegmentLayout::Named,
                    name,
                )) {
                    return Err(inputs(OffloadOfferRefusal::InvalidSelection));
                }
            }
            let mut remaining: BTreeSet<_> = selection.names.iter().cloned().collect();
            for mark in register.marks.values().filter(|mark| {
                mark.class == RemovalClass::OffloadRawRelease && mark.target == selection.target
            }) {
                if !mark
                    .proposal
                    .names
                    .iter()
                    .any(|name| selection.names.contains(name))
                {
                    continue;
                }
                if !mark
                    .proposal
                    .names
                    .iter()
                    .all(|name| remaining.contains(name))
                {
                    return Err(inputs(OffloadOfferRefusal::InvalidSelection));
                }
                for name in &mark.proposal.names {
                    remaining.remove(name);
                }
                result.push(OffloadOfferSelection {
                    names: mark.proposal.names.clone(),
                    ..selection.clone()
                });
            }
            let mut by_manifest: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
            for name in remaining {
                let manifest =
                    active(catalog, selection, &name)?.map(|witness| witness.manifest_id.clone());
                by_manifest.entry(manifest).or_default().push(name);
            }
            result.extend(
                by_manifest
                    .into_values()
                    .map(|names| OffloadOfferSelection {
                        names,
                        ..selection.clone()
                    }),
            );
        }
        if result.is_empty() {
            return Err(inputs(OffloadOfferRefusal::InvalidSelection));
        }
        Ok(result)
    }

    pub(super) fn capacity(
        operation: &PreparedArchiveOperation<'_>,
        observed: &ObservedOffloadOffers,
    ) -> Result<Arc<MediaDirectory>, OffloadCreationRefusal> {
        let mut total = 0u64;
        let mut maximum = 0;
        for file in observed.groups().iter().flat_map(|group| group.files()) {
            total = total
                .checked_add(file.observation().digest.size)
                .ok_or(OffloadCreationRefusal::Metadata)?;
            maximum = maximum.max(file.observation().digest.size);
        }
        let payload = total
            .checked_add(maximum)
            .ok_or(OffloadCreationRefusal::Metadata)?;
        capacity_for_payload(operation, payload)
    }

    pub(super) fn capacity_for_payload(
        operation: &PreparedArchiveOperation<'_>,
        payload: u64,
    ) -> Result<Arc<MediaDirectory>, OffloadCreationRefusal> {
        let metadata = u64::try_from(MAX_REGISTER_BYTES)
            .ok()
            .and_then(|bound| bound.checked_mul(2))
            .and_then(|bound| bound.checked_add(1024 * 1024))
            .ok_or(OffloadCreationRefusal::Metadata)?;
        let required = operation
            .configuration()
            .offload_required_bytes(payload, metadata)
            .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))?;
        operation
            .revalidate()
            .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))?;
        let parent = MediaDirectory::open(Arc::clone(operation.journal_root()), Path::new(""))
            .map_err(|error| inputs(error.into()))?;
        let available = parent
            .available_bytes()
            .map_err(|error| inputs(error.into()))?;
        if available < required {
            return Err(inputs(OffloadOfferRefusal::InsufficientSpace {
                available,
                required,
            }));
        }
        operation
            .revalidate()
            .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))?;
        Ok(parent)
    }

    pub(super) fn copy_inputs(
        operation: &PreparedArchiveOperation<'_>,
        parent: Arc<MediaDirectory>,
        resources: &mut Resources,
    ) -> Result<(), OffloadCreationRefusal> {
        for group in resources
            .observed
            .as_ref()
            .expect("captured inputs")
            .groups()
        {
            resources.copies.push(Vec::new());
            let copies = resources.copies.last_mut().expect("copy group");
            for file in group.files() {
                operation
                    .revalidate()
                    .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))?;
                let stage =
                    match MediaStage::create(Arc::clone(&parent), Arc::clone(operation.activity()))
                    {
                        Ok(stage) => stage,
                        Err(failure) => {
                            resources.partial = Some(PartialCopy::Stage(failure));
                            return Err(OffloadCreationRefusal::Native);
                        }
                    };
                let target = match stage.create_named_target(OsStr::new(file.name())) {
                    Ok(target) => target,
                    Err(failure) => {
                        resources.partial = Some(PartialCopy::Target(failure));
                        return Err(OffloadCreationRefusal::Native);
                    }
                };
                match target.copy_from(
                    group.directory(),
                    OsStr::new(file.name()),
                    &file.observation().digest,
                ) {
                    Ok(prepared) => copies.push(prepared),
                    Err(failure) => {
                        resources.partial = Some(PartialCopy::Copy(failure));
                        return Err(OffloadCreationRefusal::Native);
                    }
                }
                let current = group
                    .directory()
                    .observe_file(OsStr::new(file.name()))
                    .map_err(|error| inputs(error.into()))?;
                if current.as_ref() != Some(file.observation()) {
                    return Err(inputs(OffloadOfferRefusal::Changed));
                }
            }
        }
        operation
            .revalidate()
            .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))
    }

    fn manifest_proof(
        target: &Target,
        layout: SegmentLayout,
        manifest: &ArchiveManifest,
    ) -> Option<OffloadProof> {
        let first = manifest.files.first()?;
        let mut files = Vec::new();
        for witness in &manifest.files {
            if witness.journal_id != first.journal_id
                || witness.repository_id != first.repository_id
                || witness.snapshot_id != first.snapshot_id
                || witness.segment_dir.as_deref() != Some(target.dir.as_str())
                || witness.layout != Some(layout)
                || witness.manifest_id != manifest.manifest_id
            {
                return None;
            }
            files.push(ArchiveFileProof {
                witness_id: witness.witness_id.clone(),
                name: witness.file.name.clone(),
                bytes: witness.file.bytes,
                sha256: witness.file.sha256.clone(),
                archive_path: witness.archive_path.clone()?,
                processing: witness.processing.clone()?,
            });
        }
        let proof = OffloadProof {
            target: target.clone(),
            layout,
            journal_id: first.journal_id.clone()?,
            repository_id: first.repository_id.clone()?,
            snapshot_id: first.snapshot_id.clone(),
            manifest_id: manifest.manifest_id.clone(),
            files,
        };
        proof.validate().ok()?;
        Some(proof)
    }

    fn matches_inputs(
        proof: &OffloadProof,
        group: &ObservedOffloadGroup,
        root: &JournalRoot,
        repository: &str,
    ) -> bool {
        proof.target == group.selection().target
            && proof.layout == group.selection().layout
            && proof.journal_id == root.identity().observation_id()
            && proof.repository_id == repository
            && proof.files.len() == group.files().len()
            && proof
                .files
                .iter()
                .zip(group.files())
                .all(|(file, current)| {
                    file.name == current.name()
                        && file.bytes == current.observation().digest.size
                        && file.sha256 == current.observation().digest.sha256
                        && &file.processing == current.processing()
                })
    }

    fn reusable(
        catalog: &JournalCatalog,
        group: &ObservedOffloadGroup,
        root: &JournalRoot,
        repository: &str,
    ) -> Result<Option<(OffloadProof, OffloadProof)>, OffloadCreationRefusal> {
        let mut manifests = catalog
            .days
            .iter()
            .filter(|day| day.day == group.selection().target.day)
            .flat_map(|day| &day.segments)
            .filter(|segment| segment.stream == group.selection().target.stream)
            .flat_map(|segment| &segment.history);
        let proposed = group
            .expected_mark()
            .and_then(|mark| mark.offload.as_ref())
            .map(|offer| offer.proof.clone());
        let manifest_id = match proposed
            .as_ref()
            .filter(|proof| matches_inputs(proof, group, root, repository))
        {
            Some(proof) => Some(proof.manifest_id.clone()),
            None => {
                let mut id = None;
                for file in group.files() {
                    let Some(witness) = active(catalog, group.selection(), file.name())? else {
                        return Ok(None);
                    };
                    if id.as_ref().is_some_and(|id| id != &witness.manifest_id) {
                        return Ok(None);
                    }
                    id = Some(witness.manifest_id.clone());
                }
                id
            }
        };
        let Some(manifest) =
            manifests.find(|manifest| Some(&manifest.manifest_id) == manifest_id.as_ref())
        else {
            return Ok(None);
        };
        let Some(complete) = manifest_proof(
            &group.selection().target,
            group.selection().layout,
            manifest,
        ) else {
            return Ok(None);
        };
        let mut proof = complete.clone();
        proof
            .files
            .retain(|file| group.selection().names.contains(&file.name));
        if !matches_inputs(&proof, group, root, repository)
            || proof.validate().is_err()
            || proof
                .files
                .iter()
                .any(|file| catalog.active_witness(&proof, file).is_none())
        {
            return Ok(None);
        }
        Ok(Some((proof, complete)))
    }

    fn identifier(used: &mut BTreeSet<String>) -> Result<String, OffloadCreationRefusal> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).map_err(|_| OffloadCreationRefusal::Entropy)?;
        let id = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        if !used.insert(id.clone()) {
            return Err(OffloadCreationRefusal::Entropy);
        }
        Ok(id)
    }

    fn bounded_metadata(
        proofs: &[OffloadProof],
        writes: &[WitnessWrite],
    ) -> Result<(), OffloadCreationRefusal> {
        if serde_json::to_vec(proofs)
            .map_err(|_| OffloadCreationRefusal::Metadata)?
            .len()
            > MAX_REGISTER_BYTES
        {
            return Err(OffloadCreationRefusal::Metadata);
        }
        let total = writes.iter().try_fold(0usize, |total, write| {
            let proof = &write.proof;
            let event = Event::ArchiveV2 {
                schema_version: 2,
                time: u64::MAX,
                day: proof.target.day.clone(),
                stream: proof.target.stream.clone(),
                segment: solstone_core_format::segment::segment_key(&proof.target.dir)
                    .ok_or(OffloadCreationRefusal::Metadata)?,
                segment_dir: proof.target.dir.clone(),
                layout: proof.layout,
                journal_id: proof.journal_id.clone(),
                manifest_id: proof.manifest_id.clone(),
                repository_id: proof.repository_id.clone(),
                snapshot_id: proof.snapshot_id.clone(),
                files: proof.files.clone(),
            };
            total
                .checked_add(
                    serde_json::to_vec(&event)
                        .map_err(|_| OffloadCreationRefusal::Metadata)?
                        .len(),
                )
                .and_then(|total| total.checked_add(1))
                .ok_or(OffloadCreationRefusal::Metadata)
        })?;
        if total > MAX_REGISTER_BYTES {
            return Err(OffloadCreationRefusal::Metadata);
        }
        Ok(())
    }

    fn durable(receipt: &LedgerWriteReceipt, reused: bool) -> bool {
        (if reused {
            receipt.effect() == LedgerWriteEffect::AlreadyRecorded
        } else {
            matches!(
                receipt.effect(),
                LedgerWriteEffect::Appended { .. } | LedgerWriteEffect::AlreadyRecorded
            )
        }) && receipt.settles()
    }

    pub(super) fn create<'clock>(
        operation: PreparedArchiveOperation<'clock>,
        services: &BackupServices<'clock>,
        selections: &[OffloadOfferSelection],
        today: Date,
        now: Timestamp,
    ) -> OffloadCreation<'clock> {
        let mut resources = Resources::default();
        let planning = (|| {
            operation
                .revalidate()
                .map_err(|error| inputs(OffloadOfferRefusal::Admission(error)))?;
            let catalog = read_complete_catalog(operation.journal_root())
                .map_err(|error| inputs(OffloadOfferRefusal::Catalog(error)))?;
            let register = load_at_root(operation.journal_root())
                .map_err(|error| inputs(OffloadOfferRefusal::Review(error)))?;
            let selections = partition(&catalog, &register, selections)?;
            resources.observed =
                Some(capture_offload_offers(&operation, &selections, today, now).map_err(inputs)?);
            let parent = capacity(
                &operation,
                resources.observed.as_ref().expect("captured inputs"),
            )?;
            copy_inputs(&operation, parent, &mut resources)?;
            Ok(catalog)
        })();
        let catalog = match planning {
            Ok(catalog) => catalog,
            Err(reason) => return fail(Owner::Prepared(Box::new(operation)), resources, reason),
        };
        let admitted =
            match solstone_core_backup_runtime::admit_prepared_archive(operation, services) {
                Ok(admitted) => admitted,
                Err(failure) => {
                    let reason = OffloadCreationRefusal::Archive(failure.error());
                    return fail(
                        Owner::AdmissionFailure(Box::new(failure)),
                        resources,
                        reason,
                    );
                }
            };
        continue_creation(admitted, resources, catalog, today, now)
    }

    pub(super) enum ArchiveBorrow<'borrow, 'clock> {
        Generic(&'borrow mut AdmittedArchive<'clock>),
        Owned(&'borrow mut solstone_core_backup_runtime::OwnedAdmittedArchive),
    }
    impl ArchiveBorrow<'_, '_> {
        fn journal_root(&self) -> &Arc<JournalRoot> {
            match self {
                Self::Generic(archive) => archive.journal_root(),
                Self::Owned(archive) => archive.journal_root(),
            }
        }
        fn repository_id(&self) -> &str {
            match self {
                Self::Generic(archive) => archive.repository_id(),
                Self::Owned(archive) => archive.repository_id(),
            }
        }
        fn archive_owned(
            &mut self,
            sources: &[solstone_core_journal_io::media::ArchiveSource<'_>],
        ) -> Result<ArchiveSnapshot, ArchiveFailure> {
            match self {
                Self::Generic(archive) => archive.archive_owned(sources),
                Self::Owned(archive) => archive.archive_owned(sources),
            }
        }
    }
    pub(super) enum ReadBorrow<'borrow, 'clock> {
        Generic(&'borrow mut ReadOperation<'clock>),
        Owned(&'borrow mut solstone_core_backup::archive_readback::OwnedReadOperation),
    }
    impl ReadBorrow<'_, '_> {
        fn journal_root(&self) -> &Arc<JournalRoot> {
            match self {
                Self::Generic(read) => read.journal_root(),
                Self::Owned(read) => read.journal_root(),
            }
        }
        fn discover_owned_snapshot(
            &mut self,
            snapshot: &str,
            source: &solstone_core_journal_io::media::ArchiveSource<'_>,
        ) -> Result<solstone_core_backup::archive_readback::ExactArchiveFile, ReadbackError>
        {
            match self {
                Self::Generic(read) => read.discover_owned_snapshot(snapshot, source),
                Self::Owned(read) => read.discover_owned_snapshot(snapshot, source),
            }
        }
    }
    pub(super) struct ArchivePlan {
        proofs: Vec<Option<OffloadProof>>,
        writes: Vec<Option<WitnessWrite>>,
        snapshots: Vec<Option<ArchiveSnapshot>>,
        used: BTreeSet<String>,
    }

    pub(super) fn archive_groups(
        mut admitted: ArchiveBorrow<'_, '_>,
        resources: &mut Resources,
        catalog: &JournalCatalog,
    ) -> Result<ArchivePlan, OffloadCreationRefusal> {
        let mut proofs = Vec::new();
        let mut writes = Vec::new();
        let mut snapshots = Vec::new();
        let used: BTreeSet<String> = catalog
            .days
            .iter()
            .flat_map(|day| &day.segments)
            .flat_map(|segment| &segment.history)
            .flat_map(|manifest| {
                std::iter::once(manifest.manifest_id.clone())
                    .chain(manifest.files.iter().map(|file| file.witness_id.clone()))
            })
            .collect();
        for (index, group) in resources
            .observed
            .as_ref()
            .expect("captured inputs")
            .groups()
            .iter()
            .enumerate()
        {
            match reusable(
                catalog,
                group,
                admitted.journal_root(),
                admitted.repository_id(),
            ) {
                Ok(Some((proof, complete))) => {
                    proofs.push(Some(proof));
                    writes.push(Some(WitnessWrite {
                        proof: complete,
                        reused: true,
                    }));
                    snapshots.push(None);
                }
                Ok(None) => {
                    let result = (|| {
                        let sources = resources.copies[index]
                            .iter_mut()
                            .map(PreparedMedia::archive_source)
                            .collect::<std::io::Result<Vec<_>>>()
                            .map_err(|error| inputs(error.into()))?;
                        let result = admitted.archive_owned(&sources);
                        let snapshot = result.as_ref().ok().cloned();
                        let error = result.as_ref().err().map(|failure| failure.error);
                        resources.archives.push(result);
                        if let Some(error) = error {
                            return Err(OffloadCreationRefusal::Archive(error));
                        }
                        Ok(snapshot.expect("successful archive"))
                    })();
                    match result {
                        Ok(snapshot) => {
                            snapshots.push(Some(snapshot));
                            writes.push(None);
                            proofs.push(None);
                        }
                        Err(reason) => {
                            return Err(reason);
                        }
                    }
                }
                Err(reason) => return Err(reason),
            }
        }
        Ok(ArchivePlan {
            proofs,
            writes,
            snapshots,
            used,
        })
    }

    pub(super) fn discover_proofs(
        mut read: ReadBorrow<'_, '_>,
        resources: &mut Resources,
        plan: ArchivePlan,
    ) -> Result<(Vec<OffloadProof>, Vec<WitnessWrite>), OffloadCreationRefusal> {
        let ArchivePlan {
            mut proofs,
            mut writes,
            snapshots,
            mut used,
        } = plan;
        for (index, snapshot) in snapshots.iter().enumerate() {
            let Some(snapshot) = snapshot else {
                continue;
            };
            let group = &resources
                .observed
                .as_ref()
                .expect("captured inputs")
                .groups()[index];
            let mut files = Vec::new();
            for (file, copy) in group.files().iter().zip(&mut resources.copies[index]) {
                let source = copy
                    .archive_source()
                    .map_err(|error| inputs(error.into()))?;
                let exact = read
                    .discover_owned_snapshot(&snapshot.snapshot_id, &source)
                    .map_err(OffloadCreationRefusal::Readback)?;
                files.push(ArchiveFileProof {
                    witness_id: identifier(&mut used)?,
                    name: file.name().into(),
                    bytes: file.observation().digest.size,
                    sha256: file.observation().digest.sha256.clone(),
                    archive_path: exact.archive_path,
                    processing: file.processing().clone(),
                });
            }
            let proof = OffloadProof {
                journal_id: read.journal_root().identity().observation_id(),
                manifest_id: identifier(&mut used)?,
                target: group.selection().target.clone(),
                layout: group.selection().layout,
                repository_id: snapshot.repository_id.clone(),
                snapshot_id: snapshot.snapshot_id.clone(),
                files,
            };
            proof
                .validate()
                .map_err(|error| inputs(OffloadOfferRefusal::Proof(error)))?;
            writes[index] = Some(WitnessWrite {
                proof: proof.clone(),
                reused: false,
            });
            proofs[index] = Some(proof);
        }
        let writes = writes
            .into_iter()
            .map(|write| write.expect("discovered witness"))
            .collect::<Vec<_>>();
        let proofs = proofs
            .into_iter()
            .map(|proof| proof.expect("discovered proof"))
            .collect::<Vec<_>>();
        bounded_metadata(&proofs, &writes)?;
        Ok((proofs, writes))
    }

    pub(super) fn register_metadata(
        root: &Arc<JournalRoot>,
        marks: &[solstone_core_retention::marks::Mark],
    ) -> Result<(), OffloadCreationRefusal> {
        let mut register =
            load_at_root(root).map_err(|error| inputs(OffloadOfferRefusal::Review(error)))?;
        register.version = 2;
        for mark in marks {
            register.marks.insert(mark.id.clone(), mark.clone());
        }
        if serde_json::to_vec_pretty(&register)
            .map_err(|_| OffloadCreationRefusal::Metadata)?
            .len()
            > MAX_REGISTER_BYTES
        {
            return Err(OffloadCreationRefusal::Metadata);
        }
        Ok(())
    }

    pub(super) fn write_witnesses(
        root: &Arc<JournalRoot>,
        resources: &mut Resources,
        writes: Vec<WitnessWrite>,
        now: Timestamp,
    ) -> Result<(), OffloadCreationRefusal> {
        let time = match u64::try_from(now.as_second()) {
            Ok(time) => time,
            Err(_) => {
                return Err(OffloadCreationRefusal::Metadata);
            }
        };
        for write in writes {
            let result = if write.reused {
                confirm_archive_witness(root, &write.proof, time)
            } else {
                append_archive_witness(root, &write.proof, time)
            };
            let accepted = result
                .as_ref()
                .is_ok_and(|receipt| durable(receipt, write.reused));
            resources.ledger.push(result);
            if !accepted {
                return Err(OffloadCreationRefusal::Ledger);
            }
        }
        Ok(())
    }

    pub(super) fn continue_creation<'clock>(
        mut admitted: AdmittedArchive<'clock>,
        mut resources: Resources,
        catalog: JournalCatalog,
        today: Date,
        now: Timestamp,
    ) -> OffloadCreation<'clock> {
        let plan = match archive_groups(
            ArchiveBorrow::Generic(&mut admitted),
            &mut resources,
            &catalog,
        ) {
            Ok(plan) => plan,
            Err(reason) => return fail(Owner::Admitted(Box::new(admitted)), resources, reason),
        };
        let mut read = match admitted.into_read_operation() {
            Ok(read) => read,
            Err(failure) => {
                let reason = OffloadCreationRefusal::Readback(failure.reason());
                return fail(
                    Owner::ReadAdmissionFailure(Box::new(failure)),
                    resources,
                    reason,
                );
            }
        };
        let (proofs, writes) =
            match discover_proofs(ReadBorrow::Generic(&mut read), &mut resources, plan) {
                Ok(discovered) => discovered,
                Err(reason) => return fail(Owner::Read(Box::new(read)), resources, reason),
            };
        let observed = resources.observed.take().expect("captured inputs");
        let verified = match verify_offload_offers(read, observed, proofs, today, now) {
            Ok(verified) => verified,
            Err(failure) => {
                return fail(
                    Owner::VerificationFailure(Box::new(failure)),
                    resources,
                    OffloadCreationRefusal::Verification,
                );
            }
        };
        if let Err(reason) = register_metadata(verified.journal_root(), verified.marks()) {
            return fail(Owner::Verified(Box::new(verified)), resources, reason);
        }
        if let Err(reason) = write_witnesses(verified.journal_root(), &mut resources, writes, now) {
            return fail(Owner::Verified(Box::new(verified)), resources, reason);
        }
        let outcome = publish_offload_offers(verified, today, now);
        record_publication(&mut resources, outcome.publication());
        if resources.reason.is_none()
            && let Some(planning) = &mut resources.planning
        {
            planning.phase = OffloadPlanningPhase::OffersPublished;
        }
        OffloadCreation {
            owner: Owner::Published(Box::new(outcome)),
            resources,
        }
    }

    #[cfg(test)]
    mod publication_tests {
        use solstone_core_retention::mark_mutation::{
            ConditionalResult, MutationStatus, RegisterUncertainty,
        };

        use super::*;

        #[test]
        fn publication_report_preserves_uncertainty_and_candidate() {
            let outcomes = [
                Ok(BoundMutationOutcome::Published(ConditionalResult {
                    status: MutationStatus::Applied,
                    register: Register::empty(),
                })),
                Ok(BoundMutationOutcome::Unchanged(ConditionalResult {
                    status: MutationStatus::AlreadyApplied,
                    register: Register::empty(),
                })),
                Ok(BoundMutationOutcome::PublishedUncertain {
                    candidate: ConditionalResult {
                        status: MutationStatus::Applied,
                        register: Register::empty(),
                    },
                    uncertainty: RegisterUncertainty {
                        durability: Some(std::io::Error::from_raw_os_error(13)),
                        ..RegisterUncertainty::default()
                    },
                }),
                Err(OffloadOfferRefusal::Unproven),
            ];
            for (index, publication) in outcomes.iter().enumerate() {
                let mut resources = Resources::default();
                record_publication(&mut resources, publication);
                assert_eq!(
                    matches!(resources.reason, Some(OffloadCreationRefusal::Publication)),
                    index >= 2,
                );
                if let Ok(BoundMutationOutcome::PublishedUncertain {
                    candidate,
                    uncertainty,
                }) = publication
                {
                    assert_eq!(candidate.status, MutationStatus::Applied);
                    assert_eq!(candidate.register, Register::empty());
                    assert_eq!(
                        uncertainty
                            .durability
                            .as_ref()
                            .and_then(std::io::Error::raw_os_error),
                        Some(13),
                    );
                }
            }
        }
    }

    #[cfg(all(test, unix, feature = "full-tests"))]
    mod tests;
}
