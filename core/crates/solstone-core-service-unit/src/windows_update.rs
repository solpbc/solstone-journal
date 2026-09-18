// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a service update owes an existing Windows registration.
//!
//! The Task Scheduler accepts `TASK_UPDATE` only against an idle task, so
//! `journal service install` over a running registration used to refuse with
//! `task-not-idle-before-update` -- the one situation a reinstall exists for.
//! The update now stops the registration first and puts back the state the
//! owner left it in: a service that was running is started again, and one they
//! had deliberately stopped stays stopped.
//!
//! This is the journal's own cross-platform contract, not a Windows invention.
//! `service.rs::republish_linux_unit_for_route_repair` restarts a republished
//! systemd unit "only when it was already active, preserving an intentionally
//! inactive service"; this is the same sentence with the scheduler's idle
//! precondition added in front of it.

/// The steps an update owes the registration it is about to replace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsServiceUpdatePlan {
    /// Stop the running registration so the scheduler will accept the update.
    pub stop_before_update: bool,
    /// Start it again once the new profile is read back and verified.
    pub start_after_update: bool,
}

/// Reads the plan out of the registration's own pre-update snapshot.
///
/// Running means the scheduler is holding at least one instance of the task --
/// the same reading `service stop` requires before it will claim a verified
/// stop, so the plan never asks for a stop the stop itself would refuse. A
/// task that is neither running nor idle (disabled, or queued behind its own
/// trigger) is left alone on purpose: stopping cannot make a disabled task
/// idle, so the update is attempted and the refusal the owner reads is the
/// scheduler's own rather than one invented here.
pub fn windows_service_update_plan(running_instances: usize) -> WindowsServiceUpdatePlan {
    let running = running_instances > 0;
    WindowsServiceUpdatePlan {
        stop_before_update: running,
        start_after_update: running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_registration_is_stopped_for_the_update_and_started_again() {
        assert_eq!(
            windows_service_update_plan(1),
            WindowsServiceUpdatePlan {
                stop_before_update: true,
                start_after_update: true,
            }
        );
    }

    #[test]
    fn a_deliberately_stopped_service_is_updated_in_place_and_stays_stopped() {
        assert_eq!(
            windows_service_update_plan(0),
            WindowsServiceUpdatePlan {
                stop_before_update: false,
                start_after_update: false,
            }
        );
    }

    #[test]
    fn the_stop_and_the_restart_are_one_decision_about_the_owners_intent() {
        // Reading them from two places is how a service comes back up that the
        // owner had stopped, or stays down after an update that stopped it.
        for instances in 0..4 {
            let plan = windows_service_update_plan(instances);
            assert_eq!(plan.stop_before_update, plan.start_after_update);
        }
    }

    #[test]
    fn more_than_one_instance_still_reads_as_running() {
        // The stop itself refuses to guess which of several instances it owns;
        // the plan's job is only to say that a stop has to happen first.
        assert!(windows_service_update_plan(2).stop_before_update);
    }
}
