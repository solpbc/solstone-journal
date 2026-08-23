// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only recovery report and the shared discovery contract.
//!
//! [`RecoveryReport`] and [`AwaitingOwnerDecision`] hold only owned plain
//! data. They do not hold a [`solstone_core_journal_io::JournalRoot`], file
//! descriptor, lock, owner binding, claim-admission proof, permit, or borrow
//! of [`crate::Admitted`]. [`Admitted::inspect`] releases every lock before
//! constructing them. Neither type has `resume`, `mint`, `proceed`,
//! `commit`, `permit`, `from_digest`, or `from_bytes`.

use std::ffi::OsStr;

use serde::Deserialize;
use solstone_core_journal_io::read_bytes_bound;

use crate::error::{ConvergenceError, DurableRole, Refusal};
use crate::init::open_store_dirs;
use crate::intent::{open_intents_dir, read_intent};
use crate::layout::{DayKey, intent_name};
use crate::lock::acquire_days_with_timeout;
use crate::permit::TerminalOutcome;
use crate::preflight::{Admitted, CanonicalDaySet, canonicalize_discovered};
use crate::publish::inspect_against_proposed;
use crate::store::{DaySnapshot, LoadDay, PendingKind};

/// Observational awaiting-owner state. Owned data only; no capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AwaitingOwnerDecision {
    serial: u64,
    intent_digest: String,
    days: Vec<DayKey>,
    stage: AwaitingStage,
}

/// Stage recorded on [`AwaitingOwnerDecision`]. Informational only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AwaitingStage {
    AfterProjection,
}

impl AwaitingOwnerDecision {
    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn intent_digest(&self) -> &str {
        &self.intent_digest
    }

    pub fn days(&self) -> &[DayKey] {
        &self.days
    }

    pub fn stage(&self) -> AwaitingStage {
        self.stage
    }
}

/// Read-only per-journal recovery classification. `Clone`, no capabilities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    days: Vec<DayStoreRecovery>,
    awaiting: Option<AwaitingOwnerDecision>,
    terminal_outcome: Option<TerminalOutcome>,
}

/// One day's store-stage verdict. Owned data only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DayStoreRecovery {
    pub day: DayKey,
    pub verdict: StoreVerdict,
}

/// Store-stage recovery verdict (AC3/AC4).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreVerdict {
    Genesis,
    Published(DaySnapshot),
    WitnessAheadOfHead,
    HeadAheadOfRecord,
    HeadedDescendant {
        head_revision: u64,
        proposed_revision: u64,
    },
    Unknown {
        role: DurableRole,
    },
    NoPermit {
        role: DurableRole,
    },
}

impl RecoveryReport {
    pub fn days(&self) -> &[DayStoreRecovery] {
        &self.days
    }

    pub fn for_day(&self, day: &DayKey) -> Option<&DayStoreRecovery> {
        self.days.iter().find(|entry| &entry.day == day)
    }

    pub fn awaiting(&self) -> Option<&AwaitingOwnerDecision> {
        self.awaiting.as_ref()
    }

    pub fn terminal_outcome(&self) -> Option<TerminalOutcome> {
        self.terminal_outcome
    }
}

impl Admitted {
    /// Read-only store classification of the admitted day set.
    /// Acquires day locks, classifies, and releases them before return.
    pub fn inspect(&self) -> Result<RecoveryReport, ConvergenceError> {
        self.store.revalidate()?;
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
        let mut days = Vec::with_capacity(self.days().len());
        for day in self.days() {
            let verdict = match self.store.load_day(&locks, day) {
                Ok(LoadDay::Genesis) => StoreVerdict::Genesis,
                Ok(LoadDay::Published(snapshot)) => StoreVerdict::Published(snapshot),
                Ok(LoadDay::PublicationPending {
                    kind: PendingKind::WitnessAheadOfHead,
                }) => StoreVerdict::WitnessAheadOfHead,
                Ok(LoadDay::PublicationPending {
                    kind: PendingKind::HeadAheadOfRecord,
                }) => StoreVerdict::HeadAheadOfRecord,
                Ok(LoadDay::HeadedDescendant {
                    head_revision,
                    proposed_revision,
                }) => StoreVerdict::HeadedDescendant {
                    head_revision,
                    proposed_revision,
                },
                Err(ConvergenceError::Unknown {
                    role: DurableRole::EverWitness,
                }) => StoreVerdict::NoPermit {
                    role: DurableRole::EverWitness,
                },
                Err(ConvergenceError::Unknown { role }) => StoreVerdict::Unknown { role },
                Err(ConvergenceError::Refused(Refusal::NoPermit)) => StoreVerdict::NoPermit {
                    role: DurableRole::Intent,
                },
                Err(ConvergenceError::Refused(Refusal::WrongDay { .. })) => StoreVerdict::Unknown {
                    role: DurableRole::Head,
                },
                Err(error) => return Err(error),
            };
            days.push(DayStoreRecovery {
                day: day.clone(),
                verdict,
            });
        }
        let (awaiting, terminal_outcome) = classify_terminal_polarity(self, &dirs, &locks);
        drop(locks);
        Ok(RecoveryReport {
            days,
            awaiting,
            terminal_outcome,
        })
    }

