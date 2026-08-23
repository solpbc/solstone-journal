// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::ffi::OsStr;

use solstone_core_journal_io::{BoundAtomicOutcome, create_directory_bound};

use crate::allocate::load_adoption;
use crate::digest::{RecordDigest, digest_value};
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::{StoreDirs, load_allocator, open_store_dirs};
use crate::layout::{DayKey, ever_name, head_name, record_file_name, revision_witness_name};
use crate::lock::{AllocationProof, DayLockSet};
use crate::schema::{
    DayRecord, EverWitness, Head, RevisionWitness, SCHEMA_VERSION, read_json, record_digest,
    replace_json, require_day, require_ids, validate_record_numbers, write_json_exclusive,
};
use crate::store::{ConvergenceStore, LoadDay, PendingKind, snapshot_from_record};
use crate::walk::open_dir;

mod sealed {
    use crate::error::ConvergenceError;
    use crate::schema::DayRecord;

    pub trait PublicationKind {
        fn next_record(&self, current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError>;
    }
}

/// Production dirty-advance intent. The only public production intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrdinaryIntent {
    AdvanceDirty,
}

/// Handle returned by [`ConvergenceStore::propose`]. Performs no on-disk write
/// at construction; `propose` itself reads current head/record.
pub struct ValidatedProposal {
    day: DayKey,
    instance: String,
    prior: Option<DayRecord>,
}

/// Owner-supplied publication authority. `used` is set once the revision witness is durable.
#[derive(Debug)]
pub struct OrdinaryAuthority {
    record: DayRecord,
    day: DayKey,
    instance: String,
    serial: u64,
    used: bool,
}

impl OrdinaryAuthority {
    pub fn bind(
        proposal: ValidatedProposal,
        proof: AllocationProof,
    ) -> Result<Self, ConvergenceError> {
        if proposal.instance != proof.instance() {
            return Err(ConvergenceError::Refused(Refusal::StaleLease));
        }
        if !proof.days().contains(&proposal.day) {
            return Err(ConvergenceError::Refused(Refusal::WrongDay {
                expected: proposal.day.as_str().to_owned(),
                observed: String::new(),
            }));
        }
        let record = match proposal.prior {
            None => DayRecord {
                schema_version: SCHEMA_VERSION,
                journal_id: proof.journal_id().to_owned(),
                root_id: proof.root_id().to_owned(),
                adoption_id: String::new(),
                day: proposal.day.as_str().to_owned(),
                record_revision: 1,
                first_transition_serial: proof.serial(),
                dirty_by_transition_serial: proof.serial(),
                dirty_generation: 1,
                completed_generation: 0,
                auxiliary_time: crate::schema::now_rfc3339(),
            },
            Some(prior) => DayRecord {
                schema_version: SCHEMA_VERSION,
                journal_id: prior.journal_id,
                root_id: prior.root_id,
                adoption_id: prior.adoption_id,
                day: prior.day,
                record_revision: prior.record_revision + 1,
                first_transition_serial: prior.first_transition_serial,
                dirty_by_transition_serial: proof.serial(),
                dirty_generation: prior.dirty_generation + 1,
                completed_generation: prior.completed_generation,
                auxiliary_time: prior.auxiliary_time,
            },
        };
        Ok(Self {
            record,
            day: proposal.day,
            instance: proof.instance().to_owned(),
            serial: proof.serial(),
            used: false,
        })
    }
}

