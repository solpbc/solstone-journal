// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Create-only terminal publication, re-read polarity, and T2–T7 cleanup.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::{create_directory_bound, sync_dir_bound};

use crate::allocate::load_adoption;
use crate::digest::{RecordDigest, digest_value, digest_value_excluding};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::{StoreDirs, open_store_dirs};
use crate::intent::read_intent;
use crate::layout::{
    ACTIVES, CLEARANCE, DayKey, TERMINALS, active_name, barrier_name, intent_name, member_name,
    terminal_name,
};
#[cfg(test)]
use crate::lock::acquire_days_with_timeout;
use crate::lock::{DayLockSet, LOCK_TIMEOUT, hold_topology_with_timeout};
use crate::permit::{TerminalOutcome, TerminalReceipt, outcome_name, parse_outcome};
#[cfg(test)]
use crate::preflight::Admitted;
use crate::publish::inspect_against_proposed;
use crate::schema::{
    ClearanceBarrier, ClearanceMember, Head, Intent, ROLE_CLEARANCE_BARRIER, ROLE_CLEARANCE_MEMBER,
    ROLE_TERMINAL, ResolvedDay, RevisionWitness, SCHEMA_VERSION, Terminal, read_json,
    write_json_exclusive,
};
use crate::store::{ConvergenceStore, LoadDay};
use crate::transaction::HeldDays;
use crate::walk::{open_dir, unlink_bound};

pub(crate) enum DayClass {
    ExactProposed(ResolvedDay),
    SafeDescendant(ResolvedDay),
    Unresolved,
}

pub(crate) fn classify_day(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    day: &DayKey,
    intent: &Intent,
) -> Result<DayClass, ConvergenceError> {
    let proposed = *intent
        .proposed_day_revisions
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?;
    let expected_dirty = *intent
        .proposed_dirty_generations
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::ChangedProjection))?;
    match inspect_against_proposed(store, locks, day, proposed)? {
        LoadDay::Published(snapshot) if snapshot.record_revision == proposed => {
            if snapshot.dirty_by_transition_serial != intent.serial
                || snapshot.dirty_generation != expected_dirty
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                });
            }
            Ok(DayClass::ExactProposed(load_resolved_day(
                store, locks, day,
            )?))
        }
        LoadDay::HeadedDescendant { .. } => Ok(DayClass::SafeDescendant(load_resolved_day(
            store, locks, day,
        )?)),
        LoadDay::Published(_) | LoadDay::Genesis | LoadDay::PublicationPending { .. } => {
            Ok(DayClass::Unresolved)
        }
    }
}

pub(crate) fn classify_vector(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    intent: &Intent,
    days: &[DayKey],
) -> Result<(BTreeMap<String, ResolvedDay>, bool, bool), ConvergenceError> {
    let mut resolved = BTreeMap::new();
    let mut any_descendant = false;
    let mut any_unresolved = false;
    for day in days {
        match classify_day(store, locks, day, intent)? {
            DayClass::ExactProposed(slice) => {
                resolved.insert(day.as_str().to_owned(), slice);
            }
            DayClass::SafeDescendant(slice) => {
                any_descendant = true;
                resolved.insert(day.as_str().to_owned(), slice);
            }
            DayClass::Unresolved => any_unresolved = true,
        }
    }
    Ok((resolved, any_descendant, any_unresolved))
}

fn load_resolved_day(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    day: &DayKey,
) -> Result<ResolvedDay, ConvergenceError> {
    locks.matches(store.journal_id(), store.root_id(), store.object_identity())?;
    let dirs =
        open_store_dirs(store.root())?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let head = read_json::<Head>(
        &dirs.days,
        &crate::layout::head_name(day),
        DurableRole::Head,
    )?
    .ok_or(ConvergenceError::Unknown {
        role: DurableRole::Head,
    })?;
    let witness = read_json::<RevisionWitness>(
        &dirs.days,
        &crate::layout::revision_witness_name(day, head.record_revision),
        DurableRole::RevisionWitness,
    )?
    .ok_or(ConvergenceError::Unknown {
        role: DurableRole::RevisionWitness,
    })?;
    let snapshot = match store.load_day(locks, day)? {
        LoadDay::Published(snapshot) => snapshot,
        _ => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Record,
            });
        }
    };
    Ok(ResolvedDay {
        record_revision: snapshot.record_revision,
        head_digest: digest_value(&head)?.as_hex().to_owned(),
        witness_digest: digest_value(&witness)?.as_hex().to_owned(),
        record_digest: snapshot.digest.as_hex().to_owned(),
    })
}

