// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Sealed grant capability delivery, its parent issuer, and successor fencing.
//!
//! # No plaintext token is stored
//!
//! Nothing on disk holds a capability. The token is *derived*: HMAC-SHA256
//! under the journal secret over a domain-separated canonical preimage naming
//! the journal, root, operation, owner binding, selector, serial, intent, the
//! exact member tuple, that member's digest, and the all-active barrier digest.
//! Reissue after a crash recomputes identical bytes from the same durable
//! evidence, which is what lets the outbox be reissued without ever having
//! persisted what it hands out.
//!
//! # The parent issuer
//!
//! Sealing happens only inside [`ParentAuthority`], which is crate-private, has
//! no public constructor, is never returned to a caller, and is built only by
//! the validated linked-transition path below. Knowing a tuple mints nothing:
//! there is no route to the seal from outside the crate, and no public
//! constructor takes a digest, sealed bytes, or a caller-chosen identity. A
//! fresh process therefore obtains authority only from exact durable issuer
//! state, never from anything a child could serialize.
//!
//! # Lock order
//!
//! The complete canonical day set is acquired in order. Then a brief global
//! section reads and mechanically finalizes the claim index, and is released.
//! Only then is the registry entered, briefly, to re-read owner, link,
//! decision, member, and barrier state. Global and registry never overlap, and
//! neither is held while waiting on a day. The canonical record, head, and
//! witness revalidation that immediately precedes returning bytes runs under
//! the day leases *after* the registry guard is dropped, so no day-artifact
//! scan happens inside a registry section.

use crate::access::ResolverAccess;
use crate::claim::ClaimView;
use crate::decision::{accept_barrier, accept_decision, load_barrier, load_decision, load_member};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::StoreDirs;
#[cfg(test)]
use crate::init::open_store_dirs;
use crate::layout::{ACTIVE_BARRIER_SUFFIX, DayKey};
use crate::link::{LinkResolution, load_owner_intent_link, resolve_owner_intent_link};
use crate::lock::DayLockSet;
#[cfg(test)]
use crate::lock::{LOCK_TIMEOUT, acquire_days_with_timeout, hold_topology_with_timeout};
use crate::mac::hmac_hex;
use crate::owner::{load_owner_binding, reauthenticate_owner};
use crate::permit::TerminalOutcome;
use crate::preflight::Admitted;
use crate::schema::{
    DecisionKind, GrantMember, MAC_GRANT_TOKEN, MemberState, PendingStage, PreparedOwnerState,
    grant_token_preimage_bytes,
};
use crate::secret::load_journal_secret;
use crate::selector::{GrantRequestSelector, OperationId, TargetScope, WriterFamily};
use crate::store::{ConvergenceStore, LoadDay, PendingKind};

/// A sealed grant capability. Not `Clone`, no `serde`, no public constructor,
/// and its `Debug` never prints the sealed bytes.
pub struct GrantToken {
    sealed: String,
    serial: u64,
    day: String,
    writer_family: WriterFamily,
    target_scope: TargetScope,
}

/// Opaque proof that this process still owns the complete ordered day lock
/// set. It is intentionally neither serializable nor clonable.
pub struct LiveGrantLease<'admitted> {
    access: ResolverAccess<'admitted>,
}

impl std::fmt::Debug for LiveGrantLease<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveGrantLease")
            .finish_non_exhaustive()
    }
}

/// A non-clonable authority whose lifetime is bounded by a live day lease.
/// It contains no token bytes and offers no way to reconstitute one.
pub struct GrantAuthority<'lease> {
    _lease: std::marker::PhantomData<&'lease ()>,
}

impl std::fmt::Debug for GrantAuthority<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrantAuthority")
            .finish_non_exhaustive()
    }
}

/// Result of live-lease authorization. Durable malformed evidence is returned
/// as `ConvergenceError::Unknown`; this enum only represents interpretable
/// pending and denied states.
#[derive(Debug)]
pub enum Authorization<'lease> {
    Granted(GrantAuthority<'lease>),
    Pending {
        stage: PendingStage,
        recovery: &'static str,
    },
    Denied {
        reason: DeniedReason,
    },
}

impl std::fmt::Debug for GrantToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrantToken")
            .field("serial", &self.serial)
            .field("day", &self.day)
            .field("writer_family", &self.writer_family)
            .field("target_scope", &self.target_scope)
            .finish_non_exhaustive()
    }
}

impl GrantToken {
    /// The sealed bytes handed to the holder.
    pub fn as_hex(&self) -> &str {
        &self.sealed
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn day(&self) -> &str {
        &self.day
    }

    pub fn writer_family(&self) -> WriterFamily {
        self.writer_family
    }

    pub fn target_scope(&self) -> TargetScope {
        self.target_scope
    }
}

mod sealed {
    /// Implemented only in this crate, so no foreign type can become a parent
    /// issuer.
    pub trait ParentGrant {
        fn seal_member(
            &self,
            member: &super::GrantMember,
            barrier_digest: &str,
        ) -> Result<super::GrantToken, super::ConvergenceError>;
    }
}

/// Opaque parent issuer authority. Crate-private, no public constructor, never
/// returned to a caller, unreachable from serialized child context.
pub(crate) struct ParentAuthority {
    journal_id: String,
    root_id: String,
    operation_id: String,
    owner_binding_digest: String,
    selector_digest: String,
    serial: u64,
    intent_digest: String,
    key_hex: String,
}

impl std::fmt::Debug for ParentAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParentAuthority")
            .field("serial", &self.serial)
            .finish_non_exhaustive()
    }
}

impl sealed::ParentGrant for ParentAuthority {
    fn seal_member(
        &self,
        member: &GrantMember,
        barrier_digest: &str,
    ) -> Result<GrantToken, ConvergenceError> {
        let preimage = grant_token_preimage_bytes(
            &self.journal_id,
            &self.root_id,
            &self.operation_id,
            &self.owner_binding_digest,
            &self.selector_digest,
            self.serial,
            &self.intent_digest,
            &member.tuple,
            &member.member_digest,
            barrier_digest,
        )?;
        Ok(GrantToken {
            sealed: hmac_hex(self.key_hex.as_bytes(), MAC_GRANT_TOKEN, &preimage),
            serial: self.serial,
            day: member.tuple.day.clone(),
            writer_family: member.tuple.writer_family,
            target_scope: member.tuple.target_scope,
        })
    }
}

impl ParentAuthority {
    pub(crate) fn seal(
        &self,
        member: &GrantMember,
        barrier_digest: &str,
    ) -> Result<GrantToken, ConvergenceError> {
        sealed::ParentGrant::seal_member(self, member, barrier_digest)
    }
}

/// Why delivery produced no bytes but is nonetheless a settled answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeniedReason {
    /// The decision and outbox exist but the exact committed terminal is not
    /// durable, so nothing may be handed out yet.
    NotCommitted,
    /// The transition was decided abort-no-open.
    AbortDecided,
    /// A durable same-generation completion for this tuple.
    SameGenerationCompletion,
    /// A verified later dirty descendant for this tuple.
    LaterDirtyDescendant,
    /// A live overlapping successor claim.
    OverlappingSuccessor,
    /// A durable pending or final prepared-owner revocation.
    OwnerRevoked,
    /// This member is revoked.
    MemberRevoked,
    /// This member is superseded.
    MemberSuperseded,
}

/// Outcome of a delivery or reissue attempt.
///
/// `Pending` and `Denied` are settled delivery outcomes. Uninterpretable
/// durable state is always returned as [`ConvergenceError::Unknown`].
#[derive(Debug)]
pub enum Delivery {
    /// Exact eligible members, with their sealed bytes.
    Ready(Vec<GrantToken>),
    /// A unique contiguous publication is in flight. No bytes, no write, no
    /// cleanup; `recovery` names the sole publication that owns it.
    Pending {
        stage: PendingStage,
        recovery: &'static str,
    },
    /// A settled denial. No bytes, and nothing is written here.
    Denied { reason: DeniedReason },
}