impl sealed::PublicationKind for OrdinaryAuthority {
    fn next_record(&self, _current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError> {
        Ok(self.record.clone())
    }
}

#[derive(Debug)]
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

impl ConvergenceStore {
    /// Read current head/record and derive the next dirty record. No on-disk write.
    pub fn propose(
        &self,
        days: &DayLockSet,
        day: &DayKey,
        intent: OrdinaryIntent,
    ) -> Result<ValidatedProposal, ConvergenceError> {
        let _ = intent;
        match self.load_day(days, day)? {
            LoadDay::Genesis => Ok(ValidatedProposal {
                day: day.clone(),
                instance: days.instance().to_owned(),
                prior: None,
            }),
            LoadDay::Published(snapshot) => {
                let dirs = open_store_dirs(self.root())?
                    .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
                let record = load_record(&dirs, day)?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::Record,
                })?;
                let _ = snapshot;
                Ok(ValidatedProposal {
                    day: day.clone(),
                    instance: days.instance().to_owned(),
                    prior: Some(record),
                })
            }
            LoadDay::PublicationPending { .. } => Err(ConvergenceError::Unknown {
                role: DurableRole::Head,
            }),
        }
    }

    pub fn publish(
        &self,
        days: &DayLockSet,
        day: &DayKey,
        authority: &mut OrdinaryAuthority,
    ) -> Result<PublishOutcome, ConvergenceError> {
        if authority.used {
            return Err(ConvergenceError::Refused(Refusal::ReusedAuthority));
        }
        if authority.instance != days.instance() {
            return Err(ConvergenceError::Refused(Refusal::StaleLease));
        }
        if &authority.day != day || !days.contains(day) {
            return Err(ConvergenceError::Refused(Refusal::WrongDay {
                expected: day.as_str().to_owned(),
                observed: authority.day.as_str().to_owned(),
            }));
        }
        let dirs = open_store_dirs(self.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let allocator = load_allocator(&dirs)?;
        if authority.serial != allocator.next_serial.saturating_sub(1) {
            return Err(ConvergenceError::Refused(Refusal::InterveningAdvance));
        }
        let adoption = load_adoption(&dirs, day)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        })?;
        if authority.record.adoption_id.is_empty() {
            authority.record.adoption_id = adoption.adoption_id.clone();
        }
        let next = sealed::PublicationKind::next_record(authority, None)?;
        publish_record(self, days, day, &next, Some(&mut authority.used))
    }
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
        (None, None, None) => return Ok(LoadDay::Genesis),
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
    if probe_extra_revision_witness(&dirs, day, height + 1)?.is_some() {
        return Ok(LoadDay::PublicationPending {
            kind: PendingKind::WitnessAheadOfHead,
        });
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

fn publish_record(
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
        LoadDay::PublicationPending { .. } => {
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
    let record_hex = record_digest(next)?.as_hex().to_owned();
    let outcome = if is_g1 {
        publish_g1(store, &dirs, day, next, &record_hex, used)?
    } else {
        publish_next(store, &dirs, day, next, &record_hex, current.as_ref(), used)?
    };
    Ok(outcome)
}

fn revalidate_gates(
    store: &ConvergenceStore,
    days_fd: &StoreDirs,
    day: &DayKey,
    expect_g1_ever: bool,
    expect_head_revision: Option<u64>,
) -> Result<(), ConvergenceError> {
    store.revalidate()?;
    let _ = days_fd;
    match inspect_day(store, day)? {
        LoadDay::Genesis if expect_g1_ever => Ok(()),
        LoadDay::Published(snapshot) => {
            if expect_head_revision.is_some_and(|expected| snapshot.record_revision != expected) {
                return Err(ConvergenceError::Unknown {
                    role: DurableRole::Head,
                });
            }
            Ok(())
        }
        LoadDay::Genesis => Err(ConvergenceError::Unknown {
            role: DurableRole::EverWitness,
        }),
        LoadDay::PublicationPending { .. } => Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        }),
    }
}

fn publish_g1(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    day: &DayKey,
    next: &DayRecord,
    record_hex: &str,
    mut used: Option<&mut bool>,
) -> Result<PublishOutcome, ConvergenceError> {
    revalidate_gates(store, dirs, day, true, None)?;
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
    let ever_digest = digest_value(&ever)?;
    revalidate_after_ever(store, dirs, day)?;
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
    let witness_digest = digest_value(&witness)?;
    revalidate_after_witness(store, dirs, day, 1)?;
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
    store.revalidate()?;
    write_record_file(dirs, day, next)?;
    Ok(published(day, next, record_hex))
}

