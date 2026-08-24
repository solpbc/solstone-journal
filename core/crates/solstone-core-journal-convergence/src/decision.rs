// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Decision, grant members, and the historical barriers.
//!
//! Ordering, and the reason for it: a nonempty commit durably records one
//! immutable decision and its complete tuple set, activates each member under
//! the ordered day leases, then publishes an all-grants-active barrier that is
//! separate from bounded terminal history. The barrier plus the exact member
//! set only *prepares* the outbox. It grants no token, reissue, or mutation
//! authority until the exact root-bound `committed` terminal is durable, so a
//! partial or uncertain activation, and the whole barrier-to-terminal window,
//! deterministically follow the durable decision and can never validate a
//! subset early.
//!
//! Every transition-derived field of every tuple is read from that
//! transition's exact store-proposed day record under the live lease. Nothing
//! here accepts a generation or an author serial from a caller.
//!
//! Decisioned supersession is the only alternative to a fixed commit. It never
//! chooses abort: it marks every requested member superseded, publishes an
//! all-members-superseded barrier while retaining any prior all-active barrier
//! as history, and emits no token.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::digest::{digest_value, digest_value_excluding};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::layout::{
    ACTIVE_BARRIER_SUFFIX, BARRIERS, GRANTS, MEMBERS, barrier_file_name, decision_name,
    member_file_name, serial_dir,
};
use crate::owner::OwnerBinding;
use crate::registry::RegistrySection;
use crate::schema::{
    DecisionKind, GrantBarrier, GrantDecision, GrantMember, GrantTuple, Intent, MemberState,
    ROLE_GRANT_ALL_ACTIVE, ROLE_GRANT_ALL_SUPERSEDED, ROLE_GRANT_DECISION, ROLE_GRANT_MEMBER,
    SCHEMA_VERSION, read_json, write_json_exclusive,
};
use crate::selector::GrantRequestSelector;
use crate::store::DaySnapshot;
use crate::walk::open_dir;

/// Crash boundaries. Paired across `cfg(test)` so every input stays used in a
/// production build and no `allow` is needed to silence one.
#[cfg(test)]
fn member_boundary(index: usize) -> Result<(), ConvergenceError> {
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterGrantMember { index: index as u8 },
    ) {
        return Err(ConvergenceError::Io {
            operation: "inject after grant member",
            role: DurableRole::GrantMember,
            source: std::io::Error::other("injected"),
        });
    }
    Ok(())
}

#[cfg(not(test))]
fn member_boundary(_index: usize) -> Result<(), ConvergenceError> {
    Ok(())
}

#[cfg(test)]
fn barrier_boundary(suffix: &str, role: DurableRole) -> Result<(), ConvergenceError> {
    let fault = if suffix == ACTIVE_BARRIER_SUFFIX {
        crate::test_support::PublishFault::AfterAllActiveBarrier
    } else {
        crate::test_support::PublishFault::AfterAllSupersededBarrier
    };
    if crate::test_support::take_publish_fault(fault) {
        return Err(ConvergenceError::Io {
            operation: "inject after grant barrier",
            role,
            source: std::io::Error::other("injected"),
        });
    }
    Ok(())
}

#[cfg(not(test))]
fn barrier_boundary(_suffix: &str, _role: DurableRole) -> Result<(), ConvergenceError> {
    Ok(())
}

fn map_dir(error: solstone_core_journal_io::PathError) -> ConvergenceError {
    ConvergenceError::Io {
        operation: "create grants directory",
        role: DurableRole::Directory,
        source: std::io::Error::other(error.to_string()),
    }
}

fn open_grants(section: &RegistrySection<'_>) -> Result<Option<OwnedFd>, ConvergenceError> {
    open_dir(section.registry(), GRANTS)
}

fn ensure_child(parent: &OwnedFd, name: &str) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(parent, OsStr::new(name), 0o700).map_err(map_dir)?;
    open_dir(parent, name)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

/// Derive one tuple per canonical request from the exact store-proposed day
/// records. `snapshots` must have been read under the live day leases.
pub(crate) fn derive_tuples(
    selector: &GrantRequestSelector,
    snapshots: &BTreeMap<String, DaySnapshot>,
) -> Result<Vec<GrantTuple>, ConvergenceError> {
    let mut tuples = Vec::new();
    for (day, writer_family, target_scope) in selector.requests() {
        let snapshot = snapshots.get(day).ok_or(ConvergenceError::Unknown {
            role: DurableRole::Record,
        })?;
        tuples.push(GrantTuple {
            day: day.to_owned(),
            writer_family,
            target_scope,
            dirty_generation: snapshot.dirty_generation,
            dirty_by_transition_serial: snapshot.dirty_by_transition_serial,
        });
    }
    tuples.sort();
    Ok(tuples)
}

