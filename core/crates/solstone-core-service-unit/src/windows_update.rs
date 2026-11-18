// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a service update owes an existing Windows registration.
//!
//! The Task Scheduler accepts `TASK_UPDATE` only against an idle task, so
//! `journal service install` over a running registration used to refuse with
//! `task-not-idle-before-update` -- the one situation a reinstall exists for.
//! The update now stops the registration first and puts back the state the
//! owner left it in: a service that was running is started again, and one they
//! had deliberately stopped stays stopped. A registration that already carries
//! the profile is not rewritten, so a running one is not stopped at all.
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
    /// Write the new profile. A registration that already carries it is left
    /// as it is, apart from `Enabled`: rewriting it would change nothing, and
    /// for a running one it would cost a stop.
    pub update_registration: bool,
    /// Record `Enabled` on a running registration that is kept as it is but
    /// reads disabled: a running task is wanted by definition.
    pub enable_kept_registration: bool,
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
///
/// `registration_current` says the registration already carries the profile
/// the update would write, its `Enabled` flag aside
/// (`windows_registration_is_current`), and `registration_enabled` is that
/// flag. 🔴 A running, current registration is not stopped: after
/// `Update.exe apply` or a Setup over an installed build, the five-minute
/// recovery trigger can start the new build inside the post-update step's
/// window, and a stop issued then (it disables the task first) cannot complete
/// while the resident is in its first launch. That left the journal running
/// with its task disabled, so it never started again at sign-in or after a
/// reboot (measured on Windows 11: 3 of 154 forward-update runs, each ending
/// within 2.5 s of a five-minute boundary).
pub fn windows_service_update_plan(
    running_instances: usize,
    registration_current: bool,
    registration_enabled: bool,
) -> WindowsServiceUpdatePlan {
    let running = running_instances > 0;
    WindowsServiceUpdatePlan {
        stop_before_update: running && !registration_current,
        update_registration: !registration_current,
        enable_kept_registration: running && registration_current && !registration_enabled,
        start_after_update: running,
    }
}

/// The scheduler steps [`windows_update_present_registration`] drives, each
/// carrying its own verification. `S` is a registration readback.
pub struct WindowsRegistrationUpdateSteps<'a, S, E> {
    /// Set `Enabled` on the registration and return its readback.
    pub enable: &'a mut dyn FnMut(&S) -> Result<S, E>,
    /// Stop the running registration, disabling it first.
    pub stop: &'a mut dyn FnMut() -> Result<(), E>,
    /// Put back the `Enabled` flag the stop removed. Best effort: the failure
    /// that called for it is what the caller reports.
    pub restore_enabled: &'a mut dyn FnMut(),
    /// Read the stopped registration back and prove only the stop changed it.
    pub read_stopped: &'a mut dyn FnMut() -> Result<S, E>,
    /// Write the new profile over `current` and return its readback.
    pub update: &'a mut dyn FnMut(&S) -> Result<S, E>,
}

