// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;

use solstone_core_journal_io::{BoundAtomicOutcome, create_directory_bound};

use crate::allocate::load_adoption;
use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::{StoreDirs, load_allocator, open_store_dirs};
use crate::layout::{DayKey, ever_name, head_name, record_file_name, revision_witness_name};
use crate::lock::DayLockSet;
use crate::schema::{
    DayRecord, EverWitness, Head, RevisionWitness, SCHEMA_VERSION, read_json, record_digest,
    replace_json, require_day, require_ids, validate_record_numbers, write_json_exclusive,
};
use crate::store::{ConvergenceStore, LoadDay, PendingKind, snapshot_from_record};
use crate::walk::open_dir;

mod sealed {
    use crate::error::ConvergenceError;
    use crate::schema::DayRecord;

    #[allow(dead_code)]
    pub trait PublicationKind {
        fn next_record(&self, current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError>;
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum PublishOutcome {
    Published {
        day: DayKey,
        record_revision: u64,
        first_transition_serial: u64,
        dirty_by_transition_serial: u64,
        digest: RecordDigest,
    },
    PublishedDurabilityUncertain {
        day: DayKey,
        record_revision: u64,
        first_transition_serial: u64,
        dirty_by_transition_serial: u64,
        digest: RecordDigest,
        source: std::io::Error,
    },
}

fn unheaded_witness(
    dirs: &StoreDirs,
    day: &DayKey,
    ever: &EverWitness,
) -> Result<LoadDay, ConvergenceError> {
    let Some(witness) = probe_extra_revision_witness(dirs, day, 1)? else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::EverWitness,
        });
    };
    let ever_digest = digest_value(ever)?;
    if witness.prior_witness_digest == ever_digest.as_hex() && witness.record_revision == 1 {
        return Ok(LoadDay::PublicationPending {
            kind: PendingKind::WitnessAheadOfHead,
        });
    }
    Err(ConvergenceError::Unknown {
        role: DurableRole::RevisionWitness,
    })
}

fn probe_extra_revision_witness(
    dirs: &StoreDirs,
    day: &DayKey,
    revision: u64,
) -> Result<Option<RevisionWitness>, ConvergenceError> {
    let name = revision_witness_name(day, revision);
    let name_str = name.to_string_lossy();
    match crate::walk::open_file(&dirs.days, name_str.as_ref()) {
        Ok(None) => Ok(None),
        Err(ConvergenceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::InvalidInput =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
        Ok(Some(_)) => read_json(&dirs.days, &name, DurableRole::RevisionWitness),
    }
}

fn load_record(dirs: &StoreDirs, day: &DayKey) -> Result<Option<DayRecord>, ConvergenceError> {
    let Some(records_day) = crate::walk::open_dir(&dirs.records, day.as_str())? else {
        return Ok(None);
    };
    read_json(
        &records_day,
        OsStr::new(record_file_name()),
        DurableRole::Record,
    )
}

pub(crate) fn inspect_day(
    store: &ConvergenceStore,
    day: &DayKey,
) -> Result<LoadDay, ConvergenceError> {
    let dirs =
        open_store_dirs(store.root())?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let allocator = load_allocator(&dirs)?;
    require_ids(
        store.journal_id(),
        store.root_id(),
        &allocator.journal_id,
        &allocator.root_id,
    )?;
    let Some(adoption) = load_adoption(&dirs, day)? else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        });
    };
    require_ids(
        store.journal_id(),
        store.root_id(),
        &adoption.journal_id,
        &adoption.root_id,
    )?;
    require_day(day, &adoption.day)?;
    let ever = read_json::<EverWitness>(&dirs.days, &ever_name(day), DurableRole::EverWitness)?;
    let head = read_json::<Head>(&dirs.days, &head_name(day), DurableRole::Head)?;
    let record = load_record(&dirs, day)?;
    match (ever.as_ref(), head.as_ref(), record.as_ref()) {
        (None, None, None) => {
            if probe_extra_revision_witness(&dirs, day, 1)?.is_some() {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::EverWitness,
                });
            }
            return Ok(LoadDay::Genesis);
        }
        (Some(ever), None, None) => {
            return unheaded_witness(&dirs, day, ever);
        }
        (Some(_), None, Some(_)) | (None, Some(_), _) | (None, None, Some(_)) => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::EverWitness,
            });
        }
        (Some(_), Some(_), _) => {}
    }
    let Some(head) = head else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        });
    };
    let Some(ever) = ever else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::EverWitness,
        });
    };
    if head.schema_version != SCHEMA_VERSION || ever.schema_version != SCHEMA_VERSION {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        });
    }
    require_day(day, &head.day)?;
    let height = head.record_revision;
    if height == 0 {
        return Err(ConvergenceError::Refused(Refusal::PersistedZeroRevision));
    }
    let mut prior_digest = digest_value(&ever)?.as_hex().to_owned();
    let mut tail = None;
    for revision in 1..=height {
        let Some(witness) = read_json::<RevisionWitness>(
            &dirs.days,
            &revision_witness_name(day, revision),
            DurableRole::RevisionWitness,
        )?
        else {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        };
        if witness.prior_witness_digest != prior_digest || witness.record_revision != revision {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        }
        prior_digest = digest_value(&witness)?.as_hex().to_owned();
        tail = Some(witness);
    }
    let Some(tail) = tail else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::RevisionWitness,
        });
    };
    let tail_digest = digest_value(&tail)?;
    if tail_digest.as_hex() != head.witness_digest {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        });
    }
    if let Some(pending) = classify_higher_witnesses(&dirs, day, height, tail_digest.as_hex())? {
        return Ok(pending);
    }
    let Some(record) = record else {
        if height == 1 {
            return Ok(LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            });
        }
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Record,
        });
    };
    validate_record_numbers(&record)?;
    require_day(day, &record.day)?;
    require_ids(
        store.journal_id(),
        store.root_id(),
        &record.journal_id,
        &record.root_id,
    )?;
    if record.dirty_by_transition_serial >= allocator.next_serial
        || record.first_transition_serial >= allocator.next_serial
    {
        return Err(ConvergenceError::Refused(Refusal::FutureSerial {
            observed: record.dirty_by_transition_serial,
            next: allocator.next_serial,
        }));
    }
    let digest = record_digest(&record)?;
    if digest.as_hex() == head.record_digest && record.record_revision == head.record_revision {
        return Ok(LoadDay::Published(snapshot_from_record(&record)?));
    }
    if height >= 2 && record.record_revision + 1 == height {
        let Some(previous) = read_json::<RevisionWitness>(
            &dirs.days,
            &revision_witness_name(day, height - 1),
            DurableRole::RevisionWitness,
        )?
        else {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        };
        if digest.as_hex() == previous.record_digest {
            return Ok(LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            });
        }
    }
    Err(ConvergenceError::Unknown {
        role: DurableRole::Record,
    })
}

