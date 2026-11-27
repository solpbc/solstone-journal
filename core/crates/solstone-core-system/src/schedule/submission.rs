// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::request::ScheduledRequest;

pub(crate) fn schedule_reference(reference: &str) -> Option<(&str, &str)> {
    reference.strip_prefix("sched:")?.rsplit_once(':')
}

impl crate::queue::TaskQueue {
    /// Whether this named clock schedule already has pending, queued, active or held work.
    pub fn has_active_schedule(&self, name: &str) -> bool {
        self.any_reference(|reference| {
            schedule_reference(reference).is_some_and(|(entry, _)| entry == name)
        })
    }
}

/// Caller-owned destination for scheduler-created work.
///
/// `false` has the same meaning as Python's failed Callosum emission.
pub trait ScheduleSubmissionSink: Send + Sync {
    /// Include pending, queued, running and safety-held work across host restarts.
    fn has_active_schedule(&self, _name: &str) -> bool {
        false
    }

    fn submit(&self, request: ScheduledRequest) -> bool;
}
