// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Birth-bound direct-parent admission for hosted supervisors.

use crate::process::{
    InspectResult, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};
use serde::{Deserialize, Serialize};

/// A host's declaration of the process which must remain this process's direct
/// parent. A PID alone is deliberately insufficient because it can be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeclaredParent {
    instance: ProcessInstance,
}

impl DeclaredParent {
    /// Capture this process's present direct parent using one native observer.
    pub fn capture_current() -> Result<Self, ParentAdmissionFailure> {
        let source = SystemProcessInstanceSource;
        let current = source.inspect(std::process::id());
        let InspectResult::Present {
            ppid: Some(parent_pid),
            ..
        } = current
        else {
            return Err(ParentAdmissionFailure::Unverifiable);
        };
        let InspectResult::Present { instance, .. } = source.inspect(parent_pid) else {
            return Err(ParentAdmissionFailure::NotLiveOrReused);
        };
        Ok(Self { instance })
    }

    pub fn from_instance(instance: ProcessInstance) -> Self {
        Self { instance }
    }

    pub fn instance(&self) -> ProcessInstance {
        self.instance
    }
}

/// A successfully admitted parent declaration, checked again at lifecycle
/// boundaries and while the hosted runtime is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentWatch {
    declared: DeclaredParent,
}

impl ParentWatch {
    pub fn admit(
        declared: DeclaredParent,
        source: &dyn ProcessInstanceSource,
    ) -> Result<Self, ParentAdmissionFailure> {
        let actual_ppid = match source.inspect(std::process::id()) {
            InspectResult::Present {
                ppid: Some(ppid), ..
            } => ppid,
            InspectResult::Present { ppid: None, .. } | InspectResult::Unverifiable => {
                return Err(ParentAdmissionFailure::Unverifiable);
            }
            InspectResult::Absent => return Err(ParentAdmissionFailure::NotLiveOrReused),
        };
        if actual_ppid != declared.instance.pid {
            return Err(ParentAdmissionFailure::DirectParentMismatch {
                declared_pid: declared.instance.pid,
                actual_ppid,
            });
        }
        match source.observe(&declared.instance) {
            InstanceVerdict::SameLive { .. } => Ok(Self { declared }),
            InstanceVerdict::NotSameOrExited => Err(ParentAdmissionFailure::NotLiveOrReused),
            InstanceVerdict::Unverifiable => Err(ParentAdmissionFailure::Unverifiable),
        }
    }

    pub fn check(&self, source: &dyn ProcessInstanceSource) -> ParentWatchStatus {
        match source.observe(&self.declared.instance) {
            InstanceVerdict::SameLive { .. } => ParentWatchStatus::Live,
            InstanceVerdict::NotSameOrExited => {
                ParentWatchStatus::Lost(ParentLossReason::ExitedOrReused)
            }
            InstanceVerdict::Unverifiable => {
                ParentWatchStatus::Lost(ParentLossReason::Unverifiable)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentAdmissionFailure {
    DirectParentMismatch { declared_pid: u32, actual_ppid: u32 },
    NotLiveOrReused,
    Unverifiable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentWatchStatus {
    Live,
    Lost(ParentLossReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParentLossReason {
    ExitedOrReused,
    Unverifiable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{ExecutionState, InstanceCensus, ProcessBirth};

    struct Source {
        self_result: InspectResult,
        parent_result: InspectResult,
    }

    impl ProcessInstanceSource for Source {
        fn inspect(&self, pid: u32) -> InspectResult {
            if pid == std::process::id() {
                self.self_result
            } else {
                self.parent_result
            }
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    #[test]
    fn parent_admission_requires_the_direct_birth_bound_parent() {
        let expected = instance(42, 10);
        let source = Source {
            self_result: InspectResult::Present {
                instance: instance(std::process::id(), 1),
                execution: ExecutionState::Running,
                ppid: Some(42),
                pgid: None,
            },
            parent_result: InspectResult::Present {
                instance: expected,
                execution: ExecutionState::Running,
                ppid: Some(1),
                pgid: None,
            },
        };
        assert!(ParentWatch::admit(DeclaredParent::from_instance(expected), &source).is_ok());
    }

    #[test]
    fn parent_admission_fails_closed_when_observation_is_unverifiable() {
        let source = Source {
            self_result: InspectResult::Unverifiable,
            parent_result: InspectResult::Unverifiable,
        };
        assert_eq!(
            ParentWatch::admit(DeclaredParent::from_instance(instance(42, 10)), &source),
            Err(ParentAdmissionFailure::Unverifiable)
        );
    }
}