/// Classify current store state against an intent's proposed revision (AC4).
pub(crate) fn inspect_against_proposed(
    store: &ConvergenceStore,
    days: &DayLockSet,
    day: &DayKey,
    proposed_revision: u64,
) -> Result<LoadDay, ConvergenceError> {
    if proposed_revision == 0 {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        });
    }
    match store.load_day(days, day)? {
        LoadDay::Published(snapshot) if snapshot.record_revision == proposed_revision => {
            Ok(LoadDay::Published(snapshot))
        }
        LoadDay::Published(snapshot) if snapshot.record_revision + 1 == proposed_revision => {
            Ok(LoadDay::Published(snapshot))
        }
        LoadDay::Published(snapshot) if snapshot.record_revision > proposed_revision => {
            Ok(LoadDay::HeadedDescendant {
                head_revision: snapshot.record_revision,
                proposed_revision,
            })
        }
        LoadDay::Genesis if proposed_revision == 1 => Ok(LoadDay::Genesis),
        LoadDay::PublicationPending {
            kind: PendingKind::WitnessAheadOfHead,
        } => {
            // Unheaded higher witness is not supersession (AC4 / 10.149).
            Ok(LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            })
        }
        other => Ok(other),
    }
}

fn classify_higher_witnesses(
    dirs: &StoreDirs,
    day: &DayKey,
    height: u64,
    tail_digest: &str,
) -> Result<Option<LoadDay>, ConvergenceError> {
    let Some(next) = probe_extra_revision_witness(dirs, day, height + 1)? else {
        if probe_extra_revision_witness(dirs, day, height + 2)?.is_some() {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        }
        return Ok(None);
    };
    if next.prior_witness_digest != tail_digest || next.record_revision != height + 1 {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::RevisionWitness,
        });
    }
    if probe_extra_revision_witness(dirs, day, height + 2)?.is_some() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::RevisionWitness,
        });
    }
    Ok(Some(LoadDay::PublicationPending {
        kind: PendingKind::WitnessAheadOfHead,
    }))
}

pub(crate) fn publish_record(
    store: &ConvergenceStore,
    days: &DayLockSet,
    day: &DayKey,
    next: &DayRecord,
    used: Option<&mut bool>,
) -> Result<PublishOutcome, ConvergenceError> {
    store.revalidate()?;
    days.matches(store.journal_id(), store.root_id(), store.object_identity())?;
    validate_record_numbers(next)?;
    require_day(day, &next.day)?;
    let dirs =
        open_store_dirs(store.root())?.ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let adoption = load_adoption(&dirs, day)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Adoption,
    })?;
    if next.adoption_id != adoption.adoption_id {
        return Err(ConvergenceError::Refused(Refusal::WrongLineage));
    }
    let current = match inspect_day(store, day)? {
        LoadDay::Genesis => None,
        LoadDay::Published(_) => load_record(&dirs, day)?,
        LoadDay::PublicationPending { .. } | LoadDay::HeadedDescendant { .. } => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Head,
            });
        }
    };
    let is_g1 = current.is_none();
    if let Some(current) = current.as_ref() {
        if next.record_revision != current.record_revision + 1 {
            return Err(ConvergenceError::Refused(Refusal::RevisionRollback {
                observed: next.record_revision,
                current: current.record_revision,
            }));
        }
        if next.dirty_generation < current.dirty_generation {
            return Err(ConvergenceError::Refused(Refusal::GenerationRollback {
                observed: next.dirty_generation,
                current: current.dirty_generation,
            }));
        }
        if next.first_transition_serial != current.first_transition_serial {
            return Err(ConvergenceError::Refused(Refusal::WrongLineage));
        }
    } else if next.record_revision != 1 {
        return Err(ConvergenceError::Refused(Refusal::RevisionRollback {
            observed: next.record_revision,
            current: 0,
        }));
    }
    let outcome = if is_g1 {
        publish_g1(store, days, &dirs, day, next, used)?
    } else {
        let current = current.as_ref().ok_or(ConvergenceError::Unknown {
            role: DurableRole::Record,
        })?;
        publish_next(store, days, &dirs, day, next, current, used)?
    };
    Ok(outcome)
}

