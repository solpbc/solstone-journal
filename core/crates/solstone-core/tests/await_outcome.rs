// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::await_outcome::{
    CargoTestEvidence, PollState, WaitMetrics, WaitOutcome, WaitPolarity, await_outcome,
    await_outcome_async, cargo_test_abort_discriminator,
};

const INTERVAL: Duration = Duration::from_micros(1_000);

#[derive(Clone)]
struct FakeClock {
    now: Rc<Cell<Instant>>,
}

impl FakeClock {
    fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(Instant::now())),
        }
    }

    fn read(&self) -> Instant {
        self.now.get()
    }

    fn advance(&self, duration: Duration) {
        self.now.set(self.now.get() + duration);
    }
}

fn sequence(states: impl IntoIterator<Item = PollState>) -> Rc<RefCell<VecDeque<PollState>>> {
    Rc::new(RefCell::new(states.into_iter().collect()))
}

fn next_state(states: &Rc<RefCell<VecDeque<PollState>>>) -> PollState {
    states
        .borrow_mut()
        .pop_front()
        .expect("test supplied enough poll states")
}

fn metrics(outcome: &WaitOutcome) -> &WaitMetrics {
    match outcome {
        WaitOutcome::Passed(metrics) | WaitOutcome::Inconclusive(metrics) => metrics,
        WaitOutcome::Failed { metrics, .. } => metrics,
    }
}

fn run_sync(
    polarity: WaitPolarity,
    states: impl IntoIterator<Item = PollState>,
    iterations: usize,
    sleep_elapsed: Duration,
) -> WaitOutcome {
    let clock = FakeClock::new();
    let states = sequence(states);
    let now_clock = clock.clone();
    let sleep_clock = clock.clone();
    await_outcome(
        polarity,
        INTERVAL,
        iterations,
        move || now_clock.read(),
        move || next_state(&states),
        move |_| sleep_clock.advance(sleep_elapsed),
    )
}

#[test]
fn truth_table_positive_and_negative_rows() {
    assert!(matches!(
        run_sync(WaitPolarity::Positive, [PollState::Held], 1, Duration::ZERO),
        WaitOutcome::Passed(_)
    ));
    assert!(matches!(
        run_sync(
            WaitPolarity::Positive,
            [PollState::Pending],
            1,
            Duration::from_micros(1_099),
        ),
        WaitOutcome::Failed { .. }
    ));
    assert!(matches!(
        run_sync(
            WaitPolarity::Positive,
            [PollState::Pending],
            1,
            Duration::from_micros(1_101),
        ),
        WaitOutcome::Inconclusive(_)
    ));
    assert!(matches!(
        run_sync(
            WaitPolarity::Negative,
            [PollState::Held],
            1,
            Duration::from_micros(1_099),
        ),
        WaitOutcome::Passed(_)
    ));
    assert!(matches!(
        run_sync(
            WaitPolarity::Negative,
            [PollState::Held],
            1,
            Duration::from_micros(1_101),
        ),
        WaitOutcome::Inconclusive(_)
    ));
    assert!(matches!(
        run_sync(
            WaitPolarity::Negative,
            [PollState::HardFail("marker appeared".to_owned())],
            1,
            Duration::from_micros(1_101),
        ),
        WaitOutcome::Failed { .. }
    ));
}

#[test]
fn hard_fail_precedes_a_dilated_watchdog_expiry() {
    let outcome = run_sync(
        WaitPolarity::Positive,
        [
            PollState::Pending,
            PollState::HardFail("child exited early".to_owned()),
        ],
        2,
        Duration::from_micros(1_101),
    );
    assert!(matches!(
        outcome,
        WaitOutcome::Failed { ref reason, .. } if reason == "child exited early"
    ));
}

#[test]
fn negative_pending_is_a_failure() {
    assert!(matches!(
        run_sync(
            WaitPolarity::Negative,
            [PollState::Pending],
            1,
            Duration::ZERO,
        ),
        WaitOutcome::Failed { ref reason, .. } if reason == "negative wait check returned Pending"
    ));
}

#[test]
fn dilation_boundary_is_exact_and_metrics_are_real() {
    for (elapsed, expected_inconclusive) in [
        (Duration::from_micros(1_099), false),
        (Duration::from_micros(1_100), true),
        (Duration::from_micros(1_101), true),
    ] {
        let outcome = run_sync(WaitPolarity::Positive, [PollState::Pending], 1, elapsed);
        assert_eq!(metrics(&outcome).requested, INTERVAL);
        assert_eq!(metrics(&outcome).slept, elapsed);
        assert_eq!(
            matches!(outcome, WaitOutcome::Inconclusive(_)),
            expected_inconclusive,
            "elapsed {elapsed:?}"
        );
    }
}

