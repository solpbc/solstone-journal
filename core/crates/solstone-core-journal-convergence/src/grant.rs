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

use std::collections::BTreeMap;

use crate::claim::{ClaimView, mechanical_finalize};
use crate::decision::{accept_decision, load_barrier, load_decision, load_member};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::{StoreDirs, open_store_dirs};
use crate::layout::{ACTIVE_BARRIER_SUFFIX, DayKey};
use crate::link::load_owner_intent_link;
use crate::lock::{
    DayLockSet, LOCK_TIMEOUT, acquire_days_with_timeout, hold_topology_with_timeout,
};
use crate::mac::hmac_hex;
use crate::owner::load_owner_binding;
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
/// `Pending` and `Unknown` are variants here rather than errors, so a state can
/// never classify as both: exactly one variant is produced per attempt.
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
    /// The evidence cannot be interpreted. No bytes, no write, no cleanup.
    Unknown { role: DurableRole },
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
        let dirs = open_store_dirs(store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let locks = acquire_days_with_timeout(
            &dirs,
            self.days(),
            store.journal_id(),
            store.root_id(),
            store.object_identity(),
            self.lock_timeout(),
        )?;

        // Brief global, released before the registry is touched. A unique
        // unheaded introduction is live for fencing and is mechanically headed
        // rather than ignored; a disjoint claim is harmless.
        let claim = {
            let _topology = hold_topology_with_timeout(&dirs, LOCK_TIMEOUT)?;
            match mechanical_finalize(store, &dirs)? {
                ClaimView::Empty => BTreeMap::new(),
                ClaimView::Headed(body) | ClaimView::Unheaded(body) => body.table,
            }
        };

        // The operation's own link names the transition, which is the only
        // anchor that survives claim release.
        let (serial, link) = {
            let section = crate::registry::enter_registry(&dirs)?;
            match crate::link::latest_link(&section, operation.as_hex())? {
                Some(link) => (link.serial, link),
                None => {
                    return Ok(Delivery::Unknown {
                        role: DurableRole::OwnerIntentLink,
                    });
                }
            }
        };

        // Fencing: any claim on an admitted day belonging to a different
        // transition is an overlapping successor.
        for day in locks.days() {
            if let Some(entry) = claim.get(day.as_str())
                && entry.serial != serial
            {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::OverlappingSuccessor,
                });
            }
        }
        let claim_released = locks
            .days()
            .iter()
            .all(|day| !claim.contains_key(day.as_str()));

        // The intent is unlinked during cleanup, so post-eviction the link is
        // the surviving anchor. While the intent is still present it must agree
        // exactly.
        if let Some(intent) = crate::intent::read_intent(&dirs, serial)?
            && intent.intent_digest != link.intent_digest
        {
            return Ok(Delivery::Unknown {
                role: DurableRole::OwnerIntentLink,
            });
        }

        // Nothing may be handed out until the transition is committed, proven
        // either by the exact visible terminal during the cleanup window or, once
        // the terminal is evicted and the claim released, by the base committed
        // successor-clearance vector.
        match establish_committed(&dirs, serial, &link, claim_released, locks.days())? {
            Committed::Yes => {}
            Committed::No { reason } => return Ok(Delivery::Denied { reason }),
            Committed::Unknown { role } => return Ok(Delivery::Unknown { role }),
        }

        // Brief registry: re-read owner, link, decision, members, barrier and
        // mint the already-classified authority. No day scan happens here.
        let prepared = {
            let section = crate::registry::enter_registry(&dirs)?;
            let Some(secret) = load_journal_secret(section.registry())? else {
                return Ok(Delivery::Unknown {
                    role: DurableRole::JournalSecret,
                });
            };
            let Some((owner, state)) = load_owner_binding(
                &section,
                operation,
                selector,
                store.object_identity(),
                store.journal_id(),
                store.root_id(),
                &secret.key_hex,
            )?
            else {
                return Ok(Delivery::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            if state != PreparedOwnerState::Active {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::OwnerRevoked,
                });
            }
            // Authoritative re-read of the link under the registry lock.
            let Some(exact) = load_owner_intent_link(&section, operation.as_hex(), serial)? else {
                return Ok(Delivery::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            };
            if exact != link
                || exact.owner_binding_digest != owner.digest_hex()
                || exact.selector_digest != owner.selector_digest()
            {
                return Ok(Delivery::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            }
            let Some(decision) = load_decision(&section, serial)? else {
                return Ok(Delivery::Unknown {
                    role: DurableRole::Decision,
                });
            };
            if decision.kind == DecisionKind::AbortNoOpen {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::AbortDecided,
                });
            }
            let decision = accept_decision(
                decision,
                &owner,
                serial,
                &link.intent_digest,
                DecisionKind::Commit,
            )?;
            let Some(barrier) = load_barrier(&section, serial, ACTIVE_BARRIER_SUFFIX)? else {
                // Members may be complete, but without the barrier the outbox
                // is not prepared and no subset may validate early.
                return Ok(Delivery::Denied {
                    reason: DeniedReason::NotCommitted,
                });
            };
            let mut members = Vec::new();
            for tuple in &decision.tuples {
                let Some(member) = load_member(&section, serial, tuple)? else {
                    return Ok(Delivery::Unknown {
                        role: DurableRole::GrantMember,
                    });
                };
                if barrier
                    .member_digests
                    .get(&crate::decision::member_key(tuple))
                    != Some(&member.member_digest)
                {
                    return Ok(Delivery::Unknown {
                        role: DurableRole::GrantActiveBarrier,
                    });
                }
                members.push(member);
            }
            Prepared {
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
            }
        };

        // Registry released. The canonical revalidation immediately before
        // returning bytes runs under the day leases only.
        revalidate_then_seal(store, &locks, &prepared)
    }
}