struct Gate<'a> {
    ever: bool,
    head: Option<&'a Head>,
    record_digest: Option<&'a str>,
    chain_height: u64,
    next_witness: Option<u64>,
}

fn publish_gate(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    day: &DayKey,
    expect: Gate<'_>,
) -> Result<(), ConvergenceError> {
    store.revalidate()?;
    if !locks.contains(day) {
        return Err(ConvergenceError::Refused(Refusal::WrongDay {
            expected: day.as_str().to_owned(),
            observed: String::new(),
        }));
    }
    locks.matches(store.journal_id(), store.root_id(), store.object_identity())?;
    let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Adoption,
    })?;
    require_ids(
        store.journal_id(),
        store.root_id(),
        &adoption.journal_id,
        &adoption.root_id,
    )?;
    require_day(day, &adoption.day)?;
    let ever = read_json::<EverWitness>(&dirs.days, &ever_name(day), DurableRole::EverWitness)?;
    match (expect.ever, ever.as_ref()) {
        (true, Some(ever)) => {
            require_ids(
                store.journal_id(),
                store.root_id(),
                &ever.journal_id,
                &ever.root_id,
            )?;
            require_day(day, &ever.day)?;
        }
        (false, None) => {}
        _ => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::EverWitness,
            });
        }
    }
    if expect.chain_height > 0 {
        let ever = ever.as_ref().ok_or(ConvergenceError::Unknown {
            role: DurableRole::EverWitness,
        })?;
        let _ = witness_chain(dirs, day, ever, expect.chain_height)?;
    }
    let head = read_json::<Head>(&dirs.days, &head_name(day), DurableRole::Head)?;
    match (expect.head, head.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(observed)) => {
            if observed.record_revision != expected.record_revision
                || observed.witness_digest != expected.witness_digest
                || observed.record_digest != expected.record_digest
            {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Head,
                });
            }
            require_ids(
                store.journal_id(),
                store.root_id(),
                &observed.journal_id,
                &observed.root_id,
            )?;
        }
        _ => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Head,
            });
        }
    }
    let record = load_record(dirs, day)?;
    match (expect.record_digest, record.as_ref()) {
        (None, None) => {}
        (Some(expected), Some(observed)) => {
            if record_digest(observed)?.as_hex() != expected {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                });
            }
        }
        _ => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Record,
            });
        }
    }
    if let Some(revision) = expect.next_witness {
        if probe_extra_revision_witness(dirs, day, revision)?.is_none() {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        }
    } else if expect.chain_height > 0
        && probe_extra_revision_witness(dirs, day, expect.chain_height + 1)?.is_some()
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::RevisionWitness,
        });
    }
    Ok(())
}

fn witness_chain(
    dirs: &StoreDirs,
    day: &DayKey,
    ever: &EverWitness,
    height: u64,
) -> Result<RevisionWitness, ConvergenceError> {
    let mut prior_digest = digest_value(ever)?.as_hex().to_owned();
    let mut tail = None;
    for revision in 1..=height {
        let Some(witness) = read_json::<RevisionWitness>(
            &dirs.days,
            &revision_witness_name(day, revision),
            DurableRole::RevisionWitness,
        )?
        else {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        };
        if witness.prior_witness_digest != prior_digest || witness.record_revision != revision {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness,
            });
        }
        prior_digest = digest_value(&witness)?.as_hex().to_owned();
        tail = Some(witness);
    }
    tail.ok_or(ConvergenceError::Unknown {
        role: DurableRole::RevisionWitness,
    })
}

#[cfg(test)]
fn injected_abort(step: crate::test_support::PublishFault) -> Result<(), ConvergenceError> {
    if crate::test_support::take_publish_fault(step) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after publication step"),
        });
    }
    Ok(())
}

