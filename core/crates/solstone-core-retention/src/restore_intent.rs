// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict recovery evidence and decisions. No deserialized value grants native
//! publication, register mutation, helper quiescence, or cleanup authority.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive_proof::{ArchiveFileProof, OffloadProof};
use crate::mark_mutation::PlannedMarkMutation;
use crate::{Mark, Register, RemovalClass, Target};
use serde::{Deserialize, Serialize};
use solstone_core_format::segment::segment_key;
use solstone_core_journal_io::activity_obligations::{MAX_INTENT_BYTES, intent_filename};
use solstone_core_journal_io::journal_root::is_canonical_identity_observation;
use solstone_core_journal_io::media::{
    Cleanup, Confirmation, Durability, MediaDigest, MediaIdentity, MediaObservation,
    PublicationEffect, PublicationReceipt,
};
use solstone_core_journal_io::paths::{DEFAULT_STREAM, Segment, SegmentLayout};

use crate::archive_catalog::{
    ArchiveWitness, LegacyRestoreBinding, LegacyRestoreOperation, RestoreDisposition,
    RestoreFileV2, RestoreOperation, SegmentCatalog,
};

mod abandoned;
pub use abandoned::{
    AbandonedRestore, AbandonedRestoreHold, AbandonedRestoreRelease, recover_abandoned_restores,
};

const INTENT_VERSION: u32 = 1;

/// The leaf of a restore's or legacy probe's download stage, which holds only
/// backup bytes. No other producer creates it, and
/// with no extension it can never be an archived media name, so a stage
/// holding only this file is a restore's unpublished copy even without an
/// intent. Earlier restores recorded the shared `payload` leaf.
pub const RESTORE_DOWNLOAD_LEAF: &str = "restore-download";

struct BoundedDocument {
    bytes: Vec<u8>,
    exceeded: bool,
}