fn publish_next(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    day: &DayKey,
    next: &DayRecord,
    record_hex: &str,
    current: Option<&DayRecord>,
    mut used: Option<&mut bool>,
) -> Result<PublishOutcome, ConvergenceError> {
    let height = current.map(|record| record.record_revision).unwrap_or(0);
    revalidate_gates(store, dirs, day, false, Some(height))?;
    let prior_witness = read_json::<RevisionWitness>(
        &dirs.days,
        &revision_witness_name(day, height),
        DurableRole::RevisionWitness,
    )?
    .ok_or(ConvergenceError::Unknown {
        role: DurableRole::RevisionWitness,
    })?;
    let prior_digest = digest_value(&prior_witness)?;
    let witness = revision_witness(next, record_hex, prior_digest.as_hex());
    write_json_exclusive(
        &dirs.days,
        &revision_witness_name(day, next.record_revision),
        &witness,
        DurableRole::RevisionWitness,
    )?;
    if let Some(flag) = used.as_mut() {
        **flag = true;
    }
    let witness_digest = digest_value(&witness)?;
    revalidate_after_witness(store, dirs, day, next.record_revision)?;
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
    store.revalidate()?;
    write_record_file(dirs, day, next)?;
    Ok(published(day, next, record_hex))
}

fn revalidate_after_ever(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    day: &DayKey,
) -> Result<(), ConvergenceError> {
    store.revalidate()?;
    if read_json::<EverWitness>(&dirs.days, &ever_name(day), DurableRole::EverWitness)?.is_none() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::EverWitness,
        });
    }
    if read_json::<Head>(&dirs.days, &head_name(day), DurableRole::Head)?.is_some() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Head,
        });
    }
    Ok(())
}

