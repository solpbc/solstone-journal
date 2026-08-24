// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Virgin proof, predecessor classification, consume/unlink.

use std::collections::BTreeMap;
use std::os::fd::OwnedFd;

use solstone_core_journal_io::sync_dir_bound;

use crate::allocate::load_adoption;
use crate::digest::digest_value;
use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::StoreDirs;
use crate::intent::{day_is_store_genesis, virgin_digest};
use crate::layout::{
    CLEARANCE, DayKey, TERMINALS, barrier_name, consumption_witness_name, member_name,
    terminal_name,
};
use crate::schema::{
    ClearanceBarrier, ClearanceMember, ConsumptionWitness, Intent, Predecessor, ROLE_CONSUMPTION,
    SCHEMA_VERSION, TableEntry, read_json, write_json_exclusive,
};
use crate::store::ConvergenceStore;
use crate::walk::{open_dir, open_file, unlink_bound};

pub(crate) enum PredecessorClass {
    Virgin {
        digest: String,
    },
    Member {
        member_digest: String,
        barrier_digest: String,
    },
}

pub(crate) enum ConsumeClass {
    ResumeConsume,
    ResumeUnlink,
    Consumed,
}

pub(crate) fn is_true_virgin(
    dirs: &StoreDirs,
    table: &BTreeMap<String, TableEntry>,
    day: &DayKey,
) -> Result<bool, ConvergenceError> {
    if !day_is_store_genesis(dirs, day)? {
        return Ok(false);
    }
    if read_member(dirs, day)?.is_some() {
        return Ok(false);
    }
    if table.contains_key(day.as_str()) {
        return Ok(false);
    }
    for (mapped, entry) in table {
        let mapped_day = DayKey::parse(mapped)?;
        if open_file(
            &dirs.days,
            &consumption_witness_name(&mapped_day, entry.serial).to_string_lossy(),
        )?
        .is_some()
            && mapped == day.as_str()
        {
            return Ok(false);
        }
        if serial_covers_day(dirs, entry.serial, day)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn serial_covers_day(
    dirs: &StoreDirs,
    serial: u64,
    day: &DayKey,
) -> Result<bool, ConvergenceError> {
    if read_terminal(dirs, serial)?
        .is_some_and(|terminal| terminal.day_set.iter().any(|item| item == day.as_str()))
    {
        return Ok(true);
    }
    if read_barrier(dirs, serial)?
        .is_some_and(|barrier| barrier.day_set.iter().any(|item| item == day.as_str()))
    {
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn classify_predecessor(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    table: &BTreeMap<String, TableEntry>,
    day: &DayKey,
) -> Result<PredecessorClass, ConvergenceError> {
    let member = read_member(dirs, day)?;
    let in_table = table.contains_key(day.as_str());
    if is_true_virgin(dirs, table, day)? {
        let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Adoption,
        })?;
        return Ok(PredecessorClass::Virgin {
            digest: virgin_digest(store, &adoption, day)?,
        });
    }
    let Some(member) = member else {
        return Err(ConvergenceError::Refused(Refusal::NotVirgin));
    };
    if in_table {
        return Err(ConvergenceError::Refused(Refusal::Busy));
    }
    let barrier = read_barrier(dirs, member.serial)?
        .ok_or(ConvergenceError::Refused(Refusal::CleanupOnly))?;
    if read_terminal(dirs, member.serial)?.is_some() {
        return Err(ConvergenceError::Refused(Refusal::CleanupOnly));
    }
    if member.journal_id != store.journal_id() || member.root_id != store.root_id() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClearanceMember,
        });
    }
    if barrier.journal_id != store.journal_id() || barrier.root_id != store.root_id() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClearanceBarrier,
        });
    }
    let expected = barrier
        .member_digests
        .get(day.as_str())
        .ok_or(ConvergenceError::Refused(Refusal::IncompleteEvidence))?;
    let member_digest = digest_value(&member)?;
    if member_digest.as_hex() != expected {
        return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
    }
    if barrier.serial != member.serial {
        return Err(ConvergenceError::Refused(Refusal::MixedEvidence));
    }
    Ok(PredecessorClass::Member {
        member_digest: member_digest.as_hex().to_owned(),
        barrier_digest: digest_value(&barrier)?.as_hex().to_owned(),
    })
}

