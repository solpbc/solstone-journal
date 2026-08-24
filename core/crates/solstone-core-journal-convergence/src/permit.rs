// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Live mutation permit. Commit and abort consume it by value.
//!
//! There is no `Permit::supersede`. A live permit that observes a verified
//! safe descendant refuses [`Permit::commit`] with [`Refusal::Superseded`];
//! the `superseded` terminal is published only by owner-free no-permit
//! recovery. `rejected` requires the sealed named-refusal fixture.

use crate::digest::RecordDigest;
use crate::error::ConvergenceError;
use crate::transaction::HeldDays;

/// Informational receipt after an exact terminal is durable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalReceipt {
    pub serial: u64,
    pub outcome: TerminalOutcome,
    pub digest: RecordDigest,
}

/// Terminal outcome. `Rejected` is reporting-only from the sealed named
/// refusal; the public permit never authors it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOutcome {
    Committed,
    Aborted,
    Superseded,
    Rejected,
}

/// Live mutation permit. Not `Clone`. Holds a borrow of the live day leases
/// for as long as [`HeldDays`] remains; dropping the leases drops the permit.
pub struct Permit<'s, 'a> {
    pub(crate) held: &'s mut HeldDays<'a>,
}

impl std::fmt::Debug for Permit<'_, '_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Permit")
            .field("serial", &self.held.serial)
            .finish_non_exhaustive()
    }
}

impl<'s, 'a> Permit<'s, 'a> {
    /// By-value. Writes `committed`. Absence after proven prepublication
    /// failure returns [`ConvergenceError::PreservedPrior`]; the caller still
    /// holds [`HeldDays`] and retries with [`HeldDays::proceed`].
    pub fn commit(self) -> Result<TerminalReceipt, ConvergenceError> {
        crate::decision::commit_with_grants(self.held)
    }

    /// By-value. Writes `aborted`. Same prepublication polarity as [`Self::commit`].
    pub fn abort(self) -> Result<TerminalReceipt, ConvergenceError> {
        crate::decision::abort_with_decision(self.held)
    }
}

pub(crate) fn outcome_name(outcome: TerminalOutcome) -> &'static str {
    match outcome {
        TerminalOutcome::Committed => "committed",
        TerminalOutcome::Aborted => "aborted",
        TerminalOutcome::Superseded => "superseded",
        TerminalOutcome::Rejected => "rejected",
    }
}

pub(crate) fn parse_outcome(name: &str) -> Option<TerminalOutcome> {
    match name {
        "committed" => Some(TerminalOutcome::Committed),
        "aborted" => Some(TerminalOutcome::Aborted),
        "superseded" => Some(TerminalOutcome::Superseded),
        "rejected" => Some(TerminalOutcome::Rejected),
        _ => None,
    }
}

#[cfg(test)]
mod sealed {
    pub trait OutcomeBoundTerminal {
        fn bound_outcome(&self) -> super::TerminalOutcome;
    }

    pub trait NamedRefusal {
        fn bind_refusal(&self) -> super::TerminalOutcome;
    }
}

/// Successor-only outcome-bound terminal authority. No public constructor.
#[cfg(test)]
pub(crate) struct BaseSuccessorCommit {
    serial: u64,
    intent_digest: String,
}

/// Successor-only abort authority. No public constructor.
#[cfg(test)]
pub(crate) struct BaseSuccessorAbort {
    serial: u64,
    intent_digest: String,
}

/// Sealed named-refusal authority. No public constructor.
#[cfg(test)]
pub(crate) struct BaseNamedRefusal {
    serial: u64,
    intent_digest: String,
}

#[cfg(test)]
impl sealed::OutcomeBoundTerminal for BaseSuccessorCommit {
    fn bound_outcome(&self) -> TerminalOutcome {
        TerminalOutcome::Committed
    }
}

#[cfg(test)]
impl sealed::OutcomeBoundTerminal for BaseSuccessorAbort {
    fn bound_outcome(&self) -> TerminalOutcome {
        TerminalOutcome::Aborted
    }
}

#[cfg(test)]
impl sealed::NamedRefusal for BaseNamedRefusal {
    fn bind_refusal(&self) -> TerminalOutcome {
        TerminalOutcome::Rejected
    }
}

