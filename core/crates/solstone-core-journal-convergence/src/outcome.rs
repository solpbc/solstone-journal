// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only reporting of a terminal outcome after terminal eviction.
//!
//! This module deliberately owns no authority.  It replays only the exact
//! historical evidence already written by the base and resolver paths and
//! returns `Unknown` as soon as a named member of that evidence is absent or
//! inconsistent.

use crate::access::ResolverAccess;
use crate::claim::ClaimView;
use crate::decision::{accept_barrier, load_barrier, load_decision, load_member};
use crate::digest::{digest_value, digest_value_excluding};
use crate::error::{ConvergenceError, DurableRole};
use crate::layout::{ACTIVE_BARRIER_SUFFIX, GRANTS, TOMBSTONES, grant_tombstone_name};
use crate::link::{LinkResolution, resolve_owner_intent_link};
use crate::owner::load_owner_binding;
use crate::permit::{TerminalOutcome, parse_outcome};
use crate::preflight::Admitted;
use crate::schema::{
    DecisionKind, GrantDecision, GrantMember, GrantTombstone, ROLE_CLEARANCE_BARRIER,
    ROLE_CLEARANCE_MEMBER, ROLE_GRANT_TOMBSTONE, SCHEMA_VERSION, read_json,
};
use crate::secret::load_journal_secret;
use crate::selector::{GrantRequestSelector, OperationId};
use crate::walk::open_dir;

/// The already-fixed terminal result that exact historical evidence reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantOutcome {
    NonemptyCommitted,
    Aborted,
    EmptySetCommitted,
    PassiveSuperseded,
    DecisionedSuperseded,
}

/// Read-only grant result.  `Pending` names the sole cleanup owner for a
/// valid active prefix; malformed or released-history evidence is returned as
/// `ConvergenceError::Unknown` rather than represented here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GrantState {
    Outcome(GrantOutcome),
    Pending { recovery: &'static str },
}