pub(crate) fn publish_from_permit(
    held: &mut HeldDays<'_>,
    outcome: TerminalOutcome,
) -> Result<TerminalReceipt, ConvergenceError> {
    if matches!(
        outcome,
        TerminalOutcome::Rejected | TerminalOutcome::Superseded
    ) {
        if outcome == TerminalOutcome::Rejected {
            return Err(ConvergenceError::Refused(Refusal::GenericRejection));
        }
        return Err(ConvergenceError::Refused(Refusal::Superseded));
    }
    let store = &held.admitted.store;
    store.revalidate()?;
    held.locks
        .matches(store.journal_id(), store.root_id(), store.object_identity())?;
    let dirs =
        open_store_dirs(store.root())?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let serial = held
        .serial
        .ok_or(ConvergenceError::Refused(Refusal::NoPermit))?;
    let intent = read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    if Some(intent.intent_digest.as_str()) != held.intent_digest.as_deref() {
        return Err(ConvergenceError::Refused(Refusal::IntentMismatch));
    }
    let days = held.days.clone();
    let existing = read_terminal(&dirs, serial)?;
    let exact_visible = existing.as_ref().is_some_and(|terminal| {
        terminal.outcome == outcome_name(outcome) && terminal.intent_digest == intent.intent_digest
    });
    let existing_resolved = existing.as_ref().map(|terminal| terminal.resolved.clone());
    let mut resolved = BTreeMap::new();
    let mut saw_descendant = false;
    for day in &days {
        match classify_day(store, &held.locks, day, &intent)? {
            DayClass::ExactProposed(slice) => {
                resolved.insert(day.as_str().to_owned(), slice);
            }
            DayClass::SafeDescendant(slice) => {
                if !exact_visible {
                    return Err(ConvergenceError::Refused(Refusal::Superseded));
                }
                saw_descendant = true;
                resolved.insert(day.as_str().to_owned(), slice);
            }
            DayClass::Unresolved => {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                });
            }
        }
    }
    if exact_visible && !saw_descendant && existing_resolved.as_ref() != Some(&resolved) {
        return Err(ConvergenceError::Refused(Refusal::ConflictingTerminal));
    }
    let resolved = if exact_visible && saw_descendant {
        existing_resolved.expect("exact terminal")
    } else {
        resolved
    };
    write_and_cleanup(store, &held.locks, &dirs, &intent, &days, resolved, outcome)
}

pub(crate) fn publish_no_permit_superseded(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
) -> Result<Option<TerminalReceipt>, ConvergenceError> {
    let (resolved, any_descendant, any_unresolved) =
        match classify_vector(store, locks, intent, days) {
            Ok(vector) => vector,
            Err(_) => return Ok(None),
        };
    if !any_descendant || any_unresolved || resolved.len() != days.len() {
        return Ok(None);
    }
    write_and_cleanup(
        store,
        locks,
        dirs,
        intent,
        days,
        resolved,
        TerminalOutcome::Superseded,
    )
    .map(Some)
}

