// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Conditional retirement of this process's exact confirmed metadata record.
//! Root WAN-carrier settlement and retirement belong to the lower owner.

use super::*;
use solstone_core_journal_io::media::{MediaRead, MediaRemoval};

pub(in crate::restore_intent_store) enum NativeRetirement {
    None,
    StageFailure(OwnedMediaFailure<Option<MediaStage>>),
    Unconsumed(MediaStage),
    Removal(Box<MediaRemoval>),
}

pub(in crate::restore_intent_store) struct RetirementFacts {
    pub(in crate::restore_intent_store) observed: MediaRead,
    pub(in crate::restore_intent_store) native: NativeRetirement,
    pub(in crate::restore_intent_store) ancestors: [Durability; 4],
    pub(in crate::restore_intent_store) failures: Vec<StoreProblem>,
    pub(in crate::restore_intent_store) retired: bool,
}

impl TransactionOutcome<'_, '_> {
    /// Only an actual confirmed in-process store write supplies record continuity.
    /// Replay without that native observation needs separately qualified adoption.
    pub(crate) fn retire_intent(mut self, options: LockOptions) -> Self {
        if self.hold.is_some() || self.retirement.is_some() {
            return self;
        }
        #[cfg(any(unix, windows))]
        {
            self = self.retire_with(options, &mut super::super::native::NativeHooks);
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = options;
            self.hold = Some(Hold::Store(StoreProblem::Unsupported));
        }
        self
    }

    #[cfg(any(unix, windows))]
    pub(super) fn retire_with(
        mut self,
        options: LockOptions,
        hooks: &mut impl super::super::native::Hooks,
    ) -> Self {
        let result = self.retire_bound(options, hooks);
        if let Err(hold) = result {
            self.hold = Some(hold);
        }
        self
    }