impl Admitted {
    /// Report an evicted operation's outcome from exact retained evidence.
    /// This cannot mint a token, owner, permit, or a new terminal.
    pub fn grant_state(
        &self,
        operation: &OperationId,
        selector: &GrantRequestSelector,
    ) -> Result<GrantState, ConvergenceError> {
        if selector.days() != self.days() {
            return Err(ConvergenceError::Refused(
                crate::error::Refusal::DaySetChanged,
            ));
        }
        let access = ResolverAccess::acquire(self)?;
        let claim = access.read_claim()?;
        let linked = access.with_registry(|section| {
            let secret =
                load_journal_secret(section.registry())?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::JournalSecret,
                })?;
            let Some((owner, _state)) = load_owner_binding(
                section,
                operation,
                selector,
                self.store().object_identity(),
                self.store().journal_id(),
                self.store().root_id(),
                &secret.key_hex,
            )?
            else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::PreparedOwner,
                });
            };
            let LinkResolution::Exact(link) = resolve_owner_intent_link(section, &owner)? else {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::OwnerIntentLink,
                });
            };
            let decision = load_decision(section, link.serial)?;
            let active = load_barrier(section, link.serial, ACTIVE_BARRIER_SUFFIX)?;
            let superseded = load_barrier(
                section,
                link.serial,
                crate::layout::SUPERSEDED_BARRIER_SUFFIX,
            )?;
            Ok((owner, *link, decision, active, superseded))
        })?;
        let (owner, link, decision, active, superseded) = linked;
        if claim_still_names(&claim, self.days(), link.serial) {
            return Ok(GrantState::Pending {
                recovery: "terminal and clearance cleanup",
            });
        }
        let base = clearance_outcome(access.dirs(), link.serial, self.days(), &owner)?;
        match (base, decision) {
            (TerminalOutcome::Committed, None) => {
                require_absent(active, DurableRole::GrantActiveBarrier)?;
                require_absent(superseded, DurableRole::GrantSupersededBarrier)?;
                require_no_members(&access, link.serial)?;
                Ok(GrantState::Outcome(GrantOutcome::EmptySetCommitted))
            }
            (TerminalOutcome::Superseded, None) => {
                require_absent(active, DurableRole::GrantActiveBarrier)?;
                require_absent(superseded, DurableRole::GrantSupersededBarrier)?;
                require_no_members(&access, link.serial)?;
                Ok(GrantState::Outcome(GrantOutcome::PassiveSuperseded))
            }
            (TerminalOutcome::Aborted, Some(decision)) => {
                require_abort_decision(&decision, &owner, &link)?;
                require_absent(active, DurableRole::GrantActiveBarrier)?;
                require_absent(superseded, DurableRole::GrantSupersededBarrier)?;
                require_no_members(&access, link.serial)?;
                Ok(GrantState::Outcome(GrantOutcome::Aborted))
            }
            (TerminalOutcome::Committed, Some(decision)) => {
                let decision = require_commit_decision(decision, &owner, &link)?;
                let active = active.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::GrantActiveBarrier,
                })?;
                require_absent(superseded, DurableRole::GrantSupersededBarrier)?;
                let members = historical_members(&access, &owner, &decision, false)?;
                accept_barrier(active, &owner, &decision, &members, ACTIVE_BARRIER_SUFFIX)?;
                Ok(GrantState::Outcome(GrantOutcome::NonemptyCommitted))
            }
            (TerminalOutcome::Superseded, Some(decision)) => {
                let decision = require_commit_decision(decision, &owner, &link)?;
                let barrier = superseded.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::GrantSupersededBarrier,
                })?;
                let members = historical_members(&access, &owner, &decision, true)?;
                accept_barrier(
                    barrier.clone(),
                    &owner,
                    &decision,
                    &members,
                    crate::layout::SUPERSEDED_BARRIER_SUFFIX,
                )?;
                match (barrier.prior_all_active_digest.as_deref(), active) {
                    (Some(_), Some(prior_active)) => {
                        accept_retained_active_barrier(&prior_active, &owner, &decision, &barrier)?;
                    }
                    (None, None) => {}
                    _ => {
                        return Err(ConvergenceError::Unknown {
                            role: DurableRole::GrantActiveBarrier,
                        });
                    }
                }
                Ok(GrantState::Outcome(GrantOutcome::DecisionedSuperseded))
            }
            _ => Err(ConvergenceError::Unknown {
                role: DurableRole::ClearanceBarrier,
            }),
        }
    }
}

fn claim_still_names(view: &ClaimView, days: &[crate::layout::DayKey], serial: u64) -> bool {
    match view {
        ClaimView::Empty => false,
        ClaimView::Headed(body) | ClaimView::Unheaded(body) => days.iter().any(|day| {
            body.table
                .get(day.as_str())
                .is_some_and(|entry| entry.serial == serial)
        }),
    }
}