/// Read: the decision at `serial`, or `None`. Never creates.
pub(crate) fn load_decision(
    section: &RegistrySection<'_>,
    serial: u64,
) -> Result<Option<GrantDecision>, ConvergenceError> {
    let Some(decisions) = open_dir(section.registry(), crate::layout::DECISIONS)? else {
        return Ok(None);
    };
    read_json(&decisions, &decision_name(serial), DurableRole::Decision)
}

/// Write: one immutable decision. Re-running with the same decision is
/// idempotent; a different decision at the same serial is unknown and is never
/// overwritten, because two decisions for one transition cannot both be true.
pub(crate) fn publish_decision(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    intent: &Intent,
    kind: DecisionKind,
    tuples: Vec<GrantTuple>,
) -> Result<GrantDecision, ConvergenceError> {
    let mut decision = GrantDecision {
        role: ROLE_GRANT_DECISION.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: owner.journal_id().to_owned(),
        root_id: owner.root_id().to_owned(),
        serial: intent.serial,
        operation_id: owner.operation_id().to_owned(),
        owner_binding_digest: owner.digest_hex().to_owned(),
        selector_digest: owner.selector_digest().to_owned(),
        intent_digest: intent.intent_digest.clone(),
        kind,
        day_set: intent.day_set.clone(),
        tuples,
        decision_digest: String::new(),
    };
    decision.decision_digest = digest_value_excluding(&decision, "decision_digest")?
        .as_hex()
        .to_owned();
    let decisions = ensure_child(section.registry(), crate::layout::DECISIONS)?;
    match write_json_exclusive(
        &decisions,
        &decision_name(intent.serial),
        &decision,
        DurableRole::Decision,
    ) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync_dir_bound(&decisions).map_err(|source| ConvergenceError::Io {
        operation: "sync decisions directory",
        role: DurableRole::Decision,
        source,
    })?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterDecision) {
        return Err(ConvergenceError::Io {
            operation: "inject after decision",
            role: DurableRole::Decision,
            source: std::io::Error::other("injected"),
        });
    }
    let durable = load_decision(section, intent.serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Decision,
    })?;
    if durable != decision {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Decision,
        });
    }
    Ok(durable)
}

/// Read-only classification of a durable decision against this transition and
/// the outcome the caller is trying to reach. A decision already fixed for the
/// opposite outcome refuses; it is never rewritten.
pub(crate) fn accept_decision(
    decision: GrantDecision,
    owner: &OwnerBinding,
    intent: &Intent,
    wanted: DecisionKind,
) -> Result<GrantDecision, ConvergenceError> {
    if decision.role != ROLE_GRANT_DECISION || decision.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Decision,
        });
    }
    if decision.journal_id != owner.journal_id() || decision.root_id != owner.root_id() {
        return Err(ConvergenceError::Refused(Refusal::WrongLineage));
    }
    if decision.operation_id != owner.operation_id() {
        return Err(ConvergenceError::Refused(Refusal::WrongOperation));
    }
    if decision.selector_digest != owner.selector_digest() {
        return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
    }
    if decision.serial != intent.serial || decision.intent_digest != intent.intent_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Decision,
        });
    }
    let recomputed = {
        let mut probe = decision.clone();
        probe.decision_digest = String::new();
        digest_value_excluding(&probe, "decision_digest")?
            .as_hex()
            .to_owned()
    };
    if recomputed != decision.decision_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Decision,
        });
    }
    if decision.kind != wanted {
        return Err(ConvergenceError::Refused(Refusal::OppositeTerminal));
    }
    Ok(decision)
}

fn member_digest_of(member: &GrantMember) -> Result<String, ConvergenceError> {
    Ok(digest_value_excluding(member, "member_digest")?
        .as_hex()
        .to_owned())
}

/// Read: one member, or `None`. Never creates.
pub(crate) fn load_member(
    section: &RegistrySection<'_>,
    serial: u64,
    tuple: &GrantTuple,
) -> Result<Option<GrantMember>, ConvergenceError> {
    let Some(grants) = open_grants(section)? else {
        return Ok(None);
    };
    let Some(members) = open_dir(&grants, MEMBERS)? else {
        return Ok(None);
    };
    let Some(directory) = open_dir(&members, &serial_dir(serial))? else {
        return Ok(None);
    };
    read_json(
        &directory,
        &member_file_name(tuple),
        DurableRole::GrantMember,
    )
}

fn members_dir(section: &RegistrySection<'_>, serial: u64) -> Result<OwnedFd, ConvergenceError> {
    let grants = ensure_child(section.registry(), GRANTS)?;
    let members = ensure_child(&grants, MEMBERS)?;
    ensure_child(&members, &serial_dir(serial))
}