impl Delivery {
    pub fn tokens(&self) -> &[GrantToken] {
        match self {
            Self::Ready(tokens) => tokens,
            _ => &[],
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    pub fn denied_reason(&self) -> Option<DeniedReason> {
        match self {
            Self::Denied { reason } => Some(*reason),
            _ => None,
        }
    }
}

impl Admitted {
    /// Acquire the opaque complete ordered day-lock proof needed for mutation
    /// authorization. The proof remains live only while this value is held.
    pub fn grant_lease(&self) -> Result<LiveGrantLease<'_>, ConvergenceError> {
        Ok(LiveGrantLease {
            access: ResolverAccess::acquire(self)?,
        })
    }

    /// Deliver, or reissue, the sealed grant tokens for one operation.
    ///
    /// This is the only public path to a token. It is idempotent and writes
    /// nothing, so the same durable evidence yields the same bytes; that is
    /// what makes reissue safe across an activation-to-handoff crash.
    pub fn deliver_grants(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> Result<Delivery, ConvergenceError> {
        let store = self.store();
        store.revalidate()?;
        if selector.days() != self.days() {
            return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
        }
        let access = ResolverAccess::acquire(self)?;
        let dirs = access.dirs();
        let locks = access.locks();

        // Brief global, released before the registry is touched. An unheaded
        // introduction remains live for fencing; its owner gets the named
        // claim-head recovery rather than an implicit mutation during token
        // delivery.
        let claim = access.read_claim()?;

        // The operation's own link names the transition, which is the only
        // anchor that survives claim release.
        let linked = access.with_registry(|section| {
            let Some(secret) = load_journal_secret(section.registry())? else {
                return Ok(LinkLookup::MissingOwner);
            };
            let Some((owner, _state)) = load_owner_binding(
                section,
                operation,
                selector,
                store.object_identity(),
                store.journal_id(),
                store.root_id(),
                &secret.key_hex,
            )?
            else {
                return Ok(LinkLookup::MissingOwner);
            };
            match resolve_owner_intent_link(section, &owner)? {
                LinkResolution::Exact(link) => Ok(LinkLookup::Linked {
                    owner_digest: owner.digest_hex().to_owned(),
                    link,
                }),
                LinkResolution::Absent => Ok(LinkLookup::Absent {
                    owner_digest: owner.digest_hex().to_owned(),
                }),
                LinkResolution::Unknown => Ok(LinkLookup::UnknownLink),
            }
        })?;
        let (owner_digest, link) = match linked {
            LinkLookup::Linked { owner_digest, link } => {
                let link = *link;
                (owner_digest, Some(link))
            }
            LinkLookup::Absent { owner_digest } => (owner_digest, None),
            LinkLookup::MissingOwner => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            }
            LinkLookup::UnknownLink => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            }
        };

        let fence = classify_claim_fence(store, locks, dirs, &claim, &owner_digest, link.as_ref())?;
        if let ClaimFence::Own {
            serial,
            pending: Some((stage, recovery)),
        } = &fence
            && crate::terminal::read_terminal(dirs, *serial)?.is_none()
        {
            return Ok(Delivery::Pending {
                stage: *stage,
                recovery,
            });
        }

        let (serial, link) = match link {
            Some(link) => (link.serial, link),
            None => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            }
        };
        if matches!(fence, ClaimFence::Overlapping) {
            if crate::terminal::read_terminal(dirs, serial)?.is_some() {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::ClaimRevision,
                });
            }
            return Ok(Delivery::Denied {
                reason: DeniedReason::OverlappingSuccessor,
            });
        }
        let claim_released = claim_is_released(locks.days(), &claim);

        // The intent is unlinked during cleanup, so post-eviction the link is
        // the surviving anchor. While the intent is still present it must agree
        // exactly.
        if let Some(intent) = crate::intent::read_intent(dirs, serial)?
            && intent.intent_digest != link.intent_digest
        {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink,
            });
        }

        // Nothing may be handed out until the transition is committed, proven
        // either by the exact visible terminal during the cleanup window or, once
        // the terminal is evicted and the claim released, by the base committed
        // successor-clearance vector.
        match establish_committed(store, locks, dirs, serial, &link, claim_released)? {
            Committed::Yes => {}
            Committed::No { reason } => return Ok(Delivery::Denied { reason }),
            Committed::Unknown { role } => return Err(ConvergenceError::Unknown { role }),
        }
        // Brief registry: re-read owner, link, decision, members, barrier and
        // mint the already-classified authority. No day scan happens here.
        let prepared = access.with_registry(|section| {
            let Some(secret) = load_journal_secret(section.registry())? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                });
            };
            let Some((owner, state)) = load_owner_binding(
                section,
                operation,
                selector,
                store.object_identity(),
                store.journal_id(),
                store.root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            if state != PreparedOwnerState::Active
                || crate::revocation::owner_revocation_state(section, &owner, self.days())?
                    .is_some()
            {
                return Ok(RegistryPrepared::Delivery(Delivery::Denied {
                    reason: DeniedReason::OwnerRevoked,
                }));
            }
            // Authoritative re-read of the link under the registry lock.
            let Some(exact) = load_owner_intent_link(section, &owner, serial)? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            };
            if exact != link
                || exact.owner_binding_digest != owner.digest_hex()
                || exact.selector_digest != owner.selector_digest()
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            }
            let Some(decision) = load_decision(section, serial)? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Decision,
                });
            };
            if decision.kind == DecisionKind::AbortNoOpen {
                return Ok(RegistryPrepared::Delivery(Delivery::Denied {
                    reason: DeniedReason::AbortDecided,
                }));
            }
            let decision = accept_decision(
                decision,
                &owner,
                serial,
                &link.intent_digest,
                DecisionKind::Commit,
            )?;
            let Some(barrier) = load_barrier(section, serial, ACTIVE_BARRIER_SUFFIX)? else {
                // Members may be complete, but without the barrier the outbox
                // is not prepared and no subset may validate early.
                return Ok(RegistryPrepared::Delivery(Delivery::Denied {
                    reason: DeniedReason::NotCommitted,
                }));
            };
            let mut members = Vec::new();
            for tuple in &decision.tuples {
                let Some(member) = load_member(section, serial, tuple)? else {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::GrantMember,
                    });
                };
                if barrier
                    .member_digests
                    .get(&crate::decision::member_key(tuple))
                    != Some(&member.member_digest)
                {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::GrantActiveBarrier,
                    });
                }
                if crate::revocation::member_revocation_state(section, &member)?.is_some() {
                    return Ok(RegistryPrepared::Delivery(Delivery::Denied {
                        reason: DeniedReason::MemberRevoked,
                    }));
                }
                members.push(member);
            }
            let barrier = accept_barrier(
                barrier,
                &owner,
                &decision,
                &members,
                ACTIVE_BARRIER_SUFFIX,
                None,
            )?;
            Ok(RegistryPrepared::Prepared(Prepared {
                authority: ParentAuthority {
                    journal_id: store.journal_id().to_owned(),
                    root_id: store.root_id().to_owned(),
                    operation_id: operation.as_hex().to_owned(),
                    owner_binding_digest: owner.digest_hex().to_owned(),
                    selector_digest: owner.selector_digest().to_owned(),
                    serial,
                    intent_digest: link.intent_digest.clone(),
                    key_hex: secret.key_hex.clone(),
                },
                members,
                barrier_digest: barrier.barrier_digest.clone(),
            }))
        })?;
        let prepared = match prepared {
            RegistryPrepared::Prepared(prepared) => prepared,
            RegistryPrepared::Delivery(delivery) => return Ok(delivery),
        };

        // Registry released. The canonical revalidation immediately before
        // returning bytes runs under the day leases only.
        revalidate_then_seal(store, locks, &prepared)
    }
}