#[cfg(test)]
pub(crate) fn bind_successor_identity(
    admitted: &Admitted,
) -> Result<(u64, String), ConvergenceError> {
    admitted.store.revalidate()?;
    let dirs = open_store_dirs(admitted.store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let table = crate::claim::current_table(&admitted.store, &dirs)?;
    let serial = crate::claim::shared_serial(&table, admitted.days())
        .ok_or(ConvergenceError::Refused(Refusal::NoPermit))?;
    let intent = read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    Ok((serial, intent.intent_digest))
}

#[cfg(test)]
pub(crate) fn publish_from_successor(
    admitted: &Admitted,
    serial: u64,
    intent_digest: &str,
    outcome: TerminalOutcome,
) -> Result<TerminalReceipt, ConvergenceError> {
    if !matches!(
        outcome,
        TerminalOutcome::Committed | TerminalOutcome::Aborted
    ) {
        return Err(ConvergenceError::Refused(Refusal::WrongOutcome));
    }
    publish_from_admitted(admitted, serial, intent_digest, outcome)
}

#[cfg(test)]
pub(crate) fn publish_from_named_refusal(
    admitted: &Admitted,
    serial: u64,
    intent_digest: &str,
) -> Result<TerminalReceipt, ConvergenceError> {
    publish_from_admitted(admitted, serial, intent_digest, TerminalOutcome::Rejected)
}

#[cfg(test)]
pub(crate) fn attempt_generic_rejection() -> Result<TerminalReceipt, ConvergenceError> {
    Err(ConvergenceError::Refused(Refusal::GenericRejection))
}

#[cfg(test)]
fn publish_from_admitted(
    admitted: &Admitted,
    serial: u64,
    intent_digest: &str,
    outcome: TerminalOutcome,
) -> Result<TerminalReceipt, ConvergenceError> {
    admitted.store.revalidate()?;
    let dirs = open_store_dirs(admitted.store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let intent = read_intent(&dirs, serial)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    if intent.intent_digest != intent_digest {
        return Err(ConvergenceError::Refused(Refusal::IntentMismatch));
    }
    let days: Vec<DayKey> = intent
        .day_set
        .iter()
        .map(|day| DayKey::parse(day))
        .collect::<Result<_, _>>()?;
    let locks = acquire_days_with_timeout(
        &dirs,
        &days,
        admitted.store.journal_id(),
        admitted.store.root_id(),
        admitted.store.object_identity(),
        admitted.lock_timeout(),
    )?;
    let mut resolved = BTreeMap::new();
    for day in &days {
        match classify_day(&admitted.store, &locks, day, &intent)? {
            DayClass::ExactProposed(slice) | DayClass::SafeDescendant(slice) => {
                resolved.insert(day.as_str().to_owned(), slice);
            }
            DayClass::Unresolved => {
                drop(locks);
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                });
            }
        }
    }
    let receipt = write_and_cleanup(
        &admitted.store,
        &locks,
        &dirs,
        &intent,
        &days,
        resolved,
        outcome,
    );
    drop(locks);
    receipt
}

fn write_and_cleanup(
    store: &ConvergenceStore,
    _locks: &DayLockSet,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
    resolved: BTreeMap<String, ResolvedDay>,
    outcome: TerminalOutcome,
) -> Result<TerminalReceipt, ConvergenceError> {
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterTerminalPrepub,
    ) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort before terminal create"),
        });
    }
    let mut adoption_ids = BTreeMap::new();
    for day in days {
        let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        })?;
        adoption_ids.insert(day.as_str().to_owned(), adoption.adoption_id);
    }
    let mut terminal = Terminal {
        role: ROLE_TERMINAL.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        serial: intent.serial,
        owner_binding_digest: intent.owner_binding_digest.clone(),
        intent_digest: intent.intent_digest.clone(),
        day_set: intent.day_set.clone(),
        adoption_ids,
        outcome: outcome_name(outcome).to_owned(),
        predecessors: crate::clearance::authenticated_terminal_predecessors(
            store, dirs, intent, days,
        )?,
        resolved,
        terminal_digest: String::new(),
    };
    terminal.terminal_digest = digest_value_excluding(&terminal, "terminal_digest")?
        .as_hex()
        .to_owned();
    let digest = publish_terminal(dirs, &terminal)?;
    continue_cleanup(store, dirs, intent, days, &terminal, &digest)?;
    Ok(TerminalReceipt {
        serial: intent.serial,
        outcome,
        digest,
    })
}

fn ensure_named_dir(dirs: &StoreDirs, name: &str) -> Result<OwnedFd, ConvergenceError> {
    create_directory_bound(&dirs.convergence, std::ffi::OsStr::new(name), 0o700).map_err(
        |error| ConvergenceError::Io {
            operation: "create named directory",
            role: DurableRole::Directory,
            source: std::io::Error::other(error.to_string()),
        },
    )?;
    open_dir(&dirs.convergence, name)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Directory,
    })
}