/// Write: activate one member. Create-only, then exact re-read. An existing
/// member in any state other than the requested activation is left untouched
/// and reported, so activation can never silently revive revoked or superseded
/// membership.
pub(crate) fn activate_member(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    serial: u64,
    tuple: &GrantTuple,
    index: usize,
) -> Result<GrantMember, ConvergenceError> {
    if let Some(existing) = load_member(section, serial, tuple)? {
        return accept_member(existing, owner, serial, tuple);
    }
    let mut member = GrantMember {
        role: ROLE_GRANT_MEMBER.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: owner.journal_id().to_owned(),
        root_id: owner.root_id().to_owned(),
        serial,
        operation_id: owner.operation_id().to_owned(),
        owner_binding_digest: owner.digest_hex().to_owned(),
        selector_digest: owner.selector_digest().to_owned(),
        tuple: tuple.clone(),
        state: MemberState::Active,
        member_digest: String::new(),
    };
    member.member_digest = member_digest_of(&member)?;
    let directory = members_dir(section, serial)?;
    match write_json_exclusive(
        &directory,
        &member_file_name(tuple),
        &member,
        DurableRole::GrantMember,
    ) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync_dir_bound(&directory).map_err(|source| ConvergenceError::Io {
        operation: "sync members directory",
        role: DurableRole::GrantMember,
        source,
    })?;
    member_boundary(index)?;
    let durable = load_member(section, serial, tuple)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantMember,
    })?;
    accept_member(durable, owner, serial, tuple)
}

/// Read-only classification of a durable member against the expected identity.
fn accept_member(
    member: GrantMember,
    owner: &OwnerBinding,
    serial: u64,
    tuple: &GrantTuple,
) -> Result<GrantMember, ConvergenceError> {
    if member.role != ROLE_GRANT_MEMBER || member.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantMember,
        });
    }
    if member.journal_id != owner.journal_id() || member.root_id != owner.root_id() {
        return Err(ConvergenceError::Refused(Refusal::WrongLineage));
    }
    if member.operation_id != owner.operation_id() {
        return Err(ConvergenceError::Refused(Refusal::WrongOperation));
    }
    if member.selector_digest != owner.selector_digest() {
        return Err(ConvergenceError::Refused(Refusal::ConflictingSelector));
    }
    if member.serial != serial || &member.tuple != tuple {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantMember,
        });
    }
    if member.member_digest != member_digest_of(&member)? {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantMember,
        });
    }
    match member.state {
        MemberState::Active => Ok(member),
        MemberState::RevocationPending | MemberState::Revoked => {
            Err(ConvergenceError::Refused(Refusal::GrantMemberRevoked))
        }
        MemberState::Superseded => Err(ConvergenceError::Refused(Refusal::GrantMemberSuperseded)),
    }
}

/// Write: replace one member's state. Only supersession uses this in this
/// lode; the member file itself is retained so full membership survives.
pub(crate) fn set_member_state(
    section: &RegistrySection<'_>,
    serial: u64,
    tuple: &GrantTuple,
    state: MemberState,
) -> Result<GrantMember, ConvergenceError> {
    let mut member = load_member(section, serial, tuple)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantMember,
    })?;
    if member.state == state {
        return Ok(member);
    }
    member.state = state;
    member.member_digest = String::new();
    member.member_digest = member_digest_of(&member)?;
    let directory = members_dir(section, serial)?;
    crate::schema::replace_json(&directory, &member_file_name(tuple), &member)?;
    sync_dir_bound(&directory).map_err(|source| ConvergenceError::Io {
        operation: "sync members directory",
        role: DurableRole::GrantMember,
        source,
    })?;
    let durable = load_member(section, serial, tuple)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::GrantMember,
    })?;
    if durable != member {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::GrantMember,
        });
    }
    Ok(durable)
}

fn barriers_dir(section: &RegistrySection<'_>) -> Result<OwnedFd, ConvergenceError> {
    let grants = ensure_child(section.registry(), GRANTS)?;
    ensure_child(&grants, BARRIERS)
}

/// Read: a barrier by suffix, or `None`. Never creates.
pub(crate) fn load_barrier(
    section: &RegistrySection<'_>,
    serial: u64,
    suffix: &str,
) -> Result<Option<GrantBarrier>, ConvergenceError> {
    let Some(grants) = open_grants(section)? else {
        return Ok(None);
    };
    let Some(barriers) = open_dir(&grants, BARRIERS)? else {
        return Ok(None);
    };
    let role = if suffix == ACTIVE_BARRIER_SUFFIX {
        DurableRole::GrantActiveBarrier
    } else {
        DurableRole::GrantSupersededBarrier
    };
    read_json(&barriers, &barrier_file_name(serial, suffix), role)
}

/// Write: publish one barrier over the complete member set. Create-only and
/// exact-idempotent; a disagreeing barrier at the same serial is unknown.
pub(crate) struct BarrierSpec<'b> {
    pub serial: u64,
    pub day_set: &'b [String],
    pub members: &'b [GrantMember],
    pub suffix: &'b str,
    pub descendant_discriminator: Option<BTreeMap<String, String>>,
    pub prior_all_active_digest: Option<String>,
}

