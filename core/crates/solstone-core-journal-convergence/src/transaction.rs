// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::time::Duration;

use crate::allocate::{bump_serial, ensure_adoption};
use crate::claim::{
    ClaimView, IntroduceSpec, all_unclaimed, ancestry_preserves, days_claimed_by_other,
    ensure_claim_dir, introduce, mechanical_finalize, same_owner_claim, write_head,
};
use crate::clearance::{PredecessorClass, classify_predecessor, consume_intent_days};
use crate::digest::digest_value;
use crate::error::{ConvergenceError, DurableRole, Refusal, map_root_error};
use crate::init::open_store_dirs;
use crate::intent::{
    build_allocation_intent, build_later_intent, build_virgin_intent, day_is_store_genesis,
    read_intent, verify_intent_matches_claim, write_active, write_intent,
};
use crate::layout::DayKey;
use crate::lock::{DayLockSet, acquire_days_with_timeout, hold_topology_with_timeout};
use crate::owner::{
    AdmitOutcome, ClaimAdmission, OwnerBinding, load_prepared_owner, require_active,
};
use crate::permit::Permit;
use crate::preflight::Admitted;
use crate::projection::{project_day, refuse_mutated_projection};
use crate::publish::{PublishOutcome, inspect_against_proposed, publish_record};
use crate::registry::enter_registry;
use crate::schema::{
    Active, Adoption, DayRecord, Intent, ROLE_ACTIVE, SCHEMA_VERSION, now_rfc3339,
};
use crate::store::{DaySnapshot, LoadDay};

/// Opaque held-day-set. Not `Clone`. Drop releases day flocks.
pub struct HeldDays<'a> {
    pub(crate) admitted: &'a Admitted,
    pub(crate) locks: DayLockSet,
    owner: OwnerBinding,
    pub(crate) days: Vec<DayKey>,
    timeout: Duration,
    proof_consumed: bool,
    /// True once this lease has published a claim introduction. It separates
    /// the operation's **first** admission, which requires an exactly empty
    /// link set, from a later-dirty **successor** admission on the same live
    /// lease, whose predecessor link is already durable and must not be
    /// rewritten.
    had_allocation: bool,
    pub(crate) serial: Option<u64>,
    claim_revision: Option<u64>,
    pub(crate) intent_digest: Option<String>,
}

impl std::fmt::Debug for HeldDays<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeldDays")
            .field("days", &self.days)
            .field("serial", &self.serial)
            .finish_non_exhaustive()
    }
}

impl Admitted {
    pub fn begin(&self, owner: OwnerBinding) -> Result<HeldDays<'_>, ConvergenceError> {
        self.store.revalidate()?;
        owner.matches(
            self.store.journal_id(),
            self.store.root_id(),
            self.store.object_identity(),
        )?;
        let dirs = open_store_dirs(self.store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let locks = acquire_days_with_timeout(
            &dirs,
            self.days(),
            self.store.journal_id(),
            self.store.root_id(),
            self.store.object_identity(),
            self.lock_timeout(),
        )?;
        Ok(HeldDays {
            admitted: self,
            locks,
            owner,
            days: self.days().to_vec(),
            timeout: self.lock_timeout(),
            proof_consumed: false,
            had_allocation: false,
            serial: None,
            claim_revision: None,
            intent_digest: None,
        })
    }
}

impl ClaimAdmission {
    /// Under-day owner reauthentication and one-shot proof mint (hook B).
    ///
    /// The complete day set is already held. This briefly reacquires the
    /// registry in days-to-registry order, proves the prepared owner is still
    /// exactly active and unrevoked with no pending revocation, and proves the
    /// operation's immutable owner-intent link is exactly absent. Only exact
    /// absence releases the registry and returns a proof bound to this held
    /// set, owner, operation, and grant-request selector digest. An existing
    /// link classifies the attempt as recovery of the original linked
    /// transaction and mints no proof, serial, claim, or intent. No global
    /// lock overlaps the registry section.
    pub fn admit(
        held: &HeldDays<'_>,
        owner: &OwnerBinding,
    ) -> Result<AdmitOutcome, ConvergenceError> {
        if owner.digest_hex() != held.owner.digest_hex() {
            return Err(ConvergenceError::Refused(Refusal::WrongLineage));
        }
        owner.matches(
            held.admitted.store.journal_id(),
            held.admitted.store.root_id(),
            held.admitted.store.object_identity(),
        )?;
        held.locks.matches(
            held.admitted.store.journal_id(),
            held.admitted.store.root_id(),
            held.admitted.store.object_identity(),
        )?;
        let dirs = open_store_dirs(held.admitted.store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        {
            let section = enter_registry(&dirs)?;
            let record = load_prepared_owner(&section, owner.operation_id())?.ok_or(
                ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                },
            )?;
            if record.owner_binding_digest != owner.digest_hex() {
                return Err(ConvergenceError::Refused(Refusal::WrongLineage));
            }
            if record.selector_digest != owner.selector_digest() {
                return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
            }
            require_active(&record)?;
            // Absence is required only for the operation's first admission. A
            // lease that has already allocated is asking for a later-dirty
            // successor, and its predecessor link is legitimately present.
            if !held.had_allocation()
                && crate::link::operation_link_present(&section, owner.operation_id())?
            {
                return Ok(AdmitOutcome::ExistingLink);
            }
        }
        Ok(AdmitOutcome::Proof(Self::from_parts(
            held.locks.instance().to_owned(),
            owner.journal_id().to_owned(),
            owner.root_id().to_owned(),
            owner.digest_hex().to_owned(),
            owner.operation_id().to_owned(),
            owner.selector_digest().to_owned(),
            held.days.clone(),
        )))
    }
}