fn publish_terminal(
    dirs: &StoreDirs,
    terminal: &Terminal,
) -> Result<RecordDigest, ConvergenceError> {
    let parent = ensure_named_dir(dirs, TERMINALS)?;
    let name = terminal_name(terminal.serial);
    match write_json_exclusive(&parent, &name, terminal, DurableRole::Terminal) {
        Ok(digest) => {
            #[cfg(test)]
            if crate::test_support::take_publish_fault(
                crate::test_support::PublishFault::AfterTerminalSync,
            ) || crate::test_support::take_fail_dir_sync()
            {
                return reread_terminal(&parent, terminal, digest);
            }
            if let Err(source) = sync_dir_bound(&parent) {
                match reread_terminal(&parent, terminal, digest) {
                    Ok(digest) => return Ok(digest),
                    Err(_) => {
                        return Err(ConvergenceError::PreservedPrior {
                            operation: "sync terminals directory",
                            source,
                        });
                    }
                }
            }
            Ok(digest)
        }
        Err(ConvergenceError::PreservedPrior { .. }) => reread_existing(&parent, terminal),
        Err(error) => Err(error),
    }
}

fn reread_terminal(
    parent: &OwnedFd,
    expected: &Terminal,
    digest: RecordDigest,
) -> Result<RecordDigest, ConvergenceError> {
    match read_json::<Terminal>(
        parent,
        &terminal_name(expected.serial),
        DurableRole::Terminal,
    )? {
        None => Err(ConvergenceError::PreservedPrior {
            operation: "terminal re-read after uncertainty",
            source: std::io::Error::other("terminal absent after uncertain create"),
        }),
        Some(existing) => classify_existing(&existing, expected, digest),
    }
}

fn reread_existing(
    parent: &OwnedFd,
    expected: &Terminal,
) -> Result<RecordDigest, ConvergenceError> {
    let existing = read_json::<Terminal>(
        parent,
        &terminal_name(expected.serial),
        DurableRole::Terminal,
    )?
    .ok_or(ConvergenceError::PreservedPrior {
        operation: "exclusive create",
        source: std::io::Error::other("terminal absent after exclusive-create failure"),
    })?;
    let digest = digest_value(&existing)?;
    classify_existing(&existing, expected, digest)
}

fn classify_existing(
    existing: &Terminal,
    expected: &Terminal,
    digest: RecordDigest,
) -> Result<RecordDigest, ConvergenceError> {
    if existing.journal_id != expected.journal_id || existing.root_id != expected.root_id {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Terminal,
        });
    }
    if existing.outcome == expected.outcome
        && existing.intent_digest == expected.intent_digest
        && existing.resolved == expected.resolved
        && existing.predecessors == expected.predecessors
    {
        return Ok(digest);
    }
    if let (Some(got), Some(want)) = (
        parse_outcome(&existing.outcome),
        parse_outcome(&expected.outcome),
    ) && matches!(
        (got, want),
        (TerminalOutcome::Committed, TerminalOutcome::Aborted)
            | (TerminalOutcome::Aborted, TerminalOutcome::Committed)
    ) {
        return Err(ConvergenceError::Refused(Refusal::OppositeTerminal));
    }
    Err(ConvergenceError::Refused(Refusal::ConflictingTerminal))
}