pub(crate) fn publish_barrier(
    section: &RegistrySection<'_>,
    owner: &OwnerBinding,
    spec: BarrierSpec<'_>,
) -> Result<GrantBarrier, ConvergenceError> {
    let BarrierSpec {
        serial,
        day_set,
        members,
        suffix,
        descendant_discriminator,
        prior_all_active_digest,
    } = spec;
    let role = if suffix == ACTIVE_BARRIER_SUFFIX {
        ROLE_GRANT_ALL_ACTIVE
    } else {
        ROLE_GRANT_ALL_SUPERSEDED
    };
    let durable_role = if suffix == ACTIVE_BARRIER_SUFFIX {
        DurableRole::GrantActiveBarrier
    } else {
        DurableRole::GrantSupersededBarrier
    };
    let mut member_digests = BTreeMap::new();
    for member in members {
        member_digests.insert(member_key(&member.tuple), member.member_digest.clone());
    }
    let mut barrier = GrantBarrier {
        role: role.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: owner.journal_id().to_owned(),
        root_id: owner.root_id().to_owned(),
        serial,
        operation_id: owner.operation_id().to_owned(),
        selector_digest: owner.selector_digest().to_owned(),
        day_set: day_set.to_vec(),
        member_digests,
        descendant_discriminator,
        prior_all_active_digest,
        barrier_digest: String::new(),
    };
    barrier.barrier_digest = digest_value_excluding(&barrier, "barrier_digest")?
        .as_hex()
        .to_owned();
    let directory = barriers_dir(section)?;
    match write_json_exclusive(
        &directory,
        &barrier_file_name(serial, suffix),
        &barrier,
        durable_role,
    ) {
        Ok(_) => {}
        Err(ConvergenceError::PreservedPrior { .. }) => {}
        Err(error) => return Err(error),
    }
    sync_dir_bound(&directory).map_err(|source| ConvergenceError::Io {
        operation: "sync barriers directory",
        role: durable_role,
        source,
    })?;
    barrier_boundary(suffix, durable_role)?;
    let durable = load_barrier(section, serial, suffix)?
        .ok_or(ConvergenceError::Unknown { role: durable_role })?;
    if durable != barrier {
        return Err(ConvergenceError::Unknown { role: durable_role });
    }
    Ok(durable)
}

/// Stable member key used inside a barrier's digest map and as its file stem.
pub(crate) fn member_key(tuple: &GrantTuple) -> String {
    format!(
        "{}.{}.{}",
        tuple.day,
        tuple.writer_family.as_str(),
        tuple.target_scope.as_str()
    )
}

/// The canonical discriminator a superseded barrier binds: the current record
/// digest per day at the moment supersession was decided.
pub(crate) fn descendant_discriminator(
    snapshots: &BTreeMap<String, DaySnapshot>,
) -> BTreeMap<String, String> {
    snapshots
        .iter()
        .map(|(day, snapshot)| (day.clone(), snapshot.digest.as_hex().to_owned()))
        .collect()
}

/// Digest of a barrier as stored, for the retained-history reference.
pub(crate) fn barrier_digest(barrier: &GrantBarrier) -> Result<String, ConvergenceError> {
    Ok(digest_value(barrier)?.as_hex().to_owned())
}