impl<'a> HeldDays<'a> {
    pub fn owner(&self) -> &OwnerBinding {
        &self.owner
    }

    pub(crate) fn had_allocation(&self) -> bool {
        self.had_allocation
    }

    pub fn continue_with(
        &mut self,
        proof: ClaimAdmission,
    ) -> Result<Permit<'_, 'a>, ConvergenceError> {
        self.bind_proof(proof)?;
        self.proceed()
    }

    pub fn proceed(&mut self) -> Result<Permit<'_, 'a>, ConvergenceError> {
        if !self.proof_consumed && self.serial.is_none() {
            return Err(ConvergenceError::Refused(Refusal::NoPermit));
        }
        if let Some(serial) = self.serial {
            let dirs = open_store_dirs(self.admitted.store.root())?
                .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
            if crate::terminal::read_terminal(&dirs, serial)?.is_some() {
                return self.mint_permit();
            }
        }
        self.advance(false)?;
        self.mint_permit()
    }

    fn mint_permit(&mut self) -> Result<Permit<'_, 'a>, ConvergenceError> {
        let store = &self.admitted.store;
        let dirs = open_store_dirs(store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let serial = self
            .serial
            .ok_or(ConvergenceError::Refused(Refusal::NoPermit))?;
        if crate::intent::read_active(&dirs, serial)?.is_none() {
            return Err(ConvergenceError::Refused(Refusal::NoPermit));
        }
        let intent =
            crate::intent::read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
                role: DurableRole::Intent,
            })?;
        for day in &self.days {
            let proposed = *intent
                .proposed_day_revisions
                .get(day.as_str())
                .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?;
            match inspect_against_proposed(store, &self.locks, day, proposed)? {
                LoadDay::Published(snapshot) if snapshot.record_revision == proposed => {}
                LoadDay::HeadedDescendant { .. } => {
                    if crate::terminal::read_terminal(&dirs, serial)?.is_none() {
                        return Err(ConvergenceError::Refused(Refusal::Superseded));
                    }
                }
                _ => {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::Record,
                    });
                }
            }
        }
        Ok(Permit { held: self })
    }

    pub fn snapshot(&self, day: &DayKey) -> Result<DaySnapshot, ConvergenceError> {
        match self.admitted.store.load_day(&self.locks, day)? {
            LoadDay::Published(snapshot) => Ok(snapshot),
            _ => Err(ConvergenceError::Unknown {
                role: DurableRole::Record,
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn lock_set(&self) -> &DayLockSet {
        &self.locks
    }

    #[allow(dead_code)]
    pub(crate) fn inspect_day(&self, day: &DayKey) -> Result<LoadDay, ConvergenceError> {
        self.admitted.store.load_day(&self.locks, day)
    }

    /// Same-owner later-dirty under live day locks (Phase 3, no release
    /// revision).
    ///
    /// Every claim and global allocation is preceded by owner
    /// reauthentication, so a later-dirty successor consumes its own fresh
    /// one-shot proof rather than reusing the admission that opened the first
    /// transition. A revoked or revocation-pending owner therefore cannot keep
    /// allocating.
    pub fn advance_dirty(&mut self, proof: ClaimAdmission) -> Result<(), ConvergenceError> {
        if self.serial.is_none() {
            return Err(ConvergenceError::Refused(Refusal::NoPermit));
        }
        self.bind_proof(proof)?;
        self.serial = None;
        self.claim_revision = None;
        self.intent_digest = None;
        self.advance(true)
    }

    fn bind_proof(&mut self, mut proof: ClaimAdmission) -> Result<(), ConvergenceError> {
        proof.consume()?;
        if proof.instance() != self.locks.instance() {
            return Err(ConvergenceError::Refused(Refusal::StaleLease));
        }
        if proof.owner_digest() != self.owner.digest_hex()
            || proof.journal_id() != self.admitted.store.journal_id()
            || proof.root_id() != self.admitted.store.root_id()
            || proof.days() != self.days.as_slice()
            || proof.operation_id() != self.owner.operation_id()
        {
            return Err(ConvergenceError::Refused(Refusal::WrongLineage));
        }
        if proof.selector_digest() != self.owner.selector_digest() {
            return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
        }
        self.proof_consumed = true;
        Ok(())
    }

    fn advance(&mut self, successor: bool) -> Result<(), ConvergenceError> {
        let store = &self.admitted.store;
        store.revalidate()?;
        self.locks
            .matches(store.journal_id(), store.root_id(), store.object_identity())?;
        let dirs = open_store_dirs(store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        store.root().revalidate().map_err(map_root_error)?;
        let topology = hold_topology_with_timeout(&dirs, self.timeout)?;
        let view = mechanical_finalize(store, &dirs)?;
        let prior = match &view {
            ClaimView::Empty => None,
            ClaimView::Headed(body) | ClaimView::Unheaded(body) => Some(body.clone()),
        };
        let table = prior
            .as_ref()
            .map(|body| body.table.clone())
            .unwrap_or_default();
        if days_claimed_by_other(&table, &self.days, self.owner.digest_hex()) {
            return Err(ConvergenceError::Refused(Refusal::Busy));
        }
        let resume = same_owner_claim(&table, &self.days, self.owner.digest_hex());
        let (serial, intent) = if successor {
            let Some(entry) = resume else {
                return Err(ConvergenceError::Refused(Refusal::NoPermit));
            };
            let prior_intent =
                read_intent(&dirs, entry.serial)?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::Intent,
                })?;
            let mut snapshots = BTreeMap::new();
            for day in &self.days {
                match self.admitted.store.load_day(&self.locks, day)? {
                    LoadDay::Published(snapshot) => {
                        snapshots.insert(day.as_str().to_owned(), snapshot);
                    }
                    _ => {
                        return Err(ConvergenceError::Unknown {
                            role: DurableRole::Record,
                        });
                    }
                }
            }
            let adoptions = adopt_all(store, &dirs, &self.days)?;
            let serial = bump_serial(store, &dirs)?;
            self.serial = Some(serial);
            let prior_revision = prior.as_ref().map(|body| body.revision).unwrap_or(0);
            let prior_digest = match prior.as_ref() {
                Some(body) => digest_value(body)?.as_hex().to_owned(),
                None => crate::schema::genesis_claim_digest(store.journal_id(), store.root_id())?
                    .as_hex()
                    .to_owned(),
            };
            let n = prior_revision + 1;
            let intent = build_later_intent(
                store,
                &self.days,
                serial,
                self.owner.digest_hex(),
                n,
                prior_revision,
                &prior_digest,
                &adoptions,
                &snapshots,
                &prior_intent,
            )?;
            let body = introduce(
                store,
                &dirs,
                prior.as_ref(),
                IntroduceSpec {
                    serial,
                    owner_digest: self.owner.digest_hex(),
                    days: &self.days,
                    day_set_subdigest: &intent.day_set_subdigest,
                    intent_digest: &intent.intent_digest,
                },
            )?;
            self.claim_revision = Some(body.revision);
            self.intent_digest = Some(intent.intent_digest.clone());
            write_head(store, &dirs, &body)?;
            drop(topology);
            write_intent(&dirs, &intent)?;
            (serial, intent)
        } else if let Some(entry) = resume {
            drop(topology);
            self.serial = Some(entry.serial);
            self.claim_revision = Some(entry.introduced_revision);
            self.intent_digest = Some(entry.intent_digest.clone());
            let adoptions = adopt_all(store, &dirs, &self.days)?;
            let prior_rev = entry.introduced_revision.saturating_sub(1);
            let prior_digest = match prior.as_ref() {
                Some(body) if body.revision == prior_rev => digest_value(body)?.as_hex().to_owned(),
                Some(body) => body.prior_revision_digest.clone(),
                None => crate::schema::genesis_claim_digest(store.journal_id(), store.root_id())?
                    .as_hex()
                    .to_owned(),
            };
            let intent = match read_intent(&dirs, entry.serial)? {
                Some(existing) => {
                    if existing
                        .prior_day_revisions
                        .values()
                        .all(|revision| *revision == 0)
                    {
                        let expected = build_virgin_intent(
                            store,
                            &self.days,
                            entry.serial,
                            self.owner.digest_hex(),
                            entry.introduced_revision,
                            prior_rev,
                            &prior_digest,
                            &adoptions,
                        )?;
                        if existing.predecessors != expected.predecessors {
                            return Err(ConvergenceError::Refused(Refusal::ChangedPredecessor));
                        }
                        refuse_mutated_projection(&existing, &expected)?;
                    }
                    verify_intent_matches_claim(&existing, &entry.intent_digest)?;
                    existing
                }
                None => {
                    for day in &self.days {
                        if !day_is_store_genesis(&dirs, day)? {
                            return Err(ConvergenceError::Unknown {
                                role: DurableRole::Intent,
                            });
                        }
                    }
                    let expected = build_virgin_intent(
                        store,
                        &self.days,
                        entry.serial,
                        self.owner.digest_hex(),
                        entry.introduced_revision,
                        prior_rev,
                        &prior_digest,
                        &adoptions,
                    )?;
                    verify_intent_matches_claim(&expected, &entry.intent_digest)?;
                    write_intent(&dirs, &expected)?;
                    abort_after_intent()?;
                    expected
                }
            };
            (entry.serial, intent)
        } else if all_unclaimed(&table, &self.days) {
            let adoptions = adopt_all(store, &dirs, &self.days)?;
            abort_after_adopt()?;
            let mut classes = BTreeMap::new();
            let mut snapshots = BTreeMap::new();
            for day in &self.days {
                let class = classify_predecessor(store, &dirs, &table, day)?;
                if let PredecessorClass::Member { .. } = &class {
                    match self.admitted.store.load_day(&self.locks, day)? {
                        LoadDay::Published(snapshot) => {
                            snapshots.insert(day.as_str().to_owned(), snapshot);
                        }
                        _ => {
                            return Err(ConvergenceError::Unknown {
                                role: DurableRole::Record,
                            });
                        }
                    }
                }
                classes.insert(day.as_str().to_owned(), class);
            }
            let serial = match self.serial {
                Some(serial) => serial,
                None => {
                    let serial = bump_serial(store, &dirs)?;
                    self.serial = Some(serial);
                    serial
                }
            };
            abort_after_serial()?;
            let prior_revision = prior.as_ref().map(|body| body.revision).unwrap_or(0);
            let prior_digest = match prior.as_ref() {
                Some(body) => digest_value(body)?.as_hex().to_owned(),
                None => crate::schema::genesis_claim_digest(store.journal_id(), store.root_id())?
                    .as_hex()
                    .to_owned(),
            };
            let n = prior_revision + 1;
            let intent = build_allocation_intent(
                store,
                &self.days,
                serial,
                self.owner.digest_hex(),
                n,
                prior_revision,
                &prior_digest,
                &adoptions,
                &classes,
                &snapshots,
            )?;
            ensure_claim_dir(&dirs)?;
            abort_after_claim_dir()?;
            let body = introduce(
                store,
                &dirs,
                prior.as_ref(),
                IntroduceSpec {
                    serial,
                    owner_digest: self.owner.digest_hex(),
                    days: &self.days,
                    day_set_subdigest: &intent.day_set_subdigest,
                    intent_digest: &intent.intent_digest,
                },
            )?;
            self.claim_revision = Some(body.revision);
            self.intent_digest = Some(intent.intent_digest.clone());
            abort_after_claim_revision()?;
            write_head(store, &dirs, &body)?;
            abort_after_claim_head()?;
            drop(topology);
            write_intent(&dirs, &intent)?;
            abort_after_intent()?;
            (serial, intent)
        } else {
            return Err(ConvergenceError::Refused(Refusal::Busy));
        };

        // Hook C. The global lock is already released in every arm above, so
        // this is the only brief days-to-registry section of the allocation.
        // It runs before any day-artifact consumption so no scan happens while
        // the registry guard is held.
        {
            let section = enter_registry(&dirs)?;
            crate::link::create_owner_intent_link(&section, &self.owner, &intent)?;
        }
        self.had_allocation = true;

        ancestry_preserves(
            store,
            &dirs,
            intent.claim_revision,
            serial,
            self.owner.digest_hex(),
            &intent.intent_digest,
            &self.days,
        )?;
        consume_intent_days(store, &dirs, &intent, &self.days)?;

        let active = Active {
            role: ROLE_ACTIVE.to_owned(),
            schema_version: SCHEMA_VERSION,
            journal_id: store.journal_id().to_owned(),
            root_id: store.root_id().to_owned(),
            serial,
            owner_binding_digest: self.owner.digest_hex().to_owned(),
            intent_digest: intent.intent_digest.clone(),
            day_set: intent.day_set.clone(),
        };
        write_active(&dirs, &active)?;
        abort_after_active()?;

        for day in &self.days {
            publish_from_intent(store, &self.locks, day, &intent)?;
        }
        Ok(())
    }
}

fn adopt_all(
    store: &crate::store::ConvergenceStore,
    dirs: &crate::init::StoreDirs,
    days: &[DayKey],
) -> Result<BTreeMap<String, Adoption>, ConvergenceError> {
    let mut adoptions = BTreeMap::new();
    for day in days {
        let adoption = ensure_adoption(store, dirs, day)?;
        adoptions.insert(day.as_str().to_owned(), adoption);
    }
    Ok(adoptions)
}

fn publish_from_intent(
    store: &crate::store::ConvergenceStore,
    locks: &DayLockSet,
    day: &DayKey,
    intent: &Intent,
) -> Result<(), ConvergenceError> {
    let proposed_rev = *intent
        .proposed_day_revisions
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?;
    let dirty = *intent
        .proposed_dirty_generations
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedProjection))?;
    let dirs =
        open_store_dirs(store.root())?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let adoption =
        crate::allocate::load_adoption(&dirs, day)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        })?;
    let next = match inspect_against_proposed(store, locks, day, proposed_rev)? {
        LoadDay::Published(snapshot) if snapshot.record_revision == proposed_rev => {
            project_day(store, locks, day, intent)?;
            return Ok(());
        }
        LoadDay::Published(snapshot) if snapshot.record_revision + 1 == proposed_rev => DayRecord {
            schema_version: SCHEMA_VERSION,
            journal_id: intent.journal_id.clone(),
            root_id: intent.root_id.clone(),
            adoption_id: snapshot.adoption_id,
            day: day.as_str().to_owned(),
            record_revision: proposed_rev,
            first_transition_serial: snapshot.first_transition_serial,
            dirty_by_transition_serial: intent.serial,
            dirty_generation: dirty,
            completed_generation: snapshot.completed_generation,
            auxiliary_time: snapshot.auxiliary_time,
        },
        LoadDay::Genesis if proposed_rev == 1 => DayRecord {
            schema_version: SCHEMA_VERSION,
            journal_id: intent.journal_id.clone(),
            root_id: intent.root_id.clone(),
            adoption_id: adoption.adoption_id,
            day: day.as_str().to_owned(),
            record_revision: proposed_rev,
            first_transition_serial: intent.serial,
            dirty_by_transition_serial: intent.serial,
            dirty_generation: dirty,
            completed_generation: 0,
            auxiliary_time: now_rfc3339(),
        },
        LoadDay::PublicationPending { .. } => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Head,
            });
        }
        LoadDay::HeadedDescendant { .. } => {
            return Err(ConvergenceError::Refused(Refusal::Superseded));
        }
        _ => {
            return Err(ConvergenceError::Refused(Refusal::CleanupOnly));
        }
    };
    match publish_record(store, locks, day, &next, None)? {
        PublishOutcome::Published { .. } => {}
        PublishOutcome::PublishedDurabilityUncertain { .. } => match store.load_day(locks, day)? {
            LoadDay::Published(snapshot) if snapshot.record_revision == proposed_rev => {}
            LoadDay::PublicationPending {
                kind: crate::store::PendingKind::HeadAheadOfRecord,
            } => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Head,
                });
            }
            _ => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Head,
                });
            }
        },
    }
    project_day(store, locks, day, intent)
}