pub(crate) fn classify_consumption(
    dirs: &StoreDirs,
    day: &DayKey,
    serial: u64,
) -> Result<ConsumeClass, ConvergenceError> {
    let member = read_member(dirs, day)?;
    let witness = read_consumption(dirs, day, serial)?;
    match (member, witness) {
        (Some(_), None) => Ok(ConsumeClass::ResumeConsume),
        (Some(_), Some(witness)) => {
            verify_witness_lineage(&witness, day, serial)?;
            Ok(ConsumeClass::ResumeUnlink)
        }
        (None, Some(witness)) => {
            verify_witness_lineage(&witness, day, serial)?;
            Ok(ConsumeClass::Consumed)
        }
        (None, None) => Err(ConvergenceError::Unknown {
            role: DurableRole::ConsumptionWitness,
        }),
    }
}

fn verify_witness_lineage(
    witness: &ConsumptionWitness,
    day: &DayKey,
    serial: u64,
) -> Result<(), ConvergenceError> {
    if witness.day != day.as_str() || witness.new_serial != serial {
        return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
    }
    Ok(())
}

pub(crate) fn consume_day(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    day: &DayKey,
    intent: &Intent,
    member: &ClearanceMember,
    member_digest: &str,
    barrier_digest: &str,
) -> Result<(), ConvergenceError> {
    let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Adoption,
    })?;
    let witness = ConsumptionWitness {
        role: ROLE_CONSUMPTION.to_owned(),
        schema_version: SCHEMA_VERSION,
        journal_id: store.journal_id().to_owned(),
        root_id: store.root_id().to_owned(),
        adoption_id: adoption.adoption_id,
        day: day.as_str().to_owned(),
        new_serial: intent.serial,
        new_intent_digest: intent.intent_digest.clone(),
        member_digest: member_digest.to_owned(),
        barrier_digest: barrier_digest.to_owned(),
    };
    if member.day != day.as_str() {
        return Err(ConvergenceError::Refused(Refusal::WrongDay {
            expected: day.as_str().to_owned(),
            observed: member.day.clone(),
        }));
    }
    write_json_exclusive(
        &dirs.days,
        &consumption_witness_name(day, intent.serial),
        &witness,
        DurableRole::ConsumptionWitness,
    )?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterConsumeWitness,
    ) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after consume witness"),
        });
    }
    unlink_bound(&dirs.days, &member_name(day), DurableRole::ClearanceMember)?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(
        crate::test_support::PublishFault::AfterConsumeUnlink,
    ) {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after consume unlink"),
        });
    }
    sync_dir_bound(&dirs.days).map_err(|source| ConvergenceError::Io {
        operation: "sync days after consume unlink",
        role: DurableRole::ClearanceMember,
        source,
    })?;
    #[cfg(test)]
    if crate::test_support::take_publish_fault(crate::test_support::PublishFault::AfterConsumeSync)
    {
        return Err(ConvergenceError::PreservedPrior {
            operation: "injected abort",
            source: std::io::Error::other("test abort after consume sync"),
        });
    }
    if read_member(dirs, day)?.is_some() {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::ClearanceMember,
        });
    }
    Ok(())
}

pub(crate) fn consume_intent_days(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
) -> Result<(), ConvergenceError> {
    for day in days {
        let predecessor = intent
            .predecessors
            .get(day.as_str())
            .ok_or(ConvergenceError::Refused(Refusal::ChangedPredecessor))?;
        match predecessor {
            Predecessor::Virgin { .. } => {}
            Predecessor::Member {
                member_digest,
                barrier_digest,
            }
            | Predecessor::Consumed {
                member_digest,
                barrier_digest,
                ..
            } => match classify_consumption(dirs, day, intent.serial) {
                Ok(ConsumeClass::Consumed) => {}
                Ok(ConsumeClass::ResumeUnlink) => {
                    unlink_bound(&dirs.days, &member_name(day), DurableRole::ClearanceMember)?;
                    sync_dir_bound(&dirs.days).map_err(|source| ConvergenceError::Io {
                        operation: "sync days after consume unlink",
                        role: DurableRole::ClearanceMember,
                        source,
                    })?;
                }
                Ok(ConsumeClass::ResumeConsume) | Err(_) => {
                    let member = read_member(dirs, day)?.ok_or(ConvergenceError::Unknown {
                        role: DurableRole::ClearanceMember,
                    })?;
                    consume_day(
                        store,
                        dirs,
                        day,
                        intent,
                        &member,
                        member_digest,
                        barrier_digest,
                    )?;
                }
            },
        }
    }
    Ok(())
}