/// Carries a present registration through `plan` and returns its readback,
/// or `None` when `before` still is it.
///
/// After the stop has disabled a running task, a failure that leaves the
/// resident running or unrestarted puts `Enabled` back, as the update quiesce
/// does after a failed stop: a stop that never completed, or an update the
/// scheduler refused against the stopped task. The recovery trigger then
/// brings the journal back within five minutes, under its old registration.
/// A stop that completed but found the registration changed by someone else
/// is different: `read_stopped` refuses it and the service stays stopped, as
/// its message says.
pub fn windows_update_present_registration<S, E>(
    plan: WindowsServiceUpdatePlan,
    before: &S,
    steps: WindowsRegistrationUpdateSteps<'_, S, E>,
) -> Result<Option<S>, E> {
    if !plan.update_registration {
        return if plan.enable_kept_registration {
            (steps.enable)(before).map(Some)
        } else {
            Ok(None)
        };
    }
    if !plan.stop_before_update {
        return (steps.update)(before).map(Some);
    }
    if let Err(error) = (steps.stop)() {
        (steps.restore_enabled)();
        return Err(error);
    }
    let stopped = (steps.read_stopped)()?;
    match (steps.update)(&stopped) {
        Ok(updated) => Ok(Some(updated)),
        Err(error) => {
            (steps.restore_enabled)();
            Err(error)
        }
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
    use std::cell::{Cell, RefCell};

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
            windows_service_update_plan(1, false, true),
            WindowsServiceUpdatePlan {
                stop_before_update: true,
                update_registration: true,
                enable_kept_registration: false,
                start_after_update: true,
            }
        );
    }

    #[test]
    fn a_running_registration_that_is_already_current_is_kept_running() {
        assert_eq!(
            windows_service_update_plan(1, true, true),
            WindowsServiceUpdatePlan {
                stop_before_update: false,
                update_registration: false,
                enable_kept_registration: false,
                start_after_update: true,
            }
        );
        assert!(windows_service_update_plan(1, true, false).enable_kept_registration);
    }

    #[test]
    fn a_deliberately_stopped_service_stays_stopped() {
        for enabled in [false, true] {
            // A stale profile is rewritten in place; a current one is left
            // alone, its Enabled flag (the owner's stop) included.
            assert_eq!(
                windows_service_update_plan(0, false, enabled),
                WindowsServiceUpdatePlan {
                    stop_before_update: false,
                    update_registration: true,
                    enable_kept_registration: false,
                    start_after_update: false,
                }
            );
            assert_eq!(
                windows_service_update_plan(0, true, enabled),
                WindowsServiceUpdatePlan {
                    stop_before_update: false,
                    update_registration: false,
                    enable_kept_registration: false,
                    start_after_update: false,
                }
            );
        }
    }

    #[test]
    fn the_stop_and_the_restart_are_one_decision_about_the_owners_intent() {
        // Reading them from two places is how a service comes back up that the
        // owner had stopped, or stays down after an update that stopped it.
        // A running registration is either stopped and rewritten, or kept as
        // it is; the scheduler accepts no update against a running task.
        for instances in 0..4 {
            for current in [false, true] {
                for enabled in [false, true] {
                    let plan = windows_service_update_plan(instances, current, enabled);
                    assert_eq!(plan.start_after_update, instances > 0);
                    assert_eq!(
                        plan.stop_before_update,
                        plan.start_after_update && plan.update_registration
                    );
                }
            }
        }
    }

    #[test]
    fn more_than_one_instance_still_reads_as_running() {
        // The stop itself refuses to guess which of several instances it owns;
        // the plan's job is only to say that a stop has to happen first.
        assert!(windows_service_update_plan(2, false, true).stop_before_update);
        assert!(windows_service_update_plan(2, true, true).start_after_update);
    }

    /// A scheduler task as the post-update step sees it. Its resident never
    /// acknowledges a stop: it is in its first launch after the update.
    #[derive(Debug)]
    struct Task {
        enabled: bool,
        running: bool,
        current: bool,
        /// Whether the scheduler refuses an update against the stopped task.
        refuses_update: bool,
        log: Vec<String>,
    }

    impl Task {
        fn new(enabled: bool, current: bool) -> RefCell<Self> {
            RefCell::new(Self {
                enabled,
                running: true,
                current,
                refuses_update: false,
                log: Vec::new(),
            })
        }
    }

    const STOP_ELAPSED: &str = "service stop deadline elapsed; cleanup remains unverified";

    /// Drive `task` through the real plan and sequencing, with a stop that
    /// behaves as `stop_task(ctx, true)` did in the failure: it disables the
    /// task first, then fails (`stop_completes` false) or completes.
    fn drive(
        task: &RefCell<Task>,
        stop_completes: bool,
    ) -> Result<Option<&'static str>, &'static str> {
        let plan = {
            let task = task.borrow();
            windows_service_update_plan(usize::from(task.running), task.current, task.enabled)
        };
        windows_update_present_registration(
            plan,
            &"before",
            WindowsRegistrationUpdateSteps {
                enable: &mut |readback| {
                    let mut task = task.borrow_mut();
                    task.log.push(format!("enable {readback}"));
                    task.enabled = true;
                    Ok("enabled")
                },
                stop: &mut || {
                    let mut task = task.borrow_mut();
                    task.log.push("stop".into());
                    task.enabled = false;
                    if stop_completes {
                        task.running = false;
                        Ok(())
                    } else {
                        Err(STOP_ELAPSED)
                    }
                },
                restore_enabled: &mut || {
                    let mut task = task.borrow_mut();
                    task.log.push("restore".into());
                    task.enabled = true;
                },
                read_stopped: &mut || {
                    task.borrow_mut().log.push("read-stopped".into());
                    Ok("stopped")
                },
                update: &mut |readback| {
                    let mut task = task.borrow_mut();
                    task.log.push(format!("update {readback}"));
                    if task.running || task.refuses_update {
                        return Err("task-not-idle-before-update");
                    }
                    task.current = true;
                    Ok("updated")
                },
            },
        )
    }

    #[test]
    fn a_recovery_trigger_inside_the_resume_window_leaves_the_task_enabled_and_running() {
        // The trigger started the new build before the post-update step read
        // the task: running, enabled, and already carrying this profile.
        let task = Task::new(true, true);
        assert_eq!(drive(&task, false), Ok(None));
        let task = task.into_inner();
        assert!(task.log.is_empty(), "nothing is written: {:?}", task.log);
        assert!(task.enabled && task.running);
    }

    #[test]
    fn a_running_task_left_disabled_is_enabled_again_by_the_next_update() {
        let task = Task::new(false, true);
        assert_eq!(drive(&task, false), Ok(Some("enabled")));
        let task = task.into_inner();
        assert_eq!(task.log, ["enable before"]);
        assert!(task.enabled && task.running);
    }

    #[test]
    fn a_stop_that_never_completes_puts_enabled_back_when_the_profile_changes() {
        // The same race against a registration the update does have to
        // rewrite: the stop is still needed, and still cannot finish.
        let task = Task::new(true, false);
        assert_eq!(drive(&task, false), Err(STOP_ELAPSED));
        let task = task.into_inner();
        assert_eq!(task.log, ["stop", "restore"]);
        assert!(task.enabled && task.running, "{task:?}");
    }

    #[test]
    fn a_completed_stop_updates_the_stopped_readback() {
        let task = Task::new(true, false);
        assert_eq!(drive(&task, true), Ok(Some("updated")));
        let task = task.into_inner();
        assert_eq!(task.log, ["stop", "read-stopped", "update stopped"]);
        // The update writes the owner's run intent; the start follows it.
        assert!(task.current && !task.running);
    }

    #[test]
    fn an_update_refused_after_a_completed_stop_puts_enabled_back() {
        let task = Task::new(true, false);
        task.borrow_mut().refuses_update = true;
        assert_eq!(drive(&task, true), Err("task-not-idle-before-update"));
        let task = task.into_inner();
        assert_eq!(
            task.log,
            ["stop", "read-stopped", "update stopped", "restore"]
        );
        assert!(
            task.enabled,
            "the recovery trigger can bring it back: {task:?}"
        );
    }

    #[test]
    fn a_registration_changed_during_the_stop_stays_stopped() {
        // Only a failed stop or a refused update restores. A readback that
        // finds someone else changed the registration stops the update, and
        // the service stays stopped, as its message tells the owner.
        let restored = Cell::new(false);
        let order = RefCell::new(Vec::new());
        let result: Result<Option<()>, &str> = windows_update_present_registration(
            windows_service_update_plan(1, false, true),
            &(),
            WindowsRegistrationUpdateSteps {
                enable: &mut |_| unreachable!("a stale profile is rewritten"),
                stop: &mut || {
                    order.borrow_mut().push("stop");
                    Ok(())
                },
                restore_enabled: &mut || restored.set(true),
                read_stopped: &mut || {
                    order.borrow_mut().push("read-stopped");
                    Err("registration changed first")
                },
                update: &mut |_| unreachable!("a refused readback stops the update"),
            },
        );
        assert_eq!(result, Err("registration changed first"));
        assert_eq!(*order.borrow(), ["stop", "read-stopped"]);
        assert!(!restored.get());
    }
}