impl<'admitted> LiveGrantLease<'admitted> {
    /// Validate caller-supplied token bytes under this still-live complete day
    /// lease. Copying token bytes alone grants nothing: it cannot manufacture
    /// this opaque lock proof.
    #[allow(clippy::too_many_arguments)]
    pub fn authorize(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
        token_hex: &str,
        day: &DayKey,
        writer_family: WriterFamily,
        target_scope: TargetScope,
    ) -> Result<Authorization<'_>, ConvergenceError> {
        if selector.days() != self.access.days() || !self.access.locks().contains(day) {
            return Ok(Authorization::Denied {
                reason: DeniedReason::OverlappingSuccessor,
            });
        }
        self.access.store().revalidate()?;

        // The only global section is a bounded claim read; it is dropped
        // before the subsequent registry section.
        let claim = self.access.read_claim()?;
        let prepared = self.access.with_registry(|section| {
            let Some(secret) = load_journal_secret(section.registry())? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                });
            };
            let Some((owner, state)) = load_owner_binding(
                section,
                operation,
                selector,
                self.access.store().object_identity(),
                self.access.store().journal_id(),
                self.access.store().root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            if state != PreparedOwnerState::Active
                || crate::revocation::owner_revocation_state(section, &owner, self.access.days())?
                    .is_some()
            {
                return Ok(LeasePrepared::Denied(DeniedReason::OwnerRevoked));
            }
            reauthenticate_owner(
                section,
                self.access.store(),
                &owner,
                owner.transaction_class(),
                self.access.days(),
            )?;
            let LinkResolution::Exact(link) = resolve_owner_intent_link(section, &owner)? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            };
            let link = *link;
            let decision =
                load_decision(section, link.serial)?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::Decision,
                })?;
            let decision = accept_decision(
                decision,
                &owner,
                link.serial,
                &link.intent_digest,
                DecisionKind::Commit,
            )?;
            let Some(barrier) = load_barrier(section, link.serial, ACTIVE_BARRIER_SUFFIX)? else {
                return Ok(LeasePrepared::Denied(DeniedReason::NotCommitted));
            };
            let mut members = Vec::new();
            for tuple in &decision.tuples {
                let Some(member) = load_member(section, link.serial, tuple)? else {
                    return Ok(LeasePrepared::Denied(DeniedReason::NotCommitted));
                };
                members.push(member);
            }
            let Some(tuple) = decision.tuples.iter().find(|tuple| {
                tuple.day == day.as_str()
                    && tuple.writer_family == writer_family
                    && tuple.target_scope == target_scope
            }) else {
                return Ok(LeasePrepared::Denied(DeniedReason::NotCommitted));
            };
            let member = members
                .iter()
                .find(|member| &member.tuple == tuple)
                .cloned()
                .ok_or(ConvergenceError::Unknown {
                    role: DurableRole::GrantMember,
                })?;
            if crate::revocation::member_revocation_state(section, &member)?.is_some() {
                return Ok(LeasePrepared::Denied(DeniedReason::MemberRevoked));
            }
            if barrier
                .member_digests
                .get(&crate::decision::member_key(tuple))
                != Some(&member.member_digest)
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::GrantActiveBarrier,
                });
            }
            let barrier = accept_barrier(
                barrier,
                &owner,
                &decision,
                &members,
                ACTIVE_BARRIER_SUFFIX,
                None,
            )?;
            let authority = ParentAuthority {
                journal_id: self.access.store().journal_id().to_owned(),
                root_id: self.access.store().root_id().to_owned(),
                operation_id: operation.as_hex().to_owned(),
                owner_binding_digest: owner.digest_hex().to_owned(),
                selector_digest: owner.selector_digest().to_owned(),
                serial: link.serial,
                intent_digest: link.intent_digest.clone(),
                key_hex: secret.key_hex,
            };
            Ok(LeasePrepared::Ready(Box::new(LeaseReady {
                member,
                barrier_digest: barrier.barrier_digest,
                authority,
                link,
            })))
        })?;
        let LeasePrepared::Ready(prepared) = prepared else {
            let LeasePrepared::Denied(reason) = prepared else {
                unreachable!()
            };
            return Ok(Authorization::Denied { reason });
        };
        let LeaseReady {
            member,
            barrier_digest,
            authority,
            link,
        } = *prepared;

        let fence = classify_claim_fence(
            self.access.store(),
            self.access.locks(),
            self.access.dirs(),
            &claim,
            &authority.owner_binding_digest,
            Some(&link),
        )?;
        if let ClaimFence::Own {
            serial,
            pending: Some((stage, recovery)),
        } = &fence
            && crate::terminal::read_terminal(self.access.dirs(), *serial)?.is_none()
        {
            return Ok(Authorization::Pending {
                stage: *stage,
                recovery,
            });
        }
        if matches!(fence, ClaimFence::Overlapping) {
            if crate::terminal::read_terminal(self.access.dirs(), authority.serial)?.is_some() {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::ClaimRevision,
                });
            }
            return Ok(Authorization::Denied {
                reason: DeniedReason::OverlappingSuccessor,
            });
        }
        let claim_released = claim_is_released(self.access.days(), &claim);
        match establish_committed(
            self.access.store(),
            self.access.locks(),
            self.access.dirs(),
            authority.serial,
            &link,
            claim_released,
        )? {
            Committed::Yes => {}
            Committed::No { reason } => return Ok(Authorization::Denied { reason }),
            Committed::Unknown { role } => return Err(ConvergenceError::Unknown { role }),
        }

        // `ResolverAccess` owns bounded descriptor-bound day reads; the
        // subsequent shared classifier folds that same leased record into the
        // delivery/authorization polarity.
        let _current = self.access.load_day(day)?;
        match classify_member_state(self.access.store(), self.access.locks(), &member)? {
            MemberClass::Ready => {}
            MemberClass::Pending { stage, recovery } => {
                return Ok(Authorization::Pending { stage, recovery });
            }
            MemberClass::Denied(reason) => return Ok(Authorization::Denied { reason }),
        }
        let expected = authority.seal(&member, &barrier_digest)?;
        if expected.as_hex() != token_hex {
            return Ok(Authorization::Denied {
                reason: DeniedReason::NotCommitted,
            });
        }
        Ok(Authorization::Granted(GrantAuthority {
            _lease: std::marker::PhantomData,
        }))
    }
}

struct Prepared {
    authority: ParentAuthority,
    members: Vec<GrantMember>,
    barrier_digest: String,
}

enum RegistryPrepared {
    Prepared(Prepared),
    Delivery(Delivery),
}

enum LinkLookup {
    Linked {
        owner_digest: String,
        link: Box<crate::schema::OwnerIntentLink>,
    },
    Absent {
        owner_digest: String,
    },
    MissingOwner,
    UnknownLink,
}

enum ClaimFence {
    None,
    Own {
        serial: u64,
        pending: Option<(PendingStage, &'static str)>,
    },
    Overlapping,
}

fn claim_is_released<'a>(days: impl IntoIterator<Item = &'a DayKey>, claim: &ClaimView) -> bool {
    match claim {
        ClaimView::Empty => true,
        ClaimView::Headed(body) | ClaimView::Unheaded(body) => days
            .into_iter()
            .all(|day| !body.table.contains_key(day.as_str())),
    }
}