#[cfg(test)]
impl BaseSuccessorCommit {
    pub(crate) fn bind(admitted: &crate::preflight::Admitted) -> Result<Self, ConvergenceError> {
        let (serial, intent_digest) = crate::terminal::bind_successor_identity(admitted)?;
        Ok(Self {
            serial,
            intent_digest,
        })
    }

    pub(crate) fn terminate(
        self,
        admitted: &crate::preflight::Admitted,
    ) -> Result<TerminalReceipt, ConvergenceError> {
        let outcome = sealed::OutcomeBoundTerminal::bound_outcome(&self);
        crate::terminal::publish_from_successor(admitted, self.serial, &self.intent_digest, outcome)
    }

    pub(crate) fn terminate_as(
        self,
        admitted: &crate::preflight::Admitted,
        outcome: TerminalOutcome,
    ) -> Result<TerminalReceipt, ConvergenceError> {
        if outcome != TerminalOutcome::Committed {
            return Err(ConvergenceError::Refused(
                crate::error::Refusal::WrongOutcome,
            ));
        }
        self.terminate(admitted)
    }

    pub(crate) fn with_mutated_digest(mut self, digest: String) -> Self {
        self.intent_digest = digest;
        self
    }
}

#[cfg(test)]
impl BaseSuccessorAbort {
    pub(crate) fn bind(admitted: &crate::preflight::Admitted) -> Result<Self, ConvergenceError> {
        let (serial, intent_digest) = crate::terminal::bind_successor_identity(admitted)?;
        Ok(Self {
            serial,
            intent_digest,
        })
    }

    pub(crate) fn terminate(
        self,
        admitted: &crate::preflight::Admitted,
    ) -> Result<TerminalReceipt, ConvergenceError> {
        let outcome = sealed::OutcomeBoundTerminal::bound_outcome(&self);
        crate::terminal::publish_from_successor(admitted, self.serial, &self.intent_digest, outcome)
    }

    pub(crate) fn terminate_as(
        self,
        admitted: &crate::preflight::Admitted,
        outcome: TerminalOutcome,
    ) -> Result<TerminalReceipt, ConvergenceError> {
        if outcome != TerminalOutcome::Aborted {
            return Err(ConvergenceError::Refused(
                crate::error::Refusal::WrongOutcome,
            ));
        }
        self.terminate(admitted)
    }
}

#[cfg(test)]
impl BaseNamedRefusal {
    pub(crate) fn bind(admitted: &crate::preflight::Admitted) -> Result<Self, ConvergenceError> {
        let (serial, intent_digest) = crate::terminal::bind_successor_identity(admitted)?;
        Ok(Self {
            serial,
            intent_digest,
        })
    }

    pub(crate) fn terminate(
        self,
        admitted: &crate::preflight::Admitted,
    ) -> Result<TerminalReceipt, ConvergenceError> {
        let _ = sealed::NamedRefusal::bind_refusal(&self);
        crate::terminal::publish_from_named_refusal(admitted, self.serial, &self.intent_digest)
    }
}

