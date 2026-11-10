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

pub const WINDOWS_UPDATE_INTENT_SCHEMA: &str = "solstone-windows-update-intent-v1";

/// The service's run and enabled state before an update quiesce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnerRunIntent {
    pub was_running: bool,
    pub was_enabled: bool,
}

/// The settings applied to Task Scheduler before stopping the task for an update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsUpdateQuiesce {
    pub disable_before_stop: bool,
}

/// A quiesce before an update disables the task before stopping it.
pub const fn windows_update_quiesce() -> WindowsUpdateQuiesce {
    WindowsUpdateQuiesce {
        disable_before_stop: true,
    }
}

/// The restore action derived from a saved owner intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsUpdateRestore {
    pub start: bool,
    pub enable_without_start: bool,
}

/// Derive the restore action from owner run intent.
pub fn windows_update_restore(intent: OwnerRunIntent) -> WindowsUpdateRestore {
    WindowsUpdateRestore {
        start: intent.was_running,
        enable_without_start: !intent.was_running && intent.was_enabled,
    }
}

/// Derive the restore action after a failed stop attempt.
pub fn windows_update_stop_failure_restore(was_enabled: bool) -> WindowsUpdateRestore {
    WindowsUpdateRestore {
        start: false,
        enable_without_start: was_enabled,
    }
}

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct StoredUpdateIntent<'a> {
    schema: &'a str,
    was_running: bool,
    was_enabled: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadUpdateIntent {
    schema: String,
    was_running: bool,
    was_enabled: bool,
}

/// Parse serialized update intent bytes, returning None on unknown fields,
/// wrong schema, or invalid JSON.
pub fn parse_windows_update_intent(bytes: &[u8]) -> Option<OwnerRunIntent> {
    let read: ReadUpdateIntent = serde_json::from_slice(bytes).ok()?;
    if read.schema != WINDOWS_UPDATE_INTENT_SCHEMA {
        return None;
    }
    Some(OwnerRunIntent {
        was_running: read.was_running,
        was_enabled: read.was_enabled,
    })
}

/// Serialize owner run intent using the canonical schema.
pub fn windows_update_intent_bytes(intent: OwnerRunIntent) -> Vec<u8> {
    let stored = StoredUpdateIntent {
        schema: WINDOWS_UPDATE_INTENT_SCHEMA,
        was_running: intent.was_running,
        was_enabled: intent.was_enabled,
    };
    serde_json::to_vec(&stored).expect("serialize update intent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_update_quiesce_disables_before_stop() {
        assert!(windows_update_quiesce().disable_before_stop);
    }

    #[test]
    fn windows_update_restore_four_rows() {
        assert_eq!(
            windows_update_restore(OwnerRunIntent {
                was_running: true,
                was_enabled: true,
            }),
            WindowsUpdateRestore {
                start: true,
                enable_without_start: false,
            }
        );
        assert_eq!(
            windows_update_restore(OwnerRunIntent {
                was_running: true,
                was_enabled: false,
            }),
            WindowsUpdateRestore {
                start: true,
                enable_without_start: false,
            }
        );
        assert_eq!(
            windows_update_restore(OwnerRunIntent {
                was_running: false,
                was_enabled: true,
            }),
            WindowsUpdateRestore {
                start: false,
                enable_without_start: true,
            }
        );
        assert_eq!(
            windows_update_restore(OwnerRunIntent {
                was_running: false,
                was_enabled: false,
            }),
            WindowsUpdateRestore {
                start: false,
                enable_without_start: false,
            }
        );
    }

    #[test]
    fn windows_update_stop_failure_restore_rows() {
        assert_eq!(
            windows_update_stop_failure_restore(true),
            WindowsUpdateRestore {
                start: false,
                enable_without_start: true,
            }
        );
        assert_eq!(
            windows_update_stop_failure_restore(false),
            WindowsUpdateRestore {
                start: false,
                enable_without_start: false,
            }
        );
    }

    #[test]
    fn windows_update_intent_codec_round_trip() {
        let fixture = b"{\"schema\":\"solstone-windows-update-intent-v1\",\"was_running\":true,\"was_enabled\":true}";
        let parsed = parse_windows_update_intent(fixture).expect("valid fixture parses");
        assert_eq!(
            parsed,
            OwnerRunIntent {
                was_running: true,
                was_enabled: true,
            }
        );
        let bytes = windows_update_intent_bytes(parsed);
        let reparsed = parse_windows_update_intent(&bytes).expect("serialized bytes parse");
        assert_eq!(parsed, reparsed);

        // Unknown fields rejected
        let with_unknown = b"{\"schema\":\"solstone-windows-update-intent-v1\",\"was_running\":true,\"was_enabled\":true,\"unknown\":1}";
        assert!(parse_windows_update_intent(with_unknown).is_none());

        // Wrong schema rejected
        let wrong_schema =
            b"{\"schema\":\"other-schema\",\"was_running\":true,\"was_enabled\":true}";
        assert!(parse_windows_update_intent(wrong_schema).is_none());

        // Bad JSON rejected
        assert!(parse_windows_update_intent(b"not valid json").is_none());
    }

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
