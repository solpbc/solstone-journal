// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Clock abstractions shared by lifecycle admission and heartbeat collection.

/// Time and sleep dependencies used by the bounded Unix admission wait.
///
/// The monotonic deadline makes the one wait independent of wall-clock changes.
pub trait AdmissionWaitClock {
    fn wall_seconds(&mut self) -> f64;
    fn monotonic_seconds(&mut self) -> f64;
    fn sleep_until(&mut self, deadline_seconds: f64);
}

pub(crate) fn wall_clock_discontinuous(
    first_wall_seconds: f64,
    second_wall_seconds: f64,
    first_monotonic_seconds: f64,
    second_monotonic_seconds: f64,
) -> bool {
    let wall_elapsed = second_wall_seconds - first_wall_seconds;
    let monotonic_elapsed = second_monotonic_seconds - first_monotonic_seconds;
    wall_elapsed.is_sign_negative()
        || monotonic_elapsed.is_sign_negative()
        || wall_elapsed > monotonic_elapsed + super::sync::DEFAULT_INTERVAL_SECONDS
}