struct Prepared {
    authority: ParentAuthority,
    members: Vec<GrantMember>,
    barrier_digest: String,
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
        match member.state {
            MemberState::Active => {}
            MemberState::RevocationPending | MemberState::Revoked => {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::MemberRevoked,
                });
            }
            MemberState::Superseded => {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::MemberSuperseded,
                });
            }
        }
        let day = DayKey::parse(&member.tuple.day)?;
        match store.load_day(locks, &day)? {
            LoadDay::Published(snapshot) => {
                if snapshot.dirty_generation != member.tuple.dirty_generation
                    || snapshot.dirty_by_transition_serial
                        != member.tuple.dirty_by_transition_serial
                {
                    // A later dirty descendant advanced the tuple even though
                    // registry membership is still active.
                    return Ok(Delivery::Denied {
                        reason: DeniedReason::LaterDirtyDescendant,
                    });
                }
                if snapshot.completed_generation >= member.tuple.dirty_generation {
                    return Ok(Delivery::Denied {
                        reason: DeniedReason::SameGenerationCompletion,
                    });
                }
            }
            LoadDay::PublicationPending { kind } => {
                // A unique contiguous publication owns this state. No bytes, no
                // cleanup: only its own publication recovery may act.
                return Ok(Delivery::Pending {
                    stage: match kind {
                        PendingKind::WitnessAheadOfHead => PendingStage::WitnessAheadOfHead,
                        PendingKind::HeadAheadOfRecord => PendingStage::HeadAheadOfRecord,
                    },
                    recovery: "day publication recovery",
                });
            }
            LoadDay::HeadedDescendant { .. } => {
                return Ok(Delivery::Denied {
                    reason: DeniedReason::LaterDirtyDescendant,
                });
            }
            LoadDay::Genesis => {
                return Ok(Delivery::Unknown {
                    role: DurableRole::Record,
                });
            }
        }
        tokens.push(prepared.authority.seal(member, &prepared.barrier_digest)?);
    }
    Ok(Delivery::Ready(tokens))
}

/// Whether the transition is committed, and by which evidence.
enum Committed {
    Yes,
    No { reason: DeniedReason },
    Unknown { role: DurableRole },
}

/// The exact visible terminal during the cleanup window, or the post-eviction
/// nonempty-committed clearance vector once the claim is released.
fn establish_committed(
    dirs: &StoreDirs,
    serial: u64,
    link: &crate::schema::OwnerIntentLink,
    claim_released: bool,
    days: &std::collections::BTreeSet<DayKey>,
) -> Result<Committed, ConvergenceError> {
    if let Some(terminal) = crate::terminal::read_terminal(dirs, serial)? {
        if terminal.intent_digest != link.intent_digest {
            return Ok(Committed::Unknown {
                role: DurableRole::Terminal,
            });
        }
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
    for day in days {
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
        let selector =
            GrantRequestSelector::try_new(admitted.days(), requests().into_iter()).unwrap();
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
        let selector =
            GrantRequestSelector::try_new(admitted.days(), requests().into_iter()).unwrap();
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
        let selector =
            GrantRequestSelector::try_new(admitted.days(), requests().into_iter()).unwrap();
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
        let delivery = admitted.deliver_grants(&foreign, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        // No link for that operation, so there is no transition to deliver for.
        assert!(
            matches!(
                delivery,
                Delivery::Unknown {
                    role: DurableRole::OwnerIntentLink
                }
            ),
            "{delivery:?}"
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
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(matches!(
            delivery,
            Delivery::Unknown {
                role: DurableRole::PreparedOwner
            }
        ));
    }

    #[test]
    fn missing_link_is_unknown_not_a_token() {
        let (temporary, admitted, operation, selector) = committed("missing-link");
        std::fs::remove_dir_all(
            temporary
                .journal_path()
                .join("health/convergence/registry/links")
                .join(operation.as_hex()),
        )
        .unwrap();
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(matches!(
            delivery,
            Delivery::Unknown {
                role: DurableRole::OwnerIntentLink
            }
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
        let delivery = admitted.deliver_grants(&operation, &selector).unwrap();
        assert!(delivery.tokens().is_empty());
        assert!(matches!(delivery, Delivery::Unknown { .. }), "{delivery:?}");
    }

    #[test]
    fn delivery_holds_no_registry_guard_when_it_returns() {
        let (_temporary, admitted, operation, selector) = committed("no-guard");
        admitted.deliver_grants(&operation, &selector).unwrap();
        // If delivery had leaked its guard, this would time out.
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let section = crate::registry::enter_registry_with_timeout(
            &dirs,
            std::time::Duration::from_millis(80),
        )
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
        let held_registry =
            crate::registry::enter_registry_with_timeout(&dirs, LOCK_TIMEOUT).unwrap();
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