fn classify_claim_fence(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    claim: &ClaimView,
    owner_digest: &str,
    link: Option<&crate::schema::OwnerIntentLink>,
) -> Result<ClaimFence, ConvergenceError> {
    let (ClaimView::Headed(body) | ClaimView::Unheaded(body)) = claim else {
        return Ok(ClaimFence::None);
    };
    let entries: Vec<_> = locks
        .days()
        .iter()
        .filter_map(|day| body.table.get(day.as_str()))
        .collect();
    if entries.is_empty() {
        return Ok(ClaimFence::None);
    }
    if entries.len() != locks.days().len() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClaimRevision,
        });
    }
    let first = entries[0];
    if entries.iter().any(|entry| {
        entry.serial != first.serial
            || entry.intent_digest != first.intent_digest
            || entry.owner_binding_digest != first.owner_binding_digest
    }) {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClaimRevision,
        });
    }
    if first.owner_binding_digest != owner_digest {
        return Ok(ClaimFence::Overlapping);
    }
    if let Some(link) = link {
        if first.serial != link.serial {
            return Ok(ClaimFence::Overlapping);
        }
        if first.intent_digest != link.intent_digest {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink,
            });
        }
    }
    let pending = match claim {
        ClaimView::Unheaded(_) => Some((PendingStage::ClaimHead, "claim-head recovery")),
        ClaimView::Headed(_) => {
            let Some(intent) = crate::intent::read_intent(dirs, first.serial)? else {
                return Ok(ClaimFence::Own {
                    serial: first.serial,
                    pending: Some((PendingStage::ClaimIntent, "intent recovery")),
                });
            };
            if intent.serial != first.serial || intent.intent_digest != first.intent_digest {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Intent,
                });
            }
            let Some(active) = crate::intent::read_active(dirs, first.serial)? else {
                return Ok(ClaimFence::Own {
                    serial: first.serial,
                    pending: Some((PendingStage::ClaimConsumption, "consumption recovery")),
                });
            };
            if active.serial != first.serial
                || active.owner_binding_digest != first.owner_binding_digest
                || active.intent_digest != first.intent_digest
                || active.day_set != intent.day_set
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Active,
                });
            }
            let all_published = locks.days().iter().all(|day| {
                let expected = intent.proposed_day_revisions.get(day.as_str());
                matches!(
                    store.load_day(locks, day),
                    Ok(LoadDay::Published(snapshot)) if expected == Some(&snapshot.record_revision)
                )
            });
            (!all_published).then_some((
                PendingStage::ClaimDayPublication,
                "day publication recovery",
            ))
        }
        ClaimView::Empty => unreachable!(),
    };
    Ok(ClaimFence::Own {
        serial: first.serial,
        pending,
    })
}

enum LeasePrepared {
    Ready(Box<LeaseReady>),
    Denied(DeniedReason),
}

struct LeaseReady {
    member: GrantMember,
    barrier_digest: String,
    authority: ParentAuthority,
    link: crate::schema::OwnerIntentLink,
}

enum MemberClass {
    Ready,
    Pending {
        stage: PendingStage,
        recovery: &'static str,
    },
    Denied(DeniedReason),
}

/// Immediately before returning bytes, revalidate each tuple's canonical
/// current record against the generation the member was bound to.
fn revalidate_then_seal(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    prepared: &Prepared,
) -> Result<Delivery, ConvergenceError> {
    let mut tokens = Vec::new();
    for member in &prepared.members {
        match classify_member_state(store, locks, member)? {
            MemberClass::Ready => {}
            MemberClass::Pending { stage, recovery } => {
                return Ok(Delivery::Pending { stage, recovery });
            }
            MemberClass::Denied(reason) => return Ok(Delivery::Denied { reason }),
        }
        tokens.push(prepared.authority.seal(member, &prepared.barrier_digest)?);
    }
    Ok(Delivery::Ready(tokens))
}

/// The shared under-day member classifier. Delivery and lease authorization
/// consume this exact result so a canonical record cannot gain different
/// polarity at the two token boundaries.
fn classify_member_state(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    member: &GrantMember,
) -> Result<MemberClass, ConvergenceError> {
    match member.state {
        MemberState::Active => {}
        MemberState::RevocationPending | MemberState::Revoked => {
            return Ok(MemberClass::Denied(DeniedReason::MemberRevoked));
        }
        MemberState::Superseded => {
            return Ok(MemberClass::Denied(DeniedReason::MemberSuperseded));
        }
    }
    let day = DayKey::parse(&member.tuple.day)?;
    match store.load_day(locks, &day)? {
        LoadDay::Published(snapshot) => {
            if snapshot.dirty_generation != member.tuple.dirty_generation
                || snapshot.dirty_by_transition_serial != member.tuple.dirty_by_transition_serial
            {
                return Ok(MemberClass::Denied(DeniedReason::LaterDirtyDescendant));
            }
            if snapshot.completed_generation >= member.tuple.dirty_generation {
                return Ok(MemberClass::Denied(DeniedReason::SameGenerationCompletion));
            }
            Ok(MemberClass::Ready)
        }
        LoadDay::PublicationPending { kind } => Ok(MemberClass::Pending {
            stage: match kind {
                PendingKind::WitnessAheadOfHead => PendingStage::WitnessAheadOfHead,
                PendingKind::HeadAheadOfRecord => PendingStage::HeadAheadOfRecord,
            },
            recovery: "day publication recovery",
        }),
        LoadDay::HeadedDescendant { .. } => {
            Ok(MemberClass::Denied(DeniedReason::LaterDirtyDescendant))
        }
        LoadDay::Genesis => Err(ConvergenceError::Unknown {
            role: DurableRole::Record,
        }),
    }
}

/// Whether the transition is committed, and by which evidence.
pub(crate) enum Committed {
    Yes,
    No { reason: DeniedReason },
    Unknown { role: DurableRole },
}

/// The exact visible terminal during the cleanup window, or the post-eviction
/// nonempty-committed clearance vector once the claim is released.
pub(crate) fn establish_committed(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    serial: u64,
    link: &crate::schema::OwnerIntentLink,
    claim_released: bool,
) -> Result<Committed, ConvergenceError> {
    if let Some(terminal) = crate::terminal::read_terminal(dirs, serial)? {
        let (expected_resolved, expected_adoption_ids) =
            match crate::terminal::expected_terminal_values(store, locks, dirs, locks.days()) {
                Ok(values) => values,
                Err(ConvergenceError::Unknown { role }) => return Ok(Committed::Unknown { role }),
                Err(error) => return Err(error),
            };
        let terminal = match crate::terminal::accept_terminal(
            terminal,
            link,
            &expected_resolved,
            &expected_adoption_ids,
        ) {
            Ok((terminal, _digest)) => terminal,
            Err(ConvergenceError::Unknown { role }) => return Ok(Committed::Unknown { role }),
            Err(error) => return Err(error),
        };
        return Ok(match crate::permit::parse_outcome(&terminal.outcome) {
            Some(TerminalOutcome::Committed) => Committed::Yes,
            Some(_) => Committed::No {
                reason: DeniedReason::NotCommitted,
            },
            None => Committed::Unknown {
                role: DurableRole::Terminal,
            },
        });
    }
    if !claim_released {
        // Terminal absent while the claim is still live is a cleanup prefix,
        // not a committed outcome.
        return Ok(Committed::No {
            reason: DeniedReason::NotCommitted,
        });
    }
    // Terminal evicted and claim released: require the complete base committed
    // successor-clearance vector. Any missing or mismatched member makes the
    // outcome unknown rather than assumed.
    let Some(barrier) = crate::terminal::read_clearance_barrier(dirs, serial)? else {
        return Ok(Committed::Unknown {
            role: DurableRole::ClearanceBarrier,
        });
    };
    if barrier.day_set != link.day_set {
        return Ok(Committed::Unknown {
            role: DurableRole::ClearanceBarrier,
        });
    }
    for day in locks.days() {
        let Some(member) = crate::terminal::read_clearance_member(dirs, day)? else {
            return Ok(Committed::Unknown {
                role: DurableRole::ClearanceMember,
            });
        };
        if member.serial != serial {
            return Ok(Committed::Unknown {
                role: DurableRole::ClearanceMember,
            });
        }
        if !barrier.member_digests.contains_key(day.as_str()) {
            return Ok(Committed::Unknown {
                role: DurableRole::ClearanceBarrier,
            });
        }
        match crate::permit::parse_outcome(&member.outcome) {
            Some(TerminalOutcome::Committed) => {}
            Some(_) => {
                return Ok(Committed::No {
                    reason: DeniedReason::NotCommitted,
                });
            }
            None => {
                return Ok(Committed::Unknown {
                    role: DurableRole::ClearanceMember,
                });
            }
        }
    }
    Ok(Committed::Yes)
}

/// The only two canonical changes that permit pruning an immutable member.
/// This delegates to the shared delivery/authorization classifier so the
/// revocation path cannot invent a second polarity table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PruneGate {
    SameGenerationCompletion,
    LaterDirtyDescendant,
}