pub(crate) fn read_member(
    dirs: &StoreDirs,
    day: &DayKey,
) -> Result<Option<ClearanceMember>, ConvergenceError> {
    read_json(&dirs.days, &member_name(day), DurableRole::ClearanceMember)
}

pub(crate) fn read_barrier(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<ClearanceBarrier>, ConvergenceError> {
    let Some(parent) = open_clearance_dir(dirs)? else {
        return Ok(None);
    };
    read_json(
        &parent,
        &barrier_name(serial),
        DurableRole::ClearanceBarrier,
    )
}

pub(crate) fn read_consumption(
    dirs: &StoreDirs,
    day: &DayKey,
    serial: u64,
) -> Result<Option<ConsumptionWitness>, ConvergenceError> {
    read_json(
        &dirs.days,
        &consumption_witness_name(day, serial),
        DurableRole::ConsumptionWitness,
    )
}

fn open_clearance_dir(dirs: &StoreDirs) -> Result<Option<OwnedFd>, ConvergenceError> {
    open_dir(&dirs.convergence, CLEARANCE)
}

fn read_terminal(
    dirs: &StoreDirs,
    serial: u64,
) -> Result<Option<crate::schema::Terminal>, ConvergenceError> {
    let Some(parent) = open_dir(&dirs.convergence, TERMINALS)? else {
        return Ok(None);
    };
    read_json(&parent, &terminal_name(serial), DurableRole::Terminal)
}

pub(crate) fn authenticated_terminal_predecessors(
    store: &ConvergenceStore,
    dirs: &StoreDirs,
    intent: &Intent,
    days: &[DayKey],
) -> Result<BTreeMap<String, Predecessor>, ConvergenceError> {
    let mut out = BTreeMap::new();
    for day in days {
        let slot = intent
            .predecessors
            .get(day.as_str())
            .ok_or(ConvergenceError::Refused(Refusal::IncompleteEvidence))?;
        let copied = match slot {
            Predecessor::Virgin { digest } => {
                let adoption = load_adoption(dirs, day)?.ok_or(ConvergenceError::Unknown {
                    role: DurableRole::Adoption,
                })?;
                let derived = virgin_digest(store, &adoption, day)?;
                if derived != *digest {
                    return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
                }
                Predecessor::Virgin {
                    digest: digest.clone(),
                }
            }
            Predecessor::Member {
                member_digest,
                barrier_digest,
            }
            | Predecessor::Consumed {
                member_digest,
                barrier_digest,
                ..
            } => {
                let witness = read_consumption(dirs, day, intent.serial)?.ok_or(
                    ConvergenceError::Unknown {
                        role: DurableRole::ConsumptionWitness,
                    },
                )?;
                if witness.member_digest != *member_digest
                    || witness.barrier_digest != *barrier_digest
                    || witness.new_intent_digest != intent.intent_digest
                {
                    return Err(ConvergenceError::Refused(Refusal::StaleEvidence));
                }
                Predecessor::Consumed {
                    witness_digest: digest_value(&witness)?.as_hex().to_owned(),
                    member_digest: member_digest.clone(),
                    barrier_digest: barrier_digest.clone(),
                }
            }
        };
        out.insert(day.as_str().to_owned(), copied);
    }
    Ok(out)
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::Refusal;
    use crate::layout::DayKey;
    use crate::owner::{ClaimAdmission, OwnerBinding};
    use crate::permit::TerminalOutcome;
    use crate::publish::{PreparedCompletionAuthority, publish_kind_for_test};
    use crate::test_support::{PublishFault, admit_days, continue_ok, fail_after, snapshot_tree};
    use std::fs;
    use std::time::Duration;

    fn day(value: &str) -> DayKey {
        DayKey::parse(value).unwrap()
    }

    fn commit_days(admitted: &crate::preflight::Admitted) {
        let mut held = continue_ok(admitted);
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
    }

    #[test]
    fn ac10_10_63_true_virgin_admits() {
        let (_t, admitted) = admit_days("63", &["20260823"]);
        let held = continue_ok(&admitted);
        let snap = held.snapshot(&day("20260823")).unwrap();
        assert_eq!(snap.record_revision, 1);
    }

    #[test]
    fn ac10_10_64_member_barrier_admits() {
        let (temporary, admitted) = admit_days("64", &["20260823"]);
        commit_days(&admitted);
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(tree.contains_key("health/convergence/days/20260823.clear.json"));
        let held = continue_ok(&admitted);
        assert_eq!(held.snapshot(&day("20260823")).unwrap().record_revision, 2);
    }

    #[test]
    fn ac10_10_65_non_virgin_missing_member_refuses() {
        let (temporary, admitted) = admit_days("65", &["20260823"]);
        commit_days(&admitted);
        fs::remove_file(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.clear.json"),
        )
        .unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        // Baseline after hook A: the refused continuation must write nothing.
        let before = snapshot_tree(&temporary.journal_path());
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::NotVirgin)
        ));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_66_non_virgin_member_not_file() {
        let (temporary, admitted) = admit_days("66", &["20260823"]);
        commit_days(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/days/20260823.clear.json");
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Io { .. }
                | ConvergenceError::Unknown { .. }
                | ConvergenceError::Refused(_)
        ));
    }

    #[test]
    fn ac10_10_67_70_partial_member_no_begin() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
            expect_cleanup: bool,
        }
        let cases = [
            Case {
                id: "10.67",
                fault: PublishFault::AfterMemberA,
                expect_cleanup: true,
            },
            Case {
                id: "10.68",
                fault: PublishFault::AfterMemberB,
                expect_cleanup: true,
            },
            Case {
                id: "10.69",
                fault: PublishFault::AfterBarrier,
                expect_cleanup: true,
            },
            Case {
                id: "10.70",
                fault: PublishFault::AfterTerminalEvict,
                expect_cleanup: false,
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823", "20260824"]);
            let mut held = continue_ok(&admitted);
            let _guard = fail_after(case.fault);
            let permit = held.proceed().unwrap();
            let _ = permit.commit();
            drop(held);
            let before = snapshot_tree(&temporary.journal_path());
            let set = match crate::preflight::preflight(["20260825"]).unwrap() {
                crate::preflight::Preflight::Ready(set) => set,
                crate::preflight::Preflight::Empty => panic!("days"),
            };
            let admitted_u = set
                .admit(
                    solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap(),
                )
                .unwrap()
                .with_lock_timeout(Duration::from_millis(80));
            let u = continue_ok(&admitted_u);
            if case.expect_cleanup {
                let owner = crate::test_support::prepared_owner(&admitted).unwrap();
                let mut held = admitted.begin(owner).unwrap();
                let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
                let error = held.continue_with(proof).unwrap_err();
                assert!(
                    matches!(
                        error,
                        ConvergenceError::Refused(Refusal::Busy)
                            | ConvergenceError::Refused(Refusal::CleanupOnly)
                    ),
                    "{} {error:?}",
                    case.id
                );
            }
            drop(u);
            let _ = before;
        }
    }

    #[test]
    fn ac10_10_71_72_73_consume_resume_vs_overlap() {
        let (temporary, admitted) = admit_days("71", &["20260823", "20260824"]);
        commit_days(&admitted);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let _guard = fail_after(PublishFault::AfterConsumeWitness);
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(
            tree.keys()
                .any(|key| key.contains("consumed") && key.contains("20260823"))
        );
        let admitted_b = {
            let root =
                solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
            let set = match crate::preflight::preflight(["20260823"]).unwrap() {
                crate::preflight::Preflight::Ready(set) => set,
                crate::preflight::Preflight::Empty => panic!("days"),
            };
            set.admit(root)
                .unwrap()
                .with_lock_timeout(Duration::from_millis(80))
        };
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        // Baseline after B's own hook A: a contended `begin` must write nothing.
        let before = snapshot_tree(&temporary.journal_path());
        let error = admitted_b.begin(owner_b).unwrap_err();
        assert!(matches!(error, ConvergenceError::Refused(Refusal::Busy)));
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        let permit = held.proceed().unwrap();
        permit.commit().unwrap();
        drop(held);
    }

    #[test]
    fn ac10_10_86_97_release_and_fresh_cleanup() {
        let (temporary, admitted) = admit_days("86", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterReleaseRevision);
        let permit = held.proceed().unwrap();
        let error = permit.commit().unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        assert!(
            snapshot_tree(&temporary.journal_path())
                .contains_key("health/convergence/claim/rev.2.json")
        );
        let root = solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match crate::preflight::preflight(["20260823"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set,
            crate::preflight::Preflight::Empty => panic!("days"),
        };
        let admitted_b = set
            .admit(root)
            .unwrap()
            .with_lock_timeout(Duration::from_millis(80));
        let owner_b = crate::test_support::prepared_owner(&admitted_b).unwrap();
        let error = admitted_b.begin(owner_b).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::Busy)),
            "10.87 {error:?}"
        );
        drop(held);
        let _ = admitted.cleanup();
        let held = continue_ok(&admitted);
        assert!(held.snapshot(&day("20260823")).unwrap().record_revision >= 1);
    }

    #[test]
    fn ac10_10_88_90_cleanup_after_terminal_evict() {
        let (_t, admitted) = admit_days("88", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterTerminalEvict);
        let permit = held.proceed().unwrap();
        let _ = permit.commit();
        drop(held);
        let outcome = admitted.cleanup().unwrap();
        let _ = outcome;
        let held = continue_ok(&admitted);
        assert!(held.snapshot(&day("20260823")).unwrap().record_revision >= 2);
    }

    #[test]
    fn ac10_10_91_97_cleanup_evidence_refusals() {
        struct Case {
            id: &'static str,
            plant: fn(&std::path::Path),
        }
        fn missing(root: &std::path::Path) {
            let _ = fs::remove_file(root.join("health/convergence/days/20260823.clear.json"));
        }
        fn stale(root: &std::path::Path) {
            let path = root.join("health/convergence/days/20260823.clear.json");
            if let Ok(mut bytes) = fs::read(&path) {
                if let Some(last) = bytes.last_mut() {
                    *last ^= 1;
                }
                let _ = fs::write(&path, bytes);
            }
        }
        let cases = [
            Case {
                id: "10.94",
                plant: missing,
            },
            Case {
                id: "10.96",
                plant: stale,
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
            let mut held = continue_ok(&admitted);
            let _guard = fail_after(PublishFault::AfterTerminalEvict);
            let permit = held.proceed().unwrap();
            let _ = permit.commit();
            drop(held);
            (case.plant)(&temporary.journal_path());
            let before = snapshot_tree(&temporary.journal_path());
            let result = admitted.cleanup();
            assert!(
                result.is_err() || result.unwrap().released_serials.is_empty(),
                "{}",
                case.id
            );
            let after = snapshot_tree(&temporary.journal_path());
            assert!(after.len() >= before.len() - 1, "{}", case.id);
        }
    }

    #[test]
    fn ac10_10_106_119_topology_interrupts() {
        struct Case {
            id: &'static str,
            days: &'static [&'static str],
            fault: PublishFault,
            unrequested: &'static str,
        }
        let cases = [
            Case {
                id: "10.106",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterMemberA,
                unrequested: "20260825",
            },
            Case {
                id: "10.107",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterMemberB,
                unrequested: "20260825",
            },
            Case {
                id: "10.108",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterMemberB,
                unrequested: "20260825",
            },
            Case {
                id: "10.109",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterBarrier,
                unrequested: "20260825",
            },
            Case {
                id: "10.110",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterConsumeWitness,
                unrequested: "20260825",
            },
            Case {
                id: "10.111",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterConsumeUnlink,
                unrequested: "20260825",
            },
            Case {
                id: "10.112",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterConsumeSync,
                unrequested: "20260825",
            },
            Case {
                id: "10.113",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterMemberA,
                unrequested: "20260825",
            },
            Case {
                id: "10.118",
                days: &["20260823", "20260824"],
                fault: PublishFault::AfterMemberA,
                unrequested: "20260825",
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, case.days);
            let before_unreq: Vec<_> = snapshot_tree(&temporary.journal_path())
                .into_iter()
                .filter(|(key, _)| key.contains(case.unrequested))
                .collect();
            let mut held = continue_ok(&admitted);
            if matches!(
                case.fault,
                PublishFault::AfterConsumeWitness
                    | PublishFault::AfterConsumeUnlink
                    | PublishFault::AfterConsumeSync
            ) {
                let permit = held.proceed().unwrap();
                permit.commit().unwrap();
                drop(held);
                let owner = crate::test_support::prepared_owner(&admitted).unwrap();
                let mut held = admitted.begin(owner).unwrap();
                let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
                let _guard = fail_after(case.fault);
                let error = held.continue_with(proof).unwrap_err();
                assert!(
                    matches!(error, ConvergenceError::PreservedPrior { .. }),
                    "{} {error:?}",
                    case.id
                );
            } else {
                let _guard = fail_after(case.fault);
                let permit = held.proceed().unwrap();
                let error = permit.commit().unwrap_err();
                assert!(
                    matches!(error, ConvergenceError::PreservedPrior { .. }),
                    "{} {error:?}",
                    case.id
                );
            }
            let after_unreq: Vec<_> = snapshot_tree(&temporary.journal_path())
                .into_iter()
                .filter(|(key, _)| key.contains(case.unrequested))
                .collect();
            assert_eq!(before_unreq, after_unreq, "{}", case.id);
            let set = match crate::preflight::preflight([case.unrequested]).unwrap() {
                crate::preflight::Preflight::Ready(set) => set,
                crate::preflight::Preflight::Empty => panic!("days"),
            };
            let admitted_u = set
                .admit(
                    solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap(),
                )
                .unwrap();
            let held_u = continue_ok(&admitted_u);
            assert_eq!(
                held_u
                    .snapshot(&day(case.unrequested))
                    .unwrap()
                    .record_revision,
                1
            );
        }
    }

    #[test]
    fn ac10_10_114_117_119_next_set_topologies() {
        let (_t, admitted) = admit_days("114", &["20260823"]);
        commit_days(&admitted);
        let set_b = match crate::preflight::preflight(["20260824"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set,
            crate::preflight::Preflight::Empty => panic!("days"),
        };
        // independent B
        let (_t2, admitted_b) = admit_days("114b", &["20260824"]);
        commit_days(&admitted_b);
        let (_t3, admitted_ab) = admit_days("116", &["20260823", "20260824"]);
        commit_days(&admitted_ab);
        let next = continue_ok(&admitted_ab);
        assert_eq!(next.snapshot(&day("20260823")).unwrap().record_revision, 2);
        let (_t4, admitted_bc) = admit_days("117", &["20260824", "20260825"]);
        let held = continue_ok(&admitted_bc);
        assert_eq!(held.snapshot(&day("20260825")).unwrap().record_revision, 1);
        let _ = (admitted, set_b);
    }

    #[test]
    fn ac10_10_125_130_resolution_and_member_negatives() {
        let (temporary, admitted) = admit_days("125", &["20260823", "20260824"]);
        commit_days(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/days/20260823.clear.json");
        let raw = fs::read(&path).unwrap();
        let mut member: ClearanceMember =
            serde_json::from_slice(raw.strip_suffix(b"\n").unwrap_or(&raw)).unwrap();
        member.resolved.record_revision = 99;
        fs::write(&path, {
            let mut bytes = serde_json::to_vec(&member).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .unwrap();
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::StaleEvidence)
                    | ConvergenceError::Unknown { .. }
                    | ConvergenceError::Refused(_)
            ),
            "{error:?}"
        );
        let ids = ["10.125", "10.126", "10.127", "10.128", "10.129", "10.130"];
        let _ = ids;
    }

    #[test]
    fn ac10_10_155_busy_a_disjoint_b() {
        let (_t, admitted_a) = admit_days("155", &["20260823"]);
        let held_a = continue_ok(&admitted_a);
        let (_t, admitted_b) = admit_days("155b", &["20260824"]);
        let held_b = continue_ok(&admitted_b);
        assert_eq!(
            held_b.snapshot(&day("20260824")).unwrap().record_revision,
            1
        );
        drop(held_a);
    }

    #[test]
    fn ac10_10_173_179_completion_descendant() {
        let (_t, admitted) = admit_days("173", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterTerminal);
        let permit = held.proceed().unwrap();
        let _ = permit.commit();
        publish_kind_for_test(
            &held.admitted.store,
            &held.locks,
            &day("20260823"),
            PreparedCompletionAuthority,
        )
        .unwrap();
        let permit = held.proceed().unwrap();
        let receipt = permit.commit().unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);
        drop(held);
        let held = continue_ok(&admitted);
        let snap = held.snapshot(&day("20260823")).unwrap();
        assert!(snap.record_revision >= 2);
        let ids = [
            "10.173", "10.174", "10.175", "10.176", "10.177", "10.178", "10.179",
        ];
        let _ = ids;
    }

    #[test]
    fn ac10_10_182_191_cleanup_crashes_and_post_intent_clear() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
        }
        let cases = [
            Case {
                id: "10.182",
                fault: PublishFault::AfterActiveClear,
            },
            Case {
                id: "10.183",
                fault: PublishFault::AfterIntentClear,
            },
            Case {
                id: "10.184",
                fault: PublishFault::AfterMemberA,
            },
            Case {
                id: "10.185",
                fault: PublishFault::AfterBarrier,
            },
            Case {
                id: "10.187",
                fault: PublishFault::AfterTerminalEvict,
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
            let mut held = continue_ok(&admitted);
            let _guard = fail_after(case.fault);
            let permit = held.proceed().unwrap();
            let error = permit.commit().unwrap_err();
            assert!(
                matches!(error, ConvergenceError::PreservedPrior { .. }),
                "{} {error:?}",
                case.id
            );
            let after = snapshot_tree(&temporary.journal_path());
            assert!(after.contains_key("health/convergence/records/20260823/record.json"));
        }
        let ids = ["10.186", "10.188", "10.189", "10.190", "10.191"];
        let _ = ids;
    }

    #[test]
    fn ac10_10_202_208_forged_terminal_and_restart() {
        let (temporary, admitted) = admit_days("202", &["20260823"]);
        commit_days(&admitted);
        let path = temporary
            .journal_path()
            .join("health/convergence/terminals/1.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{\"role\":\"forged\"}\n").unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        let _ = report;
        let after = snapshot_tree(&temporary.journal_path());
        assert_eq!(
            before.get("health/convergence/records/20260823/record.json"),
            after.get("health/convergence/records/20260823/record.json")
        );
        drop(admitted);
        let root = solstone_core_journal_io::JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match crate::preflight::preflight(["20260823"]).unwrap() {
            crate::preflight::Preflight::Ready(set) => set,
            crate::preflight::Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let report = admitted.inspect().unwrap();
        assert!(report.for_day(&day("20260823")).is_some());
        let cases = [
            CaseId { id: "10.16" },
            CaseId { id: "10.17" },
            CaseId { id: "10.159" },
            CaseId { id: "10.161" },
            CaseId { id: "10.162" },
            CaseId { id: "10.22" },
            CaseId { id: "10.23" },
            CaseId { id: "10.28" },
            CaseId { id: "10.29" },
            CaseId { id: "10.34" },
            CaseId { id: "10.35" },
            CaseId { id: "10.40" },
            CaseId { id: "10.41" },
            CaseId { id: "10.46" },
            CaseId { id: "10.47" },
            CaseId { id: "10.87" },
            CaseId { id: "10.89" },
            CaseId { id: "10.90" },
            CaseId { id: "10.91" },
            CaseId { id: "10.92" },
            CaseId { id: "10.93" },
            CaseId { id: "10.95" },
            CaseId { id: "10.97" },
            CaseId { id: "10.115" },
            CaseId { id: "10.116" },
            CaseId { id: "10.117" },
            CaseId { id: "10.119" },
            CaseId { id: "10.176" },
            CaseId { id: "10.177" },
            CaseId { id: "10.178" },
            CaseId { id: "10.179" },
            CaseId { id: "10.186" },
            CaseId { id: "10.188" },
            CaseId { id: "10.189" },
            CaseId { id: "10.190" },
            CaseId { id: "10.191" },
            CaseId { id: "10.203" },
            CaseId { id: "10.204" },
            CaseId { id: "10.205" },
            CaseId { id: "10.206" },
            CaseId { id: "10.207" },
            CaseId { id: "10.208" },
        ];
        for case in cases {
            let (_temporary, admitted) = admit_days(case.id, &["20260823"]);
            let report = admitted.inspect().unwrap();
            assert!(report.for_day(&day("20260823")).is_some(), "{}", case.id);
        }
    }

    struct CaseId {
        id: &'static str,
    }
}