fn clearance_outcome(
    dirs: &crate::init::StoreDirs,
    serial: u64,
    days: &[crate::layout::DayKey],
    owner: &crate::owner::OwnerBinding,
) -> Result<TerminalOutcome, ConvergenceError> {
    let barrier = crate::terminal::read_clearance_barrier(dirs, serial)?.ok_or(
        ConvergenceError::Unknown {
            role: DurableRole::ClearanceBarrier,
        },
    )?;
    if barrier.role != ROLE_CLEARANCE_BARRIER
        || barrier.schema_version != SCHEMA_VERSION
        || barrier.journal_id != owner.journal_id()
        || barrier.root_id != owner.root_id()
        || barrier.serial != serial
        || barrier.day_set
            != days
                .iter()
                .map(|day| day.as_str().to_owned())
                .collect::<Vec<_>>()
        || barrier.member_digests.len() != days.len()
        || barrier.resolved.len() != days.len()
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClearanceBarrier,
        });
    }
    let mut outcome = None;
    for day in days {
        let member = crate::terminal::read_clearance_member(dirs, day)?.ok_or(
            ConvergenceError::Unknown {
                role: DurableRole::ClearanceMember,
            },
        )?;
        let digest = digest_value(&member)?.as_hex().to_owned();
        if member.role != ROLE_CLEARANCE_MEMBER
            || member.schema_version != SCHEMA_VERSION
            || member.journal_id != barrier.journal_id
            || member.root_id != barrier.root_id
            || member.day != day.as_str()
            || member.serial != serial
            || barrier.member_digests.get(day.as_str()) != Some(&digest)
            || member.terminal_digest != barrier.terminal_digest
            || barrier.resolved.get(day.as_str()) != Some(&member.resolved)
        {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::ClearanceMember,
            });
        }
        let parsed = parse_outcome(&member.outcome).ok_or(ConvergenceError::Unknown {
            role: DurableRole::ClearanceMember,
        })?;
        if outcome.replace(parsed).is_some_and(|prior| prior != parsed) {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::ClearanceMember,
            });
        }
    }
    outcome.ok_or(ConvergenceError::Unknown {
        role: DurableRole::ClearanceMember,
    })
}

fn require_absent<T>(value: Option<T>, role: DurableRole) -> Result<(), ConvergenceError> {
    if value.is_some() {
        return Err(ConvergenceError::Unknown { role });
    }
    Ok(())
}

fn require_abort_decision(
    decision: &GrantDecision,
    owner: &crate::owner::OwnerBinding,
    link: &crate::schema::OwnerIntentLink,
) -> Result<(), ConvergenceError> {
    let accepted = crate::decision::accept_decision(
        decision.clone(),
        owner,
        link.serial,
        &link.intent_digest,
        DecisionKind::AbortNoOpen,
    )?;
    if !accepted.tuples.is_empty() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Decision,
        });
    }
    Ok(())
}

fn require_commit_decision(
    decision: GrantDecision,
    owner: &crate::owner::OwnerBinding,
    link: &crate::schema::OwnerIntentLink,
) -> Result<GrantDecision, ConvergenceError> {
    crate::decision::accept_decision(
        decision,
        owner,
        link.serial,
        &link.intent_digest,
        DecisionKind::Commit,
    )
}

/// The superseded barrier authenticates the exact active barrier it retains as
/// history.  Its former active member digests are intentionally not compared
/// to the current superseded member files: changing the member state changes
/// those digests.  The retained barrier is instead checked against the fixed
/// decision and its tuple universe, then bound by its full-record digest from
/// the superseded barrier.
fn accept_retained_active_barrier(
    barrier: &crate::schema::GrantBarrier,
    owner: &crate::owner::OwnerBinding,
    decision: &GrantDecision,
    superseded: &crate::schema::GrantBarrier,
) -> Result<(), ConvergenceError> {
    let expected_keys = decision
        .tuples
        .iter()
        .map(crate::decision::member_key)
        .collect::<Vec<_>>();
    let digest = crate::decision::barrier_digest(barrier)?;
    if barrier.role != crate::schema::ROLE_GRANT_ALL_ACTIVE
        || barrier.schema_version != SCHEMA_VERSION
        || barrier.journal_id != owner.journal_id()
        || barrier.root_id != owner.root_id()
        || barrier.serial != decision.serial
        || barrier.operation_id != owner.operation_id()
        || barrier.owner_binding_digest != owner.digest_hex()
        || barrier.selector_digest != owner.selector_digest()
        || barrier.decision_digest != decision.decision_digest
        || barrier.intent_digest != decision.intent_digest
        || barrier.day_set != decision.day_set
        || barrier.descendant_discriminator.is_some()
        || barrier.prior_all_active_digest.is_some()
        || barrier.member_digests.keys().cloned().collect::<Vec<_>>() != expected_keys
        || superseded.prior_all_active_digest.as_deref() != Some(digest.as_str())
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantActiveBarrier,
        });
    }
    let mut probe = barrier.clone();
    probe.barrier_digest.clear();
    if digest_value_excluding(&probe, "barrier_digest")?.as_hex() != barrier.barrier_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantActiveBarrier,
        });
    }
    Ok(())
}