fn publish_g1(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    day: &DayKey,
    next: &DayRecord,
    mut used: Option<&mut bool>,
) -> Result<PublishOutcome, ConvergenceError> {
    let record_hex = record_digest(next)?.as_hex().to_owned();
    let record_hex = record_hex.as_str();
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: false,
            head: None,
            record_digest: None,
            chain_height: 0,
            next_witness: None,
        },
    )?;
    let ever = EverWitness {
        schema_version: SCHEMA_VERSION,
        journal_id: next.journal_id.clone(),
        root_id: next.root_id.clone(),
        adoption_id: next.adoption_id.clone(),
        day: next.day.clone(),
        first_transition_serial: next.first_transition_serial,
        dirty_generation: 1,
        completed_generation: 0,
        record_digest: record_hex.to_owned(),
    };
    write_json_exclusive(&dirs.days, &ever_name(day), &ever, DurableRole::EverWitness)?;
    #[cfg(test)]
    injected_abort(crate::test_support::PublishFault::AfterEver)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: None,
            record_digest: None,
            chain_height: 0,
            next_witness: None,
        },
    )?;
    let ever_digest = digest_value(&ever)?;
    let witness = revision_witness(next, record_hex, ever_digest.as_hex());
    write_json_exclusive(
        &dirs.days,
        &revision_witness_name(day, 1),
        &witness,
        DurableRole::RevisionWitness,
    )?;
    if let Some(flag) = used.as_mut() {
        **flag = true;
    }
    #[cfg(test)]
    {
        crate::test_support::run_after_witness_hook();
        injected_abort(crate::test_support::PublishFault::AfterWitness)?;
    }
    let witness_digest = digest_value(&witness)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: None,
            record_digest: None,
            chain_height: 0,
            next_witness: Some(1),
        },
    )?;
    let head = Head {
        schema_version: SCHEMA_VERSION,
        journal_id: next.journal_id.clone(),
        root_id: next.root_id.clone(),
        adoption_id: next.adoption_id.clone(),
        day: next.day.clone(),
        record_revision: 1,
        witness_digest: witness_digest.as_hex().to_owned(),
        record_digest: record_hex.to_owned(),
    };
    let (_, head_outcome) = replace_json(&dirs.days, &head_name(day), &head)?;
    if let Some(source) = uncertain(head_outcome) {
        return Ok(uncertain_outcome(day, next, record_hex, source));
    }
    #[cfg(test)]
    injected_abort(crate::test_support::PublishFault::AfterHead)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: Some(&head),
            record_digest: None,
            chain_height: 1,
            next_witness: None,
        },
    )?;
    write_record_file(dirs, day, next)?;
    #[cfg(test)]
    injected_abort(crate::test_support::PublishFault::AfterRecord)?;
    Ok(published(day, next, record_hex))
}

fn publish_next(
    store: &ConvergenceStore,
    locks: &DayLockSet,
    dirs: &StoreDirs,
    day: &DayKey,
    next: &DayRecord,
    current: &DayRecord,
    mut used: Option<&mut bool>,
) -> Result<PublishOutcome, ConvergenceError> {
    let record_hex = record_digest(next)?.as_hex().to_owned();
    let record_hex = record_hex.as_str();
    let height = current.record_revision;
    let prior_head = read_json::<Head>(&dirs.days, &head_name(day), DurableRole::Head)?.ok_or(
        ConvergenceError::Unknown {
            role: DurableRole::Head,
        },
    )?;
    let prior_digest = record_digest(current)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: Some(&prior_head),
            record_digest: Some(prior_digest.as_hex()),
            chain_height: height,
            next_witness: None,
        },
    )?;
    let prior_witness = read_json::<RevisionWitness>(
        &dirs.days,
        &revision_witness_name(day, height),
        DurableRole::RevisionWitness,
    )?
    .ok_or(ConvergenceError::Unknown {
        role: DurableRole::RevisionWitness,
    })?;
    let prior_witness_digest = digest_value(&prior_witness)?;
    let witness = revision_witness(next, record_hex, prior_witness_digest.as_hex());
    write_json_exclusive(
        &dirs.days,
        &revision_witness_name(day, next.record_revision),
        &witness,
        DurableRole::RevisionWitness,
    )?;
    if let Some(flag) = used.as_mut() {
        **flag = true;
    }
    #[cfg(test)]
    {
        crate::test_support::run_after_witness_hook();
        injected_abort(crate::test_support::PublishFault::AfterWitness)?;
    }
    let witness_digest = digest_value(&witness)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: Some(&prior_head),
            record_digest: Some(prior_digest.as_hex()),
            chain_height: height,
            next_witness: Some(next.record_revision),
        },
    )?;
    let head = Head {
        schema_version: SCHEMA_VERSION,
        journal_id: next.journal_id.clone(),
        root_id: next.root_id.clone(),
        adoption_id: next.adoption_id.clone(),
        day: next.day.clone(),
        record_revision: next.record_revision,
        witness_digest: witness_digest.as_hex().to_owned(),
        record_digest: record_hex.to_owned(),
    };
    let (_, head_outcome) = replace_json(&dirs.days, &head_name(day), &head)?;
    if let Some(source) = uncertain(head_outcome) {
        return Ok(uncertain_outcome(day, next, record_hex, source));
    }
    #[cfg(test)]
    injected_abort(crate::test_support::PublishFault::AfterHead)?;
    publish_gate(
        store,
        locks,
        dirs,
        day,
        Gate {
            ever: true,
            head: Some(&head),
            record_digest: Some(prior_digest.as_hex()),
            chain_height: next.record_revision,
            next_witness: None,
        },
    )?;
    write_record_file(dirs, day, next)?;
    #[cfg(test)]
    injected_abort(crate::test_support::PublishFault::AfterRecord)?;
    Ok(published(day, next, record_hex))
}