#[test]
fn inconclusive_outcome_describes_elapsed_requested_and_dilation() {
    let clock = FakeClock::new();
    let now_clock = clock.clone();
    let sleep_clock = clock.clone();
    let outcome = await_outcome(
        WaitPolarity::Positive,
        Duration::from_millis(100),
        20,
        move || now_clock.read(),
        || PollState::Pending,
        move |_| sleep_clock.advance(Duration::from_millis(150)),
    );
    let WaitOutcome::Inconclusive(metrics) = outcome else {
        panic!("20 pending 100ms waits with 3s elapsed must be inconclusive");
    };
    assert_eq!(metrics.requested, Duration::from_secs(2));
    assert_eq!(metrics.slept, Duration::from_secs(3));
    assert!((metrics.dilation() - 1.5).abs() < f64::EPSILON);

    let description = metrics.describe();
    assert!(description.contains("elapsed"));
    assert!(description.contains("requested"));
    assert!(description.contains("1.50x"));
}

#[test]
fn sleep_measurement_excludes_condition_work() {
    let clock = FakeClock::new();
    let now_clock = clock.clone();
    let check_clock = clock.clone();
    let sleep_clock = clock.clone();
    let outcome = await_outcome(
        WaitPolarity::Positive,
        INTERVAL,
        3,
        move || now_clock.read(),
        move || {
            check_clock.advance(Duration::from_millis(100));
            PollState::Pending
        },
        move |interval| sleep_clock.advance(interval),
    );
    assert!(matches!(outcome, WaitOutcome::Failed { .. }));
    assert_eq!(metrics(&outcome).requested, Duration::from_micros(3_000));
    assert_eq!(metrics(&outcome).slept, Duration::from_micros(3_000));
}

#[test]
fn positive_held_ignores_prior_high_dilation() {
    let outcome = run_sync(
        WaitPolarity::Positive,
        [PollState::Pending, PollState::Held],
        2,
        Duration::from_millis(20),
    );
    assert!(matches!(outcome, WaitOutcome::Passed(_)));
}

#[tokio::test]
async fn async_wait_uses_the_same_classifier() {
    let clock = FakeClock::new();
    let now_clock = clock.clone();
    let check_clock = clock.clone();
    let sleep_clock = clock.clone();
    let outcome = await_outcome_async(
        WaitPolarity::Negative,
        INTERVAL,
        1,
        move || now_clock.read(),
        move || {
            check_clock.advance(Duration::from_millis(50));
            PollState::Held
        },
        move |interval| {
            sleep_clock.advance(interval);
            std::future::ready(())
        },
    )
    .await;
    assert!(matches!(outcome, WaitOutcome::Passed(_)));
    assert_eq!(metrics(&outcome).slept, INTERVAL);
}

fn panic_or_log_termination(outcome: &WaitOutcome) {
    if matches!(outcome, WaitOutcome::Passed(_)) {
        return;
    }
    if std::thread::panicking() {
        eprintln!("suppressed termination failure while unwinding: {outcome:?}");
        return;
    }
    panic!("termination wait failed: {outcome:?}");
}

fn watchdog_expiry() -> WaitOutcome {
    WaitOutcome::Inconclusive(WaitMetrics {
        requested: INTERVAL,
        slept: Duration::from_millis(20),
    })
}

#[test]
fn termination_wrapper_panics_outside_unwinding() {
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            panic_or_log_termination(&watchdog_expiry());
        }))
        .is_err()
    );
}

struct UnwindingDropProbe;

impl Drop for UnwindingDropProbe {
    fn drop(&mut self) {
        panic_or_log_termination(&watchdog_expiry());
    }
}

#[test]
fn termination_wrapper_suppresses_a_second_panic_during_drop() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _probe = UnwindingDropProbe;
        panic!("outer panic");
    }))
    .expect_err("outer panic must survive");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("outer panic"));
}

#[test]
fn cargo_abort_discriminator_extracts_the_fixture_target() {
    let fixture = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/cargo-test-sigkill-supervisor-tick.txt"
    ));
    assert_eq!(
        cargo_test_abort_discriminator(fixture),
        CargoTestEvidence::RanWithoutParseableOutcome {
            target: "-p solstone-core --test supervisor_tick".to_owned(),
        }
    );
}

#[test]
fn cargo_abort_discriminator_requires_cargo_test_footer() {
    assert_eq!(
        cargo_test_abort_discriminator("test process ended unexpectedly"),
        CargoTestEvidence::NoTestBinaryEvidence
    );
}
