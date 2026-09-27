// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::time::{Duration, Instant};

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static TEST_NOW: Cell<Option<Duration>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Origin {
    Real(Instant),
    #[cfg(test)]
    Fake(Duration),
}

pub(crate) fn sample() -> Origin {
    #[cfg(test)]
    if let Some(now) = TEST_NOW.with(Cell::get) {
        return Origin::Fake(now);
    }
    Origin::Real(Instant::now())
}

pub(crate) fn since(origin: &Origin) -> Duration {
    match origin {
        Origin::Real(instant) => instant.elapsed(),
        #[cfg(test)]
        Origin::Fake(base) => {
            let now = TEST_NOW
                .with(Cell::get)
                .expect("fake clock uninstalled while run still in flight");
            now.saturating_sub(*base)
        }
    }
}

#[cfg(test)]
pub(crate) struct ClockGuard;

#[cfg(test)]
impl Drop for ClockGuard {
    fn drop(&mut self) {
        TEST_NOW.with(|cell| cell.set(None));
    }
}

#[cfg(test)]
pub(crate) fn install_test_clock(now: Duration) -> ClockGuard {
    TEST_NOW.with(|cell| cell.set(Some(now)));
    ClockGuard
}

#[cfg(test)]
pub(crate) fn set_test_clock(now: Duration) {
    TEST_NOW.with(|cell| cell.set(Some(now)));
}