    /// Classify one admitted day against an intent's proposed revision (AC4).
    pub fn inspect_proposed(
        &self,
        day: &DayKey,
        proposed_revision: u64,
    ) -> Result<StoreVerdict, ConvergenceError> {
        self.store.revalidate()?;
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
        let load = inspect_against_proposed(&self.store, &locks, day, proposed_revision);
        drop(locks);
        Ok(match load {
            Ok(LoadDay::Genesis) => StoreVerdict::Genesis,
            Ok(LoadDay::Published(snapshot)) => StoreVerdict::Published(snapshot),
            Ok(LoadDay::PublicationPending {
                kind: PendingKind::WitnessAheadOfHead,
            }) => StoreVerdict::WitnessAheadOfHead,
            Ok(LoadDay::PublicationPending {
                kind: PendingKind::HeadAheadOfRecord,
            }) => StoreVerdict::HeadAheadOfRecord,
            Ok(LoadDay::HeadedDescendant {
                head_revision,
                proposed_revision,
            }) => StoreVerdict::HeadedDescendant {
                head_revision,
                proposed_revision,
            },
            Err(ConvergenceError::Unknown {
                role: DurableRole::EverWitness,
            }) => StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            },
            Err(ConvergenceError::Unknown { role }) => StoreVerdict::Unknown { role },
            Err(ConvergenceError::Refused(Refusal::NoPermit)) => StoreVerdict::NoPermit {
                role: DurableRole::Intent,
            },
            Err(error) => return Err(error),
        })
    }
}

