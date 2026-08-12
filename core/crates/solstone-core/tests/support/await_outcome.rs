// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::future::Future;
use std::time::{Duration, Instant};

// Idle 1000-sample maxima were 1.0640x for 5ms and 1.0326x for 10ms sleeps;
// the shared 1.1000x threshold leaves headroom for the noisier interval.
const DILATION_NUMERATOR: u128 = 11;
const DILATION_DENOMINATOR: u128 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitPolarity {
    Positive,
    Negative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PollState {
    Pending,
    Held,
    HardFail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitMetrics {
    pub(crate) requested: Duration,
    pub(crate) slept: Duration,
}

impl WaitMetrics {
    pub(crate) fn dilation(&self) -> f64 {
        if self.requested.is_zero() {
            return 0.0;
        }
        self.slept.as_secs_f64() / self.requested.as_secs_f64()
    }

    pub(crate) fn describe(&self) -> String {
        format!(
            "elapsed {:?}, requested {:?}, dilation {:.2}x",
            self.slept,
            self.requested,
            self.dilation()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitOutcome {
    Passed(WaitMetrics),
    Failed {
        reason: String,
        metrics: WaitMetrics,
    },
    Inconclusive(WaitMetrics),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CargoTestEvidence {
    RanWithoutParseableOutcome { target: String },
    NoTestBinaryEvidence,
}

pub(crate) fn await_outcome<Now, Check, Sleep>(
    polarity: WaitPolarity,
    interval: Duration,
    iterations: usize,
    now: Now,
    check: Check,
    sleep: Sleep,
) -> WaitOutcome
where
    Now: FnMut() -> Instant,
    Check: FnMut() -> PollState,
    Sleep: FnMut(Duration),
{
    let mut tracker = WaitTracker::new(polarity, interval, iterations);
    let mut now = now;
    let mut check = check;
    let mut sleep = sleep;

    for _ in 0..iterations {
        if let Some(outcome) = tracker.observe(check()) {
            return outcome;
        }
        tracker.measure_sleep(&mut now, || sleep(interval));
    }

    tracker.exhausted()
}

pub(crate) async fn await_outcome_async<Now, Check, Sleep, SleepFuture>(
    polarity: WaitPolarity,
    interval: Duration,
    iterations: usize,
    now: Now,
    check: Check,
    sleep: Sleep,
) -> WaitOutcome
where
    Now: FnMut() -> Instant,
    Check: FnMut() -> PollState,
    Sleep: FnMut(Duration) -> SleepFuture,
    SleepFuture: Future<Output = ()>,
{
    let mut tracker = WaitTracker::new(polarity, interval, iterations);
    let mut now = now;
    let mut check = check;
    let mut sleep = sleep;

    for _ in 0..iterations {
        if let Some(outcome) = tracker.observe(check()) {
            return outcome;
        }
        let before = now();
        sleep(interval).await;
        tracker.record_sleep(now().saturating_duration_since(before));
    }

    tracker.exhausted()
}

pub(crate) fn cargo_test_abort_discriminator(output: &str) -> CargoTestEvidence {
    const PREFIX: &str = "error: test failed, to rerun pass \x60";

    output
        .lines()
        .find_map(|line| {
            line.strip_prefix(PREFIX)
                .and_then(|target| target.strip_suffix('\x60'))
                .filter(|target| rerun_target_is_test_binary(target))
                .map(str::to_owned)
        })
        .map(|target| CargoTestEvidence::RanWithoutParseableOutcome { target })
        .unwrap_or(CargoTestEvidence::NoTestBinaryEvidence)
}

fn rerun_target_is_test_binary(target: &str) -> bool {
    let words = target.split_whitespace().collect::<Vec<_>>();
    matches!(words.as_slice(), ["-p", package, "--test", test] if !package.is_empty() && !test.is_empty())
}

struct WaitTracker {
    polarity: WaitPolarity,
    interval: Duration,
    requested: Duration,
    slept: Duration,
}

impl WaitTracker {
    fn new(polarity: WaitPolarity, interval: Duration, iterations: usize) -> Self {
        assert!(
            iterations > 0,
            "await_outcome requires a nonzero iteration budget"
        );
        Self {
            polarity,
            interval,
            requested: Duration::ZERO,
            slept: Duration::ZERO,
        }
    }

    fn observe(&self, state: PollState) -> Option<WaitOutcome> {
        match (self.polarity, state) {
            (_, PollState::HardFail(reason)) => Some(self.failed(reason)),
            (WaitPolarity::Positive, PollState::Held) => Some(WaitOutcome::Passed(self.metrics())),
            (WaitPolarity::Positive, PollState::Pending) => None,
            (WaitPolarity::Negative, PollState::Held) => None,
            (WaitPolarity::Negative, PollState::Pending) => {
                Some(self.failed("negative wait check returned Pending".to_owned()))
            }
        }
    }

    fn measure_sleep<Now, Sleep>(&mut self, now: &mut Now, sleep: Sleep)
    where
        Now: FnMut() -> Instant,
        Sleep: FnOnce(),
    {
        let before = now();
        sleep();
        self.record_sleep(now().saturating_duration_since(before));
    }

    fn record_sleep(&mut self, elapsed: Duration) {
        self.requested = self.requested.saturating_add(self.interval);
        self.slept = self.slept.saturating_add(elapsed);
    }

    fn exhausted(self) -> WaitOutcome {
        match self.polarity {
            WaitPolarity::Positive if self.is_dilated() => {
                WaitOutcome::Inconclusive(self.metrics())
            }
            WaitPolarity::Positive => {
                self.failed("positive wait exhausted before condition held".to_owned())
            }
            WaitPolarity::Negative if self.is_dilated() => {
                WaitOutcome::Inconclusive(self.metrics())
            }
            WaitPolarity::Negative => WaitOutcome::Passed(self.metrics()),
        }
    }

    fn is_dilated(&self) -> bool {
        self.slept.as_nanos().saturating_mul(DILATION_DENOMINATOR)
            >= self.requested.as_nanos().saturating_mul(DILATION_NUMERATOR)
    }

    fn failed(&self, reason: String) -> WaitOutcome {
        WaitOutcome::Failed {
            reason,
            metrics: self.metrics(),
        }
    }

    fn metrics(&self) -> WaitMetrics {
        WaitMetrics {
            requested: self.requested,
            slept: self.slept,
        }
    }
}