fn revalidate_after_witness(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    day: &DayKey,
    revision: u64,
) -> Result<(), ConvergenceError> {
    store.revalidate()?;
    if read_json::<RevisionWitness>(
        &dirs.days,
        &revision_witness_name(day, revision),
        DurableRole::RevisionWitness,
    )?
    .is_none()
    {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::RevisionWitness,
        });
    }
    Ok(())
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
        LoadDay::PublicationPending { .. } => {
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
pub(crate) struct MigrationAuthority;

#[cfg(test)]
impl sealed::PublicationKind for MigrationAuthority {
    fn next_record(&self, current: Option<&DayRecord>) -> Result<DayRecord, ConvergenceError> {
        PreparedCompletionAuthority.next_record(current)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::OrdinaryIntent;
    use crate::error::Refusal;
    use crate::layout::DayKey;
    use crate::store::{LoadDay, PendingKind};
    use crate::test_support::{
        days_dir, dirty, fail_next_dir_sync, initialized_store, records_dir, sample_day,
    };
    use std::fs;
    use std::path::Path;

    fn published_snapshot(
        store: &ConvergenceStore,
        locks: &DayLockSet,
        day: &DayKey,
    ) -> crate::store::DaySnapshot {
        match store.load_day(locks, day).unwrap() {
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
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let days = days_dir(&temporary);
        let ever = days.join("20260823.ever.wit.json");
        let witness = days.join("20260823.rev.1.wit.json");
        assert!(ever.exists());
        assert!(witness.exists());
        let ever_meta = fs::metadata(&ever).unwrap();
        let witness_meta = fs::metadata(&witness).unwrap();
        assert!(ever_meta.modified().unwrap() <= witness_meta.modified().unwrap());
    }

    #[test]
    fn publish_order_is_witness_then_head_then_record() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let days = days_dir(&temporary);
        assert!(days.join("20260823.rev.1.wit.json").exists());
        assert!(days.join("20260823.head.json").exists());
        assert!(
            records_dir(&temporary)
                .join("20260823/record.json")
                .exists()
        );
    }

    #[test]
    fn ever_created_once_at_g1_and_immutable() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let ever_path = days_dir(&temporary).join("20260823.ever.wit.json");
        let before = fs::read(&ever_path).unwrap();
        dirty(&store, &locks, &day);
        assert_eq!(before, fs::read(&ever_path).unwrap());
    }

    #[test]
    fn adoption_immutable_outside_records() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        store.allocate(&locks).unwrap();
        let adoption = days_dir(&temporary).join("20260823.adopt.json");
        assert!(adoption.exists());
        assert!(!records_dir(&temporary).join("20260823.adopt.json").exists());
        let before = fs::read(&adoption).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        assert_eq!(before, fs::read(&adoption).unwrap());
    }

    #[test]
    fn revision_witness_has_prior_digest_chain() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
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
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
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
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
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
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let days = days_dir(&temporary);
        fs::copy(
            days.join("20260823.rev.1.wit.json"),
            days.join("20260823.rev.2.wit.json"),
        )
        .unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn gapped_witness_chain_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness
            }
        ));
    }

    #[test]
    fn conflicting_witness_digest_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        let path = days_dir(&temporary).join("20260823.rev.2.wit.json");
        let mut witness: RevisionWitness = read_json_file(&path);
        witness.prior_witness_digest = "ab".repeat(32);
        fs::write(&path, serde_json::to_vec(&witness).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::RevisionWitness
            }
        ));
    }

    #[test]
    fn contiguous_chain_must_match_head() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        let path = days_dir(&temporary).join("20260823.head.json");
        let mut head: Head = read_json_file(&path);
        head.witness_digest = "cd".repeat(32);
        fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::Head
            }
        ));
    }

    #[test]
    fn crash_before_witness_preserves_prior() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let before = published_snapshot(&store, &locks, &day);
        fs::create_dir(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        let error = store.publish(&locks, &day, &mut authority).unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        assert_eq!(
            published_snapshot(&store, &locks, &day).digest,
            before.digest
        );
    }

    #[test]
    fn pre_witness_failure_does_not_consume_authority() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let blocker = days_dir(&temporary).join("20260823.rev.2.wit.json");
        fs::create_dir(&blocker).unwrap();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        assert!(store.publish(&locks, &day, &mut authority).is_err());
        fs::remove_dir(&blocker).unwrap();
        store.publish(&locks, &day, &mut authority).unwrap();
        assert_eq!(published_snapshot(&store, &locks, &day).record_revision, 2);
    }

    #[test]
    fn failure_before_witness_preserves_prior() {
        crash_before_witness_preserves_prior();
    }

    #[test]
    fn crash_after_witness_before_head_is_pending() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let days = days_dir(&temporary);
        let head1 = fs::read(days.join("20260823.head.json")).unwrap();
        let record1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        dirty(&store, &locks, &day);
        fs::write(days.join("20260823.head.json"), head1).unwrap();
        fs::write(
            records_dir(&temporary).join("20260823/record.json"),
            record1,
        )
        .unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn witness_newer_than_head_is_pending() {
        crash_after_witness_before_head_is_pending();
    }

    #[test]
    fn witness_without_head_is_pending() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        fs::remove_file(records_dir(&temporary).join("20260823/record.json")).unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn head_durability_uncertain_does_not_write_record() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let record_path = records_dir(&temporary).join("20260823/record.json");
        let before = fs::read(&record_path).unwrap();
        fail_next_dir_sync();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        match store.publish(&locks, &day, &mut authority).unwrap() {
            PublishOutcome::PublishedDurabilityUncertain { .. } => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(before, fs::read(&record_path).unwrap());
    }

    #[test]
    fn witness_head_uncertainty_does_not_change_record() {
        head_durability_uncertain_does_not_write_record();
    }

    #[test]
    fn uncertainty_after_witness_or_head_is_pending() {
        head_durability_uncertain_does_not_write_record();
    }

    #[test]
    fn crash_after_head_before_record_is_pending_mismatch() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let record1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        dirty(&store, &locks, &day);
        fs::write(
            records_dir(&temporary).join("20260823/record.json"),
            record1,
        )
        .unwrap();
        match store.load_day(&locks, &day).unwrap() {
            LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn durable_head_then_record_failure_is_pending_mismatch() {
        crash_after_head_before_record_is_pending_mismatch();
    }

    #[test]
    fn g1_crash_after_ever_before_witness_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let days = days_dir(&temporary);
        fs::remove_file(days.join("20260823.rev.1.wit.json")).unwrap();
        fs::remove_file(days.join("20260823.head.json")).unwrap();
        fs::remove_file(records_dir(&temporary).join("20260823/record.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::EverWitness
            }
        ));
    }

    #[test]
    fn ever_or_head_missing_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn same_generation_completion_preserves_both_serials() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let dirty_snap = published_snapshot(&store, &locks, &day);
        publish_kind_for_test(&store, &locks, &day, PreparedCompletionAuthority).unwrap();
        let done = published_snapshot(&store, &locks, &day);
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
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        for _ in 0..5 {
            dirty(&store, &locks, &day);
        }
        let dirty_snap = published_snapshot(&store, &locks, &day);
        assert_eq!(dirty_snap.dirty_generation, 5);
        assert_eq!(dirty_snap.completed_generation, 0);
        assert_eq!(dirty_snap.record_revision, 5);
        publish_kind_for_test(&store, &locks, &day, PreparedCompletionAuthority).unwrap();
        let done = published_snapshot(&store, &locks, &day);
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
    fn prepared_completion_authority_uses_shared_path() {
        g5_dirty_then_same_g5_completed();
    }

    #[test]
    fn migration_authority_uses_shared_path() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        publish_kind_for_test(&store, &locks, &day, MigrationAuthority).unwrap();
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
        let done = published_snapshot(&store, &locks, &day);
        assert_eq!(done.completed_generation, done.dirty_generation);
    }

    #[test]
    fn fixtures_cannot_skip_head_validation() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            publish_kind_for_test(&store, &locks, &day, PreparedCompletionAuthority),
            Err(ConvergenceError::Unknown { .. })
        ));
    }

    #[test]
    fn replay_older_completed_or_dirty_record_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let g1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        for _ in 0..4 {
            dirty(&store, &locks, &day);
        }
        publish_kind_for_test(&store, &locks, &day, PreparedCompletionAuthority).unwrap();
        fs::write(records_dir(&temporary).join("20260823/record.json"), g1).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown {
                role: DurableRole::Record
            }
        ));
    }

    #[test]
    fn deleted_tail_witness_and_rolled_record_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        let g1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.rev.2.wit.json")).unwrap();
        fs::write(records_dir(&temporary).join("20260823/record.json"), g1).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn head_deleted_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn head_rewound_is_unknown() {
        let (temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        let path = days_dir(&temporary).join("20260823.head.json");
        let mut head: Head = read_json_file(&path);
        head.record_revision = 1;
        fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        assert!(matches!(
            store.load_day(&locks, &day).unwrap_err(),
            ConvergenceError::Unknown { .. }
        ));
    }

    #[test]
    fn authority_reuse_after_witness_refused() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        store.publish(&locks, &day, &mut authority).unwrap();
        let error = store.publish(&locks, &day, &mut authority).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::ReusedAuthority)
        ));
    }

    #[test]
    fn publish_day_not_in_lock_set_refused() {
        let (_temporary, store) = initialized_store();
        let day_a = DayKey::parse("20260823").unwrap();
        let day_b = DayKey::parse("20260824").unwrap();
        let locks = store.acquire_days(std::slice::from_ref(&day_a)).unwrap();
        let proof = store.allocate(&locks).unwrap();
        let proposal = store
            .propose(&locks, &day_a, OrdinaryIntent::AdvanceDirty)
            .unwrap();
        let mut authority = OrdinaryAuthority::bind(proposal, proof).unwrap();
        let error = store.publish(&locks, &day_b, &mut authority).unwrap_err();
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
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        let error = publish_kind_for_test(&store, &locks, &day, LowerRevision).unwrap_err();
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
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        dirty(&store, &locks, &day);
        let error = publish_kind_for_test(&store, &locks, &day, LowerGeneration).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::GenerationRollback { .. })
        ));
    }

    #[test]
    fn revalidates_before_witness_head_and_record() {
        let (_temporary, store) = initialized_store();
        let day = sample_day();
        let locks = store.acquire_days(std::slice::from_ref(&day)).unwrap();
        dirty(&store, &locks, &day);
        store.revalidate().unwrap();
        dirty(&store, &locks, &day);
        assert_eq!(published_snapshot(&store, &locks, &day).record_revision, 2);
    }
}