impl io::Write for BoundedDocument {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(buffer.len())
            .is_none_or(|length| length > MAX_INTENT_BYTES)
        {
            self.exceeded = true;
            return Err(io::Error::other("restore intent exceeds record limit"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepairHold {
    UnreadableIntent,
    OversizedIntent,
    InvalidIntent,
    UnboundWitness,
    DestinationChanged,
    DegradedCatalog,
    CatalogChanged,
    OperationCollision,
    MarkChanged,
    UncertainContent,
    PublishedContentMissing,
    ImmutableEvent,
    CounterOverflow,
    ClaimsUnavailable,
}

fn plain_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
}

fn same_platform(left: MediaIdentity, right: MediaIdentity) -> bool {
    matches!(
        (left, right),
        (MediaIdentity::Unix { .. }, MediaIdentity::Unix { .. })
            | (MediaIdentity::Windows { .. }, MediaIdentity::Windows { .. })
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreLocator {
    pub target: Target,
    pub segment: String,
    pub layout: SegmentLayout,
}

impl RestoreLocator {
    pub fn from_segment(day: &str, segment: &Segment) -> Result<Self, RepairHold> {
        let identity = segment
            .locator_identity()
            .map_err(|_| RepairHold::InvalidIntent)?;
        let locator = Self {
            target: Target {
                day: day.into(),
                stream: identity.stream.into(),
                dir: identity.name.into(),
            },
            segment: identity.key.into(),
            layout: identity.layout,
        };
        locator.validate()?;
        Ok(locator)
    }

    fn validate(&self) -> Result<(), RepairHold> {
        if solstone_core_time::parse_day(&self.target.day).is_none()
            || self.target.day.len() != 8
            || !plain_name(&self.target.stream)
            || !plain_name(&self.target.dir)
            || segment_key(&self.target.dir).as_ref() != Some(&self.segment)
            || (self.layout == SegmentLayout::Direct && self.target.stream != DEFAULT_STREAM)
        {
            return Err(RepairHold::InvalidIntent);
        }
        Ok(())
    }

    pub fn relative_segment(&self) -> PathBuf {
        let day = Path::new("chronicle").join(&self.target.day);
        match self.layout {
            SegmentLayout::Direct => day.join(&self.target.dir),
            SegmentLayout::Named => day.join(&self.target.stream).join(&self.target.dir),
        }
    }
}

/// Independent destination evidence, never compared to archived source provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationObservation {
    pub root: String,
    pub parent: MediaIdentity,
    pub segment: MediaIdentity,
}

/// Diagnostic owned-stage evidence only. Restart cannot reconstruct ownership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageEvidence {
    pub parent: MediaIdentity,
    pub directory: MediaIdentity,
    pub leaf: MediaIdentity,
    pub stage_name: String,
    pub leaf_name: String,
    pub boot_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkRetirement {
    NoMark,
    Exact { plan: Box<PlannedMarkMutation> },
    PreserveLegacy { expected: Option<Box<Mark>> },
}

fn proof_file(witness: &ArchiveWitness) -> Result<ArchiveFileProof, RepairHold> {
    Ok(ArchiveFileProof {
        witness_id: witness.witness_id.clone(),
        name: witness.file.name.clone(),
        bytes: witness.file.bytes,
        sha256: witness.file.sha256.clone(),
        archive_path: witness
            .archive_path
            .clone()
            .ok_or(RepairHold::UnboundWitness)?,
        processing: witness
            .processing
            .clone()
            .ok_or(RepairHold::UnboundWitness)?,
    })
}

fn validate_witness(locator: &RestoreLocator, witness: &ArchiveWitness) -> Result<(), RepairHold> {
    if witness.layout != Some(locator.layout)
        || witness.segment_dir.as_ref() != Some(&locator.target.dir)
    {
        return Err(RepairHold::UnboundWitness);
    }
    OffloadProof {
        journal_id: witness
            .journal_id
            .clone()
            .ok_or(RepairHold::UnboundWitness)?,
        manifest_id: witness.manifest_id.clone(),
        target: locator.target.clone(),
        layout: locator.layout,
        repository_id: witness
            .repository_id
            .clone()
            .ok_or(RepairHold::UnboundWitness)?,
        snapshot_id: witness.snapshot_id.clone(),
        files: vec![proof_file(witness)?],
    }
    .validate()
    .map_err(|_| RepairHold::InvalidIntent)
}

impl MarkRetirement {
    fn plan(
        locator: &RestoreLocator,
        witness: &ArchiveWitness,
        mark: Option<&Mark>,
    ) -> Result<Self, RepairHold> {
        let Some(mark) = mark else {
            return Ok(Self::NoMark);
        };
        let offer = mark.offload.as_ref().ok_or(RepairHold::UnboundWitness)?;
        if mark.class != RemovalClass::OffloadRawRelease
            || mark.target != locator.target
            || offer.proof.layout != locator.layout
            || offer.proof.manifest_id != witness.manifest_id
            || Some(&offer.proof.repository_id) != witness.repository_id.as_ref()
            || offer.proof.snapshot_id != witness.snapshot_id
            || Some(&offer.proof.journal_id) != witness.journal_id.as_ref()
            || !offer.proof.files.contains(&proof_file(witness)?)
        {
            return Err(RepairHold::MarkChanged);
        }
        let names = mark
            .proposal
            .names
            .iter()
            .filter(|name| **name != witness.file.name)
            .cloned()
            .collect::<Vec<_>>();
        let plan =
            PlannedMarkMutation::remainder(mark, &names).map_err(|_| RepairHold::InvalidIntent)?;
        Ok(Self::Exact {
            plan: Box::new(plan),
        })
    }

    pub fn expected(&self) -> Option<&Mark> {
        match self {
            Self::NoMark => None,
            Self::Exact { plan } => Some(plan.expected()),
            Self::PreserveLegacy { expected } => expected.as_deref(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Obligation {
    Pending,
    Complete,
    NotRequired,
}

/// A lossless snapshot of the receipt's independent facts, including failures
/// after a real native mutation. `error` is diagnostic, never an effect classifier.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationFacts {
    pub effect: PublicationEffect,
    pub confirmation: Confirmation,
    pub file_durability: Durability,
    pub parent_durability: Durability,
    pub cleanup: Cleanup,
    pub identity: Option<MediaIdentity>,
    pub error: Option<String>,
}

impl PublicationFacts {
    pub fn matches_ignoring_cleanup(&self, other: &Self) -> bool {
        self.effect == other.effect
            && self.confirmation == other.confirmation
            && self.file_durability == other.file_durability
            && self.parent_durability == other.parent_durability
            && self.identity == other.identity
            && self.error == other.error
    }

    pub fn is_valid_successor(&self, next: &Self) -> bool {
        if self.effect != next.effect
            || self.confirmation != next.confirmation
            || self.cleanup != next.cleanup
            || self.identity != next.identity
            || self.error != next.error
        {
            return false;
        }
        let file_valid = self.file_durability == next.file_durability;
        let parent_valid = self.parent_durability == next.parent_durability
            || (self.parent_durability == Durability::Unproven
                && next.parent_durability == Durability::Synced);
        file_valid && parent_valid
    }
}

impl From<&PublicationReceipt> for PublicationFacts {
    fn from(receipt: &PublicationReceipt) -> Self {
        Self {
            effect: receipt.effect,
            confirmation: receipt.confirmation,
            file_durability: receipt.file_durability,
            parent_durability: receipt.parent_durability,
            cleanup: receipt.cleanup,
            identity: receipt.identity,
            error: receipt.error.as_ref().map(ToString::to_string),
        }
    }
}

/// A failed state transition cannot erase the native invocation's effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutcomeRefusal {
    pub hold: RepairHold,
    pub facts: PublicationFacts,
    pub effects: AttemptEffects,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedContent {
    pub identity: MediaIdentity,
    pub digest: MediaDigest,
}

impl From<MediaObservation> for ConfirmedContent {
    fn from(observed: MediaObservation) -> Self {
        Self {
            identity: observed.identity,
            digest: observed.digest,
        }
    }
}

/// Whole values are serialized only as bounded small-document evidence. The
/// native metadata adapter must confirm exact bytes before any publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreIntent {
    version: u32,
    pub locator: RestoreLocator,
    pub witness: ArchiveWitness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_binding: Option<LegacyRestoreBinding>,
    /// Original live-owner observations. Decoding these cannot recreate admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_binding:
        Option<solstone_core_journal_io::archive_recovery::ArchiveRecoveryBinding>,
    pub destination: DestinationObservation,
    pub stage: StageEvidence,
    pub retirement: MarkRetirement,
    pub event: RestoreOperation,
    pub outcome: Option<PublicationFacts>,
    pub confirmed: Option<ConfirmedContent>,
    pub marker: Obligation,
    pub event_recorded: Obligation,
    pub register: Obligation,
    pub cleanup: Obligation,
}

impl RestoreIntent {
    pub fn new(
        locator: RestoreLocator,
        witness: ArchiveWitness,
        destination: DestinationObservation,
        stage: StageEvidence,
        mark: Option<&Mark>,
    ) -> Result<Self, RepairHold> {
        let mut entropy = [0u8; 16];
        getrandom::fill(&mut entropy).map_err(|_| RepairHold::InvalidIntent)?;
        Self::with_id(
            format!("{:032x}", u128::from_le_bytes(entropy)),
            locator,
            witness,
            destination,
            stage,
            mark,
        )
    }

    fn with_id(
        id: String,
        locator: RestoreLocator,
        witness: ArchiveWitness,
        destination: DestinationObservation,
        stage: StageEvidence,
        mark: Option<&Mark>,
    ) -> Result<Self, RepairHold> {
        let retirement = MarkRetirement::plan(&locator, &witness, mark)?;
        Self::initialize(id, locator, witness, destination, stage, retirement, None)
    }

    pub fn new_legacy(
        binding: LegacyRestoreBinding,
        witness: ArchiveWitness,
        destination: DestinationObservation,
        stage: StageEvidence,
        mark: Option<&Mark>,
    ) -> Result<Self, RepairHold> {
        let mut entropy = [0u8; 16];
        getrandom::fill(&mut entropy).map_err(|_| RepairHold::InvalidIntent)?;
        let locator = RestoreLocator {
            segment: segment_key(&binding.target.dir).ok_or(RepairHold::InvalidIntent)?,
            target: binding.target.clone(),
            layout: binding.layout,
        };
        Self::initialize(
            format!("{:032x}", u128::from_le_bytes(entropy)),
            locator,
            witness,
            destination,
            stage,
            MarkRetirement::PreserveLegacy {
                expected: mark.cloned().map(Box::new),
            },
            Some(binding),
        )
    }

    fn initialize(
        id: String,
        locator: RestoreLocator,
        witness: ArchiveWitness,
        destination: DestinationObservation,
        stage: StageEvidence,
        retirement: MarkRetirement,
        legacy_binding: Option<LegacyRestoreBinding>,
    ) -> Result<Self, RepairHold> {
        let intent = Self {
            version: INTENT_VERSION,
            event: RestoreOperation {
                operation_id: id,
                files: vec![RestoreFileV2 {
                    witness_id: witness.witness_id.clone(),
                    name: witness.file.name.clone(),
                    disposition: RestoreDisposition::Unattempted,
                }],
            },
            locator,
            witness,
            legacy_binding,
            recovery_binding: None,
            destination,
            stage,
            register: if matches!(retirement, MarkRetirement::Exact { .. }) {
                Obligation::Pending
            } else {
                Obligation::NotRequired
            },
            retirement,
            outcome: None,
            confirmed: None,
            marker: Obligation::Pending,
            event_recorded: Obligation::Pending,
            cleanup: Obligation::Pending,
        };
        intent.validate()?;
        Ok(intent)
    }

    pub fn expected_digest(&self) -> MediaDigest {
        MediaDigest {
            size: self.witness.file.bytes,
            sha256: self.witness.file.sha256.clone(),
        }
    }

    pub fn legacy_operation(&self) -> Option<LegacyRestoreOperation> {
        self.legacy_binding
            .as_ref()
            .map(|binding| LegacyRestoreOperation {
                adoption_id: binding.adoption_id.clone(),
                operation: self.event.clone(),
            })
    }

    pub fn validate(&self) -> Result<(), RepairHold> {
        self.locator.validate()?;
        if let Some(binding) = &self.recovery_binding {
            let repository = self
                .legacy_binding
                .as_ref()
                .map(|binding| binding.repository_id.as_str())
                .or(self.witness.repository_id.as_deref());
            if repository != Some(binding.repository_id()) {
                return Err(RepairHold::InvalidIntent);
            }
        }
        match (&self.legacy_binding, &self.retirement) {
            (None, MarkRetirement::NoMark | MarkRetirement::Exact { .. }) => {
                validate_witness(&self.locator, &self.witness)?;
                if self.retirement
                    != MarkRetirement::plan(
                        &self.locator,
                        &self.witness,
                        self.retirement.expected(),
                    )?
                {
                    return Err(RepairHold::InvalidIntent);
                }
            }
            (Some(binding), MarkRetirement::PreserveLegacy { expected }) => {
                binding.validate().map_err(|_| RepairHold::InvalidIntent)?;
                if binding.target != self.locator.target
                    || binding.layout != self.locator.layout
                    || !binding.legacy_manifest.files.contains(&self.witness)
                    || self.register != Obligation::NotRequired
                    || expected.as_ref().is_some_and(|mark| {
                        mark.class != RemovalClass::OffloadRawRelease
                            || mark.target != self.locator.target
                            || !mark.proposal.names.contains(&self.witness.file.name)
                            || mark.proposal.names.iter().any(|name| {
                                solstone_core_journal_io::name_admission::check_portable_component(
                                    name,
                                )
                                .is_err()
                            })
                    })
                {
                    return Err(RepairHold::InvalidIntent);
                }
            }
            _ => return Err(RepairHold::InvalidIntent),
        }
        if self.version != INTENT_VERSION
            || intent_filename(&self.event.operation_id).is_none()
            || !is_canonical_identity_observation(&self.destination.root)
            || self.destination.root.starts_with("native-root-v1:unix:")
                != matches!(self.destination.parent, MediaIdentity::Unix { .. })
            || !same_platform(self.destination.parent, self.destination.segment)
            || !same_platform(self.destination.parent, self.stage.directory)
            || !same_platform(self.destination.parent, self.stage.leaf)
            || self.stage.parent != self.destination.parent
            || !plain_name(&self.stage.stage_name)
            || !self.stage.stage_name.starts_with(".tmp-offload-")
            || !matches!(
                self.stage.leaf_name.as_str(),
                RESTORE_DOWNLOAD_LEAF | "payload"
            )
            || self
                .stage
                .boot_id
                .as_ref()
                .is_some_and(|id| id.is_empty() || id.chars().any(char::is_control))
            || self.event.files.len() != 1
            || self.event.files.first().is_none_or(|file| {
                file.witness_id != self.witness.witness_id || file.name != self.witness.file.name
            })
            || matches!(self.event_recorded, Obligation::NotRequired)
            || matches!(self.cleanup, Obligation::NotRequired)
        {
            return Err(RepairHold::InvalidIntent);
        }
        let no_effect_complete = self.no_effect_outcome()
            && self.event_recorded == Obligation::Complete
            && self.cleanup == Obligation::Complete
            && self.marker == Obligation::NotRequired
            && self.register == Obligation::NotRequired;
        if (self.marker == Obligation::NotRequired && !no_effect_complete)
            || (!matches!(self.retirement, MarkRetirement::Exact { .. })
                != matches!(self.register, Obligation::NotRequired)
                && !no_effect_complete)
            || self
                .confirmed
                .as_ref()
                .is_some_and(|observed| observed.digest != self.expected_digest())
        {
            return Err(RepairHold::InvalidIntent);
        }
        let disposition = self.disposition();
        let successful = matches!(
            disposition,
            RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
        );
        let changed_day = self.outcome.as_ref().is_some_and(|outcome| {
            matches!(
                outcome.effect,
                PublicationEffect::Published | PublicationEffect::AlreadyPresent
            )
        });
        if successful && self.confirmed.is_none()
            || self.marker == Obligation::Complete && !successful && !changed_day
            || self.register == Obligation::Complete
                && (self.marker != Obligation::Complete
                    || self.event_recorded != Obligation::Complete
                    || !successful)
        {
            return Err(RepairHold::InvalidIntent);
        }
        match &self.outcome {
            None if matches!(
                disposition,
                RestoreDisposition::Published | RestoreDisposition::Unverified
            ) =>
            {
                return Err(RepairHold::InvalidIntent);
            }
            Some(outcome)
                if outcome.effect == PublicationEffect::Published
                    && outcome.identity != Some(self.stage.leaf) =>
            {
                return Err(RepairHold::InvalidIntent);
            }
            Some(outcome)
                if matches!(
                    outcome.effect,
                    PublicationEffect::Published | PublicationEffect::AlreadyPresent
                ) && outcome.identity.is_none() =>
            {
                return Err(RepairHold::InvalidIntent);
            }
            Some(outcome)
                if matches!(
                    outcome.effect,
                    PublicationEffect::Conflict | PublicationEffect::NotPublished
                ) && (outcome.identity.is_some()
                    || outcome.confirmation == Confirmation::Confirmed) =>
            {
                return Err(RepairHold::InvalidIntent);
            }
            _ => {}
        }
        if disposition == RestoreDisposition::Published
            && !self.outcome.as_ref().is_some_and(|outcome| {
                outcome.effect == PublicationEffect::Published
                    && outcome.confirmation == Confirmation::Confirmed
                    && self
                        .confirmed
                        .as_ref()
                        .is_some_and(|content| Some(content.identity) == outcome.identity)
            })
        {
            return Err(RepairHold::InvalidIntent);
        }
        if disposition == RestoreDisposition::Unverified
            && !self.outcome.as_ref().is_some_and(|outcome| {
                matches!(
                    outcome.effect,
                    PublicationEffect::Published | PublicationEffect::AlreadyPresent
                ) && outcome.confirmation == Confirmation::Unverified
            })
        {
            return Err(RepairHold::InvalidIntent);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, RepairHold> {
        self.validate()?;
        let mut document = BoundedDocument {
            bytes: Vec::new(),
            exceeded: false,
        };
        serde_json::to_writer(&mut document, self).map_err(|_| {
            if document.exceeded {
                RepairHold::OversizedIntent
            } else {
                RepairHold::InvalidIntent
            }
        })?;
        Ok(document.bytes)
    }

    /// Pure evidence monotonicity. This grants no authority to complete an
    /// obligation: the orchestrator still owes the actual owning receipt.
    pub fn validate_successor(&self, next: &Self) -> Result<(), RepairHold> {
        self.validate()?;
        next.validate()?;
        if self.version != next.version
            || self.event.operation_id != next.event.operation_id
            || self.locator != next.locator
            || self.witness != next.witness
            || self.legacy_binding != next.legacy_binding
            || self.recovery_binding != next.recovery_binding
            || self.destination != next.destination
            || self.stage != next.stage
            || self.retirement != next.retirement
            || self.outcome.as_ref().is_some_and(|facts| {
                match &next.outcome {
                    Some(next_facts) => !facts.is_valid_successor(next_facts),
                    None => true,
                }
            })
            || self.confirmed.as_ref().is_some_and(|facts| next.confirmed.as_ref() != Some(facts))
            || self.event_recorded == Obligation::Complete && self.event != next.event
            || self.event_recorded == Obligation::Complete
                && (self.outcome != next.outcome || self.confirmed != next.confirmed)
            // Record only the exact prospective event already persisted.
            || next.event_recorded == Obligation::Complete && self.event != next.event
        {
            return Err(RepairHold::ImmutableEvent);
        }
        let first_changed_day = self.outcome.is_none()
            && next.outcome.as_ref().is_some_and(|facts| {
                matches!(
                    facts.effect,
                    PublicationEffect::Published | PublicationEffect::AlreadyPresent
                )
            });
        if first_changed_day && next.marker != Obligation::Pending {
            return Err(RepairHold::ImmutableEvent);
        }
        for (before, after) in [
            (self.event_recorded, next.event_recorded),
            (self.register, next.register),
            (self.cleanup, next.cleanup),
        ] {
            if before != Obligation::Pending && before != after {
                return Err(RepairHold::ImmutableEvent);
            }
        }
        if self.marker != Obligation::Pending
            && self.marker != next.marker
            && !(self.marker == Obligation::Complete && first_changed_day)
        {
            return Err(RepairHold::ImmutableEvent);
        }
        Ok(())
    }

    /// The adapter supplies a capped, read-only read. Failed reads and torn data
    /// remain a hold; this function never rewrites or repairs their bytes.
    pub fn decode(bytes: io::Result<&[u8]>) -> Result<Self, RepairHold> {
        let bytes = bytes.map_err(|_| RepairHold::UnreadableIntent)?;
        if bytes.len() > MAX_INTENT_BYTES {
            return Err(RepairHold::OversizedIntent);
        }
        let intent: Self = serde_json::from_slice(bytes).map_err(|_| RepairHold::InvalidIntent)?;
        intent.validate()?;
        Ok(intent)
    }

    fn disposition(&self) -> RestoreDisposition {
        self.event
            .files
            .first()
            .map_or(RestoreDisposition::Unattempted, |file| file.disposition)
    }

    /// Evidence classification only; cleanup still needs the actual native owner.
    pub fn no_effect_outcome(&self) -> bool {
        self.confirmed.is_none()
            && self.outcome.as_ref().is_some_and(|outcome| {
                outcome.identity.is_none()
                    && outcome.confirmation == Confirmation::Unverified
                    && matches!(
                        (outcome.effect, self.disposition()),
                        (PublicationEffect::Conflict, RestoreDisposition::Conflict)
                            | (
                                PublicationEffect::NotPublished,
                                RestoreDisposition::Unattempted
                            )
                    )
            })
    }

    /// Snapshot a single native attempt. Callers must persist this even if the
    /// final-name confirmation failed; the receipt's actual effect is retained.
    pub fn record_outcome(
        &mut self,
        receipt: &PublicationReceipt,
    ) -> Result<AttemptEffects, OutcomeRefusal> {
        let facts = PublicationFacts::from(receipt);
        let effects = AttemptEffects::from_receipt(receipt, self.witness.file.bytes);
        self.record_outcome_checked(receipt)
            .map_err(|hold| OutcomeRefusal {
                hold,
                facts,
                effects,
            })
    }

    /// Advance parent directory durability monotonically from Unproven to Synced
    /// before event publication completes.
    pub fn advance_parent_durability(&mut self) -> Result<(), RepairHold> {
        if self.event_recorded == Obligation::Complete {
            return Err(RepairHold::ImmutableEvent);
        }
        let Some(outcome) = &self.outcome else {
            return Err(RepairHold::InvalidIntent);
        };
        if outcome.parent_durability == Durability::Synced {
            return Ok(());
        }
        let mut updated_outcome = outcome.clone();
        updated_outcome.parent_durability = Durability::Synced;
        let mut updated = self.clone();
        updated.outcome = Some(updated_outcome);
        self.validate_successor(&updated)?;
        *self = updated;
        Ok(())
    }

    fn record_outcome_checked(
        &mut self,
        receipt: &PublicationReceipt,
    ) -> Result<AttemptEffects, RepairHold> {
        if self.outcome.is_some() || self.event_recorded == Obligation::Complete {
            return Err(RepairHold::ImmutableEvent);
        }
        let disposition = match (receipt.effect, receipt.confirmation) {
            (PublicationEffect::Published, Confirmation::Confirmed) => {
                RestoreDisposition::Published
            }
            (PublicationEffect::AlreadyPresent, Confirmation::Confirmed) => {
                RestoreDisposition::AlreadyPresent
            }
            (PublicationEffect::Conflict, _) => RestoreDisposition::Conflict,
            (PublicationEffect::NotPublished, _) => RestoreDisposition::Unattempted,
            _ => RestoreDisposition::Unverified,
        };
        let mut updated = self.clone();
        updated.outcome = Some(PublicationFacts::from(receipt));
        updated.event.files = vec![RestoreFileV2 {
            witness_id: self.witness.witness_id.clone(),
            name: self.witness.file.name.clone(),
            disposition,
        }];
        if matches!(
            disposition,
            RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
        ) {
            updated.confirmed = Some(ConfirmedContent {
                identity: receipt.identity.ok_or(RepairHold::InvalidIntent)?,
                digest: self.expected_digest(),
            });
        }
        if receipt.cleanup == Cleanup::Complete {
            updated.cleanup = Obligation::Complete;
        }
        if matches!(
            receipt.effect,
            PublicationEffect::Published | PublicationEffect::AlreadyPresent
        ) {
            // A new native attempt cannot inherit a completed invalidation from
            // an earlier observation under this still-unrecorded operation.
            updated.marker = Obligation::Pending;
        }
        updated.validate()?;
        let effects = AttemptEffects::from_receipt(receipt, self.witness.file.bytes);
        *self = updated;
        Ok(effects)
    }

    /// Before event append, persist the exact prospective recovery event. A
    /// recorded event is immutable, even when matching bytes are found later.
    pub fn prepare_recovery_event(
        &mut self,
        decision: &RecoveryDecision,
    ) -> Result<(), RepairHold> {
        if self.event_recorded == Obligation::Complete || decision.event_recorded {
            return Err(RepairHold::ImmutableEvent);
        }
        if decision.event.operation_id != self.event.operation_id
            || decision.event.files.first().is_none_or(|file| {
                file.witness_id != self.witness.witness_id || file.name != self.witness.file.name
            })
        {
            return Err(RepairHold::InvalidIntent);
        }
        let mut updated = self.clone();
        updated.event = decision.event.clone();
        updated.confirmed = decision.confirmed.clone();
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    pub fn output_paths(&self, journal: &Path) -> Vec<PathBuf> {
        let segment = journal.join(self.locator.relative_segment());
        let mut paths = vec![
            segment
                .join(&self.witness.file.name)
                .with_extension("jsonl"),
        ];
        if let Some(mark) = self.retirement.expected() {
            paths.extend(
                mark.proposal
                    .names
                    .iter()
                    .map(|name| segment.join(name).with_extension("jsonl")),
            );
        }
        paths.sort();
        paths.dedup();
        paths
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AttemptEffects {
    pub files: u64,
    pub bytes: u64,
}

impl AttemptEffects {
    fn from_receipt(receipt: &PublicationReceipt, bytes: u64) -> Self {
        if receipt.effect == PublicationEffect::Published {
            Self { files: 1, bytes }
        } else {
            Self::default()
        }
    }
    pub fn checked_add(self, other: Self) -> Result<Self, RepairHold> {
        Ok(Self {
            files: self
                .files
                .checked_add(other.files)
                .ok_or(RepairHold::CounterOverflow)?,
            bytes: self
                .bytes
                .checked_add(other.bytes)
                .ok_or(RepairHold::CounterOverflow)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CurrentContent {
    Missing,
    Present(ConfirmedContent),
    Conflict,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationDecision {
    Missing,
    AlreadyPresent,
    Conflict,
    HistoricalOnly,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegisterDecision {
    NoMark,
    AwaitingSuccessfulEvent,
    AwaitingMarker,
    Ready(Box<PlannedMarkMutation>),
    AlreadyApplied,
    Superseded,
    Collision,
    Complete,
}

/// Data only. In particular Missing/Ready cannot be used as capabilities after
/// releasing exclusion. Native admission and owning CAS must run at execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryDecision {
    pub publication: PublicationDecision,
    pub event: RestoreOperation,
    pub event_recorded: bool,
    pub confirmed: Option<ConfirmedContent>,
    pub marker: Obligation,
    pub register: RegisterDecision,
    pub cleanup: Obligation,
    /// Replay never recounts an earlier native publication.
    pub newly_published: AttemptEffects,
}

fn related_mark<'a>(
    intent: &RestoreIntent,
    register: &'a Register,
) -> impl Iterator<Item = &'a Mark> {
    register.marks.values().filter(|mark| {
        mark.class == RemovalClass::OffloadRawRelease
            && mark.target == intent.locator.target
            && mark.proposal.names.contains(&intent.witness.file.name)
    })
}

/// Equality checks only, not a second register mutation implementation. The
/// owning full-Mark conditional API remains the sole executor and is held.
fn register_decision(
    intent: &RestoreIntent,
    register: &Register,
    event_recorded: bool,
    successful: bool,
) -> RegisterDecision {
    let MarkRetirement::Exact { plan } = &intent.retirement else {
        return RegisterDecision::NoMark;
    };
    if !event_recorded || !successful {
        return RegisterDecision::AwaitingSuccessfulEvent;
    }
    if intent.marker != Obligation::Complete {
        return RegisterDecision::AwaitingMarker;
    }
    if intent.register == Obligation::Complete {
        return RegisterDecision::Complete;
    }
    let original = register.marks.get(&plan.expected().id);
    let replacement = plan.replacement();
    if original.is_none()
        && replacement.is_some_and(|mark| register.marks.get(&mark.id) == Some(mark))
    {
        return RegisterDecision::AlreadyApplied;
    }
    if original != Some(plan.expected()) {
        return RegisterDecision::Superseded;
    }
    if replacement
        .is_some_and(|mark| mark.id != plan.expected().id && register.marks.contains_key(&mark.id))
    {
        return RegisterDecision::Collision;
    }
    RegisterDecision::Ready(plan.clone())
}

pub fn inspect_recovery(
    intent: &RestoreIntent,
    destination: &DestinationObservation,
    catalog: &SegmentCatalog,
    register: &Register,
    content: CurrentContent,
) -> Result<RecoveryDecision, RepairHold> {
    intent.validate()?;
    if destination != &intent.destination {
        return Err(RepairHold::DestinationChanged);
    }
    if catalog.degraded() {
        return Err(RepairHold::DegradedCatalog);
    }
    if catalog.day != intent.locator.target.day
        || catalog.stream != intent.locator.target.stream
        || catalog.segment != intent.locator.segment
    {
        return Err(RepairHold::CatalogChanged);
    }
    if let Some(binding) = &intent.legacy_binding {
        let mut bindings = catalog
            .restore_bindings
            .iter()
            .filter(|current| current.adoption_id == binding.adoption_id);
        if bindings.next() != Some(binding) || bindings.next().is_some() {
            return Err(RepairHold::CatalogChanged);
        }
        let mut manifests = catalog
            .history
            .iter()
            .filter(|manifest| manifest.manifest_id == binding.legacy_manifest.manifest_id);
        if manifests.next() != Some(&binding.legacy_manifest) || manifests.next().is_some() {
            return Err(RepairHold::CatalogChanged);
        }
        // Restore-only binding never retires a proposal. Even recorded history
        // must preserve the entire selected current Mark or original absence.
        let mut marks = related_mark(intent, register);
        if marks.next() != intent.retirement.expected() || marks.next().is_some() {
            return Err(RepairHold::MarkChanged);
        }
    }
    let history = catalog
        .history
        .iter()
        .flat_map(|manifest| &manifest.files)
        .filter(|w| w.witness_id == intent.witness.witness_id)
        .collect::<Vec<_>>();
    if history.len() != 1 || history.first().copied() != Some(&intent.witness) {
        return Err(RepairHold::CatalogChanged);
    }
    let recorded = catalog
        .restores
        .iter()
        .filter(|operation| operation.operation_id == intent.event.operation_id)
        .collect::<Vec<_>>();
    if recorded.len() > 1
        || recorded
            .first()
            .is_some_and(|event| **event != intent.event)
    {
        return Err(RepairHold::OperationCollision);
    }
    let event_recorded = !recorded.is_empty();
    let mut legacy_records = catalog
        .legacy_restores
        .iter()
        .filter(|record| record.operation.operation_id == intent.event.operation_id);
    let legacy_record = legacy_records.next();
    if legacy_records.next().is_some()
        || match intent.legacy_operation() {
            Some(expected) => match legacy_record {
                Some(actual) => actual != &expected || !event_recorded,
                None => event_recorded,
            },
            None => legacy_record.is_some(),
        }
    {
        return Err(RepairHold::OperationCollision);
    }
    if intent.event_recorded == Obligation::Complete && !event_recorded {
        return Err(RepairHold::OperationCollision);
    }
    if event_recorded {
        // Successful events already retired this witness. Validate exact history
        // rather than requiring it active again, and leave later generations alone.
        let successful = matches!(
            intent.disposition(),
            RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
        ) && intent.confirmed.is_some();
        return Ok(RecoveryDecision {
            publication: PublicationDecision::HistoricalOnly,
            event: intent.event.clone(),
            event_recorded: true,
            confirmed: intent.confirmed.clone(),
            marker: intent.marker,
            register: register_decision(intent, register, true, successful),
            cleanup: intent.cleanup,
            newly_published: AttemptEffects::default(),
        });
    }
    if !catalog.active.contains(&intent.witness) {
        return Err(RepairHold::CatalogChanged);
    }
    match &intent.retirement {
        MarkRetirement::NoMark if related_mark(intent, register).next().is_some() => {
            return Err(RepairHold::MarkChanged);
        }
        MarkRetirement::Exact { plan }
            if register.marks.get(&plan.expected().id) != Some(plan.expected()) =>
        {
            return Err(RepairHold::MarkChanged);
        }
        MarkRetirement::Exact { plan }
            if plan.replacement().is_some_and(|mark| {
                mark.id != plan.expected().id && register.marks.contains_key(&mark.id)
            }) =>
        {
            return Err(RepairHold::MarkChanged);
        }
        _ => {}
    }
    let mut event = intent.event.clone();
    let (publication, confirmed) = match content {
        CurrentContent::Uncertain => return Err(RepairHold::UncertainContent),
        CurrentContent::Conflict => {
            event.files = vec![RestoreFileV2 {
                witness_id: intent.witness.witness_id.clone(),
                name: intent.witness.file.name.clone(),
                disposition: RestoreDisposition::Conflict,
            }];
            (PublicationDecision::Conflict, None)
        }
        CurrentContent::Missing => {
            if intent.outcome.as_ref().is_some_and(|outcome| {
                matches!(
                    outcome.effect,
                    PublicationEffect::Published | PublicationEffect::AlreadyPresent
                )
            }) {
                return Err(RepairHold::PublishedContentMissing);
            }
            event.files = vec![RestoreFileV2 {
                witness_id: intent.witness.witness_id.clone(),
                name: intent.witness.file.name.clone(),
                disposition: RestoreDisposition::Unattempted,
            }];
            (PublicationDecision::Missing, None)
        }
        CurrentContent::Present(observed) => {
            if observed.digest != intent.expected_digest() {
                return Err(RepairHold::UncertainContent);
            }
            let disposition = if intent.outcome.as_ref().is_some_and(|outcome| {
                outcome.effect == PublicationEffect::Published
                    && outcome.confirmation == Confirmation::Confirmed
                    && outcome.identity == Some(observed.identity)
            }) {
                RestoreDisposition::Published
            } else {
                RestoreDisposition::AlreadyPresent
            };
            event.files = vec![RestoreFileV2 {
                witness_id: intent.witness.witness_id.clone(),
                name: intent.witness.file.name.clone(),
                disposition,
            }];
            (PublicationDecision::AlreadyPresent, Some(observed))
        }
    };
    Ok(RecoveryDecision {
        publication,
        event,
        event_recorded: false,
        confirmed,
        marker: intent.marker,
        register: register_decision(intent, register, false, false),
        cleanup: intent.cleanup,
        newly_published: AttemptEffects::default(),
    })
}

pub fn content_observation(
    intent: &RestoreIntent,
    observation: io::Result<Option<MediaObservation>>,
) -> CurrentContent {
    match observation {
        Ok(None) => CurrentContent::Missing,
        Ok(Some(observed)) if observed.digest == intent.expected_digest() => {
            CurrentContent::Present(observed.into())
        }
        Ok(Some(_)) => CurrentContent::Conflict,
        Err(_) => CurrentContent::Uncertain,
    }
}

pub fn selected_name(intent: &RestoreIntent) -> &OsStr {
    OsStr::new(&intent.witness.file.name)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects
)]
mod legacy_tests;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::arithmetic_side_effects
)]
pub(super) mod tests {
    use super::*;
    use crate::archive_catalog::{ArchiveManifest, OffloadFile};
    use crate::archive_proof::{AcceptedProcessingObservation, OffloadOffer};
    use crate::{MarkId, MarkState, Proposal};

    pub fn identity(inode: u64) -> MediaIdentity {
        MediaIdentity::Unix { device: 1, inode }
    }

    pub fn fixture(with_mark: bool) -> RestoreIntent {
        let locator = RestoreLocator {
            target: Target {
                day: "20261009".into(),
                stream: "camera".into(),
                dir: "120000_60_suffix".into(),
            },
            segment: "120000_60".into(),
            layout: SegmentLayout::Named,
        };
        let witness = ArchiveWitness {
            manifest_id: "manifest-1".into(),
            witness_id: "witness-a".into(),
            snapshot_id: "b".repeat(64),
            repository_id: Some("c".repeat(64)),
            archive_path: Some("/stage/a.flac".into()),
            journal_id: Some("native-root-v1:unix:0000000000000001:0000000000000063".into()),
            layout: Some(locator.layout),
            segment_dir: Some(locator.target.dir.clone()),
            processing: Some(AcceptedProcessingObservation {
                record: None,
                has_analysis_row: true,
            }),
            file: OffloadFile {
                name: "a.flac".into(),
                bytes: 3,
                sha256: "a".repeat(64),
            },
        };
        let mark = with_mark.then(|| {
            let first = proof_file(&witness).unwrap();
            let second = ArchiveFileProof {
                witness_id: "witness-b".into(),
                name: "b.flac".into(),
                bytes: 7,
                archive_path: "/stage/b.flac".into(),
                ..first.clone()
            };
            let proof = OffloadProof {
                journal_id: witness.journal_id.clone().unwrap(),
                manifest_id: witness.manifest_id.clone(),
                target: locator.target.clone(),
                layout: locator.layout,
                repository_id: witness.repository_id.clone().unwrap(),
                snapshot_id: witness.snapshot_id.clone(),
                files: vec![first, second],
            };
            let proposal = Proposal {
                names: vec!["a.flac".into(), "b.flac".into()],
                bytes: 10,
                reason: "archive".into(),
            };
            // Fixed externally authored wire nonce keeps these tests runtime-free.
            let offer: OffloadOffer = serde_json::from_value(serde_json::json!({
                "proof": proof,
                "nonce": "d".repeat(64),
                "revision": "9d64a97d8bbe9cf220cde6894cf5c00f3e9b6beb0cbd8a900489d3c411bdf064",
            }))
            .unwrap();
            offer.validate().unwrap();
            Mark {
                id: MarkId::derive(
                    RemovalClass::OffloadRawRelease,
                    &locator.target,
                    &proposal.names,
                ),
                class: RemovalClass::OffloadRawRelease,
                target: locator.target.clone(),
                marked_at: "2026-10-10T00:00:00Z".into(),
                proposal,
                state: MarkState::Marked,
                offload: Some(offer),
            }
        });
        RestoreIntent::with_id(
            "f".repeat(32),
            locator,
            witness,
            DestinationObservation {
                root: "native-root-v1:unix:0000000000000001:0000000000000002".into(),
                parent: identity(3),
                segment: identity(4),
            },
            StageEvidence {
                parent: identity(3),
                directory: identity(5),
                leaf: identity(6),
                stage_name: ".tmp-offload-1234567890abcdef1234567890abcdef".into(),
                leaf_name: "payload".into(),
                boot_id: None,
            },
            mark.as_ref(),
        )
        .unwrap()
    }

    pub fn catalog(intent: &RestoreIntent) -> SegmentCatalog {
        SegmentCatalog {
            day: intent.locator.target.day.clone(),
            stream: intent.locator.target.stream.clone(),
            segment: intent.locator.segment.clone(),
            history: vec![ArchiveManifest {
                manifest_id: intent.witness.manifest_id.clone(),
                files: vec![intent.witness.clone()],
            }],
            active: vec![intent.witness.clone()],
            ..Default::default()
        }
    }

    pub fn register(intent: &RestoreIntent) -> Register {
        let mut register = Register::empty();
        if let Some(mark) = intent.retirement.expected() {
            register.version = 2;
            register.marks.insert(mark.id.clone(), mark.clone());
        }
        register
    }

    fn matching(intent: &RestoreIntent, inode: u64) -> CurrentContent {
        CurrentContent::Present(ConfirmedContent {
            identity: identity(inode),
            digest: intent.expected_digest(),
        })
    }

    fn inspect(
        intent: &RestoreIntent,
        catalog: &SegmentCatalog,
        register: &Register,
        content: CurrentContent,
    ) -> Result<RecoveryDecision, RepairHold> {
        inspect_recovery(intent, &intent.destination, catalog, register, content)
    }

    fn receipt(
        intent: &RestoreIntent,
        effect: PublicationEffect,
        confirmation: Confirmation,
    ) -> PublicationReceipt {
        PublicationReceipt {
            effect,
            confirmation,
            file_durability: Durability::Synced,
            parent_durability: Durability::Unproven,
            cleanup: Cleanup::Pending,
            identity: match effect {
                PublicationEffect::Published => Some(intent.stage.leaf),
                PublicationEffect::AlreadyPresent => Some(identity(90)),
                _ => None,
            },
            error: (confirmation == Confirmation::Unverified)
                .then(|| io::Error::other("cut after native effect")),
        }
    }

    #[test]
    fn no_effect_completion_preserves_mark_plan_and_requires_exact_recorded_cleanup() {
        for with_mark in [false, true] {
            for effect in [PublicationEffect::Conflict, PublicationEffect::NotPublished] {
                let mut before = fixture(with_mark);
                before
                    .record_outcome(&receipt(&before, effect, Confirmation::Unverified))
                    .unwrap();
                before.event_recorded = Obligation::Complete;
                let mut complete = before.clone();
                complete.cleanup = Obligation::Complete;
                complete.marker = Obligation::NotRequired;
                complete.register = Obligation::NotRequired;
                assert_eq!(before.validate_successor(&complete), Ok(()));
                assert_eq!(complete.retirement, before.retirement);
                for missing in 0..3 {
                    let mut invalid = complete.clone();
                    match missing {
                        0 => invalid.cleanup = Obligation::Pending,
                        1 => invalid.event_recorded = Obligation::Pending,
                        _ => {
                            invalid.event.files[0].disposition = RestoreDisposition::AlreadyPresent
                        }
                    }
                    assert!(invalid.validate().is_err());
                }
                let mut reopened = complete.clone();
                reopened.register = Obligation::Pending;
                assert!(complete.validate_successor(&reopened).is_err());
            }
        }
    }

    #[test]
    fn successor_freezes_independently_valid_bindings_and_whole_mark() {
        let original = fixture(false);
        let mut changed = Vec::new();
        let mut next = original.clone();
        next.locator.target.dir = "120000_60_other".into();
        next.witness.segment_dir = Some(next.locator.target.dir.clone());
        changed.push(next);
        let mut next = original.clone();
        next.witness.snapshot_id = "e".repeat(64);
        changed.push(next);
        let mut next = original.clone();
        next.destination.root = "native-root-v1:unix:0000000000000001:0000000000000009".into();
        changed.push(next);
        let mut next = original.clone();
        next.destination.parent = identity(20);
        next.stage.parent = identity(20);
        changed.push(next);
        let mut next = original.clone();
        next.stage.boot_id = Some("new-boot".into());
        changed.push(next);
        let mut next = original.clone();
        next.event.operation_id = "0".repeat(32);
        changed.push(next);
        for next in changed {
            next.validate().unwrap();
            assert_eq!(
                original.validate_successor(&next),
                Err(RepairHold::ImmutableEvent)
            );
        }
        let original = fixture(true);
        let mut next = original.clone();
        let mut mark = original.retirement.expected().unwrap().clone();
        mark.marked_at = "2026-10-10T00:01:00Z".into();
        next.retirement = MarkRetirement::plan(&next.locator, &next.witness, Some(&mark)).unwrap();
        next.validate().unwrap();
        assert_eq!(
            original.validate_successor(&next),
            Err(RepairHold::ImmutableEvent)
        );
    }

    #[test]
    fn successor_preserves_outcome_confirmation_and_recorded_event() {
        let before = fixture(true);
        let mut published = before.clone();
        published
            .record_outcome(&receipt(
                &before,
                PublicationEffect::Published,
                Confirmation::Confirmed,
            ))
            .unwrap();
        assert_eq!(before.validate_successor(&published), Ok(()));
        let mut erased = before.clone();
        erased.cleanup = published.cleanup;
        assert!(published.validate_successor(&erased).is_err());
        let mut rebound = published.clone();
        rebound.outcome.as_mut().unwrap().error = None;
        rebound.outcome.as_mut().unwrap().parent_durability = Durability::Synced;
        rebound.validate().unwrap();
        assert_eq!(published.validate_successor(&rebound), Ok(()));
        let mut regressed = rebound.clone();
        regressed.outcome.as_mut().unwrap().parent_durability = Durability::Unproven;
        regressed.validate().unwrap();
        assert!(rebound.validate_successor(&regressed).is_err());
        let mut modified = rebound.clone();
        modified.outcome.as_mut().unwrap().error = Some("changed".into());
        modified.validate().unwrap();
        assert!(published.validate_successor(&modified).is_err());
        let mut rebound = published.clone();
        rebound.event.files[0].disposition = RestoreDisposition::AlreadyPresent;
        rebound.confirmed.as_mut().unwrap().identity = identity(99);
        rebound.validate().unwrap();
        assert!(published.validate_successor(&rebound).is_err());
        let mut complete = published.clone();
        complete.marker = Obligation::Complete;
        complete.event_recorded = Obligation::Complete;
        complete.register = Obligation::Complete;
        complete.cleanup = Obligation::Complete;
        assert_eq!(published.validate_successor(&complete), Ok(()));
        for reset in 0..4 {
            let mut next = complete.clone();
            match reset {
                0 => {
                    next.marker = Obligation::Pending;
                    next.register = Obligation::Pending;
                }
                1 => {
                    next.event_recorded = Obligation::Pending;
                    next.register = Obligation::Pending;
                }
                2 => next.register = Obligation::Pending,
                _ => next.cleanup = Obligation::Pending,
            }
            next.validate().unwrap();
            assert!(complete.validate_successor(&next).is_err());
        }
        let mut next = complete.clone();
        next.event.files[0].disposition = RestoreDisposition::AlreadyPresent;
        next.validate().unwrap();
        assert!(complete.validate_successor(&next).is_err());
    }

    #[test]
    fn event_revision_must_precede_recorded_completion_and_first_effect_reopens_marker() {
        let before = fixture(false);
        let mut next = before.clone();
        next.event.files[0].disposition = RestoreDisposition::AlreadyPresent;
        next.confirmed = Some(ConfirmedContent {
            identity: identity(90),
            digest: next.expected_digest(),
        });
        next.validate().unwrap();
        assert_eq!(before.validate_successor(&next), Ok(()));
        next.event_recorded = Obligation::Complete;
        assert!(before.validate_successor(&next).is_err());
        let mut observed = next.clone();
        observed.event_recorded = Obligation::Pending;
        observed.marker = Obligation::Complete;
        let mut first = observed.clone();
        first
            .record_outcome(&receipt(
                &observed,
                PublicationEffect::AlreadyPresent,
                Confirmation::Confirmed,
            ))
            .unwrap();
        assert_eq!(observed.validate_successor(&first), Ok(()));
        assert_eq!(first.marker, Obligation::Pending);
        first.marker = Obligation::Complete;
        first.validate().unwrap();
        assert!(observed.validate_successor(&first).is_err());
    }

    #[test]
    fn strict_small_document_roundtrip_preserves_windows_ids_and_unknown_boot() {
        for (with_mark, captured) in [
            (
                false,
                include_bytes!("restore_intent/fixtures/without-mark.json").as_slice(),
            ),
            (
                true,
                include_bytes!("restore_intent/fixtures/with-mark.json").as_slice(),
            ),
        ] {
            let original = fixture(with_mark);
            assert_eq!(original.encode().unwrap(), captured);
            assert_eq!(RestoreIntent::decode(Ok(captured)).unwrap(), original);
        }
        let mut intent = fixture(true);
        intent.destination.root =
            "native-root-v1:windows:000000000000000c:000102030405060708090a0b0c0d0e0f".into();
        intent.destination.parent = MediaIdentity::Windows {
            volume: 12,
            file_id: [1; 16],
        };
        intent.destination.segment = MediaIdentity::Windows {
            volume: 12,
            file_id: [2; 16],
        };
        intent.stage.parent = intent.destination.parent;
        intent.stage.directory = MediaIdentity::Windows {
            volume: 12,
            file_id: [3; 16],
        };
        intent.stage.leaf = MediaIdentity::Windows {
            volume: 12,
            file_id: [255; 16],
        };
        let bytes = intent.encode().unwrap();
        assert_eq!(RestoreIntent::decode(Ok(&bytes)).unwrap(), intent);
        assert!(intent.stage.boot_id.is_none());
        assert_ne!(
            intent.destination.root,
            intent.witness.journal_id.clone().unwrap()
        );
        assert_eq!(
            inspect(
                &intent,
                &catalog(&intent),
                &register(&intent),
                CurrentContent::Missing
            )
            .unwrap()
            .publication,
            PublicationDecision::Missing
        );
        intent.stage.directory = identity(5);
        assert_eq!(intent.validate(), Err(RepairHold::InvalidIntent));
    }

    #[test]
    fn torn_unknown_contradictory_and_unreadable_intents_are_preserved_holds() {
        let intent = fixture(true);
        let bytes = intent.encode().unwrap();
        let original = bytes.clone();
        assert_eq!(
            RestoreIntent::decode(Ok(&bytes[..bytes.len() - 1])),
            Err(RepairHold::InvalidIntent)
        );
        assert_eq!(bytes, original);
        assert_eq!(
            RestoreIntent::decode(Err(io::Error::other("read failure"))),
            Err(RepairHold::UnreadableIntent)
        );
        for field in ["unknown", "version"] {
            let mut value = serde_json::to_value(&intent).unwrap();
            value[field] = 99.into();
            assert_eq!(
                RestoreIntent::decode(Ok(&serde_json::to_vec(&value).unwrap())),
                Err(RepairHold::InvalidIntent)
            );
        }
        let mut value = serde_json::to_value(&intent).unwrap();
        value["witness"]["processing"]["unknown"] = true.into();
        assert_eq!(
            RestoreIntent::decode(Ok(&serde_json::to_vec(&value).unwrap())),
            Err(RepairHold::InvalidIntent)
        );
        let mut intent = intent;
        intent.event.files[0].name = "b.flac".into();
        assert_eq!(intent.validate(), Err(RepairHold::InvalidIntent));
        assert_eq!(
            RestoreIntent::decode(Ok(&vec![b' '; MAX_INTENT_BYTES + 1])),
            Err(RepairHold::OversizedIntent)
        );
        let mut huge = fixture(false);
        huge.stage.boot_id = Some("a".repeat(MAX_INTENT_BYTES));
        assert_eq!(huge.encode(), Err(RepairHold::OversizedIntent));
    }

    #[test]
    fn exact_selection_uses_full_witness_and_default_legacy_refuses() {
        let intent = fixture(false);
        let mut current = catalog(&intent);
        current.active[0]
            .processing
            .as_mut()
            .unwrap()
            .has_analysis_row = false;
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::CatalogChanged)
        );
        current = catalog(&intent);
        current.active[0].witness_id = "new-generation".into();
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::CatalogChanged)
        );
        let mut legacy = intent.clone();
        legacy.witness.repository_id = None;
        assert_eq!(legacy.validate(), Err(RepairHold::UnboundWitness));
        let mut wrong_history = catalog(&intent);
        wrong_history.history[0].files[0].archive_path = Some("/other/a.flac".into());
        assert_eq!(
            inspect(
                &intent,
                &wrong_history,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::CatalogChanged)
        );
    }

    #[test]
    fn planned_remainder_covers_absent_selected_output_and_all_original_names() {
        let intent = fixture(true);
        let MarkRetirement::Exact { plan } = &intent.retirement else {
            panic!("expected plan");
        };
        assert_eq!(plan.replacement().unwrap().proposal.names, vec!["b.flac"]);
        assert_eq!(plan.replacement().unwrap().proposal.bytes, 7);
        assert_eq!(
            intent.output_paths(Path::new("/j")),
            vec![
                PathBuf::from("/j/chronicle/20261009/camera/120000_60_suffix/a.jsonl"),
                PathBuf::from("/j/chronicle/20261009/camera/120000_60_suffix/b.jsonl")
            ]
        );
        let no_mark = fixture(false);
        assert_eq!(no_mark.output_paths(Path::new("/j")).len(), 1);
        let mut tampered = serde_json::to_value(&intent).unwrap();
        tampered["retirement"]["plan"]["replacement"]["proposal"]["reason"] =
            "new remainder".into();
        assert!(RestoreIntent::decode(Ok(&serde_json::to_vec(&tampered).unwrap())).is_err());
    }

    #[test]
    fn changed_original_and_new_mark_after_no_mark_refuse_before_publication() {
        let intent = fixture(true);
        let mut changed = register(&intent);
        changed.marks.values_mut().next().unwrap().marked_at = "later".into();
        assert_eq!(
            inspect(
                &intent,
                &catalog(&intent),
                &changed,
                CurrentContent::Missing
            ),
            Err(RepairHold::MarkChanged)
        );
        let absent = fixture(false);
        assert_eq!(
            inspect(
                &absent,
                &catalog(&absent),
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::MarkChanged)
        );
        let mut colliding = register(&intent);
        let MarkRetirement::Exact { plan } = &intent.retirement else {
            panic!("expected plan");
        };
        let remainder = plan.replacement().unwrap();
        colliding
            .marks
            .insert(remainder.id.clone(), remainder.clone());
        assert_eq!(
            inspect(
                &intent,
                &catalog(&intent),
                &colliding,
                CurrentContent::Missing
            ),
            Err(RepairHold::MarkChanged)
        );
    }

    #[test]
    fn recorded_success_uses_history_without_republishing_or_touching_new_generation() {
        let mut intent = fixture(true);
        let receipt = receipt(
            &intent,
            PublicationEffect::Published,
            Confirmation::Confirmed,
        );
        assert_eq!(
            intent.record_outcome(&receipt).unwrap(),
            AttemptEffects { files: 1, bytes: 3 }
        );
        let mut current = catalog(&intent);
        current.restores.push(intent.event.clone());
        current.active[0].witness_id = "newer".into();
        let before = current.clone();
        let decision = inspect(
            &intent,
            &current,
            &register(&intent),
            CurrentContent::Missing,
        )
        .unwrap();
        assert_eq!(decision.publication, PublicationDecision::HistoricalOnly);
        assert_eq!(decision.newly_published, AttemptEffects::default());
        assert_eq!(decision.marker, Obligation::Pending);
        assert_eq!(decision.register, RegisterDecision::AwaitingMarker);
        assert_eq!(current, before);
        intent.marker = Obligation::Complete;
        assert!(matches!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            )
            .unwrap()
            .register,
            RegisterDecision::Ready(_)
        ));
    }

    #[test]
    fn recorded_success_classifies_exact_remainder_newer_mark_and_collision_read_only() {
        let mut intent = fixture(true);
        let receipt = receipt(
            &intent,
            PublicationEffect::AlreadyPresent,
            Confirmation::Confirmed,
        );
        intent.record_outcome(&receipt).unwrap();
        intent.marker = Obligation::Complete;
        let mut current = catalog(&intent);
        current.active.clear();
        current.restores.push(intent.event.clone());
        let MarkRetirement::Exact { plan } = &intent.retirement else {
            panic!("expected plan");
        };
        let remainder = plan.replacement().unwrap();
        let mut applied = Register::empty();
        applied.version = 2;
        applied
            .marks
            .insert(remainder.id.clone(), remainder.clone());
        assert_eq!(
            inspect(&intent, &current, &applied, CurrentContent::Missing)
                .unwrap()
                .register,
            RegisterDecision::AlreadyApplied
        );
        let mut newer = register(&intent);
        newer.marks.values_mut().next().unwrap().proposal.reason = "newer proposal".into();
        let before = newer.clone();
        assert_eq!(
            inspect(&intent, &current, &newer, CurrentContent::Missing)
                .unwrap()
                .register,
            RegisterDecision::Superseded
        );
        assert_eq!(newer, before);
        let mut collision = register(&intent);
        collision
            .marks
            .insert(remainder.id.clone(), remainder.clone());
        let before = collision.clone();
        assert_eq!(
            inspect(&intent, &current, &collision, CurrentContent::Missing)
                .unwrap()
                .register,
            RegisterDecision::Collision
        );
        assert_eq!(collision, before);
    }

    #[test]
    fn recorded_unsuccessful_events_are_immutable_when_matching_bytes_appear() {
        for effect in [
            PublicationEffect::Published,
            PublicationEffect::Conflict,
            PublicationEffect::NotPublished,
        ] {
            let mut intent = fixture(true);
            let receipt = receipt(&intent, effect, Confirmation::Unverified);
            intent.record_outcome(&receipt).unwrap();
            let event = intent.event.clone();
            let mut current = catalog(&intent);
            current.restores.push(event.clone());
            let decision =
                inspect(&intent, &current, &register(&intent), matching(&intent, 80)).unwrap();
            assert_eq!(decision.event, event);
            assert_eq!(decision.register, RegisterDecision::AwaitingSuccessfulEvent);
            assert_eq!(decision.newly_published, AttemptEffects::default());
            assert_eq!(
                intent.prepare_recovery_event(&decision),
                Err(RepairHold::ImmutableEvent)
            );
            assert_eq!(intent.event, event);
        }
    }

    #[test]
    fn id_collisions_duplicates_and_degraded_catalog_are_holds() {
        let intent = fixture(false);
        let mut current = catalog(&intent);
        let mut wrong = intent.event.clone();
        wrong.files[0].disposition = RestoreDisposition::Conflict;
        current.restores.push(wrong);
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::OperationCollision)
        );
        current.restores = vec![intent.event.clone(), intent.event.clone()];
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::OperationCollision)
        );
        current.restores.clear();
        current.skipped_records = 1;
        current.active.clear();
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::DegradedCatalog)
        );
        current.skipped_records = 0;
        current.unreadable_ledgers.push("unreadable".into());
        assert_eq!(
            inspect(
                &intent,
                &current,
                &register(&intent),
                CurrentContent::Missing
            ),
            Err(RepairHold::DegradedCatalog)
        );
    }

    #[test]
    fn crash_before_outcome_save_recovers_matching_bytes_without_attributing_effect() {
        let intent = fixture(false);
        let durable_before_publication = intent.encode().unwrap();
        let mut after_cut = RestoreIntent::decode(Ok(&durable_before_publication)).unwrap();
        let decision = inspect(
            &after_cut,
            &catalog(&after_cut),
            &register(&after_cut),
            matching(&after_cut, 80),
        )
        .unwrap();
        assert_eq!(
            decision.event.files[0].disposition,
            RestoreDisposition::AlreadyPresent
        );
        assert_eq!(decision.newly_published, AttemptEffects::default());
        assert_eq!(decision.marker, Obligation::Pending);
        after_cut.prepare_recovery_event(&decision).unwrap();
        assert!(after_cut.outcome.is_none());
        assert_eq!(
            RestoreIntent::decode(Ok(&after_cut.encode().unwrap())).unwrap(),
            after_cut
        );
    }

    #[test]
    fn actual_unverified_effect_is_counted_once_and_marker_survives_zero_byte_retry() {
        let mut intent = fixture(true);
        let receipt = receipt(
            &intent,
            PublicationEffect::Published,
            Confirmation::Unverified,
        );
        assert_eq!(
            intent.record_outcome(&receipt).unwrap(),
            AttemptEffects { files: 1, bytes: 3 }
        );
        let saved = intent.encode().unwrap();
        let mut recovered = RestoreIntent::decode(Ok(&saved)).unwrap();
        let decision = inspect(
            &recovered,
            &catalog(&recovered),
            &register(&recovered),
            matching(&recovered, 6),
        )
        .unwrap();
        assert_eq!(
            decision.event.files[0].disposition,
            RestoreDisposition::AlreadyPresent
        );
        assert_eq!(decision.marker, Obligation::Pending);
        assert_eq!(decision.newly_published, AttemptEffects::default());
        recovered.prepare_recovery_event(&decision).unwrap();
        assert_eq!(
            recovered.outcome.as_ref().unwrap().effect,
            PublicationEffect::Published
        );
        assert_eq!(
            recovered.record_outcome(&receipt).unwrap_err().hold,
            RepairHold::ImmutableEvent
        );
        assert_eq!(
            inspect(
                &recovered,
                &catalog(&recovered),
                &register(&recovered),
                CurrentContent::Missing
            ),
            Err(RepairHold::PublishedContentMissing)
        );
    }

    #[test]
    fn current_conflict_retains_prior_actual_effect_and_invalidation_obligation() {
        let mut intent = fixture(true);
        let receipt = receipt(
            &intent,
            PublicationEffect::Published,
            Confirmation::Confirmed,
        );
        intent.record_outcome(&receipt).unwrap();
        let decision = inspect(
            &intent,
            &catalog(&intent),
            &register(&intent),
            CurrentContent::Conflict,
        )
        .unwrap();
        intent.prepare_recovery_event(&decision).unwrap();
        assert_eq!(
            intent.outcome.as_ref().unwrap().effect,
            PublicationEffect::Published
        );
        assert_eq!(intent.marker, Obligation::Pending);
        intent.marker = Obligation::Complete;
        assert!(intent.validate().is_ok());
    }

    #[test]
    fn a_later_native_attempt_reopens_a_completed_marker_obligation() {
        let mut intent = fixture(false);
        let decision = inspect(
            &intent,
            &catalog(&intent),
            &register(&intent),
            matching(&intent, 80),
        )
        .unwrap();
        intent.prepare_recovery_event(&decision).unwrap();
        intent.marker = Obligation::Complete;
        let receipt = receipt(
            &intent,
            PublicationEffect::Published,
            Confirmation::Confirmed,
        );
        intent.record_outcome(&receipt).unwrap();
        assert_eq!(intent.marker, Obligation::Pending);
    }

    #[test]
    fn post_effect_state_failure_preserves_actual_receipt_and_attempt_counters() {
        let mut intent = fixture(false);
        let receipt = receipt(
            &intent,
            PublicationEffect::Published,
            Confirmation::Unverified,
        );
        intent.stage.stage_name = "invalid".into();
        let before = intent.clone();
        let failure = intent.record_outcome(&receipt).unwrap_err();
        assert_eq!(failure.hold, RepairHold::InvalidIntent);
        assert_eq!(failure.effects, AttemptEffects { files: 1, bytes: 3 });
        assert_eq!(failure.facts.effect, PublicationEffect::Published);
        assert_eq!(failure.facts.confirmation, Confirmation::Unverified);
        assert_eq!(intent, before);
    }

    #[test]
    fn copied_root_rebound_parent_and_segment_never_grant_replay() {
        let intent = fixture(false);
        for changed in [
            DestinationObservation {
                root: "native-root-v1:unix:0000000000000001:0000000000000012".into(),
                ..intent.destination.clone()
            },
            DestinationObservation {
                parent: identity(42),
                ..intent.destination.clone()
            },
            DestinationObservation {
                segment: identity(42),
                ..intent.destination.clone()
            },
        ] {
            assert_eq!(
                inspect_recovery(
                    &intent,
                    &changed,
                    &catalog(&intent),
                    &register(&intent),
                    CurrentContent::Missing
                ),
                Err(RepairHold::DestinationChanged)
            );
        }
    }

    #[test]
    fn observation_errors_do_not_certify_conflict_or_completion() {
        let intent = fixture(false);
        for kind in [io::ErrorKind::InvalidData, io::ErrorKind::NotFound] {
            let content = content_observation(&intent, Err(kind.into()));
            assert_eq!(content, CurrentContent::Uncertain);
            assert_eq!(
                inspect(&intent, &catalog(&intent), &register(&intent), content),
                Err(RepairHold::UncertainContent)
            );
        }
        assert_eq!(
            content_observation(&intent, Ok(None)),
            CurrentContent::Missing
        );
    }

    #[test]
    fn ordinary_failures_and_already_present_have_zero_new_effect_and_arithmetic_is_checked() {
        for effect in [
            PublicationEffect::NotPublished,
            PublicationEffect::Conflict,
            PublicationEffect::AlreadyPresent,
        ] {
            let mut intent = fixture(false);
            let confirmation = if effect == PublicationEffect::AlreadyPresent {
                Confirmation::Confirmed
            } else {
                Confirmation::Unverified
            };
            let receipt = receipt(&intent, effect, confirmation);
            assert_eq!(
                intent.record_outcome(&receipt).unwrap(),
                AttemptEffects::default()
            );
        }
        assert_eq!(
            AttemptEffects {
                files: u64::MAX,
                bytes: 0
            }
            .checked_add(AttemptEffects { files: 1, bytes: 0 }),
            Err(RepairHold::CounterOverflow)
        );
        assert_eq!(
            AttemptEffects {
                files: 0,
                bytes: u64::MAX
            }
            .checked_add(AttemptEffects { files: 0, bytes: 1 }),
            Err(RepairHold::CounterOverflow)
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod recovery_tests;
