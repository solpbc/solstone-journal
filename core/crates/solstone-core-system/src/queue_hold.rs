// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Partition hold evaluation and proof logic.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::partition::Partition;
use crate::process::{InstanceVerdict, ProcessBirth, ProcessOwner};

/// Snapshot status of a held partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPartitionStatus {
    pub partition: Partition,
    pub reference: String,
    pub references: Vec<String>,
    pub command: Vec<String>,
    pub reasons: Vec<ReasonCode>,
    pub termination_error: Option<String>,
    pub snapshot_unavailable: bool,
    pub held_since_unix: u64,
}

/// Reason codes explaining why a partition hold cannot be released.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    RootLive,
    RootUnverifiable,
    BoundLive,
    BoundUnverifiable,
    GroupMemberLive,
    GroupQueryIncomplete,
    JobNotQuiescent,
    JobQueryFailed,
    WorkerEndedWithoutProof,
}

impl ReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RootLive => "root_live",
            Self::RootUnverifiable => "root_unverifiable",
            Self::BoundLive => "bound_live",
            Self::BoundUnverifiable => "bound_unverifiable",
            Self::GroupMemberLive => "group_member_live",
            Self::GroupQueryIncomplete => "group_query_incomplete",
            Self::JobNotQuiescent => "job_not_quiescent",
            Self::JobQueryFailed => "job_query_failed",
            Self::WorkerEndedWithoutProof => "worker_ended_without_proof",
        }
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The result of evaluating a partition hold against observation evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HoldProof {
    Proven,
    Unproven { reasons: Vec<ReasonCode> },
}

/// One live process-table row observed during a process-group query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    pub pid: u32,
    pub pgid: i32,
    pub uid: u32,
    pub birth: ProcessBirth,
}

/// The result of querying a task's process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupCensus {
    Incomplete,
    Complete(Vec<GroupMember>),
}

/// Observation of the task root process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootObservation {
    /// Gate (a) already satisfied: reaped, exit code recorded, or NotSameOrExited.
    Gone {
        birth_verifiable: bool,
        birth: Option<ProcessBirth>,
    },
    SameLive {
        birth: ProcessBirth,
    },
    Unverifiable {
        birth_verifiable: bool,
        birth: Option<ProcessBirth>,
    },
}

/// Platform observations gathered for hold evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformObservations {
    Unix {
        root: RootObservation,
        bound: Vec<InstanceVerdict>,
        group: GroupCensus,
        group_id: u32,
        owner_uid: u32,
    },
    Windows {
        job: Result<bool, ()>,
    },
}

/// Evaluate whether all processes associated with a task have ended.
pub fn evaluate_hold_proof(
    obs: PlatformObservations,
    worker_ended_without_proof: bool,
) -> HoldProof {
    let mut reasons = Vec::new();

    match obs {
        PlatformObservations::Unix {
            root,
            bound,
            group,
            group_id,
            owner_uid,
        } => {
            // Gate (a): root
            let (root_birth_verifiable, root_birth) = match root {
                RootObservation::Gone {
                    birth_verifiable,
                    birth,
                } => (birth_verifiable, birth),
                RootObservation::SameLive { birth } => {
                    reasons.push(ReasonCode::RootLive);
                    (true, Some(birth))
                }
                RootObservation::Unverifiable {
                    birth_verifiable,
                    birth,
                } => {
                    reasons.push(ReasonCode::RootUnverifiable);
                    (birth_verifiable, birth)
                }
            };

            // Gate (b): bound identities
            let mut bound_live = false;
            let mut bound_unverifiable = false;
            for verdict in bound {
                match verdict {
                    InstanceVerdict::SameLive { .. } => bound_live = true,
                    InstanceVerdict::Unverifiable => bound_unverifiable = true,
                    InstanceVerdict::NotSameOrExited => {}
                }
            }
            if bound_live {
                reasons.push(ReasonCode::BoundLive);
            }
            if bound_unverifiable {
                reasons.push(ReasonCode::BoundUnverifiable);
            }

            // Gate (c): group census
            match group {
                GroupCensus::Incomplete => {
                    reasons.push(ReasonCode::GroupQueryIncomplete);
                }
                GroupCensus::Complete(members) => {
                    let mut group_member_live = false;
                    for member in members {
                        if member.uid != owner_uid {
                            continue;
                        }
                        let is_reused_leader = member.pid == group_id
                            && member.pgid == (group_id as i32)
                            && root_birth_verifiable
                            && root_birth.is_some_and(|b| b != member.birth);

                        if is_reused_leader {
                            continue;
                        }

                        group_member_live = true;
                        break;
                    }
                    if group_member_live {
                        reasons.push(ReasonCode::GroupMemberLive);
                    }
                }
            }
        }
        PlatformObservations::Windows { job } => match job {
            Ok(true) => {}
            Ok(false) => reasons.push(ReasonCode::JobNotQuiescent),
            Err(()) => reasons.push(ReasonCode::JobQueryFailed),
        },
    }

    if worker_ended_without_proof {
        reasons.push(ReasonCode::WorkerEndedWithoutProof);
    }

    if reasons.is_empty() {
        HoldProof::Proven
    } else {
        HoldProof::Unproven { reasons }
    }
}