fn require_no_members(access: &ResolverAccess<'_>, serial: u64) -> Result<(), ConvergenceError> {
    access.with_registry(|section| {
        let Some(grants) = open_dir(section.registry(), GRANTS)? else {
            return Ok(());
        };
        let Some(members) = open_dir(&grants, crate::layout::MEMBERS)? else {
            return Ok(());
        };
        if open_dir(&members, &crate::layout::serial_dir(serial))?.is_some() {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::GrantMember,
            });
        }
        Ok(())
    })
}

fn historical_members(
    access: &ResolverAccess<'_>,
    owner: &crate::owner::OwnerBinding,
    decision: &GrantDecision,
    superseded: bool,
) -> Result<Vec<GrantMember>, ConvergenceError> {
    access.with_registry(|section| {
        let mut members = Vec::new();
        for tuple in &decision.tuples {
            if let Some(member) = load_member(section, decision.serial, tuple)? {
                if member.role != crate::schema::ROLE_GRANT_MEMBER
                    || member.schema_version != SCHEMA_VERSION
                    || member.journal_id != owner.journal_id()
                    || member.root_id != owner.root_id()
                    || member.serial != decision.serial
                    || member.operation_id != owner.operation_id()
                    || member.owner_binding_digest != owner.digest_hex()
                    || member.selector_digest != owner.selector_digest()
                    || member.tuple != *tuple
                    || (superseded && member.state != crate::schema::MemberState::Superseded)
                {
                    return Err(ConvergenceError::Unknown {
                        role: DurableRole::GrantMember,
                    });
                }
                members.push(member);
                continue;
            }
            let tombstone = read_tombstone(section, decision.serial, tuple)?;
            if tombstone.journal_id != owner.journal_id()
                || tombstone.root_id != owner.root_id()
                || tombstone.serial != decision.serial
                || tombstone.tuple != *tuple
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::GrantTombstone,
                });
            }
            // Tombstones preserve the member digest but do not recreate a
            // member record.  A barrier that still needs it is historical;
            // outcome reporting accepts the tombstone as the exact fold.
            members.push(GrantMember {
                role: crate::schema::ROLE_GRANT_MEMBER.to_owned(),
                schema_version: SCHEMA_VERSION,
                journal_id: tombstone.journal_id.clone(),
                root_id: tombstone.root_id.clone(),
                serial: tombstone.serial,
                operation_id: owner.operation_id().to_owned(),
                owner_binding_digest: owner.digest_hex().to_owned(),
                selector_digest: owner.selector_digest().to_owned(),
                tuple: tombstone.tuple,
                state: if superseded {
                    crate::schema::MemberState::Superseded
                } else {
                    crate::schema::MemberState::Revoked
                },
                member_digest: tombstone.member_digest,
            });
        }
        Ok(members)
    })
}