fn revision_witness(next: &DayRecord, record_hex: &str, prior: &str) -> RevisionWitness {
    RevisionWitness {
        schema_version: SCHEMA_VERSION,
        journal_id: next.journal_id.clone(),
        root_id: next.root_id.clone(),
        adoption_id: next.adoption_id.clone(),
        day: next.day.clone(),
        record_revision: next.record_revision,
        first_transition_serial: next.first_transition_serial,
        dirty_by_transition_serial: next.dirty_by_transition_serial,
        dirty_generation: next.dirty_generation,
        completed_generation: next.completed_generation,
        record_digest: record_hex.to_owned(),
        prior_witness_digest: prior.to_owned(),
    }
}

fn write_record_file(
    dirs: &StoreDirs,
    day: &DayKey,
    next: &DayRecord,
) -> Result<(), ConvergenceError> {
    create_directory_bound(&dirs.records, OsStr::new(day.as_str()), 0o700).map_err(|error| {
        ConvergenceError::Io {
            operation: "create record day directory",
            role: DurableRole::Record,
            source: std::io::Error::other(error.to_string()),
        }
    })?;
    let record_dir = open_dir(&dirs.records, day.as_str())?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Record,
    })?;
    replace_json(&record_dir, OsStr::new(record_file_name()), next)?;
    Ok(())
}

fn uncertain(outcome: BoundAtomicOutcome) -> Option<std::io::Error> {
    #[cfg(test)]
    if crate::test_support::take_fail_dir_sync() {
        return Some(std::io::Error::other("injected dir sync failure"));
    }
    match outcome {
        BoundAtomicOutcome::Published => None,
        BoundAtomicOutcome::PublishedDurabilityUncertain { source } => Some(source),
    }
}

fn published(day: &DayKey, next: &DayRecord, record_hex: &str) -> PublishOutcome {
    PublishOutcome::Published {
        day: day.clone(),
        record_revision: next.record_revision,
        first_transition_serial: next.first_transition_serial,
        dirty_by_transition_serial: next.dirty_by_transition_serial,
        digest: RecordDigest(record_hex.to_owned()),
    }
}

fn uncertain_outcome(
    day: &DayKey,
    next: &DayRecord,
    record_hex: &str,
    source: std::io::Error,
) -> PublishOutcome {
    PublishOutcome::PublishedDurabilityUncertain {
        day: day.clone(),
        record_revision: next.record_revision,
        first_transition_serial: next.first_transition_serial,
        dirty_by_transition_serial: next.dirty_by_transition_serial,
        digest: RecordDigest(record_hex.to_owned()),
        source,
    }
}

#[cfg(test)]
pub(crate) fn publish_kind_for_test<K: sealed::PublicationKind>(
    store: &ConvergenceStore,
    days: &DayLockSet,
    day: &DayKey,
    kind: K,
) -> Result<PublishOutcome, ConvergenceError> {
    let current = match store.load_day(days, day)? {
        LoadDay::Genesis => None,
        LoadDay::Published(_) => {
            let dirs = open_store_dirs(store.root())?.unwrap();
            load_record(&dirs, day)?
        }
        LoadDay::PublicationPending { .. } | LoadDay::HeadedDescendant { .. } => {
            return Err(ConvergenceError::Unknown {
                role: DurableRole::Head,
            });
        }
    };
    let next = kind.next_record(current.as_ref())?;
    publish_record(store, days, day, &next, None)
}

#[cfg(test)]
pub(crate) struct PreparedCompletionAuthority;

#[cfg(test)]
impl sealed::PublicationKind for PreparedCompletionAuthority {
    fn next_record(&self, current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError> {
        let current = current.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Record,
        })?;
        let mut next = current.clone();
        next.record_revision += 1;
        next.completed_generation = next.dirty_generation;
        Ok(next)
    }
}

#[cfg(test)]
pub(crate) struct MigrationAuthority {
    pub inventory_first_serial: u64,
}