/// The ordered nonempty-commit sequence: decision, then every member under the
/// held leases, then the all-active barrier, then the base committed terminal.
///
/// If the canonical fold no longer permits the fixed commit -- an exact
/// same-generation completion or a verified later dirty descendant -- the only
/// alternative is decisioned-supersession reconciliation. It binds the commit
/// decision, marks every requested member superseded, publishes the
/// all-members-superseded barrier while retaining any prior all-active barrier
/// as history, and then refuses as superseded so the base's owner-free
/// no-permit recovery publishes the superseded terminal and drives clearance,
/// eviction, and claim release. It never chooses abort and never emits a token.
pub(crate) fn commit_with_grants(
    held: &mut crate::transaction::HeldDays<'_>,
) -> Result<crate::permit::TerminalReceipt, ConvergenceError> {
    let selector_is_empty = held.owner().selector().is_empty();
    if selector_is_empty {
        // Empty grant set follows ordinary commit with no resolver decision,
        // member, or barrier state at all.
        return crate::terminal::publish_from_permit(
            held,
            crate::permit::TerminalOutcome::Committed,
        );
    }
    let store = &held.admitted.store;
    store.revalidate()?;
    held.locks
        .matches(store.journal_id(), store.root_id(), store.object_identity())?;
    let dirs = crate::init::open_store_dirs(store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let serial = held
        .serial
        .ok_or(ConvergenceError::Refused(Refusal::NoPermit))?;
    let intent = crate::intent::read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    let (snapshots, commit_permitted) = fold_days(held, &intent)?;

    // The decision is durable before any member exists, so every partial
    // activation prefix has exactly one lawful continuation. It is bound once
    // and never rebound: on resume the durable decision is authoritative, and
    // its tuples are *not* re-derived, because a descendant may have advanced
    // the day records since it was fixed. Re-deriving would silently mint a
    // different decision for one transition.
    let decision = {
        let section = crate::registry::enter_registry(&dirs)?;
        match load_decision(&section, serial)? {
            Some(existing) => {
                accept_decision(existing, held.owner(), &intent, DecisionKind::Commit)?
            }
            None => {
                let tuples = derive_tuples(held.owner().selector(), &snapshots)?;
                publish_decision(
                    &section,
                    held.owner(),
                    &intent,
                    DecisionKind::Commit,
                    tuples,
                )?
            }
        }
    };
    let tuples = decision.tuples.clone();

    if !commit_permitted {
        return reconcile_superseded(held, &dirs, serial, &intent, &tuples, &snapshots);
    }

    let mut members = Vec::new();
    for (index, tuple) in tuples.iter().enumerate() {
        let section = crate::registry::enter_registry(&dirs)?;
        members.push(activate_member(
            &section,
            held.owner(),
            serial,
            tuple,
            index,
        )?);
    }
    {
        let section = crate::registry::enter_registry(&dirs)?;
        publish_barrier(
            &section,
            held.owner(),
            BarrierSpec {
                serial,
                day_set: &intent.day_set,
                members: &members,
                suffix: ACTIVE_BARRIER_SUFFIX,
                descendant_discriminator: None,
                prior_all_active_digest: None,
            },
        )?;
    }
    crate::terminal::publish_from_permit(held, crate::permit::TerminalOutcome::Committed)
}

/// Read-only fold of the held days against the intent's proposed revisions.
/// Returns the snapshots plus whether the fixed commit is still permitted.
fn fold_days(
    held: &crate::transaction::HeldDays<'_>,
    intent: &Intent,
) -> Result<(BTreeMap<String, DaySnapshot>, bool), ConvergenceError> {
    let store = &held.admitted.store;
    let mut snapshots = BTreeMap::new();
    let mut permitted = true;
    for day in &held.days {
        let proposed = *intent
            .proposed_day_revisions
            .get(day.as_str())
            .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?;
        match crate::publish::inspect_against_proposed(store, &held.locks, day, proposed)? {
            crate::store::LoadDay::Published(snapshot) => {
                if snapshot.record_revision != proposed
                    || snapshot.completed_generation >= snapshot.dirty_generation
                {
                    permitted = false;
                }
                snapshots.insert(day.as_str().to_owned(), snapshot);
            }
            crate::store::LoadDay::HeadedDescendant { .. } => {
                permitted = false;
                match store.load_day(&held.locks, day)? {
                    crate::store::LoadDay::Published(snapshot) => {
                        snapshots.insert(day.as_str().to_owned(), snapshot);
                    }
                    _ => {
                        return Err(ConvergenceError::Unknown {
                            role: DurableRole::Record,
                        });
                    }
                }
            }
            _ => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                });
            }
        }
    }
    Ok((snapshots, permitted))
}

/// Decisioned supersession. Every prefix of this resumes only this branch.
fn reconcile_superseded(
    held: &mut crate::transaction::HeldDays<'_>,
    dirs: &crate::init::StoreDirs,
    serial: u64,
    intent: &Intent,
    tuples: &[GrantTuple],
    snapshots: &BTreeMap<String, DaySnapshot>,
) -> Result<crate::permit::TerminalReceipt, ConvergenceError> {
    let mut superseded = Vec::new();
    let prior_all_active = {
        let section = crate::registry::enter_registry(dirs)?;
        load_barrier(&section, serial, crate::layout::ACTIVE_BARRIER_SUFFIX)?
    };
    for (index, tuple) in tuples.iter().enumerate() {
        let section = crate::registry::enter_registry(dirs)?;
        // Activation prefixes, including a complete all-active barrier, all
        // convert to superseded membership. Nothing is delivered from here.
        if load_member(&section, serial, tuple)?.is_none() {
            let _ = activate_member(&section, held.owner(), serial, tuple, index);
        }
        superseded.push(set_member_state(
            &section,
            serial,
            tuple,
            MemberState::Superseded,
        )?);
    }
    let prior_digest = match prior_all_active.as_ref() {
        Some(barrier) => Some(barrier_digest(barrier)?),
        None => None,
    };
    {
        let section = crate::registry::enter_registry(dirs)?;
        publish_barrier(
            &section,
            held.owner(),
            BarrierSpec {
                serial,
                day_set: &intent.day_set,
                members: &superseded,
                suffix: crate::layout::SUPERSEDED_BARRIER_SUFFIX,
                descendant_discriminator: Some(descendant_discriminator(snapshots)),
                prior_all_active_digest: prior_digest,
            },
        )?;
    }
    // The base publishes the superseded terminal from owner-free no-permit
    // recovery, never from a live permit, so the reconciliation stops here and
    // the caller routes into that recovery.
    Err(ConvergenceError::Refused(Refusal::Superseded))
}