/// Re-check an unverifiable instance verdict against host process owner lookup.
pub fn verdict_after_owner_recheck(
    verdict: InstanceVerdict,
    owner: ProcessOwner,
    task_uid: u32,
) -> InstanceVerdict {
    if verdict != InstanceVerdict::Unverifiable {
        return verdict;
    }
    match owner {
        ProcessOwner::Absent => InstanceVerdict::NotSameOrExited,
        ProcessOwner::Uid(other) if other != task_uid && other != 0 => {
            InstanceVerdict::NotSameOrExited
        }
        ProcessOwner::Uid(_) | ProcessOwner::Unknown => InstanceVerdict::Unverifiable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ExecutionState;

    fn test_birth(token: u64) -> ProcessBirth {
        #[cfg(target_os = "linux")]
        {
            ProcessBirth::linux(token, 0, 100)
        }
        #[cfg(target_os = "macos")]
        {
            ProcessBirth::macos(token as i64)
        }
        #[cfg(windows)]
        {
            ProcessBirth::windows(token)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            ProcessBirth::linux(token, 0, 100)
        }
    }

    #[test]
    fn clean_exit_with_clean_census_is_proven() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![InstanceVerdict::NotSameOrExited],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn root_live_holds() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::SameLive {
                birth: test_birth(1),
            },
            bound: vec![],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::RootLive]
            }
        );
    }

    #[test]
    fn root_unverifiable_with_same_uid_holds() {
        let verdict = verdict_after_owner_recheck(
            InstanceVerdict::Unverifiable,
            ProcessOwner::Uid(1000),
            1000,
        );
        assert_eq!(verdict, InstanceVerdict::Unverifiable);
        let obs = PlatformObservations::Unix {
            root: RootObservation::Unverifiable {
                birth_verifiable: false,
                birth: None,
            },
            bound: vec![],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::RootUnverifiable]
            }
        );
    }

    #[test]
    fn root_unverifiable_with_different_uid_is_proven() {
        let verdict = verdict_after_owner_recheck(
            InstanceVerdict::Unverifiable,
            ProcessOwner::Uid(2000),
            1000,
        );
        assert_eq!(verdict, InstanceVerdict::NotSameOrExited);
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: false,
                birth: None,
            },
            bound: vec![],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn bound_live_holds() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![InstanceVerdict::SameLive {
                execution: ExecutionState::Running,
            }],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::BoundLive]
            }
        );
    }

    #[test]
    fn bound_unverifiable_with_same_uid_holds() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![InstanceVerdict::Unverifiable],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::BoundUnverifiable]
            }
        );
    }

    #[test]
    fn bound_unverifiable_with_different_uid_is_proven() {
        let verdict = verdict_after_owner_recheck(
            InstanceVerdict::Unverifiable,
            ProcessOwner::Uid(2000),
            1000,
        );
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![verdict],
            group: GroupCensus::Complete(Vec::new()),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn group_member_live_with_same_uid_holds() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![],
            group: GroupCensus::Complete(vec![GroupMember {
                pid: 105,
                pgid: 100,
                uid: 1000,
                birth: test_birth(5),
            }]),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::GroupMemberLive]
            }
        );
    }

    #[test]
    fn group_member_live_with_different_uid_is_proven() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![],
            group: GroupCensus::Complete(vec![GroupMember {
                pid: 105,
                pgid: 100,
                uid: 2000,
                birth: test_birth(5),
            }]),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn reused_pgid_leader_with_different_birth_is_proven() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![],
            group: GroupCensus::Complete(vec![GroupMember {
                pid: 100,
                pgid: 100,
                uid: 1000,
                birth: test_birth(999),
            }]),
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn incomplete_group_query_holds() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::Gone {
                birth_verifiable: true,
                birth: Some(test_birth(1)),
            },
            bound: vec![],
            group: GroupCensus::Incomplete,
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::GroupQueryIncomplete]
            }
        );
    }

    #[test]
    fn non_quiescent_windows_job_holds() {
        let obs = PlatformObservations::Windows { job: Ok(false) };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::JobNotQuiescent]
            }
        );
    }

    #[test]
    fn windows_job_query_error_holds() {
        let obs = PlatformObservations::Windows { job: Err(()) };
        assert_eq!(
            evaluate_hold_proof(obs, false),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::JobQueryFailed]
            }
        );
    }

    #[test]
    fn quiescent_windows_job_is_proven() {
        let obs = PlatformObservations::Windows { job: Ok(true) };
        assert_eq!(evaluate_hold_proof(obs, false), HoldProof::Proven);
    }

    #[test]
    fn worker_ended_without_proof_adds_reason() {
        let obs = PlatformObservations::Windows { job: Ok(true) };
        assert_eq!(
            evaluate_hold_proof(obs, true),
            HoldProof::Unproven {
                reasons: vec![ReasonCode::WorkerEndedWithoutProof]
            }
        );
    }

    #[test]
    fn multiple_failures_accumulate_all_reasons_in_canonical_order() {
        let obs = PlatformObservations::Unix {
            root: RootObservation::SameLive {
                birth: test_birth(1),
            },
            bound: vec![
                InstanceVerdict::SameLive {
                    execution: ExecutionState::Running,
                },
                InstanceVerdict::Unverifiable,
            ],
            group: GroupCensus::Incomplete,
            group_id: 100,
            owner_uid: 1000,
        };
        assert_eq!(
            evaluate_hold_proof(obs, true),
            HoldProof::Unproven {
                reasons: vec![
                    ReasonCode::RootLive,
                    ReasonCode::BoundLive,
                    ReasonCode::BoundUnverifiable,
                    ReasonCode::GroupQueryIncomplete,
                    ReasonCode::WorkerEndedWithoutProof,
                ]
            }
        );
    }
}