    #[cfg(any(unix, windows))]
    fn retire_bound(
        &mut self,
        options: LockOptions,
        _hooks: &mut impl super::super::native::Hooks,
    ) -> Result<(), Hold> {
        use sha2::{Digest, Sha256};
        use solstone_core_journal_io::activity_obligations::{
            IntentRead, NativeObligation, intent_filename,
        };
        use solstone_core_journal_io::media::{Cleanup, OriginalName, RemovalEffect, RemovalPhase};
        use std::path::Path;

        self.owner.native_check()?;
        self.owner.admission.check()?;
        let no_effect = self
            .no_effect_cleanup
            .as_ref()
            .is_some_and(NoEffectCleanup::confirmed)
            && self.effects == AttemptEffects::default()
            && self.publication.as_ref().is_some_and(|receipt| {
                matches!(
                    receipt.effect,
                    PublicationEffect::Conflict | PublicationEffect::NotPublished
                ) && receipt.identity.is_none()
                    && self.record.as_ref().is_some_and(|record| {
                        record.intent.no_effect_outcome()
                            && record.intent.outcome.as_ref().is_some_and(|facts| {
                                facts.matches_ignoring_cleanup(
                                    &solstone_core_retention::restore_intent::PublicationFacts::from(receipt),
                                )
                            })
                    })
            });
        let record = self.record()?;
        let intent = &record.intent;
        if record.persisted.as_ref() != Some(intent)
            || intent.marker
                != if no_effect {
                    Obligation::NotRequired
                } else {
                    Obligation::Complete
                }
            || intent.event_recorded != Obligation::Complete
            || !matches!(
                intent.register,
                Obligation::Complete | Obligation::NotRequired
            )
            || intent.cleanup != Obligation::Complete
            || !no_effect
                && !intent.event.files.iter().all(|file| {
                    matches!(
                        file.disposition,
                        RestoreDisposition::Published | RestoreDisposition::AlreadyPresent
                    )
                })
            || !no_effect
                && intent.outcome.as_ref().is_none_or(|facts| {
                    if cfg!(windows) {
                        facts.confirmation != Confirmation::Confirmed
                            || (facts.effect == PublicationEffect::Published
                                && facts.file_durability != Durability::Synced)
                    } else {
                        facts.confirmation != Confirmation::Confirmed
                            || (facts.effect == PublicationEffect::Published
                                && (facts.file_durability != Durability::Synced
                                    || facts.parent_durability != Durability::Synced))
                    }
                })
        {
            return Err(Hold::Retirement);
        }
        let confirmed = record
            .writes
            .last()
            .filter(|write| write.fully_confirmed())
            .and_then(|write| write.confirmation.as_ref())
            .ok_or(Hold::Retirement)?
            .clone();
        let claims = OutputClaims::acquire(
            intent.output_paths(self.owner.root().canonical_path()),
            options,
        )
        .map_err(|_| Hold::Repair(RepairHold::ClaimsUnavailable))?;
        let commit = claims
            .commit(self.owner.destination.live().diagnostic_path())
            .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?;
        if claims
            .segment_identity(self.owner.destination.live().diagnostic_path())
            .map_err(|_| Hold::Repair(RepairHold::DestinationChanged))?
            != self.owner.destination.live().object_identity()
        {
            return Err(Hold::Repair(RepairHold::DestinationChanged));
        }
        self.effect_check(&commit)?;
        if no_effect {
            self.unchanged_mark()?;
        } else {
            self.live_matches()?;
        }
        let decision = self.inspect()?;
        if !decision.event_recorded
            || !no_effect
                && !matches!(
                    decision.register,
                    RegisterDecision::NoMark | RegisterDecision::Complete
                )
        {
            return Err(Hold::Retirement);
        }
        let record = self.record()?;
        let store = record.store.as_ref().ok_or(Hold::Retirement)?;
        let observed = store
            .read(&record.intent.event.operation_id)
            .map_err(Hold::Store)?
            .ok_or(Hold::Retirement)?;
        if observed.intent != record.intent || observed.read != confirmed {
            return Err(Hold::Store(StoreProblem::Changed));
        }
        let name = intent_filename(&observed.intent.event.operation_id).ok_or(Hold::Retirement)?;
        let relative = Path::new("health/offload/restore-intents").join(&name);
        let census = observe_restore_obligations(&self.owner)?;
        let mut matched = 0_usize;
        for entry in census.unresolved_entries() {
            if matches!(entry, NativeObligation::Intent { relative: current, entry, contents:
                IntentRead::Complete { operation_id, bytes } }
                if *current == relative && *entry == observed.read.entry
                    && *operation_id == observed.intent.event.operation_id
                    && *bytes == observed.read.bytes)
            {
                matched += 1;
            } else {
                return Err(Hold::Obligations);
            }
        }
        if matched != 1 {
            return Err(Hold::Obligations);
        }
        drop(census);
        self.retirement = Some(RetirementFacts {
            observed: observed.read.clone(),
            native: NativeRetirement::None,
            ancestors: [Durability::Unproven; 4],
            failures: Vec::new(),
            retired: false,
        });
        let store = self.record()?.store.as_ref().ok_or(Hold::Retirement)?;
        store.revalidate().map_err(Hold::Store)?;
        let mut bindings = store.chain.iter();
        let _health = bindings.next().ok_or(Hold::Retirement)?;
        let parent = Arc::clone(
            bindings
                .next()
                .and_then(|binding| binding.media.as_ref())
                .ok_or(Hold::Retirement)?,
        );
        let source = Arc::clone(
            bindings
                .next()
                .and_then(|binding| binding.media.as_ref())
                .ok_or(Hold::Retirement)?,
        );
        self.effect_check(&commit)?;
        let stage = match MediaStage::create(parent, Arc::clone(self.owner.carrier.activity())) {
            Ok(stage) => stage,
            Err(failure) => {
                self.retirement.as_mut().ok_or(Hold::Retirement)?.native =
                    NativeRetirement::StageFailure(failure);
                return Err(Hold::Retirement);
            }
        };
        self.retirement.as_mut().ok_or(Hold::Retirement)?.native =
            NativeRetirement::Unconsumed(stage);
        let expected = source
            .observe_file(&name)
            .map_err(|_| Hold::Retirement)?
            .ok_or(Hold::Retirement)?;
        let current = self
            .record()?
            .store
            .as_ref()
            .ok_or(Hold::Retirement)?
            .read(&observed.intent.event.operation_id)
            .map_err(Hold::Store)?
            .ok_or(Hold::Retirement)?;
        if current.read != confirmed
            || current.intent != observed.intent
            || expected.identity != confirmed.entry.identity
            || expected.digest.size != confirmed.entry.size
            || expected.digest.sha256 != format!("{:x}", Sha256::digest(&confirmed.bytes))
        {
            return Err(Hold::Store(StoreProblem::Changed));
        }
        self.record()?
            .store
            .as_ref()
            .ok_or(Hold::Retirement)?
            .revalidate()
            .map_err(Hold::Store)?;
        self.effect_check(&commit)?;
        let native = &mut self.retirement.as_mut().ok_or(Hold::Retirement)?.native;
        let NativeRetirement::Unconsumed(stage) = std::mem::replace(native, NativeRetirement::None)
        else {
            return Err(Hold::Retirement);
        };
        let mut removal = Box::new(source.remove_observed(&name, &expected, stage));
        if removal.receipt().cleanup != Cleanup::Complete {
            removal.cleanup();
        }
        let source_durability = removal.receipt().source_durability;
        *native = NativeRetirement::Removal(removal);
        let prep_durability = self
            .record()?
            .store
            .as_ref()
            .ok_or(Hold::Retirement)?
            .preparation_durability();
        let ancestors = [
            source_durability,
            prep_durability[1],
            prep_durability[2],
            prep_durability[3],
        ];
        let facts = self.retirement.as_mut().ok_or(Hold::Retirement)?;
        facts.ancestors = ancestors;
        facts.failures = Vec::new();
        let confirmed_removal = matches!(&facts.native, NativeRetirement::Removal(removal)
            if removal.receipt().effect == RemovalEffect::Deleted
                && removal.receipt().original_name == OriginalName::Absent
                && removal.receipt().claim_confirmation == Confirmation::Confirmed
                && if cfg!(windows) {
                    matches!(removal.receipt().source_durability, Durability::Synced | Durability::Unproven)
                        && matches!(removal.receipt().claim_durability, Durability::Synced | Durability::Unproven)
                } else {
                    removal.receipt().source_durability == Durability::Synced
                        && removal.receipt().claim_durability == Durability::Synced
                }
                && !removal.receipt().failures.iter().any(|f| f.phase != RemovalPhase::Cleanup));
        let ancestors_valid = if cfg!(windows) {
            ancestors
                .iter()
                .all(|d| matches!(d, Durability::Synced | Durability::Unproven))
        } else {
            ancestors == [Durability::Synced; 4]
        };
        if !confirmed_removal || !ancestors_valid || !facts.failures.is_empty() {
            return Err(Hold::Retirement);
        }
        self.record()?
            .store
            .as_ref()
            .ok_or(Hold::Retirement)?
            .revalidate()
            .map_err(Hold::Store)?;
        self.owner.native_check()?;
        if no_effect {
            self.unchanged_mark()?;
        }
        let census = observe_restore_obligations(&self.owner)?;
        if census.unresolved_entries().next().is_some() {
            return Err(Hold::Obligations);
        }
        self.retirement.as_mut().ok_or(Hold::Retirement)?.retired = true;
        Ok(())
    }
}