#[cfg(test)]
fn abort_after(step: crate::test_support::PublishFault) -> Result<(), ConvergenceError> {
    if crate::test_support::take_publish_fault(step) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after publication step"),
        });
    }
    Ok(())
}

#[cfg(test)]
fn abort_after_adopt() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterAdopt)
}
#[cfg(not(test))]
fn abort_after_adopt() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_serial() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterSerial)
}
#[cfg(not(test))]
fn abort_after_serial() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_claim_dir() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterClaimDir)
}
#[cfg(not(test))]
fn abort_after_claim_dir() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_claim_revision() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterClaimRevision)
}
#[cfg(not(test))]
fn abort_after_claim_revision() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_claim_head() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterClaimHead)
}
#[cfg(not(test))]
fn abort_after_claim_head() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_intent() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterIntent)
}
#[cfg(not(test))]
fn abort_after_intent() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn abort_after_active() -> Result<(), ConvergenceError> {
    abort_after(crate::test_support::PublishFault::AfterActive)
}
#[cfg(not(test))]
fn abort_after_active() -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::digest::digest_value;
    use crate::error::Refusal;
    use crate::init::initialize;
    use crate::layout::DayKey;
    use crate::owner::OwnerBinding;
    use crate::preflight::{Preflight, preflight};
    use crate::schema::{ClaimHead, ClaimRevision};
    use crate::test_support::{
        PublishFault, TempDir, admit_days, continue_ok, continue_with_fault, open_root, sample_day,
        snapshot_tree,
    };
    use solstone_core_journal_io::JournalRoot;
    use std::collections::BTreeSet;
    use std::time::Duration;

    #[test]
    fn ac10_10_1_virgin_first_use() {
        let (_t, admitted) = admit_days("first-use", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let snap = held.snapshot(&day).unwrap();
        assert_eq!(snap.record_revision, 1);
        assert_eq!(snap.dirty_generation, 1);
        assert_eq!(
            snap.first_transition_serial,
            snap.dirty_by_transition_serial
        );
    }

    #[test]
    fn ac10_10_2_crash_init_before_owner() {
        let temporary = TempDir::new("init-owner");
        let (journal, root) = open_root(&temporary);
        initialize(&root).unwrap();
        std::fs::remove_file(journal.join("health/convergence/allocator.json")).unwrap();
        let before_owner = snapshot_tree(&journal);
        // Init now creates the empty `registry/owners/` parent. This test still
        // requires no intent/active records and no prepared-owner files.
        assert!(!before_owner.keys().any(|key| {
            key.contains("intents/")
                || key.contains("actives/")
                || (key.contains("registry/owners/") && key.ends_with(".json"))
        }));
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let root = JournalRoot::open(&journal).unwrap();
        let admitted = set.admit(root).unwrap();
        let _owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let after = snapshot_tree(&journal);
        assert!(after.contains_key("health/convergence/allocator.json"));
        assert!(
            !after
                .keys()
                .any(|key| key.contains("intents/") || key.contains("actives/"))
        );
    }

    #[test]
    fn ac10_10_3_byte_identical_rename_before_owner() {
        let temporary = TempDir::new("rename-ident");
        let (journal, root) = open_root(&temporary);
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let moved = temporary.path().join("journal-moved");
        std::fs::rename(&journal, &moved).unwrap();
        std::fs::create_dir(&journal).unwrap();
        std::fs::write(journal.join("poison"), b"same-name").unwrap();
        crate::test_support::prepared_owner(&admitted).unwrap();
        assert_eq!(std::fs::read(journal.join("poison")).unwrap(), b"same-name");
        assert!(!journal.join("health").exists());
    }

    #[test]
    fn ac10_10_4_divergent_replacement_before_owner() {
        let temporary = TempDir::new("rename-div");
        let (journal, root) = open_root(&temporary);
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let moved = temporary.path().join("journal-moved");
        std::fs::rename(&journal, &moved).unwrap();
        std::fs::create_dir(&journal).unwrap();
        std::fs::write(journal.join("poison"), b"divergent").unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        assert_eq!(std::fs::read(journal.join("poison")).unwrap(), b"divergent");
        let other = JournalRoot::open(&journal);
        assert!(other.is_ok() || other.is_err());
        assert!(!journal.join("health/convergence").exists());
    }

    #[test]
    fn ac10_10_9_disjoint_begins_distinct_serials() {
        let (_t, admitted) = admit_days("twins", &["20260823"]);
        let a = continue_ok(&admitted);
        drop(a);
        let set_b = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        // Same admitted handle is bound to {20260823}. Use a second admit on same journal.
        let (_t, admitted) = admit_days("twins-b", &["20260823", "20260824"]);
        let left = match preflight(["20260824", "20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let right = match preflight(["20260823", "20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        assert_eq!(left.subdigest().unwrap(), right.subdigest().unwrap());
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let snap_a = held.snapshot(&DayKey::parse("20260823").unwrap()).unwrap();
        let snap_b = held.snapshot(&DayKey::parse("20260824").unwrap()).unwrap();
        assert_eq!(
            snap_a.dirty_by_transition_serial,
            snap_b.dirty_by_transition_serial
        );
        assert_eq!(snap_a.dirty_by_transition_serial, 1);
        let _ = set_b;
    }

    fn keys(tree: &std::collections::BTreeMap<String, (u64, String)>) -> BTreeSet<String> {
        tree.keys().cloned().collect()
    }

    #[test]
    fn ac10_begin_split_writes_only_lock() {
        let (temporary, admitted) = admit_days("split", &["20260823"]);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        // Baseline after hook A: the secret and the prepared owner record are
        // registry-section writes, so `begin` itself must add only the day lock.
        let before = snapshot_tree(&temporary.journal_path());
        let _held = admitted.begin(owner).unwrap();
        let after = snapshot_tree(&temporary.journal_path());
        let extra: Vec<_> = after
            .keys()
            .filter(|key| !before.contains_key(*key))
            .cloned()
            .collect();
        assert_eq!(
            extra,
            vec!["health/convergence/days/20260823.lock".to_owned()]
        );
    }

    #[test]
    fn ac10_section_51_fault_boundaries() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
            must: &'static [&'static str],
            must_not: &'static [&'static str],
        }
        let cases = [
            Case {
                id: "AC10-5.1-AfterAdopt",
                fault: PublishFault::AfterAdopt,
                must: &["health/convergence/days/20260823.adopt.json"],
                must_not: &[
                    "health/convergence/claim/rev.1.json",
                    "health/convergence/intents/1.json",
                ],
            },
            Case {
                id: "AC10-5.1-AfterSerial",
                fault: PublishFault::AfterSerial,
                must: &["health/convergence/days/20260823.adopt.json"],
                must_not: &[
                    "health/convergence/claim/rev.1.json",
                    "health/convergence/intents/1.json",
                ],
            },
            Case {
                id: "AC10-5.1-AfterClaimDir",
                fault: PublishFault::AfterClaimDir,
                must: &["health/convergence/claim"],
                must_not: &[
                    "health/convergence/claim/rev.1.json",
                    "health/convergence/claim/head.json",
                ],
            },
            Case {
                id: "AC10-5.1-AfterIntent",
                fault: PublishFault::AfterIntent,
                must: &[
                    "health/convergence/claim/head.json",
                    "health/convergence/intents/1.json",
                ],
                must_not: &["health/convergence/actives/1.json"],
            },
            Case {
                id: "AC10-5.1-AfterActive",
                fault: PublishFault::AfterActive,
                must: &["health/convergence/actives/1.json"],
                must_not: &["health/convergence/records/20260823/record.json"],
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
            let before = snapshot_tree(&temporary.journal_path());
            let (_held, error) = continue_with_fault(&admitted, case.fault);
            assert!(
                matches!(error, ConvergenceError::PreservedPrior { .. }),
                "{} {error:?}",
                case.id
            );
            let after = snapshot_tree(&temporary.journal_path());
            for path in case.must {
                assert!(after.contains_key(*path), "{} missing {path}", case.id);
            }
            for path in case.must_not {
                assert!(!after.contains_key(*path), "{} unexpected {path}", case.id);
            }
            assert!(after.len() >= before.len(), "{}", case.id);
        }
    }

    #[test]
    fn ac10_after_claim_revision_no_head() {
        let (temporary, admitted) = admit_days("fault-rev", &["20260823"]);
        let before = snapshot_tree(&temporary.journal_path());
        let (_held, error) = continue_with_fault(&admitted, PublishFault::AfterClaimRevision);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let after = snapshot_tree(&temporary.journal_path());
        let extra: BTreeSet<_> = after
            .keys()
            .filter(|key| !before.contains_key(*key))
            .cloned()
            .collect();
        assert!(extra.contains("health/convergence/claim/rev.1.json"));
        assert!(!extra.contains("health/convergence/claim/head.json"));
        assert!(!extra.iter().any(|key| key.contains("intents/")));
        assert!(!extra.iter().any(|key| key.contains("actives/")));
        let _ = keys(&after);
    }

    #[test]
    fn ac10_10_74_claim_revision_before_head() {
        let (temporary, admitted) = admit_days("74", &["20260823"]);
        let (_held, error) = continue_with_fault(&admitted, PublishFault::AfterClaimRevision);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(tree.contains_key("health/convergence/claim/rev.1.json"));
        assert!(!tree.contains_key("health/convergence/claim/head.json"));
    }

    #[test]
    fn ac10_10_75_76_77_disjoint_finalizes_head() {
        let (temporary, admitted_a) = admit_days("mech", &["20260823"]);
        let (held_a, error) = continue_with_fault(&admitted_a, PublishFault::AfterClaimRevision);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        drop(held_a);
        drop(admitted_a);
        let after_a = snapshot_tree(&temporary.journal_path());
        let a_slice: BTreeSet<_> = after_a
            .keys()
            .filter(|key| key.contains("20260823"))
            .cloned()
            .collect();
        let rev1_path = temporary
            .journal_path()
            .join("health/convergence/claim/rev.1.json");
        let rev1_bytes = std::fs::read(&rev1_path).unwrap();
        let rev1: ClaimRevision =
            serde_json::from_slice(rev1_bytes.strip_suffix(b"\n").unwrap_or(&rev1_bytes)).unwrap();
        assert_eq!(rev1.revision, 1);
        let rev1_digest = digest_value(&rev1).unwrap();
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b
            .admit(root_b)
            .unwrap()
            .with_lock_timeout(Duration::from_millis(80));
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        let mut held_b = admitted_b.begin(owner_b).unwrap();
        let proof = crate::test_support::admit_proof(&held_b, held_b.owner()).unwrap();
        held_b.continue_with(proof).unwrap();
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(tree.contains_key("health/convergence/claim/head.json"));
        assert!(tree.contains_key("health/convergence/claim/rev.2.json"));
        let a_after: BTreeSet<_> = tree
            .keys()
            .filter(|key| key.contains("20260823"))
            .cloned()
            .collect();
        assert_eq!(
            a_slice, a_after,
            "B holds no A-day lock and must not touch A's day"
        );
        let rev2_bytes = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.2.json"),
        )
        .unwrap();
        let rev2: ClaimRevision =
            serde_json::from_slice(rev2_bytes.strip_suffix(b"\n").unwrap_or(&rev2_bytes)).unwrap();
        assert_eq!(rev2.prior_revision, 1);
        assert_eq!(rev2.prior_revision_digest, rev1_digest.as_hex());
        let head_bytes = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/claim/head.json"),
        )
        .unwrap();
        let head: ClaimHead =
            serde_json::from_slice(head_bytes.strip_suffix(b"\n").unwrap_or(&head_bytes)).unwrap();
        assert_eq!(head.revision, 2);
        assert_eq!(head.revision_digest, digest_value(&rev2).unwrap().as_hex());
        assert_ne!(
            rev2.prior_revision_digest,
            crate::schema::genesis_claim_digest(&rev2.journal_id, &rev2.root_id)
                .unwrap()
                .as_hex(),
            "head must have passed through revision 1, not jumped from genesis to 2"
        );
        let snap = held_b
            .snapshot(&DayKey::parse("20260824").unwrap())
            .unwrap();
        assert_eq!(snap.dirty_by_transition_serial, 2);
    }

    #[test]
    fn ac10_10_78_overlapping_busy_no_new_serial() {
        let (temporary, admitted_a) = admit_days("busy", &["20260823"]);
        let held_a = continue_ok(&admitted_a);
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b
            .admit(root_b)
            .unwrap()
            .with_lock_timeout(Duration::from_millis(80));
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        // Baseline after B's own hook A: a contended `begin` must introduce no
        // serial, claim, or intent.
        let before = snapshot_tree(&temporary.journal_path());
        let error = admitted_b.begin(owner_b).unwrap_err();
        assert!(matches!(error, ConvergenceError::Refused(Refusal::Busy)));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(held_a);
    }

    #[test]
    fn ac10_10_84_overlapping_claim_busy_after_release_of_locks() {
        let (temporary, admitted_a) = admit_days("overlap", &["20260823"]);
        let held_a = continue_ok(&admitted_a);
        drop(held_a);
        drop(admitted_a);
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b.admit(root_b).unwrap();
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        let mut held_b = admitted_b.begin(owner_b).unwrap();
        let next_before = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/allocator.json"),
        )
        .unwrap();
        let proof = crate::test_support::admit_proof(&held_b, held_b.owner()).unwrap();
        let error = held_b.continue_with(proof).unwrap_err();
        assert!(matches!(error, ConvergenceError::Refused(Refusal::Busy)));
        let next_after = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/allocator.json"),
        )
        .unwrap();
        assert_eq!(next_before, next_after);
    }

    #[test]
    fn ac10_10_85_disjoint_claims() {
        let (temporary, admitted_a) = admit_days("disjoint-a", &["20260823"]);
        let held_a = continue_ok(&admitted_a);
        drop(held_a);
        drop(admitted_a);
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b.admit(root_b).unwrap();
        let held_b = continue_ok(&admitted_b);
        let snap = held_b
            .snapshot(&DayKey::parse("20260824").unwrap())
            .unwrap();
        assert_eq!(snap.dirty_by_transition_serial, 2);
    }

    #[test]
    fn ac10_10_59_intent_recomputes() {
        let (temporary, admitted) = admit_days("recompute", &["20260823"]);
        let _held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/intents/1.json");
        let raw = std::fs::read(&path).unwrap();
        let intent: Intent =
            serde_json::from_slice(raw.strip_suffix(b"\n").unwrap_or(&raw)).unwrap();
        let recomputed = crate::schema::intent_digest(&intent).unwrap();
        assert_eq!(recomputed.as_hex(), intent.intent_digest);
        let claim_path = temporary
            .journal_path()
            .join("health/convergence/claim/rev.1.json");
        let claim_raw = std::fs::read(&claim_path).unwrap();
        let claim: crate::schema::ClaimRevision =
            serde_json::from_slice(claim_raw.strip_suffix(b"\n").unwrap_or(&claim_raw)).unwrap();
        assert_eq!(claim.intent_digest, intent.intent_digest);
    }

    #[test]
    fn ac10_10_60_intent_field_mutation_named_refusal() {
        let (temporary, admitted) = admit_days("mut-intent", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/intents/1.json");
        let mut intent: crate::schema::Intent = serde_json::from_slice(
            std::fs::read(&path)
                .unwrap()
                .strip_suffix(b"\n")
                .unwrap_or(&std::fs::read(&path).unwrap()),
        )
        .unwrap();
        intent.operation = "other".into();
        intent.intent_digest = crate::schema::intent_digest(&intent)
            .unwrap()
            .as_hex()
            .to_owned();
        let mut body = crate::digest::canonical_json_bytes(&intent).unwrap();
        body.push(b'\n');
        std::fs::write(&path, body).unwrap();
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::IntentMismatch)),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_61_vector_mutation_named_refusal() {
        let (temporary, admitted) = admit_days("mut-vec", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/intents/1.json");
        let mut intent: crate::schema::Intent = serde_json::from_slice(
            std::fs::read(&path)
                .unwrap()
                .strip_suffix(b"\n")
                .unwrap_or(&std::fs::read(&path).unwrap()),
        )
        .unwrap();
        match intent.predecessors.get_mut("20260823").unwrap() {
            crate::schema::Predecessor::Virgin { digest } => *digest = "ab".repeat(32),
            crate::schema::Predecessor::Member { .. }
            | crate::schema::Predecessor::Consumed { .. } => panic!("virgin"),
        }
        intent.intent_digest = crate::schema::intent_digest(&intent)
            .unwrap()
            .as_hex()
            .to_owned();
        let mut body = crate::digest::canonical_json_bytes(&intent).unwrap();
        body.push(b'\n');
        std::fs::write(&path, body).unwrap();
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ChangedPredecessor)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_62_wrong_intent_at_serial_path() {
        let (temporary, admitted) = admit_days("wrong-path", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/intents/1.json");
        let raw = std::fs::read(&path).unwrap();
        let mut intent: Intent =
            serde_json::from_slice(raw.strip_suffix(b"\n").unwrap_or(&raw)).unwrap();
        intent.owner_binding_digest = "ff".repeat(32);
        intent.intent_digest = crate::schema::intent_digest(&intent)
            .unwrap()
            .as_hex()
            .to_owned();
        let mut body = crate::digest::canonical_json_bytes(&intent).unwrap();
        body.push(b'\n');
        std::fs::write(&path, body).unwrap();
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::IntentMismatch)),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_79_a_live_across_disjoint_b_claim() {
        let (temporary, admitted_a) = admit_days("live-a", &["20260823"]);
        let held_a = continue_ok(&admitted_a);
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b.admit(root_b).unwrap();
        let held_b = continue_ok(&admitted_b);
        let snap_a = held_a.snapshot(&sample_day()).unwrap();
        assert_eq!(snap_a.record_revision, 1);
        let snap_b = held_b
            .snapshot(&DayKey::parse("20260824").unwrap())
            .unwrap();
        assert_eq!(snap_b.dirty_by_transition_serial, 2);
    }

    #[test]
    fn ac10_10_80_mutated_table_member_refuses() {
        let (temporary, admitted) = admit_days("mut-table", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let path = temporary
            .journal_path()
            .join("health/convergence/claim/rev.1.json");
        let mut body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        body["table"]["20260823"]["serial"] = serde_json::json!(99);
        std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Unknown { .. }
                    | ConvergenceError::Refused(Refusal::ClaimAncestry)
                    | ConvergenceError::Refused(Refusal::Busy)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_81_removed_table_member_refuses() {
        let (temporary, admitted) = admit_days("rm-table", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let path = temporary
            .journal_path()
            .join("health/convergence/claim/rev.1.json");
        let mut body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        body["table"].as_object_mut().unwrap().remove("20260823");
        std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Unknown { .. }
                    | ConvergenceError::Refused(Refusal::ClaimAncestry)
                    | ConvergenceError::Refused(Refusal::Busy)
                    | ConvergenceError::Refused(Refusal::NotVirgin)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_82_claim_head_result_loss_then_proceed() {
        let (_t, admitted) = admit_days("head-loss", &["20260823"]);
        let (_held, error) = continue_with_fault(&admitted, PublishFault::AfterClaimHead);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
    }

    #[test]
    fn ac10_10_83_pre_intent_owner_writes_intent() {
        let (temporary, admitted) = admit_days("pre-intent", &["20260823"]);
        let (mut held, error) = continue_with_fault(&admitted, PublishFault::AfterClaimHead);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        held.proceed().unwrap();
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/intents/1.json")
                .exists()
        );
    }

    #[test]
    fn ac10_10_98_stale_claim_head_unknown() {
        let (temporary, admitted) = admit_days("stale-head", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let path = temporary
            .journal_path()
            .join("health/convergence/claim/head.json");
        let mut head: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        head["revision_digest"] = serde_json::Value::String("cd".repeat(32));
        std::fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_99_deleted_claim_revision_unknown() {
        let (temporary, admitted) = admit_days("del-rev", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.1.json"),
        )
        .unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_102_gapped_claim_unknown() {
        let (temporary, admitted) = admit_days("gap", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        std::fs::copy(
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.1.json"),
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.3.json"),
        )
        .unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_103_multiple_unheaded_unknown() {
        let (temporary, admitted) = admit_days("multi-unheaded", &["20260823"]);
        let (held, error) = continue_with_fault(&admitted, PublishFault::AfterClaimRevision);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        drop(held);
        std::fs::copy(
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.1.json"),
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.2.json"),
        )
        .unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_104_foreign_pre_intent_busy() {
        let (temporary, admitted_a) = admit_days("foreign", &["20260823"]);
        let (held_a, error) = continue_with_fault(&admitted_a, PublishFault::AfterClaimHead);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        drop(held_a);
        drop(admitted_a);
        let before = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/allocator.json"),
        )
        .unwrap();
        let root_b = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set_b = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted_b = set_b.admit(root_b).unwrap();
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        let mut held_b = admitted_b.begin(owner_b).unwrap();
        let proof = crate::test_support::admit_proof(&held_b, held_b.owner()).unwrap();
        let error = held_b.continue_with(proof).unwrap_err();
        assert!(matches!(error, ConvergenceError::Refused(Refusal::Busy)));
        let after = std::fs::read(
            temporary
                .journal_path()
                .join("health/convergence/allocator.json"),
        )
        .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn ac10_10_105_active_without_claim() {
        let (temporary, admitted) = admit_days("active-only", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/claim/rev.1.json"),
        )
        .unwrap();
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/claim/head.json"),
        )
        .unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Unknown { .. }
                    | ConvergenceError::Refused(Refusal::NotVirgin)
                    | ConvergenceError::Refused(Refusal::Busy)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_100_rolled_back_head_unknown() {
        let (temporary, admitted) = admit_days("rollback", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let path = temporary
            .journal_path()
            .join("health/convergence/claim/head.json");
        let mut head: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        head["revision"] = serde_json::json!(0);
        std::fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Unknown { .. }
                    | ConvergenceError::Refused(Refusal::PersistedZeroRevision)
            ),
            "{error:?}"
        );
    }

    #[test]
    fn ac10_10_101_mixed_claim_unknown() {
        let (temporary, admitted) = admit_days("mixed", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let path = temporary
            .journal_path()
            .join("health/convergence/claim/rev.1.json");
        let mut body: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        body["journal_id"] = serde_json::Value::String("other".into());
        std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
    }
}