fn classify_terminal_polarity(
    admitted: &Admitted,
    dirs: &crate::init::StoreDirs,
    locks: &crate::lock::DayLockSet,
) -> (Option<AwaitingOwnerDecision>, Option<TerminalOutcome>) {
    let Ok(table) = crate::claim::current_table(&admitted.store, dirs) else {
        return (None, None);
    };
    let Some(serial) = crate::claim::shared_serial(&table, admitted.days()) else {
        return (None, None);
    };
    let Ok(Some(intent)) = read_intent(dirs, serial) else {
        return (None, None);
    };
    if let Ok(Some(terminal)) = crate::terminal::read_terminal(dirs, serial) {
        return (None, crate::permit::parse_outcome(&terminal.outcome));
    }
    let days = admitted.days();
    match crate::terminal::publish_no_permit_superseded(&admitted.store, locks, dirs, &intent, days)
    {
        Ok(Some(receipt)) => (None, Some(receipt.outcome)),
        Ok(None) => (
            Some(AwaitingOwnerDecision {
                serial,
                intent_digest: intent.intent_digest,
                days: days.to_vec(),
                stage: AwaitingStage::AfterProjection,
            }),
            None,
        ),
        Err(_) => (
            Some(AwaitingOwnerDecision {
                serial,
                intent_digest: intent.intent_digest,
                days: days.to_vec(),
                stage: AwaitingStage::AfterProjection,
            }),
            None,
        ),
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct DiscoveredDaySet {
    day_set: Vec<String>,
}

/// Discovery-only parse of a `day_set` field. Supplies no authority (AC3).
#[allow(dead_code)]
pub(crate) fn parse_discovered_day_set(bytes: &[u8]) -> Result<Vec<String>, ConvergenceError> {
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let parsed: DiscoveredDaySet =
        serde_json::from_slice(trimmed).map_err(|_| ConvergenceError::Unknown {
            role: DurableRole::Intent,
        })?;
    Ok(parsed.day_set)
}

/// Canonicalize discovered day strings. Empty/alias/duplicate refuse **before**
/// the caller acquires any day lock.
#[allow(dead_code)]
pub(crate) fn discovered_canonical_set(
    day_set: Vec<String>,
) -> Result<CanonicalDaySet, ConvergenceError> {
    canonicalize_discovered(day_set)
}

/// Shared intent/terminal discovery: parse without locks, refuse bad day sets,
/// acquire the derived set, byte-compare a re-read under those locks.
#[allow(dead_code)]
pub(crate) fn discover_then_reread(
    admitted: &Admitted,
    first_bytes: Vec<u8>,
    reread: impl FnOnce() -> Result<Option<Vec<u8>>, ConvergenceError>,
) -> Result<CanonicalDaySet, ConvergenceError> {
    let strings = parse_discovered_day_set(&first_bytes)?;
    let set = discovered_canonical_set(strings)?;
    #[cfg(test)]
    crate::test_support::run_after_discovery_hook();
    admitted.store.revalidate()?;
    let dirs = open_store_dirs(admitted.store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let locks = acquire_days_with_timeout(
        &dirs,
        set.days(),
        admitted.store.journal_id(),
        admitted.store.root_id(),
        admitted.store.object_identity(),
        admitted.lock_timeout(),
    )?;
    let second = reread()?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    drop(locks);
    if second != first_bytes {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Intent,
        });
    }
    let again = parse_discovered_day_set(&second)?;
    let again_set = discovered_canonical_set(again)?;
    if again_set.days() != set.days() {
        return Err(ConvergenceError::Refused(Refusal::DaySetChanged));
    }
    Ok(set)
}

/// Discover an intent by serial: parse day set, acquire, re-read.
#[allow(dead_code)]
pub(crate) fn recover_intent(
    admitted: &Admitted,
    serial: u64,
) -> Result<CanonicalDaySet, ConvergenceError> {
    admitted.store.revalidate()?;
    let dirs = open_store_dirs(admitted.store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let intents = open_intents_dir(&dirs)?.ok_or(ConvergenceError::Unknown {
        role: DurableRole::Intent,
    })?;
    let first = read_bytes_bound(&intents, &intent_name(serial)).map_err(|_| {
        ConvergenceError::Unknown {
            role: DurableRole::Intent,
        }
    })?;
    let Some(first) = first else {
        return Err(ConvergenceError::Unknown {
            role: DurableRole::Intent,
        });
    };
    discover_then_reread(admitted, first, || {
        let dirs = open_store_dirs(admitted.store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let intents = open_intents_dir(&dirs)?.ok_or(ConvergenceError::Unknown {
            role: DurableRole::Intent,
        })?;
        read_bytes_bound(&intents, &intent_name(serial)).map_err(|_| ConvergenceError::Unknown {
            role: DurableRole::Intent,
        })
    })
}

/// Discover a planted JSON object's `day_set` under a bound parent (intent or
/// terminal-shaped). Same contract as [`recover_intent`].
#[allow(dead_code)]
pub(crate) fn recover_named_json(
    admitted: &Admitted,
    parent: &str,
    name: &OsStr,
    role: DurableRole,
) -> Result<CanonicalDaySet, ConvergenceError> {
    admitted.store.revalidate()?;
    let dirs = open_store_dirs(admitted.store.root())?
        .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
    let directory = crate::walk::open_dir(&dirs.convergence, parent)?
        .ok_or(ConvergenceError::Unknown { role })?;
    let first =
        read_bytes_bound(&directory, name).map_err(|_| ConvergenceError::Unknown { role })?;
    let Some(first) = first else {
        return Err(ConvergenceError::Unknown { role });
    };
    discover_then_reread(admitted, first, move || {
        let dirs = open_store_dirs(admitted.store.root())?
            .ok_or(ConvergenceError::Refused(Refusal::Uninitialized))?;
        let directory = crate::walk::open_dir(&dirs.convergence, parent)?
            .ok_or(ConvergenceError::Unknown { role })?;
        read_bytes_bound(&directory, name).map_err(|_| ConvergenceError::Unknown { role })
    })
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::{ConvergenceError, DurableRole, Refusal};
    use crate::layout::TERMINALS;
    use crate::owner::{ClaimAdmission, OwnerBinding};
    use crate::test_support::{
        PublishFault, admit_days, after_discovery, continue_ok, continue_with_fault, days_dir,
        records_dir, sample_day, snapshot_tree,
    };
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    fn plant(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn slice_day<'a>(
        tree: &'a std::collections::BTreeMap<String, (u64, String)>,
        day: &str,
    ) -> Vec<(&'a str, &'a (u64, String))> {
        tree.iter()
            .filter(|(key, _)| key.contains(day))
            .map(|(key, value)| (key.as_str(), value))
            .collect()
    }

    #[test]
    fn ac10_10_10_11_split_phase_root_replacement() {
        for (name, poison) in [("ident", b"same-name".as_slice()), ("div", b"divergent")] {
            let (temporary, admitted) = admit_days(name, &["20260823"]);
            let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
            let mut held = admitted.begin(owner).unwrap();
            let journal = temporary.journal_path();
            let moved = temporary.path().join(format!("journal-moved-{name}"));
            fs::rename(&journal, &moved).unwrap();
            fs::create_dir(&journal).unwrap();
            fs::write(journal.join("poison"), poison).unwrap();
            let proof = ClaimAdmission::issue_from_base(&held, held.owner()).unwrap();
            held.continue_with(proof).unwrap();
            assert_eq!(fs::read(journal.join("poison")).unwrap(), poison);
            assert!(!journal.join("health").exists());
            let snap = held.snapshot(&sample_day()).unwrap();
            assert_eq!(snap.record_revision, 1);
        }
    }

    #[test]
    fn ac10_10_12_13_store_phase_root_replacement() {
        for (name, poison) in [
            ("store-ident", b"same-name".as_slice()),
            ("store-div", b"divergent"),
        ] {
            let (temporary, admitted) = admit_days(name, &["20260823"]);
            let held = continue_ok(&admitted);
            let journal = temporary.journal_path();
            let moved = temporary.path().join(format!("journal-moved-{name}"));
            fs::rename(&journal, &moved).unwrap();
            fs::create_dir(&journal).unwrap();
            fs::write(journal.join("poison"), poison).unwrap();
            match held.inspect_day(&sample_day()).unwrap() {
                crate::store::LoadDay::Published(snapshot) => {
                    assert_eq!(snapshot.record_revision, 1)
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(fs::read(journal.join("poison")).unwrap(), poison);
            assert!(!journal.join("health").exists());
            drop(held);
        }
    }

    #[test]
    fn ac10_10_133_g1_crash_before_ever_retry_creates() {
        let (temporary, admitted) = admit_days("133", &["20260823"]);
        let (mut held, error) = continue_with_fault(&admitted, PublishFault::AfterActive);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(!tree.contains_key("health/convergence/days/20260823.ever.wit.json"));
        held.proceed().unwrap();
        let snap = held.snapshot(&sample_day()).unwrap();
        assert_eq!(snap.record_revision, 1);
        assert!(
            temporary
                .journal_path()
                .join("health/convergence/days/20260823.ever.wit.json")
                .exists()
        );
    }

    #[test]
    fn ac10_s1_s2_s3_s4_store_fault_boundaries() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
            must: &'static [&'static str],
            must_not: &'static [&'static str],
        }
        let cases = [
            Case {
                id: "AC10-5.2-AfterEver",
                fault: PublishFault::AfterEver,
                must: &["health/convergence/days/20260823.ever.wit.json"],
                must_not: &[
                    "health/convergence/days/20260823.rev.1.wit.json",
                    "health/convergence/days/20260823.head.json",
                    "health/convergence/records/20260823/record.json",
                ],
            },
            Case {
                id: "AC10-5.2-AfterWitness",
                fault: PublishFault::AfterWitness,
                must: &[
                    "health/convergence/days/20260823.ever.wit.json",
                    "health/convergence/days/20260823.rev.1.wit.json",
                ],
                must_not: &[
                    "health/convergence/days/20260823.head.json",
                    "health/convergence/records/20260823/record.json",
                ],
            },
            Case {
                id: "AC10-5.2-AfterHead",
                fault: PublishFault::AfterHead,
                must: &[
                    "health/convergence/days/20260823.ever.wit.json",
                    "health/convergence/days/20260823.rev.1.wit.json",
                    "health/convergence/days/20260823.head.json",
                ],
                must_not: &["health/convergence/records/20260823/record.json"],
            },
            Case {
                id: "AC10-5.2-AfterRecord",
                fault: PublishFault::AfterRecord,
                must: &[
                    "health/convergence/days/20260823.head.json",
                    "health/convergence/records/20260823/record.json",
                ],
                must_not: &[],
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
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
        }
    }

    #[test]
    fn ac10_10_134_135_136_absent_ever_with_g1_survivor_no_permit() {
        let (temporary, admitted) = admit_days("134", &["20260823"]);
        let journal = temporary.journal_path();
        let days = days_dir(&temporary);
        let held = continue_ok(&admitted);
        fs::remove_file(days.join("20260823.ever.wit.json")).unwrap();
        drop(held);
        let before = snapshot_tree(&journal);
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            } => {}
            other => panic!("10.134 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&journal));

        let (temporary, admitted) = admit_days("135", &["20260823"]);
        let held = continue_ok(&admitted);
        fs::remove_file(days_dir(&temporary).join("20260823.ever.wit.json")).unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.rev.1.wit.json")).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            } => {}
            other => panic!("10.135 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        let (temporary, admitted) = admit_days("136", &["20260823"]);
        let held = continue_ok(&admitted);
        fs::remove_file(days_dir(&temporary).join("20260823.ever.wit.json")).unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.rev.1.wit.json")).unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.head.json")).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            } => {}
            other => panic!("10.136 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_137_138_139_140_pre_witness_unknown_no_witness_write() {
        let (temporary, admitted) = admit_days("137", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let record = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::write(
            records_dir(&temporary).join("20260823/record.json"),
            &record,
        )
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.advance_dirty().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(held);

        let (temporary, admitted) = admit_days("138", &["20260823"]);
        let mut held = continue_ok(&admitted);
        fs::remove_file(records_dir(&temporary).join("20260823/record.json")).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.advance_dirty().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(held);

        let (temporary, admitted) = admit_days("139", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let path = records_dir(&temporary).join("20260823/record.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["auxiliary_time"] = serde_json::Value::String("tampered".into());
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.advance_dirty().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(held);

        let (temporary, admitted) = admit_days("140", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let path = days_dir(&temporary).join("20260823.head.json");
        let mut head: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        head["record_digest"] = serde_json::Value::String("ab".repeat(32));
        fs::write(&path, serde_json::to_vec(&head).unwrap()).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.advance_dirty().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_141_142_143_144_pre_head_unknown_no_head_write() {
        let (temporary, admitted) = admit_days("141", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _inject = crate::test_support::fail_after(PublishFault::AfterWitness);
        let error = held.advance_dirty().unwrap_err();
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let path = records_dir(&temporary).join("20260823/record.json");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        record["dirty_generation"] = serde_json::json!(9);
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let error = held.proceed().unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        assert_eq!(
            before.get("health/convergence/days/20260823.head.json"),
            snapshot_tree(&temporary.journal_path())
                .get("health/convergence/days/20260823.head.json")
        );
    }

    #[test]
    fn ac10_10_145_146_147_g5_ever_exact_missing_wrong() {
        let (_temporary, admitted) = admit_days("145", &["20260823"]);
        let mut held = continue_ok(&admitted);
        for _ in 0..4 {
            held.advance_dirty().unwrap();
        }
        drop(held);
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::Published(snapshot) => assert_eq!(snapshot.dirty_generation, 5),
            other => panic!("10.145 {other:?}"),
        }

        let (temporary, admitted) = admit_days("146", &["20260823"]);
        let mut held = continue_ok(&admitted);
        for _ in 0..4 {
            held.advance_dirty().unwrap();
        }
        fs::remove_file(days_dir(&temporary).join("20260823.ever.wit.json")).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            } => {}
            other => panic!("10.146 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        let (temporary, admitted) = admit_days("147", &["20260823"]);
        let mut held = continue_ok(&admitted);
        for _ in 0..4 {
            held.advance_dirty().unwrap();
        }
        let path = days_dir(&temporary).join("20260823.ever.wit.json");
        let mut ever: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        ever["journal_id"] = serde_json::Value::String("wrong".into());
        fs::write(&path, serde_json::to_vec(&ever).unwrap()).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::NoPermit {
                role: DurableRole::EverWitness,
            }
            | StoreVerdict::Unknown { .. }
            | StoreVerdict::NoPermit { .. } => {}
            other => panic!("10.147 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_148_149_150_headed_descendant_vs_unheaded_and_gap() {
        let (_temporary, admitted) = admit_days("148", &["20260823"]);
        let mut held = continue_ok(&admitted);
        held.advance_dirty().unwrap();
        drop(held);
        let verdict = admitted.inspect_proposed(&sample_day(), 1).unwrap();
        match verdict {
            StoreVerdict::HeadedDescendant {
                head_revision: 2,
                proposed_revision: 1,
            } => {}
            other => panic!("10.148 expected HeadedDescendant, got {other:?}"),
        }

        let (temporary, admitted) = admit_days("149", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let days = days_dir(&temporary);
        let r1 = fs::read(days.join("20260823.rev.1.wit.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::write(days.join("20260823.rev.3.wit.json"), r1).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let verdict = admitted.inspect_proposed(&sample_day(), 1).unwrap();
        match verdict {
            StoreVerdict::HeadedDescendant { .. } => {
                panic!("10.149 planted unheaded extra must not classify as supersession")
            }
            StoreVerdict::Unknown { .. } | StoreVerdict::WitnessAheadOfHead => {}
            other => panic!("10.149 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        let (temporary, admitted) = admit_days("150", &["20260823"]);
        let mut held = continue_ok(&admitted);
        held.advance_dirty().unwrap();
        fs::remove_file(days_dir(&temporary).join("20260823.rev.1.wit.json")).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        match &report.for_day(&sample_day()).unwrap().verdict {
            StoreVerdict::Unknown {
                role: DurableRole::RevisionWitness,
            } => {}
            other => panic!("10.150 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_151_head_uncertainty_no_record_until_exact() {
        let (temporary, admitted) = admit_days("151", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let record_path = records_dir(&temporary).join("20260823/record.json");
        let before_record = fs::read(&record_path).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let _inject = crate::test_support::fail_next_dir_sync();
        let error = held.advance_dirty().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Unknown {
                    role: DurableRole::Head
                }
            ),
            "{error:?}"
        );
        let after = snapshot_tree(&temporary.journal_path());
        assert_eq!(
            before.get("health/convergence/records/20260823/record.json"),
            after.get("health/convergence/records/20260823/record.json")
        );
        assert_eq!(before_record, fs::read(&record_path).unwrap());
    }

    #[test]
    fn ac10_10_152_completion_then_later_dirty_preserves_first() {
        let (_temporary, admitted) = admit_days("152", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let day = sample_day();
        crate::publish::publish_kind_for_test(
            admitted.store(),
            held.lock_set(),
            &day,
            crate::publish::PreparedCompletionAuthority,
        )
        .unwrap();
        let completed = held.snapshot(&day).unwrap();
        assert_eq!(completed.completed_generation, completed.dirty_generation);
        held.advance_dirty().unwrap();
        let later = held.snapshot(&day).unwrap();
        assert_eq!(
            later.first_transition_serial,
            completed.first_transition_serial
        );
        assert_eq!(later.dirty_generation, completed.dirty_generation + 1);
        assert_eq!(later.completed_generation, completed.completed_generation);
        assert_ne!(
            later.dirty_by_transition_serial,
            completed.dirty_by_transition_serial
        );
    }

    #[test]
    fn ac10_10_153_stale_descendant_unknown_not_superseded() {
        let (temporary, admitted) = admit_days("153", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let g1 = fs::read(records_dir(&temporary).join("20260823/record.json")).unwrap();
        held.advance_dirty().unwrap();
        fs::write(records_dir(&temporary).join("20260823/record.json"), g1).unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let verdict = admitted.inspect_proposed(&sample_day(), 1);
        match verdict {
            Ok(StoreVerdict::HeadedDescendant { .. }) => {
                panic!("stale record must not be headed descendant")
            }
            Ok(StoreVerdict::Unknown { .. })
            | Ok(StoreVerdict::HeadAheadOfRecord)
            | Err(ConvergenceError::Unknown { .. }) => {}
            other => panic!("10.153 {other:?}"),
        }
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_18_to_44_lineage_copy_store_surfaces() {
        struct Case {
            id: &'static str,
            kind: &'static str,
            surface: &'static str,
        }
        let cases = [
            Case {
                id: "10.18",
                kind: "a",
                surface: "witness",
            },
            Case {
                id: "10.19",
                kind: "a",
                surface: "head",
            },
            Case {
                id: "10.20",
                kind: "a",
                surface: "record",
            },
            Case {
                id: "10.24",
                kind: "b",
                surface: "witness",
            },
            Case {
                id: "10.25",
                kind: "b",
                surface: "head",
            },
            Case {
                id: "10.26",
                kind: "b",
                surface: "record",
            },
            Case {
                id: "10.30",
                kind: "c",
                surface: "witness",
            },
            Case {
                id: "10.31",
                kind: "c",
                surface: "head",
            },
            Case {
                id: "10.32",
                kind: "c",
                surface: "record",
            },
            Case {
                id: "10.36",
                kind: "d",
                surface: "witness",
            },
            Case {
                id: "10.37",
                kind: "d",
                surface: "head",
            },
            Case {
                id: "10.38",
                kind: "d",
                surface: "record",
            },
            Case {
                id: "10.42",
                kind: "e",
                surface: "witness",
            },
            Case {
                id: "10.43",
                kind: "e",
                surface: "head",
            },
            Case {
                id: "10.44",
                kind: "e",
                surface: "record",
            },
        ];
        for case in cases {
            let dst_days_input: &[&str] = if case.kind == "b" {
                &["20260823", "20260824"]
            } else {
                &["20260823"]
            };
            let (src, admitted_src) = admit_days(&format!("{}-src", case.id), &["20260823"]);
            let held_src = continue_ok(&admitted_src);
            drop(held_src);
            let (dst, admitted_dst) = admit_days(&format!("{}-dst", case.id), dst_days_input);
            if case.kind == "e" {
                let (held_dst, error) =
                    continue_with_fault(&admitted_dst, PublishFault::AfterAdopt);
                assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
                drop(held_dst);
            } else {
                let held_dst = continue_ok(&admitted_dst);
                drop(held_dst);
            }
            let src_days = days_dir(&src);
            let dst_days = days_dir(&dst);
            let src_rec = records_dir(&src).join("20260823/record.json");
            let dst_rec = records_dir(&dst).join("20260823/record.json");
            let (from, to) = match (case.kind, case.surface) {
                ("b", "witness") => (
                    src_days.join("20260823.rev.1.wit.json"),
                    dst_days.join("20260824.rev.1.wit.json"),
                ),
                ("b", "head") => (
                    src_days.join("20260823.head.json"),
                    dst_days.join("20260824.head.json"),
                ),
                ("b", "record") => (
                    src_rec.clone(),
                    records_dir(&dst).join("20260824/record.json"),
                ),
                (_, "witness") => (
                    src_days.join("20260823.rev.1.wit.json"),
                    dst_days.join("20260823.rev.1.wit.json"),
                ),
                (_, "head") => (
                    src_days.join("20260823.head.json"),
                    dst_days.join("20260823.head.json"),
                ),
                (_, "record") => (src_rec.clone(), dst_rec.clone()),
                _ => panic!("{}", case.surface),
            };
            let mut bytes = fs::read(&from).unwrap();
            match case.kind {
                "a" | "b" => {}
                "c" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "journal_id".into(),
                            serde_json::Value::String("other-owner".into()),
                        );
                    }
                    bytes = serde_json::to_vec(&value).unwrap();
                }
                "d" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert(
                            "journal_id".into(),
                            serde_json::Value::String("mixed-j".into()),
                        );
                        if obj.contains_key("root_id") {
                            obj.insert(
                                "root_id".into(),
                                serde_json::Value::String("mixed-r".into()),
                            );
                        }
                    }
                    bytes = serde_json::to_vec(&value).unwrap();
                }
                "e" => {
                    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    let dest_ids: serde_json::Value = serde_json::from_slice(
                        &fs::read(dst_days.join("20260823.adopt.json")).unwrap(),
                    )
                    .unwrap();
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("journal_id".into(), dest_ids["journal_id"].clone());
                        obj.insert("root_id".into(), dest_ids["root_id"].clone());
                        obj.insert("adoption_id".into(), dest_ids["adoption_id"].clone());
                        obj.insert("day".into(), dest_ids["day"].clone());
                    }
                    bytes = serde_json::to_vec(&value).unwrap();
                }
                _ => panic!("{}", case.kind),
            }
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&to, &bytes).unwrap();
            let before = snapshot_tree(&dst.journal_path());
            let report = admitted_dst.inspect().unwrap();
            let inspect_day = if case.kind == "b" {
                crate::layout::DayKey::parse("20260824").unwrap()
            } else {
                sample_day()
            };
            match &report.for_day(&inspect_day).unwrap().verdict {
                StoreVerdict::Published(_) | StoreVerdict::Genesis => {
                    panic!(
                        "{} copy must refuse even if the bundle is internally consistent",
                        case.id
                    )
                }
                _ => {}
            }
            assert_eq!(
                before,
                snapshot_tree(&dst.journal_path()),
                "{} wrote during inspect",
                case.id
            );
            let _ = slice_day(&before, "20260823");
        }
    }

    #[test]
    fn ac10_10_192_193_duplicate_alias_intent_before_lock() {
        let (temporary, admitted) = admit_days("192", &["20260823"]);
        let intent_dir = temporary.journal_path().join("health/convergence/intents");
        plant(
            &intent_dir.join("1.json"),
            br#"{"day_set":["20260823","20260823"]}"#,
        );
        let before = snapshot_tree(&temporary.journal_path());
        let error = recover_intent(&admitted, 1).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::DuplicateDays)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        plant(&intent_dir.join("1.json"), br#"{"day_set":["2026-08-23"]}"#);
        let before = snapshot_tree(&temporary.journal_path());
        let error = recover_intent(&admitted, 1).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::NonCanonicalDays)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_194_195_duplicate_alias_terminal_before_lock() {
        let (temporary, admitted) = admit_days("194", &["20260823"]);
        let term_dir = temporary
            .journal_path()
            .join("health/convergence")
            .join(TERMINALS);
        plant(
            &term_dir.join("1.json"),
            br#"{"day_set":["20260823","20260823"]}"#,
        );
        let before = snapshot_tree(&temporary.journal_path());
        let error = recover_named_json(
            &admitted,
            TERMINALS,
            OsStr::new("1.json"),
            DurableRole::Terminal,
        )
        .unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::DuplicateDays)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        plant(&term_dir.join("1.json"), br#"{"day_set":["2026-08-23"]}"#);
        let before = snapshot_tree(&temporary.journal_path());
        let error = recover_named_json(
            &admitted,
            TERMINALS,
            OsStr::new("1.json"),
            DurableRole::Terminal,
        )
        .unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Refused(Refusal::NonCanonicalDays)),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_196_197_198_intent_swap_evict_dayset() {
        let (temporary, admitted) = admit_days("196", &["20260823"]);
        let path = temporary
            .journal_path()
            .join("health/convergence/intents/1.json");
        plant(&path, br#"{"day_set":["20260823"]}"#);
        let before = snapshot_tree(&temporary.journal_path());
        let swapped = path.clone();
        let _guard = after_discovery(move || {
            fs::write(&swapped, br#"{"day_set":["20260823"],"x":1}"#).unwrap();
        });
        let error = recover_intent(&admitted, 1).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        let after = snapshot_tree(&temporary.journal_path());
        assert!(after.contains_key("health/convergence/intents/1.json"));
        let _ = before;

        plant(&path, br#"{"day_set":["20260823"]}"#);
        let evicted = path.clone();
        let before = snapshot_tree(&temporary.journal_path());
        let _guard = after_discovery(move || {
            fs::remove_file(&evicted).unwrap();
        });
        let error = recover_intent(&admitted, 1).unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );
        assert!(
            !snapshot_tree(&temporary.journal_path())
                .contains_key("health/convergence/intents/1.json")
        );
        let _ = before;

        plant(&path, br#"{"day_set":["20260823"]}"#);
        let changed = path.clone();
        let before = snapshot_tree(&temporary.journal_path());
        let _guard = after_discovery(move || {
            fs::write(&changed, br#"{"day_set":["20260824"]}"#).unwrap();
        });
        let error = recover_intent(&admitted, 1).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::DaySetChanged)
                    | ConvergenceError::Unknown { .. }
            ),
            "{error:?}"
        );
        let _ = before;
    }

    #[test]
    fn ac10_10_199_200_201_terminal_swap_evict_dayset() {
        let (temporary, admitted) = admit_days("199", &["20260823"]);
        let path = temporary
            .journal_path()
            .join("health/convergence")
            .join(TERMINALS)
            .join("1.json");
        plant(&path, br#"{"day_set":["20260823"]}"#);
        let swapped = path.clone();
        let _guard = after_discovery(move || {
            fs::write(&swapped, br#"{"day_set":["20260823"],"n":1}"#).unwrap();
        });
        let error = recover_named_json(
            &admitted,
            TERMINALS,
            OsStr::new("1.json"),
            DurableRole::Terminal,
        )
        .unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );

        plant(&path, br#"{"day_set":["20260823"]}"#);
        let evicted = path.clone();
        let _guard = after_discovery(move || {
            fs::remove_file(&evicted).unwrap();
        });
        let error = recover_named_json(
            &admitted,
            TERMINALS,
            OsStr::new("1.json"),
            DurableRole::Terminal,
        )
        .unwrap_err();
        assert!(
            matches!(error, ConvergenceError::Unknown { .. }),
            "{error:?}"
        );

        plant(&path, br#"{"day_set":["20260823"]}"#);
        let changed = path.clone();
        let before = snapshot_tree(&temporary.journal_path());
        let _guard = after_discovery(move || {
            fs::write(&changed, br#"{"day_set":["20260824"]}"#).unwrap();
        });
        let error = recover_named_json(
            &admitted,
            TERMINALS,
            OsStr::new("1.json"),
            DurableRole::Terminal,
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::DaySetChanged)
                    | ConvergenceError::Unknown { .. }
            ),
            "{error:?}"
        );
        let _ = before;
    }

    #[test]
    fn recovery_report_is_read_only_and_releases_locks() {
        let (_temporary, admitted) = admit_days("report", &["20260823"]);
        let held = continue_ok(&admitted);
        drop(held);
        let report = admitted.inspect().unwrap();
        let cloned = report.clone();
        assert_eq!(report, cloned);
        let owner = OwnerBinding::issue_from_base(&admitted).unwrap();
        let _held = admitted.begin(owner).unwrap();
    }
}