fn continue_cleanup(
    store: &crate::store::ConvergenceStore,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
    terminal: &Terminal,
    terminal_digest: &RecordDigest,
) -> Result<(), ConvergenceError> {
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterTerminal) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after terminal"),
        });
    }
    if let Some(actives) = open_dir(&dirs.convergence, ACTIVES)? {
        unlink_bound(&actives, &active_name(intent.serial), DurableRole::Active)?;
    }
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterActiveClear)
    {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after active clear"),
        });
    }
    if let Some(intents) = crate::intent::open_intents_dir(dirs)? {
        unlink_bound(&intents, &intent_name(intent.serial), DurableRole::Intent)?;
    }
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterIntentClear)
    {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after intent clear"),
        });
    }
    let mut member_digests = BTreeMap::new();
    for (index, day) in days.iter().enumerate() {
        let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        })?;
        let resolved = terminal
            .resolved
            .get(day.as_str())
            .ok_or(ConvergenceError::Refused(Refusal::IncompleteEvidence))?
            .clone();
        let predecessor = terminal
            .predecessors
            .get(day.as_str())
            .ok_or(ConvergenceError::Refused(Refusal::IncompleteEvidence))?
            .clone();
        let member = ClearanceMember {
            role: ROLE_CLEARANCE_MEMBER.to_owned(),
            schema_version: SCHEMA_VERSION,
            journal_id: terminal.journal_id.clone(),
            root_id: terminal.root_id.clone(),
            adoption_id: adoption.adoption_id,
            day: day.as_str().to_owned(),
            serial: terminal.serial,
            outcome: terminal.outcome.clone(),
            terminal_digest: terminal_digest.as_hex().to_owned(),
            resolved,
            predecessor_consumption: predecessor,
        };
        let digest = write_member(dirs, day, &member)?;
        member_digests.insert(day.as_str().to_owned(), digest.as_hex().to_owned());
        abort_after_member(index)?;
    }
    let barrier = ClearanceBarrier {
        role: ROLE_CLEARANCE_BARRIER.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: terminal.journal_id.clone(),
        root_id: terminal.root_id.clone(),
        serial: terminal.serial,
        terminal_digest: terminal_digest.as_hex().to_owned(),
        day_set: terminal.day_set.clone(),
        member_digests,
        resolved: terminal.resolved.clone(),
    };
    write_barrier(dirs, &barrier)?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterBarrier) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after barrier"),
        });
    }
    let terminals = ensure_named_dir(dirs, TERMINALS)?;
    unlink_bound(
        &terminals,
        &terminal_name(terminal.serial),
        DurableRole::Terminal,
    )?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterTerminalEvict,
    ) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after terminal eviction"),
        });
    }
    release_after_evict(store, dirs, intent, days)?;
    Ok(())
}

fn release_after_evict(
    store: &crate::store::ConvergenceStore,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
) -> Result<(), ConvergenceError> {
    let _topology = hold_topology_with_timeout(dirs, LOCK_TIMEOUT)?;
    let view = crate::claim::mechanical_finalize(store, dirs)?;
    let prior = match view {
        crate::claim::ClaimView::Headed(body) | crate::claim::ClaimView::Unheaded(body) => body,
        crate::claim::ClaimView::Empty => return Ok(()),
    };
    if !prior
        .table
        .values()
        .any(|entry| entry.serial == intent.serial)
    {
        return Ok(());
    }
    let body = crate::claim::release(store, dirs, &prior, intent.serial, days)?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterReleaseRevision,
    ) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after release revision"),
        });
    }
    crate::claim::write_head(store, dirs, &body)?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterReleaseHead)
    {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after release head"),
        });
    }
    Ok(())
}

/// Read: the retained per-day clearance member, or `None`. Never creates.
pub(crate) fn read_clearance_member(
    dirs: &StoreDirs,
    day: &DayKey,
) -> Result<Option<ClearanceMember>, ConvergenceError> {
    read_json(&dirs.days, &member_name(day), DurableRole::ClearanceMember)
}

/// Read: the retained clearance barrier for `serial`, or `None`. Never creates.
pub(crate) fn read_clearance_barrier(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<ClearanceBarrier>, ConvergenceError> {
    let Some(parent) = crate::walk::open_dir(&dirs.convergence, CLEARANCE)? else {
        return Ok(None);
    };
    read_json(
        &parent,
        &barrier_name(serial),
        DurableRole::ClearanceBarrier,
    )
}

fn write_member(
    dirs: &StoreDirs,
    day: &DayKey,
    member: &ClearanceMember,
) -> Result<RecordDigest, ConvergenceError> {
    match write_json_exclusive(
        &dirs.days,
        &member_name(day),
        member,
        DurableRole::ClearanceMember,
    ) {
        Ok(digest) => Ok(digest),
        Err(ConvergenceError::PreservedPrior { .. }) => {
            let existing = read_json::<ClearanceMember>(
                &dirs.days,
                &member_name(day),
                DurableRole::ClearanceMember,
            )?
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::ClearanceMember,
            })?;
            if existing != *member {
                return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
            }
            digest_value(&existing)
        }
        Err(error) => Err(error),
    }
}