#[cfg(test)]
impl sealed::PublicationKind for MigrationAuthority {
    fn next_record(&self, current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError> {
        let current = current.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Record,
        })?;
        if current.first_transition_serial != self.inventory_first_serial {
            return Err(ConvergenceError::Refused(Refusal::WrongLineage));
        }
        let mut next = current.clone();
        next.record_revision += 1;
        next.completed_generation = next.dirty_generation;
        Ok(next)
    }
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::Refusal;
    use crate::layout::DayKey;
    use crate::store::{LoadDay, PendingKind};
    use crate::test_support::{
        PublishFault, admit_days, after_witness, continue_ok, continue_with_fault, days_dir,
        fail_after_witness, fail_next_dir_sync, records_dir, sample_day,
    };
    use crate::transaction::HeldDays;
    use std::fs;
    use std::path::Path;

    fn published_snapshot(held: &HeldDays<'_>, day: &DayKey) -> crate::store::DaySnapshot {
        match held.inspect_day(day).unwrap() {
            LoadDay::Published(snapshot) => snapshot,
            other => panic!("{other:?}"),
        }
    }

    fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> T {
        let bytes = fs::read(path).unwrap();
        let trimmed = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        serde_json::from_slice(trimmed).unwrap()
    }

    #[test]
    fn g1_creates_ever_before_witness() {
        let (temporary, admitted) = admit_days("g1-ever", &["20260823"]);
        let (_held, error) = continue_with_fault(&admitted, PublishFault::AfterEver);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let days = days_dir(&temporary);
        assert!(days.join("20260823.ever.wit.json").exists());
        assert!(!days.join("20260823.rev.1.wit.json").exists());
        assert!(!days.join("20260823.head.json").exists());
        assert!(
            !records_dir(&temporary)
                .join("20260823/record.json")
                .exists()
        );
    }

    #[test]
    fn publish_order_is_witness_then_head_then_record() {
        let (temporary, admitted) = admit_days("order", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let days = days_dir(&temporary);
        let old_head = fs::read(days.join("20260823.head.json")).unwrap();
        let old_record = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        let _inject = fail_after_witness();
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        assert!(days.join("20260823.rev.2.wit.json").exists());
        assert_eq!(fs::read(days.join("20260823.head.json")).unwrap(), old_head);
        assert_eq!(
            fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap(),
            old_record
        );
    }

    #[test]
    fn ever_created_once_at_g1_and_immutable() {
        let (temporary, admitted) = admit_days("ever-once", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let ever_path = days_dir(&temporary).join("20260823.ever.wit.json");
        let before = fs::read(&ever_path).unwrap();
        held.advance_dirty().unwrap();
        assert_eq!(before, fs::read(&ever_path).unwrap());
    }

    #[test]
    fn adoption_immutable_outside_records() {
        let (temporary, admitted) = admit_days("adopt-imm", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let adoption = days_dir(&temporary).join("20260823.adopt.json");
        assert!(adoption.exists());
        assert!(!records_dir(&temporary).join("20260823.adopt.json").exists());
        let before = fs::read(&adoption).unwrap();
        held.advance_dirty().unwrap();
        assert_eq!(before, fs::read(&adoption).unwrap());
    }

    #[test]
    fn revision_witness_has_prior_digest_chain() {
        let (temporary, admitted) = admit_days("wit-chain", &["20260823"]);
        let mut held = continue_ok(&admitted);
        held.advance_dirty().unwrap();
        let days = days_dir(&temporary);
        let rev1: RevisionWitness = read_json_file(&days.join("20260823.rev.1.wit.json"));
        let rev2: RevisionWitness = read_json_file(&days.join("20260823.rev.2.wit.json"));
        assert_eq!(
            rev2.prior_witness_digest,
            digest_value(&rev1).unwrap().as_hex()
        );
    }

    #[test]
    fn rev1_prior_is_ever_digest() {
        let (temporary, admitted) = admit_days("rev1-ever", &["20260823"]);
        let _held = continue_ok(&admitted);
        let days = days_dir(&temporary);
        let ever: EverWitness = read_json_file(&days.join("20260823.ever.wit.json"));
        let rev1: RevisionWitness = read_json_file(&days.join("20260823.rev.1.wit.json"));
        assert_eq!(
            rev1.prior_witness_digest,
            digest_value(&ever).unwrap().as_hex()
        );
        assert_ne!(rev1.prior_witness_digest, "0".repeat(64));
    }

    #[test]
    fn head_carries_witness_and_record_digest() {
        let (temporary, admitted) = admit_days("head-digests", &["20260823"]);
        let _held = continue_ok(&admitted);
        let days = days_dir(&temporary);
        let head: Head = read_json_file(&days.join("20260823.head.json"));
        let rev1: RevisionWitness = read_json_file(&days.join("20260823.rev.1.wit.json"));
        let record: DayRecord =
            read_json_file(&records_dir(&temporary).join("20260823/record.json"));
        assert_eq!(head.witness_digest, digest_value(&rev1).unwrap().as_hex());
        assert_eq!(head.record_digest, record_digest(&record).unwrap().as_hex());
    }

    #[test]
    fn head_is_read_directly_not_from_directory_tail() {
        let (temporary, admitted) = admit_days("head-direct", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let days = days_dir(&temporary);
        fs::copy(
            days.join("20260823.rev.1.wit.json"),
            days.join("20260823.rev.2.wit.json"),
        )
        .unwrap();
        // Identical R1 bytes planted as R2 are a malformed extra, not pending (AC3.26 / 10.149).
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness
            }
        ));
    }

    #[test]
    fn gapped_witness_chain_is_unknown() {
        let (temporary, admitted) = admit_days("gap-wit", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        held.advance_dirty().unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness
            }
        ));
    }

    #[test]
    fn conflicting_witness_digest_is_unknown() {
        let (temporary, admitted) = admit_days("conflict-wit", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        let path = days_dir(&temporary).join("20260823.rev.2.wit.json");
        let mut witness: RevisionWitness = read_json_file(&path);
        witness.prior_witness_digest = "ab".repeat(32);
        fs::write(&path, serde_json::to_vec(&witness).unwrap()).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness
            }
        ));
    }

    #[test]
    fn contiguous_chain_must_match_head() {
        let (temporary, admitted) = admit_days("chain-head", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        let path = days_dir(&temporary).join("20260823.head.json");
        let mut head: Head = read_json_file(&path);
        head.witness_digest = "cd".repeat(32);
        fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::Head
            }
        ));
    }

    #[test]
    fn crash_before_witness_preserves_prior() {
        let (temporary, admitted) = admit_days("before-wit", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let before = published_snapshot(&held, &day);
        fs::create_dir(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        assert_eq!(published_snapshot(&held, &day).digest, before.digest);
    }

    #[test]
    fn pre_witness_failure_does_not_consume_authority() {
        let (temporary, admitted) = admit_days("retry-wit", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let blocker = days_dir(&temporary).join("20260823.rev.2.wit.json");
        fs::create_dir(&blocker).unwrap();
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        fs::remove_dir(&blocker).unwrap();
        held.proceed().unwrap();
        assert_eq!(published_snapshot(&held, &day).record_revision, 2);
    }

    #[test]
    fn crash_after_witness_before_head_is_pending() {
        let (temporary, admitted) = admit_days("after-wit", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let days = days_dir(&temporary);
        let head1 = fs::read(days.join("20260823.head.json")).unwrap();
        let record1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::write(days.join("20260823.head.json"), head1).unwrap();
        fs::write(
            records_dir(&temporary).join("20260823/record.json"),
            record1,
        )
        .unwrap();
        match held.inspect_day(&day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn witness_without_head_is_pending() {
        let (temporary, admitted) = admit_days("wit-no-head", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        fs::remove_file(records_dir(&temporary).join("20260823/record.json")).unwrap();
        match held.inspect_day(&day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn head_durability_uncertain_does_not_write_record() {
        let (temporary, admitted) = admit_days("head-unc", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let record_path = records_dir(&temporary).join("20260823/record.json");
        let before = fs::read(&record_path).unwrap();
        let _inject = fail_next_dir_sync();
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::Head
            }
        ));
        assert_eq!(before, fs::read(&record_path).unwrap());
    }

    #[test]
    fn crash_after_head_before_record_is_pending_mismatch() {
        let (temporary, admitted) = admit_days("after-head", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let record1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::write(
            records_dir(&temporary).join("20260823/record.json"),
            record1,
        )
        .unwrap();
        match held.inspect_day(&day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn g1_crash_after_ever_before_witness_is_unknown() {
        let (temporary, admitted) = admit_days("after-ever", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let days = days_dir(&temporary);
        fs::remove_file(days.join("20260823.rev.1.wit.json")).unwrap();
        fs::remove_file(days.join("20260823.head.json")).unwrap();
        fs::remove_file(records_dir(&temporary).join("20260823/record.json")).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::EverWitness
            }
        ));
    }

    #[test]
    fn ever_or_head_missing_is_unknown() {
        let (temporary, admitted) = admit_days("missing-head", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
        drop(held);
        let (temporary, admitted) = admit_days("missing-ever", &["20260823"]);
        let held = continue_ok(&admitted);
        fs::remove_file(days_dir(&temporary).join("20260823.ever.wit.json")).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn same_generation_completion_preserves_both_serials() {
        let (_temporary, admitted) = admit_days("complete", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let dirty_snap = published_snapshot(&held, &day);
        publish_kind_for_test(
            admitted.store(),
            held.lock_set(),
            &day,
            PreparedCompletionAuthority,
        )
        .unwrap();
        let done = published_snapshot(&held, &day);
        assert_eq!(
            done.first_transition_serial,
            dirty_snap.first_transition_serial
        );
        assert_eq!(
            done.dirty_by_transition_serial,
            dirty_snap.dirty_by_transition_serial
        );
        assert_eq!(done.dirty_generation, dirty_snap.dirty_generation);
        assert_eq!(done.completed_generation, done.dirty_generation);
        assert_eq!(done.record_revision, dirty_snap.record_revision + 1);
    }

    #[test]
    fn g5_dirty_then_same_g5_completed() {
        let (_temporary, admitted) = admit_days("g5", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        for _ in 0..4 {
            held.advance_dirty().unwrap();
        }
        let dirty_snap = published_snapshot(&held, &day);
        assert_eq!(dirty_snap.dirty_generation, 5);
        assert_eq!(dirty_snap.completed_generation, 0);
        assert_eq!(dirty_snap.record_revision, 5);
        publish_kind_for_test(
            admitted.store(),
            held.lock_set(),
            &day,
            PreparedCompletionAuthority,
        )
        .unwrap();
        let done = published_snapshot(&held, &day);
        assert_eq!(done.dirty_generation, 5);
        assert_eq!(done.completed_generation, 5);
        assert_eq!(done.record_revision, 6);
        assert_eq!(
            done.first_transition_serial,
            dirty_snap.first_transition_serial
        );
        assert_eq!(
            done.dirty_by_transition_serial,
            dirty_snap.dirty_by_transition_serial
        );
    }

    #[test]
    fn migration_authority_uses_shared_path() {
        let (temporary, admitted) = admit_days("migrate", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let first = published_snapshot(&held, &day).first_transition_serial;
        publish_kind_for_test(
            admitted.store(),
            held.lock_set(),
            &day,
            MigrationAuthority {
                inventory_first_serial: first,
            },
        )
        .unwrap();
        assert!(
            days_dir(&temporary)
                .join("20260823.rev.2.wit.json")
                .exists()
        );
        assert!(days_dir(&temporary).join("20260823.head.json").exists());
        assert!(
            records_dir(&temporary)
                .join("20260823/record.json")
                .exists()
        );
        let done = published_snapshot(&held, &day);
        assert_eq!(done.completed_generation, done.dirty_generation);
        assert_eq!(done.record_revision, 2);
        assert_eq!(done.first_transition_serial, first);
        assert!(matches!(
            publish_kind_for_test(
                admitted.store(),
                held.lock_set(),
                &day,
                MigrationAuthority {
                    inventory_first_serial: first + 1,
                },
            ),
            Err(ConvergenceError::Refused(Refusal::WrongLineage))
        ));
    }

    #[test]
    fn fixtures_cannot_skip_head_validation() {
        let (temporary, admitted) = admit_days("skip-head", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        let first = published_snapshot(&held, &day).first_transition_serial;
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            publish_kind_for_test(
                admitted.store(),
                held.lock_set(),
                &day,
                PreparedCompletionAuthority
            ),
            Err(ConvergenceError::Unknown { .. })
        ));
        assert!(matches!(
            publish_kind_for_test(
                admitted.store(),
                held.lock_set(),
                &day,
                MigrationAuthority {
                    inventory_first_serial: first,
                },
            ),
            Err(ConvergenceError::Unknown { .. })
        ));
    }

    #[test]
    fn replay_older_completed_or_dirty_record_is_unknown() {
        let (temporary, admitted) = admit_days("replay", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let g1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        for _ in 0..4 {
            held.advance_dirty().unwrap();
        }
        publish_kind_for_test(
            admitted.store(),
            held.lock_set(),
            &day,
            PreparedCompletionAuthority,
        )
        .unwrap();
        fs::write(records_dir(&temporary).join("20260823/record.json"), g1).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::Record
            }
        ));
    }

    #[test]
    fn deleted_tail_witness_and_rolled_record_is_unknown() {
        let (temporary, admitted) = admit_days("del-tail", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        let g1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        fs::write(records_dir(&temporary).join("20260823/record.json"), g1).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn head_deleted_is_unknown() {
        let (temporary, admitted) = admit_days("head-del", &["20260823"]);
        let held = continue_ok(&admitted);
        let day = sample_day();
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn head_rewound_is_unknown() {
        let (temporary, admitted) = admit_days("head-rewound", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        let path = days_dir(&temporary).join("20260823.head.json");
        let mut head: Head = read_json_file(&path);
        head.record_revision = 1;
        fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(matches!(
            held.inspect_day(&day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn authority_reuse_after_witness_refused() {
        let (_temporary, admitted) = admit_days("reuse", &["20260823"]);
        let owner = crate::owner::OwnerBinding::issue_from_base(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::owner::ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        held.continue_with(proof).unwrap();
        let mut reused =
            crate::owner::ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
        reused.consume().unwrap();
        let error = held.continue_with(reused).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ReusedAuthority)
        ));
    }

    #[test]
    fn publish_day_not_in_lock_set_refused() {
        let (_temporary, admitted) = admit_days("wrong-day-pub", &["20260823"]);
        let held = continue_ok(&admitted);
        let day_b = DayKey::parse("20260824").unwrap();
        let error = held.inspect_day(&day_b).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongDay { .. })
        ));
    }

    #[test]
    fn revision_rollback_is_refused() {
        struct LowerRevision;
        impl sealed::PublicationKind for LowerRevision {
            fn next_record(
                &self,
                current: Option<&DayRecord>,
            ) -> Result<DayRecord, ConvergenceError> {
                let mut record = current.unwrap().clone();
                record.record_revision = 1;
                Ok(record)
            }
        }
        let (_temporary, admitted) = admit_days("rev-rollback", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        let error = publish_kind_for_test(admitted.store(), held.lock_set(), &day, LowerRevision)
            .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::RevisionRollback { .. })
        ));
    }

    #[test]
    fn generation_rollback_is_refused() {
        struct LowerGeneration;
        impl sealed::PublicationKind for LowerGeneration {
            fn next_record(
                &self,
                current: Option<&DayRecord>,
            ) -> Result<DayRecord, ConvergenceError> {
                let mut record = current.unwrap().clone();
                record.record_revision += 1;
                record.dirty_generation = 1;
                Ok(record)
            }
        }
        let (_temporary, admitted) = admit_days("gen-rollback", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        held.advance_dirty().unwrap();
        let error = publish_kind_for_test(admitted.store(), held.lock_set(), &day, LowerGeneration)
            .unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::GenerationRollback { .. })
        ));
    }

    #[test]
    fn revalidates_before_witness_head_and_record() {
        let (temporary, admitted) = admit_days("reval", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let head_path = days_dir(&temporary).join("20260823.head.json");
        let original_head = fs::read(&head_path).unwrap();
        let _inject = after_witness(move || {
            let mut head: Head = read_json_file(&head_path);
            head.witness_digest = "ff".repeat(32);
            fs::write(&head_path, serde_json::to_vec(&head).unwrap()).unwrap();
        });
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Unknown {
                role: DurableRole::Head
            }
        ));
        assert!(
            days_dir(&temporary)
                .join("20260823.rev.2.wit.json")
                .exists()
        );
        let restored = fs::read(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert_ne!(restored, original_head);
        let record: DayRecord =
            read_json_file(&records_dir(&temporary).join("20260823/record.json"));
        assert_eq!(record.record_revision, 1);
    }
}