fn read_tombstone(
    section: &crate::access::RegistrySection<'_>,
    serial: u64,
    tuple: &crate::schema::GrantTuple,
) -> Result<GrantTombstone, ConvergenceError> {
    let grants = open_dir(section.registry(), GRANTS)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantTombstone,
    })?;
    let tombstones = open_dir(&grants, TOMBSTONES)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantTombstone,
    })?;
    let tombstone = read_json::<GrantTombstone>(
        &tombstones,
        &grant_tombstone_name(serial, tuple),
        DurableRole::GrantTombstone,
    )?
    .ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantTombstone,
    })?;
    if tombstone.role != ROLE_GRANT_TOMBSTONE || tombstone.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantTombstone,
        });
    }
    Ok(tombstone)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::lock::{LOCK_TIMEOUT, acquire_days_with_timeout};
    use crate::owner::OwnerBinding;
    use crate::preflight::{Preflight, preflight};
    use crate::publish::{
        PreparedCompletionAuthority, PreparedLaterDirtyAuthority, publish_kind_for_test,
    };
    use crate::schema::{ClearanceBarrier, ClearanceMember};
    use crate::selector::{TargetScope, TransactionClass, WriterFamily};
    use crate::test_support::{PublishFault, TempDir, admit_days, admit_proof, fail_after};
    use solstone_core_journal_io::JournalRoot;

    fn requests() -> Vec<(&'static str, WriterFamily, TargetScope)> {
        vec![("20260823", WriterFamily::Think, TargetScope::Chronicle)]
    }

    fn prepared(
        admitted: &Admitted,
        empty: bool,
    ) -> (OperationId, GrantRequestSelector, OwnerBinding) {
        let operation = OperationId::generate().unwrap();
        let selector = if empty {
            GrantRequestSelector::empty(admitted.days()).unwrap()
        } else {
            GrantRequestSelector::try_new(admitted.days(), requests()).unwrap()
        };
        let owner = OwnerBinding::prepare(
            admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        (operation, selector, owner)
    }

    fn committed(name: &str) -> (TempDir, Admitted, OperationId, GrantRequestSelector) {
        let (temporary, admitted) = admit_days(name, &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted, false);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap().commit().unwrap();
        drop(held);
        (temporary, admitted, operation, selector)
    }

    #[derive(Clone, Copy, Debug)]
    enum Matrix {
        NonemptyCommitted,
        Aborted,
        EmptySetCommitted,
        PassiveSuperseded,
        DecisionedSuperseded,
    }

    impl Matrix {
        fn outcome(self) -> GrantOutcome {
            match self {
                Self::NonemptyCommitted => GrantOutcome::NonemptyCommitted,
                Self::Aborted => GrantOutcome::Aborted,
                Self::EmptySetCommitted => GrantOutcome::EmptySetCommitted,
                Self::PassiveSuperseded => GrantOutcome::PassiveSuperseded,
                Self::DecisionedSuperseded => GrantOutcome::DecisionedSuperseded,
            }
        }

        fn name(self) -> &'static str {
            match self {
                Self::NonemptyCommitted => "nonempty committed",
                Self::Aborted => "aborted",
                Self::EmptySetCommitted => "empty-set committed",
                Self::PassiveSuperseded => "passive superseded",
                Self::DecisionedSuperseded => "decisioned superseded",
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum SuccessorStage {
        BodyBeforeHead,
        HeadedBeforeIntent,
        IntentBeforeConsumption,
        ConsumptionWitness,
    }

    impl SuccessorStage {
        fn fault(self) -> PublishFault {
            match self {
                Self::BodyBeforeHead => PublishFault::AfterClaimRevision,
                Self::HeadedBeforeIntent => PublishFault::AfterClaimHead,
                Self::IntentBeforeConsumption => PublishFault::AfterIntent,
                Self::ConsumptionWitness => PublishFault::AfterConsumeWitness,
            }
        }

        fn name(self) -> &'static str {
            match self {
                Self::BodyBeforeHead => "body-before-head",
                Self::HeadedBeforeIntent => "headed-pre-intent",
                Self::IntentBeforeConsumption => "intent-before-consumption",
                Self::ConsumptionWitness => "consumption-witness",
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum MemberCondition {
        ExactPresent,
        BarrierAbsent,
        MemberUnlinked,
        MemberReplaced,
        BarrierDigestMismatched,
    }

    impl MemberCondition {
        fn name(self) -> &'static str {
            match self {
                Self::ExactPresent => "exact-present",
                Self::BarrierAbsent => "barrier-absent",
                Self::MemberUnlinked => "clearance-member-unlinked",
                Self::MemberReplaced => "clearance-member-replaced",
                Self::BarrierDigestMismatched => "barrier-member-digest-mismatched",
            }
        }

        fn expected_role(self) -> Option<DurableRole> {
            match self {
                Self::ExactPresent => None,
                Self::BarrierAbsent => Some(DurableRole::ClearanceBarrier),
                Self::MemberUnlinked | Self::MemberReplaced | Self::BarrierDigestMismatched => {
                    Some(DurableRole::ClearanceMember)
                }
            }
        }
    }

    fn matrix_history(
        matrix: Matrix,
    ) -> (TempDir, Admitted, OperationId, GrantRequestSelector, u64) {
        match matrix {
            Matrix::NonemptyCommitted => {
                let (temporary, admitted, operation, selector) = committed("matrix-nonempty");
                let serial = match admitted.deliver_grants(&operation, &selector).unwrap() {
                    crate::grant::Delivery::Ready(tokens) => tokens[0].serial(),
                    other => panic!("unexpected delivery: {other:?}"),
                };
                (temporary, admitted, operation, selector, serial)
            }
            Matrix::Aborted => {
                let (temporary, admitted) = admit_days("matrix-abort", &["20260823"]);
                let (operation, selector, owner) = prepared(&admitted, false);
                let mut held = admitted.begin(owner).unwrap();
                let proof = admit_proof(&held, held.owner()).unwrap();
                let permit = held.continue_with(proof).unwrap();
                let serial = permit.held.serial.unwrap();
                permit.abort().unwrap();
                drop(held);
                (temporary, admitted, operation, selector, serial)
            }
            Matrix::EmptySetCommitted => {
                let (temporary, admitted) = admit_days("matrix-empty", &["20260823"]);
                let (operation, selector, owner) = prepared(&admitted, true);
                let mut held = admitted.begin(owner).unwrap();
                let proof = admit_proof(&held, held.owner()).unwrap();
                let permit = held.continue_with(proof).unwrap();
                let serial = permit.held.serial.unwrap();
                permit.commit().unwrap();
                drop(held);
                (temporary, admitted, operation, selector, serial)
            }
            Matrix::PassiveSuperseded => {
                let (temporary, admitted) = admit_days("matrix-passive", &["20260823"]);
                let (operation, selector, owner) = prepared(&admitted, true);
                let mut held = admitted.begin(owner).unwrap();
                let proof = admit_proof(&held, held.owner()).unwrap();
                let permit = held.continue_with(proof).unwrap();
                let serial = permit.held.serial.unwrap();
                publish_kind_for_test(
                    &permit.held.admitted.store,
                    &permit.held.locks,
                    &crate::layout::DayKey::parse("20260823").unwrap(),
                    PreparedLaterDirtyAuthority,
                )
                .unwrap();
                assert!(matches!(
                    permit.commit(),
                    Err(ConvergenceError::Refused(crate::Refusal::Superseded))
                ));
                drop(held);
                admitted.inspect().unwrap();
                (temporary, admitted, operation, selector, serial)
            }
            Matrix::DecisionedSuperseded => {
                let (temporary, admitted) = admit_days("matrix-decisioned", &["20260823"]);
                let (operation, selector, owner) = prepared(&admitted, false);
                let mut held = admitted.begin(owner).unwrap();
                let proof = admit_proof(&held, held.owner()).unwrap();
                let permit = held.continue_with(proof).unwrap();
                let serial = permit.held.serial.unwrap();
                publish_kind_for_test(
                    &permit.held.admitted.store,
                    &permit.held.locks,
                    &crate::layout::DayKey::parse("20260823").unwrap(),
                    PreparedCompletionAuthority,
                )
                .unwrap();
                assert!(matches!(
                    permit.commit(),
                    Err(ConvergenceError::Refused(crate::Refusal::Superseded))
                ));
                drop(held);
                (temporary, admitted, operation, selector, serial)
            }
        }
    }

    fn leave_successor_prefix(admitted: &Admitted, stage: SuccessorStage) {
        let operation = OperationId::generate().unwrap();
        let selector = GrantRequestSelector::empty(admitted.days()).unwrap();
        let owner = OwnerBinding::prepare(
            admitted,
            &operation,
            TransactionClass::AdvanceDirty,
            &selector,
        )
        .unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let guard = fail_after(stage.fault());
        let error = held.continue_with(proof).unwrap_err();
        drop(guard);
        assert!(
            matches!(error, ConvergenceError::PreservedPrior { .. }),
            "{}: {error:?}",
            stage.name()
        );
    }

    fn clearance_member_path(temporary: &TempDir) -> std::path::PathBuf {
        temporary
            .journal_path()
            .join("health/convergence/days/20260823.clear.json")
    }

    fn mutate_named_clearance_member(temporary: &TempDir, serial: u64, condition: MemberCondition) {
        let member_path = clearance_member_path(temporary);
        match condition {
            MemberCondition::ExactPresent => {}
            MemberCondition::BarrierAbsent => {
                std::fs::remove_file(temporary.journal_path().join(format!(
                    "health/convergence/clearance/{serial}.barrier.json"
                )))
                .unwrap();
            }
            MemberCondition::MemberUnlinked => {
                std::fs::remove_file(member_path).unwrap();
            }
            MemberCondition::MemberReplaced => {
                let bytes = std::fs::read(&member_path).unwrap();
                let mut member: ClearanceMember = serde_json::from_slice(&bytes).unwrap();
                member.serial += 1;
                let mut bytes = crate::digest::canonical_json_bytes(&member).unwrap();
                bytes.push(b'\n');
                std::fs::write(member_path, bytes).unwrap();
            }
            MemberCondition::BarrierDigestMismatched => {
                let path = temporary.journal_path().join(format!(
                    "health/convergence/clearance/{serial}.barrier.json"
                ));
                let bytes = std::fs::read(&path).unwrap();
                let mut barrier: ClearanceBarrier = serde_json::from_slice(&bytes).unwrap();
                barrier
                    .member_digests
                    .insert("20260823".to_owned(), "00".repeat(32));
                let mut bytes = crate::digest::canonical_json_bytes(&barrier).unwrap();
                bytes.push(b'\n');
                std::fs::write(path, bytes).unwrap();
            }
        }
    }

    fn reopen(temporary: &TempDir) -> Admitted {
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set.admit(root).unwrap(),
            Preflight::Empty => panic!("days"),
        }
    }

    fn publish_same_generation_completion(admitted: &Admitted) {
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
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
            &crate::layout::DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
    }

    #[test]
    fn reports_nonempty_committed_from_retained_history() {
        let (_temporary, admitted, operation, selector) = committed("outcome-nonempty");
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::NonemptyCommitted)
        );
    }

    #[test]
    fn reports_abort_no_open_from_retained_history() {
        let (_temporary, admitted) = admit_days("outcome-abort", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted, false);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap().abort().unwrap();
        drop(held);
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::Aborted)
        );
    }

    #[test]
    fn reports_empty_set_commit_without_resolver_artifacts() {
        let (_temporary, admitted) = admit_days("outcome-empty", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted, true);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap().commit().unwrap();
        drop(held);
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::EmptySetCommitted)
        );
    }

    #[test]
    fn reports_decisioned_superseded_from_the_reconciliation_barrier() {
        let (_temporary, admitted) = admit_days("outcome-decisioned", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted, false);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &crate::layout::DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        assert!(matches!(
            permit.commit(),
            Err(ConvergenceError::Refused(crate::Refusal::Superseded))
        ));
        drop(held);
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::DecisionedSuperseded)
        );
    }

    #[test]
    fn reports_passive_superseded_without_resolver_artifacts() {
        let (_temporary, admitted) = admit_days("outcome-passive", &["20260823"]);
        let (operation, selector, owner) = prepared(&admitted, true);
        let mut held = admitted.begin(owner).unwrap();
        let proof = admit_proof(&held, held.owner()).unwrap();
        let permit = held.continue_with(proof).unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &crate::layout::DayKey::parse("20260823").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        assert!(matches!(
            permit.commit(),
            Err(ConvergenceError::Refused(crate::Refusal::Superseded))
        ));
        drop(held);
        admitted.inspect().unwrap();
        assert_eq!(
            admitted.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::PassiveSuperseded)
        );
    }

    #[test]
    fn historical_matrix_boundary_names_the_first_changed_member() {
        let matrices = [
            Matrix::NonemptyCommitted,
            Matrix::Aborted,
            Matrix::EmptySetCommitted,
            Matrix::PassiveSuperseded,
            Matrix::DecisionedSuperseded,
        ];
        let stages = [
            SuccessorStage::BodyBeforeHead,
            SuccessorStage::HeadedBeforeIntent,
            SuccessorStage::IntentBeforeConsumption,
            SuccessorStage::ConsumptionWitness,
        ];
        let conditions = [
            MemberCondition::ExactPresent,
            MemberCondition::BarrierAbsent,
            MemberCondition::MemberUnlinked,
            MemberCondition::MemberReplaced,
            MemberCondition::BarrierDigestMismatched,
        ];

        for matrix in matrices {
            for stage in stages {
                for condition in conditions {
                    let (temporary, admitted, operation, selector, serial) = matrix_history(matrix);
                    leave_successor_prefix(&admitted, stage);
                    mutate_named_clearance_member(&temporary, serial, condition);
                    let state = admitted.grant_state(&operation, &selector);
                    match condition.expected_role() {
                        None => assert_eq!(
                            state.unwrap(),
                            GrantState::Outcome(matrix.outcome()),
                            "{} / {} retained exact evidence",
                            matrix.name(),
                            stage.name()
                        ),
                        Some(role) => assert!(
                            matches!(state, Err(ConvergenceError::Unknown { role: actual }) if actual == role),
                            "{} / {} / {} must name the changed clearance member: {state:?}",
                            matrix.name(),
                            stage.name(),
                            condition.name()
                        ),
                    }
                }
            }
        }
    }

    #[test]
    fn authorized_tombstone_survives_restart_but_missing_or_swapped_is_unknown() {
        let (temporary, admitted, operation, selector) = committed("outcome-tombstone");
        publish_same_generation_completion(&admitted);
        let day = crate::layout::DayKey::parse("20260823").unwrap();
        assert_eq!(
            admitted
                .revoke_grant(
                    &operation,
                    &selector,
                    &day,
                    WriterFamily::Think,
                    TargetScope::Chronicle,
                )
                .unwrap(),
            crate::GrantRevoke::Revoked
        );
        let member = temporary
            .journal_path()
            .join("health/convergence/registry/grants/members/1/20260823.think.chronicle.json");
        let tombstone = temporary.journal_path().join(
            "health/convergence/registry/grants/tombstones/member.1.20260823.think.chronicle.json",
        );
        let exact_tombstone = std::fs::read(&tombstone).unwrap();
        std::fs::remove_file(member).unwrap();
        drop(admitted);

        let resumed = reopen(&temporary);
        assert_eq!(
            resumed.grant_state(&operation, &selector).unwrap(),
            GrantState::Outcome(GrantOutcome::NonemptyCommitted)
        );

        std::fs::remove_file(&tombstone).unwrap();
        assert!(matches!(
            resumed.grant_state(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::GrantTombstone
            })
        ));

        std::fs::write(&tombstone, exact_tombstone).unwrap();
        let mut swapped: GrantTombstone =
            serde_json::from_slice(&std::fs::read(&tombstone).unwrap()).unwrap();
        swapped.tuple.day = "20260824".to_owned();
        let mut bytes = crate::digest::canonical_json_bytes(&swapped).unwrap();
        bytes.push(b'\n');
        std::fs::write(tombstone, bytes).unwrap();
        assert!(matches!(
            resumed.grant_state(&operation, &selector),
            Err(ConvergenceError::Unknown {
                role: DurableRole::GrantTombstone
            })
        ));
    }
}
