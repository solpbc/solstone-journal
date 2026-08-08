// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::request::ScheduledRequest;

/// Caller-owned destination for scheduler-created work.
///
/// `false` has the same meaning as Python's failed Callosum emission.
pub trait ScheduleSubmissionSink: Send + Sync {
    fn submit(&self, request: ScheduledRequest) -> bool;
}
