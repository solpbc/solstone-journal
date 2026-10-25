// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Progress lines for the setup readiness battery, written to stderr.
//!
//! Setup stops readiness only after a stretch in which doctor writes nothing,
//! so a check that is slow but moving, such as hashing the signed speech model
//! on a slow disk, finishes, and a check that stops is named in what setup
//! keeps. The report on stdout is unchanged.

use std::io::Write;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use solstone_core_distribution::windows_payload::{HashProgressScope, report_hash_progress};
use solstone_core_setup::steps::DOCTOR_PROGRESS_PREFIX;

/// Well inside setup's shortest no-output bound.
const HASH_REPORT_INTERVAL: Duration = Duration::from_secs(5);
/// A member this large is named as soon as its hash starts.
const LARGE_MEMBER_BYTES: u64 = 64 * 1024 * 1024;

struct Clock {
    started: Instant,
    last_hash_report: Instant,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

/// Reporting for one readiness run; it ends when the value drops.
pub(crate) struct Reporting {
    _hashes: HashProgressScope,
}

pub(crate) fn start() -> Reporting {
    let now = Instant::now();
    *clock() = Some(Clock {
        started: now,
        last_hash_report: now,
    });
    Reporting {
        _hashes: report_hash_progress(hashing),
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        *clock() = None;
    }
}

pub(crate) fn checking(name: &str) {
    let Some(elapsed) = clock().as_ref().map(|clock| clock.started.elapsed()) else {
        return;
    };
    line(elapsed, format_args!("checking {name}"));
}

fn hashing(member: &str, hashed: u64, total: u64) {
    let elapsed = {
        let mut clock = clock();
        let Some(clock) = clock.as_mut() else {
            return;
        };
        let now = Instant::now();
        let starting_large = hashed == 0 && total >= LARGE_MEMBER_BYTES;
        if !starting_large && now.duration_since(clock.last_hash_report) < HASH_REPORT_INTERVAL {
            return;
        }
        clock.last_hash_report = now;
        now.duration_since(clock.started)
    };
    const MB: u64 = 1024 * 1024;
    line(
        elapsed,
        format_args!("verifying {member}: {} of {} MB", hashed / MB, total / MB),
    );
}

fn line(elapsed: Duration, text: std::fmt::Arguments<'_>) {
    // A closed stderr must not fail the check that is reporting.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{DOCTOR_PROGRESS_PREFIX}{:.1}s {text}",
        elapsed.as_secs_f64()
    );
}

fn clock() -> std::sync::MutexGuard<'static, Option<Clock>> {
    CLOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