fn write_barrier(
    dirs: &StoreDirs,
    barrier: &ClearanceBarrier,
) -> Result<RecordDigest, ConvergenceError> {
    let parent = ensure_named_dir(dirs, CLEARANCE)?;
    match write_json_exclusive(
        &parent,
        &barrier_name(barrier.serial),
        barrier,
        DurableRole::ClearanceBarrier,
    ) {
        Ok(digest) => Ok(digest),
        Err(ConvergenceError::PreservedPrior { .. }) => {
            let existing = read_json::<ClearanceBarrier>(
                &parent,
                &barrier_name(barrier.serial),
                DurableRole::ClearanceBarrier,
            )?
            .ok_or(ConvergenceError::Unknown {
                role: DurableRole::ClearanceBarrier,
            })?;
            if existing != *barrier {
                return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
            }
            digest_value(&existing)
        }
        Err(error) => Err(error),
    }
}

fn abort_after_member(index: usize) -> Result<(), ConvergenceError> {
    #[cfg(test)]
    {
        let fault = match index {
            0 => crate::test_support::PublishFault::AfterMemberA,
            1 => crate::test_support::PublishFault::AfterMemberB,
            _ => return Ok(()),
        };
        if crate::test_support::take_publish_fault(fault) {
            return Err(ConvergenceError::PreservedPrior {
                operation: "injected abort",
                source: std::io::Error::other("test abort after clearance member"),
            });
        }
    }
    #[cfg(not(test))]
    let _ = index;
    Ok(())
}