#[cfg(test)]
// Tests plant and inspect journal files via std::fs; clippy.toml forbids those in production.
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use crate::error::Refusal;
    use crate::layout::DayKey;
    use crate::preflight::{Preflight, preflight};
    use crate::publish::{
        PreparedCompletionAuthority, PreparedLaterDirtyAuthority, publish_kind_for_test,
    };
    use crate::schema::Terminal;
    use crate::test_support::{
        PublishFault, admit_days, continue_ok, continue_with_fault, fail_after, snapshot_tree,
    };
    use solstone_core_journal_io::JournalRoot;
    use std::fs;
    use std::path::Path;

    fn sample_day() -> DayKey {
        DayKey::parse("20260823").unwrap()
    }

    fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> T {
        let bytes = fs::read(path).unwrap();
        let trimmed = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        serde_json::from_slice(trimmed).unwrap()
    }

    #[test]
    fn ac10_10_131_intent_before_active_no_permit() {
        let (temporary, admitted) = admit_days("131", &["20260823"]);
        let before = snapshot_tree(&temporary.journal_path());
        let (held, error) = continue_with_fault(&admitted, PublishFault::AfterIntent);
        assert!(matches!(error, ConvergenceError::PreservedPrior { .. }));
        let after = snapshot_tree(&temporary.journal_path());
        assert!(after.contains_key("health/convergence/intents/1.json"));
        assert!(!after.contains_key("health/convergence/actives/1.json"));
        assert!(!after.contains_key("health/convergence/terminals/1.json"));
        assert!(after.len() >= before.len());
        drop(held);
    }

    #[test]
    fn ac10_10_132_wrong_live_permit_stale_instance() {
        let (_temporary, admitted) = admit_days("132", &["20260823"]);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let held = admitted.begin(owner).unwrap();
        let proof = crate::test_support::admit_proof(&held, held.owner()).unwrap();
        drop(held);
        let owner = crate::test_support::prepared_owner(&admitted).unwrap();
        let mut held = admitted.begin(owner).unwrap();
        let error = held.continue_with(proof).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::StaleLease)
        ));
    }

    #[test]
    fn ac10_10_164_165_prepublication_returns_permit_via_proceed() {
        for (id, outcome, fault_commit) in [
            ("10.164", TerminalOutcome::Committed, true),
            ("10.165", TerminalOutcome::Aborted, false),
        ] {
            let (temporary, admitted) = admit_days(id, &["20260823"]);
            let mut held = continue_ok(&admitted);
            let before = snapshot_tree(&temporary.journal_path());
            let _guard = fail_after(PublishFault::AfterTerminalPrepub);
            let permit = held.proceed().unwrap();
            let error = if fault_commit {
                permit.commit().unwrap_err()
            } else {
                permit.abort().unwrap_err()
            };
            assert!(
                matches!(error, ConvergenceError::PreservedPrior { .. }),
                "{id} {error:?}"
            );
            let after = snapshot_tree(&temporary.journal_path());
            assert!(!after.contains_key("health/convergence/terminals/1.json"));
            assert_eq!(
                after.get("health/convergence/records/20260823/record.json"),
                before.get("health/convergence/records/20260823/record.json"),
                "{id} dirty must not roll back"
            );
            let permit = held.proceed().unwrap();
            let receipt = if matches!(outcome, TerminalOutcome::Committed) {
                permit.commit().unwrap()
            } else {
                permit.abort().unwrap()
            };
            assert_eq!(receipt.outcome, outcome, "{id}");
            assert_eq!(receipt.serial, 1);
        }
    }

    #[test]
    fn ac10_10_166_171_terminal_fault_polarity() {
        struct Case {
            id: &'static str,
            commit: bool,
            fault: PublishFault,
            must: &'static [&'static str],
            must_not: &'static [&'static str],
            consume: bool,
        }
        let cases = [
            Case {
                id: "10.166",
                commit: true,
                fault: PublishFault::AfterTerminalSync,
                must: &["health/convergence/days/20260823.clear.json"],
                must_not: &["health/convergence/terminals/1.json"],
                consume: true,
            },
            Case {
                id: "10.167",
                commit: false,
                fault: PublishFault::AfterTerminalSync,
                must: &["health/convergence/days/20260823.clear.json"],
                must_not: &["health/convergence/terminals/1.json"],
                consume: true,
            },
            Case {
                id: "10.168",
                commit: true,
                fault: PublishFault::AfterTerminal,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/days/20260823.clear.json"],
                consume: true,
            },
            Case {
                id: "10.169",
                commit: false,
                fault: PublishFault::AfterTerminal,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/days/20260823.clear.json"],
                consume: true,
            },
            Case {
                id: "10.170",
                commit: true,
                fault: PublishFault::AfterActiveClear,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/actives/1.json"],
                consume: true,
            },
            Case {
                id: "10.171",
                commit: false,
                fault: PublishFault::AfterActiveClear,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/actives/1.json"],
                consume: true,
            },
        ];
        for case in cases {
            let (temporary, admitted) = admit_days(case.id, &["20260823"]);
            let mut held = continue_ok(&admitted);
            let before = snapshot_tree(&temporary.journal_path());
            let _guard = fail_after(case.fault);
            let permit = held.proceed().unwrap();
            let result = if case.commit {
                permit.commit()
            } else {
                permit.abort()
            };
            if case.consume {
                assert!(
                    result.is_ok()
                        || matches!(result, Err(ConvergenceError::PreservedPrior { .. })),
                    "{id} {result:?}",
                    id = case.id
                );
            }
            let after = snapshot_tree(&temporary.journal_path());
            for path in case.must {
                assert!(after.contains_key(*path), "{} missing {path}", case.id);
            }
            for path in case.must_not {
                assert!(!after.contains_key(*path), "{} unexpected {path}", case.id);
            }
            assert_eq!(
                after.get("health/convergence/records/20260823/record.json"),
                before.get("health/convergence/records/20260823/record.json"),
                "{} no-rollback",
                case.id
            );
        }
    }

    #[test]
    fn ac10_section_54_t0_t7_artifact_sets() {
        struct Case {
            id: &'static str,
            fault: PublishFault,
            must: &'static [&'static str],
            must_not: &'static [&'static str],
        }
        let cases = [
            Case {
                id: "AC10-5.4-AfterTerminalPrepub",
                fault: PublishFault::AfterTerminalPrepub,
                must: &["health/convergence/actives/1.json"],
                must_not: &["health/convergence/terminals/1.json"],
            },
            Case {
                id: "AC10-5.4-AfterTerminal",
                fault: PublishFault::AfterTerminal,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/days/20260823.clear.json"],
            },
            Case {
                id: "AC10-5.4-AfterActiveClear",
                fault: PublishFault::AfterActiveClear,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/actives/1.json"],
            },
            Case {
                id: "AC10-5.4-AfterIntentClear",
                fault: PublishFault::AfterIntentClear,
                must: &["health/convergence/terminals/1.json"],
                must_not: &["health/convergence/intents/1.json"],
            },
            Case {
                id: "AC10-5.4-AfterMemberA",
                fault: PublishFault::AfterMemberA,
                must: &["health/convergence/days/20260823.clear.json"],
                must_not: &[
                    "health/convergence/days/20260824.clear.json",
                    "health/convergence/clearance/1.barrier.json",
                ],
            },
            Case {
                id: "AC10-5.4-AfterMemberB",
                fault: PublishFault::AfterMemberB,
                must: &[
                    "health/convergence/days/20260823.clear.json",
                    "health/convergence/days/20260824.clear.json",
                ],
                must_not: &["health/convergence/clearance/1.barrier.json"],
            },
            Case {
                id: "AC10-5.4-AfterBarrier",
                fault: PublishFault::AfterBarrier,
                must: &["health/convergence/clearance/1.barrier.json"],
                must_not: &[],
            },
            Case {
                id: "AC10-5.4-AfterTerminalEvict",
                fault: PublishFault::AfterTerminalEvict,
                must: &["health/convergence/clearance/1.barrier.json"],
                must_not: &["health/convergence/terminals/1.json"],
            },
        ];
        for case in cases {
            let two_day = case.id.contains("Member")
                || case.id.contains("Barrier")
                || case.id.contains("Evict");
            let days: &[&str] = if two_day {
                &["20260823", "20260824"]
            } else {
                &["20260823"]
            };
            let (temporary, admitted) = admit_days(case.id, days);
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
            for path in case.must {
                assert!(after.contains_key(*path), "{} missing {path}", case.id);
            }
            for path in case.must_not {
                assert!(!after.contains_key(*path), "{} unexpected {path}", case.id);
            }
        }
    }

    #[test]
    fn ac10_10_172_conflicting_terminal_no_overwrite() {
        let (temporary, admitted) = admit_days("172", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterTerminal);
        let permit = held.proceed().unwrap();
        permit.commit().unwrap_err();
        let path = temporary
            .journal_path()
            .join("health/convergence/terminals/1.json");
        let mut terminal: Terminal = read_json_file(&path);
        terminal.outcome = "aborted".to_owned();
        fs::write(&path, serde_json::to_vec(&terminal).unwrap()).unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let permit = held.proceed().unwrap();
        let error = permit.commit().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::OppositeTerminal)
                    | ConvergenceError::Refused(Refusal::ConflictingTerminal)
            ),
            "{error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_156_no_permit_superseded() {
        let (temporary, admitted) = admit_days("156", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &sample_day(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        let error = permit.commit().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::Superseded)
        ));
        drop(held);
        let report = admitted.inspect().unwrap();
        assert_eq!(report.terminal_outcome(), Some(TerminalOutcome::Superseded));
        assert!(report.awaiting().is_none());
        let tree = snapshot_tree(&temporary.journal_path());
        assert!(
            tree.contains_key("health/convergence/days/20260823.clear.json")
                || tree.contains_key("health/convergence/terminals/1.json")
        );
    }

    #[test]
    fn ac10_10_157_generic_rejection_refused() {
        let error = crate::terminal::attempt_generic_rejection().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::GenericRejection)
        ));
    }

    #[test]
    fn ac10_10_120_mixed_descendants_superseded() {
        let (temporary, admitted) = admit_days("120", &["20260823", "20260824"]);
        let mut held = continue_ok(&admitted);
        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260824").unwrap(),
            PreparedLaterDirtyAuthority,
        )
        .unwrap();
        let error = permit.commit().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::Superseded)
        ));
        drop(held);
        drop(admitted);
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823", "20260824"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let report = admitted.inspect().unwrap();
        assert_eq!(report.terminal_outcome(), Some(TerminalOutcome::Superseded));
        drop(admitted);
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260825", "20260826"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let next = continue_ok(&admitted);
        let snap = next.snapshot(&DayKey::parse("20260825").unwrap()).unwrap();
        assert_eq!(snap.dirty_by_transition_serial, 2);
    }

    #[test]
    fn ac10_10_123_124_swapped_and_stale_resolution() {
        let (temporary, admitted) = admit_days("123", &["20260823", "20260824"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterTerminal);
        let permit = held.proceed().unwrap();
        permit.commit().unwrap_err();
        let path = temporary
            .journal_path()
            .join("health/convergence/terminals/1.json");
        let original: Terminal = read_json_file(&path);
        let mut swapped = original.clone();
        let a = swapped.resolved.get("20260823").unwrap().clone();
        let b = swapped.resolved.get("20260824").unwrap().clone();
        swapped.resolved.insert("20260823".to_owned(), b);
        swapped.resolved.insert("20260824".to_owned(), a);
        fs::write(&path, {
            let mut bytes = serde_json::to_vec(&swapped).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let permit = held.proceed().unwrap();
        let error = permit.commit().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ConflictingTerminal)
                    | ConvergenceError::Refused(Refusal::StaleEvidence)
            ),
            "10.123 {error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));

        let mut stale = original;
        stale.resolved.get_mut("20260823").unwrap().record_revision = 99;
        fs::write(&path, {
            let mut bytes = serde_json::to_vec(&stale).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let permit = held.proceed().unwrap();
        let error = permit.commit().unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::ConflictingTerminal)
                    | ConvergenceError::Refused(Refusal::StaleEvidence)
            ),
            "10.124 {error:?}"
        );
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_121_122_named_refusal_vs_generic() {
        let (_temporary, admitted) = admit_days("121", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let refusal = BaseNamedRefusal::bind(&admitted).unwrap();
        let receipt = refusal.terminate(&admitted).unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Rejected);
        let error = crate::terminal::attempt_generic_rejection().unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::GenericRejection)
        ));
    }

    #[test]
    fn ac10_10_158_163_successor_terminalization() {
        let (temporary, admitted) = admit_days("158", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let successor = BaseSuccessorCommit::bind(&admitted).unwrap();
        let receipt = successor.terminate(&admitted).unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Committed);
        assert_ne!(before, snapshot_tree(&temporary.journal_path()));

        let (_temporary_abort, admitted) = admit_days("159", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let successor = BaseSuccessorAbort::bind(&admitted).unwrap();
        let receipt = successor.terminate(&admitted).unwrap();
        assert_eq!(receipt.outcome, TerminalOutcome::Aborted);

        let (_temporary, admitted) = admit_days("161", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let successor = BaseSuccessorCommit::bind(&admitted).unwrap();
        let error = successor
            .terminate_as(&admitted, TerminalOutcome::Aborted)
            .unwrap_err();
        let abort = BaseSuccessorAbort::bind(&admitted).unwrap();
        let abort_error = abort
            .terminate_as(&admitted, TerminalOutcome::Committed)
            .unwrap_err();
        assert!(matches!(
            abort_error,
            ConvergenceError::Refused(Refusal::WrongOutcome)
        ));
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::WrongOutcome)
        ));

        let (temporary, admitted) = admit_days("162", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _guard = fail_after(PublishFault::AfterTerminal);
        let permit = held.proceed().unwrap();
        permit.commit().unwrap_err();
        drop(held);
        let successor = BaseSuccessorAbort::bind(&admitted).unwrap();
        let error = successor.terminate(&admitted).unwrap_err();
        assert!(
            matches!(
                error,
                ConvergenceError::Refused(Refusal::OppositeTerminal)
                    | ConvergenceError::Refused(Refusal::ConflictingTerminal)
            ),
            "{error:?}"
        );
        let _ = temporary;

        let (_temporary, admitted) = admit_days("163", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let successor = BaseSuccessorCommit::bind(&admitted)
            .unwrap()
            .with_mutated_digest("0".repeat(64));
        let error = successor.terminate(&admitted).unwrap_err();
        assert!(matches!(
            error,
            ConvergenceError::Refused(Refusal::IntentMismatch)
        ));
    }

    #[test]
    fn ac10_10_160_serialized_decision_no_constructor() {
        let (temporary, admitted) = admit_days("160", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let intent_bytes = fs::read(
            temporary
                .journal_path()
                .join("health/convergence/intents/1.json"),
        )
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let _ = intent_bytes;
        let report = admitted.inspect().unwrap();
        assert!(report.awaiting().is_some());
        assert!(report.terminal_outcome().is_none());
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
    }

    #[test]
    fn ac10_10_180_181_passive_restart_no_mint() {
        let (temporary, admitted) = admit_days("180", &["20260823"]);
        let mut held = continue_ok(&admitted);
        let _ = held.proceed().unwrap();
        drop(held);
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        assert!(report.awaiting().is_some());
        assert_eq!(report.awaiting().unwrap().serial(), 1);
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        drop(admitted);
        let root = JournalRoot::open(&temporary.journal_path()).unwrap();
        let set = match preflight(["20260823"]).unwrap() {
            Preflight::Ready(set) => set,
            Preflight::Empty => panic!("days"),
        };
        let admitted = set.admit(root).unwrap();
        let intent = fs::read(
            temporary
                .journal_path()
                .join("health/convergence/intents/1.json"),
        )
        .unwrap();
        let active = fs::read(
            temporary
                .journal_path()
                .join("health/convergence/actives/1.json"),
        )
        .unwrap();
        let before = snapshot_tree(&temporary.journal_path());
        let report = admitted.inspect().unwrap();
        assert!(report.awaiting().is_some());
        assert!(report.terminal_outcome().is_none());
        assert_eq!(before, snapshot_tree(&temporary.journal_path()));
        let _ = (intent, active);
    }

    #[test]
    fn ac10_10_drop_does_not_roll_back_dirty() {
        let (temporary, admitted) = admit_days("drop", &["20260823"]);
        let held = continue_ok(&admitted);
        let before = snapshot_tree(&temporary.journal_path());
        drop(held);
        let after = snapshot_tree(&temporary.journal_path());
        assert_eq!(
            before.get("health/convergence/records/20260823/record.json"),
            after.get("health/convergence/records/20260823/record.json")
        );
        assert!(after.contains_key("health/convergence/records/20260823/record.json"));
    }

    #[test]
    fn ac10_10_unresolved_sibling_stays_pending() {
        let (_temporary, admitted) = admit_days("pending", &["20260823", "20260824"]);
        let mut held = continue_ok(&admitted);
        let permit = held.proceed().unwrap();
        publish_kind_for_test(
            &permit.held.admitted.store,
            &permit.held.locks,
            &DayKey::parse("20260823").unwrap(),
            PreparedCompletionAuthority,
        )
        .unwrap();
        let _ = permit;
        drop(held);
        fs::remove_file(
            _temporary
                .journal_path()
                .join("health/convergence/records/20260824/record.json"),
        )
        .unwrap();
        let report = admitted.inspect().unwrap();
        assert!(
            report.awaiting().is_some(),
            "descendant + unresolved stays pending"
        );
        assert!(report.terminal_outcome().is_none());
    }
}