pub(crate) fn prune_gate(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    member: &GrantMember,
) -> Result<Option<PruneGate>, ConvergenceError> {
    match classify_member_state(store, locks, member)? {
        MemberClass::Denied(DeniedReason::SameGenerationCompletion) => {
            Ok(Some(PruneGate::SameGenerationCompletion))
        }
        MemberClass::Denied(DeniedReason::LaterDirtyDescendant) => {
            Ok(Some(PruneGate::LaterDirtyDescendant))
        }
        MemberClass::Ready | MemberClass::Pending { .. } | MemberClass::Denied(_) => Ok(None),
    }
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::owner::OwnerBinding;
    use crate::preflight::{Preflight, preflight};
    use crate::publish::{
        PreparedCompletionAuthority, PreparedLaterDirtyAuthority, publish_kind_for_test,
    };
    use crate::test_support::{PublishFault, TempDir, admit_days, fail_after, snapshot_tree};

    fn requests() -> Vec<(&'static str, WriterFamily, TargetScope)> {
        vec![
            ("20260823", WriterFamily::Think, TargetScope::Chronicle),
            ("20260823", WriterFamily::Observe, TargetScope::Entities),
        ]
    }

    fn reopen(temporary: &TempDir, days: &[&str]) -> Admitted {
        let root = solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        match preflight(days.iter().copied()).unwrap() {
            Preflight::Ready(set) => set.admit(root).unwrap(),
            Preflight::Empty => panic!("days"),
        }
    }

    /// Commit one nonempty-grant transition and return everything needed to
    /// ask for its tokens later.
    fn committed(name: &str) -> (TempDir, Admitted, OperationId, GrantRequestSelector) {
        let (temporary, admitted) = admit_days(name, &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
        drop(held);
        (temporary, admitted, operation, selector)
    }

    fn with_live_token(
        name: &str,
        assertion: impl FnOnce(
            &TempDir,
            &Admitted,
            &OperationId,
            &GrantRequestSelector,
            &GrantToken,
            &LiveGrantLease<'_>,
        ),
    ) {
        let (temporary, admitted, operation, selector) = committed(name);
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let lease = admitted.grant_lease().unwrap();
        assertion(
            &temporary,
            &admitted,
            &operation,
            &selector,
            &delivery.tokens()[0],
            &lease,
        );
    }

    fn rewrite_member_state(temporary: &TempDir, token: &GrantToken, state: MemberState) {
        let directory = temporary
            .journal_path()
            .join("health/convergence/registry/grants/members")
            .join(token.serial().to_string());
        let path = directory.join(format!(
            "{}.{}.{}.json",
            token.day(),
            token.writer_family().as_str(),
            token.target_scope().as_str(),
        ));
        let mut member: GrantMember =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        member.state = state;
        member.member_digest = String::new();
        member.member_digest = crate::digest::digest_value_excluding(&member, "member_digest")
            .unwrap()
            .as_hex()
            .to_owned();
        let mut bytes = crate::digest::canonical_json_bytes(&member).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();

        let path = temporary.journal_path().join(format!(
            "health/convergence/registry/grants/barriers/{}.active.json",
            token.serial(),
        ));
        let mut barrier: crate::schema::GrantBarrier =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        barrier.member_digests.insert(
            crate::decision::member_key(&member.tuple),
            member.member_digest,
        );
        barrier.barrier_digest = crate::digest::digest_value_excluding(&barrier, "barrier_digest")
            .unwrap()
            .as_hex()
            .to_owned();
        let mut bytes = crate::digest::canonical_json_bytes(&barrier).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
    }

    fn exact_link(
        admitted: &Admitted,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> (OwnerBinding, crate::schema::OwnerIntentLink) {
        let owner = OwnerBinding::prepare(
            admitted,
            operation,
            crate::selector::TransactionClass::AdvanceDirty,
            selector,
        )
        .unwrap();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let link = crate::access::with_registry(&dirs, admitted.lock_timeout(), |section| {
            let LinkResolution::Exact(link) = resolve_owner_intent_link(section, &owner)? else {
                panic!("exact link")
            };
            Ok(*link)
        })
        .unwrap();
        (owner, link)
    }

    fn append_claim(
        admitted: &Admitted,
        owner: &OwnerBinding,
        serial: u64,
        intent_digest: &str,
        publish_head: bool,
    ) {
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let topology = hold_topology_with_timeout(&dirs, admitted.lock_timeout()).unwrap();
        let prior = match crate::claim::classify(admitted.store(), &dirs).unwrap() {
            ClaimView::Empty => None,
            ClaimView::Headed(body) | ClaimView::Unheaded(body) => Some(body),
        };
        let day_set_subdigest = crate::schema::day_set_subdigest(admitted.days())
            .unwrap()
            .as_hex()
            .to_owned();
        let body = crate::claim::introduce(
            admitted.store(),
            &dirs,
            prior.as_ref(),
            crate::claim::IntroduceSpec {
                serial,
                owner_digest: owner.digest_hex(),
                days: admitted.days(),
                day_set_subdigest: &day_set_subdigest,
                intent_digest,
            },
        )
        .unwrap();
        if publish_head {
            crate::claim::write_head(admitted.store(), &dirs, &body).unwrap();
        }
        drop(topology);
    }

    fn successor_prefix(admitted: &Admitted, selector: &GrantRequestSelector, fault: PublishFault) {
        let operation = OperationId::generate().unwrap();
        let owner = OwnerBinding::prepare(
            admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(fault);
        assert!(held.continue_with(proof).is_err());
        drop(guard);
        drop(held);
    }

    fn assert_successor_prefix_fenced(fault: PublishFault) {
        let (_temporary, admitted, operation, selector) = committed("successor-prefix");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let writer_family = token.writer_family();
        let target_scope = token.target_scope();
        successor_prefix(&admitted, &selector, fault);
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Denied {
                reason: DeniedReason::OverlappingSuccessor
            }
        ));
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token_hex,
                    &day,
                    writer_family,
                    target_scope,
                )
                .unwrap(),
            Authorization::Denied {
                reason: DeniedReason::OverlappingSuccessor
            }
        ));
    }

    fn assert_day_publication_pending(fault: PublishFault, stage: PendingStage) {
        let (temporary, admitted, operation, selector) = committed("day-publication-pending");
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store().journal_id(),
            admitted.store().root_id(),
            admitted.store().object_identity(),
            LOCK_TIMEOUT,
        )
        .unwrap();
        let guard = fail_after(fault);
        assert!(
            publish_kind_for_test(
                admitted.store(),
                &locks,
                &DayKey::parse("20260823").unwrap(),
                PreparedLaterDirtyAuthority,
            )
            .is_err()
        );
        drop(guard);
        drop(locks);
        let before = snapshot_tree(&temporary.journal_path());
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        assert!(matches!(
            delivery,
            Delivery::Pending {
                stage: observed,
                recovery: "day publication recovery",
            } if observed == stage
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    fn assert_own_claim_prefix_pending(
        fault: PublishFault,
        stage: PendingStage,
        recovery: &'static str,
    ) {
        let (temporary, admitted) = admit_days("own-claim-prefix", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(fault);
        assert!(matches!(
            held.continue_with(proof),
            Err(ConvergenceError::PreservedPrior { .. })
        ));
        drop(guard);
        drop(held);

        let before = snapshot_tree(&temporary.journal_path());
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        assert!(matches!(
            delivery,
            Delivery::Pending {
                stage: observed,
                recovery: observed_recovery,
            } if observed == stage && observed_recovery == recovery
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn own_unheaded_claim_is_pending_without_bytes_or_writes() {
        let (temporary, admitted, operation, selector) = committed("own-claim-pending");
        let (owner, link) = exact_link(&admitted, &operation, &selector);
        append_claim(&admitted, &owner, link.serial, &link.intent_digest, false);
        let before = snapshot_tree(&temporary.journal_path());
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        assert!(matches!(
            delivery,
            Delivery::Pending {
                stage: PendingStage::ClaimHead,
                recovery: "claim-head recovery",
            }
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(delivery);
    }

    #[test]
    fn own_unheaded_claim_authorization_is_pending() {
        let (_temporary, admitted, operation, selector) = committed("own-claim-authorize");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let writer_family = token.writer_family();
        let target_scope = token.target_scope();
        let (owner, link) = exact_link(&admitted, &operation, &selector);
        append_claim(&admitted, &owner, link.serial, &link.intent_digest, false);
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token_hex,
                    &day,
                    writer_family,
                    target_scope,
                )
                .unwrap(),
            Authorization::Pending {
                stage: PendingStage::ClaimHead,
                recovery: "claim-head recovery",
            }
        ));
    }

    #[test]
    fn own_headed_pre_intent_claim_is_pending_for_intent_recovery() {
        // Contrasts with `overlapping_headed_pre_intent_successor_denies`:
        // the same claim-head prefix belongs to this operation, not a newer
        // claimant, so only its intent recovery may continue it.
        assert_own_claim_prefix_pending(
            PublishFault::AfterClaimHead,
            PendingStage::ClaimIntent,
            "intent recovery",
        );
    }

    #[test]
    fn own_intent_before_consumption_claim_is_pending_for_consumption_recovery() {
        // Contrasts with `overlapping_intent_before_consumption_successor_denies`.
        assert_own_claim_prefix_pending(
            PublishFault::AfterIntent,
            PendingStage::ClaimConsumption,
            "consumption recovery",
        );
    }

    #[test]
    fn own_witness_before_dirty_record_claim_is_pending_for_day_publication() {
        // Contrasts with `overlapping_witness_before_dirty_record_successor_denies`.
        assert_own_claim_prefix_pending(
            PublishFault::AfterActive,
            PendingStage::ClaimDayPublication,
            "day publication recovery",
        );
    }

    #[test]
    fn overlapping_unheaded_successor_is_live_before_and_after_head_recovery() {
        let (temporary, admitted, operation, selector) = committed("successor-unheaded");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let writer_family = token.writer_family();
        let target_scope = token.target_scope();
        successor_prefix(&admitted, &selector, PublishFault::AfterClaimRevision);
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Denied {
                reason: DeniedReason::OverlappingSuccessor
            }
        ));
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let topology = hold_topology_with_timeout(&dirs, admitted.lock_timeout()).unwrap();
        assert!(matches!(
            crate::claim::mechanical_finalize(admitted.store(), &dirs).unwrap(),
            ClaimView::Headed(_)
        ));
        drop(topology);
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Denied {
                reason: DeniedReason::OverlappingSuccessor
            }
        ));
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token_hex,
                    &day,
                    writer_family,
                    target_scope,
                )
                .unwrap(),
            Authorization::Denied {
                reason: DeniedReason::OverlappingSuccessor
            }
        ));
        drop(temporary);
    }

    #[test]
    fn overlapping_headed_pre_intent_successor_denies() {
        assert_successor_prefix_fenced(PublishFault::AfterClaimHead);
    }

    #[test]
    fn overlapping_intent_before_consumption_successor_denies() {
        assert_successor_prefix_fenced(PublishFault::AfterIntent);
    }

    #[test]
    fn overlapping_witness_before_dirty_record_successor_denies() {
        assert_successor_prefix_fenced(PublishFault::AfterActive);
    }

    #[test]
    fn terminal_visible_overlapping_successor_is_unknown_without_bytes() {
        let (temporary, admitted, operation, selector, serial) =
            visible_terminal_prefix("terminal-overlap");
        let ready = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &ready.tokens()[0];
        let token_hex = token.as_hex().to_owned();
        let day = DayKey::parse(token.day()).unwrap();
        let writer_family = token.writer_family();
        let target_scope = token.target_scope();
        let (owner, link) = exact_link(&admitted, &operation, &selector);
        append_claim(
            &admitted,
            &owner,
            serial + 1,
            &format!("{}-successor", link.intent_digest),
            true,
        );
        let before = snapshot_tree(&temporary.journal_path());
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision
            })
        ));
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease.authorize(
                &operation,
                &selector,
                &token_hex,
                &day,
                writer_family,
                target_scope,
            ),
            Err(ConvergenceError::Unknown {
                role: DurableRole::ClaimRevision
            })
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn witness_before_head_is_pending_without_delivery_write() {
        assert_day_publication_pending(
            PublishFault::AfterWitness,
            PendingStage::WitnessAheadOfHead,
        );
    }

    #[test]
    fn head_before_record_is_pending_without_delivery_write() {
        assert_day_publication_pending(PublishFault::AfterHead, PendingStage::HeadAheadOfRecord);
    }

    fn assert_disjoint_claim_is_harmless(fault: PublishFault) {
        let (temporary, admitted, operation, selector) = committed("disjoint-claim");
        let other = reopen(&temporary, &["20260824"]);
        let other_selector = GrantRequestSelector::try_new(
            other.days(),
            [("20260824", WriterFamily::Think, TargetScope::Chronicle)],
        )
        .unwrap();
        successor_prefix(&other, &other_selector, fault);
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector).unwrap(),
            Delivery::Ready(_)
        ));
    }

    #[test]
    fn disjoint_unheaded_claim_is_harmless() {
        assert_disjoint_claim_is_harmless(PublishFault::AfterClaimRevision);
    }

    #[test]
    fn disjoint_headed_claim_is_harmless() {
        assert_disjoint_claim_is_harmless(PublishFault::AfterClaimHead);
    }

    #[test]
    fn committed_transition_delivers_sealed_tokens() {
        let (_temporary, admitted, operation, selector) = committed("deliver");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.is_ready(), "{delivery:?}");
        let tokens = delivery.tokens();
        assert_eq!(tokens.len(), 2);
        for token in tokens {
            // Sealed hex, not a stored secret and not a digest of one.
            assert_eq!(token.as_hex().len(), 64);
            assert!(token.as_hex().bytes().all(|b| b.is_ascii_hexdigit()));
        }
        // Distinct tuples seal to distinct bytes.
        assert_ne!(tokens[0].as_hex(), tokens[1].as_hex());
    }

    #[test]
    fn live_lease_accepts_exact_token_and_denies_forged_bytes() {
        let (_temporary, admitted, operation, selector) = committed("lease-authorize");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let day = DayKey::parse(token.day()).unwrap();
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    token.as_hex(),
                    &day,
                    token.writer_family(),
                    token.target_scope(),
                )
                .unwrap(),
            Authorization::Granted(_)
        ));
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &"00".repeat(32),
                    &day,
                    token.writer_family(),
                    token.target_scope(),
                )
                .unwrap(),
            Authorization::Denied { .. }
        ));
    }

    #[test]
    fn authorize_rejects_wrong_target_token() {
        with_live_token("lease-target", |_, _, operation, selector, token, lease| {
            let day = DayKey::parse(token.day()).unwrap();
            let wrong_target = match token.target_scope() {
                TargetScope::Chronicle => TargetScope::Entities,
                TargetScope::Entities => TargetScope::Chronicle,
                TargetScope::Facets => TargetScope::Chronicle,
            };
            assert!(matches!(
                lease
                    .authorize(
                        operation,
                        selector,
                        token.as_hex(),
                        &day,
                        token.writer_family(),
                        wrong_target,
                    )
                    .unwrap(),
                Authorization::Denied { .. }
            ));
        });
    }

    #[test]
    fn authorize_rejects_forged_token_bytes() {
        with_live_token("lease-forged", |_, _, operation, selector, token, lease| {
            let day = DayKey::parse(token.day()).unwrap();
            assert!(matches!(
                lease
                    .authorize(
                        operation,
                        selector,
                        &"00".repeat(32),
                        &day,
                        token.writer_family(),
                        token.target_scope(),
                    )
                    .unwrap(),
                Authorization::Denied { .. }
            ));
        });
    }

    #[test]
    fn authorize_rejects_revocation_pending_member() {
        with_live_token(
            "lease-member-pending",
            |temporary, _, operation, selector, token, lease| {
                rewrite_member_state(temporary, token, MemberState::RevocationPending);
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::MemberRevoked
                    }
                ));
            },
        );
    }

    #[test]
    fn authorize_rejects_revoked_member() {
        with_live_token(
            "lease-member-revoked",
            |temporary, _, operation, selector, token, lease| {
                rewrite_member_state(temporary, token, MemberState::Revoked);
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::MemberRevoked
                    }
                ));
            },
        );
    }

    #[test]
    fn authorize_rejects_superseded_member() {
        with_live_token(
            "lease-member-superseded",
            |temporary, _, operation, selector, token, lease| {
                rewrite_member_state(temporary, token, MemberState::Superseded);
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::MemberSuperseded
                    }
                ));
            },
        );
    }

    #[test]
    fn authorize_rejects_stale_token_after_later_dirty() {
        let (temporary, admitted, operation, selector) = committed("lease-stale");
        let token = admitted
            .deliver_grants(&operation, &selector)
            .unwrap()
            .tokens()[0]
            .as_hex()
            .to_owned();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store().journal_id(),
            admitted.store().root_id(),
            admitted.store().object_identity(),
            LOCK_TIMEOUT,
        )
        .unwrap();
        publish_kind_for_test(
            admitted.store(),
            &locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        drop(locks);
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    &token,
                    &DayKey::parse("20260823").unwrap(),
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            Authorization::Denied {
                reason: DeniedReason::LaterDirtyDescendant
            }
        ));
        drop(temporary);
    }

    #[test]
    fn authorize_denies_pending_owner_without_authority() {
        with_live_token(
            "lease-owner-pending",
            |temporary, admitted, operation, selector, token, lease| {
                let path = temporary
                    .journal_path()
                    .join("health/convergence/registry/owners")
                    .join(format!("{}.json", operation.as_hex()));
                let mut record: crate::schema::PreparedOwner =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                record.state = PreparedOwnerState::RevocationPending;
                let mut bytes = crate::digest::canonical_json_bytes(&record).unwrap();
                bytes.push(b'\n');
                std::fs::write(path, bytes).unwrap();
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::OwnerRevoked
                    }
                ));
                let _ = admitted;
            },
        );
    }

    #[test]
    fn authorize_denies_revoked_owner_without_authority() {
        with_live_token(
            "lease-owner-revoked",
            |temporary, _, operation, selector, token, lease| {
                let path = temporary
                    .journal_path()
                    .join("health/convergence/registry/owners")
                    .join(format!("{}.json", operation.as_hex()));
                let mut record: crate::schema::PreparedOwner =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                record.state = PreparedOwnerState::Revoked;
                let mut bytes = crate::digest::canonical_json_bytes(&record).unwrap();
                bytes.push(b'\n');
                std::fs::write(path, bytes).unwrap();
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::OwnerRevoked
                    }
                ));
            },
        );
    }

    #[test]
    fn discovery_to_locked_owner_change_is_unknown_without_authority() {
        with_live_token(
            "lease-discovery-change",
            |temporary, _, operation, selector, token, lease| {
                let path = temporary
                    .journal_path()
                    .join("health/convergence/registry/owners")
                    .join(format!("{}.json", operation.as_hex()));
                let mut record: crate::schema::PreparedOwner =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                record.owner_binding_mac = "00".repeat(32);
                let mut bytes = crate::digest::canonical_json_bytes(&record).unwrap();
                bytes.push(b'\n');
                std::fs::write(path, bytes).unwrap();
                let before = snapshot_tree(&temporary.journal_path());
                let day = DayKey::parse(token.day()).unwrap();
                let error = lease
                    .authorize(
                        operation,
                        selector,
                        token.as_hex(),
                        &day,
                        token.writer_family(),
                        token.target_scope(),
                    )
                    .unwrap_err();
                assert!(matches!(
                    error,
                    ConvergenceError::Unknown {
                        role: DurableRole::PreparedOwner
                    }
                ));
                assert_eq!(before, snapshot_tree(&temporary.journal_path()));
            },
        );
    }

    #[test]
    fn authorize_denies_when_all_active_barrier_is_absent() {
        with_live_token(
            "lease-no-barrier",
            |temporary, _, operation, selector, token, lease| {
                std::fs::remove_file(temporary.journal_path().join(format!(
                    "health/convergence/registry/grants/barriers/{}.active.json",
                    token.serial()
                )))
                .unwrap();
                let day = DayKey::parse(token.day()).unwrap();
                assert!(matches!(
                    lease
                        .authorize(
                            operation,
                            selector,
                            token.as_hex(),
                            &day,
                            token.writer_family(),
                            token.target_scope()
                        )
                        .unwrap(),
                    Authorization::Denied {
                        reason: DeniedReason::NotCommitted
                    }
                ));
            },
        );
    }

    #[test]
    fn authorize_obeys_lock_order_and_redacts_authority_debug() {
        with_live_token("lease-trace", |_, _, operation, selector, token, lease| {
            crate::access::initialize_lock_trace();
            let day = DayKey::parse(token.day()).unwrap();
            let authorization = lease
                .authorize(
                    operation,
                    selector,
                    token.as_hex(),
                    &day,
                    token.writer_family(),
                    token.target_scope(),
                )
                .unwrap();
            let Authorization::Granted(authority) = authorization else {
                panic!("granted");
            };
            assert!(!format!("{authority:?}").contains(token.as_hex()));
            assert_eq!(crate::access::lock_trace(), vec!["topology", "registry"]);
        });
    }

    #[test]
    fn authorize_denies_same_generation_completion() {
        let (temporary, admitted, operation, selector) = committed("lease-complete");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let token = &delivery.tokens()[0];
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store().journal_id(),
            admitted.store().root_id(),
            admitted.store().object_identity(),
            LOCK_TIMEOUT,
        )
        .unwrap();
        publish_kind_for_test(
            admitted.store(),
            &locks,
            &DayKey::parse(token.day()).unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        drop(locks);
        let lease = admitted.grant_lease().unwrap();
        assert!(matches!(
            lease
                .authorize(
                    &operation,
                    &selector,
                    token.as_hex(),
                    &DayKey::parse(token.day()).unwrap(),
                    token.writer_family(),
                    token.target_scope(),
                )
                .unwrap(),
            Authorization::Denied {
                reason: DeniedReason::SameGenerationCompletion
            }
        ));
        drop(temporary);
    }

    fn visible_terminal_prefix(
        name: &str,
    ) -> (TempDir, Admitted, OperationId, GrantRequestSelector, u64) {
        let (temporary, admitted) = admit_days(name, &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterTerminal);
        assert!(permit.commit().is_err());
        drop(guard);
        drop(held);
        (temporary, admitted, operation, selector, serial)
    }

    #[test]
    fn invalid_terminal_blocks_delivery_before_sealing() {
        let (temporary, admitted, operation, selector, serial) =
            visible_terminal_prefix("delivery-terminal");
        let path = temporary
            .journal_path()
            .join(format!("health/convergence/terminals/{serial}.json"));
        let mut terminal: crate::schema::Terminal =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        terminal.terminal_digest = "00".repeat(32);
        let mut bytes = crate::digest::canonical_json_bytes(&terminal).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::Terminal
            })
        ));
    }

    #[test]
    fn invalid_barrier_blocks_delivery_after_terminal_acceptance() {
        let (temporary, admitted, operation, selector, serial) =
            visible_terminal_prefix("delivery-barrier");
        let path = temporary.journal_path().join(format!(
            "health/convergence/registry/grants/barriers/{serial}.active.json"
        ));
        let mut barrier: crate::schema::GrantBarrier =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        barrier.barrier_digest = "00".repeat(32);
        let mut bytes = crate::digest::canonical_json_bytes(&barrier).unwrap();
        bytes.push(b'\n');
        std::fs::write(path, bytes).unwrap();
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::GrantActiveBarrier
            })
        ));
    }

    #[test]
    fn no_plaintext_token_is_stored_anywhere() {
        let (temporary, admitted, operation, selector) = committed("no-plaintext");
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        let sealed: Vec<String> = delivery
            .tokens()
            .iter()
            .map(|token| token.as_hex().to_owned())
            .collect();
        assert_eq!(sealed.len(), 2);
        // Walk every durable byte in the journal and prove no token appears.
        for (path, _) in snapshot_tree(&temporary.journal_path()) {
            let full = temporary.journal_path().join(&path);
            if !full.is_file() {
                continue;
            }
            let bytes = std::fs::read(&full).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for token in &sealed {
                assert!(!text.contains(token.as_str()), "token found in {path}");
            }
        }
    }

    #[test]
    fn reissue_reproduces_identical_bytes_across_processes() {
        let (temporary, admitted, operation, selector) = committed("reissue");
        let first: Vec<String> = admitted
            .deliver_grants(&operation, &selector)
            .unwrap()
            .tokens()
            .iter()
            .map(|token| token.as_hex().to_owned())
            .collect();
        drop(admitted);
        // Fresh process-equivalent: the outbox derives the same bytes from the
        // same durable evidence, with no caller durability assertion.
        let resumed = reopen(&temporary, &["20260823"]);
        let again: Vec<String> = resumed
            .deliver_grants(&operation, &selector)
            .unwrap()
            .tokens()
            .iter()
            .map(|token| token.as_hex().to_owned())
            .collect();
        assert_eq!(first, again);
        assert_eq!(first.len(), 2);
    }

    #[test]
    fn barrier_without_terminal_delivers_nothing_and_writes_nothing() {
        let (temporary, admitted) = admit_days("outbox", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterAllActiveBarrier);
        permit.commit().unwrap_err();
        drop(guard);
        // The outbox is fully prepared: decision, every member, and the
        // all-active barrier are durable.
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/registry/grants/barriers/1.active.json")
                .is_file()
        );
        drop(held);

        // Delivery acquires the day set, so it runs after the lease drops. The
        // exact committed terminal is not durable, so nothing is handed out.
        let before = snapshot_tree(&temporary.journal_path());
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert_eq!(
            delivery.denied_reason(),
            Some(DeniedReason::NotCommitted),
            "{delivery:?}"
        );
        assert!(delivery.tokens().is_empty());
        // Delivery never writes, on any path.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn abort_decided_transition_never_delivers() {
        let (_temporary, admitted) = admit_days("abort-deliver", &["20260823"]);
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::try_new(admitted.days(), requests()).unwrap();
        let owner = OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let permit = held.proceed().unwrap();
        permit.abort().unwrap();
        drop(held);
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(matches!(
            delivery,
            Delivery::Denied {
                reason: DeniedReason::AbortDecided | DeniedReason::NotCommitted
            }
        ));
        assert!(delivery.tokens().is_empty());
    }

    #[test]
    fn tuple_knowledge_alone_mints_nothing() {
        let (temporary, admitted, operation, selector) = committed("tuple-only");
        // A caller who knows the tuples but not the operation gets nothing: the
        // only public route is `deliver_grants`, and it is keyed by operation.
        let foreign = OperationId::generate().unwrap();
        // No link for that operation, so there is no transition to deliver for.
        assert!(
            matches!(
                admitted.deliver_grants(&foreign, &selector),
                Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner
                })
            ),
            "tuple knowledge unexpectedly reached delivery"
        );
        // And asking with the right operation but a different selector cannot
        // even reach the outbox.
        let other = GrantRequestSelector::empty(admitted.days()).unwrap();
        let error = admitted.deliver_grants(&operation, &other).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ConflictingSelector)
        ));
        drop(temporary);
    }

    #[test]
    fn missing_owner_record_is_unknown_not_a_token() {
        let (temporary, admitted, operation, selector) = committed("missing-owner");
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/registry/owners")
                .join(format!("{}.json", operation.as_hex())),
        )
        .unwrap();
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::PreparedOwner
            })
        ));
    }

    #[test]
    fn missing_link_is_unknown_not_a_token() {
        let (temporary, admitted, operation, selector) = committed("missing-link");
        let owner = crate::owner::OwnerBinding::prepare(
            &admitted,
            &operation,
            crate::selector::TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        std::fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/registry/links")
                .join(crate::layout::link_name(
                    owner.digest_hex(),
                    owner.selector_digest(),
                )),
        )
        .unwrap();
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::OwnerIntentLink
            })
        ));
    }

    #[test]
    fn same_generation_completion_denies_bytes_even_while_active() {
        let (temporary, admitted, operation, selector) = committed("completion-deny");
        // The registry member is still exactly active; the canonical record is
        // what denies delivery.
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store().journal_id(),
            admitted.store().root_id(),
            admitted.store().object_identity(),
            LOCK_TIMEOUT,
        )
        .unwrap();
        publish_kind_for_test(
            admitted.store(),
            &locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        drop(locks);
        let before = snapshot_tree(&temporary.journal_path());
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert_eq!(
            delivery.denied_reason(),
            Some(DeniedReason::SameGenerationCompletion),
            "{delivery:?}"
        );
        assert!(delivery.tokens().is_empty());
        // Denial performs no cleanup of its own.
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn later_dirty_descendant_denies_bytes_even_while_active() {
        let (_temporary, admitted, operation, selector) = committed("later-dirty-deny");
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let locks = acquire_days_with_timeout(
            &dirs,
            admitted.days(),
            admitted.store().journal_id(),
            admitted.store().root_id(),
            admitted.store().object_identity(),
            LOCK_TIMEOUT,
        )
        .unwrap();
        publish_kind_for_test(
            admitted.store(),
            &locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        drop(locks);
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert_eq!(
            delivery.denied_reason(),
            Some(DeniedReason::LaterDirtyDescendant),
            "{delivery:?}"
        );
        assert!(delivery.tokens().is_empty());
    }

    #[test]
    fn tampered_member_digest_is_unknown() {
        let (temporary, admitted, operation, selector) = committed("tamper-member");
        let members = temporary
            .journal_path()
            .join("health/convergence/registry/grants/members/1");
        let entry = std::fs::read_dir(&members)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let bytes = std::fs::read(&entry).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap();
        value.as_object_mut().unwrap().insert(
            "member_digest".to_owned(),
            serde_json::Value::String("00".repeat(32)),
        );
        let mut out = serde_json::to_vec(&value).unwrap();
        out.push(b'\n');
        std::fs::write(&entry, out).unwrap();
        assert!(matches!(
            admitted.deliver_grants(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::GrantActiveBarrier
            })
        ));
    }

    #[test]
    fn delivery_holds_no_registry_guard_when_it_returns() {
        let (_temporary, admitted, operation, selector) = committed("no-guard");
        admitted.deliver_grants(&operation, &selector).unwrap();
        // If delivery had leaked its guard, this would time out.
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let section =
            crate::access::hold_registry_for_test(&dirs, std::time::Duration::from_millis(80))
                .unwrap();
        drop(section);
    }

    #[test]
    fn delivery_takes_the_registry_only_after_releasing_the_global() {
        let (temporary, admitted, operation, selector) = committed("global-order");
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        // Hold the registry from outside. Delivery must already have released
        // the global by the time it blocks here, so a concurrent global holder
        // is never deadlocked against it.
        let held_registry = crate::access::hold_registry_for_test(&dirs, LOCK_TIMEOUT).unwrap();
        let narrowed = reopen(&temporary, &["20260823"])
            .with_lock_timeout(std::time::Duration::from_millis(80));
        let error = narrowed.deliver_grants(&operation, &selector).unwrap_err();
        assert!(matches!(error, ConvergenceError::Refused(Refusal::Busy)));
        // The global is demonstrably free while delivery is blocked on the
        // registry: acquiring it here would hang otherwise.
        let topology =
            hold_topology_with_timeout(&dirs, std::time::Duration::from_millis(80)).unwrap();
        drop(topology);
        drop(held_registry);
        let delivery = narrowed.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.is_ready(), "{delivery:?}");
    }
}