pub(crate) fn read_terminal(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<Terminal>, ConvergenceError> {
    let Some(parent) = open_dir(&dirs.convergence, TERMINALS)? else {
        return Ok(None);
    };
    read_json(&parent, &terminal_name(serial), DurableRole::Terminal)
}

/// Accept only the exact terminal rooted in an immutable owner-intent link.
/// Parsing alone is not authority: all identity, membership, and digest
/// fields are revalidated at delivery/authorization boundaries.
pub(crate) fn accept_terminal(
    terminal: Terminal,
    link: &crate::schema::OwnerIntentLink,
) -> Result<(Terminal, RecordDigest), ConvergenceError> {
    let expected_days = &link.day_set;
    if terminal.role != ROLE_TERMINAL
        || terminal.schema_version != SCHEMA_VERSION
        || terminal.journal_id != link.journal_id
        || terminal.root_id != link.root_id
        || terminal.serial != link.serial
        || terminal.owner_binding_digest != link.owner_binding_digest
        || terminal.intent_digest != link.intent_digest
        || terminal.day_set != *expected_days
        || terminal.resolved.len() != expected_days.len()
        || terminal.adoption_ids.len() != expected_days.len()
        || expected_days
            .iter()
            .any(|day| !terminal.resolved.contains_key(day))
        || expected_days
            .iter()
            .any(|day| !terminal.adoption_ids.contains_key(day))
        || terminal.resolved.values().any(|resolved| {
            resolved.record_revision == 0
                || resolved.head_digest.is_empty()
                || resolved.witness_digest.is_empty()
                || resolved.record_digest.is_empty()
        })
        || terminal.adoption_ids.values().any(String::is_empty)
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Terminal,
        });
    }
    let expected = digest_value_excluding(&terminal, "terminal_digest")?
        .as_hex()
        .to_owned();
    if terminal.terminal_digest != expected {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Terminal,
        });
    }
    let digest = digest_value(&terminal)?;
    Ok((terminal, digest))
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::digest::digest_value_excluding;
    use crate::schema::{OwnerIntentLink, ROLE_OWNER_INTENT_LINK};
    use crate::test_support::{TempDir, admit_days, continue_ok};

    fn exact_terminal() -> (TempDir, Terminal, OwnerIntentLink) {
        let (temporary, admitted) = admit_days("terminal-accept", &["20260823"]);
        let held = continue_ok(&admitted);
        let serial = held.serial.unwrap();
        let dirs = open_store_dirs(admitted.store().root()).unwrap().unwrap();
        let intent = read_intent(&dirs, serial).unwrap().unwrap();
        let link = OwnerIntentLink {
            role: ROLE_OWNER_INTENT_LINK.to_owned(),
            schema_version: SCHEMA_VERSION,
            journal_id: held.owner().journal_id().to_owned(),
            root_id: held.owner().root_id().to_owned(),
            operation_id: held.owner().operation_id().to_owned(),
            owner_binding_digest: held.owner().digest_hex().to_owned(),
            serial,
            intent_digest: intent.intent_digest,
            day_set: intent.day_set,
            day_set_subdigest: intent.day_set_subdigest,
            selector_digest: held.owner().selector_digest().to_owned(),
        };
        let mut resolved = BTreeMap::new();
        resolved.insert(
            "20260823".to_owned(),
            ResolvedDay {
                record_revision: 1,
                head_digest: "11".repeat(32),
                witness_digest: "22".repeat(32),
                record_digest: "33".repeat(32),
            },
        );
        let mut adoption_ids = BTreeMap::new();
        adoption_ids.insert("20260823".to_owned(), "adoption".to_owned());
        let mut terminal = Terminal {
            role: ROLE_TERMINAL.to_owned(),
            schema_version: SCHEMA_VERSION,
            journal_id: link.journal_id.clone(),
            root_id: link.root_id.clone(),
            serial,
            owner_binding_digest: link.owner_binding_digest.clone(),
            intent_digest: link.intent_digest.clone(),
            day_set: link.day_set.clone(),
            adoption_ids,
            outcome: outcome_name(TerminalOutcome::Committed).to_owned(),
            predecessors: BTreeMap::new(),
            resolved,
            terminal_digest: String::new(),
        };
        terminal.terminal_digest = digest_value_excluding(&terminal, "terminal_digest")
            .unwrap()
            .as_hex()
            .to_owned();
        (temporary, terminal, link)
    }

    fn assert_terminal_unknown(mutate: impl FnOnce(&mut Terminal)) {
        let (_temporary, mut terminal, link) = exact_terminal();
        mutate(&mut terminal);
        assert!(matches!(
            accept_terminal(terminal, &link),
            Err(ConvergenceError::Unknown {
                role: DurableRole::Terminal
            })
        ));
    }

    #[test]
    fn accept_terminal_accepts_exact_record() {
        let (_temporary, terminal, link) = exact_terminal();
        assert_eq!(
            accept_terminal(terminal.clone(), &link).unwrap().0,
            terminal
        );
    }

    #[test]
    fn accept_terminal_rejects_wrong_role() {
        assert_terminal_unknown(|terminal| terminal.role = "wrong".to_owned());
    }
    #[test]
    fn accept_terminal_rejects_wrong_version() {
        assert_terminal_unknown(|terminal| terminal.schema_version += 1);
    }
    #[test]
    fn accept_terminal_rejects_wrong_journal() {
        assert_terminal_unknown(|terminal| terminal.journal_id = "wrong".to_owned());
    }
    #[test]
    fn accept_terminal_rejects_wrong_root() {
        assert_terminal_unknown(|terminal| terminal.root_id = "wrong".to_owned());
    }
    #[test]
    fn accept_terminal_rejects_wrong_serial() {
        assert_terminal_unknown(|terminal| terminal.serial += 1);
    }
    #[test]
    fn accept_terminal_rejects_wrong_intent() {
        assert_terminal_unknown(|terminal| terminal.intent_digest = "00".repeat(32));
    }
    #[test]
    fn accept_terminal_rejects_wrong_day_set() {
        assert_terminal_unknown(|terminal| terminal.day_set = vec!["20260824".to_owned()]);
    }
    #[test]
    fn accept_terminal_rejects_missing_resolved_member() {
        assert_terminal_unknown(|terminal| {
            terminal.resolved.clear();
        });
    }
    #[test]
    fn accept_terminal_rejects_digest_mismatch() {
        assert_terminal_unknown(|terminal| terminal.terminal_digest = "00".repeat(32));
    }
}