/// Abort records the no-open decision, then terminalizes. No member or barrier
/// state is ever created on this path.
pub(crate) fn abort_with_decision(
    held: &mut crate::transaction::HeldDays<'_>,
) -> Result<crate::permit::TerminalReceipt, ConvergenceError> {
    let store = &held.admitted.store;
    store.revalidate()?;
    let dirs = crate::init::open_store_dirs(store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let serial = held
        .serial
        .ok_or(ConvergenceError::Refused(Refusal::NoPermit))?;
    let intent = crate::intent::read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    {
        let section = crate::registry::enter_registry(&dirs)?;
        match load_decision(&section, serial)? {
            // A fixed commit decision can never be turned into an abort by an
            // owner choosing the opposite terminal.
            Some(existing) => {
                accept_decision(existing, held.owner(), &intent, DecisionKind::AbortNoOpen)?;
            }
            None => {
                publish_decision(
                    &section,
                    held.owner(),
                    &intent,
                    DecisionKind::AbortNoOpen,
                    Vec::new(),
                )?;
            }
        }
    }
    crate::terminal::publish_from_permit(held, crate::permit::TerminalOutcome::Aborted)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::layout::{DayKey, SUPERSEDED_BARRIER_SUFFIX};
    use crate::permit::TerminalOutcome;
    use crate::publish::{
        PreparedCompletionAuthority, PreparedLaterDirtyAuthority, publish_kind_for_test,
    };
    use crate::selector::{TargetScope, WriterFamily};
    use crate::test_support::{
        PublishFault, TempDir, admit_days, continue_ok, continue_ok_with, fail_after,
    };
    use std::path::PathBuf;

    const ONE: [(&str, WriterFamily, TargetScope); 1] =
        [("20260823", WriterFamily::Think, TargetScope::Chronicle)];

    fn two() -> Vec<(&'static str, WriterFamily, TargetScope)> {
        vec![
            ("20260823", WriterFamily::Think, TargetScope::Chronicle),
            ("20260823", WriterFamily::Observe, TargetScope::Entities),
        ]
    }

    fn grants_dir(temporary: &TempDir) -> PathBuf {
        temporary
            .journal_path()
            .join("health/convergence/registry/grants")
    }

    fn decisions_dir(temporary: &TempDir) -> PathBuf {
        temporary
            .journal_path()
            .join("health/convergence/registry/decisions")
    }

    fn read_decision(temporary: &TempDir, serial: u64) -> GrantDecision {
        let bytes = std::fs::read(decisions_dir(temporary).join(format!("{serial}.json"))).unwrap();
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap()
    }

    fn member_files(temporary: &TempDir, serial: u64) -> Vec<String> {
        let directory = grants_dir(temporary)
            .join("members")
            .join(serial.to_string());
        let Ok(listing) = std::fs::read_dir(&directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = listing
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn read_member_file(temporary: &TempDir, serial: u64, name: &str) -> GrantMember {
        let bytes = std::fs::read(
            grants_dir(temporary)
                .join("members")
                .join(serial.to_string())
                .join(name),
        )
        .unwrap();
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap()
    }

    fn barrier_exists(temporary: &TempDir, serial: u64, suffix: &str) -> bool {
        grants_dir(temporary)
            .join("barriers")
            .join(format!("{serial}.{suffix}.json"))
            .is_file()
    }

    fn read_barrier_file(temporary: &TempDir, serial: u64, suffix: &str) -> GrantBarrier {
        let bytes = std::fs::read(
            grants_dir(temporary)
                .join("barriers")
                .join(format!("{serial}.{suffix}.json")),
        )
        .unwrap();
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).unwrap()
    }

    #[test]
    fn empty_selector_commit_creates_no_resolver_grant_state() {
        let (temporary, admitted) = admit_days("empty-grants", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
        // Empty grant set follows ordinary commit: no decision, no member, no
        // barrier.
        assert!(
            !decisions_dir(&temporary)
                .join(format!("{serial}.json"))
                .exists()
        );
        assert!(member_files(&temporary, serial).is_empty());
        assert!(!barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        assert!(!barrier_exists(
            &temporary,
            serial,
            SUPERSEDED_BARRIER_SUFFIX
        ));
        drop(held);
    }

    #[test]
    fn nonempty_commit_records_decision_members_and_barrier() {
        let (temporary, admitted) = admit_days("nonempty", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &two());
        let serial = held.serial.unwrap();
        let snapshot = held.snapshot(&DayKey::parse("20260823").unwrap()).unwrap();
        let permit = held.proceed().unwrap();
        let receipt = permit.commit().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);

        let decision = read_decision(&temporary, serial);
        assert_eq!(decision.role, ROLE_GRANT_DECISION);
        assert_eq!(decision.kind, DecisionKind::Commit);
        assert_eq!(decision.tuples.len(), 2);

        // Every transition-derived field comes from the store-proposed record,
        // never from the caller: the caller only named day/family/scope.
        for tuple in &decision.tuples {
            assert_eq!(tuple.dirty_generation, snapshot.dirty_generation);
            assert_eq!(
                tuple.dirty_by_transition_serial,
                snapshot.dirty_by_transition_serial
            );
        }

        assert_eq!(
            member_files(&temporary, serial),
            vec![
                "20260823.observe.entities.json".to_owned(),
                "20260823.think.chronicle.json".to_owned()
            ]
        );
        for name in member_files(&temporary, serial) {
            let member = read_member_file(&temporary, serial, &name);
            assert_eq!(member.state, MemberState::Active);
            assert_eq!(member.serial, serial);
        }

        let barrier = read_barrier_file(&temporary, serial, ACTIVE_BARRIER_SUFFIX);
        assert_eq!(barrier.role, ROLE_GRANT_ALL_ACTIVE);
        assert_eq!(barrier.member_digests.len(), 2);
        assert!(barrier.descendant_discriminator.is_none());
        assert!(!barrier_exists(
            &temporary,
            serial,
            SUPERSEDED_BARRIER_SUFFIX
        ));
        drop(held);
    }

    #[test]
    fn abort_records_no_open_and_never_opens_a_member() {
        let (temporary, admitted) = admit_days("abort-decision", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &ONE);
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        let receipt = permit.abort().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Aborted);
        let decision = read_decision(&temporary, serial);
        assert_eq!(decision.kind, DecisionKind::AbortNoOpen);
        assert!(decision.tuples.is_empty());
        // Aborted history requires the exact absence of member and barrier state.
        assert!(member_files(&temporary, serial).is_empty());
        assert!(!barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        drop(held);
    }

    #[test]
    fn crash_after_decision_leaves_no_member_and_resumes_to_commit() {
        let (temporary, admitted) = admit_days("crash-decision", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &two());
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterDecision);
        let error = permit.commit().unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::Decision,
                ..
            }
        ));
        // Decision durable, nothing opened.
        assert_eq!(read_decision(&temporary, serial).kind, DecisionKind::Commit);
        assert!(member_files(&temporary, serial).is_empty());
        assert!(!barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        // Resume finishes the same decision.
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
        assert_eq!(member_files(&temporary, serial).len(), 2);
        assert!(barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        drop(held);
    }

    #[test]
    fn crash_after_first_member_resumes_the_remaining_members() {
        let (temporary, admitted) = admit_days("crash-member", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &two());
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterGrantMember { index: 0 });
        let error = permit.commit().unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::GrantMember,
                ..
            }
        ));
        assert_eq!(member_files(&temporary, serial).len(), 1);
        assert!(!barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
        assert_eq!(member_files(&temporary, serial).len(), 2);
        assert!(barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        drop(held);
    }

    #[test]
    fn crash_after_all_active_barrier_leaves_no_terminal_and_resumes() {
        let (temporary, admitted) = admit_days("crash-barrier", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &ONE);
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterAllActiveBarrier);
        let error = permit.commit().unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::GrantActiveBarrier,
                ..
            }
        ));
        // The outbox is fully prepared, but the terminal is not durable, so
        // nothing downstream may treat the grant as usable yet.
        assert!(barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        assert!(
            !temporary
                .journal_path()
                .join(format!("health/convergence/terminals/{serial}.json"))
                .exists()
        );
        let permit = held.proceed().unwrap();
        assert_eq!(permit.commit().unwrap().outcome, TerminalOutcome::Committed);
        drop(held);
    }

    #[test]
    fn completion_after_decision_forces_decisioned_supersession() {
        let (temporary, admitted) = admit_days("supersede-completion", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &two());
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        let error = permit.commit().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::Superseded)
        ));
        // The commit decision is still bound; supersession never rewrites it as
        // an abort.
        assert_eq!(read_decision(&temporary, serial).kind, DecisionKind::Commit);
        // Every requested member is superseded, and the superseded barrier
        // binds the descendant discriminator.
        let names = member_files(&temporary, serial);
        assert_eq!(names.len(), 2);
        for name in &names {
            assert_eq!(
                read_member_file(&temporary, serial, name).state,
                MemberState::Superseded
            );
        }
        let barrier = read_barrier_file(&temporary, serial, SUPERSEDED_BARRIER_SUFFIX);
        assert_eq!(barrier.role, ROLE_GRANT_ALL_SUPERSEDED);
        assert!(barrier.descendant_discriminator.is_some());
        assert_eq!(barrier.member_digests.len(), 2);
        drop(held);
        // The base publishes the superseded terminal from owner-free recovery.
        let report = admitted.inspect().unwrap();
        assert_eq!(report.terminal_outcome(), Some(TerminalOutcome::Superseded));
    }

    #[test]
    fn later_dirty_after_all_active_barrier_supersedes_and_retains_history() {
        let (temporary, admitted) = admit_days("supersede-after-barrier", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &ONE);
        let serial = held.serial.unwrap();
        // Reach a complete all-active barrier first.
        let permit = held.proceed().unwrap();
        let guard = fail_after(PublishFault::AfterAllActiveBarrier);
        permit.commit().unwrap_err();
        drop(guard);
        assert!(barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        let active_before = std::fs::read(
            grants_dir(&temporary)
                .join("barriers")
                .join(format!("{serial}.{ACTIVE_BARRIER_SUFFIX}.json")),
        )
        .unwrap();

        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        let error = permit.commit().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::Superseded)),
            "{error:?}"
        );
        // A complete all-active barrier converts to the all-members-superseded
        // barrier, and the prior barrier is retained as history rather than
        // being unlinked.
        assert!(barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        assert_eq!(
            active_before,
            std::fs::read(
                grants_dir(&temporary)
                    .join("barriers")
                    .join(format!("{serial}.{ACTIVE_BARRIER_SUFFIX}.json"))
            )
            .unwrap()
        );
        let superseded = read_barrier_file(&temporary, serial, SUPERSEDED_BARRIER_SUFFIX);
        assert!(superseded.prior_all_active_digest.is_some());
        for name in member_files(&temporary, serial) {
            assert_eq!(
                read_member_file(&temporary, serial, &name).state,
                MemberState::Superseded
            );
        }
        drop(held);
    }

    #[test]
    fn crash_during_supersession_resumes_only_supersession() {
        let (temporary, admitted) = admit_days("supersede-crash", &["20260823"]);
        let mut held = continue_ok_with(&admitted, &ONE);
        let serial = held.serial.unwrap();
        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        let guard = fail_after(PublishFault::AfterAllSupersededBarrier);
        let error = permit.commit().unwrap_err();
        drop(guard);
        assert!(matches!(
            error,
            ConvergenceError::Io {
                role: DurableRole::GrantSupersededBarrier,
                ..
            }
        ));
        // The superseded barrier is already durable from the crashed attempt.
        assert!(barrier_exists(
            &temporary,
            serial,
            SUPERSEDED_BARRIER_SUFFIX
        ));
        // Retrying cannot choose commit or abort. With the descendant standing,
        // the permit itself is refused, so no owner-selected terminal is
        // reachable and the only continuation is the base's owner-free
        // superseded recovery.
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::Superseded)),
            "{error:?}"
        );
        assert_eq!(read_decision(&temporary, serial).kind, DecisionKind::Commit);
        drop(held);
        let report = admitted.inspect().unwrap();
        assert_eq!(report.terminal_outcome(), Some(TerminalOutcome::Superseded));
        // Supersession never emitted a token and never became an abort.
        assert!(!barrier_exists(&temporary, serial, ACTIVE_BARRIER_SUFFIX));
        assert_eq!(read_decision(&temporary, serial).kind, DecisionKind::Commit);
    }

    #[test]
    fn member_state_is_not_revived_by_reactivation() {
        let (temporary, admitted) = admit_days("no-revive", &["20260823"]);
        let held = continue_ok_with(&admitted, &ONE);
        let serial = held.serial.unwrap();
        let dirs = crate::init::open_store_dirs(admitted.store().root())
            .unwrap()
            .unwrap();
        let snapshot = held.snapshot(&DayKey::parse("20260823").unwrap()).unwrap();
        let tuple = GrantTuple {
            day: "20260823".to_owned(),
            writer_family: WriterFamily::Think,
            target_scope: TargetScope::Chronicle,
            dirty_generation: snapshot.dirty_generation,
            dirty_by_transition_serial: snapshot.dirty_by_transition_serial,
        };
        let section = crate::registry::enter_registry(&dirs).unwrap();
        activate_member(&section, held.owner(), serial, &tuple, 0).unwrap();
        set_member_state(&section, serial, &tuple, MemberState::Revoked).unwrap();
        // Activation over revoked membership reports the revocation instead of
        // silently reviving it.
        let error = activate_member(&section, held.owner(), serial, &tuple, 0).unwrap_err();
        drop(section);
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::GrantMemberRevoked)
        ));
        assert_eq!(
            read_member_file(&temporary, serial, "20260823.think.chronicle.json").state,
            MemberState::Revoked
        );
        drop(held);
    }
}
